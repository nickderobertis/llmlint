#!/usr/bin/env bash
# Packaging trust boundary, shared by production builders and corrupted-artifact journeys.
set -euo pipefail
command -v python3 >/dev/null 2>&1 || {
  echo 'release-verify: Python 3 is missing; install Python 3.11 or newer and rerun the release builder' >&2
  exit 1
}
python3 - "$@" <<'PY'
import email, glob, hashlib, json, pathlib, re, struct, sys, tarfile, tomllib, zipfile

# llmlint: ignore[code_lands_in_the_domain_that_owns_it] Production release builders stay in scripts, their existing owning domain; release-targets owns their artifact contract and on-demand journeys.
def require(condition, message):
    if not condition:
        raise ValueError(message)

try:
    require(len(sys.argv) in [7, 8], 'expected wheel directory, archive, binary, output directory, target, Cargo version and optional backend Python version')
    wheel_dir, archive, binary, out, target, version = sys.argv[1:7]
    python_version = sys.argv[7] if len(sys.argv) == 8 else version
    require(target in ['aarch64-apple-darwin', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'], f'unsupported target {target}')
    project = tomllib.load(open('pyproject.toml', 'rb'))['project']
    wheels = glob.glob(wheel_dir + '/*.whl')
    require(len(wheels) == 1, f'expected one wheel, found {wheels}')
    wheel = wheels[0]
    filename = pathlib.Path(wheel).name
    tags = filename[:-4].split('-')
    require(len(tags) in [5, 6], f'invalid wheel filename {filename}')
    normalize_name = lambda name: re.sub(r'[-_.]+', '_', name).lower()
    require(normalize_name(tags[0]) == normalize_name(project['name']), 'wrong wheel distribution filename')
    require(tags[1] == python_version, 'wrong wheel version filename')
    require(tags[-3] == 'py3' and tags[-2] == 'none', 'wrong binary-wheel Python/ABI tag')
    if len(tags) == 6:
        require(re.fullmatch(r'[0-9][A-Za-z0-9_]*', tags[2]), 'invalid wheel build tag')
    platforms = tags[-1].split('.')
    arch = ('x86_64' if target.startswith('x86_64') else ('arm64' if target.endswith('apple-darwin') else 'aarch64'))
    if target.endswith('apple-darwin'):
        require(all(re.fullmatch(r'macosx_[0-9]+_[0-9]+_' + arch, tag) for tag in platforms), 'wrong Darwin wheel tag')
    else:
        require(all(re.fullmatch(r'(linux|manylinux(?:[0-9]+|_[0-9]+_[0-9]+))_' + arch, tag) for tag in platforms), 'wrong Linux wheel tag')
    with zipfile.ZipFile(wheel) as z:
        scripts = [p for p in z.namelist() if p.endswith('.data/scripts/llmlint')]
        require(len(scripts) == 1, 'wheel must install exactly one llmlint executable')
        payload = z.read(scripts[0])
        metadata_paths = [p for p in z.namelist() if p.endswith('.dist-info/METADATA')]
        require(len(metadata_paths) == 1 and metadata_paths[0] == f'{normalize_name(project["name"])}-{python_version}.dist-info/METADATA', 'wheel must contain exactly one matching distribution METADATA')
        metadata = email.message_from_bytes(z.read(metadata_paths[0]))
        require(metadata['Name'] == project['name'], f'wrong distribution name {metadata["Name"]}')
        require(metadata['Version'] == python_version, f'wrong distribution version {metadata["Version"]}, expected {python_version}')
        require(metadata['Requires-Python'] == project['requires-python'], 'Python compatibility changed')
        normalize = lambda r: r.replace(' ', '').replace('(', '').replace(')', '')
        require(sorted(map(normalize, metadata.get_all('Requires-Dist', []))) == sorted(map(normalize, project['dependencies'])), 'runtime dependencies differ from pyproject.toml')
    with tarfile.open(archive) as t:
        members = t.getmembers()
        require(len(members) == 4 and set(t.getnames()) == {'llmlint', 'README.md', 'LICENSE', 'CHANGELOG.md'}, 'wrong archive contents')
        require(all(member.isfile() for member in members), 'archive contains a non-regular member')
        require(t.getmember('llmlint').isfile() and t.getmember('llmlint').mode & 0o111, 'archive executable is not an executable regular file')
        archived = t.extractfile('llmlint').read()
    compiled = pathlib.Path(binary).read_bytes()
    if target.endswith('apple-darwin'):
        require(compiled[:4] == b'\xcf\xfa\xed\xfe', 'not a Mach-O binary')
        require(struct.unpack('<I', compiled[4:8])[0] == 0x0100000c, 'wrong Darwin CPU')

    else:
        require(target in ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'], f'unsupported target {target}')
        require(compiled[:4] == b'\x7fELF', 'not an ELF binary')
        require(struct.unpack('<H', compiled[18:20])[0] == (62 if target.startswith('x86_64') else 183), 'wrong Linux CPU')

    require(compiled == payload == archived, 'archive/wheel binary reuse mismatch')
    sha = hashlib.sha256(compiled).hexdigest()
    pathlib.Path(out, target + '.json').write_text(json.dumps({'target': target, 'version': version, 'binarySha256': sha, 'archiveBinarySha256': sha, 'wheelBinarySha256': sha, 'wheel': pathlib.Path(wheel).name}))
except (ValueError, OSError, KeyError, StopIteration, struct.error, tarfile.TarError, zipfile.BadZipFile) as error:
    sys.exit(f'release-verify: {error}; fix package metadata, target or staged bytes and rerun scripts/release-packages.sh')
PY
