//! The production packaging journey: the release builder run natively for the
//! host target with the real compiler, maturin and pip, its archive and wheel
//! checked against the one compiled binary, installed and run, and the release
//! verifier shown to refuse each tampered form. Only forge downloads are
//! doubled. The offline contract over the same scripts is the release-targets
//! project's `test`; this journey is the `packaging` target here because it
//! drives real host tools.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root: this crate lives two levels below it
/// (`tests/release-packaging/`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Real compiler, maturin, pip and installed CLI.
#[test]
fn native_release_packages_install_and_run() {
    let root = repo_root();
    let temp = tempfile::tempdir().unwrap();
    let manifest: toml::Value = toml::from_str(&read(&root.join("Cargo.toml"))).unwrap();
    let version = manifest["package"]["version"].as_str().unwrap();
    let host = std::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .unwrap();
    assert!(host.status.success());
    let host = String::from_utf8(host.stdout).unwrap();
    let target = host.lines().find_map(|l| l.strip_prefix("host: ")).unwrap();
    let output = std::process::Command::new("bash")
        .arg("scripts/release-packages.sh")
        .arg(temp.path())
        .arg(target)
        .current_dir(&root)
        .env("GITHUB_REF_NAME", format!("v{version}"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(temp
        .path()
        .join(format!("llmlint-v{version}-{target}.sha256"))
        .exists());
    assert!(temp.path().join(format!("{target}.json")).exists());
    let wheel = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "whl"))
        .unwrap();
    let original_wheel = fs::read(&wheel).unwrap();
    let python_version = wheel
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .split('-')
        .nth(1)
        .unwrap();
    let record: serde_json::Value =
        serde_json::from_str(&read(&temp.path().join(format!("{target}.json")))).unwrap();
    assert_eq!(record["target"].as_str(), Some(target));
    assert_eq!(record["version"].as_str(), Some(version));
    assert_eq!(
        record["wheel"].as_str(),
        wheel.file_name().unwrap().to_str()
    );

    let archive = temp
        .path()
        .join(format!("llmlint-v{version}-{target}.tar.gz"));
    let compiled = root.join(format!("target/{target}/release/llmlint"));
    let digest = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(&compiled)
        .output()
        .unwrap();
    assert!(digest.status.success());
    let digest = String::from_utf8(digest.stdout).unwrap();
    let hash = digest.split_whitespace().next().unwrap();
    for key in ["binarySha256", "archiveBinarySha256", "wheelBinarySha256"] {
        assert_eq!(record[key].as_str(), Some(hash));
    }
    let verify = |binary: &Path| {
        std::process::Command::new("bash")
            .arg("scripts/release-verify.sh")
            .arg(temp.path())
            .arg(&archive)
            .arg(binary)
            .arg(temp.path())
            .arg(target)
            .arg(version)
            .arg(python_version)
            .current_dir(&root)
            .output()
            .unwrap()
    };
    let checksum = std::process::Command::new("shasum")
        .args(["-a", "256", "-c"])
        .arg(format!("llmlint-v{version}-{target}.sha256"))
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(checksum.status.success(), "{checksum:?}");
    // Only forge downloads are doubled: run the public POSIX installer against real assets.
    let installer = std::process::Command::new("python3").arg("-c").arg(r#"
import os,pathlib,shutil,subprocess,sys
assets,root,version=sys.argv[1:]
assets=pathlib.Path(assets); tools=assets/'installer-tools'; tools.mkdir()
for name in ['uname','mktemp','rm','mkdir','tar','gzip','awk','sha256sum','shasum','openssl','cp','chmod','mv','grep','sed','cat','dirname','basename']:
    executable=shutil.which(name)
    if executable: (tools/name).symlink_to(executable)
python=sys.executable
curl=tools/'curl'
curl.write_text('#!'+python+'\nimport pathlib,shutil,sys\nargs=sys.argv[1:]\nname=args[-1].rsplit("/",1)[-1]\nshutil.copyfile(pathlib.Path('+repr(str(assets))+')/name,args[args.index("-o")+1])\n')
curl.chmod(0o755)
env={**os.environ,'PATH':str(tools),'LLMLINT_VERSION':'v'+version,'LLMLINT_INSTALL_DIR':str(assets/'public-install')}
run=lambda:subprocess.run(['/bin/sh',str(pathlib.Path(root)/'scripts/install.sh')],env=env,capture_output=True,text=True)
result=run(); assert result.returncode==0,(result.stdout,result.stderr)
installed=subprocess.run([str(assets/'public-install/llmlint'),'--version'],capture_output=True,text=True)
assert installed.returncode==0 and installed.stdout.strip()=='llmlint '+version,installed
checksum=next(assets.glob('llmlint-v'+version+'-*.sha256')); original=checksum.read_bytes()
checksum.write_text('0'*64+'  invalid\n')
try:
    refused=run(); assert refused.returncode!=0 and 'checksum' in refused.stderr.lower(),refused
finally: checksum.write_bytes(original)
"#).arg(temp.path()).arg(&root).arg(version).output().unwrap();
    assert!(installer.status.success(), "{installer:?}");
    let original_archive = fs::read(&archive).unwrap();
    for (kind, expected) in [
        ("metadata", "wrong distribution name"),
        ("payload", "binary reuse mismatch"),
        ("version", "wrong distribution version"),
        ("python", "Python compatibility changed"),
        ("dependencies", "runtime dependencies differ"),
        ("missing-script", "exactly one llmlint"),
        ("multiple-scripts", "exactly one llmlint"),
        (
            "multiple-metadata",
            "exactly one matching distribution METADATA",
        ),
        (
            "foreign-metadata",
            "exactly one matching distribution METADATA",
        ),
        ("archive-contents", "wrong archive contents"),
        ("archive-payload", "binary reuse mismatch"),
        ("archive-mode", "executable regular file"),
        ("archive-duplicate", "wrong archive contents"),
        ("archive-link", "non-regular member"),
    ] {
        let mutation = std::process::Command::new("python3").arg("-c").arg(r#"
import io,re,sys,tarfile,zipfile
path,archive,kind=sys.argv[1:]
if kind.startswith('archive-'):
    with tarfile.open(archive) as t:
        entries=[(info,t.extractfile(info).read()) for info in t.getmembers()]
    with tarfile.open(archive,'w:gz') as t:
        if kind=='archive-duplicate':
            info,data=entries[0]
            t.addfile(info,io.BytesIO(data))
        for info,data in entries:
            if kind=='archive-contents' and info.name=='LICENSE': continue
            if kind=='archive-payload' and info.name=='llmlint': data+=b'corrupt'
            if kind=='archive-mode' and info.name=='llmlint': info.mode=0o644
            if kind=='archive-link' and info.name=='README.md':
                info.type=tarfile.SYMTYPE; info.linkname='../../outside'; data=b''
            info.size=len(data)
            t.addfile(info,io.BytesIO(data))
else:
    with zipfile.ZipFile(path) as z:
        entries=[(info,z.read(info.filename)) for info in z.infolist()]
    with zipfile.ZipFile(path,'w') as z:
        for info,data in entries:
            if info.filename.endswith('/METADATA'):
                if kind=='multiple-metadata': z.writestr('foreign-1.dist-info/METADATA',data)
                if kind=='foreign-metadata': info.filename='foreign-1.dist-info/METADATA'
                fields={'metadata':('Name','incorrect-package'),'version':('Version','0.0.0-corrupt'),'python':('Requires-Python','<2.7'),'dependencies':('Requires-Dist','incorrect-dependency>=99')}
                if kind in fields:
                    key,value=fields[kind]
                    data=re.sub(('^'+key+':.*$').encode(),(key+': '+value).encode(),data,flags=re.M)
            if info.filename.endswith('.data/scripts/llmlint'):
                if kind=='missing-script': continue
                if kind=='payload': data+=b'corrupt'
                if kind=='multiple-scripts': z.writestr('duplicate.data/scripts/llmlint',data)
            z.writestr(info,data)
"#).arg(&wheel).arg(&archive).arg(kind).output().unwrap();
        assert!(mutation.status.success(), "{mutation:?}");
        let refusal = verify(&compiled);
        assert!(!refusal.status.success(), "accepted corrupted {kind}");
        assert!(
            String::from_utf8_lossy(&refusal.stderr).contains(expected),
            "{refusal:?}"
        );
        fs::write(&wheel, &original_wheel).unwrap();
        fs::write(&archive, &original_archive).unwrap();
    }
    let hidden_wheel = wheel.with_extension("hidden");
    fs::rename(&wheel, &hidden_wheel).unwrap();
    let refusal = verify(&compiled);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("expected one wheel"));
    fs::rename(&hidden_wheel, &wheel).unwrap();
    let duplicate = temp.path().join("duplicate.whl");
    fs::copy(&wheel, &duplicate).unwrap();
    let refusal = verify(&compiled);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("expected one wheel"));
    fs::remove_file(&duplicate).unwrap();
    let wrong_tag = temp
        .path()
        .join(format!("llmlint_cli-{python_version}-py3-none-invalid.whl"));
    fs::rename(&wheel, &wrong_tag).unwrap();
    let refusal = verify(&compiled);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("wheel tag"));
    fs::rename(&wrong_tag, &wheel).unwrap();
    let opposite_os = if target.ends_with("apple-darwin") {
        "linux"
    } else {
        "macosx_11_0"
    };
    let arch = if target.starts_with("x86_64") {
        "x86_64"
    } else if target.ends_with("apple-darwin") {
        "arm64"
    } else {
        "aarch64"
    };
    let wrong_os = temp.path().join(format!(
        "llmlint_cli-{version}-py3-none-{opposite_os}_{arch}.whl"
    ));
    fs::rename(&wheel, &wrong_os).unwrap();
    let refusal = verify(&compiled);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("wheel tag"));
    fs::rename(&wrong_os, &wheel).unwrap();

    if target.ends_with("unknown-linux-gnu") {
        let musl = temp.path().join(format!(
            "llmlint_cli-{python_version}-py3-none-musllinux_1_2_{arch}.whl"
        ));
        fs::rename(&wheel, &musl).unwrap();
        let refusal = verify(&compiled);
        assert!(!refusal.status.success());
        assert!(String::from_utf8_lossy(&refusal.stderr).contains("wheel tag"));
        fs::rename(&musl, &wheel).unwrap();
    }
    let platform = wheel
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('-')
        .next()
        .unwrap();
    for (filename, reason) in [
        (
            format!("incorrect-{python_version}-py3-none-{platform}"),
            "distribution filename",
        ),
        (
            format!("llmlint_cli-0.0.0-py3-none-{platform}"),
            "version filename",
        ),
        (
            format!("llmlint_cli-{python_version}-py2-none-{platform}"),
            "Python/ABI tag",
        ),
        (
            format!("llmlint_cli-{python_version}-py3-cp312-{platform}"),
            "Python/ABI tag",
        ),
        (
            format!("llmlint_cli-{python_version}-invalid-py3-none-{platform}"),
            "build tag",
        ),
    ] {
        let renamed = temp.path().join(filename);
        fs::rename(&wheel, &renamed).unwrap();
        let refused = verify(&compiled);
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(reason),
            "{refused:?}"
        );
        fs::rename(&renamed, &wheel).unwrap();
    }
    let numbered = temp.path().join(format!(
        "llmlint_cli-{python_version}-1-py3-none-{platform}"
    ));
    fs::rename(&wheel, &numbered).unwrap();
    let accepted = verify(&compiled);
    assert!(accepted.status.success(), "{accepted:?}");
    fs::rename(&numbered, &wheel).unwrap();
    let invalid_format = temp.path().join("invalid-format");
    fs::write(&invalid_format, b"invalid binary format").unwrap();
    let refusal = verify(&invalid_format);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("not a"));
    let unsupported = std::process::Command::new("bash")
        .arg("scripts/release-verify.sh")
        .arg(temp.path())
        .arg(&archive)
        .arg(&compiled)
        .arg(temp.path())
        .arg("../unsupported-apple-darwin")
        .arg(version)
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("unsupported target"));
    // The backend's normalized PEP-440 spelling is distinct from the Cargo tag.
    // This checks metadata acceptance only; the builder separately runs the CLI version check.
    let core_version = version.split(['-', '+']).next().unwrap();
    let normalized = format!("{core_version}rc1");
    let prerelease = format!("{core_version}-rc.1");
    let normalize_fixture = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import re,sys,zipfile
path,version=sys.argv[1:]
with zipfile.ZipFile(path) as z:
    entries=[(info,z.read(info.filename)) for info in z.infolist()]
with zipfile.ZipFile(path,'w') as z:
    for info,data in entries:
        if info.filename.endswith('/METADATA'):
            data=re.sub(b'^Version:.*$',('Version: '+version).encode(),data,flags=re.M)
        info.filename=re.sub(r'llmlint_cli-[^/]+\.dist-info/', 'llmlint_cli-'+version+'.dist-info/', info.filename)
        z.writestr(info,data)
"#,
        )
        .arg(&wheel)
        .arg(&normalized)
        .output()
        .unwrap();
    assert!(normalize_fixture.status.success());
    let normalized_wheel = temp.path().join(format!(
        "llmlint_cli-{normalized}-py3-none-{}",
        wheel
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .rsplit('-')
            .next()
            .unwrap()
    ));
    fs::rename(&wheel, &normalized_wheel).unwrap();
    let accepted = std::process::Command::new("bash")
        .arg("scripts/release-verify.sh")
        .arg(temp.path())
        .arg(&archive)
        .arg(&compiled)
        .arg(temp.path())
        .arg(target)
        .arg(&prerelease)
        .arg(&normalized)
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(accepted.status.success(), "{accepted:?}");
    fs::rename(&normalized_wheel, &wheel).unwrap();
    fs::write(&wheel, &original_wheel).unwrap();
    let wrong_cpu = temp.path().join("wrong-cpu");
    let mut bytes = fs::read(&compiled).unwrap();
    if target.ends_with("apple-darwin") {
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
    } else {
        bytes[18..20].copy_from_slice(&0u16.to_le_bytes());
    }
    fs::write(&wrong_cpu, bytes).unwrap();
    let refusal = verify(&wrong_cpu);
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("CPU"),
        "{refusal:?}"
    );
    // A real filesystem refusal after compilation must remove both scratch directories.
    let scratch = temp.path().join("failure-scratch");
    let failed_out = temp.path().join("failure-output");
    fs::create_dir(&scratch).unwrap();
    fs::create_dir_all(failed_out.join(format!("llmlint-v{version}-{target}.tar.gz"))).unwrap();
    let collision = std::process::Command::new("bash")
        .arg("scripts/release-packages.sh")
        .arg(&failed_out)
        .arg(target)
        .current_dir(&root)
        .env("GITHUB_REF_NAME", format!("v{version}"))
        .env("TMPDIR", &scratch)
        .output()
        .unwrap();
    assert!(!collision.status.success());
    assert!(
        String::from_utf8_lossy(&collision.stderr).contains("cannot pack"),
        "{collision:?}"
    );
    assert!(
        fs::read_dir(&scratch).unwrap().next().is_none(),
        "failure leaked staging data"
    );
    let failure = std::process::Command::new("bash")
        .arg("scripts/release-packages.sh")
        .arg(temp.path())
        .arg(target)
        .current_dir(&root)
        .env("GITHUB_REF_NAME", "v0.0.0-wrong")
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("tag does not match"));
}
