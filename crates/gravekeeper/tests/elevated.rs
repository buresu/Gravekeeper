//! Runs the real `gravekeeper elevated` helper without elevation, with requests that change
//! nothing, to check its command line and report format.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_gravekeeper");

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn report(request: &str) -> String {
    let out = Command::new(BIN)
        .args(["elevated", request])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn an_empty_request_succeeds() {
    assert_eq!(report(&hex("[]")), r#"{"Ok":null}"#);
}

#[test]
fn invalid_requests_are_reported_not_run() {
    assert!(report("not-hex").starts_with(r#"{"Err":"invalid request"#));
    let unknown = hex(r#"[{"task":"run-command","program":"sh"}]"#);
    assert!(report(&unknown).starts_with(r#"{"Err":"invalid request"#));
}
