#!/usr/bin/env python3
"""Create a reproducible source ZIP, excluding local and generated files."""
import hashlib
import json
import stat
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = json.loads((ROOT / 'package.json').read_text())['version']
PREFIX = f'onyx-{VERSION}'
FILES = [
    '.gitattributes', '.gitignore', 'Cargo.toml', 'Cargo.lock', 'package.json',
    'package-lock.json', 'tsconfig.json', 'tsconfig.node.json', 'vite.config.ts',
    'index.html', 'eq.html', 'theme.html', 'README.md', 'CONTRIBUTING.md',
    'CHANGELOG.md', 'SECURITY.md', 'SPEC.md', 'THEMING.md', 'THIRD-PARTY.md',
]
DIRECTORIES = ['.github', 'src', 'src-tauri', 'crates', 'scripts', 'docs']
EXCLUDE = {'target', 'node_modules', '__pycache__', '.DS_Store', '.git'}
paths = [ROOT / name for name in FILES]
for name in DIRECTORIES:
    paths.extend(p for p in (ROOT / name).rglob('*')
                 if p.is_file() and not any(part in EXCLUDE for part in p.relative_to(ROOT).parts)
                 and not p.relative_to(ROOT).as_posix().startswith('src-tauri/gen/')
                 and p.suffix not in {'.pyc', '.log'} and not p.name.startswith('.env'))
paths = sorted(set(paths))
for path in paths:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'Expected a regular source file: {path}')
output = ROOT / 'artifacts'
output.mkdir(exist_ok=True)
archive = output / f'{PREFIX}-source.zip'
with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
    for path in paths:
        relative = path.relative_to(ROOT)
        entry = zipfile.ZipInfo(f'{PREFIX}/{relative.as_posix()}', (2026, 1, 1, 0, 0, 0))
        entry.create_system = 3
        mode = 0o755 if path.suffix == '.sh' else 0o644
        entry.external_attr = (stat.S_IFREG | mode) << 16
        entry.compress_type = zipfile.ZIP_DEFLATED
        bundle.writestr(entry, path.read_bytes())
with zipfile.ZipFile(archive) as bundle:
    if bundle.testzip() is not None:
        raise ValueError('ZIP integrity check failed')
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
checksum = archive.with_suffix('.zip.sha256')
checksum.write_text(f'{digest}  {archive.name}\n')
print(f'{archive}\n{len(paths)} files, {archive.stat().st_size:,} bytes\nSHA-256 {digest}')
