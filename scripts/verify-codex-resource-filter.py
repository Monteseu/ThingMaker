#!/usr/bin/env python3
"""Real Codex app-server skill-filter probe; no global configuration writes.

Spends one short subscription turn and inspects the actual transcript's skill
instructions, rather than trusting a model's description of its tools.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time

sys.dont_write_bytecode = True
module = importlib.util.spec_from_file_location('claude_probe', Path(__file__).with_name('verify-claude-resource-filter.py'))
probe = importlib.util.module_from_spec(module)
module.loader.exec_module(probe)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default=shutil.which('codex'))
    parser.add_argument('--model', default='gpt-6-luna')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='thingmaker-shared-codex-') as scratch:
        root = Path(scratch)
        (root / '.git').mkdir()
        probe.skill(root / '.agents/skills', 'qa-native-probe', 'NATIVE-LEAK')
        probe.skill(root / 'external', 'shared-art-probe', 'ART-PASS')
        server = root / 'mcp.py'
        server.write_text(probe.SERVER)
        log = root / 'mcp.log'
        spec = lambda name: {'command': sys.executable, 'args': [str(server), str(log), name]}
        excluded = spec('excluded')
        peer = probe.Peer(args.codex, 'app-server', root, extra_args=['-c', f'mcp_servers.excluded.command={json.dumps(excluded["command"])}', '-c', f'mcp_servers.excluded.args={json.dumps(excluded["args"])}'])
        try:
            initialized = peer.call('initialize', {'clientInfo': {'name': 'thingmaker-resource-probe', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
            peer.send({'jsonrpc': '2.0', 'method': 'initialized'})
            print('Codex initialized', flush=True)
            peer.call('skills/extraRoots/set', {'extraRoots': [str(root / 'external')]})
            discovered = peer.call('skills/list', {'cwds': [str(root)], 'forceReload': True})
            skills = discovered['data'][0]['skills']
            selected = [skill for skill in skills if skill['name'] == 'shared-art-probe']
            assert len(selected) == 1, 'extra root skill must be discovered'
            config = peer.call('config/read', {'includeLayers': False})['config']
            overrides = {
                'skills.config': [{'path': skill['path'], 'enabled': skill['name'] == 'shared-art-probe'} for skill in skills],
                'skills.bundled.enabled': False, 'features.apps': False, 'features.multi_agent': False,
                'skills.include_instructions': False, 'agents.enabled': False,
            }
            overrides['mcp_servers'] = {}
            overrides['plugins'] = {name: {'enabled': False} for name in config.get('plugins', {})}
            overrides['mcp_servers']['selected'] = {**spec('selected'), 'required': True, 'enabled': True}
            instructions = '<skills_instructions>\n## Available skills\n' + json.dumps({'name':selected[0]['name'],'description':selected[0]['description'],'path':selected[0]['path']}) + '\n</skills_instructions>'
            started = peer.call('thread/start', {'cwd': str(root), 'model': args.model, 'approvalPolicy': 'on-request', 'sandbox': 'read-only', 'config': overrides, 'developerInstructions':instructions})
            thread = started['thread']
            print('Thread opened with selected skill and native exclusions', flush=True)
            peer.call('turn/start', {'threadId': thread['id'], 'input': [{'type': 'text', 'text': 'Call the selected MCP server stamp tool once, then reply OK. Do not use shell tools or load skills.'}], 'effort': 'low'})
            until = time.monotonic() + 180
            while time.monotonic() < until:
                message = peer.messages.get(timeout=max(.1, until - time.monotonic()))
                peer.events.append(message)
                if message.get('method') == 'turn/completed':
                    assert message['params']['turn']['status'] == 'completed', 'turn must complete'
                    break
                if message.get('id') is not None and message.get('method'):
                    params = message.get('params', {})
                    if message['method'] == 'mcpServer/elicitation/request' and params.get('serverName') == 'selected':
                        print('Fixture approval schema:', json.dumps(params.get('requestedSchema')), flush=True)
                        schema = params.get('requestedSchema', {})
                        content = {}
                        for key, field in schema.get('properties', {}).items():
                            content[key] = field.get('default', True if field.get('type') == 'boolean' else (field.get('enum') or ['approve'])[0])
                        peer.send({'jsonrpc': '2.0', 'id': message['id'], 'result': {'action':'accept','content':content,'_meta':None}})
                    else:
                        peer.send({'jsonrpc': '2.0', 'id': message['id'], 'error': {'code': -32601, 'message': 'Probe does not grant extra permissions'}})
            else:
                raise TimeoutError('turn did not complete')
            path = thread.get('path')
            if not path:
                loaded = peer.call('thread/read', {'threadId': thread['id'], 'includeTurns': False})
                path = loaded['thread'].get('path')
            transcript = [json.loads(line) for line in Path(path).read_text().splitlines()]
            instructions = []
            for entry in transcript:
                payload = entry.get('payload', {})
                if entry.get('type') == 'response_item' and payload.get('type') == 'message' and payload.get('role') in ('user', 'developer'):
                    text = '\n'.join(block.get('text', '') for block in payload.get('content', []))
                    if 'Available skills' in text or '<skills_instructions>' in text:
                        instructions.append(text)
            assert instructions, 'actual skill instructions must be captured'
            text = '\n'.join(instructions)
            checks = {
                'selected_external_skill_in_actual_context': 'shared-art-probe' in text,
                'excluded_project_skill_absent_from_context': 'qa-native-probe' not in text,
                'only_selected_skill_paths_in_context': all(skill['path'] not in text for skill in skills if skill['name'] != 'shared-art-probe'),
                'selected_mcp_called': log.exists() and 'selected:called' in log.read_text(),
                'excluded_native_mcp_never_started': not log.exists() or 'excluded:started' not in log.read_text(),
            }
            print(json.dumps(checks, indent=2), flush=True)
            if not all(checks.values()):
                print('Fixture MCP log:', log.read_text() if log.exists() else '(not started)')
                for event in peer.events:
                    if event.get('method') == 'item/completed':
                        item = event.get('params', {}).get('item', {})
                        if item.get('type') in ('agentMessage', 'mcpToolCall'):
                            print('Probe result:', json.dumps(item)[:4000])
                raise SystemExit('Codex resource probe did not pass')
        finally:
            peer.close()


if __name__ == '__main__':
    main()
