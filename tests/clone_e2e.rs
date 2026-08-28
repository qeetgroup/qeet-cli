//! The whole pipeline against the real `git` executable, with no network.
//!
//! Bare repositories are created on disk and cloned over `file://`. That exercises argument
//! construction, process spawning, concurrency, failure classification, cleanup and the
//! exit code exactly as a real run would -- but deterministically, with no credentials, and
//! identically on macOS, Linux and Windows.

mod common;

use common::Fixture;
use predicates::str::contains;

const EXIT_INCOMPLETE: i32 = 1;

/// The headline promise: one command, every repository.
#[test]
fn clones_every_repository_of_a_product() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for(
        "demo",
        &[
            ("alpha", fixture.bare_repo("alpha", &[])),
            ("beta", fixture.bare_repo("beta", &[])),
            ("gamma", fixture.bare_repo("gamma", &[])),
            ("delta", fixture.bare_repo("delta", &[])),
        ],
    );

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("Cloned:          4"))
        .stdout(contains("Failed:          0"));

    for name in ["alpha", "beta", "gamma", "delta"] {
        assert!(fixture.is_cloned(name), "{name} should have a working tree");
    }
}

/// Running it twice must be safe. This is the property that makes `qeet clone` usable as a
/// habit rather than a one-shot.
#[test]
fn a_second_run_changes_nothing_and_still_succeeds() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for(
        "demo",
        &[("alpha", fixture.bare_repo("alpha", &[])), ("beta", fixture.bare_repo("beta", &[]))],
    );

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    // Something the developer did after cloning must survive.
    let marker = fixture.path("alpha").join("work-in-progress.txt");
    std::fs::write(&marker, "do not lose me").expect("write");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("Already present: 2"))
        .stdout(contains("Cloned:          0"));

    assert_eq!(
        std::fs::read_to_string(&marker).expect("read"),
        "do not lose me",
        "a re-run must not touch existing work"
    );
}

/// One failure, four successes, exit 1 -- and no partial directory left behind.
#[test]
fn a_partial_failure_preserves_the_successes_and_exits_non_zero() {
    let fixture = Fixture::new();
    let absent = common::file_url(&fixture.remotes.join("absent.git"));
    let manifest = fixture.manifest_for(
        "demo",
        &[
            ("alpha", fixture.bare_repo("alpha", &[])),
            ("beta", fixture.bare_repo("beta", &[])),
            ("missing", absent),
            ("gamma", fixture.bare_repo("gamma", &[])),
            ("delta", fixture.bare_repo("delta", &[])),
        ],
    );

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stdout(contains("Cloned:          4"))
        .stdout(contains("Failed:          1"))
        .stderr(contains("missing (failed)"))
        // git's own words, not an invention of qeet's.
        .stderr(contains("does not appear to be a git repository"))
        .stderr(contains("Next steps:"));

    for name in ["alpha", "beta", "gamma", "delta"] {
        assert!(fixture.is_cloned(name), "{name} must survive a sibling's failure");
    }
    assert!(
        !fixture.path("missing").exists(),
        "a directory qeet created and did not finish must be cleaned up"
    );
}

/// A failed clone must not leave a half-populated directory that a later run would then
/// refuse to touch.
#[test]
fn a_failed_clone_leaves_nothing_behind_so_a_retry_can_succeed() {
    let fixture = Fixture::new();
    let bare = fixture.remotes.join("late.git");
    let manifest = fixture.manifest_for("demo", &[("late", common::file_url(&bare))]);

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE);
    assert!(!fixture.path("late").exists());

    // Now the remote exists. The same command must simply work.
    let created = fixture.bare_repo("late", &[]);
    assert_eq!(created, common::file_url(&bare), "sanity: same URL");

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();
    assert!(fixture.is_cloned("late"));
}

/// The concurrency bound is an option, not a behaviour change: the result is identical.
#[test]
fn the_result_is_the_same_at_every_concurrency() {
    for concurrency in ["1", "2", "8"] {
        let fixture = Fixture::new();
        let manifest = fixture.manifest_for(
            "demo",
            &[
                ("alpha", fixture.bare_repo("alpha", &[])),
                ("beta", fixture.bare_repo("beta", &[])),
                ("gamma", fixture.bare_repo("gamma", &[])),
            ],
        );

        fixture
            .qeet()
            .args(["clone", "demo", "--concurrency", concurrency, "--manifest"])
            .arg(&manifest)
            .assert()
            .success()
            .stdout(contains("Cloned:          3"));

        for name in ["alpha", "beta", "gamma"] {
            assert!(fixture.is_cloned(name), "{name} at concurrency {concurrency}");
        }
    }
}

/// `ref` pins a branch, and a `ref` that does not exist is reported as such rather than as
/// a generic failure.
#[test]
fn a_pinned_ref_is_honoured_and_a_bad_one_is_diagnosed() {
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &["release"]);

    let good = fixture.manifest(&format!(
        "schema = 1\n[remote]\nhost = \"h\"\nowner = \"o\"\nprotocol = \"https\"\n\
         [products.demo]\nname = \"Test Product\"\n\
         repositories = [{{ name = \"alpha\", url = \"{url}\", ref = \"release\" }}]\n"
    ));
    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&good).assert().success();

    let head = common::git(&fixture.path("alpha"), &["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("git should run");
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "release");

    // A ref that is not there is an InvalidRef, and is not retried.
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let bad = fixture.manifest(&format!(
        "schema = 1\n[remote]\nhost = \"h\"\nowner = \"o\"\nprotocol = \"https\"\n\
         [products.demo]\nname = \"Test Product\"\n\
         repositories = [{{ name = \"alpha\", url = \"{url}\", ref = \"nope\" }}]\n"
    ));
    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&bad)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stderr(contains("branch or tag"));
}

/// Every repository appears in the report, even a product with only one.
#[test]
fn a_single_repository_product_works() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("solo", &[("only", fixture.bare_repo("only", &[]))]);

    fixture
        .qeet()
        .args(["clone", "solo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("1 of 1 repositories"));
}

/// Non-interactive output must be deterministic and complete: no spinner escape codes, one
/// line per event. This is what CI logs look like.
#[test]
fn non_interactive_output_is_plain_and_complete() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for(
        "demo",
        &[("alpha", fixture.bare_repo("alpha", &[])), ("beta", fixture.bare_repo("beta", &[]))],
    );

    let output = fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cloning alpha..."), "{stderr}");
    assert!(stderr.contains("alpha: cloned"), "{stderr}");
    assert!(stderr.contains("beta: cloned"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "no terminal escape codes off a TTY: {stderr:?}");
    assert!(!stderr.contains('\r'), "no carriage returns off a TTY: {stderr:?}");
}
