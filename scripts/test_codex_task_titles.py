"""Regression tests for sidebar names in SSH queue and interruption discovery."""
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def helper(file, constant, tail):
    text = (ROOT / 'src-tauri/src/modules' / file).read_text()
    code = text.split(f'const {constant}: &str = r#"', 1)[1].split('"#;', 1)[0]
    namespace = {}
    exec(code.split(tail, 1)[0], namespace)
    return namespace


DISCOVERY = helper('codex_task_queue.rs', 'PYTHON_SSH_DISCOVERY', '\ntry:\n    res = handle()')
PROBE = helper('codex_auto_resume.rs', 'PYTHON_REMOTE_PROBE', '\nprint("__CODEX_AUTO_RESUME_JSON_START__")')


class TaskTitleTests(unittest.TestCase):
    def test_names_fallbacks_and_old_schema(self):
        for named_schema in (True, False):
            with self.subTest(named_schema=named_schema), tempfile.TemporaryDirectory() as folder:
                home = Path(folder)
                conn = sqlite3.connect(home / 'state_5.sqlite')
                conn.execute('CREATE TABLE threads(id TEXT, title TEXT, model TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER, thread_source TEXT, source TEXT, rollout_path TEXT' + (', name TEXT' if named_schema else '') + ')')
                for tid, name in [('regular', 'Sidebar title'), ('goal', 'Goal sidebar'), ('blank', '  '), ('null', None)]:
                    values = [tid, f'Opening prompt {tid}', 'model', '/project', int(time.time()), 0, None, None, '/unused']
                    if named_schema:
                        values.append(name)
                    conn.execute('INSERT INTO threads VALUES (' + ','.join('?' for _ in values) + ')', values)
                conn.commit()
                goals = sqlite3.connect(home / 'goals_1.sqlite')
                goals.execute('CREATE TABLE thread_goals(thread_id TEXT, goal_id TEXT, objective TEXT, status TEXT, updated_at_ms INTEGER)')
                goals.execute("INSERT INTO thread_goals VALUES ('goal','g','Original objective','usage_limited',?)", (int(time.time()) * 1000,))
                goals.commit()
                expected = {'regular': 'Sidebar title', 'goal': 'Goal sidebar', 'blank': 'Opening prompt blank', 'null': 'Opening prompt null'} if named_schema else {tid: f'Opening prompt {tid}' for tid in ('regular', 'goal', 'blank', 'null')}
                def discover(action, **extra):
                    args = json.dumps({'action': action, 'codex_home': folder, **extra})
                    with patch('sys.argv', ['helper', args]):
                        return DISCOVERY['handle']()
                for cwd in (None, '/project'):
                    result = discover('list_threads', cwd=cwd)
                    self.assertEqual({t['id']: t['title'] for t in result['threads']}, expected)
                # Explicit ID refresh works for persisted entries, including renames.
                self.assertEqual(discover('thread_titles', thread_ids=['regular', 'missing'])['titles'], {'regular': expected['regular']})
                if named_schema:
                    conn.execute("UPDATE threads SET name='Renamed sidebar title' WHERE id='regular'")
                    conn.commit()
                    expected['regular'] = 'Renamed sidebar title'
                    self.assertEqual(discover('thread_titles', thread_ids=['regular'])['titles'], {'regular': expected['regular']})
                with patch.dict(os.environ, {'CODEX_HOME': folder}), patch.dict(PROBE, {
                    'parse_rollout_tail': lambda path: (0, 1, 'quota', True),
                    'is_thread_in_progress': lambda *args: False,
                    'restart_interruption': lambda *args: None,
                }):
                    tasks = PROBE['detect_interrupted_tasks']()
                self.assertEqual({t['thread_id']: t['title'] for t in tasks}, expected)
                self.assertTrue(next(t for t in tasks if t['thread_id'] == 'goal')['is_goal'])
                goals.close()
                conn.close()


if __name__ == '__main__':
    unittest.main()
