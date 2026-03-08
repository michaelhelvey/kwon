use crate::config::JobSpecification;
use log::{error, info, warn};
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;

/// Executes a single job, with configurable concurrency control via the passed
/// `tokio::sync::Semaphore`
pub async fn execute_job(
    name: &str,
    job: &JobSpecification,
    ctrl_sem: Arc<tokio::sync::Semaphore>,
) -> anyhow::Result<()> {
    let _permit = ctrl_sem.acquire().await?;

    let mut cmd = tokio::process::Command::new(&job.executable);

    cmd.kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(args) = &job.args {
        cmd.args(args);
    }

    if let Some(env) = &job.environment {
        cmd.envs(env);
    }

    if let Some(working_directory) = &job.working_directory {
        cmd.current_dir(working_directory);
    }

    let mut child = cmd.spawn()?;

    let stdout = child
        .stdout
        .take()
        .expect("Stdio::piped() should always capture the stdout");
    let stderr = child
        .stderr
        .take()
        .expect("Stdio::piped() should always capture the stderr");

    let name_string = name.to_string();
    let name_cloned = name_string.clone();

    let stdout_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: name_cloned.as_str(), "[stdout]: {}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!(target: name_string.as_str(), "[stderr]: {}", line);
        }
    });

    let status = child.wait().await;
    let (stdout_result, stderr_result) = tokio::join!(stdout_task, stderr_task);

    // If we reach this, that means that something in one of our tasks panicked, which basically
    // means that our logging backend panicked.  That leads to a "who's watching the watchers"
    // problem, because what are we going to do here?  Call the logging backend again.  So I'm not
    // sure that we're substantially adding to the integrity of the program by handling these
    // results.
    if let Err(e) = stdout_result {
        error!("error joining stdout reader task for job {name}: {e:?}");
    }

    if let Err(e) = stderr_result {
        error!("error joining stderr reader task for job {name}: {e:?}");
    }

    match status {
        Ok(s) if s.success() => {
            info!("job {name} exited successfully");
            Ok(())
        }
        Ok(s) => {
            warn!("job {name} exited with non-0 code {}", s);
            anyhow::bail!("job {name} exited with non-0 code {s}")
        }
        Err(e) => {
            error!("error waiting for process to exit: {e:?}");
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Local;
    use tokio::sync::Semaphore;

    use super::*;

    #[tokio::test]
    async fn test_execute_job() {
        // this is a simple smoke test for execute_job.  it's hard to test all the logging stuff
        // and orchestration without a larger test harness -- which we have in the e2e tests. so
        // we'll mostly test there.
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let job = JobSpecification {
            executable: "/bin/bash".to_string(),
            args: Some(vec!["-c".to_string(), "echo $FOO".to_string()]),
            environment: Some(env),
            working_directory: None,
            start_at: Local::now(),
            interval_seconds: 0,
        };

        let sem = Arc::new(Semaphore::new(1));
        execute_job("some_job", &job, sem)
            .await
            .expect("expected execute_job to succeed");
    }
}
