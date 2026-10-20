#!/usr/bin/env python3
"""Run the real macOS WebKit boundary with a loopback TCP/UDP traffic sink.

No Grove app is launched. Writes raw JSON, compiler output and traffic evidence
under --output; exits nonzero for any violated invariant.
"""
import argparse
import http.server
import json
from pathlib import Path
import re
import socket
import subprocess
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    repo = Path(__file__).resolve().parents[1]
    traffic = []
    class Sink(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            traffic.append({'transport': 'http', 'path': self.path})
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'loopback marker')
        do_POST = do_GET
        def log_message(self, *_): pass
    class Server(http.server.ThreadingHTTPServer):
        def get_request(self):
            connection, address = super().get_request()
            traffic.append({'transport': 'tcp'})
            return connection, address
    server = Server(('127.0.0.1', 0), Sink)
    port = server.server_address[1]
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(('127.0.0.1', port))
    udp.settimeout(0.1)
    stopped = threading.Event()
    def receive():
        while not stopped.is_set():
            try:
                data, _ = udp.recvfrom(65535)
                traffic.append({'transport': 'udp', 'bytes': len(data)})
            except socket.timeout: pass
    threads = [threading.Thread(target=server.serve_forever, daemon=True), threading.Thread(target=receive, daemon=True)]
    for thread in threads: thread.start()
    # Prove both sinks receive traffic before testing blocked probes.
    with socket.create_connection(('127.0.0.1', port)) as check:
        check.sendall(b'GET /listener-self-test HTTP/1.0\r\n\r\n')
        while check.recv(4096): pass
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as check:
        check.sendto(b'listener-self-test', ('127.0.0.1', port))
    threading.Event().wait(0.15)
    assert {row['transport'] for row in traffic} == {'tcp', 'http', 'udp'}, traffic
    sink_self_test = traffic[:]
    traffic.clear()
    csp = re.search(r'const CSP: &str = "([^"]*)"', (repo/'src/views/inline_html.rs').read_text())[1]
    source = '<!doctype html><meta http-equiv="Content-Security-Policy" content="'+csp+'">' + '''<html><head><title>Inline HTML native fixture</title></head><body>
<input id="name" value="Original"><button id="count" onclick="this.textContent=Number(this.textContent)+1">0</button>
<button id="tasks" onclick="document.body.dataset.tab='tasks'">Tasks</button><a href="#internal-anchor">Jump</a>
<div style="height:2400px">Long document</div><div id="internal-anchor">Anchor</div></body></html>'''
    fixture = output/'native-fixture.html'
    fixture.write_text(source)
    binary = output/'verify-webkit'
    command = ['clang', '-fobjc-arc', '-fblocks', str(repo/'scripts/verify-inline-html-webkit.m'), '-framework', 'AppKit', '-framework', 'WebKit', '-framework', 'QuartzCore', '-o', str(binary)]
    try:
        compile_result = subprocess.run(command, capture_output=True, text=True)
        (output/'native-compile.log').write_text(compile_result.stdout+compile_result.stderr)
        compile_result.check_returncode()
        result = subprocess.run([str(binary), str(fixture), str(port)], capture_output=True, text=True, timeout=30)
        (output/'native-webkit.json').write_text(result.stdout)
        (output/'native-webkit.stderr').write_text(result.stderr)
        result.check_returncode()
        raw = json.loads(result.stdout)
        checks = []
        def require(condition, label):
            checks.append({'check': label, 'passed': bool(condition)})
        def decoded(key):
            value = raw.get(key)
            return json.loads(value) if isinstance(value, str) else value
        initial = decoded('initial')
        require(raw.get('loaded') == 1, 'real WebKit document loaded')
        accessibility = raw.get('accessibility', {})
        for key in ['native_root_children', 'synthetic_child_preserved', 'host_focus_preserved',
                    'outside_clip_host_hit', 'native_focus', 'native_hit', 'hidden_excluded', 'disposed_excluded']:
            require(accessibility.get(key) is True, 'native AX boundary: ' + key)
        require(raw.get('editing_dispatch') is True, 'native editing shortcuts dispatch to owned responder')
        require(raw.get('composer_reclaims_native_space') is True, 'composer reclaims native focus and receives Space after HTML interaction')
        require(raw.get('composer_preserves_other_native_focus') is True, 'composer handoff leaves unrelated native responders untouched')
        require(raw.get('editing_host_and_hidden_untouched') is True, 'editing routing leaves host hidden and unsupported keys untouched')
        require(raw.get('editing_webkit_select_all') is True, 'Command-A selects entire real WebKit input')
        require(raw.get('editing_webkit_copy_cut_paste') is True, 'Command-C/X/V copy cut and paste real WebKit input')
        require(raw.get('editing_webkit_undo_redo') is True, 'Command-Z and Shift-Command-Z undo and redo native WebKit edits')
        require(raw.get('editing_undo_stack_owned') is True, 'WebKit editing uses dedicated clip undo stack distinct from host window')
        require(raw.get('dynamic_js') is True, 'self-contained author JavaScript retains eval and Function semantics')
        require(initial.get('doctype') == 'CSS1Compat' and initial.get('title') == 'Inline HTML native fixture', 'author HTML loaded in standards mode')
        require(initial.get('guard') == 'undefined' and initial.get('bridge') == [], 'no RTC constructor or native message bridge')
        expected = {'value': 'Retained edit', 'count': '1', 'tab': 'tasks', 'scroll': 700}
        require(decoded('mutate') == expected, 'author form/JS/tab/scroll are interactive')
        require(decoded('retained') == expected, 'same document retains edits/tab/scroll after hide and resize')
        require(raw.get('replacement_frame_keeps_surface') is True, 'replacement GPUI frame does not detach a repainted surface or surrender native responder')
        require(raw.get('omitted_frame_retires_surface') is True, 'omitted GPUI frame detaches retired surface on main queue')
        realm = decoded('fresh_realm')
        require(realm.get('rtc') == 'undefined' and realm.get('worker') == 'undefined' and not realm.get('created'), 'synchronous fresh iframe has no RTC or Worker')
        require(raw.get('local_file') == 'blocked', 'file fetch rejected asynchronously')
        require(decoded('network_results') == {'socket': 'blocked', 'fetch': 'blocked'}, 'fetch and WebSocket rejection probes completed')
        require(raw.get('fragment') == '#internal-anchor', 'same-document fragment navigation succeeds')
        after = decoded('after_navigation')
        require(after.get('value') == 'Retained edit' and after.get('url') == 'grove-html://document/#internal-anchor', 'external navigation/form do not replace document')
        require(raw.get('popup_blocked') is True, 'popup has no new browsing context')
        require(raw.get('expanded_sibling_hidden') is True and raw.get('expanded_surface_on_top') is True, 'expansion hides/detaches sibling surface and puts selected native view on top')
        require(isinstance(raw.get('browser_application'), str) and raw['browser_application'].endswith('.app'), 'public NSWorkspace API resolves default HTTPS browser (launch intentionally untested)')
        require(raw.get('hidden') is True and raw.get('detached') == 1, 'surface hides and detaches on dispose')
        require(not traffic, 'zero loopback TCP/UDP marker traffic across resource/navigation/RTC probes')
        evidence = {'command': command, 'port': port, 'sink_self_test': sink_self_test, 'traffic': traffic, 'checks': checks}
        (output/'native-validation.json').write_text(json.dumps(evidence, indent=2)+'\n')
        for row in checks: print(('PASS' if row['passed'] else 'FAIL') + ': '+row['check'])
        assert all(row['passed'] for row in checks), output/'native-validation.json'
    finally:
        stopped.set()
        server.shutdown()
        server.server_close()
        udp.close()

if __name__ == '__main__': main()
