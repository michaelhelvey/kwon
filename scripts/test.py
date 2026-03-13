# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
import os
import subprocess
import time
import unittest
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from typing import Optional

# adapt as necessary for your system (override via DOCKER env var)
DOCKER = os.environ.get("DOCKER", "/opt/podman/bin/podman")
# When E2E_CONTAINER is set, use `docker exec <container>` directly instead of
# `docker compose exec kwon`.  This is used in CI where we run the container
# via `docker run` (to pass --cgroupns=host which docker-compose doesn't support).
E2E_CONTAINER = os.environ.get("E2E_CONTAINER", "")
CONTAINER_CONFIG_PATH = "/etc/kwon/jobs.toml"
CONTAINER_KWON_BIN = "/opt/kwon-bin/kwon"
CONTAINER_LOG_FILE = "/var/log/kwon/kwon.log"
CONTAINER_HISTORY_FILE = "/tmp/kwon-test-history.json"
CONTAINER_MARKER_DIR = "/tmp/kwon-test-markers"
HOST_LOG_PATH = "./data/kwon.log"
LOG_SEP = ("-" * 10) + "\n"

# Short tick rate for fast tests
TICK_RATE = 2


def docker_exec(cmd: str, check=True, input_bytes: Optional[bytes] = None):
    if E2E_CONTAINER:
        # Direct docker exec mode (CI)
        args = [DOCKER, "exec"]
        if input_bytes is not None:
            args.append("-i")
        args += [E2E_CONTAINER, "bash", "-c", cmd]
    else:
        # docker compose exec mode (local dev)
        args = [DOCKER, "compose", "exec"]
        if input_bytes is not None:
            args.append("-T")
        args += ["kwon", "bash", "-c", cmd]

    result = subprocess.run(args, capture_output=True, input=input_bytes)

    if check and result.returncode:
        print(f"cmd '{cmd}' failed with return code {result.returncode}")
        print("failed stdout: ", result.stdout)
        print("failed stderr: ", result.stderr)
        result.check_returncode()

    return result


def write_container_config(contents: str):
    docker_exec(f"mkdir -p $(dirname {CONTAINER_CONFIG_PATH})")
    docker_exec(
        f"cat > {CONTAINER_CONFIG_PATH}",
        input_bytes=contents.encode(),
    )


@contextmanager
def custom_config(contents: str):
    write_container_config(contents)
    try:
        yield
    finally:
        write_container_config("")


def exec_and_log(cmd: str, write_log_separator=False):
    if write_log_separator:
        with open(HOST_LOG_PATH, "a") as log_file:
            log_file.write(LOG_SEP)

    return docker_exec(cmd)


def last_log_section():
    with open(HOST_LOG_PATH, "r") as log_file:
        contents = log_file.read()
        sections = contents.split(LOG_SEP)
        return sections[-1]


def rfc3339(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%dT%H:%M:%S%z")


def past_time() -> str:
    return rfc3339(datetime.now(timezone.utc) - timedelta(hours=1))


def future_time() -> str:
    return rfc3339(datetime.now(timezone.utc) + timedelta(hours=1))


def daemon_config(jobs_toml: str) -> str:
    return f"""\
log_file = "$STDERR"
log_level = "debug"
tick_rate_seconds = {TICK_RATE}
state_file_location = "{CONTAINER_HISTORY_FILE}"

{jobs_toml}
"""


@contextmanager
def run_daemon(config: str):
    """Start the daemon in the background, yield, then kill it and clean up."""
    docker_exec(f"rm -rf {CONTAINER_MARKER_DIR} {CONTAINER_HISTORY_FILE}")
    docker_exec(f"mkdir -p {CONTAINER_MARKER_DIR}")
    write_container_config(config)
    # start daemon in background
    docker_exec(f"nohup {CONTAINER_KWON_BIN} daemon > /dev/null 2>&1 & echo $!")
    try:
        yield
    finally:
        docker_exec("pkill -f 'kwon daemon'", check=False)
        docker_exec(
            f"rm -rf {CONTAINER_MARKER_DIR} {CONTAINER_HISTORY_FILE}", check=False
        )
        write_container_config("")


def marker_exists(name: str) -> bool:
    result = docker_exec(f"test -f {CONTAINER_MARKER_DIR}/{name}", check=False)
    return result.returncode == 0


def marker_count(name: str) -> int:
    """Read the integer content of a marker file, or 0 if it doesn't exist."""
    result = docker_exec(f"cat {CONTAINER_MARKER_DIR}/{name}", check=False)
    if result.returncode != 0:
        return 0
    return int(result.stdout.decode().strip())


class BaseTest(unittest.TestCase):
    def assertContains(self, source: str, pattern: str):
        if pattern not in source:
            msg = f"expected '{source}' to contain '{pattern}'"
            raise self.failureException(msg)


class TestKwon(BaseTest):
    def test_initialization_and_logging_to_custom_log_file(self):
        config = f'log_file = "{CONTAINER_LOG_FILE}"\nlog_level = "debug"\n'
        with custom_config(config):
            result = exec_and_log(f"{CONTAINER_KWON_BIN} doctor", True)
            log = last_log_section()
            self.assertContains(
                result.stderr.decode(),
                f'configuring logging to "{CONTAINER_LOG_FILE}" with level Debug',
            )
            self.assertContains(
                log,
                "successfully configured logging: ResolvedLogConfig { level: Debug, backend: File { path:",
            )

    def test_initialization_and_logging_to_syslog(self):
        with custom_config(""):
            result = exec_and_log(f"{CONTAINER_KWON_BIN} doctor", False)
            self.assertContains(
                result.stderr.decode(),
                "configuring logging to default sink (syslog) with level Info",
            )

            result = exec_and_log("grep 'kwon' /var/log/syslog")
            self.assertContains(
                result.stdout.decode(),
                "successfully configured logging: ResolvedLogConfig { level: Info, backend: Syslog }",
            )

    def test_initialization_and_logging_to_stderr(self):
        config = 'log_file = "$STDERR"\nlog_level = "debug"\n'
        with custom_config(config):
            result = exec_and_log(f"{CONTAINER_KWON_BIN} doctor", False)
            self.assertContains(
                result.stderr.decode(),
                "configuring logging to stderr with level Debug",
            )
            self.assertContains(
                result.stderr.decode(),
                "successfully configured logging: ResolvedLogConfig { level: Debug, backend: Stderr }",
            )


class TestDaemonLoop(BaseTest):
    def test_job_runs_when_start_at_is_in_the_past(self):
        """A job with start_at in the past should execute on the first tick."""
        config = daemon_config(f"""\
[jobs.touch_marker]
executable = "touch"
args = ["{CONTAINER_MARKER_DIR}/ran"]
start_at = "{past_time()}"
interval_seconds = 3600
""")
        with run_daemon(config):
            # wait for one full tick + buffer
            time.sleep(TICK_RATE + 2)
            self.assertTrue(
                marker_exists("ran"), "expected job to have created marker file"
            )

    def test_job_does_not_run_when_start_at_is_in_the_future(self):
        """A job with start_at in the future should NOT execute."""
        config = daemon_config(f"""\
[jobs.future_job]
executable = "touch"
args = ["{CONTAINER_MARKER_DIR}/should_not_exist"]
start_at = "{future_time()}"
interval_seconds = 3600
""")
        with run_daemon(config):
            time.sleep(TICK_RATE + 2)
            self.assertFalse(
                marker_exists("should_not_exist"),
                "job with future start_at should not have run",
            )

    def test_job_respects_interval(self):
        """A job should not re-run before its interval has elapsed."""
        # Use a counter file: each run appends a line. With a long interval,
        # only one run should happen across two ticks.
        config = daemon_config(f"""\
[jobs.counter]
executable = "bash"
args = ["-c", "expr $(cat {CONTAINER_MARKER_DIR}/count 2>/dev/null || echo 0) + 1 > {CONTAINER_MARKER_DIR}/count"]
start_at = "{past_time()}"
interval_seconds = 3600
""")
        with run_daemon(config):
            # wait for two ticks
            time.sleep((TICK_RATE + 1) * 2)
            count = marker_count("count")
            self.assertEqual(
                count, 1, f"expected job to run exactly once, but ran {count} times"
            )

    def test_failed_job_does_not_record_history(self):
        """A job that exits non-zero should not be recorded in history,
        so it will be retried on the next tick."""
        config = daemon_config(f"""\
[jobs.failing]
executable = "bash"
args = ["-c", "expr $(cat {CONTAINER_MARKER_DIR}/fail_count 2>/dev/null || echo 0) + 1 > {CONTAINER_MARKER_DIR}/fail_count; exit 1"]
start_at = "{past_time()}"
interval_seconds = 3600
""")
        with run_daemon(config):
            # wait for two ticks — failing job should retry each tick
            time.sleep((TICK_RATE + 1) * 2)
            count = marker_count("fail_count")
            self.assertGreaterEqual(
                count, 2, f"expected failing job to retry, but only ran {count} times"
            )

    def test_multiple_jobs_run_concurrently(self):
        """Two independent jobs should both execute within a single tick."""
        config = daemon_config(f"""\
[jobs.job_a]
executable = "touch"
args = ["{CONTAINER_MARKER_DIR}/a"]
start_at = "{past_time()}"
interval_seconds = 3600

[jobs.job_b]
executable = "touch"
args = ["{CONTAINER_MARKER_DIR}/b"]
start_at = "{past_time()}"
interval_seconds = 3600
""")
        with run_daemon(config):
            time.sleep(TICK_RATE + 2)
            self.assertTrue(marker_exists("a"), "expected job_a to have run")
            self.assertTrue(marker_exists("b"), "expected job_b to have run")


class TestInstall(BaseTest):
    """Tests for `kwon install --systemd`."""

    def setUp(self):
        """Clean up any prior install artifacts so each test starts fresh."""
        docker_exec(f"rm -f {CONTAINER_CONFIG_PATH}", check=False)
        docker_exec("rm -rf /etc/kwon", check=False)
        docker_exec("rm -rf /var/lib/kwon", check=False)
        docker_exec("rm -f /etc/systemd/system/kwon.service", check=False)
        docker_exec("systemctl stop kwon", check=False)
        docker_exec("systemctl disable kwon", check=False)
        docker_exec("systemctl daemon-reload", check=False)

    def tearDown(self):
        """Stop and clean up the installed service."""
        docker_exec("systemctl stop kwon", check=False)
        docker_exec("systemctl disable kwon", check=False)
        docker_exec("rm -f /etc/systemd/system/kwon.service", check=False)
        docker_exec("systemctl daemon-reload", check=False)
        docker_exec("rm -rf /etc/kwon /var/lib/kwon", check=False)

    def test_install_creates_config_file(self):
        """kwon install --systemd should create /etc/kwon/jobs.toml."""
        result = docker_exec(f"{CONTAINER_KWON_BIN} install --systemd")
        stderr = result.stderr.decode()
        self.assertContains(stderr, "writing default config to /etc/kwon/jobs.toml")

        # Verify the config file exists and is valid TOML (kwon doctor can parse it)
        result = docker_exec(f"test -f {CONTAINER_CONFIG_PATH}")
        self.assertEqual(result.returncode, 0)

    def test_install_creates_state_directory(self):
        """kwon install --systemd should create /var/lib/kwon."""
        docker_exec(f"{CONTAINER_KWON_BIN} install --systemd")
        result = docker_exec("test -d /var/lib/kwon")
        self.assertEqual(result.returncode, 0)

    def test_install_creates_and_starts_systemd_service(self):
        """kwon install --systemd should create, enable, and start the kwon service."""
        docker_exec(f"{CONTAINER_KWON_BIN} install --systemd")

        # The unit file should exist
        result = docker_exec("test -f /etc/systemd/system/kwon.service")
        self.assertEqual(result.returncode, 0)

        # The service should be enabled
        result = docker_exec("systemctl is-enabled kwon")
        self.assertContains(result.stdout.decode(), "enabled")

        # The service should be active
        time.sleep(1)  # give systemd a moment to start the process
        result = docker_exec("systemctl is-active kwon")
        self.assertContains(result.stdout.decode(), "active")

    def test_install_does_not_overwrite_existing_config(self):
        """If /etc/kwon/jobs.toml already exists, install should skip it."""
        docker_exec("mkdir -p /etc/kwon")
        custom_content = "# my custom config"
        docker_exec(
            f"cat > {CONTAINER_CONFIG_PATH}",
            input_bytes=custom_content.encode(),
        )

        result = docker_exec(f"{CONTAINER_KWON_BIN} install --systemd")
        stderr = result.stderr.decode()
        self.assertContains(stderr, "config file already exists")

        # Verify our custom content was preserved
        result = docker_exec(f"cat {CONTAINER_CONFIG_PATH}")
        self.assertEqual(result.stdout.decode().strip(), custom_content)

    def test_install_without_systemd_flag_fails(self):
        """kwon install without --systemd should fail with guidance."""
        result = docker_exec(f"{CONTAINER_KWON_BIN} install", check=False)
        self.assertNotEqual(result.returncode, 0)

    def test_install_unit_file_references_correct_binary(self):
        """The generated systemd unit should point ExecStart at the kwon binary."""
        docker_exec(f"{CONTAINER_KWON_BIN} install --systemd")
        result = docker_exec("cat /etc/systemd/system/kwon.service")
        unit_content = result.stdout.decode()
        self.assertContains(unit_content, f"ExecStart={CONTAINER_KWON_BIN} daemon")


if __name__ == "__main__":
    unittest.main()
