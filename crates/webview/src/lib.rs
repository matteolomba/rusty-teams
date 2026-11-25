#[cfg(any(windows, test))]
mod cdp_failure;
#[cfg(windows)]
mod dialog;
#[cfg(windows)]
mod embed;
#[cfg(windows)]
mod fanout;
mod geometry;
mod guards;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod host_dialog;
#[cfg(windows)]
mod main_embed;
#[cfg(any(windows, test))]
mod park;
mod sanitize;
#[cfg(any(windows, test))]
mod sleep;
#[cfg(windows)]
mod transport;

#[cfg(windows)]
pub use dialog::{DialogEvent, DialogHandle, DialogMode, DialogSpec, NEW_WINDOW_KIND};
#[cfg(windows)]
pub use embed::{EmbedEvent, EmbedSpec};
pub use geometry::{EmbedBounds, local_cutouts};
#[cfg(windows)]
pub use host::{HostConfig, start};
#[cfg(windows)]
pub use main_embed::{EmbedHost, MainEmbed};
#[cfg(windows)]
pub use transport::{HostState, WebViewTransport};

#[cfg(windows)]
pub(crate) const FORWARDED_EVENTS: [&str; 2] = ["Runtime.bindingCalled", "Page.frameNavigated"];

pub(crate) const BROWSER_ARGUMENTS: &str = concat!(
    "--disable-features=Translate,MediaRouter,OptimizationHints ",
    "--disable-default-apps --disable-gpu ",
    "--disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_arguments_keep_chromium_protections() {
        for flag in [
            "--disable-client-side-phishing-detection",
            "--disable-breakpad",
            "--disable-component-update",
            "--disable-background-networking",
        ] {
            assert!(!BROWSER_ARGUMENTS.contains(flag), "{flag} is still present");
        }
        assert!(BROWSER_ARGUMENTS.contains("--disable-gpu"));
    }
}
