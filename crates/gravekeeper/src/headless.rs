//! Running the supervisor without a GUI (for services, servers and SSH sessions).

use std::process::ExitCode;
use std::sync::Arc;

use gravekeeper_core::paths::Paths;
use gravekeeper_core::{CoreHandle, StartError};

/// Runs until SIGTERM, SIGINT or Ctrl-C, then stops all applications. On Unix, SIGHUP
/// reloads the configuration file.
pub fn run(paths: Paths) -> ExitCode {
    let core = match CoreHandle::start(paths, Arc::new(|| {})) {
        Ok(core) => core,
        Err(StartError::AlreadyRunning) => {
            eprintln!("Gravekeeper is already running (in the tray or as a service).");
            return ExitCode::from(3);
        }
        Err(err) => {
            tracing::error!(%err, "failed to start engine");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("running headless");

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(%err, "cannot wait for signals");
            core.shutdown();
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = runtime.block_on(wait_for_stop(&core)) {
        tracing::error!(%err, "cannot wait for signals");
    }
    tracing::info!("stopping");
    core.shutdown();
    ExitCode::SUCCESS
}

#[cfg(unix)]
async fn wait_for_stop(core: &CoreHandle) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    loop {
        tokio::select! {
            _ = terminate.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = hangup.recv() => {
                tracing::info!("SIGHUP: reloading the configuration");
                core.send(gravekeeper_core::Command::ReloadConfig);
            }
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_stop(_core: &CoreHandle) -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
