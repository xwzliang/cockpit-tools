"""Exercise the shipped queue helper against a deterministic WebSocket peer."""
import json
from pathlib import Path
import unittest
from unittest.mock import patch

SOURCE = (Path(__file__).resolve().parents[1] / 'src-tauri/src/modules/codex_task_queue.rs').read_text()
HELPER = SOURCE.split('const PYTHON_QUEUE_EXECUTE: &str = r#"', 1)[1].split('"#;', 1)[0]
NAMESPACE = {}
exec(HELPER.split('\ntry:\n    res = execute()', 1)[0], NAMESPACE)
PROMPT = 'Continue the validation.'


class Peer:
    def __init__(self, scenario):
        self.scenario = scenario
        self.incoming = bytearray()
        self.requests = []
        self.reads = 0
        self.elapsed = 0
        self.client_id = None

    def connect(self, path):
        pass

    def settimeout(self, value):
        pass

    def close(self):
        pass

    def recv(self, size):
        if not self.incoming:
            raise TimeoutError('No response')
        chunk = self.incoming[:size]
        del self.incoming[:size]
        return bytes(chunk)

    def sendall(self, frame):
        if frame.startswith(b'GET'):
            self.incoming.extend(b'HTTP/1.1 101 Switching Protocols\r\n\r\n')
            return
        length = frame[1] & 127
        offset = 2
        if length == 126:
            length = int.from_bytes(frame[2:4], 'big')
            offset = 4
        elif length == 127:
            length = int.from_bytes(frame[2:10], 'big')
            offset = 10
        mask = frame[offset:offset+4]
        request = json.loads(bytes(v ^ mask[i % 4] for i, v in enumerate(frame[offset+4:])))
        self.requests.append(request)
        if 'id' not in request:
            return
        method = request['method']
        result = {}
        if method == 'turn/steer':
            self.client_id = request['params']['clientUserMessageId']
            result = {'turnId': 'wrong' if self.scenario == 'mismatch' else 'turn'}
        elif method == 'thread/read':
            self.reads += 1
            if self.reads > 1 and self.scenario == 'disconnect':
                return
            items = [{'type': 'userMessage', 'id': 'old', 'content': [{'type': 'text', 'text': PROMPT}]}]
            if self.reads >= 3 and self.scenario in ('confirmed', 'legacy', 'other_turn', 'wrong_client'):
                item = {'type': 'userMessage', 'id': 'new', 'content': [{'type': 'text', 'text': PROMPT}]}
                if self.scenario != 'legacy':
                    item['clientId'] = 'other-client' if self.scenario == 'wrong_client' else self.client_id
                items.append(item)
            turns = [{'id': 'turn', 'status': 'inProgress', 'items': items}]
            if self.scenario == 'other_turn' and len(items) > 1:
                turns.append({'id': 'other-turn', 'items': [items.pop()]})
            result = {'thread': {'turns': turns}}
        response = {'id': request['id'], 'result': result}
        if method == 'turn/steer' and self.scenario == 'rejected':
            response = {'id': request['id'], 'error': {'message': 'no active turn'}}
        payload = json.dumps(response).encode()
        header = bytes([0x81, len(payload)]) if len(payload) < 126 else bytes([0x81, 126]) + len(payload).to_bytes(2, 'big')
        self.incoming.extend(header + payload)

    def sleep(self, seconds):
        self.elapsed += seconds


class SteeringTests(unittest.TestCase):
    def run_scenario(self, scenario):
        peer = Peer(scenario)
        data = json.dumps({'thread_id': 'thread', 'prompt': PROMPT, 'steer_only': True, 'model': 'ignored'})
        with patch('sys.argv', ['helper', data]), patch('pathlib.Path.exists', return_value=True), \
             patch('socket.socket', return_value=peer), patch('time.monotonic', side_effect=lambda: peer.elapsed), \
             patch('time.sleep', side_effect=peer.sleep):
            result = NAMESPACE['execute']()
        sends = [r for r in peer.requests if r['method'] == 'turn/steer']
        self.assertEqual(len(sends), 1)
        self.assertNotIn('model', sends[0]['params'])
        self.assertNotIn('cwd', sends[0]['params'])
        self.assertFalse(any(r['method'] in ('turn/start', 'thread/resume') for r in peer.requests))
        return result

    def test_confirm_recorded_message(self):
        for scenario in ('confirmed', 'legacy'):
            with self.subTest(scenario=scenario):
                result = self.run_scenario(scenario)
                self.assertTrue(result['success'])
                self.assertEqual(result['steer_message_id'], 'new')

    def test_acknowledged_is_not_recorded(self):
        for scenario in ('pending', 'disconnect', 'other_turn', 'wrong_client'):
            with self.subTest(scenario=scenario):
                result = self.run_scenario(scenario)
                self.assertTrue(result['success'])
                self.assertIsNone(result['steer_message_id'])

    def test_rejected_or_mismatched_acknowledgement(self):
        for scenario in ('rejected', 'mismatch'):
            with self.subTest(scenario=scenario):
                self.assertFalse(self.run_scenario(scenario)['success'])


if __name__ == '__main__':
    unittest.main()
