use std::sync::Arc;
use std::time::Duration;

use camino::Utf8PathBuf;
use log::{debug, error, warn};
use tokio::sync::Semaphore;

use crate::config::{Config, HistoryDatabase};
use crate::job;

const MAX_CONCURRENCY: usize = 20;

/// Runs the main daemon loop: on each tick, reloads the config, determines which jobs are due,
/// spawns them concurrently (bounded by [`MAX_CONCURRENCY`]), and waits for completion with a
/// timeout equal to the tick rate.
pub async fn run(config_path: &Utf8PathBuf, app_config: &Config) -> anyhow::Result<()> {
    let history = HistoryDatabase::get_or_default(&app_config.state_file_location).await?;
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENCY));

    loop {
        debug!("reloading configuration from {config_path}");
        // Reload config (but not the logger) on each tick, in case jobs have changed.
        // If the config is invalid (e.g. mid-edit), log the error and skip this tick.
        let app_config = match Config::parse_from_path(config_path).await {
            Ok(config) => config,
            Err(e) => {
                warn!("failed to reload config from {config_path}, skipping tick: {e:#}");
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };
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
                            let elapsed = (now - last_run).num_seconds().max(0) as u64;
                            elapsed > value.interval_seconds
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

        let abort_handles: Vec<_> = job_handles.iter().map(|h| h.abort_handle()).collect();

        let batch_timeout = Duration::from_secs(tick_rate);
        let start = tokio::time::Instant::now();
        if tokio::time::timeout(batch_timeout, futures::future::join_all(job_handles))
            .await
            .is_err()
        {
            warn!(
                "batch timed out -- all jobs did not complete in {tick_rate} seconds, aborting remaining tasks"
            );
            for handle in &abort_handles {
                handle.abort();
            }
        }

        let elapsed = start.elapsed().as_secs();
        let remaining = tick_rate.saturating_sub(elapsed);
        debug!("jobs complete, waiting for {remaining} more seconds to fill tick_rate={tick_rate}");
        tokio::time::sleep(Duration::from_secs(remaining)).await;
    }
}
