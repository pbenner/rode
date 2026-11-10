//! Real-process regressions for bounded capture ownership and independent limits.
use super::*;
use std::{fs, path::Path, process::Child, time::Instant};

struct KillChild(Child);
impl Drop for KillChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Descendant(u32);
impl Drop for Descendant {
    fn drop(&mut self) {
        // This fixture PID belongs to the harmless sleep started below. Bound
        // cleanup even when testing a runner that leaves descendants alive.
        if running(self.0) {
            unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
        }
    }
}

fn running(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|text| {
            text.rsplit_once(") ")
                .map(|(_, state)| !state.starts_with('Z'))
        })
        .unwrap_or(false)
}

fn fixture(pid: &Path, timeout: bool, hold_output: bool) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "if [ \"$3\" = hold ]; then /bin/sleep 30 </dev/null & else /bin/sleep 30 </dev/null >/dev/null 2>&1 & fi; printf '%s\\n' \"$!\" > \"$1\"; if [ \"$2\" = timeout ]; then exec /bin/sleep 30; fi; printf done",
        "capture-fixture",
    ]);
    command
        .arg(pid)
        .arg(if timeout { "timeout" } else { "exit" })
        .arg(if hold_output { "hold" } else { "close" });
    command
}

fn descendants_end_with_capture(project: bool, timeout: bool, hold_output: bool) {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("descendant.pid");
    let command = fixture(&pid_file, timeout, hold_output);
    let mut independent = KillChild(Command::new("/bin/sleep").arg("30").spawn().unwrap());
    let deadline = Duration::from_millis(500);
    let result = if project {
        crate::project::run(command, vec![], deadline).map(|output| {
            assert!(output.success());
            output.stdout.into_bytes()
        })
    } else {
        run_bounded(command, vec![], deadline)
    };
    let descendant = Descendant(
        fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    if timeout {
        assert!(result.unwrap_err().to_string().contains("timed out"));
    } else {
        assert_eq!(result.unwrap(), b"done");
    }
    let until = Instant::now() + Duration::from_secs(1);
    while running(descendant.0) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        independent.0.try_wait().unwrap().is_none(),
        "An unrelated command was stopped"
    );
    assert!(
        !running(descendant.0),
        "A completed capture left its descendant running"
    );
}

#[test]
fn helper_timeout_stops_descendants_without_stopping_independent_work() {
    descendants_end_with_capture(false, true, false);
}

#[test]
fn successful_helper_exit_stops_descendants_even_after_they_close_output() {
    descendants_end_with_capture(false, false, false);
}

#[test]
fn successful_project_exit_stops_descendants_even_after_they_close_output() {
    descendants_end_with_capture(true, false, false);
}

#[test]
fn successful_exit_closes_pipes_inherited_by_background_descendants() {
    descendants_end_with_capture(false, false, true);
}

#[test]
fn successful_helpers_cannot_silently_exceed_the_stderr_budget() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "head -c 65537 /dev/zero >&2"]);
    assert!(run_bounded_limited(command, vec![], Duration::from_secs(5), 8).is_err());
}
