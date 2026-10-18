"""Read-only activity check before an account switch, including background turns."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time

def oldest_codex_process_start():
    try:
        output = subprocess.check_output(["ps", "-axo", "etime=,comm="], text=True)
        ages = []
        for line in output.splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) != 2 or "codex" not in Path(fields[1]).name.lower():
                continue
            elapsed = fields[0]
            days, clock = elapsed.split("-", 1) if "-" in elapsed else ("0", elapsed)
            seconds = 0
            for part in clock.split(":"):
                seconds = seconds * 60 + int(part)
            ages.append(int(days) * 86400 + seconds)
        return time.time() - max(ages) - 5 if ages else time.time()
    except (OSError, ValueError, subprocess.SubprocessError):
        return None

process_start = oldest_codex_process_start()

home = Path(os.environ.get('CODEX_HOME') or Path.home() / '.codex')
running = False
history = {}
if (home / 'thread_history_1.sqlite').exists():
    with sqlite3.connect((home / 'thread_history_1.sqlite').as_uri() + '?mode=ro', uri=True) as db:
        for tid, status, started_at in db.execute('SELECT thread_id, status, started_at FROM thread_turns ORDER BY started_at DESC'):
            history.setdefault(tid, (status, started_at))
if (home / 'state_5.sqlite').exists():
    with sqlite3.connect((home / 'state_5.sqlite').as_uri() + '?mode=ro', uri=True) as db:
        for tid, path in db.execute('SELECT id, rollout_path FROM threads WHERE archived = 0'):
            status, started_at = history.get(tid, (None, None))
            active = status in ('inProgress', 'in_progress', 'running')
            if path:
                try:
                    with open(path, 'rb') as stream:
                        stream.seek(0, 2)
                        stream.seek(max(0, stream.tell() - 262144))
                        lines = stream.read().decode('utf-8', errors='replace').splitlines()
                except FileNotFoundError:
                    # Thread rows can outlive deleted rollouts. Preserve active
                    # history, without blocking switches on old missing logs.
                    lines = []
                    if process_start is not None and started_at is not None and started_at < process_start:
                        active = False
                for line in reversed(lines):
                    try:
                        event = json.loads(line)
                    except ValueError:
                        continue
                    payload = event.get('payload', {})
                    if event.get('type') == 'event_msg':
                        kind = payload.get('type')
                        if kind in ('task_complete', 'task_completed', 'turn_aborted'):
                            active = False
                            break
                        if kind == 'task_started':
                            active = True
                            break
                        if kind in ('agent_reasoning', 'agent_message', 'exec_command_begin', 'exec_command_end'):
                            active = True
                            break
                    elif event.get('type') == 'response_item' and payload.get('type') != 'compaction':
                        active = True
                        break
            if active:
                print("__CODEX_ACTIVE_THREAD__" + tid)
                running = True
                break
print('__CODEX_RUNNING__' + json.dumps(running))
