//! Diagnostics for shared project-host command results.
use super::Output;

#[test]
fn checked_output_preserves_success_and_reports_silent_failure_status() {
    assert_eq!(
        Output {
            stdout: "  exact output\n".into(),
            ..Default::default()
        }
        .checked()
        .unwrap(),
        "  exact output\n"
    );
    for (code, expected) in [
        (7, "Command exited with status 7"),
        (-1, "Command terminated without an exit code"),
    ] {
        let error = Output {
            code,
            ..Default::default()
        }
        .checked()
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
    let error = Output {
        code: 1,
        stderr: "  stderr detail\n".into(),
        stdout: "stdout detail\n".into(),
    }
    .checked()
    .unwrap_err();
    assert_eq!(error.to_string(), "stderr detail\nstdout detail");
}
