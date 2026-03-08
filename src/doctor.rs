use camino::Utf8PathBuf;
use colored::Colorize;
use std::collections::HashMap;
use syslog::Formatter3164;

use crate::config::{ResolvedLogBackend, ResolvedLogConfig};

async fn check_logging_config(
    checks: &mut Vec<String>,
    logging_config: &ResolvedLogConfig,
) -> anyhow::Result<()> {
    match &logging_config.backend {
        ResolvedLogBackend::File { path } => {
            // if we're logging to a file, make sure that we can access the file
            let mut options = tokio::fs::OpenOptions::new();
            let file = options.read(true).write(true).create(true).open(path).await;

            if let Err(e) = file {
                checks.push(format!(
                    "⛔️ invalid log file path ({path:?}) provided: {e:?}"
                ));
            } else {
                checks.push("✅ log file is accessible".to_string());
            }
        }
        ResolvedLogBackend::Syslog => {
            let fake_formatter = Formatter3164 {
                process: "fake".into(),
                hostname: None,
                facility: syslog::Facility::LOG_USER,
                pid: 0,
            };
            match syslog::unix(fake_formatter) {
                Err(e) => {
                    checks.push(format!("⛔️ could not connect to syslog: {e:?}"));
                }
                _ => {
                    checks.push("✅ connected to syslog successfully".to_string());
                }
            }
        }
        _ => {}
    }

    Ok(())
}

pub async fn print_doctor_checks(
    config_path: &Utf8PathBuf,
    logging_config: &ResolvedLogConfig,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let cwd = Utf8PathBuf::from_path_buf(cwd)
        .map_err(|e| anyhow::anyhow!("could not convert {e:?} to a utf8 string"))?;
    let username = whoami::username()?;

    let title = "kwon doctor: debug kwon configuration".bold().magenta();
    let basic_config_title = "basic configuration".bold().cyan();
    let checks_title = "checks".bold().cyan();
    let stderr_note = r#"
 Note: this will always print to stdout even if your logging config points somewhere else.  The
 fact that you are reading this does not mean that kwon is configured to log to stdout.
            "#
    .italic();

    let mut config: HashMap<&str, String> = HashMap::new();
    config.insert("running as user", username);
    config.insert("cwd", cwd.to_string());
    config.insert("resolved valid config file", config_path.to_string());
    config.insert("logging config", format!("{:?}", logging_config));

    let separator = "=".repeat(100);

    // TODO: add more checks as we think of them.
    let mut checks: Vec<String> = Vec::new();
    check_logging_config(&mut checks, logging_config).await?;
    let checks_str: String = checks.iter().map(|s| format!(" - {s}\n")).collect();

    let basic_config_str = config
        .iter()
        .map(|(k, v)| format!(" {}: {}", k.bold(), v))
        .collect::<Vec<String>>()
        .join("\n");

    let about = format!(
        r#"
{separator}
 {title}
{separator}
{stderr_note}
{separator}
 {basic_config_title}
{separator}

{basic_config_str}

{separator}
 {checks_title}
{separator}

{checks_str}
            "#
    );
    println!("{}", about.trim_end());

    Ok(())
}
