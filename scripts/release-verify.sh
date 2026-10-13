#!/usr/bin/env bash
# Packaging trust boundary, shared by production builders and corrupted-artifact journeys.
set -euo pipefail
python3 - "$@" <<'PY'
import email, glob, hashlib, json, pathlib, struct, sys, tarfile, tomllib, zipfile

def require(condition, message):
    if not condition:
        raise ValueError(message)

try:
    require(len(sys.argv) == 7, 'expected wheel directory, archive, binary, output directory, target and version')
    wheel_dir, archive, binary, out, target, version = sys.argv[1:]
    require(target in ['x86_64-apple-darwin', 'aarch64-apple-darwin', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'], f'unsupported target {target}')
    project = tomllib.load(open('pyproject.toml', 'rb'))['project']
    wheels = glob.glob(wheel_dir + '/*.whl')
    require(len(wheels) == 1, f'expected one wheel, found {wheels}')
    wheel = wheels[0]
    with zipfile.ZipFile(wheel) as z:
        scripts = [p for p in z.namelist() if p.endswith('.data/scripts/llmlint')]
        require(len(scripts) == 1, 'wheel must install exactly one llmlint executable')
        payload = z.read(scripts[0])
        metadata = email.message_from_bytes(z.read(next(p for p in z.namelist() if p.endswith('.dist-info/METADATA'))))
        require(metadata['Name'] == project['name'], f'wrong distribution name {metadata["Name"]}')
        require(metadata['Version'] == version, f'wrong distribution version {metadata["Version"]}, expected {version}')
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
        expected = 0x01000007 if target.startswith('x86_64') else 0x0100000c
        require(struct.unpack('<I', compiled[4:8])[0] == expected, 'wrong Darwin CPU')
        arch = 'x86_64' if target.startswith('x86_64') else 'arm64'
        require('macosx_' in wheel and wheel.endswith(arch + '.whl'), 'wrong Darwin wheel tag')
    else:
        require(target in ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'], f'unsupported target {target}')
        require(compiled[:4] == b'\x7fELF', 'not an ELF binary')
        require(struct.unpack('<H', compiled[18:20])[0] == (62 if target.startswith('x86_64') else 183), 'wrong Linux CPU')
        require(wheel.endswith(('x86_64' if target.startswith('x86_64') else 'aarch64') + '.whl'), 'wrong Linux wheel tag')
    require(compiled == payload == archived, 'archive/wheel binary reuse mismatch')
    sha = hashlib.sha256(compiled).hexdigest()
    pathlib.Path(out, target + '.json').write_text(json.dumps({'target': target, 'version': version, 'binarySha256': sha, 'archiveBinarySha256': sha, 'wheelBinarySha256': sha, 'wheel': pathlib.Path(wheel).name}))
except (ValueError, OSError, KeyError, StopIteration, struct.error, tarfile.TarError, zipfile.BadZipFile) as error:
    sys.exit(f'release-verify: {error}; fix package metadata, target or staged bytes and rerun scripts/release-packages.sh')
PY
