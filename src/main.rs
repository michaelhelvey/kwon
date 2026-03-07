use anyhow::Context;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use ftail::Ftail;
use log::{LevelFilter, info};
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "/etc/kwon/jobs.toml";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Arguments {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Optional absolute path to a configuration file. Default: /etc/kwon/jobs.toml
    #[arg(short, long, global = true)]
    config: Option<Utf8PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Installs kwon as a daemon on your system
    Install {
        /// Install as systemd
        systemd: Option<bool>,
    },
    /// Starts the daemon process.  Use this to run kwon in the foreground, or use this as the
    /// command to run in your sytemd service config or other
    Daemon {},
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum CustomLogLevel {
    Debug,
    Info,
    Warn,
}

impl CustomLogLevel {
    /// Converts a deserialized `CustomLogLevel` to a `LevelFilter` from the `log` crate.
    fn to_level_filter(&self) -> LevelFilter {
        match self {
            CustomLogLevel::Debug => LevelFilter::Debug,
            CustomLogLevel::Info => LevelFilter::Info,
            _ => LevelFilter::Warn,
        }
    }
}

#[derive(Deserialize, Debug)]
struct Config {
    /// Optional absolute path to a file to append all log output to.  By default, logs to syslog.
    /// By setting this option, you are taking all responsibility for log rotation and maintenance.
    /// Kwon will simply blindly append all logs to this file.  Use at your own discretion.
    /// This option supports one magic string, "$STDERR".  If you set this, all log output will be
    /// redirected to stderr rather than to a file.  This is useful for debugging.
    log_file: Option<String>,
    /// Optional log level to set for kwon itself.  Default: Info
    log_level: Option<CustomLogLevel>,
    // TODO: set up jobs configuration
}

fn configure_log_backend(
    file: Option<String>,
    level: Option<CustomLogLevel>,
) -> anyhow::Result<()> {
    let level = level
        .map(|l| l.to_level_filter())
        .unwrap_or(LevelFilter::Info);

    if let Some(file) = file {
        if file == "$STDERR" {
            eprintln!("configuring logging to stderr with level {level:?}");
            env_logger::builder()
                .filter_module("kwon", level)
                .try_init()?;
            return Ok(());
        }

        eprintln!("configuring logging to {file:?} with level {level:?}");
        let path = Utf8PathBuf::from(file).into_std_path_buf();
        Ftail::new().single_file(&path, true, level).init()?;
        return Ok(());
    }

    eprintln!("configuring logging to default sink (syslog) with level {level:?}");
    let formatter = syslog::Formatter3164 {
        facility: syslog::Facility::LOG_USER,
        hostname: None,
        process: "kwon".into(),
        pid: 0,
    };

    match syslog::unix(formatter) {
        Err(e) => Err(anyhow::anyhow!(e).context("not able to connect to syslog")),
        Ok(logger) => {
            log::set_boxed_logger(Box::new(syslog::BasicLogger::new(logger)))
                .map(|()| log::set_max_level(level))?;
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Start up, parse our arguments, parse our configuration, and then figure out where we should
    // log to.
    let args = Arguments::parse();
    let config_path = args
        .config
        .unwrap_or_else(|| Utf8PathBuf::from(DEFAULT_CONFIG_PATH));

    let config_content = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("could not read file at {config_path:?}"))?;
    eprintln!("read config content: {config_content:?}");
    let config: Config =
        toml::from_str(config_content.as_str()).context("invalid toml configuration file")?;

    configure_log_backend(config.log_file, config.log_level)?;
    info!("successfully configured logging to configured sink.  see stderr for details");

    Ok(())
}
