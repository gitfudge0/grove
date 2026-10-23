#!/usr/bin/env python3
"""Exercise Grove's actual preference JavaScript in the macOS WebKit boundary.

No app/provider launched. Generates fixtures from the Rust bridge and bundled
font files, compiles the native harness, and retains its raw JSON evidence.
"""
import argparse
import base64
import json
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=True)
rust = (repo / 'src/views/inline_html.rs').read_text()
script = re.search(r'fn preference_script\(.*?r"(.*?)"\s*\)', rust, re.S)[1]
script = script.replace('{{', '{').replace('}}', '}')
on = script.replace('{reduced}', 'true')
off = script.replace('{reduced}', 'false')
css = ''.join("@font-face{font-family:'DM Sans';font-style:normal;font-weight:" + str(weight) + ";font-display:swap;src:url(data:font/ttf;base64," + base64.b64encode((repo / ('assets/fonts/DMSans-' + name + '.ttf')).read_bytes()).decode() + ") format('truetype')}" for name, weight in [('Regular',400), ('Medium',500), ('SemiBold',600), ('Bold',700)])
font_script = re.search(r'''fn document_font_script\(\).*?r#"(.*?)"#''', rust, re.S)[1]
css += ''.join("@font-face{font-family:'JetBrains Mono';font-style:normal;font-weight:" + str(weight) + ";src:url(data:font/ttf;base64," + base64.b64encode((repo / ('assets/fonts/JetBrainsMono-' + name + '.ttf')).read_bytes()).decode() + ") format('truetype')}" for name, weight in [('Regular',400), ('Bold',700)])
csp = re.search(r'const CSP: &str = "([^"]*)"', rust)[1]
source = '<!doctype html><meta http-equiv="Content-Security-Policy" content="' + csp + '"><style>' + css + '</style><script>' + on + '</script><script>' + font_script + '''</script><html><head><style>@keyframes spin{to{transform:rotate(1turn)}}#animated{animation:spin 1s infinite}</style></head><body><input id="retained" value="keep me"><div id="animated" style="font-family:serif!important">Animation</div><pre id="mono">Code <span id="mono-child" style="font-family:serif">child</span></pre><span id="sans" style="font-family:Arial">Sans</span><script>
window.initialMotion=matchMedia('(PREFERS-REDUCED-MOTION: REDUCE)').matches;
window.query=matchMedia('screen and (min-width: 1px) and (prefers-reduced-motion: reduce)');
window.events=[];query.addEventListener('change',e=>events.push(e.matches));
window.loadedFonts=false;Promise.all([document.fonts.load('13px \"DM Sans\"'),document.fonts.load('13px \"JetBrains Mono\"')]).then(faces=>loadedFonts=faces.every(group=>group.length>0));
document.addEventListener('DOMContentLoaded',()=>{const dynamic=document.createElement('div');dynamic.id='dynamic';dynamic.style.fontFamily='serif';dynamic.textContent='Added text';document.body.append(dynamic)});
</script></body></html>'''
(out / 'fixture.html').write_text(source)
(out / 'motion-on.js').write_text(on)
(out / 'motion-off.js').write_text(off)
binary = out / 'verify-preferences'
compile = subprocess.run(['clang','-fobjc-arc','-fblocks', str(repo/'scripts/verify-html-preferences-webkit.m'),'-framework','AppKit','-framework','WebKit','-framework','QuartzCore','-o',str(binary)],capture_output=True,text=True)
(out/'compile.log').write_text(compile.stdout+compile.stderr)
compile.check_returncode()
run = subprocess.run([str(binary),str(out/'fixture.html'),str(out/'motion-off.js'),str(out/'motion-on.js')],capture_output=True,text=True)
(out/'native.json').write_text(run.stdout)
(out/'native-stderr.log').write_text(run.stderr)
print(run.stdout.strip())
run.check_returncode()
assert json.loads(run.stdout)['pass'] is True
