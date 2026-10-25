#!/usr/bin/env python3
"""Compile the production Linux native backend and exercise it under Xvfb."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import json
import os
import shlex
import socket
import subprocess
import tempfile
import threading

root = Path(__file__).resolve().parent.parent
requests = []
class Sink(BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"unexpected network access")
    def log_message(self, *_):
        pass

with ThreadingHTTPServer(("127.0.0.1", 0), Sink) as server, tempfile.TemporaryDirectory(prefix="grove-linux-html-") as temporary:
    threading.Thread(target=server.serve_forever, daemon=True).start()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("127.0.0.1", 0))
    udp.setblocking(False)
    directory = Path(temporary)
    target = f"http://127.0.0.1:{server.server_port}/blocked"
    policy = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'"
    fixture = directory / "document.html"
    fixture.write_text(f'''<!doctype html><meta http-equiv="Content-Security-Policy" content="{policy}">
<html><body><button id="counter" style="position:absolute;left:70px;top:80px;width:100px;height:32px" onclick="counter++">Count</button><input id="entry" style="position:absolute;left:70px;top:120px;width:100px;height:32px">
<input id="upload" type="file"><form id="form" action="{target}"><input name="test" value="blocked"></form><script>
window.counter=0; window.blocked=0;
window.guardOK=typeof RTCPeerConnection==='undefined' && typeof Worker==='undefined';
try{{Object.defineProperty(globalThis,'RTCPeerConnection',{{value:function(){{}}}})}}catch{{}}
fetch('{target}').catch(()=>blocked++);
new Image().src='{target}';
try{{new RTCPeerConnection({{iceServers:[{{urls:'stun:127.0.0.1:{udp.getsockname()[1]}'}}]}})}}catch{{}}
try{{new WebSocket('ws://127.0.0.1:{server.server_port}/socket')}}catch{{}}
</script></body></html>''')
    flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "x11", "xext"], text=True))
    binary = directory / "native-probe"
    subprocess.run(["cc", "-Wall", "-Wextra", "-Werror=implicit-function-declaration", "-Wno-deprecated-declarations", str(root / "scripts/verify-inline-html-linux.c"), "-o", str(binary), *flags, "-lm", "-ldl"], check=True)
    environment = os.environ.copy()
    # Container root requires a test-only sandbox exemption; production never sets it.
    if os.geteuid() == 0:
        environment["WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS"] = "1"
    environment["LIBGL_ALWAYS_SOFTWARE"] = "1"
    for native_filter_only in (False, True):
        if native_filter_only:
            fixture.write_text(fixture.read_text().replace(f'<meta http-equiv="Content-Security-Policy" content="{policy}">', ''))
        result = subprocess.run(["xvfb-run", "-a", str(binary), str(fixture)], env=environment, text=True, capture_output=True, timeout=45)
        print(result.stdout, end="")
        print(result.stderr, end="")
        result.check_returncode()
        probe = json.loads(next(line.split("=", 1)[1] for line in result.stdout.splitlines() if line.startswith("document-probe=")))
        assert probe == {"width":600,"height":400,"counter":0,"rtc":"undefined","worker":"undefined","immutable":True,"guard":True,"blocked":1}, probe
        report = json.loads(next(line.split("=", 1)[1] for line in result.stdout.splitlines() if line.startswith("report-probe=")))
        assert report == {"initial":420,"shorter":180,"taller":1200,"retained_without_messages":True,"forwarded":True,"held":True,"native":True}, report
    try:
        packet, _ = udp.recvfrom(4096)
        raise AssertionError(f"Document sent UDP: {packet!r}")
    except BlockingIOError:
        pass
    finally:
        udp.close()
    assert requests == [], requests
    server.shutdown()
    print("PASS no document network requests reached the loopback server")
