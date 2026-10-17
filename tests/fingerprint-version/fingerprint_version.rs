//! llmlint's own version is part of `llmlint fingerprint`, proved through the
//! real CLI: the `llmlint` this workspace built and a second one built from a
//! scratch copy of this repository with only `Cargo.toml`'s package version
//! changed print different digests for the same config, plugin, prompt template,
//! harness configuration and stand-in oneharness — while two runs of one build
//! agree. It compiles a second binary, so it is its own project's `version-proof`
//! target (see this directory's `project.json`), outside the gate tiers and the
//! e2e suite; `just test-fingerprint-version` and the `Fingerprint version`
//! workflow run it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use llmlint::io::diff::git_command;
use tempfile::TempDir;

/// The package version the second build carries.
const OTHER_VERSION: &str = "999.0.0-fingerprint";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The fixture project: a local rule, a local plugin, a prompt template, and a
/// discovered oneharness config that `extends` a parent. Written here rather
/// than kept on disk, so the repository's own config discovery never meets it.
fn write_project(dir: &Path) {
    let files: [(&str, &str); 6] = [
        (
            "llmlint.yml",
            "version: 1\nfiles:\n  include: [\"src/**\"]\nplugins:\n  - ./team.yml\n\
             prompt_template: |\n  Judge {{ rules }} over {{ files }}.\nrules:\n  \
             - { name: local_rule, description: \"true when ok; false otherwise.\" }\n",
        ),
        (
            "team.yml",
            "rules:\n  - { name: team_rule, description: \"true when ok; false otherwise.\" }\n",
        ),
        (
            "oneharness.toml",
            "extends = \"harness/base.toml\"\nrun_mode = \"fallback\"\n\n[harness.codex]\nmodel = \"codex-model\"\n",
        ),
        ("harness/base.toml", "model = \"base-model\"\n"),
        ("src/lib.rs", "// code\n"),
        ("README.md", "# fixture\n"),
    ];
    for (rel, body) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
}

/// `<bin> fingerprint` in `project` against the stand-in harness, held to the
/// output contract (exit 0, one line of 64 lowercase hex).
fn fingerprint(bin: &Path, project: &Path) -> String {
    let mut cmd = Command::new(bin);
    cmd.current_dir(project)
        .arg("fingerprint")
        .arg("--oneharness-bin")
        .arg(cargo_bin("llmlint-mock-oneharness"));
    // Only the environment this test states: no ambient llmlint/oneharness
    // steering variable may reach either build.
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if key.starts_with("LLMLINT_") || key.starts_with("ONEHARNESS_") {
            cmd.env_remove(&name);
        }
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{} fingerprint: {stdout}{}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let digest = stdout.strip_suffix('\n').unwrap().to_string();
    assert_eq!(digest.len(), 64, "{stdout:?}");
    assert!(digest
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    digest
}

/// Copy every file git would carry for this checkout (tracked, plus untracked
/// files not ignored, so an uncommitted edit is built too) into `to`.
fn copy_checkout(root: &Path, to: &Path) {
    let out = git_command("git", root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git ls-files failed in {}",
        root.display()
    );
    for rel in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let rel = Path::new(std::str::from_utf8(rel).unwrap());
        let from = root.join(rel);
        // A tracked file deleted from the work tree is simply not there.
        if !from.is_file() {
            continue;
        }
        let dest = to.join(rel);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(&from, &dest).unwrap();
    }
}

/// Rewrite the `[package]` version in a `Cargo.toml`, and nothing else.
fn set_package_version(manifest: &Path, version: &str) -> String {
    let text = fs::read_to_string(manifest).unwrap();
    let mut in_package = false;
    let mut old = None;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.starts_with('[') {
                in_package = line.trim() == "[package]";
            }
            match line.strip_prefix("version = \"") {
                Some(rest) if in_package && old.is_none() => {
                    old = Some(rest.trim_end_matches('"').to_string());
                    format!("version = \"{version}\"")
                }
                _ => line.to_string(),
            }
        })
        .collect();
    let old = old.expect("Cargo.toml's [package] declares a version");
    fs::write(manifest, lines.join("\n") + "\n").unwrap();
    old
}

/// Build `llmlint` from `checkout` into its own target directory under this
/// workspace's `target/`, so the build caches across runs without ever touching
/// the binary the workspace itself built.
fn build_llmlint(checkout: &Path, root: &Path) -> PathBuf {
    let target = root.join("target").join("fingerprint-version");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(checkout)
        .env("CARGO_TARGET_DIR", &target)
        .args(["build", "--offline", "-p", "llmlint", "--bin", "llmlint"])
        .status()
        .unwrap();
    assert!(status.success(), "building the scratch llmlint failed");
    target
        .join("debug")
        .join(format!("llmlint{}", std::env::consts::EXE_SUFFIX))
}

fn version_line(bin: &Path) -> String {
    let out = Command::new(bin).arg("--version").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn llmlint_s_own_version_changes_the_digest() {
    let root = workspace_root();
    let built = cargo_bin("llmlint");

    let scratch = TempDir::new().unwrap();
    copy_checkout(&root, scratch.path());
    let version = set_package_version(&scratch.path().join("Cargo.toml"), OTHER_VERSION);
    assert_ne!(version, OTHER_VERSION);
    let other = build_llmlint(scratch.path(), &root);
    // The two builds differ in exactly the version they report.
    assert_eq!(version_line(&built), format!("llmlint {version}"));
    assert_eq!(version_line(&other), format!("llmlint {OTHER_VERSION}"));

    let project = TempDir::new().unwrap();
    write_project(project.path());
    let first = fingerprint(&built, project.path());
    assert_eq!(
        fingerprint(&built, project.path()),
        first,
        "one build must print one digest"
    );
    let second = fingerprint(&other, project.path());
    assert_eq!(fingerprint(&other, project.path()), second);
    assert_ne!(
        first, second,
        "builds differing only in their package version must differ"
    );
}
