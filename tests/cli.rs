//! The command-line surface, exercised through the real binary.

mod common;

use common::Fixture;
use predicates::str::contains;

/// Documented in the README and depended on by scripts.
const EXIT_INCOMPLETE: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_CONFIG: i32 = 3;

#[test]
fn help_succeeds_and_explains_the_problem() {
    Fixture::new()
        .qeet()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("qeet clone id"))
        .stdout(contains("polyrepo"))
        .stdout(contains("clone"));
}

#[test]
fn version_prints_the_package_version() {
    Fixture::new()
        .qeet()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn clone_help_documents_every_option() {
    Fixture::new()
        .qeet()
        .args(["clone", "--help"])
        .assert()
        .success()
        .stdout(contains("--concurrency"))
        .stdout(contains("--protocol"))
        .stdout(contains("--manifest"));
}

/// The built-in registry is what makes the installed binary work with no setup, so the
/// product list must come from it.
#[test]
fn an_unknown_product_lists_the_real_registry() {
    Fixture::new()
        .qeet()
        .args(["clone", "definitely-not-a-product"])
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("Unknown product: definitely-not-a-product"))
        .stderr(contains("Available products:"))
        .stderr(contains("  id"))
        .stderr(contains("  pay"))
        .stderr(contains("  logs"))
        .stderr(contains("  notify"))
        .stderr(contains("  people"));
}

#[test]
fn a_near_miss_is_suggested() {
    Fixture::new()
        .qeet()
        .args(["clone", "poeple"])
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("Did you mean `people`?"));
}

#[test]
fn a_product_argument_is_required() {
    Fixture::new().qeet().arg("clone").assert().code(EXIT_USAGE);
}

/// Guards the v1 scope boundary. These verbs are deliberately not implemented.
#[test]
fn the_deferred_subcommands_do_not_exist() {
    for absent in ["status", "pull", "sync", "graph", "dev"] {
        Fixture::new().qeet().arg(absent).assert().code(EXIT_USAGE);
    }
}

#[test]
fn concurrency_must_be_a_positive_bounded_number() {
    for (value, expected) in [
        ("0", "at least 1"),
        ("lots", "is not a whole number"),
        ("1.5", "is not a whole number"),
        ("100000", "at most 64"),
        // clap rejects a leading `-` as an unknown flag before the parser sees it, which
        // is the same refusal by a different route.
        ("-3", "unexpected argument"),
    ] {
        Fixture::new()
            .qeet()
            .args(["clone", "id", "--concurrency", value])
            .assert()
            .code(EXIT_USAGE)
            .stderr(contains(expected));
    }
}

#[test]
fn an_unknown_protocol_is_refused_with_the_choices() {
    Fixture::new()
        .qeet()
        .args(["clone", "id", "--protocol", "ftp"])
        .assert()
        .code(EXIT_USAGE)
        .stderr(contains("ssh"))
        .stderr(contains("https"));
}

/// Progress belongs on stderr; stdout carries the result. That is what makes
/// `qeet clone id > summary.txt` useful.
#[test]
fn the_summary_goes_to_stdout_and_progress_to_stderr() {
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let manifest = fixture.manifest_for("demo", &[("alpha", url)]);

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("Cloned:"))
        .stdout(contains("Test Product"))
        .stderr(contains("cloning alpha"));
}

/// A failing clone must still exit non-zero even though qeet itself worked fine.
#[test]
fn an_incomplete_run_exits_non_zero() {
    let fixture = Fixture::new();
    let manifest = fixture
        .manifest_for("demo", &[("absent", common::file_url(&fixture.remotes.join("absent.git")))]);

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE);
}
