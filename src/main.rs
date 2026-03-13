use crate::config::{Arguments, Commands, Config};
use clap::Parser;

mod config;
mod daemon;
mod doctor;
mod install;
mod job;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();

    // Install doesn't need an existing config file — it creates one.
    if let Commands::Install { systemd } = &args.command {
        if !systemd {
            anyhow::bail!(
                "please specify --systemd to install as a systemd service. \
                 See `kwon install --help` for more information."
            );
        }
        return install::install_systemd();
    }

    let config_path = Config::get_path_or_default(&args.config);
    let (app_config, logging_config) = Config::load_from_path(&config_path).await?;

    match args.command {
        Commands::Doctor => {
            doctor::print_doctor_checks(&config_path, &logging_config).await?;
        }
        Commands::Daemon => {
            daemon::run(&config_path, &app_config).await?;
        }
        Commands::Install { .. } => unreachable!("handled above"),
    }

    Ok(())
}
