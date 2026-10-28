//! Command-line arguments.

pub const USAGE: &str = "\
usage:
  gravekeeper [--minimized]       open the GUI (minimized: only the tray icon)
  gravekeeper --headless          run the supervisor without a GUI until stopped
                                  (SIGTERM/Ctrl-C stops it, SIGHUP reloads the configuration)
  gravekeeper service install     run the headless supervisor as a user service
  gravekeeper service uninstall   remove that service
  gravekeeper service status      show whether the service is installed and running
  gravekeeper --version
  gravekeeper --help";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Gui {
        minimized: bool,
    },
    Headless,
    Service(ServiceCommand),
    /// The elevated helper Gravekeeper starts itself (`elevate.rs`); not listed in the usage.
    Elevated {
        request: String,
        report: Option<String>,
    },
    Help,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceCommand {
    Install,
    Uninstall,
    Status,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Mode, String> {
    let args: Vec<String> = args.into_iter().collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => Ok(Mode::Gui { minimized: false }),
        ["--minimized"] => Ok(Mode::Gui { minimized: true }),
        ["--headless"] => Ok(Mode::Headless),
        ["service", "install"] => Ok(Mode::Service(ServiceCommand::Install)),
        ["service", "uninstall"] => Ok(Mode::Service(ServiceCommand::Uninstall)),
        ["service", "status"] => Ok(Mode::Service(ServiceCommand::Status)),
        ["elevated", request] => Ok(Mode::Elevated {
            request: (*request).to_owned(),
            report: None,
        }),
        ["elevated", request, "--report", pipe] => Ok(Mode::Elevated {
            request: (*request).to_owned(),
            report: Some((*pipe).to_owned()),
        }),
        ["--help" | "-h"] => Ok(Mode::Help),
        ["--version" | "-V"] => Ok(Mode::Version),
        _ => Err(format!("unrecognized arguments: {}", args.join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Mode, String> {
        parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn modes() {
        assert_eq!(parse_str(""), Ok(Mode::Gui { minimized: false }));
        assert_eq!(parse_str("--minimized"), Ok(Mode::Gui { minimized: true }));
        assert_eq!(parse_str("--headless"), Ok(Mode::Headless));
        assert_eq!(
            parse_str("service status"),
            Ok(Mode::Service(ServiceCommand::Status))
        );
        assert_eq!(parse_str("-V"), Ok(Mode::Version));
        assert_eq!(
            parse_str(r"elevated 5b5d --report \\.\pipe\p"),
            Ok(Mode::Elevated {
                request: "5b5d".into(),
                report: Some(r"\\.\pipe\p".into())
            })
        );
        assert!(parse_str("elevated").is_err());
        assert!(parse_str("service").is_err());
        assert!(parse_str("--headless --minimized").is_err());
    }
}
