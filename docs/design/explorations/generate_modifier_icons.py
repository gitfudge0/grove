from pathlib import Path
import base64,re
root=Path(__file__).resolve().parents[3]
source=(root/'src/icons.rs').read_text()
cmd=re.search(r'"command" => \{\s*r#"(.*?)"#',source,re.S)[1]
font=base64.b64encode((root/'assets/fonts/DMSans-Regular.ttf').read_bytes()).decode()
def svg(body,size=16): return f'<svg aria-hidden="true" width="{size}" height="{size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{body}</svg>'
paths={'outline':{'ctrl':'<path d="m3.5 10 4.5-4.5 4.5 4.5"/>','shift':'<path d="m3 7 5-5 5 5h-3v6H6V7z"/>'},'solid':{'ctrl':'<path d="m4 9.5 4-4 4 4"/>','shift':'<path d="m3.5 7 4.5-4.5L12.5 7H10v5.5H6V7z"/>'}}
def key(k,v='outline',size=16):
 if k=='cmd': content=svg(cmd,size)
 elif v=='text': content={'ctrl':'Ctrl','shift':'Shift'}.get(k,k)
 else: content=svg(paths[v][k],size) if k in paths[v] else k
 return f'<span class="key" role="img" aria-label="{dict(cmd="Command",ctrl="Control",shift="Shift").get(k,k)}">{content}</span>'
def chord(keys,v): return '<span class="chord">'+('<span aria-hidden="true">+</span>' if v=='text' else '').join(key(k,v) for k in keys)+'</span>'
css='''*{box-sizing:border-box}body{margin:0;background:var(--grove-bg,#fafafa);color:var(--grove-fg,#222);font:14px/1.5 "DM Sans",sans-serif}main{padding:24px;max-width:1120px;margin:auto}h1{font-size:26px;line-height:1.2;margin:8px 0}h2{font-size:18px;margin:0}h3{font-size:12px;font-weight:400;color:var(--grove-muted,#666);margin:20px 0 8px}p{margin:8px 0;color:var(--grove-muted,#666)}.eyebrow{font-size:12px;color:var(--grove-muted,#666)}.grid{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:16px;margin-top:24px}.variant{border:1px solid var(--grove-border,#ddd);border-radius:var(--grove-radius,8px);padding:16px;background:var(--grove-surface,#fff)}.tag{font-size:11px;color:var(--grove-info,#2867a0)}.specimen{display:flex;align-items:center;justify-content:space-around;gap:8px;padding:20px 0 8px}.specimen>div{text-align:center}.specimen small{display:block;color:var(--grove-muted,#666);margin-top:8px}.key{display:inline-flex;align-items:center;justify-content:center;min-width:16px;height:20px;font-size:12px;white-space:nowrap}.key svg{display:block;flex:none}.chord{display:inline-flex;align-items:center;gap:4px;flex:none;color:var(--grove-muted,#666)}.row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:8px 0;border-bottom:1px solid var(--grove-border,#ddd)}.row>span:first-child{min-width:0}.composer{border:1px solid var(--grove-border,#ddd);border-radius:6px;padding:12px}.composer p{margin:0 0 18px;color:var(--grove-fg,#222)}.hint{display:flex;align-items:center;gap:4px;flex-wrap:wrap;color:var(--grove-muted,#666);font-size:12px}.detail{font-size:12px;margin-top:16px}.sizes{display:flex;gap:20px;align-items:center}.sizegroup{display:flex;gap:4px;align-items:center}.usage{margin-top:24px;padding-top:16px;border-top:1px solid var(--grove-border,#ddd)}.usage p{max-width:85ch}kbd{font:inherit}button{font:inherit;color:inherit;background:var(--grove-surface,#fff);border:1px solid var(--grove-border,#ddd);border-radius:5px;padding:6px 10px;cursor:pointer}button:focus-visible{outline:2px solid var(--grove-fg,#222);outline-offset:3px}.toolbar{display:flex;gap:8px;margin-top:16px;flex-wrap:wrap}@media(max-width:760px){.grid{grid-template-columns:1fr}main{padding:16px}}body[data-local-theme="dark"]{--grove-bg:#1c1c1e;--grove-fg:#eee;--grove-muted:#aaa;--grove-surface:#242426;--grove-border:#454548;--grove-info:#9cc9ff}body[data-local-theme="light"]{--grove-bg:#fafafa;--grove-fg:#222;--grove-muted:#666;--grove-surface:#fff;--grove-border:#ddd;--grove-info:#2867a0}'''
html='<!doctype html>\n<!-- grove:integrated-report -->\n<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Grove · Modifier icon exploration</title><style>@font-face{font-family:"DM Sans";src:url(data:font/ttf;base64,'+font+')}'+css+'</style><body><main><div class="eyebrow">GROVE / ICON EXPLORATION</div><h1>Grove icons & keyboard modifiers</h1><p>The native icon catalog, followed by modifier candidates using the same 16×16 grid, 1.5-unit stroke, and rounded caps and joins.</p><div class="toolbar"><button onclick="document.body.dataset.localTheme=\'light\'">Light preview</button><button onclick="document.body.dataset.localTheme=\'dark\'">Dark preview</button><button onclick="delete document.body.dataset.localTheme">Grove theme</button></div><div class="grid">'
variants=[('outline','A · Outline symbols','Preferred for compact macOS hints','A quiet caret for Control and the existing Shift outline. Fits the 16×16 catalog and keeps counters open at small sizes.'),('solid','B · Compact outlines','Alternative · same native stroke','A narrower Control caret and a shorter Shift outline. Both retain the catalog’s 1.5-unit rounded stroke; A reuses the existing Shift exactly.'),('text','C · Explicit labels','Preferred for Windows / Linux','Ctrl and Shift stay immediately readable. Wider than symbols, but avoids relying on users recognizing the Control caret.')]
for v,title,sub,note in variants:
 html+=f'<section class="variant"><h2>{title}</h2><div class="tag">{sub}</div><div class="specimen">'
 for k,label in [('cmd','Command · existing'),('ctrl','Control'),('shift','Shift')]: html+=f'<div>{key(k,v,28)}<small>{label}</small></div>'
 html+='</div><h3>SIDEBAR · WINDOWS / LINUX BINDING</h3><div class="row"><span>History</span>'+chord(['ctrl','shift','1'],v)+'</div><div class="row"><span>Projects</span>'+chord(['ctrl','shift','2'],v)+'</div><h3>COMMAND MENU · PLATFORM EXAMPLES</h3><div class="row"><span>Command palette · Mac</span>'+chord(['cmd','P'],v)+'</div><div class="row"><span>Command palette · PC</span>'+chord(['ctrl','shift','P'],v)+'</div><h3>COMPOSER · SHARED HINT</h3><div class="composer"><p>Ask Grove anything…</p><div class="hint">'+chord(['shift','Enter'],v)+'<span>new line</span></div></div><h3>12 / 14 / 16 PX · GLYPH CHECK</h3><div class="sizes">'
 for size in [12,14,16]: html+='<div class="sizegroup">'+key('ctrl',v,size)+key('shift',v,size)+f'<small>{size}</small></div>'
 html+=f'</div><p class="detail">{note}</p></section>'
html+='''</div><section class="usage"><h2>Suggested direction</h2><p>Use A for macOS: ⌃ and ⇧ alongside the current ⌘. Use C for Windows and Linux: Ctrl and Shift. Keep the full key names in accessible descriptions; a Control caret should never serve as a standalone action icon.</p><p>Render modifier icons in a fixed slot with a 4 px gap. Keep shortcut hints quiet, right aligned, and separate from the action label. These are static hint specimens, not working application controls.</p><p>Source context: Command is used by Settings → View shortcuts. Sidebar hints currently use ⌘1/2 on macOS and Ctrl+Shift+1/2 elsewhere. The current checkout also contains a Shift outline; A reuses its exact path. B and the Control caret are exploratory paths.</p><p>Exploratory HTML, not a measured native reference. Implementation adopts the recommended Control caret and existing Shift outline in application shortcut hints, preserving keybindings. Other proposed icon redesigns remain exploratory. Linux sidebar, shortcut reference, and input context menus were inspected in an isolated app; macOS/Windows rendering and live Codex/Claude behavior remain untested. Theme preview colors are local samples; “Grove theme” inherits the report host.</p></section></main></body></html>'''


import html as escape_html
catalog={}
match_body=source.split('fn svg_for(name: &str)')[1].split('_ => ""')[0]
for m in re.finditer(r'((?:"[a-z0-9-]+"\s*\|\s*)*"[a-z0-9-]+")\s*=>\s*(?:\{\s*(?://[^\n]*\n\s*)?)?r#"(.*?)"#',match_body,re.S):
 for name in re.findall(r'"([a-z0-9-]+)"',m[1]): catalog[name]=m[2]
expected=set(re.findall(r'"([a-z0-9-]+)"(?=\s*(?:\||=>))',match_body))
assert set(catalog)==expected,expected-set(catalog)
files={str(p.relative_to(root)):p.read_text() for p in (root/'src').rglob('*.rs') if p.name!='icons.rs'}
def refs(name):
 hits=[]
 for path,text in files.items():
  for n,line in enumerate(text.splitlines(),1):
   if '"'+name+'"' in line or 'icons/'+name+'.svg' in line:
    hits.append(f'{path}:{n}')
 return hits
cards=[]
for name,body in sorted(catalog.items()):
 hits=refs(name)
 note=' · '.join(hits[:3]) if hits else 'Registered; no exact literal reference found (dynamic callers may apply)'
 if len(hits)>3: note+=f' · +{len(hits)-3} more references'
 cls=' brand-claude' if name=='claude' else ' brand-codex' if name=='codex' else ''
 cards.append(f'<article class="catalog-item{cls}" data-icon="{name}"><div class="catalog-glyph">{svg(body,20)}{svg(body,12)}</div><strong>{name}</strong><small>{escape_html.escape(note)}</small></article>')
frames=''.join(f'<span class="frame">{svg(f'<path d="M8 2a6 6 0 1 1-4.24 1.76" transform="rotate({i*30} 8 8)"/>',16)}<small>{i}</small></span>' for i in range(12))
compact=''
for name in ['claude','codex','opencode']:
 body=f'<defs><mask id="clearance-{name}"><rect width="16" height="16" fill="white" stroke="none"/><circle cx="14" cy="14" r="6" fill="black" stroke="none"/></mask></defs><g mask="url(#clearance-{name})">{catalog[name]}</g>'
 compact+=f'<article class="catalog-item brand-{name}">{svg(body,24)}<strong>compact-status-{name}</strong><small>Provider mark with clearance for the separate trailing status mark.</small></article>'
section=f'<section class="inventory"><h2>Native icon catalog · {len(catalog)} names</h2><p>Exact paths from src/icons.rs, shown at 20 and 12 px. Includes aliases and registered shapes even where no literal call site was found. Reference locations are static source matches, including possible fixtures; they do not establish runtime usage.</p><div class="catalog">'+''.join(cards)+'</div><h3>GENERATED SPINNER · ALL 12 FRAMES</h3><div class="frames">'+frames+'</div><h3>COMPACT PROVIDER VARIANTS</h3><div class="catalog">'+compact+'</div><p>Other compact-status-* names are generated by applying the same clearance mask to any catalog glyph. Spinner frames are static here for easy comparison; no decorative animation.</p><h3>OTHER VISIBLE MARKS</h3><p>Textual keyboard hints also use ⌘, ⌥, ⇧, ↑, ↓, ←, →, Enter and Esc. These are text with font fallback, not extra SVG assets. The Grove application/OS icon is a separate raster asset in assets/icon; vendor editor controls and content-provided images are outside this native sprite catalog.</p></section><section class="options-heading"><h2>Modifier options in the same family</h2><p>A is the closest match: the existing Shift path plus a Control caret using the native stroke. B tests tighter outline geometry. C preserves explicit labels for Windows and Linux. Command remains the exact existing filled path in every option.</p></section>'
html=html.replace('<div class="grid">',section+'<div class="grid">',1)
app_icon=base64.b64encode((root/'assets/icon/32x32.png').read_bytes()).decode()
html=html.replace('<h3>OTHER VISIBLE MARKS</h3>',f'<h3>APPLICATION ICON</h3><img width="32" height="32" alt="Grove application icon" src="data:image/png;base64,{app_icon}"><h3>OTHER VISIBLE MARKS</h3>')
html=html.replace('</style>', '.inventory{margin-top:28px}.catalog{display:grid;grid-template-columns:repeat(auto-fit,minmax(210px,1fr));gap:0;border-top:1px solid var(--grove-border,#ddd)}.catalog-item{padding:14px 10px;border-bottom:1px solid var(--grove-border,#ddd);min-width:0}.catalog-item strong{display:block;font-size:12px;font-weight:400;margin:6px 0}.catalog-item small{display:block;font-size:11px;overflow-wrap:anywhere;color:var(--grove-muted,#666)}.catalog-glyph{display:flex;align-items:center;gap:16px}.brand-claude svg{color:#d97757}.brand-codex svg{color:var(--grove-fg,#000)}.frames{display:flex;flex-wrap:wrap;gap:18px}.frame{text-align:center}.frame small{display:block}.options-heading{margin-top:32px;border-top:1px solid var(--grove-border,#ddd);padding-top:20px}</style>')

# Exploratory redesigns are intentionally separate from the exact native catalog.
proposals={
 'command':'<path d="M5.5 5.5H3.75A1.75 1.75 0 1 1 5.5 3.75v8.5A1.75 1.75 0 1 1 3.75 10.5h8.5A1.75 1.75 0 1 1 10.5 12.25v-8.5a1.75 1.75 0 1 1 1.75 1.75z"/>',
 'cog':'<path d="m6.5 2-.4 1.5-1.3.8-1.5-.4-1.5 2.6L3 7.6v1.5l-1.2 1.1 1.5 2.6 1.5-.4 1.3.8.4 1.3h3l.4-1.3 1.3-.8 1.5.4 1.5-2.6L13 9.1V7.6l1.2-1.1-1.5-2.6-1.5.4-1.3-.8L9.5 2z"/><circle cx="8" cy="8.25" r="2.25"/>',
 'terminal':'<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="m4.5 6.5 2 2-2 2M8.5 10.5H11"/>',
 'split':'<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M8 2.5v11"/>',
 'panel-right':'<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M10 2.5v11"/>',
 'shift':catalog['shift'],
}
reasons={
 'command':'Current: filled outline, about 1 unit thick on straight segments. Proposed: true 1.5-unit stroke.',
 'cog':'Current: compound filled outline, with 1-unit circular ring. Proposed: stroke-built gear; tooth shape still needs optical review.',
 'terminal':'Current: 13×10 frame, radius 1.5. Proposed: shared 12×11 frame, radius 1.5.',
 'split':'Current: 13×11 frame, radius 1.2. Proposed: shared 12×11 frame, radius 1.5.',
 'panel-right':'Current: 12×11 frame, radius 1. Proposed: same frame, radius 1.5.',
 'shift':'Unchanged reference: already uses the target 1.5-unit rounded stroke. Its pointed silhouette remains recognizable.',
}
audit='<section class="consistency"><h2>The consistency gap</h2><p>The previous comparison matched the wrapper settings, but that does not make the paths a consistent family. Filled paths bypass stroke width and line joins. Different occupied areas, corner radii, and detail density also change perceived weight.</p><p><strong>Proposed direction:</strong> stroke-built utility icons on the existing 16×16 grid, rounded 1.5-unit strokes, and shared geometry for related frames. Check optical weight at 12, 14, and 16 px; equal numeric settings alone are not enough.</p><div class="audit-grid">'
for name,body in proposals.items():
 audit+=f'<article class="audit-item"><h3>{name}</h3><div class="audit-pair"><div><small>Current · exact source</small>{svg(catalog[name],40)}<div class="audit-sizes">'+''.join(svg(catalog[name],n) for n in [12,14,16])+f'</div></div><div><small>Proposed · exploration</small>{svg(body,40)}<div class="audit-sizes">'+''.join(svg(body,n) for n in [12,14,16])+f'</div></div></div><p>{reasons[name]}</p></article>'
audit+='</div><h3>THE MODIFIER FAMILY, RECONSIDERED</h3><div class="family-strip">'+svg(proposals['command'],20)+svg(paths['outline']['ctrl'],20)+svg(catalog['shift'],20)+svg(catalog['enter'],20)+'</div><p>Proposed Command, Control, existing Shift, and existing Enter shown together. The options below retain the current Command for comparison; this strip explores the whole family rather than matching new symbols to its thinner outline.</p><h3>KEEP PURPOSEFUL DIFFERENCES</h3><p>Provider logos retain their authored geometry and brand colors. Play/Stop, selected stars, and status dots may retain purposeful fills. Tiny dots and compact task marks should be judged in context, rather than forced into hollow outlines.</p><p>These six examples establish a direction, not a completed icon redesign. The catalog below remains the exact current source. Native sizes, rasterization, and interaction states still require verification before adopting new paths.</p></section>'
html=html.replace('<section class="inventory">',audit+'<section class="inventory">',1)
html=html.replace('</style>', '.consistency{margin-top:24px}.audit-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(270px,1fr));gap:16px;margin-top:16px}.audit-item{padding:16px;border:1px solid var(--grove-border,#ddd);border-radius:var(--grove-radius,8px)}.audit-item h3{margin:0 0 12px;color:var(--grove-fg,#222);font-size:14px}.audit-item p{font-size:12px}.audit-pair{display:grid;grid-template-columns:1fr 1fr;gap:12px}.audit-pair small{display:block;font-size:11px;color:var(--grove-muted,#666);margin-bottom:12px}.audit-sizes{display:flex;gap:12px;align-items:center;margin:12px 0}.family-strip{display:flex;gap:16px;align-items:center;margin:12px 0}</style>')
html=html.replace('The native icon catalog, followed by modifier candidates using the same 16×16 grid, 1.5-unit stroke, and rounded caps and joins.', 'A consistency audit, a proposed family direction, and the complete current native icon catalog.')

(root/'docs/design/explorations/modifier-icons.html').write_text(html)
print(f'Generated {len(catalog)} catalog names, 12 spinner frames, 3 compact provider variants')
