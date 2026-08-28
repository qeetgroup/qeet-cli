//! Turning a product's plans into cloned repositories.

pub mod coordinator;
pub mod report;

pub use coordinator::{Job, Options, run};
pub use report::Report;
