<h1 align=center><code>kwon</code></h1>

<p align="left">
  <a href="https://github.com/michaelhelvey/kwon/actions/workflows/ci.yml">
    <img alt="CI Status" src="https://github.com/michaelhelvey/kwon/actions/workflows/ci.yml/badge.svg">
  </a>
  <a href="https://github.com/michaelhelvey/kwon/actions/workflows/ci.yml">
    <img alt="GitHub License" src="https://img.shields.io/github/license/michaelhelvey/kwon">
  </a>
</p>

`kwon` is a cron-like job manager for personal linux computers. It's designed to be the simplest way
to run scheduled jobs on your machine.

**Key differences from cron**:

- Designed for single-user systems. It always runs as root.
- Human-oriented configuration syntax. Every job is specified as just 1) the number of seconds
  between invocations and 2) the start time, as an ISO 8601 formattted date string. This is very
  simple, but also very easy to remember.
- Easy observability. All logs, including the stdout and stderr pipes from your scheduled jobs, go
  to syslog by default. Or you can optionally take control of logging yourself by passing a custom
  log file path.

`kwon` isn't trying to be `cron`. If you're building a server-side system for production, you're
probably better off with `cron`, or better yet, a real distributed job queue. `kwon` is very
consciously for single-user personal computers.

## Getting Started

**Pre-requisites**: A computer running Linux. (You can probably adapt this to work on any nix-like
pretty easily, but I've only tested on Linux -- see the e2e tests.)

### Install

Install from [crates.io](https://crates.io/crates/kwon) using Cargo:

```sh
cargo install kwon
```

This will download, compile, and place the `kwon` binary in your `~/.cargo/bin/` directory. Make
sure that's on your `$PATH`.

If you'd rather build from source, clone this repository and run `cargo build --release`. Feel free
to customize the target with `--target` as you see fit, e.g. to build against `musl` as your libc.

### Setup

**I have a systemd-based linux distribution**

Run `sudo kwon install --systemd`. This will a) create a default configuration file for you at
`/etc/kwon/jobs.toml`, b) create a systemd service for you called `kwon` and start it.

**I am a special snowflake and I'm not using sudo/systemd/etc**

First of all, I love you, and I'm proud of you. Now here's how to install `kwon`: configure your
init system of choice to run `kwon daemon` as a background process. This will, each minute, load
your configuration file and execute any pending jobs.

After installation, run `kwon doctor` to get some simple debugging output that you can verify to
make sure that it looks like kwon is doing the right thing.

## Architectural Recommendations

- `kwon` is not designed to run more than 20 jobs at a time in parallel. this limit is enforced with
  a `tokio::sync::Semaphore`. You can easily change this in the source code and rebuild, which may
  or may not be fine, depending on your hardware, and how expensive your jobs are. I'm setting it to
  20 because I think that there should be _a_ hard cap on parallelism, and I think that very few
  people will want to run more than 20 jobs at a time. _Note that this is not a hard-cap on the
  number of jobs in your config file: it's a cap on the number of jobs that all run at the same
  time. If you have lots of jobs that run, for example, every morning, for the sake of your
  computer, you should probably offset them a little bit._
- `kwon` does not run jobs for longer than the tick rate of the main loop (default: 60 seconds).
  Jobs are designed to be relatively short-lived, repeatable actions. If you need something that
  runs longer, you're probably better off creating a standalone systemd service or similar rather
  than using `kwon`. If the process created by your job runs for longer than 60 seconds, it will be
  terminated with `SIGKILL`.

## Configuration Reference

[See the `Config` struct in `./src/config.rs` for more details](./src/config.rs). An example
configuration file is provided below. Please do not copy-paste the example. Please read the
doc-comments on the struct to understand each option. There aren't a lot of them.

```toml
log_file = "/var/log/kwon.log"
log_level = "debug"
tick_rate_seconds = 120

[jobs.my_thing]
executable = "/bin/zsh"
args = ["-lic", "echo $FOO"]
environment = { FOO = "bar" }
working_directory = "/home/myuser/foo/bar/"
interval_seconds = 3600
start_at = "2026-03-07T22:24:26.286Z"
```

## Local Development

**Pre-requisites**: [just](https://github.com/casey/just) as a task runner,
[uv](https://github.com/astral-sh/uv) as a Python toolchain manager, a container runtime (Docker or
Podman), and the `aarch64-unknown-linux-musl` Rust target
(`rustup target add aarch64-unknown-linux-musl`).

If you are on macOS or somewhere else that does not have the GNU linker, you need to install a cross
toolchain so that you can compile for the linux container `kwon` uses for its e2e tests on your host
system. For example, via Homebrew on macOS, `brew install filosottile/musl-cross/musl-cross`. Then
set the `linker` argument for the target in your `~/.cargo/config.toml` accordingly.

Run `just --list` to see all available commands. The typical workflow is:

```sh
just setup    # cross-compile + start the test container (slow, once)
just test     # run e2e tests against the running container (fast, repeatable)
just test     # run them again -- no rebuild or container recreation needed
just teardown # stop and remove the container when you're done
```

Other useful recipes:

```sh
just check    # fmt + lint + build (host target)
just retest   # rebuild the cross binary and re-run tests in one step
just e2e      # full setup → test → teardown cycle
```

Note that basically everything about kwon will work on macOS natively, except for the `systemd`
integration. One could probably port this software to macOS or any other \*nix without any trouble.

## License

MIT
