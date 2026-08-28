//! The command-line surface, exercised through the real binary.

mod common;

use common::Fixture;
use predicates::prelude::*;
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

// ---------------------------------------------------------------------------------------
// The platform commands.
// ---------------------------------------------------------------------------------------

#[test]
fn products_lists_the_registry_with_counts() {
    Fixture::new()
        .qeet()
        .arg("products")
        .assert()
        .success()
        .stdout(contains("Qeet Products"))
        .stdout(contains("id"))
        .stdout(contains("Qeet ID"))
        .stdout(contains("qeet-id"))
        .stdout(contains("16 products"));
}

#[test]
fn repos_lists_a_product_and_where_it_lands() {
    Fixture::new()
        .qeet()
        .args(["repos", "id"])
        .assert()
        .success()
        .stdout(contains("Qeet ID"))
        .stdout(contains("into qeet-id/"))
        .stdout(contains("qeet-id-server"))
        // The verified name, not the illustrative `qeet-id-api` from the original brief.
        .stdout(predicates::str::contains("qeet-id-api").not());
}

#[test]
fn repos_honours_the_protocol_override() {
    Fixture::new()
        .qeet()
        .args(["repos", "id", "--protocol", "https"])
        .assert()
        .success()
        .stdout(contains("https://github.com/qeetgroup/qeet-id-server.git"));
}

#[test]
fn repos_rejects_an_unknown_product_like_clone_does() {
    Fixture::new()
        .qeet()
        .args(["repos", "nope"])
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("Unknown product: nope"));
}

/// `status` on an empty workspace reports everything missing, and says what to do.
#[test]
fn status_reports_repositories_that_are_not_cloned() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    fixture
        .qeet()
        .args(["status", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("not cloned"))
        .stdout(contains("qeet clone demo"));
}

/// The property that matters most: `update` must not touch a repository with uncommitted
/// work, and must say why it declined.
#[test]
fn update_refuses_a_dirty_repository_and_preserves_the_work() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    let precious = fixture.path("alpha").join("do-not-lose-me.txt");
    std::fs::write(&precious, "uncommitted work").expect("write");

    fixture
        .qeet()
        .args(["update", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("skipped"))
        .stdout(contains("uncommitted change"));

    assert_eq!(
        std::fs::read_to_string(&precious).expect("read"),
        "uncommitted work",
        "update must never touch uncommitted work"
    );
}

/// `--dry-run` must not fetch or merge anything at all.
#[test]
fn update_dry_run_changes_nothing() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    fixture
        .qeet()
        .args(["update", "demo", "--dry-run", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("dry run"))
        .stdout(contains("nothing was changed"));
}

#[test]
fn update_reports_a_repository_that_was_never_cloned() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest_for("demo", &[("alpha", fixture.bare_repo("alpha", &[]))]);

    fixture
        .qeet()
        .args(["update", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(contains("not cloned"));
}

#[test]
fn doctor_checks_the_environment_and_says_so() {
    Fixture::new()
        .qeet()
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains("qeet doctor"))
        .stdout(contains("git"))
        .stdout(contains("manifest"))
        .stdout(contains("workspace"));
}

#[test]
fn self_update_explains_the_route_without_overwriting_anything() {
    Fixture::new()
        .qeet()
        .arg("self-update")
        .assert()
        .success()
        .stdout(contains("qeet self-update"))
        .stdout(contains(env!("CARGO_PKG_VERSION")))
        // It must never claim to have updated itself.
        .stdout(predicates::str::contains("updated").not());
}

/// `clone all` walks every product in the manifest.
#[test]
fn clone_all_clones_every_product() {
    let fixture = Fixture::new();
    let alpha = fixture.bare_repo("alpha", &[]);
    let beta = fixture.bare_repo("beta", &[]);
    let manifest = fixture.manifest(&format!(
        "schema = 1\n\
         [remote]\nhost = \"h\"\nowner = \"o\"\nprotocol = \"https\"\n\
         [products.one]\nname = \"One\"\ndirectory = \"grp-one\"\n\
         repositories = [{{ name = \"alpha\", url = \"{alpha}\" }}]\n\
         [products.two]\nname = \"Two\"\ndirectory = \"grp-two\"\n\
         repositories = [{{ name = \"beta\", url = \"{beta}\" }}]\n"
    ));

    fixture.qeet().args(["clone", "all", "--manifest"]).arg(&manifest).assert().success();

    assert!(fixture.path("grp-one/alpha/.git").exists(), "product one cloned into its group");
    assert!(fixture.path("grp-two/beta/.git").exists(), "product two cloned into its group");
}

/// Grouping, end to end through the binary.
#[test]
fn clone_groups_repositories_under_the_product_directory() {
    let fixture = Fixture::new();
    let alpha = fixture.bare_repo("alpha", &[]);
    let manifest = fixture.manifest(&format!(
        "schema = 1\n\
         [remote]\nhost = \"h\"\nowner = \"o\"\nprotocol = \"https\"\n\
         [products.demo]\nname = \"Demo\"\ndirectory = \"qeet-demo\"\n\
         repositories = [{{ name = \"alpha\", url = \"{alpha}\" }}]\n"
    ));

    fixture.qeet().args(["clone", "demo", "--manifest"]).arg(&manifest).assert().success();

    assert!(fixture.path("qeet-demo/alpha/.git").exists(), "grouped");
    assert!(!fixture.path("alpha").exists(), "not also placed flat");
}
