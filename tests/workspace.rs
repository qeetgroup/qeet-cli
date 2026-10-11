//! Workspace safety, end to end: what qeet does when something is already there.
//!
//! The rule under test throughout: nothing that already exists is deleted or overwritten,
//! and when qeet cannot establish what a directory is, it refuses to touch it.

mod common;

use common::{AssertOk, Fixture, git};
use predicates::str::contains;

const EXIT_INCOMPLETE: i32 = 1;

/// The flat layout: a product with no directory lands directly in the workspace.
#[test]
fn repositories_land_directly_in_the_workspace() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for(
        "demo",
        &[("alpha", fixture.bare_repo("alpha", &[])), ("beta", fixture.bare_repo("beta", &[]))],
    );

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    assert!(fixture.is_cloned("alpha"));
    assert!(fixture.is_cloned("beta"));
    assert!(!fixture.path("demo").exists(), "no product directory is created");
}

/// Case B: an existing empty directory is a fine place to clone into.
#[test]
fn an_empty_directory_is_cloned_into() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);
    std::fs::create_dir(fixture.path("alpha")).expect("create empty dir");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("Cloned:          1"));

    assert!(fixture.is_cloned("alpha"));
}

/// Case D: occupied by something that is not a repository. The developer's file survives.
#[test]
fn a_non_repository_directory_blocks_and_is_left_alone() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    std::fs::create_dir(fixture.path("alpha")).expect("create dir");
    std::fs::write(fixture.path("alpha").join("notes.txt"), "my work").expect("write");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stderr(contains("not empty and is not a git repository"));

    assert_eq!(
        std::fs::read_to_string(fixture.path("alpha").join("notes.txt")).expect("read"),
        "my work",
        "the developer's file must be untouched"
    );
}

#[test]
fn a_file_in_the_way_blocks() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);
    std::fs::write(fixture.path("alpha"), "not a directory").expect("write");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stderr(contains("not a directory is in the way"));

    assert_eq!(std::fs::read_to_string(fixture.path("alpha")).expect("read"), "not a directory");
}

/// Case C': the directory holds a *different* repository. Both URLs are named so the
/// developer can see why qeet refused.
#[test]
fn a_different_repository_blocks_and_names_both_sides() {
    let fixture = Fixture::new();
    let alpha = fixture.bare_repo("alpha", &[]);
    let beta = fixture.bare_repo("beta", &[]);
    let manifest = fixture.manifest_for("demo", &[("alpha", alpha)]);

    // Put beta where alpha belongs.
    git(&fixture.work, &["clone", "--quiet", &beta, "alpha"]).assert_ok();
    let before = std::fs::read_to_string(fixture.path("alpha").join("README.md")).expect("read");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stderr(contains("a different repository is already here"))
        .stderr(contains("found:"))
        .stderr(contains("expected:"));

    assert_eq!(
        std::fs::read_to_string(fixture.path("alpha").join("README.md")).expect("read"),
        before,
        "the existing repository must be untouched"
    );
    assert!(before.contains("beta"), "sanity: beta was the one on disk");
}

/// A repository with no `origin` cannot be confirmed to be the right one, so it is refused
/// rather than assumed.
#[test]
fn a_repository_without_an_origin_is_not_assumed_to_match() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    let path = fixture.path("alpha");
    std::fs::create_dir(&path).expect("create");
    git(&path, &["init", "--quiet"]).assert_ok();
    std::fs::write(path.join("local.txt"), "local only").expect("write");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stderr(contains("no `origin` remote"));

    assert!(path.join("local.txt").exists(), "the local repository must survive");
}

/// One blocked repository must not stop the others.
#[test]
fn a_blocked_repository_does_not_stop_its_siblings() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for(
        "demo",
        &[
            ("alpha", fixture.bare_repo("alpha", &[])),
            ("blocked", fixture.bare_repo("blocked", &[])),
            ("gamma", fixture.bare_repo("gamma", &[])),
        ],
    );

    std::fs::create_dir(fixture.path("blocked")).expect("create");
    std::fs::write(fixture.path("blocked").join("mine.txt"), "keep").expect("write");

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_INCOMPLETE)
        .stdout(contains("Cloned:          2"))
        .stdout(contains("Failed:          1"));

    assert!(fixture.is_cloned("alpha"));
    assert!(fixture.is_cloned("gamma"));
    assert!(fixture.path("blocked").join("mine.txt").exists());
}

/// A `path` override creates the intermediate directories and clones into the leaf.
#[test]
fn a_custom_path_is_created_and_used() {
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let manifest = fixture.manifest(&format!(
        "schema = 1\n\
         [remote]\n\
         host = \"example.invalid\"\n\
         owner = \"fixture\"\n\
         protocol = \"https\"\n\
         [products.demo]\n\
         name = \"Test Product\"\n\
         repositories = [{{ name = \"alpha\", url = \"{url}\", path = \"services/api\" }}]\n"
    ));

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    assert!(fixture.path("services/api/.git").exists());
    assert!(!fixture.path("alpha").exists(), "the name must not also be used");
}

/// Re-running with a custom path is idempotent too: the destination is recognised.
#[test]
fn a_custom_path_is_idempotent() {
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let manifest = fixture.manifest(&format!(
        "schema = 1\n\
         [remote]\n\
         host = \"example.invalid\"\n\
         owner = \"fixture\"\n\
         protocol = \"https\"\n\
         [products.demo]\n\
         name = \"Test Product\"\n\
         repositories = [{{ name = \"alpha\", url = \"{url}\", path = \"services/api\" }}]\n"
    ));

    for _ in 0..2 {
        fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();
    }

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("Already present: 1"));
}

// ---------------------------------------------------------------------------------------
// Where the workspace is.
// ---------------------------------------------------------------------------------------

/// With nothing set, the workspace is `QG` in the home directory -- not wherever qeet runs.
#[test]
fn the_default_workspace_is_qg_in_the_home_directory() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    fixture
        .qeet()
        .env_remove("QEET_WORKSPACE")
        // What `std::env::home_dir` reads on Windows; `HOME` is already faked for Unix.
        .env("USERPROFILE", fixture.home())
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success();

    assert!(fixture.home().join("QG/alpha/.git").exists(), "should land in ~/QG");
    assert!(!fixture.path("alpha").exists(), "the current directory must not be used");
}

/// `QEET_WORKSPACE` moves it, and a first clone creates it, however deep.
#[test]
fn qeet_workspace_moves_it_and_the_first_clone_creates_it() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);
    let elsewhere = fixture.outside("elsewhere/nested");

    fixture
        .qeet()
        .env("QEET_WORKSPACE", &elsewhere)
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success();

    assert!(elsewhere.join("alpha/.git").exists(), "should land in QEET_WORKSPACE");
    assert!(!fixture.path("alpha").exists(), "the current directory must not be used");
}

/// Only `clone` creates the workspace. Looking at one that does not exist yet is not an
/// error -- nothing is cloned in it -- and must not create it as a side effect.
#[test]
fn looking_at_a_missing_workspace_reports_and_creates_nothing() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);
    let missing = fixture.outside("not-yet");

    for command in ["status", "update", "pull"] {
        fixture
            .qeet()
            .env("QEET_WORKSPACE", &missing)
            .args([command, "demo", "--manifest"])
            .arg(&manifest)
            .assert()
            .success()
            .stdout(contains("not cloned"));
    }
    assert!(!missing.exists(), "a read must not create the workspace");
}

/// A workspace that cannot be created is one clear startup error that names the path.
#[test]
fn a_workspace_that_cannot_be_created_is_a_clear_error() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);
    let file = fixture.outside("a-file");
    std::fs::write(&file, "in the way").expect("write");

    fixture
        .qeet()
        .env("QEET_WORKSPACE", &file)
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(3)
        .stderr(contains("cannot use the workspace"))
        .stderr(contains("QEET_WORKSPACE"))
        .stderr(contains("a-file"));
}
