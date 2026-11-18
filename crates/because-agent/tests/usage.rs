//! The usage endpoint's reply, parsed.
//!
//! Its shape is not a published contract, so these pin the behaviour that
//! matters when it changes: known windows are read, unknown ones are carried
//! rather than dropped, and anything unrecognisable is an error and never a
//! number that looks right.

use because_agent::claude::usage::parse;

#[test]
fn reads_the_windows_shortest_first() {
    let u = parse(
        r#"{
            "seven_day": {"utilization": 41, "resets_at": "2026-10-06T00:00:00Z"},
            "five_hour": {"utilization": 28, "resets_at": "2026-10-02T18:00:00Z"},
            "seven_day_opus": {"utilization": 3}
        }"#,
    )
    .expect("parses");
    let keys: Vec<&str> = u.windows.iter().map(|w| w.key.as_str()).collect();
    assert_eq!(keys, ["five_hour", "seven_day", "seven_day_opus"]);
    let labels: Vec<&str> = u.windows.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(labels, ["5h", "week", "opus"]);
    assert_eq!(u.windows[0].used_pct, 28.0);
    assert_eq!(u.windows[0].left_pct(), 72.0);
    assert_eq!(u.windows[0].resets_at, "2026-10-02T18:00:00Z");
    // Absent rather than guessed.
    assert_eq!(u.windows[2].resets_at, "");
}

/// A window nobody here has heard of still gets a bar: the account's limits are
/// the API's to change, and dropping one would understate what is being spent.
#[test]
fn keeps_windows_it_does_not_know() {
    let u = parse(r#"{"thirty_day_sonnet": {"utilization": 12.5}}"#).expect("parses");
    assert_eq!(u.windows.len(), 1);
    assert_eq!(u.windows[0].label, "thirty");
    assert_eq!(u.windows[0].used_pct, 12.5);
}

/// Fields that are not windows are not windows.
#[test]
fn ignores_everything_without_a_utilization() {
    let u = parse(r#"{"subscription": "max", "five_hour": {"utilization": 0}}"#).expect("parses");
    assert_eq!(u.windows.len(), 1);
    assert_eq!(u.windows[0].left_pct(), 100.0);
}

#[test]
fn an_error_reply_is_an_error() {
    let e = parse(r#"{"error": {"type": "authentication_error", "message": "invalid bearer token"}}"#)
        .expect_err("should not parse");
    assert!(e.to_string().contains("invalid bearer token"), "{e}");
}

#[test]
fn html_or_empty_is_an_error_not_an_empty_bar() {
    for body in ["", "<html>502 Bad Gateway</html>", "{}", "[]"] {
        assert!(parse(body).is_err(), "{body:?} should not parse as usage");
    }
}
