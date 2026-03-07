# /// script
# requires-python = ">=3.14"
# dependencies = []
# ///
import os
import subprocess
import unittest
from contextlib import contextmanager
from typing import Optional

# adapt as necessary for your system (override via DOCKER env var)
DOCKER = os.environ.get("DOCKER", "/opt/podman/bin/podman")
CONTAINER_CONFIG_PATH = "/etc/kwon/jobs.toml"
CONTAINER_KWON_BIN = "/opt/kwon-bin/kwon"
CONTAINER_LOG_FILE = "/var/log/kwon/kwon.log"
HOST_LOG_PATH = "./data/kwon.log"
LOG_SEP = ("-" * 10) + "\n"


def docker_exec(cmd: str, check=True, input_bytes: Optional[bytes] = None):
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


class BaseTest(unittest.TestCase):
    def assertContains(self, source: str, pattern: str):
        if pattern not in source:
            msg = f"expected '{source}' to contain '{pattern}'"
            raise self.failureException(msg)


class TestKwon(BaseTest):
    def test_initialization_and_logging_to_custom_log_file(self):
        config = f'log_file = "{CONTAINER_LOG_FILE}"\nlog_level = "debug"\n'
        with custom_config(config):
            result = exec_and_log(CONTAINER_KWON_BIN, True)
            log = last_log_section()
            self.assertContains(
                result.stderr.decode(),
                f'configuring logging to "{CONTAINER_LOG_FILE}" with level Debug',
            )
            self.assertContains(
                log, "successfully configured logging to configured sink"
            )

    def test_initialization_and_logging_to_syslog(self):
        with custom_config(""):
            result = exec_and_log(CONTAINER_KWON_BIN, False)
            self.assertContains(
                result.stderr.decode(),
                "configuring logging to default sink (syslog) with level Info",
            )

            result = exec_and_log("grep 'kwon' /var/log/syslog")
            self.assertContains(
                result.stdout.decode(),
                "successfully configured logging to configured sink",
            )


if __name__ == "__main__":
    unittest.main()
