use std::sync::Arc;
use std::time::Duration;

use crate::config::{Arguments, Commands, Config, HistoryDatabase};
use clap::Parser;
use log::{debug, error, warn};
use tokio::sync::Semaphore;

mod config;
mod doctor;
mod job;

const MAX_CONCURRENCY: usize = 20;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    let config_path = Config::get_path_or_default(&args.config);
    let (app_config, logging_config) = Config::load_from_path(&config_path).await?;

    match args.command {
        Commands::Doctor => {
            doctor::print_doctor_checks(&config_path, &logging_config).await?;
        }
        Commands::Daemon => {
            let history =
                HistoryDatabase::get_or_default(&app_config.state_file_location).await?;
            let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENCY));
            loop {
                debug!("reloading configuration from {config_path}");
                // Reload config (but not the logger) on each tick, in case jobs have changed
                let app_config = Config::parse_from_path(&config_path).await?;
                let tick_rate = app_config.tick_rate_seconds.unwrap_or(60u64);
                let job_count = app_config.jobs.len();
                debug!("found {job_count} total jobs in config");

                let now = chrono::Local::now();

                // Collect jobs to run while holding the read lock, then drop it
                // before spawning tasks that need write access.
                let jobs_to_run: Vec<_> = {
                    let guard = history.read().await;
                    app_config
                        .jobs
                        .into_iter()
                        .filter(|(key, value)| {
                            if now < value.start_at {
                                return false;
                            }
                            match guard.get_last_run(key) {
                                Some(last_run) => {
                                    (now - last_run).num_seconds() > value.interval_seconds
                                }
                                None => true,
                            }
                        })
                        .collect()
                };

                let job_handles: Vec<_> = jobs_to_run
                    .into_iter()
                    .map(|(name, job)| {
                        let sem = semaphore.clone();
                        let history = history.clone();
                        tokio::spawn(async move {
                            match job::execute_job(name.as_str(), &job, sem).await {
                                Err(e) => {
                                    error!("error executing job {name}: {e:?}");
                                }
                                Ok(()) => {
                                    let mut guard = history.write().await;
                                    if let Err(e) = guard.write_last_run(&name).await {
                                        error!("could not write history for job {name}: {e:?}");
                                    }
                                }
                            }
                        })
                    })
                    .collect();

                let batch_timeout = Duration::from_secs(tick_rate);
                let start = tokio::time::Instant::now();
                if tokio::time::timeout(
                    batch_timeout,
                    futures::future::join_all(job_handles),
                )
                .await
                .is_err()
                {
                    warn!("batch timed out -- all jobs did not complete in {tick_rate} seconds");
                }

                let elapsed = start.elapsed().as_secs();
                let remaining = tick_rate.saturating_sub(elapsed);
                debug!(
                    "jobs complete, waiting for {remaining} more seconds to fill tick_rate={tick_rate}"
                );
                tokio::time::sleep(Duration::from_secs(remaining)).await;
            }
        }
        Commands::Install { .. } => {
            anyhow::bail!("the 'install' command is not yet implemented");
        }
    }

    Ok(())
}
