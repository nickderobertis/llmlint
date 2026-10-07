//! Journeys over the repository's own tooling scripts (`scripts/`): the pinned
//! actionlint installer and the workflow lint that runs it, the dev-environment
//! readiness check, and the agent command allowlist held to the justfile. Each
//! drives the real script the way `just`, `scripts/setup.sh`, or CI runs it, with
//! only the host's OS/CPU, the release server, and third-party binaries stood in.
//! This is the `repo-tooling` project's `test` target.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

/// The repository root: this crate's manifest sits one level below it, and
/// every path these journeys read or drive is relative to the root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the repository root")
        .to_path_buf()
}

/// Drop every inherited `GIT_*` variable from a command: git's
/// repository-selection variables outrank `-C`, and the repository's own gate
/// runs inside a `pre-push` hook that exports `GIT_DIR` for its own repository.
fn clear_git_env(cmd: &mut std::process::Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&name);
        }
    }
}

/// A throwaway directory with helpers to write files into it.
struct Project {
    dir: TempDir,
}

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

#[test]
fn allowlisted_just_commands_are_declared_recipes() {
    // `.claude/settings.json` pre-approves routine recipes by name; a renamed or
    // removed recipe would leave a grant for a command that no longer exists (and
    // re-prompt for the one that replaced it).
    let root = repo_root();
    let out = std::process::Command::new("just")
        .arg("--summary")
        .current_dir(&root)
        .output()
        .expect("`just` is a required dev tool (see scripts/setup-lib.sh)");
    assert!(out.status.success(), "{out:?}");
    let summary = String::from_utf8_lossy(&out.stdout);
    let recipes: Vec<&str> = summary.split_whitespace().collect();
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    let granted: Vec<&str> = settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str()?.strip_prefix("Bash(just "))
        .map(|rest| rest.split([')', ':', ' ']).next().unwrap())
        .collect();
    assert!(granted.contains(&"lint-workflows"), "{granted:?}");
    for recipe in granted {
        assert!(
            recipes.contains(&recipe),
            ".claude/settings.json grants `just {recipe}`, which the justfile does not declare"
        );
    }
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

// llmlint: ignore-block[e2e_not_mocked] the real scripts run with real curl/tar/install/bash; a test cannot own the host's OS/CPU, rhysd's release server, or a second actionlint release, so only `uname`, the release tree, and (for the version/exit-code journeys) the actionlint binary are stood in
/// Read from the justfile, as both scripts do, so these journeys follow a pin
/// bump instead of going stale.
#[cfg(unix)]
fn actionlint_version() -> String {
    let justfile = fs::read_to_string(repo_root().join("justfile")).unwrap();
    justfile
        .lines()
        .find_map(|l| l.strip_prefix("actionlint-version := "))
        .expect("the justfile pins actionlint-version")
        .trim()
        .trim_matches('"')
        .to_string()
}

/// Every asset `scripts/install-actionlint.sh` can choose, as the `uname -s` /
/// `uname -m` answer that selects it plus its `<os>_<arch>` suffix — read from the
/// script's `case` arms, so the journeys follow its platform matrix instead of
/// restating it.
#[cfg(unix)]
fn actionlint_installable_assets() -> Vec<(String, String, String)> {
    let script = fs::read_to_string(repo_root().join("scripts/install-actionlint.sh")).unwrap();
    let arms = |var: &str| -> Vec<(String, String)> {
        let needle = format!(") {var}=\"");
        script
            .lines()
            .filter_map(|l| l.split_once(&needle))
            .map(|(pattern, rest)| {
                let uname = pattern.split('|').next().unwrap().trim().to_string();
                (uname, rest.split('"').next().unwrap().to_string())
            })
            .collect()
    };
    let (oses, arches) = (arms("asset_os"), arms("asset_arch"));
    assert!(
        !oses.is_empty() && !arches.is_empty(),
        "no platform arms read"
    );
    oses.iter()
        .flat_map(|(s, os)| {
            arches
                .iter()
                .map(move |(m, arch)| (s.clone(), m.clone(), format!("{os}_{arch}")))
        })
        .collect()
}

#[cfg(unix)]
fn write_exe(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `PATH` with `dir` in front and every directory holding an `actionlint`
/// removed, so a journey controls exactly which actionlint (if any) is found.
#[cfg(unix)]
fn path_without_actionlint(dir: &Path) -> String {
    let mut parts = vec![dir.display().to_string()];
    parts.extend(
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .filter(|d| !d.is_empty() && !Path::new(d).join("actionlint").exists())
            .map(str::to_string),
    );
    parts.join(":")
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ActionlintRelease {
    /// Well-formed; each archive's `actionlint` prints the asset it came from.
    Good,
    /// The pinned digests name different bytes (a tampered download).
    WrongDigest,
    /// The pin file lacks the asset's line.
    Unpinned,
    /// Matches its pin, but holds no top-level `actionlint`.
    NoBinary,
    /// Matches its pin, but is not a gzip tarball at all.
    Corrupt,
}

/// A stand-in release tree + pin file under `p`, served over `file://`.
#[cfg(unix)]
fn actionlint_release(p: &Project, kind: ActionlintRelease) -> PathBuf {
    let version = actionlint_version();
    let releases = p.path().join(format!("releases/v{version}"));
    fs::create_dir_all(&releases).unwrap();
    let mut sums = String::new();
    for (_, _, suffix) in actionlint_installable_assets() {
        let asset = format!("actionlint_{version}_{suffix}");
        let staging = p.path().join("staging").join(&asset);
        let inner = match kind {
            ActionlintRelease::NoBinary => "README.md",
            _ => "actionlint",
        };
        write_exe(
            &staging.join(inner),
            &format!(
                "#!/usr/bin/env bash\n[ \"${{1:-}}\" = -version ] && {{ printf '{version}\\n'; exit 0; }}\nprintf '{asset}\\n'\n"
            ),
        );
        let archive = releases.join(format!("{asset}.tar.gz"));
        if matches!(kind, ActionlintRelease::Corrupt) {
            fs::write(&archive, "not a tarball").unwrap();
        } else {
            let tar = std::process::Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&staging)
                .arg(inner)
                .status()
                .unwrap();
            assert!(tar.success(), "packing the stand-in {asset} release");
        }
        let digest = match kind {
            ActionlintRelease::WrongDigest => "0".repeat(64),
            _ => sha256_hex(&archive),
        };
        if !matches!(kind, ActionlintRelease::Unpinned) {
            sums.push_str(&format!("{digest}  {asset}.tar.gz\n"));
        }
    }
    let sums_file = p.path().join("actionlint.sha256");
    fs::write(&sums_file, &sums).unwrap();
    sums_file
}

/// Run the real `scripts/install-actionlint.sh` on a host whose `uname` reports
/// `os`/`arch`, with `envs` layered over the stand-in defaults.
#[cfg(unix)]
fn run_install_actionlint_in(
    p: &Project,
    os: &str,
    arch: &str,
    envs: &[(&str, String)],
) -> std::process::Output {
    let stub_dir = p.path().join("bin");
    write_exe(
        &stub_dir.join("uname"),
        &format!(
            "#!/usr/bin/env bash\ncase \"$1\" in -s) printf '{os}\\n' ;; -m) printf '{arch}\\n' ;; *) exit 2 ;; esac\n"
        ),
    );
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(repo_root().join("scripts/install-actionlint.sh"))
        .env("PATH", path_without_actionlint(&stub_dir))
        .env(
            "ACTIONLINT_BASE_URL",
            format!("file://{}", p.path().join("releases").display()),
        )
        .env("ACTIONLINT_INSTALL_DIR", p.path().join("out"))
        .env("ACTIONLINT_SHA256_FILE", p.path().join("actionlint.sha256"));
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

#[cfg(unix)]
fn run_install_actionlint(
    os: &str,
    arch: &str,
    kind: ActionlintRelease,
) -> (std::process::Output, Project) {
    let p = Project::new();
    actionlint_release(&p, kind);
    let out = run_install_actionlint_in(&p, os, arch, &[]);
    (out, p)
}

#[cfg(unix)]
#[test]
fn install_actionlint_installs_the_build_matching_the_host() {
    // `just setup` and CI's gate job both install through this script, on Linux
    // and macOS hosts of either architecture, so each `uname` spelling must land
    // the matching release asset — proven by running what was installed.
    for (os, arch, asset) in [
        ("Linux", "x86_64", "linux_amd64"),
        ("Linux", "amd64", "linux_amd64"),
        ("Linux", "aarch64", "linux_arm64"),
        ("Linux", "arm64", "linux_arm64"),
        ("Darwin", "x86_64", "darwin_amd64"),
        ("Darwin", "amd64", "darwin_amd64"),
        ("Darwin", "arm64", "darwin_arm64"),
        ("Darwin", "aarch64", "darwin_arm64"),
    ] {
        let (out, p) = run_install_actionlint(os, arch, ActionlintRelease::Good);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{os}/{arch}: {stderr}");
        let installed = p.path().join("out/actionlint");
        let said = std::process::Command::new(&installed)
            .output()
            .unwrap_or_else(|e| panic!("{os}/{arch}: nothing installed ({e}): {stderr}"));
        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            format!("actionlint_{}_{asset}", actionlint_version()),
            "{os}/{arch}: installed the wrong release asset"
        );
    }
}

#[cfg(unix)]
#[test]
fn install_actionlint_reuses_an_installed_pin_without_fetching() {
    // `just setup` runs the installer on every provision: once the pinned version
    // is in the install dir, a re-run leaves it alone and needs no release server
    // (proven by deleting the stand-in tree before the second run).
    let (first, p) = run_install_actionlint("Linux", "x86_64", ActionlintRelease::Good);
    assert!(first.status.success(), "{first:?}");
    fs::remove_dir_all(p.path().join("releases")).unwrap();
    let again = run_install_actionlint_in(&p, "Linux", "x86_64", &[]);
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(stdout.contains("already at"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn install_actionlint_replaces_an_installed_actionlint_off_the_pin() {
    // A stale actionlint in the install dir (an earlier pin) is replaced by the
    // pinned release rather than mistaken for it.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    write_exe(
        &p.path().join("out/actionlint"),
        "#!/usr/bin/env bash\nprintf '0.0.1\\n'\n",
    );
    let out = run_install_actionlint_in(&p, "Linux", "x86_64", &[]);
    assert!(out.status.success(), "{out:?}");
    let said = std::process::Command::new(p.path().join("out/actionlint"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        format!("actionlint_{}_linux_amd64", actionlint_version())
    );
}

#[cfg(unix)]
#[test]
fn install_actionlint_refuses_to_install_unverified_without_a_sha256_tool() {
    // With no sha256sum/shasum/openssl the archive cannot be checked against its
    // pin, so nothing is installed and the missing tool is named — not reported
    // as a digest mismatch the user would chase upstream.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    let tools = p.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    for tool in [
        "bash", "curl", "awk", "tar", "gzip", "install", "mktemp", "rm", "head", "grep", "cut",
        "dirname", "cat",
    ] {
        let found = std::process::Command::new("bash")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let real = String::from_utf8_lossy(&found.stdout).trim().to_string();
        assert!(!real.is_empty(), "test host lacks {tool}");
        std::os::unix::fs::symlink(real, tools.join(tool)).unwrap();
    }
    write_exe(
        &tools.join("uname"),
        "#!/usr/bin/env bash\ncase \"$1\" in -s) echo Linux ;; *) echo x86_64 ;; esac\n",
    );
    let out = std::process::Command::new(tools.join("bash"))
        .arg(repo_root().join("scripts/install-actionlint.sh"))
        .env("PATH", &tools)
        .env("HOME", p.path())
        .env(
            "ACTIONLINT_BASE_URL",
            format!("file://{}", p.path().join("releases").display()),
        )
        .env("ACTIONLINT_INSTALL_DIR", p.path().join("out"))
        .env("ACTIONLINT_SHA256_FILE", p.path().join("actionlint.sha256"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no SHA-256 tool"), "{stderr}");
    assert!(
        !p.path().join("out/actionlint").exists(),
        "installed anyway"
    );
}

#[cfg(unix)]
#[test]
fn install_actionlint_refuses_a_release_that_fails_validation() {
    // A tampered download, an asset the pin file does not cover, and an archive
    // whose layout moved are each refused before anything is installed, with the
    // cause and the next action named.
    for (kind, needles) in [
        (
            ActionlintRelease::WrongDigest,
            ["sha256 mismatch", "checksums.txt"],
        ),
        (
            ActionlintRelease::Unpinned,
            ["no pinned sha256", "checksums.txt"],
        ),
        (
            ActionlintRelease::NoBinary,
            ["archive layout", "actionlint-version"],
        ),
    ] {
        let (out, p) = run_install_actionlint("Linux", "x86_64", kind);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        for needle in needles {
            assert!(stderr.contains(needle), "missing {needle:?}: {stderr}");
        }
        assert!(
            !p.path().join("out/actionlint").exists(),
            "installed anyway"
        );
    }
}

#[cfg(unix)]
#[test]
fn install_actionlint_names_the_recovery_when_a_step_fails() {
    // Unreachable release, an archive that will not unpack, and an install dir
    // that cannot be written each stop with the cause and what to do next, never
    // a bare curl/tar/install error.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    fs::remove_dir_all(p.path().join("releases")).unwrap();
    let blocker = p.path().join("a-file");
    fs::write(&blocker, "").unwrap();
    let corrupt = Project::new();
    actionlint_release(&corrupt, ActionlintRelease::Corrupt);
    let unwritable = Project::new();
    actionlint_release(&unwritable, ActionlintRelease::Good);
    for (project, envs, needles) in [
        (&p, vec![], ["could not download", "ACTIONLINT_BASE_URL"]),
        (
            &corrupt,
            vec![],
            ["did not unpack", "re-pin actionlint-version"],
        ),
        (
            &unwritable,
            vec![(
                "ACTIONLINT_INSTALL_DIR",
                blocker.join("bin").display().to_string(),
            )],
            ["could not install into", "ACTIONLINT_INSTALL_DIR"],
        ),
    ] {
        let out = run_install_actionlint_in(project, "Linux", "x86_64", &envs);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        for needle in needles {
            assert!(stderr.contains(needle), "missing {needle:?}: {stderr}");
        }
    }
}

#[cfg(unix)]
#[test]
fn both_scripts_name_a_missing_version_pin() {
    // Both scripts take the version only from the justfile they ship beside; in a
    // checkout whose justfile lost the pin each names the line to restore, rather
    // than installing nothing or comparing against an empty version.
    for script in ["scripts/install-actionlint.sh", "scripts/lint-workflows.sh"] {
        let p = Project::new();
        let root = repo_root();
        for file in [script, "scripts/setup-lib.sh"] {
            p.write(file, &fs::read_to_string(root.join(file)).unwrap());
        }
        p.write("justfile", "default:\n    @true\n");
        let out = std::process::Command::new("bash")
            .arg(p.path().join(script))
            .env("HOME", p.path())
            .env("ACTIONLINT_INSTALL_DIR", p.path().join("out"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{script}: {stderr}");
        assert!(
            stderr.contains("no actionlint-version pin"),
            "{script}: {stderr}"
        );
        assert!(
            stderr.contains("actionlint-version :="),
            "{script}: {stderr}"
        );
        assert!(
            !p.path().join("out/actionlint").exists(),
            "installed anyway"
        );
    }
}

#[cfg(unix)]
#[test]
fn install_actionlint_refuses_a_host_with_no_pinned_build() {
    // A platform the script does not map is named, with where to get actionlint
    // instead, rather than fetching a URL that does not exist.
    for (os, arch, needle) in [
        ("Linux", "riscv64", "riscv64"),
        ("FreeBSD", "x86_64", "FreeBSD"),
    ] {
        let (out, p) = run_install_actionlint(os, arch, ActionlintRelease::Good);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{os}/{arch}: {stderr}");
        assert!(stderr.contains(needle), "{os}/{arch}: {stderr}");
        assert!(stderr.contains("install.md"), "no next action: {stderr}");
        assert!(
            !p.path().join("out/actionlint").exists(),
            "installed anyway"
        );
    }
}

#[cfg(unix)]
#[test]
fn install_actionlint_accepts_a_pinned_actionlint_already_on_path() {
    // The recovery the unsupported-host message gives — install the pinned
    // version yourself and put it on PATH — must let `just setup` pass there.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    let manual = p.path().join("manual");
    write_exe(
        &manual.join("actionlint"),
        &format!(
            "#!/usr/bin/env bash\nprintf '{}\\n'\n",
            actionlint_version()
        ),
    );
    let path = format!(
        "{}:{}",
        manual.display(),
        path_without_actionlint(&p.path().join("bin"))
    );
    let out = run_install_actionlint_in(&p, "FreeBSD", "x86_64", &[("PATH", path)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout.contains("already at"), "{stdout}");
    assert!(
        !p.path().join("out/actionlint").exists(),
        "installed anyway"
    );
}

#[cfg(unix)]
#[test]
fn install_actionlint_replaces_a_stale_install_even_with_the_pin_on_path() {
    // A pinned actionlint elsewhere on PATH does not excuse a stale one in the
    // install dir: the lint step may find the install dir first, so it is
    // replaced rather than reported as already installed.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    write_exe(
        &p.path().join("out/actionlint"),
        "#!/usr/bin/env bash\nprintf '0.0.1\\n'\n",
    );
    let manual = p.path().join("manual");
    write_exe(
        &manual.join("actionlint"),
        &format!(
            "#!/usr/bin/env bash\nprintf '{}\\n'\n",
            actionlint_version()
        ),
    );
    let path = format!(
        "{}:{}",
        p.path().join("bin").display(),
        path_without_actionlint(&manual)
    );
    let out = run_install_actionlint_in(&p, "Linux", "x86_64", &[("PATH", path)]);
    assert!(out.status.success(), "{out:?}");
    let said = std::process::Command::new(p.path().join("out/actionlint"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        format!("actionlint_{}_linux_amd64", actionlint_version())
    );
}

#[cfg(unix)]
#[test]
fn install_actionlint_refuses_a_malformed_override_before_fetching() {
    // The overrides steer a download and two filesystem paths, so a bad one fails
    // with its own name attached rather than deep inside curl or awk.
    for (var, value, needle) in [
        (
            "ACTIONLINT_BASE_URL",
            "ftp://example.invalid",
            "ACTIONLINT_BASE_URL",
        ),
        ("ACTIONLINT_INSTALL_DIR", "", "ACTIONLINT_INSTALL_DIR"),
        (
            "ACTIONLINT_SHA256_FILE",
            "/nonexistent/actionlint.sha256",
            "digest pin file",
        ),
        // Readable, but a directory: refused here, not handed to awk.
        ("ACTIONLINT_SHA256_FILE", "", "digest pin file"),
    ] {
        let p = Project::new();
        let value = if var == "ACTIONLINT_SHA256_FILE" && value.is_empty() {
            p.path().display().to_string()
        } else {
            value.to_string()
        };
        let out = run_install_actionlint_in(&p, "Linux", "x86_64", &[(var, value)]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{var}: {stderr}");
        assert!(stderr.contains(needle), "{var}: {stderr}");
        assert!(
            !p.path().join("out/actionlint").exists(),
            "{var}: installed anyway"
        );
    }
}

#[cfg(unix)]
#[test]
fn committed_actionlint_pins_cover_every_installable_asset() {
    // Run the installer on every host it supports against the COMMITTED pin file:
    // each must reach the digest comparison (a stand-in archive never matches a
    // real digest) rather than stop at "no pinned sha256" — a missing line would
    // fail `just setup` or CI's gate on exactly the platform nobody pinned.
    let committed = repo_root().join("scripts/actionlint.sha256");
    for (uname_s, uname_m, suffix) in actionlint_installable_assets() {
        let p = Project::new();
        actionlint_release(&p, ActionlintRelease::Good);
        let out = run_install_actionlint_in(
            &p,
            &uname_s,
            &uname_m,
            &[("ACTIONLINT_SHA256_FILE", committed.display().to_string())],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{suffix}: {stderr}");
        let expected = stderr
            .split("expected ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("{suffix}: no pinned digest reached: {stderr}"));
        assert!(
            expected.len() == 64 && expected.chars().all(|c| c.is_ascii_hexdigit()),
            "{suffix}: pinned digest is not a sha256: {expected}"
        );
    }
}

/// Run the real `scripts/lint-workflows.sh` with `bin` (which may hold an
/// `actionlint`) as the only place an actionlint can be found.
#[cfg(unix)]
fn run_lint_workflows(p: &Project, bin: &Path) -> std::process::Output {
    fs::create_dir_all(bin).unwrap();
    std::process::Command::new("bash")
        .arg(repo_root().join("scripts/lint-workflows.sh"))
        .env("PATH", path_without_actionlint(bin))
        .env("HOME", p.path())
        .output()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn lint_workflows_names_the_install_command_when_actionlint_is_absent() {
    // With no actionlint anywhere on PATH (nor in the ~/.local/bin setup installs
    // into), `just check`'s workflow step fails and says how to install the pin,
    // rather than a bare "command not found".
    let p = Project::new();
    let out = run_lint_workflows(&p, &p.path().join("bin"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("actionlint not found"), "{stderr}");
    assert!(stderr.contains("just actionlint-tools"), "{stderr}");
    assert!(stderr.contains(&actionlint_version()), "{stderr}");
}

#[cfg(unix)]
#[test]
fn lint_workflows_refuses_an_actionlint_off_the_pin() {
    // A different actionlint release checks different things, so an off-pin
    // binary is refused (naming both versions) before it lints anything, rather
    // than letting local and CI results disagree. In the install dir, reinstalling
    // fixes it; elsewhere on PATH it would shadow the reinstall, so that is said.
    for (dir, remedy) in [
        (".local/bin", "install the pinned release"),
        ("bin", "shadows"),
    ] {
        let p = Project::new();
        let bin = p.path().join(dir);
        let ran = p.path().join("linted");
        write_exe(
            &bin.join("actionlint"),
            &format!(
                "#!/usr/bin/env bash\n[ \"${{1:-}}\" = -version ] && {{ printf '0.0.1\\n'; exit 0; }}\ntouch '{}'\n",
                ran.display()
            ),
        );
        let out = run_lint_workflows(&p, &bin);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{dir}: {stderr}");
        assert!(stderr.contains("0.0.1"), "{dir}: {stderr}");
        assert!(stderr.contains(&actionlint_version()), "{dir}: {stderr}");
        assert!(stderr.contains(remedy), "{dir}: {stderr}");
        assert!(stderr.contains("just actionlint-tools"), "{dir}: {stderr}");
        assert!(!ran.exists(), "{dir}: linted with an off-pin actionlint");
    }
}

#[cfg(unix)]
#[test]
fn lint_workflows_fails_with_the_finding_when_actionlint_reports_one() {
    // The pinned actionlint's findings are the gate: they reach the user on
    // stderr with the next action, and fail the step; a clean run is silent. It is
    // invoked with no arguments from the repo root — actionlint's own "every
    // workflow in .github/workflows" mode — so no workflow is left out.
    for (exit, finding) in [(1, "bench.yml:32:12: bad [syntax-check]"), (0, "")] {
        let p = Project::new();
        let bin = p.path().join("bin");
        let call = p.path().join("call");
        write_exe(
            &bin.join("actionlint"),
            &format!(
                "#!/usr/bin/env bash\n[ \"${{1:-}}\" = -version ] && {{ printf '{}\\n'; exit 0; }}\nprintf '%s|%s' \"$PWD\" \"$#\" > '{}'\nprintf '{finding}'\nexit {exit}\n",
                actionlint_version(),
                call.display()
            ),
        );
        let out = run_lint_workflows(&p, &bin);
        assert_eq!(out.status.code(), Some(exit), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.stdout.is_empty(), "{out:?}");
        if exit == 0 {
            assert!(stderr.is_empty(), "not quiet on success: {stderr}");
        } else {
            assert!(stderr.starts_with(finding), "{stderr}");
            assert!(stderr.contains("re-run: just lint-workflows"), "{stderr}");
        }
        let root = fs::canonicalize(repo_root()).unwrap();
        assert_eq!(
            fs::read_to_string(&call).unwrap(),
            format!("{}|0", root.display()),
            "actionlint must run argument-free from the repo root"
        );
    }
}

#[cfg(unix)]
#[test]
fn lint_workflows_finds_the_actionlint_the_installer_put_in_its_default_dir() {
    // Setup installs to the default dir with no override, in a shell that may not
    // have ~/.local/bin on PATH; the lint step must still find that binary.
    let p = Project::new();
    actionlint_release(&p, ActionlintRelease::Good);
    let stub_dir = p.path().join("bin");
    write_exe(
        &stub_dir.join("uname"),
        "#!/usr/bin/env bash\ncase \"$1\" in -s) echo Linux ;; *) echo x86_64 ;; esac\n",
    );
    let install = std::process::Command::new("bash")
        .arg(repo_root().join("scripts/install-actionlint.sh"))
        .env("PATH", path_without_actionlint(&stub_dir))
        .env("HOME", p.path())
        .env_remove("ACTIONLINT_INSTALL_DIR")
        .env(
            "ACTIONLINT_BASE_URL",
            format!("file://{}", p.path().join("releases").display()),
        )
        .env("ACTIONLINT_SHA256_FILE", p.path().join("actionlint.sha256"))
        .output()
        .unwrap();
    assert!(install.status.success(), "{install:?}");
    assert!(p.path().join(".local/bin/actionlint").is_file());
    let out = run_lint_workflows(&p, &p.path().join("empty"));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        format!("actionlint_{}_linux_amd64", actionlint_version()),
        "the lint step ran some other actionlint"
    );
}

#[cfg(unix)]
#[test]
fn setup_check_reports_a_missing_actionlint_as_not_ready() {
    // actionlint is a gate tool, so a machine without it is not ready: the
    // session hook's readiness check names it and points at `just setup`.
    let p = Project::new();
    let bin = p.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let out = std::process::Command::new("bash")
        .arg(repo_root().join("scripts/setup-check.sh"))
        .env("PATH", path_without_actionlint(&bin))
        .env("HOME", p.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let missing = stdout
        .lines()
        .find(|l| l.contains("missing tools:"))
        .unwrap_or_else(|| panic!("no missing-tools line: {stdout}"));
    assert!(missing.contains("actionlint"), "{stdout}");
    assert!(stdout.contains("just setup"), "{stdout}");
}
// llmlint: ignore-end[e2e_not_mocked]

// ---- gate recipes: the tier and its explicit base ---------------------------

/// A scratch repository carrying the real justfile and the real
/// `scripts/nx-tier.sh` + `scripts/nx-base.sh`, with only `scripts/nx` (Nx
/// itself, the orchestrator the recipes hand off to) stood in by a stub that
/// records its argv. History: `base` -> `feature` (HEAD) on one side, and
/// `origin/main` moved on to `upstream` on the other, so the merge base with
/// `origin/main` (`base`) is not `origin/main` itself.
#[cfg(unix)]
struct GateRepo {
    p: Project,
    base: String,
    upstream: String,
}

#[cfg(unix)]
impl GateRepo {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let p = Project::new();
        let root = repo_root();
        for file in ["justfile", "scripts/nx-tier.sh", "scripts/nx-base.sh"] {
            p.write(file, &fs::read_to_string(root.join(file)).unwrap());
        }
        p.write(
            "scripts/nx",
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$NX_CALLS\"\n",
        );
        fs::set_permissions(
            p.path().join("scripts/nx"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let git_out = |args: &[&str]| {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(p.path()).args(args);
            clear_git_env(&mut cmd);
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git_out(&["init", "-q", "-b", "main"]);
        git_out(&["config", "user.email", "t@t.t"]);
        git_out(&["config", "user.name", "t"]);
        git_out(&["add", "."]);
        git_out(&["commit", "-q", "-m", "base"]);
        let base = git_out(&["rev-parse", "HEAD"]);
        git_out(&["commit", "-q", "--allow-empty", "-m", "upstream"]);
        let upstream = git_out(&["rev-parse", "HEAD"]);
        git_out(&["update-ref", "refs/remotes/origin/main", &upstream]);
        git_out(&["checkout", "-q", "-b", "feature", &base]);
        git_out(&["commit", "-q", "--allow-empty", "-m", "feature"]);
        GateRepo { p, base, upstream }
    }

    /// Run `just <args>` here, with `NX_BASE` set to `nx_base` (or unset);
    /// returns the output and the argv each `scripts/nx` call received.
    fn just(&self, args: &[&str], nx_base: Option<&str>) -> (std::process::Output, Vec<String>) {
        let calls = self.p.path().join("nx-calls");
        let _ = fs::remove_file(&calls);
        let mut cmd = std::process::Command::new("just");
        cmd.args(args)
            .current_dir(self.p.path())
            .env("NX_CALLS", &calls)
            .env_remove("NX_BASE");
        clear_git_env(&mut cmd);
        if let Some(base) = nx_base {
            cmd.env("NX_BASE", base);
        }
        let out = cmd
            .output()
            .expect("`just` is a required dev tool (see scripts/setup-lib.sh)");
        let recorded = fs::read_to_string(&calls).unwrap_or_default();
        (out, recorded.lines().map(str::to_string).collect())
    }
}

#[cfg(unix)]
const GATE_TARGETS: &str = "-t format lint lint-sh lint-workflows build test doc coverage";

#[cfg(unix)]
#[test]
fn check_runs_the_affected_tier_from_the_merge_base_with_origin_main_by_default() {
    let repo = GateRepo::new();
    let (out, calls) = repo.just(&["check"], None);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        calls,
        vec![format!("affected --base={} {GATE_TARGETS}", repo.base)]
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("merge-base with origin/main"));
}

#[cfg(unix)]
#[test]
fn check_takes_its_base_from_a_valid_nx_base() {
    let repo = GateRepo::new();
    for base in [repo.upstream.as_str(), "origin/main", "main"] {
        let (out, calls) = repo.just(&["check"], Some(base));
        assert!(out.status.success(), "{base}: {out:?}");
        assert_eq!(
            calls,
            vec![format!("affected --base={base} {GATE_TARGETS}")]
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("(NX_BASE)"));
    }
}

#[cfg(unix)]
#[test]
fn check_refuses_an_invalid_nx_base_before_any_target_runs() {
    let repo = GateRepo::new();
    // `HEAD~1` resolves, so only the plain-ref-or-SHA rule refuses it; the
    // rest are malformed or name nothing.
    for bad in [
        "HEAD~1",
        "main..feature",
        "-x",
        "main;rm",
        "",
        "no-such-ref",
    ] {
        let (out, calls) = repo.just(&["check"], Some(bad));
        assert!(!out.status.success(), "{bad:?} was accepted: {out:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("NX_BASE"), "{bad:?}: {err}");
        assert!(calls.is_empty(), "{bad:?} reached Nx: {calls:?}");
    }
}

#[cfg(unix)]
#[test]
fn check_all_runs_the_full_sweep_and_a_mistyped_tier_runs_nothing() {
    let repo = GateRepo::new();
    let (out, calls) = repo.just(&["check", "--all"], Some("main..feature"));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(calls, vec![format!("run-many --all {GATE_TARGETS}")]);

    let (out, calls) = repo.just(&["check", "--al"], None);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown flag(s) '--al'"));
    assert!(calls.is_empty(), "{calls:?}");
}

#[cfg(unix)]
#[test]
fn every_gate_recipe_takes_the_same_tier() {
    // The test/lint/format/doc recipes are the same tier choice over one
    // target each, so `just test` in CI's cross jobs and `just check` agree.
    let repo = GateRepo::new();
    for (recipe, targets) in [
        ("test", "-t test"),
        ("lint", "-t lint"),
        ("lint-sh", "-t lint-sh"),
        ("lint-workflows", "-t lint-workflows"),
        ("fmt-check", "-t format"),
        ("format", "-t format --configuration=write"),
        ("doc", "-t doc"),
    ] {
        let (out, calls) = repo.just(&[recipe], None);
        assert!(out.status.success(), "{recipe}: {out:?}");
        assert_eq!(
            calls,
            vec![format!("affected --base={} {targets}", repo.base)]
        );
        let (out, calls) = repo.just(&[recipe, "--all"], None);
        assert!(out.status.success(), "{recipe} --all: {out:?}");
        assert_eq!(calls, vec![format!("run-many --all {targets}")]);
    }
}
