//! Manifest resolution and validation, seen the way a developer sees it: as CLI output.

mod common;

use std::path::PathBuf;

use common::Fixture;
use predicates::str::contains;

const EXIT_INCOMPLETE: i32 = 1;
const EXIT_CONFIG: i32 = 3;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// A valid manifest gets all the way to cloning. The clones then fail because the fixture
/// points at repositories that do not exist -- which is exactly the point: reaching a clone
/// failure proves validation passed.
#[test]
fn a_valid_manifest_reaches_the_clone_stage() {
    Fixture::new()
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(fixture_path("valid.toml"))
        .assert()
        .code(EXIT_INCOMPLETE)
        .stdout(contains("Demo Product"))
        .stderr(contains("cloning plain"));
}

#[test]
fn malformed_toml_names_a_line() {
    Fixture::new()
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(fixture_path("invalid.toml"))
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("invalid manifest"))
        .stderr(contains("line 6"));
}

/// All problems in one pass: fixing a 66-repository manifest one error per run would be
/// miserable.
#[test]
fn every_validation_problem_is_reported_at_once() {
    Fixture::new()
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(fixture_path("collision.toml"))
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("same destination `shared`"))
        .stderr(contains("duplicate repository `first`"))
        .stderr(contains("remote-helper transport"));
}

#[test]
fn an_explicitly_requested_manifest_that_is_missing_is_an_error() {
    let fixture = Fixture::new();
    let absent = fixture.path("nowhere/products.toml");

    // Never a silent fall back to the built-in registry: that would hide the mistake.
    fixture
        .qeet()
        .args(["clone", "id", "--manifest"])
        .arg(&absent)
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("cannot read manifest"));

    fixture
        .qeet()
        .env("QEET_MANIFEST", &absent)
        .args(["clone", "id"])
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("cannot read manifest"));
}

#[test]
fn the_flag_takes_precedence_over_the_environment_variable() {
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let chosen = fixture.manifest_for("fromflag", &[("alpha", url)]);

    fixture
        .qeet()
        .env("QEET_MANIFEST", fixture_path("collision.toml"))
        .args(["clone", "fromflag", "--manifest"])
        .arg(&chosen)
        .assert()
        .success();
}

#[test]
fn a_non_default_manifest_is_named_in_the_output() {
    // Knowing which manifest is in effect is the difference between a five-second and a
    // fifty-minute debugging session.
    let fixture = Fixture::new();
    let url = fixture.bare_repo("alpha", &[]);
    let manifest = fixture.manifest_for("demo", &[("alpha", url)]);

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stderr(contains("manifest: --manifest"));
}

#[test]
fn an_unsupported_schema_is_refused_rather_than_guessed_at() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest(
        "schema = 99\n\
         [remote]\n\
         host = \"github.com\"\n\
         owner = \"qeetgroup\"\n\
         protocol = \"ssh\"\n\
         [products.demo]\n\
         name = \"Demo\"\n\
         repositories = [{ name = \"a\" }]\n",
    );

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("unsupported manifest schema 99"));
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest(
        "schema = 1\n\
         [remote]\n\
         host = \"github.com\"\n\
         owner = \"qeetgroup\"\n\
         protocol = \"ssh\"\n\
         [products.demo]\n\
         name = \"Demo\"\n\
         repositories = [{ name = \"a\", privte = true }]\n",
    );

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("unknown field"));
}

/// A manifest path that escapes the workspace is refused before anything is created.
#[test]
fn a_path_escaping_the_workspace_is_refused() {
    let fixture = Fixture::new();
    let manifest = fixture.manifest(
        "schema = 1\n\
         [remote]\n\
         host = \"github.com\"\n\
         owner = \"qeetgroup\"\n\
         protocol = \"ssh\"\n\
         [products.demo]\n\
         name = \"Demo\"\n\
         repositories = [{ name = \"a\", path = \"../escaped\" }]\n",
    );

    fixture
        .qeet()
        .args(["clone", "demo", "--manifest"])
        .arg(&manifest)
        .assert()
        .code(EXIT_CONFIG)
        .stderr(contains("contains `..`"));

    assert!(
        !fixture.work().parent().expect("parent").join("escaped").exists(),
        "nothing may be created outside the workspace"
    );
}

/// Re-verify the shipped registry against the live organization.
///
/// Ignored by default: it needs network access and an authenticated `gh`, and the
/// organization standard is that tests do not depend on the network. Run deliberately:
///
/// ```text
/// cargo test --test manifest -- --ignored
/// ```
#[test]
#[ignore = "requires network access and an authenticated gh CLI"]
fn the_shipped_registry_matches_the_organization() {
    use std::collections::BTreeSet;

    // Repositories that exist in the organization but belong to no product, each with a
    // reason. Anything else new must be added to config/products.toml.
    const EXCLUDED: &[(&str, &str)] = &[
        ("qeetrix", "archived historical monorepo, superseded by the qeetrix-* repositories"),
        (".github", "organization profile; a `.github` clone directory would be hidden"),
    ];

    let output = std::process::Command::new("gh")
        .args(["repo", "list", "qeetgroup", "--limit", "500", "--json", "name"])
        .output()
        .expect("the gh CLI must be installed to run this test");
    assert!(
        output.status.success(),
        "gh repo list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let listing = String::from_utf8_lossy(&output.stdout);
    // Avoiding a JSON dependency for one shape: `[{"name":"x"},...]`.
    let actual: BTreeSet<String> = listing
        .split("\"name\":\"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').next())
        .map(str::to_string)
        .collect();
    assert!(!actual.is_empty(), "gh returned no repositories: {listing}");

    let manifest = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/products.toml"),
    )
    .expect("read the shipped manifest");
    let shipped: BTreeSet<String> = manifest
        .split("{ name = \"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').next())
        .map(str::to_string)
        .collect();

    let excluded: BTreeSet<String> = EXCLUDED.iter().map(|(name, _)| name.to_string()).collect();

    let missing: Vec<&String> = shipped.difference(&actual).collect();
    assert!(missing.is_empty(), "shipped but no longer in the organization: {missing:?}");

    let unmapped: Vec<&String> =
        actual.difference(&shipped).filter(|n| !excluded.contains(*n)).collect();
    assert!(unmapped.is_empty(), "in the organization but in no product: {unmapped:?}");

    let gone: Vec<&String> = excluded.difference(&actual).collect();
    assert!(gone.is_empty(), "excluded but no longer exists: {gone:?}");
}
