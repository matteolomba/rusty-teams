use std::mem::{ManuallyDrop, size_of};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    PROCESS_LOOPBACK_MODE, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IAgileObject, IAgileObject_Impl,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, Ref};
use windows_core::implement;

use crate::audio::SAMPLE_RATE;
use crate::capture::Capture;
use crate::error::{Error, Result};
use crate::pcm::SampleQueue;

const CHANNELS: u16 = 2;
const BITS: u16 = 16;
const ACTIVATION_WAIT: Duration = Duration::from_secs(5);
const EVENT_WAIT_MS: u32 = 100;
const PCM_TAG: u16 = 1;

struct ActivatedClient(IAudioClient);

unsafe impl Send for ActivatedClient {}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct ActivationHandler {
    sender: mpsc::Sender<windows::core::Result<ActivatedClient>>,
}

impl IAgileObject_Impl for ActivationHandler_Impl {}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        // SAFETY: the completion handler runs with COM initialised; `operation` and both out-params
        // are valid for the call, and the resulting owned interface is sent through the channel.
        let activated = (|| unsafe {
            let operation = operation.ok()?;
            let mut result = windows::core::HRESULT(0);
            let mut unknown = None;
            operation.GetActivateResult(&mut result, &mut unknown)?;
            result.ok()?;
            let unknown: windows::core::IUnknown =
                unknown.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            unknown.cast::<IAudioClient>().map(ActivatedClient)
        })();
        let _ = self.sender.send(activated);
        Ok(())
    }
}

fn audio_error(what: &str, error: impl std::fmt::Display) -> Error {
    Error::Audio(format!("{what}: {error}"))
}

fn activate(process_id: u32, mode: PROCESS_LOOPBACK_MODE) -> Result<IAudioClient> {
    let mut parameters = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: process_id,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let mut variant = ManuallyDrop::new(PROPVARIANT::default());
    // SAFETY: `variant` is a live ManuallyDrop PROPVARIANT kept alive past the call below, and
    // `parameters` is a live stack value whose bytes the activation API copies; the BLOB is not
    // owned by COM.
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob = BLOB {
            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: (&raw mut parameters).cast(),
        };
    }
    let (sender, receiver) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler { sender }.into();
    // SAFETY: `variant` and `handler` are live for this call; the async operation holds the agile
    // handler (and its channel sender) until completion, which is awaited with a timeout.
    let _operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *variant),
            &handler,
        )
    }
    .map_err(|error| audio_error("activate process loopback", error))?;
    receiver
        .recv_timeout(ACTIVATION_WAIT)
        .map_err(|_| Error::Audio("process loopback activation timed out".into()))?
        .map(|client| client.0)
        .map_err(|error| audio_error("process loopback activation", error))
}

fn initialise(client: &IAudioClient) -> Result<(IAudioCaptureClient, HANDLE)> {
    let block_align = CHANNELS * BITS / 8;
    let format = WAVEFORMATEX {
        wFormatTag: PCM_TAG,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: BITS,
        cbSize: 0,
    };
    let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;
    // SAFETY: COM is initialised on this thread by the caller and `client` is live; `format` is a
    // live stack value passed by pointer only for Initialize, and the returned interfaces and
    // event handle are owned by the caller.
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                0,
                0,
                &raw const format,
                None,
            )
            .map_err(|error| audio_error("initialize loopback", error))?;
        let event =
            CreateEventW(None, false, false, None).map_err(|error| audio_error("event", error))?;
        client
            .SetEventHandle(event)
            .map_err(|error| audio_error("event handle", error))?;
        let capture = client
            .GetService::<IAudioCaptureClient>()
            .map_err(|error| audio_error("capture service", error))?;
        client
            .Start()
            .map_err(|error| audio_error("start loopback", error))?;
        Ok((capture, event))
    }
}

fn drain(capture: &IAudioCaptureClient, queue: &SampleQueue) -> windows::core::Result<()> {
    // SAFETY: `capture` is a live IAudioCaptureClient; the buffer pointer from GetBuffer is read
    // only until the matching ReleaseBuffer, and samples are copied before that release.
    unsafe {
        while capture.GetNextPacketSize()? > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let mono: Vec<i16> =
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    vec![0; frames as usize]
                } else {
                    let pairs = std::slice::from_raw_parts(
                        data.cast::<i16>(),
                        frames as usize * usize::from(CHANNELS),
                    );
                    pairs
                        .chunks_exact(usize::from(CHANNELS))
                        .map(|pair| ((i32::from(pair[0]) + i32::from(pair[1])) / 2) as i16)
                        .collect()
                };
            capture.ReleaseBuffer(frames)?;
            queue.push(&mono);
        }
    }
    Ok(())
}

pub fn start_process_loopback(queue: SampleQueue, own_process_id: u32) -> Result<Capture> {
    start(
        queue,
        own_process_id,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    )
}

/// Arbitrary-process capture is diagnostic-only and off unless explicitly enabled.
const PROCESS_CAPTURE_ENV: &str = "CALLING_PROCESS_CAPTURE";

fn process_capture_allowed(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}

/// Only the audio of `process_id` and its children, for checking what an app plays out.
pub fn start_process_capture(queue: SampleQueue, process_id: u32) -> Result<Capture> {
    if !process_capture_allowed(std::env::var(PROCESS_CAPTURE_ENV).ok().as_deref()) {
        return Err(audio_error(
            "process capture",
            "disabled; set CALLING_PROCESS_CAPTURE=1 to opt in",
        ));
    }
    start(
        queue,
        process_id,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    )
}

fn start(queue: SampleQueue, process_id: u32, mode: PROCESS_LOOPBACK_MODE) -> Result<Capture> {
    let (ready_sender, ready) = mpsc::channel::<Result<()>>();
    let capture = Capture::spawn_thread("process-loopback", move |stop| {
        // SAFETY: this is a freshly spawned thread with no COM initialised; the CoUninitialize at
        // the end of this closure pairs with this call.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let started = activate(process_id, mode).and_then(|client| {
            initialise(&client).map(|(capture, event)| (client, capture, event))
        });
        match started {
            Ok((client, capture, event)) => {
                let _ = ready_sender.send(Ok(()));
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    // SAFETY: `event` is the owned handle created in initialise and stays open
                    // until the cleanup after this loop.
                    let waited = unsafe { WaitForSingleObject(event, EVENT_WAIT_MS) };
                    if waited == WAIT_OBJECT_0 && drain(&capture, &queue).is_err() {
                        break;
                    }
                }
                // SAFETY: `client` and `event` are the owned values from initialise, not used after
                // this point; Stop and CloseHandle each run once while the values are still valid.
                unsafe {
                    let _ = client.Stop();
                    let _ = CloseHandle(event);
                }
            }
            Err(error) => {
                let _ = ready_sender.send(Err(error));
            }
        }
        // SAFETY: pairs with the CoInitializeEx at the start of this closure; every COM object
        // created in it has been dropped by the time this runs.
        unsafe { CoUninitialize() };
    })
    .map_err(|error| audio_error("loopback thread", error))?;
    ready
        .recv_timeout(crate::capture::CAPTURE_START_TIMEOUT)
        .map_err(|_| Error::Audio("loopback thread ended".into()))??;
    Ok(capture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_capture_is_opt_in() {
        assert!(!process_capture_allowed(None));
        assert!(!process_capture_allowed(Some("0")));
        assert!(process_capture_allowed(Some("1")));
        assert!(process_capture_allowed(Some("true")));
    }
}
