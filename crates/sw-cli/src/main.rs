//! The `subwoofa` command line: encode, verify, StreamTools, Atmos and a few listings. Every run prints its version on
//! stderr first. The commands are thin: they build the same job the app builds and hand it to `sw-queue`.

mod args;
mod atmos;
mod downmix;
mod encode;
mod jobs;
mod listing;
mod loudness;
mod progress;
mod tools;
mod verify;
mod watch;

use args::{Cli, Cmd};
use clap::Parser;

// the system allocator on Windows takes a lock for every allocation, which stops the encoders scaling over cores
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // Every run says which build it is. On stderr, so stdout (and --json) stays clean for scripts.
    eprintln!("subwoofa {}", sw_core::version::display());
    if std::env::args().skip(1).any(|a| a == "-V" || a == "--version") {
        return;
    }
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match Cli::parse().cmd {
        Cmd::Layouts => listing::layouts(),
        Cmd::Formats => listing::formats(),
        Cmd::Info { path, start, duration } => listing::info(&path, start, duration)?,
        Cmd::StreamInfo { path, count, first } => listing::stream_info(&path, first, count)?,
        Cmd::Tools { op } => tools::run_tools(op)?,
        Cmd::Verify(args) => verify::verify_cmd(args)?,
        Cmd::Atmos { op } => atmos::run_atmos(op)?,
        Cmd::DownmixCheck(args) => downmix::run_downmix_check(args)?,
        Cmd::Loudness(args) => loudness::run_loudness(args)?,
        Cmd::Encode(args) => encode::run_encode(args)?,
        Cmd::Run(args) => jobs::run_jobs(args)?,
        Cmd::Watch(args) => watch::run_watch(args)?,
    }
    Ok(())
}
