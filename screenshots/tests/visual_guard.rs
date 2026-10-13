//! The screenshot script journeys: the lane helper
//! (`host-arch.sh`), the baseline blesser (`bless-baseline.sh`), and CI's
//! `freeze` installer (`ci-install-freeze.sh`), each driven the way a
//! developer, or the Visual docs workflow runs it, with only third-party tools
//! and servers stood in. Offline and model-free: this is the `screenshots`
//! project's `test` target. The capture itself (`screenshots.sh`, which needs
//! `freeze`) is its `capture` target, run by the Visual docs workflow.

// llmlint: ignore-file[shell_test_tiers_stay_split] every journey here is offline and hermetic: installers fetch stand-in releases over file:// into scratch directories, and the only host tools used (just, curl, tar, unzip, install, sha256sum) are the gate's own required tools or the base system's, so no journey reaches a network or installs a real version; they exercise only the scripts this project owns, so a separate project would be selected by exactly the same edits

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use tempfile::TempDir;

/// The repository root: this crate's manifest sits one level below it, and
/// every path these journeys read or drive is relative to the root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the repository root")
        .to_path_buf()
}

#[cfg(unix)]
/// A throwaway directory with helpers to write files into it.
struct Project {
    dir: TempDir,
}

#[cfg(unix)]
impl Project {
    fn new() -> Self {
        Project {
            dir: TempDir::new().unwrap(),
        }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, rel: &str, contents: &str) -> &Self {
        let p = self.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
        self
    }
}

#[cfg(unix)]
/// Drop every inherited `GIT_*` variable from a command. git's
/// repository-selection variables outrank `-C`, and a `pre-push` hook (where the
/// repository's own gate runs) exports `GIT_DIR` and `GIT_INDEX_FILE` for the
/// repository it fired in — so a scratch repo built or diffed with them inherited
/// is not a scratch repo at all. Sweeping the prefix covers every such variable
/// (a superset of llmlint's own `AMBIENT_REPOSITORY_VARS`) with no list to drift.
fn clear_git_env(cmd: &mut std::process::Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&name);
        }
    }
}

/// Every lane `screencomp.toml` declares under `[capture].arches`, read
/// independently of the hook's own parsing.
fn declared_capture_lanes(toml: &str) -> Vec<String> {
    let arches = toml
        .lines()
        .find_map(|l| l.strip_prefix("arches = ["))
        .expect("screencomp.toml declares [capture].arches");
    arches
        .split(']')
        .next()
        .unwrap()
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect()
}

/// Every lane declared in `[capture].arches` has a committed baseline manifest,
/// and — under the identical-bytes contract screencomp.toml states — they carry
/// the same shots. A lane whose baseline never landed would fail its CI job.
#[test]
fn every_declared_capture_lane_has_a_committed_baseline() {
    let root = repo_root();
    let toml = fs::read_to_string(root.join("screencomp.toml")).unwrap();
    let lanes = declared_capture_lanes(&toml);
    assert!(lanes.len() >= 2, "expected several lanes; got {lanes:?}");
    let manifests: Vec<(String, String)> = lanes
        .iter()
        .map(|lane| {
            let path = root.join(format!("shots/baseline/{lane}.json"));
            let body = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("no baseline for declared lane {lane}: {e}"));
            (lane.clone(), body)
        })
        .collect();
    let (first_lane, first) = &manifests[0];
    for (lane, body) in &manifests[1..] {
        assert_eq!(
            body, first,
            "shots/baseline/{lane}.json must carry the same shots as \
             shots/baseline/{first_lane}.json (the SVGs are byte-identical across arches)"
        );
    }
}

// llmlint: ignore-block[e2e_not_mocked] the real script runs with real curl/tar/install; a test cannot own the runner's CPU or charmbracelet's release server, so only those two are stood in
/// The `freeze` version `screenshots/ci-install-freeze.sh` pins, read from the script
/// so these journeys follow a pin bump instead of going stale.
#[cfg(unix)]
fn ci_freeze_version() -> String {
    let path = repo_root().join("screenshots/ci-install-freeze.sh");
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .find_map(|l| l.trim().strip_prefix("freeze_version="))
        .expect("screenshots/ci-install-freeze.sh pins freeze_version")
        .trim_matches('"')
        .to_string()
}

/// How a stand-in release tarball is built, so a journey can drive the script's
/// validation paths as well as its happy one.
#[cfg(unix)]
enum StandIn {
    /// A well-formed archive whose `freeze` prints the asset it came out of.
    Good,
    /// Well-formed, but the pinned digest names different bytes (a tampered or
    /// truncated download).
    WrongDigest,
    /// Matches its pinned digest, but upstream moved the binary out of the stem
    /// directory.
    NoBinary,
}

/// Drive the real `screenshots/ci-install-freeze.sh` on a host whose `uname -m` says
/// `raw_arch`, against a stand-in release tree served over `file://`: one tarball
/// per arch freeze publishes for Linux, each carrying a `freeze` that prints the
/// asset it came out of, plus a pinned-digest file in the release's own
/// `checksums.txt` format. Returns the script's output and the scratch project, so
/// a journey can ask the INSTALLED binary which asset was chosen (`out/freeze`).
#[cfg(unix)]
fn run_ci_install_freeze(raw_arch: &str, kind: StandIn) -> (std::process::Output, Project) {
    use std::os::unix::fs::PermissionsExt;
    let version = ci_freeze_version();
    let p = Project::new();
    let releases = p.path().join(format!("releases/v{version}"));
    fs::create_dir_all(&releases).unwrap();
    let staging = p.path().join("staging");
    let mut sums = String::new();
    for asset_arch in ["x86_64", "arm64"] {
        let stem = format!("freeze_{version}_Linux_{asset_arch}");
        let inner = match kind {
            StandIn::NoBinary => "README.md",
            _ => "freeze",
        };
        let bin = staging.join(&stem).join(inner);
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, format!("#!/usr/bin/env bash\nprintf '{stem}\\n'\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let archive = releases.join(format!("{stem}.tar.gz"));
        let tar = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .arg(&stem)
            .status()
            .unwrap();
        assert!(tar.success(), "packing the stand-in {stem} release");
        let digest = match kind {
            StandIn::WrongDigest => "0".repeat(64),
            _ => sha256_hex(&archive),
        };
        sums.push_str(&format!("{digest}  {stem}.tar.gz\n"));
    }
    let sums_file = p.path().join("freeze.sha256");
    fs::write(&sums_file, &sums).unwrap();

    let stub_dir = p.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let uname = stub_dir.join("uname");
    fs::write(
        &uname,
        format!("#!/usr/bin/env bash\nprintf '{raw_arch}\\n'\n"),
    )
    .unwrap();
    fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).unwrap();

    let out = std::process::Command::new("bash")
        .arg(repo_root().join("screenshots/ci-install-freeze.sh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                stub_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env(
            "FREEZE_BASE_URL",
            format!("file://{}", p.path().join("releases").display()),
        )
        .env("FREEZE_INSTALL_DIR", p.path().join("out"))
        .env("FREEZE_SHA256_FILE", &sums_file)
        .output()
        .unwrap();
    (out, p)
}

/// The SHA-256 of a file as lowercase hex, computed the way the script does.
#[cfg(unix)]
fn sha256_hex(path: &Path) -> String {
    let out = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .or_else(|_| {
            std::process::Command::new("shasum")
                .args(["-a", "256"])
                .arg(path)
                .output()
        })
        .expect("a sha256 tool");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

#[cfg(unix)]
#[test]
fn ci_install_freeze_installs_the_build_matching_the_runner_architecture() {
    // CI captures one lane per [capture].arches entry and runs the arm64 lane on
    // an arm64 runner, so the capture step must fetch the freeze release for the
    // runner it is on — a hard-coded x86_64 asset simply will not execute there.
    for (raw_arch, asset_arch) in [
        ("x86_64", "x86_64"),
        ("amd64", "x86_64"),
        ("aarch64", "arm64"),
        ("arm64", "arm64"),
    ] {
        let (out, p) = run_ci_install_freeze(raw_arch, StandIn::Good);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{raw_arch}: {stderr}");
        let installed = p.path().join("out/freeze");
        assert!(
            installed.is_file(),
            "{raw_arch}: nothing installed: {stderr}"
        );
        let said = String::from_utf8_lossy(
            &std::process::Command::new(&installed)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert_eq!(
            said,
            format!("freeze_{}_Linux_{asset_arch}", ci_freeze_version()),
            "{raw_arch}: installed the wrong release asset"
        );
    }
}

#[cfg(unix)]
#[test]
fn ci_install_freeze_refuses_an_architecture_with_no_pinned_build() {
    // freeze publishes Linux x86_64 and arm64; on anything else the script names
    // the architecture and exits non-zero, rather than fetching a URL that does
    // not exist and leaving the capture to fail later on a confusing tar error.
    let (out, p) = run_ci_install_freeze("riscv64", StandIn::Good);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("riscv64"), "{stderr}");
    assert!(
        stderr.contains("just screenshots-tools"),
        "no next action: {stderr}"
    );
    assert!(!p.path().join("out/freeze").exists(), "installed anyway");
}

#[cfg(unix)]
#[test]
fn ci_install_freeze_refuses_an_archive_that_misses_its_pinned_digest() {
    // The digests are pinned in this repository, not fetched beside the archive,
    // so a tampered or truncated download is refused before it is unpacked —
    // never installed and then discovered later.
    let (out, p) = run_ci_install_freeze("x86_64", StandIn::WrongDigest);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("sha256 mismatch"), "{stderr}");
    assert!(stderr.contains("checksums.txt"), "no next action: {stderr}");
    assert!(!p.path().join("out/freeze").exists(), "installed anyway");
}

#[cfg(unix)]
#[test]
fn ci_install_freeze_refuses_an_archive_with_no_freeze_binary() {
    // An archive that matches its pin but no longer carries <stem>/freeze means
    // upstream moved the layout: say so and stop, rather than leaving `install`
    // to fail with a bare "cannot stat" the capture step cannot act on.
    let (out, p) = run_ci_install_freeze("aarch64", StandIn::NoBinary);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("archive layout"), "{stderr}");
    assert!(!p.path().join("out/freeze").exists(), "installed anyway");
}

/// The digests `screenshots/ci-install-freeze.sh` pins cover both Linux assets of the
/// version it installs — a missing line fails the capture at the digest check, on
/// the lane whose asset was never pinned.
#[cfg(unix)]
#[test]
fn pinned_freeze_digests_cover_every_installable_asset() {
    let root = repo_root();
    let sums = fs::read_to_string(root.join("screenshots/freeze.sha256")).unwrap();
    let version = ci_freeze_version();
    for asset_arch in ["x86_64", "arm64"] {
        let want = format!("freeze_{version}_Linux_{asset_arch}.tar.gz");
        let line = sums
            .lines()
            .find(|l| l.split_whitespace().nth(1) == Some(want.as_str()))
            .unwrap_or_else(|| panic!("screenshots/freeze.sha256 pins no digest for {want}"));
        let digest = line.split_whitespace().next().unwrap();
        assert_eq!(digest.len(), 64, "not a sha256 for {want}: {digest}");
        assert!(
            digest.chars().all(|c| c.is_ascii_hexdigit()),
            "not hex for {want}: {digest}"
        );
    }
}

#[cfg(unix)]
#[test]
fn ci_install_freeze_refuses_a_malformed_override_before_fetching() {
    // The three overrides steer a download and two filesystem paths, so a bad one
    // must fail with its own name attached rather than deep inside curl or awk.
    for (var, value, needle) in [
        (
            "FREEZE_BASE_URL",
            "ftp://example.invalid",
            "FREEZE_BASE_URL",
        ),
        // A scheme with no host or path, or a value carrying whitespace.
        ("FREEZE_BASE_URL", "https://", "FREEZE_BASE_URL"),
        ("FREEZE_BASE_URL", "file://", "FREEZE_BASE_URL"),
        (
            "FREEZE_BASE_URL",
            "https://example.invalid/a b",
            "FREEZE_BASE_URL",
        ),
        ("FREEZE_BASE_URL", "file:///srv/a b", "FREEZE_BASE_URL"),
        ("FREEZE_INSTALL_DIR", "", "FREEZE_INSTALL_DIR"),
        ("FREEZE_INSTALL_DIR", "relative/bin", "FREEZE_INSTALL_DIR"),
        (
            "FREEZE_SHA256_FILE",
            "/nonexistent/freeze.sha256",
            "digest pin file",
        ),
    ] {
        let p = Project::new();
        let stub_dir = p.path().join("bin");
        fs::create_dir_all(&stub_dir).unwrap();
        fs::write(
            stub_dir.join("uname"),
            "#!/usr/bin/env bash\nprintf 'x86_64\\n'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(stub_dir.join("uname"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = std::process::Command::new("bash")
            .arg(repo_root().join("screenshots/ci-install-freeze.sh"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    stub_dir.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("FREEZE_INSTALL_DIR", p.path().join("out"))
            .env(var, value)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{var}: {stderr}");
        assert!(stderr.contains(needle), "{var}: {stderr}");
        assert!(
            !p.path().join("out/freeze").exists(),
            "{var}: installed anyway"
        );
    }
}

#[cfg(unix)]
#[test]
fn bless_baseline_refuses_when_there_is_no_capture_to_bless() {
    // `just screenshots-bless` runs this script; with
    // no capture in shots/current there is nothing to write a manifest from, so it
    // says so instead of handing screencomp a path that is not there.
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    let root = repo_root();
    for helper in ["screenshots/host-arch.sh", "screenshots/bless-baseline.sh"] {
        p.write(helper, &fs::read_to_string(root.join(helper)).unwrap());
    }
    let calls = p.path().join("calls");
    p.write(
        "bin/screencomp",
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$STUB_CALLS\"\n",
    );
    fs::set_permissions(
        p.path().join("bin/screencomp"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let out = std::process::Command::new("bash")
        .arg("screenshots/bless-baseline.sh")
        .current_dir(p.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                p.path().join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("STUB_CALLS", &calls)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no capture to bless"), "{stderr}");
    assert!(
        stderr.contains("just screenshots"),
        "no next action: {stderr}"
    );
    assert!(
        !calls.exists(),
        "screencomp should not have been called: {:?}",
        fs::read_to_string(&calls)
    );
}

/// CI's installer and `just screenshots-tools` must pin the SAME freeze: the
/// vendored-font SVGs reflow when the renderer changes, so a drifted pin would
/// make every local capture disagree with the baseline CI classifies against.
#[cfg(unix)]
#[test]
fn ci_install_freeze_pins_the_version_the_justfile_pins() {
    let justfile = fs::read_to_string(repo_root().join("justfile")).unwrap();
    let pinned = justfile
        .lines()
        .find_map(|l| l.strip_prefix("freeze-version := "))
        .expect("the justfile pins freeze-version")
        .trim()
        .trim_matches('"');
    assert_eq!(ci_freeze_version(), pinned);
}
// llmlint: ignore-end[e2e_not_mocked]

#[cfg(unix)]
/// The real `screenshots.sh` with `freeze` absent from PATH, capturing into
/// `shots_out`: it stops at the capture-directory guards or at the `freeze`
/// check, before it builds or renders anything.
fn capture_into(shots_out: &str) -> std::process::Output {
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(repo_root().join("screenshots/screenshots.sh"))
        .env("PATH", "/usr/bin:/bin")
        .env("SHOTS_OUT", shots_out);
    clear_git_env(&mut cmd);
    cmd.output().unwrap()
}

#[cfg(unix)]
/// A scratch directory inside the repository's (gitignored) `shots/current/`,
/// the only tree the capture may delete from, removed when dropped; the second
/// value is its path relative to the repository root, as SHOTS_OUT names it.
fn shots_scratch() -> (TempDir, String) {
    let current = repo_root().join("shots/current");
    fs::create_dir_all(&current).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("guard-test-")
        .tempdir_in(&current)
        .unwrap();
    let rel = format!(
        "shots/current/{}",
        dir.path().file_name().unwrap().to_str().unwrap()
    );
    (dir, rel)
}

#[cfg(unix)]
#[test]
fn the_capture_refuses_to_delete_a_shots_out_that_is_not_a_capture_directory() {
    let (scratch, rel) = shots_scratch();
    fs::create_dir_all(scratch.path().join("unrelated")).unwrap();
    fs::write(scratch.path().join("unrelated/keep.txt"), "not a capture\n").unwrap();
    let out = capture_into(&format!("{rel}/unrelated"));
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("holds files but no captures.json") && err.contains("SHOTS_OUT="),
        "{err}"
    );
    assert!(
        scratch.path().join("unrelated/keep.txt").exists(),
        "nothing deleted"
    );

    // A previous capture, an empty directory and an absent one pass the guard,
    // named relative to the repository root or absolutely under it: each run goes
    // on to the freeze check, the next step.
    fs::create_dir_all(scratch.path().join("previous")).unwrap();
    fs::write(scratch.path().join("previous/captures.json"), "{}\n").unwrap();
    fs::create_dir_all(scratch.path().join("empty")).unwrap();
    for dir in ["previous", "empty", "absent"] {
        for shots_out in [
            format!("{rel}/{dir}"),
            scratch.path().join(dir).display().to_string(),
        ] {
            let out = capture_into(&shots_out);
            let err = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{shots_out}: {out:?}");
            assert!(err.contains("'freeze' not on PATH"), "{shots_out}: {err}");
        }
    }
}

#[cfg(unix)]
#[test]
fn the_capture_refuses_a_shots_out_outside_the_shots_tree() {
    // The capture deletes SHOTS_OUT before writing, so an override naming any
    // directory outside the repository's shots/ tree — elsewhere on disk, the
    // repository's own sources, or a path that walks back out with `..` — is
    // refused by name before anything is deleted, even one shaped like a capture.
    let outside = Project::new();
    outside.write("captures.json", "{}\n");
    let (_scratch, rel) = shots_scratch();
    for shots_out in [
        outside.path().display().to_string(),
        "src".to_owned(),
        "shots".to_owned(),
        format!("{rel}/../../../src"),
        "shots/./current".to_owned(),
        String::new(),
    ] {
        let out = capture_into(&shots_out);
        let err = String::from_utf8_lossy(&out.stderr);
        // An empty SHOTS_OUT falls back to this host's lane, which passes.
        if shots_out.is_empty() {
            assert!(err.contains("'freeze' not on PATH"), "empty: {err}");
            continue;
        }
        assert_eq!(out.status.code(), Some(1), "{shots_out}: {out:?}");
        assert!(
            err.contains("SHOTS_OUT must name a directory inside this repository's shots/"),
            "{shots_out}: {err}"
        );
    }
    // A symlinked step that leads out of the tree is refused too, even though
    // the path reads as inside shots/ and its target looks like a capture.
    let (scratch, rel) = shots_scratch();
    std::os::unix::fs::symlink(outside.path(), scratch.path().join("link")).unwrap();
    outside.write("capture/captures.json", "{}\n");
    for shots_out in [
        format!("{rel}/link"),
        format!("{rel}/link/capture"),
        format!("{rel}/link/new/dir"),
    ] {
        let out = capture_into(&shots_out);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{shots_out}: {out:?}");
        assert!(
            err.contains("SHOTS_OUT must name a directory inside this repository's shots/"),
            "{shots_out}: {err}"
        );
    }
    assert!(
        outside.path().join("captures.json").exists(),
        "nothing deleted"
    );
    assert!(
        outside.path().join("capture/captures.json").exists(),
        "nothing deleted through the link"
    );
    assert!(repo_root().join("src/main.rs").exists(), "nothing deleted");
}

#[cfg(unix)]
#[test]
fn host_arch_names_a_lane_only_for_a_plain_machine_name() {
    // The lane names shots/current/<arch>, which the capture deletes and
    // rebuilds, so `uname -m` output that is not a plain machine name is refused
    // rather than turned into a path; the two vocabularies still normalize.
    use std::os::unix::fs::PermissionsExt;
    for (machine, want) in [
        ("aarch64", Some("arm64")),
        ("amd64", Some("x86_64")),
        ("riscv64", Some("riscv64")),
        ("../../src", None),
        ("x86 64", None),
        ("", None),
    ] {
        let p = Project::new();
        p.write(
            "bin/uname",
            &format!("#!/usr/bin/env bash\nprintf '%s\\n' '{machine}'\n"),
        );
        fs::set_permissions(
            p.path().join("bin/uname"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let out = std::process::Command::new("bash")
            .arg(repo_root().join("screenshots/host-arch.sh"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    p.path().join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        match want {
            Some(lane) => {
                assert!(out.status.success(), "{machine}: {stderr}");
                assert_eq!(stdout.trim_end(), lane, "{machine}");
            }
            None => {
                assert_eq!(out.status.code(), Some(1), "{machine}: {out:?}");
                assert!(
                    stderr.contains("not a plain machine name"),
                    "{machine}: {stderr}"
                );
                assert!(stdout.is_empty(), "{machine}: printed a lane: {stdout}");
            }
        }
    }
}

/// `demo-gif.py` draws each rule by its serialized report outcome from a table
/// of its own (`OUTCOME_STYLE`); it is an ungated Pillow helper, so this is the
/// gate that keeps that table equal to the report's `Outcome`. The match below
/// has no wildcard, so a new variant fails to compile here until it is listed —
/// and then fails this test until the helper draws it.
#[test]
fn the_demo_gif_draws_every_report_outcome() {
    use llmlint::domain::verdict::Outcome;
    let every = [
        Outcome::Pass,
        Outcome::Fail,
        Outcome::Skipped,
        Outcome::Ignored,
        Outcome::NotRelevant,
    ];
    for o in every {
        match o {
            Outcome::Pass
            | Outcome::Fail
            | Outcome::Skipped
            | Outcome::Ignored
            | Outcome::NotRelevant => {}
        }
    }
    let mut serialized: Vec<String> = every
        .iter()
        .map(|o| {
            serde_json::to_value(o)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    serialized.sort();

    let helper = fs::read_to_string(repo_root().join("screenshots/demo-gif.py")).unwrap();
    let table = helper
        .split_once("OUTCOME_STYLE = {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .expect("demo-gif.py defines OUTCOME_STYLE as a dict literal")
        .0;
    let mut drawn: Vec<String> = table
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split_once("\":"))
        .map(|(key, _)| key.to_owned())
        .collect();
    drawn.sort();
    assert_eq!(
        drawn, serialized,
        "OUTCOME_STYLE in screenshots/demo-gif.py"
    );
}

/// Runs `demo-gif.py`'s real `build_frames` over `rules` (a JSON array of report
/// rules) and returns the process output; stdout is `{"yellow": …, "frames": …}`.
/// Only `render_gif` needs Pillow, which the gate never installs, so the helper's
/// `from PIL import …` is answered by an empty placeholder package: everything
/// `build_frames` runs is the helper's own code.
#[cfg(unix)]
fn demo_gif_frames(rules: &str) -> std::process::Output {
    const DRIVER: &str = r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("demo_gif", sys.argv[1])
gif = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gif)
frames = gif.build_frames(json.loads(sys.argv[2]), ["PASS a-rule"])
print(json.dumps({"yellow": gif.YELLOW, "frames": [lines for lines, _ in frames]}))
"#;
    let pil = Project::new();
    pil.write("PIL/__init__.py", "Image = ImageDraw = ImageFont = None\n");
    std::process::Command::new("python3")
        .args(["-B", "-s", "-c", DRIVER])
        .arg(repo_root().join("screenshots/demo-gif.py"))
        .arg(rules)
        .env("PYTHONPATH", pil.path())
        .output()
        .expect("python3 runs the demo GIF helper")
}

/// An `ignored` rule is resolved before any judge runs, so the animation draws it
/// as ignored, in the not-judged colour, from the very first frame — never
/// queued behind the judged rules, which do start queued.
#[cfg(unix)]
#[test]
fn the_demo_gif_draws_an_ignored_rule_resolved_from_the_first_frame() {
    let out = demo_gif_frames(
        r#"[{"name": "a-rule", "outcome": "pass", "votes_total": 1},
            {"name": "b-rule", "outcome": "ignored"},
            {"name": "c-rule", "outcome": "fail", "votes_total": 1}]"#,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let drawn: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let frames = drawn["frames"].as_array().unwrap();
    let (report, live) = frames.split_last().expect("the helper draws frames");
    assert_eq!(report[0][0][0], "PASS", "the last frame is the report");
    let text = |line: &serde_json::Value| -> String {
        line.as_array()
            .unwrap()
            .iter()
            .map(|seg| seg[0].as_str().unwrap())
            .collect()
    };
    assert!(
        live[0]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| text(l) == "▖ a-rule  queued"),
        "a judged rule starts queued: {:?}",
        live[0]
    );
    for (i, frame) in live.iter().enumerate() {
        let lines = frame.as_array().unwrap();
        let ignored: Vec<_> = lines
            .iter()
            .filter(|l| text(l).contains("b-rule"))
            .collect();
        assert_eq!(ignored.len(), 1, "frame {i}: {frame}");
        assert_eq!(text(ignored[0]), "– b-rule  ignored", "frame {i}");
        assert_eq!(ignored[0][0][1], drawn["yellow"], "frame {i}: its colour");
    }
}

/// An outcome the helper has no style for stops it with the outcome named,
/// rather than a KeyError midway through the animation.
#[cfg(unix)]
#[test]
fn the_demo_gif_refuses_an_outcome_it_cannot_draw_naming_it() {
    let out = demo_gif_frames(r#"[{"name": "a-rule", "outcome": "deferred"}]"#);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("['deferred']"), "{stderr}");
    assert!(stderr.contains("OUTCOME_STYLE"), "{stderr}");
    assert!(
        out.stdout.is_empty(),
        "drew frames anyway: {:?}",
        out.stdout
    );
}
