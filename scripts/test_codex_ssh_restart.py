"""Verify the shipped Linux restart helper reports unsuccessful restarts."""
import contextlib
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

source = (Path(__file__).resolve().parents[1] / 'src-tauri/src/modules/ssh_server.rs').read_text()
script = source.split('fn reload_app_server_script()', 1)[1].split('PY_EOF\'\n', 1)[1].split('\nPY_EOF', 1)[0]


class RestartTests(unittest.TestCase):
    def run_helper(self, restart_status, start_status=0):
        calls = []
        def run(args, **kwargs):
            calls.append(args)
            if args[0] == 'which':
                return subprocess.CompletedProcess(args, 0)
            if args[-1] == 'restart' and restart_status == 'timeout':
                raise subprocess.TimeoutExpired(args, 5)
            return subprocess.CompletedProcess(args, restart_status if args[-1] == 'restart' else start_status)
        out, err = io.StringIO(), io.StringIO()
        # No filesystem/process mutations: pretend auth and /proc are absent.
        with patch('sys.argv', ['helper', '/unused']), patch('pathlib.Path.exists', return_value=False), \
             patch('subprocess.run', side_effect=run), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            with self.assertRaises(SystemExit) as result:
                exec(script, {})
        return result.exception.code, out.getvalue(), err.getvalue(), calls

    def test_successful_restart(self):
        code, out, err, calls = self.run_helper(0)
        self.assertEqual(code, 0)
        self.assertIn('daemon=restarted', out)
        self.assertEqual(err, '')
        self.assertFalse(any(args[-1] == 'start' for args in calls))

    def test_start_fallback(self):
        code, out, _, _ = self.run_helper(1)
        self.assertEqual(code, 0)
        self.assertIn('daemon=started', out)

    def test_failure_and_timeout_are_errors(self):
        for restart in (1, 'timeout'):
            with self.subTest(restart=restart):
                code, out, err, _ = self.run_helper(restart, 1)
                self.assertEqual(code, 1)
                self.assertEqual(out, '')
                self.assertIn('daemon=', err)


if __name__ == '__main__':
    unittest.main()
