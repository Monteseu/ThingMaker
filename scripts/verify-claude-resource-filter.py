#!/usr/bin/env python3
"""Opt-in real ACP probe. Uses the installed adapter and a scratch workspace.

No personal configuration is edited. A few Claude subscription tokens are used.
Run with --adapter /path/to/claude-agent-acp/dist/index.js.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time


SERVER = r'''
import json, sys
from pathlib import Path
log, label = Path(sys.argv[1]), sys.argv[2]
with log.open('a') as f: f.write(label + ':started\n')
for line in sys.stdin:
    m = json.loads(line)
    method = m.get('method')
    if 'id' not in m: continue
    result = {}
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': label, 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'stamp', 'description': 'Return the probe marker', 'inputSchema': {'type': 'object', 'properties': {}}}]}
    elif method == 'tools/call':
        with log.open('a') as f: f.write(label + ':called\n')
        result = {'content': [{'type': 'text', 'text': label + '-MCP-PASS'}]}
    print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': result}), flush=True)
'''


class Peer:
    def __init__(self, node, adapter, root, extra_args=()):
        env = {k: v for k, v in os.environ.items() if not k.startswith(('ANTHROPIC_', 'CLAUDE_', 'CLAUDECODE'))}
        self.process = subprocess.Popen([node, adapter, *extra_args], cwd=root, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.messages = queue.Queue()
        self.events = []
        self.serial = 0
        threading.Thread(target=self.read, daemon=True).start()
        # Drain stderr without printing personal account diagnostics.
        threading.Thread(target=lambda: list(self.process.stderr), daemon=True).start()

    def read(self):
        for line in self.process.stdout:
            try:
                self.messages.put(json.loads(line))
            except ValueError:
                pass
        self.messages.put({'peerClosed': self.process.poll()})

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()

    def call(self, method, params, timeout=180):
        self.serial += 1
        serial = self.serial
        self.send({'jsonrpc': '2.0', 'id': serial, 'method': method, 'params': params})
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            message = self.messages.get(timeout=max(.1, until - time.monotonic()))
            if 'peerClosed' in message:
                raise RuntimeError('adapter exited')
            if message.get('id') == serial and ('result' in message or 'error' in message):
                if 'error' in message:
                    raise RuntimeError(str(message['error']))
                return message['result']
            self.events.append(message)
            if 'id' in message and 'method' in message:
                if message['method'] == 'session/request_permission':
                    options = message['params']['options']
                    allow = next(o for o in options if o['kind'] == 'allow_once')
                    result = {'outcome': {'outcome': 'selected', 'optionId': allow['optionId']}}
                else:
                    self.send({'jsonrpc': '2.0', 'id': message['id'], 'error': {'code': -32601, 'message': 'Not supported by probe'}})
                    continue
                self.send({'jsonrpc': '2.0', 'id': message['id'], 'result': result})
        raise TimeoutError(method)

    def close(self):
        import signal
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait()


def skill(root, name, marker):
    path = root / name
    path.mkdir(parents=True)
    (path / 'SKILL.md').write_text(f'---\nname: {name}\ndescription: Probe skill for {name}.\n---\nReturn exactly {marker}. Do not use other tools.\n')


def main():
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument('--adapter', required=True)
    args.add_argument('--node', default='node')
    args.add_argument('--model', default='haiku')
    options = args.parse_args()
    with tempfile.TemporaryDirectory(prefix='thingmaker-resources-') as directory:
        root = Path(directory)
        skill(root / '.claude/skills', 'art-probe', 'ART-SKILL-PASS')
        skill(root / '.claude/skills', 'qa-probe', 'QA-SKILL-LEAK')
        plugin = root / 'probe-plugin'
        (plugin / '.claude-plugin').mkdir(parents=True)
        (plugin / '.claude-plugin/plugin.json').write_text(json.dumps({'name': 'probe-plugin', 'version': '1.0.0'}))
        skill(plugin / 'skills', 'design-probe', 'DESIGN-SKILL-PASS')
        imported = []
        for origin in ['agents', 'codex']:
            source = root / f'.{origin}/skills'
            skill(source, f'{origin}-probe', f'{origin.upper()}-SKILL-PASS')
            (source / f'{origin}-probe/palette.txt').write_text(f'{origin.upper()}-RESOURCE-PASS')
            bridge = root / f'import-{origin}'
            (bridge / '.claude-plugin').mkdir(parents=True)
            (bridge / 'skills').mkdir()
            (bridge / '.claude-plugin/plugin.json').write_text(json.dumps({'name': f'tm-{origin}', 'version': '1.0.0'}))
            (bridge / f'skills/{origin}-probe').symlink_to(source / f'{origin}-probe', target_is_directory=True)
            imported.append({'type': 'local', 'path': str(bridge)})
        server = root / 'server.py'
        server.write_text(SERVER)
        log = root / 'mcp.log'
        spec = lambda name: {'command': sys.executable, 'args': [str(server), str(log), name]}
        (root / '.mcp.json').write_text(json.dumps({'mcpServers': {'excluded': spec('excluded')}}))
        (plugin / '.mcp.json').write_text(json.dumps({'mcpServers': {'excluded-plugin': spec('excluded-plugin')}}))
        peer = Peer(options.node, str(Path(options.adapter).resolve()), root)
        try:
            initialized = peer.call('initialize', {'protocolVersion': 1, 'clientCapabilities': {}, 'clientInfo': {'name': 'thingmaker-filter-probe', 'version': '1'}})
            print('adapter:', initialized.get('agentInfo'), flush=True)
            internal = {'name': 'team-probe', **spec('internal'), 'env': []}
            new = peer.call('session/new', {'cwd': str(root), 'mcpServers': [internal], '_meta': {'claudeCode': {'emitRawSDKMessages': True, 'options': {
                'model': options.model, 'settingSources': ['project'], 'tools': ['Skill', 'Read'],
                'skills': ['art-probe', 'probe-plugin:design-probe', 'tm-agents:agents-probe', 'tm-codex:codex-probe'], 'strictMcpConfig': True,
                'plugins': [{'type': 'local', 'path': str(plugin)}, *imported], 'mcpServers': {'plugin:probe-plugin:selected': spec('selected')},
            }}}})
            sid = new['sessionId']
            print('session opened; invoking allowed skills and MCP, then attempting excluded skill', flush=True)
            peer.call('session/prompt', {'sessionId': sid, 'prompt': [{'type': 'text', 'text':
                'This is a tool-filter integration test. First call Skill with skill="art-probe". '
                'Then call Skill with skill="probe-plugin:design-probe". Then call the stamp tool from the selected server and the stamp tool from team-probe. '
                'Also call Skill with skill="tm-agents:agents-probe" and then Skill with skill="tm-codex:codex-probe". '
                f'Then use Read on {root}/import-agents/skills/agents-probe/palette.txt and {root}/import-codex/skills/codex-probe/palette.txt. '
                'Finally attempt Skill with skill="qa-probe" exactly once even if it is not listed: its expected rejection is the test. '
                'Do not use slash commands. Report results briefly.'}]}, timeout=240)
            raw = json.dumps(peer.events)
            calls = log.read_text() if log.exists() else ''
            # A result must come from a tool notification, not the model's final answer.
            tool_events = [e for e in peer.events if e.get('method') == 'session/update' and e.get('params', {}).get('update', {}).get('sessionUpdate') in ('tool_call', 'tool_call_update')]
            evidence = json.dumps(tool_events)
            checks = {
                'allowed_local_skill': any(e['params']['update'].get('_meta', {}).get('claudeCode', {}).get('toolResponse') == {'success': True, 'commandName': 'art-probe'} for e in tool_events),
                'allowed_plugin_skill': any(e['params']['update'].get('_meta', {}).get('claudeCode', {}).get('toolResponse') == {'success': True, 'commandName': 'probe-plugin:design-probe'} for e in tool_events),
                'agents_skill_loaded_via_temporary_plugin': any(e['params']['update'].get('_meta', {}).get('claudeCode', {}).get('toolResponse') == {'success': True, 'commandName': 'tm-agents:agents-probe'} for e in tool_events),
                'codex_skill_loaded_via_temporary_plugin': any(e['params']['update'].get('_meta', {}).get('claudeCode', {}).get('toolResponse') == {'success': True, 'commandName': 'tm-codex:codex-probe'} for e in tool_events),
                'external_supporting_files_readable': 'AGENTS-RESOURCE-PASS' in evidence and 'CODEX-RESOURCE-PASS' in evidence,
                'selected_mcp_called': 'selected:called' in calls,
                'excluded_mcp_never_started': 'excluded:started' not in calls,
                'excluded_plugin_mcp_never_started': 'excluded-plugin:started' not in calls,
                'internal_acp_mcp_preserved': 'internal:called' in calls,
                'excluded_skill_rejected': any('qa-probe' in str(e['params']['update'].get('rawOutput', '')) and 'allowlist' in str(e['params']['update'].get('rawOutput', '')) and e['params']['update'].get('status') == 'failed' for e in tool_events),
                'excluded_skill_body_not_loaded': 'QA-SKILL-LEAK' not in raw,
            }
            print(json.dumps(checks, indent=2), flush=True)
            if not all(checks.values()):
                # Fixtures and tool evidence only; no inherited config or credentials.
                print(evidence[:24000])
                raise SystemExit(1)
            # A second live session must not inherit the first one's allowlist.
            start = len(peer.events)
            empty = peer.call('session/new', {'cwd': str(root), 'mcpServers': [], '_meta': {'claudeCode': {'options': {
                'model': options.model, 'settingSources': ['project'], 'tools': ['Skill'],
                'skills': [], 'strictMcpConfig': True, 'mcpServers': {},
            }}}})
            peer.call('session/prompt', {'sessionId': empty['sessionId'], 'prompt': [{'type': 'text', 'text':
                'Integration test: attempt Skill with skill="art-probe" exactly once, even if not listed. '
                'Its rejection is the expected result. Do not use slash commands or other tools.'}]}, timeout=120)
            updates = [e['params']['update'] for e in peer.events[start:] if e.get('method') == 'session/update']
            rejected = any(update.get('status') == 'failed' and 'allowlist' in str(update.get('rawOutput')) for update in updates)
            unchanged = log.read_text() == calls
            print(json.dumps({'empty_selection_rejected_skill': rejected, 'second_session_started_no_mcps': unchanged}, indent=2), flush=True)
            if not rejected or not unchanged:
                raise SystemExit('empty-selection isolation failed')
        finally:
            peer.close()


if __name__ == '__main__':
    main()
