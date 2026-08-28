//! `qeet` — a product-aware CLI for Qeet Group's polyrepo organization.
//!
//! This file is deliberately almost empty. Argument parsing lives in [`cli`], the pipeline
//! in [`commands::clone`], and every rule worth testing lives in a module of its own.

mod cli;
mod clone;
mod commands;
mod error;
mod git;
mod manifest;
mod output;
mod product;
mod remote;
mod workspace;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    cli::run().await
}
