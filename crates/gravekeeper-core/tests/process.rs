//! Integration tests for process control, driven by the `gk-test-helper` binary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gravekeeper_core::process::{ExitKind, LogBuffer, ManagedChild, ProcessSpec, Stream};

const HELPER: &str = env!("CARGO_BIN_EXE_gk-test-helper");
const GRACE: Duration = Duration::from_secs(5);

fn helper(args: &[&str]) -> ProcessSpec {
    args.iter()
        .fold(ProcessSpec::new(HELPER), |spec, arg| spec.arg(*arg))
}

fn spawn(args: &[&str], capacity: usize) -> (ManagedChild, Arc<LogBuffer>) {
    let log = Arc::new(LogBuffer::new(capacity));
    let child = ManagedChild::spawn(&helper(args), Arc::clone(&log)).expect("spawn helper");
    (child, log)
}

#[tokio::test]
async fn captures_stdout_and_stderr_separately() {
    let (mut child, log) = spawn(&["lines", "3"], 100);
    assert_eq!(child.wait().await.unwrap(), ExitKind::Code(0));
    assert!(child.drain_output(GRACE).await);

    let lines = log.since(0);
    let of = |stream| {
        lines
            .iter()
            .filter(|l| l.stream == stream)
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(of(Stream::Stdout), ["out 0", "out 1", "out 2"]);
    assert_eq!(of(Stream::Stderr), ["err 0", "err 1", "err 2"]);
}

#[tokio::test]
async fn reports_exit_codes() {
    let (mut child, _) = spawn(&["exit", "3"], 10);
    let exit = child.wait().await.unwrap();
    assert_eq!(exit, ExitKind::Code(3));
    assert!(!exit.looks_like_crash());
}

#[tokio::test]
async fn abort_looks_like_a_crash() {
    let (mut child, _) = spawn(&["abort"], 10);
    let exit = child.wait().await.unwrap();
    assert!(exit.looks_like_crash(), "{exit:?}");
}

#[tokio::test]
async fn spawn_failure_is_an_error() {
    let log = Arc::new(LogBuffer::new(10));
    let spec = ProcessSpec::new("gravekeeper-definitely-missing-program");
    assert!(ManagedChild::spawn(&spec, log).is_err());
}

/// 64 MiB on each stream must flow through without the child blocking on a full pipe, and
/// the bounded buffer must keep only the newest lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heavy_output_does_not_block_the_child() {
    const BYTES: u64 = 64 * 1024 * 1024;
    const LINES_PER_STREAM: u64 = BYTES / 1024;
    let started = Instant::now();

    let (mut child, log) = spawn(&["flood", &BYTES.to_string()], 1000);
    let exit = tokio::time::timeout(Duration::from_secs(60), child.wait())
        .await
        .expect("child blocked on output")
        .unwrap();
    assert_eq!(exit, ExitKind::Code(0));
    assert!(child.drain_output(GRACE).await);

    assert_eq!(log.total(), LINES_PER_STREAM * 2);
    assert_eq!(log.since(0).len(), 1000);
    eprintln!("flood of 2 x {BYTES} bytes took {:?}", started.elapsed());
}

#[tokio::test]
async fn stop_terminates_a_running_process() {
    let (mut child, _) = spawn(&["sleep", "60"], 10);
    let started = Instant::now();
    let outcome = child.stop(GRACE).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!outcome.exit.is_success());
    // Unix: SIGTERM is honored. Windows: no graceful stop yet, so the job is killed.
    assert_eq!(outcome.forced, cfg!(windows));
}

#[tokio::test]
async fn stop_also_terminates_descendants() {
    let (mut child, log) = spawn(&["tree", "60"], 10);
    let grandchild = wait_for_first_line(&log).await.parse::<u32>().unwrap();
    assert!(is_alive(grandchild), "grandchild should be running");

    child.stop(GRACE).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while is_alive(grandchild) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !is_alive(grandchild),
        "grandchild {grandchild} survived stop"
    );
}

/// SIGTERM is ignored here (and inherited by the child), so stop must escalate to SIGKILL.
#[cfg(unix)]
#[tokio::test]
async fn stop_escalates_to_kill_after_grace_period() {
    let log = Arc::new(LogBuffer::new(10));
    let spec = ProcessSpec::new("/bin/sh")
        .arg("-c")
        .arg("trap '' TERM; echo ready; sleep 60");
    let mut child = ManagedChild::spawn(&spec, Arc::clone(&log)).unwrap();
    wait_for_first_line(&log).await;

    let outcome = child.stop(Duration::from_millis(300)).await.unwrap();
    assert!(outcome.forced);
    assert_eq!(outcome.exit, ExitKind::Signal(9));
}

async fn wait_for_first_line(log: &LogBuffer) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(line) = log.since(0).into_iter().next() {
            return line.text;
        }
        assert!(Instant::now() < deadline, "no output from child");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn is_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(pid)
        .is_some_and(|p| !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead))
}
