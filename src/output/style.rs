//! Colour.
//!
//! One palette, used everywhere, so the CLI reads as a single tool rather than each command
//! inventing its own look.
//!
//! Every helper goes through [`console::style`], which decides on its own whether to emit
//! escape codes: it honours `NO_COLOR`, `CLICOLOR`, `TERM=dumb`, and whether the stream is a
//! terminal. That detection is not reimplemented here — getting it wrong means either a
//! colourless terminal or a log file full of escape sequences.
//!
//! Colour is never the only signal. Every state that has a colour also has a symbol and a
//! word, so the output survives being piped, screen-read, or viewed by someone who cannot
//! distinguish red from green.

use std::fmt::Display;

use console::{Style, StyledObject};

/// Success: a clone that worked, a check that passed.
pub fn ok<D: Display>(value: D) -> StyledObject<D> {
    Style::new().green().apply_to(value)
}

/// Failure: a clone that failed, a check that did not pass.
pub fn bad<D: Display>(value: D) -> StyledObject<D> {
    Style::new().red().apply_to(value)
}

/// Something the developer should look at, but which is not a failure — a skipped
/// repository, a blocked destination, a warning from `doctor`.
pub fn warn<D: Display>(value: D) -> StyledObject<D> {
    Style::new().yellow().apply_to(value)
}

/// Identifiers: product keys, repository names, paths.
pub fn name<D: Display>(value: D) -> StyledObject<D> {
    Style::new().cyan().apply_to(value)
}

/// Headings.
pub fn heading<D: Display>(value: D) -> StyledObject<D> {
    Style::new().bold().apply_to(value)
}

/// Secondary detail that should not compete with the line it annotates.
pub fn dim<D: Display>(value: D) -> StyledObject<D> {
    Style::new().dim().apply_to(value)
}

/// The symbol for a state. Paired with colour, never replaced by it.
pub mod symbol {
    pub const OK: &str = "✓";
    pub const BAD: &str = "✗";
    pub const SKIP: &str = "·";
    pub const PENDING: &str = "○";
    pub const WARN: &str = "!";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helpers must not panic or mangle their input, whatever the environment decides
    /// about colour. Under `cargo test` stdout is not a terminal, so these render bare.
    #[test]
    fn every_helper_preserves_its_content() {
        for rendered in [
            ok("cloned").to_string(),
            bad("failed").to_string(),
            warn("skipped").to_string(),
            name("qeet-id-server").to_string(),
            heading("Qeet ID").to_string(),
            dim("2 uncommitted").to_string(),
        ] {
            assert!(!rendered.is_empty());
        }
        assert!(ok("cloned").to_string().contains("cloned"));
        assert!(name("qeet-id-server").to_string().contains("qeet-id-server"));
    }

    #[test]
    fn symbols_are_distinct() {
        let all = [symbol::OK, symbol::BAD, symbol::SKIP, symbol::PENDING, symbol::WARN];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "{all:?}");
    }
}
