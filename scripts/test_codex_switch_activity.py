import base64
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
from datetime import datetime, timezone

root = Path(__file__).resolve().parents[1]
source = (root / 'src-tauri/src/modules/codex_auto_resume.rs').read_text()
probe = source.split('const PYTHON_REMOTE_PROBE: &str = r#"', 1)[1].split('"#;', 1)[0]
progress = source.split('const PYTHON_RECOVERY_PROGRESS: &str = r#"', 1)[1].split('"#;', 1)[0]
now = int(time.time())
with tempfile.TemporaryDirectory() as folder:
    home = Path(folder)
    rollout = home / 'rollout.jsonl'
    history = sqlite3.connect(home / 'thread_history_1.sqlite')
    history.execute('CREATE TABLE thread_turns(thread_id TEXT,status TEXT,started_at INTEGER)')
    history.execute("INSERT INTO thread_turns VALUES ('t','interrupted',?)", (now,))
    history.commit()
    state = sqlite3.connect(home / 'state_5.sqlite')
    state.execute('CREATE TABLE threads(id TEXT,rollout_path TEXT,archived INTEGER,cwd TEXT,title TEXT,model TEXT,updated_at INTEGER,thread_source TEXT,source TEXT)')
    state.execute("INSERT INTO threads VALUES ('t',?,0,'/tmp','task','model',?,NULL,NULL)", (str(rollout), now))
    state.commit()
    env = {**os.environ, 'CODEX_HOME': folder, 'COCKPIT_CODEX_SWITCH_AT': str(now)}
    def events(kinds, at=now, reason=None):
        rollout.write_text('\n'.join(json.dumps({'type':'event_msg','timestamp':datetime.fromtimestamp(at, timezone.utc).isoformat(), 'payload':{'type':kind, 'reason':reason}}) for kind in kinds))
    def activity():
        # Use a deterministic process lifetime, independent of the test runner.
        wrapper = "import runpy; from unittest.mock import patch; " + "\nwith patch('subprocess.check_output', return_value='01:00 codex\\n'):\n runpy.run_path(" + repr(str(root / 'src-tauri/src/modules/codex_activity_probe.py')) + ")"
        return subprocess.check_output(['python3', '-c', wrapper], env=env, text=True).strip().splitlines()[-1]
    for kinds, active in [(['task_started','exec_command_begin'],True),(['task_started','agent_reasoning'],True),(['task_started','error'],True),(['task_started','task_complete'],False),(['task_started','turn_aborted'],False)]:
        events(kinds)
        assert activity() == '__CODEX_RUNNING__' + json.dumps(active)
    # A deleted historical rollout must not break the entire host probe, and
    # must not hide an active turn recorded in history or another rollout.
    rollout.unlink()
    assert activity() == '__CODEX_RUNNING__false'
    history.execute("UPDATE thread_turns SET status='inProgress'")
    history.commit()
    assert activity() == '__CODEX_RUNNING__true'
    history.execute("UPDATE thread_turns SET status='interrupted'")
    history.commit()
    history.execute("UPDATE thread_turns SET status='inProgress', started_at=1")
    history.commit()
    assert activity() == '__CODEX_RUNNING__false'
    history.execute("UPDATE thread_turns SET status='interrupted', started_at=?", (now,))
    history.commit()
    state.execute("INSERT INTO threads VALUES ('missing',?,0,'/tmp','old','model',?,NULL,NULL)", (str(home / 'deleted.jsonl'), now))
    state.commit()
    events(['task_started', 'exec_command_begin'])
    assert activity() == '__CODEX_RUNNING__true'
    events(['task_started', 'task_complete'])
    assert activity() == '__CODEX_RUNNING__false'
    # Hydrated/forked chats may contain only a compaction snapshot, with no turn.
    snapshot = json.dumps({'type':'response_item', 'payload':{'type':'compaction'}})
    rollout.write_text(snapshot)
    assert activity() == '__CODEX_RUNNING__false'
    events(['task_started'])
    with rollout.open('a') as stream:
        stream.write('\n' + snapshot)
    assert activity() == '__CODEX_RUNNING__true'
    events(['task_started', 'task_complete'])
    with rollout.open('a') as stream:
        stream.write('\n' + snapshot)
    assert activity() == '__CODEX_RUNNING__false'
    rollout.write_text(snapshot)
    history.execute("UPDATE thread_turns SET status='inProgress'")
    history.commit()
    assert activity() == '__CODEX_RUNNING__true'
    history.execute("UPDATE thread_turns SET status='interrupted'")
    history.commit()
    def detected():
        output = subprocess.check_output(['python3','-c',probe],env=env,text=True)
        return json.loads(output.split('__CODEX_AUTO_RESUME_JSON_START__')[1].split('__CODEX_AUTO_RESUME_JSON_END__')[0])
    # Routing failures are recoverable only when the latest turn ended in error.
    events(['task_started'])
    with rollout.open('a') as stream:
        stream.write('\n' + json.dumps({'type':'event_msg', 'payload':{'type':'task_complete', 'error':{'message':'workspace routing discovery failed','codex_error_info':'other'}}}))
    assert len(detected()) == 1
    with rollout.open('a') as stream:
        stream.write('\n' + json.dumps({'type':'event_msg', 'payload':{'type':'task_started'}}))
    assert detected() == []
    events(['task_started', 'task_complete'])
    assert detected() == []
    events(['task_started','turn_aborted'])
    tasks = detected()
    assert len(tasks)==1 and tasks[0]['restart_interrupted_at']==now
    args = base64.b64encode(json.dumps({'thread_id':'t','since':0,'restart_interrupted_at':now}).encode()).decode()
    assert subprocess.check_output(['python3','-c',progress,args],env=env,text=True).strip()=='__RECOVERY_PROGRESS__retry'
    for kinds, at, reason in [(['task_started','turn_aborted'],now-600,None),(['task_started','task_complete'],now,None),(['task_started','turn_aborted'],now,'user_requested'),(['task_started','turn_aborted','task_started'],now,None)]:
        events(kinds,at,reason)
        assert detected()==[], (kinds,at,reason)
    events(['task_started','turn_aborted'])
    env['COCKPIT_CODEX_SWITCH_AT']='0'
    assert detected()==[]
print('Codex switch activity and restart recovery checks passed')
