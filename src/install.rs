use std::process::Command;

use camino::Utf8PathBuf;

use crate::config::DEFAULT_CONFIG_PATH;

const DEFAULT_STATE_DIR: &str = "/var/lib/kwon";
const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/kwon.service";

const DEFAULT_CONFIG_CONTENTS: &str = r#"# kwon configuration file
# See https://github.com/michaelhelvey/kwon for documentation.

# Uncomment to log to a file instead of syslog:
# log_file = "/var/log/kwon.log"

# Log level: debug, info, or warn. Default: info
# log_level = "info"

# How often (in seconds) kwon checks for pending jobs. Default: 60
# tick_rate_seconds = 60

# Where to store job run history. Default: /var/lib/kwon/history.json
# state_file_location = "/var/lib/kwon/history.json"

# Define jobs below. Example:
# [jobs.my_job]
# executable = "/usr/bin/echo"
# args = ["hello from kwon"]
# interval_seconds = 3600
# start_at = "2026-01-01T00:00:00+00:00"
"#;

fn systemd_unit(kwon_bin: &str) -> String {
    format!(
        r#"[Unit]
Description=kwon job scheduler
After=network.target

[Service]
Type=simple
ExecStart={kwon_bin} daemon
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
"#
    )
}

fn resolve_current_exe() -> anyhow::Result<Utf8PathBuf> {
    let exe = std::env::current_exe()?;
    let path = Utf8PathBuf::from_path_buf(exe)
        .map_err(|p| anyhow::anyhow!("current exe path is not valid UTF-8: {p:?}"))?;
    Ok(path)
}

fn run_systemctl(args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("systemctl").args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("systemctl {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

pub fn install_systemd() -> anyhow::Result<()> {
    let kwon_bin = resolve_current_exe()?;
    eprintln!("[kwon-install] resolved kwon binary: {kwon_bin}");

    // 1. Create config directory and default config file
    let config_path = Utf8PathBuf::from(DEFAULT_CONFIG_PATH);
    let config_dir = config_path
        .parent()
        .expect("DEFAULT_CONFIG_PATH should have a parent directory");

    if !config_dir.exists() {
        eprintln!("[kwon-install] creating config directory: {config_dir}");
        std::fs::create_dir_all(config_dir.as_std_path())?;
    }

    if config_path.exists() {
        eprintln!("[kwon-install] config file already exists at {config_path}, skipping");
    } else {
        eprintln!("[kwon-install] writing default config to {config_path}");
        std::fs::write(config_path.as_std_path(), DEFAULT_CONFIG_CONTENTS)?;
    }

    // 2. Create state directory
    let state_dir = Utf8PathBuf::from(DEFAULT_STATE_DIR);
    if !state_dir.exists() {
        eprintln!("[kwon-install] creating state directory: {state_dir}");
        std::fs::create_dir_all(state_dir.as_std_path())?;
    }

    // 3. Write the systemd unit file
    eprintln!("[kwon-install] writing systemd unit to {SYSTEMD_UNIT_PATH}");
    let unit = systemd_unit(kwon_bin.as_str());
    std::fs::write(SYSTEMD_UNIT_PATH, unit)?;

    // 4. Reload systemd, enable and start the service
    eprintln!("[kwon-install] reloading systemd daemon");
    run_systemctl(&["daemon-reload"])?;

    eprintln!("[kwon-install] enabling kwon service");
    run_systemctl(&["enable", "kwon"])?;

    eprintln!("[kwon-install] starting kwon service");
    run_systemctl(&["start", "kwon"])?;

    eprintln!("[kwon-install] kwon installed and running as a systemd service");
    Ok(())
}
