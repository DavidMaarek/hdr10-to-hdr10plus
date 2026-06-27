use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[allow(deprecated)]
fn cmd() -> Command {
    Command::cargo_bin("mkvhdr10plus").expect("Failed to find mkvhdr10plus binary")
}

#[test]
fn test_missing_input_shows_usage() {
    cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}

#[test]
fn test_help_flag() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("HDR10+"));
}

#[test]
fn test_version_flag() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("mkvhdr10plus 0.1.0"));
}

#[test]
fn test_nonexistent_input_fails() {
    // --json-only skips the external-tool dependency check, so this exercises
    // the input-existence guard rather than a missing ffmpeg/mkvmerge.
    cmd()
        .arg("/nonexistent/path/to/video.mkv")
        .arg("--json-only")
        .assert()
        .failure()
        .stderr(predicate::str::contains("input file not found"));
}
