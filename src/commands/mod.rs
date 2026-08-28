//! Command implementations.
//!
//! Each module owns one subcommand and nothing else. Shared work -- resolving a manifest,
//! resolving a product, preparing a workspace -- lives in [`context`], so no command
//! reimplements the pipeline's opening moves.

pub mod clone;
pub mod context;
pub mod doctor;
pub mod products;
pub mod repos;
pub mod self_update;
pub mod status;
pub mod update;
