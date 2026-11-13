#!/usr/bin/env python3
"""Offline sabotage fixtures: no Docker, root or TUN access."""
import copy
import subprocess
import sys
import unittest
from pathlib import Path
from e2e_assert import check


class Contracts(unittest.TestCase):
    def test_named_paths_sabotage(self):
        good = [dict(name='a1', path='direct'), dict(name='a2', path='relay')]
        args = ['a1', 'direct', 'a2', 'relay']
        check('peers', good, args)
        for mutation in ([good[0]], [good[1], good[1]],
                         [dict(name='other', path='direct'), good[1]],
                         [good[0], dict(name='a2', path='direct')],
                         [dict(name='a1', path='relay'), good[1]], {}):
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                check('peers', mutation, args)

    def test_probe_sabotage(self):
        good = dict(peer='a1', port=8421, path='direct', connected=True, error=None)
        args = ['a1', 'direct', 'connected']
        check('probe', good, args)
        for key, value in [('peer', 'a2'), ('port', 22), ('path', 'relay'),
                           ('connected', False), ('connected', 1), ('error', 'timeout')]:
            bad = copy.copy(good); bad[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                check('probe', bad, args)

    def test_policy_sabotage(self):
        denied = dict(peer='a1', port=8421, path='relay', connected=False, error='timeout')
        args = ['a1', 'relay', 'timeout']
        check('probe', denied, args)
        for values in [dict(connected=True, error=None), dict(error='connection refused'),
                       dict(error=None), dict(connected=0)]:
            with self.subTest(values=values), self.assertRaises(ValueError):
                check('probe', {**denied, **values}, args)
        with self.assertRaises(ValueError):
            check('acl', dict(compiled=dict(rules=[])), [])

    def test_status(self):
        good = dict(node_name='site', control_connected=True, relay_connected=True)
        check('status', good, ['site'])
        for key in ['control_connected', 'relay_connected']:
            with self.assertRaises(ValueError):
                check('status', {**good, key: False}, ['site'])

    def test_cli_rejects_malformed_json(self):
        helper = str(Path(__file__).with_name('e2e_assert.py'))
        result = subprocess.run([sys.executable, helper, 'peers', 'a1', 'direct'],
                                input='secret not JSON', text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('secret', result.stderr)

    def test_bounded_command_failure_and_timeout(self):
        helper = str(Path(__file__).with_name('e2e_assert.py'))
        for code, seconds in [('raise SystemExit(7)', '2'), ('import time; time.sleep(2)', '.01')]:
            result = subprocess.run([sys.executable, helper, 'run', seconds,
                                     sys.executable, '-c', code], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(code.encode(), result.stderr)


if __name__ == '__main__':
    unittest.main()
