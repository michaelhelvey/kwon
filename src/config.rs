use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use ftail::Ftail;
use log::{LevelFilter, info};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

const DEFAULT_CONFIG_PATH: &str = "/etc/kwon/jobs.toml";
const DEFAULT_HISTORY_PATH: &str = "/var/lib/kwon/history.json";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub command: Commands,

    /// Optional absolute path to a configuration file. Default: /etc/kwon/jobs.toml
    #[arg(short, long, global = true)]
    pub config: Option<Utf8PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Installs kwon as a daemon on your system
    Install {
        /// Install as a systemd service
        systemd: Option<bool>,
    },
    /// Starts the daemon process.  Use this to run kwon in the foreground, or use this as the
    /// command to run in your sytemd service config or other
    Daemon,

    /// Load kwon's configuration and print everything it knows about its environment to stdout.
    /// Useful for debugging your local installation.  Outputs with colors by default, respects
    /// NO_COLOR variable.
    Doctor,
}

/// Alias for `log::LevelFilter` that we can deserialize from TOML with serde.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CustomLogLevel {
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

/// Represents a single job to be executed on a schedule
#[derive(Deserialize, Debug)]
pub struct JobSpecification {
    /// Executable to run.  This will be passed as the first argument to
    /// `std::process::Command::new`, and has the same semantics: $PATH search will be performed,
    /// unless the name contains a path separator.
    pub executable: String,

    /// Optional list of arguments to provide to the program.  This must be a list, not a string,
    /// as kwon does not perform command string parsing.
    pub args: Option<Vec<String>>,

    /// Optional mapping of additional environment variables to add to the execution environment of
    /// the job.  By default, the job will inherit the current process (kwon)'s environment.
    ///
    /// N.B. You can only _upsert_ to the current environment via this method.  Clearing or
    /// restricting a job's environment variables is not supported.
    pub environment: Option<HashMap<String, String>>,

    /// Optional absolute path to the working directory to set for the job.  By default, inherits the working directory
    /// from which `kwon daemon` was executed.
    pub working_directory: Option<Utf8PathBuf>,

    /// The first time at which to start the job, in RFC 3339 format.
    pub start_at: chrono::DateTime<chrono::Local>,

    /// The number of seconds between job runs.  This interval is best effort, based on the
    /// concurrency settings and tick rate (default: 60s) of the main loop.
    ///
    /// If you need jobs to run with more detailed granularity, or more often than once every 60
    /// seconds, `kwon` is probably not a good fit.
    pub interval_seconds: i64,
}

/// Specification for the TOML configuration file, which by default lives at /etc/kwon/jobs.toml,
/// but which can be customized via the global -c/--config argument to the kwon binary.
#[derive(Deserialize, Debug)]
pub struct Config {
    /// Optional absolute path to a file to append all log output to.  When this option is not set,
    /// kwon will log to syslog.
    ///
    /// By setting this option, you are taking all responsibility for log rotation and maintenance.
    /// Kwon will simply append all logs to this file.
    ///
    /// This option supports one magic string, "$STDERR".  If you set this, all log output will be
    /// redirected to stderr rather than to a file.  This is useful for debugging, or for using
    /// journald instead of syslog.
    pub log_file: Option<String>,

    /// Optional log level to set for kwon itself.  Default: Info
    pub log_level: Option<CustomLogLevel>,

    /// Optional number of seconds between each iteration of the main loop, which wakes up,
    /// re-reads the config file for any new jobs, and executes any pending jobs.  Default: 60
    pub tick_rate_seconds: Option<u64>,

    /// Optional path to the file that kwon will use to store job history (e.g. when it last ran
    /// a job).  Default: /var/lib/kwon/history.json
    pub state_file_location: Option<Utf8PathBuf>,

    /// A mapping from human-readable names to job specifications.
    #[serde(default)]
    pub jobs: HashMap<String, JobSpecification>,
}

/// Schema for the history file which stores the run history for our list of jobs.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct HistoryFile {
    jobs: HashMap<String, JobHistoryEntry>,
}

#[derive(Serialize, Deserialize, Debug)]
struct JobHistoryEntry {
    last_run: chrono::DateTime<chrono::Local>,
}

pub struct HistoryDatabase {
    resolved_path: Utf8PathBuf,
    history: HistoryFile,
}

impl HistoryDatabase {
    /// Reads the history file from its default location, or from a custom location from our config
    /// file.  Returns either a default (empty) HistoryFile struct, or the decoded struct from the
    /// history file.
    ///
    /// Returns an Arc<RwLock<Self>> because the intended use of this type is from a parallel job
    /// execution loop.  We need to protect the underlying file from concurrent writes.  So
    /// individual job executions have to acquire a lock on the database type in order to write to
    /// the file.
    ///
    /// The RwLock has to be a tokio::sync::RwLock and not a std::sync::RwLock because we have to
    /// be able to hold the lock across the `await` boundary of writing the file, such that writing
    /// to the in-memory HashMap and flushing it to the file is a single atomic operation.
    pub async fn get_or_default(
        custom_path: &Option<Utf8PathBuf>,
    ) -> anyhow::Result<Arc<RwLock<Self>>> {
        let default_path = Utf8PathBuf::from(DEFAULT_HISTORY_PATH);
        let path = custom_path.as_ref().unwrap_or(&default_path);

        let mut options = tokio::fs::OpenOptions::new();
        let mut file = options
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .await?;

        let mut history_bytes = String::new();
        file.read_to_string(&mut history_bytes).await?;
        if history_bytes.trim().is_empty() {
            let db = HistoryDatabase {
                resolved_path: path.to_owned(),
                history: HistoryFile::default(),
            };
            return Ok(Arc::new(RwLock::new(db)));
        }

        let decoded_history: HistoryFile = serde_json::from_str(&history_bytes)?;
        let db = HistoryDatabase {
            resolved_path: path.to_owned(),
            history: decoded_history,
        };
        Ok(Arc::new(RwLock::new(db)))
    }

    /// Given a string job id, returns the last known time that we ran that job, or None if there
    /// is no entry in the database.
    pub fn get_last_run(&self, job_id: &str) -> Option<chrono::DateTime<chrono::Local>> {
        self.history.jobs.get(job_id).map(|entry| entry.last_run)
    }

    /// Inserts a job run into the database and persists the database to the file.
    pub async fn write_last_run(&mut self, job_id: &str) -> anyhow::Result<()> {
        self.history.jobs.insert(
            job_id.to_string(),
            JobHistoryEntry {
                last_run: chrono::Local::now(),
            },
        );

        let history_bytes = serde_json::to_string(&self.history)?;
        tokio::fs::write(&self.resolved_path, history_bytes).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum ResolvedLogBackend {
    Stderr,
    Syslog,
    File { path: Utf8PathBuf },
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ResolvedLogConfig {
    pub level: LevelFilter,
    pub backend: ResolvedLogBackend,
}

/// Based on the resolved configuration, configures a backend for the `log` crate.
///
/// For every option: calclates a `log::LevelFilter` as a function of `Config.log_level`.
/// 1) File option exists, and is equal to the magic string `$STDERR` => use env_logger to print to
///    stdout.  Only really useful for local development on the host.
/// 2) File option exists, and is not equal to a magic string => use Ftail to create a logging
///    backend to the file specified by the path.
/// 3) File option does not exist: log to `syslog` with the name `kwon` as our service name.  Users
///    can query syslog either via journalctl or `cat /var/log/syslog`.
fn configure_log_backend(config: &Config) -> anyhow::Result<ResolvedLogConfig> {
    let level = config
        .log_level
        .as_ref()
        .map(|l| l.to_level_filter())
        .unwrap_or(LevelFilter::Info);

    if let Some(file) = &config.log_file {
        if file == "$STDERR" {
            eprintln!("[kwon-init]: configuring logging to stderr with level {level:?}");
            env_logger::builder()
                .filter_module("kwon", level)
                .try_init()?;
            return Ok(ResolvedLogConfig {
                level,
                backend: ResolvedLogBackend::Stderr,
            });
        }

        eprintln!("[kwon-init]: configuring logging to {file:?} with level {level:?}");
        let path_buf = Utf8PathBuf::from(file);
        let path = path_buf.clone().into_std_path_buf();
        Ftail::new().single_file(&path, true, level).init()?;
        return Ok(ResolvedLogConfig {
            level,
            backend: ResolvedLogBackend::File { path: path_buf },
        });
    }

    eprintln!("[kwon-init]: configuring logging to default sink (syslog) with level {level:?}");
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
            Ok(ResolvedLogConfig {
                level,
                backend: ResolvedLogBackend::Syslog,
            })
        }
    }
}

impl Config {
    /// Loads and parses the config file without initializing logging.
    /// Use this for reloading config inside the daemon loop.
    pub async fn parse_from_path(config_path: &Utf8PathBuf) -> anyhow::Result<Self> {
        let config_content = tokio::fs::read_to_string(&config_path)
            .await
            .with_context(|| format!("could not read file at {config_path:?}"))?;

        let config: Config =
            toml::from_str(config_content.as_str()).context("invalid toml configuration file")?;

        Ok(config)
    }

    /// Loads the config file and initializes the logging backend.
    /// Should only be called once at startup.
    pub async fn load_from_path(
        config_path: &Utf8PathBuf,
    ) -> anyhow::Result<(Self, ResolvedLogConfig)> {
        let config = Self::parse_from_path(config_path).await?;
        let log_config = configure_log_backend(&config)?;
        info!("successfully configured logging: {log_config:?}");

        Ok((config, log_config))
    }

    pub fn get_path_or_default(custom_path: &Option<Utf8PathBuf>) -> Utf8PathBuf {
        let default_config_path = Utf8PathBuf::from(DEFAULT_CONFIG_PATH);
        let config_path = custom_path.as_ref().unwrap_or(&default_config_path);
        config_path.to_owned()
    }
}
