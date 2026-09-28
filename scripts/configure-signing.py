#!/usr/bin/env python3
"""Validate optional macOS signing secrets and export complete configurations."""
import os
import uuid
from pathlib import Path


def signing_environment(env):
    signing = ['APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY']
    notarizing = ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID']
    present = lambda name: bool(env.get(name))
    any_signing = any(present(name) for name in signing)
    any_notarizing = any(present(name) for name in notarizing)
    if not any_signing and not any_notarizing:
        return {}
    missing = [name for name in signing if not present(name)]
    if missing:
        raise ValueError('Incomplete signing configuration; missing: ' + ', '.join(missing))
    if any_notarizing:
        missing = [name for name in notarizing if not present(name)]
        if missing:
            raise ValueError('Incomplete notarization configuration; missing: ' + ', '.join(missing))
    names = signing + (notarizing if any_notarizing else [])
    return {name: env[name] for name in names if present(name)}


def main():
    values = signing_environment(os.environ)
    with Path(os.environ['GITHUB_ENV']).open('a') as output:
        for name, value in values.items():
            delimiter = 'ONYX_' + uuid.uuid4().hex
            output.write(f'{name}<<{delimiter}\n{value}\n{delimiter}\n')
    print('macOS: ' + ('unsigned' if not values else 'signing + notarization' if 'APPLE_ID' in values else 'signing only'))


if __name__ == '__main__':
    main()
