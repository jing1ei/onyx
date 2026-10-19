#!/usr/bin/env python3
"""Offline tests for optional CI signing configuration."""
import importlib.util
from pathlib import Path
spec = importlib.util.spec_from_file_location('configure_signing', Path(__file__).with_name('configure-signing.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
configure = module.signing_environment
assert configure({}) == {}
assert configure({'APPLE_CERTIFICATE': ''}) == {}
complete = {'APPLE_CERTIFICATE': 'fixture', 'APPLE_CERTIFICATE_PASSWORD': 'fixture', 'APPLE_SIGNING_IDENTITY': 'fixture'}
assert configure(complete) == complete
notary = {'APPLE_ID': 'fixture', 'APPLE_PASSWORD': 'fixture', 'APPLE_TEAM_ID': 'fixture'}
assert configure({**complete, **notary}) == {**complete, **notary}
for partial in [{'APPLE_CERTIFICATE':'fixture'}, notary, {**complete,'APPLE_ID':'fixture'}]:
    try:
        configure(partial)
    except ValueError as error:
        assert 'fixture' not in str(error), 'errors must not disclose secret values'
    else:
        raise AssertionError('Partial signing credentials were accepted')
print('Signing checks passed: unsigned, signed, notarized, incomplete credentials, no secret disclosure.')
