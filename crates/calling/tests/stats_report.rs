//! The stats report is parsed inside a C++ callback, where a panic aborts the app.

use libwebrtc::stats::{RtcStats, parse_stats};

#[test]
fn unknown_stats_types_are_skipped_not_fatal() {
    let report = r#"[
        {"type": "peer-connection", "id": "P", "timestamp": 1, "dataChannelsOpened": 2},
        {"type": "a-stats-type-from-a-newer-webrtc", "id": "X", "timestamp": 1}
    ]"#;
    // The old callback did `serde_json::from_str(..).unwrap()` on the whole list.
    assert!(serde_json::from_str::<Vec<RtcStats>>(report).is_err());
    let stats = parse_stats(report).unwrap();
    assert_eq!(stats.len(), 1);
    assert!(matches!(&stats[0], RtcStats::PeerConnection(pc) if pc.pc.data_channels_opened == 2));
}

#[test]
fn a_report_that_is_not_a_list_is_an_error() {
    assert!(parse_stats("{not json").is_err());
    assert!(parse_stats("").unwrap().is_empty());
}
