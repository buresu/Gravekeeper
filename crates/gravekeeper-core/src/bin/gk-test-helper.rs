//! Test helper with predictable behavior, so process tests run the same on every OS
//! without depending on shell utilities. Not part of the product.
//!
//! Usage: gk-test-helper <mode> [arg]
//!   lines N      print N lines to stdout ("out i") and N to stderr ("err i")
//!   flood BYTES  write BYTES bytes of 1 KiB lines to stdout and to stderr, concurrently
//!   exit CODE    exit with CODE
//!   abort        abort the process (crash)
//!   sleep SECS   sleep, then exit 0
//!   tree SECS    spawn `sleep SECS` as a child, print its pid, then sleep SECS

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    match mode {
        "lines" => {
            let mut out = std::io::stdout().lock();
            let mut err = std::io::stderr().lock();
            for i in 0..n {
                writeln!(out, "out {i}").unwrap();
                writeln!(err, "err {i}").unwrap();
            }
        }
        "flood" => {
            let stderr = std::thread::spawn(move || flood(&mut std::io::stderr().lock(), n));
            flood(&mut std::io::stdout().lock(), n);
            stderr.join().unwrap();
        }
        "exit" => std::process::exit(n as i32),
        "abort" => std::process::abort(),
        "sleep" => std::thread::sleep(Duration::from_secs(n)),
        // The grandchild is deliberately left running: tests check that stopping this
        // process also stops it.
        #[allow(clippy::zombie_processes)]
        "tree" => {
            let exe = std::env::current_exe().unwrap();
            let child = Command::new(exe)
                .args(["sleep", &n.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            println!("{}", child.id());
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(n));
        }
        _ => {
            eprintln!("unknown mode: {mode:?}");
            std::process::exit(2);
        }
    }
}

fn flood(w: &mut impl Write, bytes: u64) {
    let line = [b"x".repeat(1023), b"\n".to_vec()].concat();
    let mut written = 0;
    while written < bytes {
        w.write_all(&line).unwrap();
        written += line.len() as u64;
    }
    w.flush().unwrap();
}
