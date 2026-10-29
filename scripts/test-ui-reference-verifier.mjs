#!/usr/bin/env node
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { auditSource, compareFontBindings, comparePaint, compareMeasurements, compareRasters, classifyEdgeRaster, verifyInteraction, decodePng, encodePng, sha256 } from './verify-ui-reference.mjs';

function fixture() {
  const context = { platform: 'macos', native_engine: 'gpui-test', theme: 'grove-dark', rem: 16, scale: 2, viewport: { width: 320, height: 200 }, font_hashes: { 'fonts/DMSans-Regular.ttf': 'font-sha256' }, motion: 'reduced', locale: 'en-US' };
  const measurement = { schema_version: 1, reference_hash: 'reference-sha256', contract_hash: 'contract-sha256', context,
    nodes: {
      root: { kind: 'row', bounds: { x: 0, y: 0, width: 320, height: 24 } },
      label: { kind: 'text', bounds: { x: 8, y: 4, width: 80, height: 16 }, text: { content: 'Run script', font_family: 'DM Sans', font_style: 'normal', font_size: 12, font_weight: 400, line_height: 16, bounds: { x: 8, y: 4, width: 80, height: 16 }, baseline: 16, glyphs: [...'Run script'].map((text, index) => ({ text, x: 8 + index * 5.1125, y: 4, width: 5.1125, height: 16 })), lines: [{ text: 'Run script', width: 51.125, y: 4, height: 16, baseline: 16 }] } },
      action: { kind: 'dense_action', bounds: { x: 272, y: 0, width: 48, height: 24 } }
    } };
  const native = structuredClone(measurement), browser = structuredClone(measurement);
  browser.observed = { browser_engine: 'actual browser test fixture', fonts_ready: true, fonts: [{ family: 'DM Sans', status: 'loaded' }], font_errors: [] };
  browser.duplicates = []; browser.errors = [];
  return { native, browser };
}
test('matching bounded measurements pass geometry without certifying rendering or interactions', () => {
  const { native, browser } = fixture();
  assert.equal(compareMeasurements(native, browser).status, 'passed');
});
test('serde shortest f32 decimals match their exact browser representation', () => {
  const { native, browser } = fixture();
  native.nodes.label.bounds.x = 32.09204;
  browser.nodes.label.bounds.x = Math.fround(32.09204);
  assert.equal(compareMeasurements(native, browser).status, 'passed');
});
for (const [value, shifted] of [[8, 8.000000953674316], [0, 2 ** -149], [32.09204, Math.fround(32.09204) + 2 ** -18]]) {
  test(`next representable f32 geometry drift fails at ${value}`, () => {
    const { native, browser } = fixture();
    native.nodes.label.bounds.x = value;
    browser.nodes.label.bounds.x = shifted;
    assert.equal(compareMeasurements(native, browser).status, 'failed');
  });
}
test('f64 glyph advance drift is never rounded to f32', () => {
  const { native, browser } = fixture();
  browser.nodes.label.text.glyphs[0].width += Number.EPSILON * 4;
  assert.equal(compareMeasurements(native, browser).status, 'failed');
});

for (const [name, mutate, field] of [
  ['wrong inset', b => { b.nodes.label.bounds.x += 4; }, 'label.bounds.x'],
  ['wrong inner label width', b => { b.nodes.label.text.lines[0].width -= 3; }, 'label.text.lines[0].width'],
  ['font substitution', b => { b.nodes.label.text.font_family = 'Arial'; }, 'label.text.font_family'],
  ['display font in chrome', b => { b.nodes.label.text.font_size = 20; }, 'label.text.font_size'],
  ['wrong font weight', b => { b.nodes.label.text.font_weight = 700; }, 'label.text.font_weight'],
  ['line-height mismatch', b => { b.nodes.label.text.line_height = 24; }, 'label.text.line_height'],
  ['unexpected wrapping', b => { b.nodes.label.text.lines = [{ text: 'Run', width: 20 }, { text: 'script', width: 30 }]; }, 'label.text.lines'],
  ['shrink moved reserved action', b => { b.nodes.action.bounds.x -= 8; }, 'action.bounds.x'],
  ['missing content', b => { b.nodes.label.text.content = 'Run'; }, 'label.text.content'],
  ['missing node', b => { delete b.nodes.action; }, 'node IDs'],
  ['extra node', b => { b.nodes.extra = structuredClone(b.nodes.root); }, 'node IDs'],
  ['stale reference', b => { b.reference_hash = 'older'; }, 'reference_hash'],
  ['stale contract', b => { b.contract_hash = 'older'; }, 'contract_hash'],
  ['root reset rem', b => { b.context.rem = 16 * 1.25; }, 'context.rem'],
  ['wrong device scale', b => { b.context.scale = 1; }, 'context.scale'],
  ['wrong viewport', b => { b.context.viewport.width = 400; }, 'context.viewport.width'],
  ['wrong theme', b => { b.context.theme = 'grove-light'; }, 'context.theme'],
  ['stale native engine', b => { b.context.native_engine = 'old-gpui'; }, 'context.native_engine'],
  ['font payload changed', b => { b.context.font_hashes['fonts/DMSans-Regular.ttf'] = 'changed'; }, 'context.font_hashes'],
  ['duplicate node', b => { b.duplicates.push('label'); }, 'duplicate node IDs'],
  ['browser failure', b => { b.errors.push('unloaded asset'); }, 'browser measurement errors'],
  ['font not loaded', b => { b.observed.font_errors.push({ family: 'DM Sans', status: 'unloaded' }); }, 'observed.font_errors'],
  ['missing native-relevant text metrics', b => { delete b.nodes.label.text.glyphs; }, 'label.text.browser.measurement_completeness'],
  ['unobserved engine', b => { delete b.observed; }, 'observed'],
  ['nonfinite bounds', b => { b.nodes.label.bounds.width = NaN; }, 'label.bounds.width']
]) test(`${name} fails and restored-valid measurements pass`, () => {
  const { native, browser } = fixture(); mutate(browser);
  const result = compareMeasurements(native, browser);
  assert.equal(result.status, 'failed');
  assert.ok(result.errors.some(error => error.where === field), JSON.stringify(result.errors));
  assert.equal(compareMeasurements(...Object.values(fixture())).status, 'passed');
});
test('missing measurements fail closed', () => {
  const { native, browser } = fixture(); browser.nodes = {};
  assert.equal(compareMeasurements(native, browser).status, 'failed');
  assert.equal(compareMeasurements({ schema_version: 1 }, {}).status, 'failed');
});
test('visible ellipsis cannot silently drop the full accessible source', () => {
  const { native, browser } = fixture();
  native.nodes.label.text.source_content = 'Full original action label';
  browser.nodes.label.text.source_content = 'Full original action label';
  assert.equal(compareMeasurements(native, browser).status, 'passed');
  browser.nodes.label.text.source_content = 'Full…';
  const result = compareMeasurements(native, browser);
  assert.equal(result.status, 'failed');
  assert.ok(result.errors.some(error => error.where === 'label.text.source_content'));
});
function image(width = 2, height = 2) {
  return { width, height, pixels: Buffer.from(Array.from({ length: width * height }, () => [32, 44, 56, 255]).flat()) };
}
test('lossless PNG decode retains every RGBA byte', () => {
  const expected = image();
  assert.deepEqual(decodePng(encodePng(expected)), expected);
});
test('equal decoded pixels pass despite PNG encoding details', () => {
  const a = encodePng(image()), b = encodePng(image());
  const { result, diff, overlay } = compareRasters(a, b);
  assert.equal(result.status, 'passed'); assert.equal(result.differing_pixels, 0);
  assert.equal(decodePng(diff).width, 2); assert.deepEqual(decodePng(overlay), image());
});
test('one changed channel fails raster; restored image passes', () => {
  const a = image(), b = image(); b.pixels[0]++;
  const changed = compareRasters(encodePng(a), encodePng(b));
  assert.equal(changed.result.status, 'failed'); assert.equal(changed.result.differing_pixels, 1);
  assert.deepEqual([...decodePng(changed.diff).pixels.subarray(0, 4)], [255, 0, 255, 255]);
  assert.equal(compareRasters(encodePng(a), encodePng(a)).result.status, 'passed');
});
test('different screenshot dimensions fail without scaling', () => {
  assert.equal(compareRasters(encodePng(image()), encodePng(image(1, 2))).result.status, 'failed');
});
test('invalid or corrupted PNG rejects without producing a raster pass', () => {
  assert.throws(() => decodePng(Buffer.from('not png')), /Not a PNG/);
  const bytes = encodePng(image()); bytes[29] ^= 1;
  assert.throws(() => decodePng(bytes), /checksum/);
});

function sourceFixture() {
  const rgba = [0.5, 0.5, 0.5, 1], asset = '<svg viewBox="0 0 24 24"><path d="M1 1L2 2"/></svg>';
  const snapshot = { contract_hash: 'contract', rem_base: 16, tokens: { icon_sm: 12 }, dense_action: { height: 24, text_size: 12, radius: 4, loading_icon: 'activity' }, ui_family: 'DM Sans', colors_rgba: { supporting: rgba, rail: rgba }, font_hashes: { 'fonts/DMSans-Regular.ttf': 'font' }, font_faces: [{ path: 'fonts/DMSans-Regular.ttf', family: 'DM Sans', weight: 400, style: 'normal', sha256: 'font' }], icons: { play: asset, activity: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8"/></svg>' } };
  const action = { id: 'run', kind: 'dense_action', state: 'rest', text: 'Run', icon: 'play', layout: { text_policy: 'single_line' }, children: [] };
  const document = { contract_hash: 'contract', fixture: { rem: 16 }, root: { id: 'root', kind: 'column', children: [action] } };
  const native = { contract_hash: 'contract', nodes: {
    root: { kind: 'column' },
    run: { kind: 'dense_action', bounds: { height: 24 }, paint: { kind: 'rounded_rect', radius: 4, fill_rgba: rgba } },
    'run-label': { kind: 'text', text: { content: 'Run', source_content: 'Run', font_family: 'DM Sans', font_style: 'normal', font_size: 12, font_weight: 400 }, paint: { kind: 'text', foreground_rgba: rgba, font_asset_sha256: 'font' } },
    'run-icon': { kind: 'icon', bounds: { width: 12, height: 12 }, paint: { kind: 'svg', foreground_rgba: rgba, asset_svg: asset, asset_sha256: sha256(asset) } }
  } };
  return { document, snapshot, native };
}
test('source audit derives expectations from the authored document and native design-system export', () => {
  const { document, snapshot, native } = sourceFixture();
  assert.equal(auditSource(document, snapshot, native).status, 'passed');
});
test('source audit accepts semantic key reordering without reconstructing Rust document hashes', () => {
  const { document, snapshot, native } = sourceFixture();
  const reverse = value => Array.isArray(value) ? value.map(reverse) : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).reverse().map(([k, v]) => [k, reverse(v)])) : value;
  assert.equal(auditSource(reverse(document), reverse(snapshot), reverse(native)).status, 'passed');
});
test('native color identity handles serde f32 printing without accepting changed Float32 values', () => {
  const { document, snapshot, native } = sourceFixture();
  const short = [0.06666667, 0.06666667, 0.06666667, 1], widened = [0.06666667014360428, 0.06666667014360428, 0.06666667014360428, 1];
  snapshot.colors_rgba.supporting = short; snapshot.colors_rgba.rail = short;
  native.nodes.run.paint.fill_rgba = widened; native.nodes['run-label'].paint.foreground_rgba = widened; native.nodes['run-icon'].paint.foreground_rgba = widened;
  assert.equal(auditSource(document, snapshot, native).status, 'passed');
  native.nodes.run.paint.fill_rgba = [0.06666668, ...widened.slice(1)];
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
});
test('typed text binding resolves to the authored fixture value', () => {
  const { document, snapshot, native } = sourceFixture();
  document.values = { action_label: 'Run' };
  document.root.children[0].text_binding = 'action_label'; document.root.children[0].text = null;
  assert.equal(auditSource(document, snapshot, native).status, 'passed');
  document.values.action_label = 'Different';
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
});
test('loading identity follows the owning recipe and rejects missing, unregistered or mismatched identities', () => {
  const { document, snapshot, native } = sourceFixture();
  document.root.children[0].state = 'loading';
  snapshot.colors_rgba.supporting_loading = snapshot.colors_rgba.supporting;
  snapshot.colors_rgba.rail_loading = snapshot.colors_rgba.rail;
  native.nodes['run-icon'].paint.asset_svg = snapshot.icons.activity;
  native.nodes['run-icon'].paint.asset_sha256 = sha256(snapshot.icons.activity);
  assert.equal(auditSource(document, snapshot, native).status, 'passed');
  snapshot.dense_action.loading_icon = 'play';
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
  snapshot.dense_action.loading_icon = 'missing';
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
  delete snapshot.dense_action.loading_icon;
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
});
for (const [name, mutate] of [
  ['dropped candidate node mirrored into HTML', n => { delete n.nodes['run-label']; }],
  ['invented label mirrored into HTML', n => { n.nodes['run-label'].text.content = 'Wrong'; n.nodes['run-label'].text.source_content = 'Wrong'; }],
  ['wrong catalog path with matching bounds', n => { n.nodes['run-icon'].paint.asset_svg = '<svg><path d="M9 9"/></svg>'; n.nodes['run-icon'].paint.asset_sha256 = sha256(n.nodes['run-icon'].paint.asset_svg); }],
  ['changed button radius with unchanged outer bounds', n => { n.nodes.run.paint.radius = 5; }],
  ['changed flat fill', n => { n.nodes.run.paint.fill_rgba = [0.4, 0.5, 0.5, 1]; }],
  ['changed native font size', n => { n.nodes['run-label'].text.font_size = 13; }],
  ['changed text foreground', n => { n.nodes['run-label'].paint.foreground_rgba = [0.4, 0.5, 0.5, 1]; }]
]) test(`source audit rejects ${name}, independently of paired agreement`, () => {
  const { document, snapshot, native } = sourceFixture(); mutate(native);
  assert.equal(auditSource(document, snapshot, native).status, 'failed');
});
function coverageFixture() {
  const width = 11, height = 11, pixels = Buffer.alloc(width * height * 4);
  for (let i = 3; i < pixels.length; i += 4) pixels[i] = 255;
  for (let y = 2; y <= 8; y++) for (let x = 2; x <= 8; x++) for (let c = 0; c < 3; c++) pixels[(y * width + x) * 4 + c] = 255;
  const nativeImage = { width, height, pixels }, browserImage = { width, height, pixels: Buffer.from(pixels) };
  for (let y = 2; y <= 8; y++) for (let x = 2; x <= 8; x++) if (x === 2 || y === 2 || x === 8 || y === 8)
    for (let c = 0; c < 3; c++) browserImage.pixels[(y * width + x) * 4 + c] = 220;
  const bounds = { x: 2, y: 2, width: 7, height: 7 };
  const native = { context: { scale: 1, renderer: 'unit-test-fixture-not-native', os_build: 'unit-test', native_engine: 'unit-test', font_hashes: {} }, nodes: { shape: { bounds, paint: { kind: 'rounded_rect', html_fill: [255, 255, 255], html_background: [0, 0, 0] } } } };
  const browser = { observed: { browser_version: 'unit-test-fixture-not-browser' }, nodes: { shape: { bounds, paint: { bounds } } } };
  const gates = { source_validation: { status: 'passed' }, geometry: { status: 'passed' }, paint: { status: 'passed' } };
  return { nativeImage, browserImage, native, browser, gates };
}
function paintFixture() {
  const bounds = { x: 0, y: 0, width: 12, height: 12 }, asset = '<svg><path d="M0 0L1 1"/></svg>';
  const common = { html_fill: [128, 128, 128], html_background: [0, 0, 0] };
  const native = { nodes: {
    action: { kind: 'dense_action', bounds, paint: { ...common, kind: 'rounded_rect', radius: 4 } },
    icon: { kind: 'icon', bounds, paint: { ...common, kind: 'svg', asset_svg: asset, asset_sha256: sha256(asset) } },
    label: { kind: 'text', bounds, paint: { ...common, kind: 'text' } }
  } };
  const canonical = { path: 'M0 0L1 1' }, styles = [{ strokeWidth: '2px' }];
  const browser = { nodes: {
    action: { paint: { ...common, kind: 'rounded_rect', radius: 4, radius_y: 4 } },
    icon: { paint: { ...common, kind: 'svg', bounds, asset_canonical: structuredClone(canonical), expected_asset_canonical: canonical, computed_styles: structuredClone(styles), expected_computed_styles: styles } },
    label: { paint: { ...common, kind: 'text' } }
  } };
  return { native, browser };
}
test('paint gate compares independently observed styles before edge policy', () => {
  const { native, browser } = paintFixture(); assert.equal(comparePaint(native, browser).status, 'passed');
});
test('font face binding rejects swapped payloads even when every native hash remains present', () => {
  const regular = 'a'.repeat(64), bold = 'b'.repeat(64);
  const snapshot = { font_hashes: { regular, bold }, font_faces: [
    { path: 'regular', family: 'DM Sans', weight: 400, style: 'normal', sha256: regular },
    { path: 'bold', family: 'DM Sans', weight: 700, style: 'normal', sha256: bold }
  ] };
  const browser = { observed: { font_faces: [
    { family: 'DM Sans', weight: 400, style: 'normal', sha256: regular, byte_length: 100 },
    { family: 'DM Sans', weight: 700, style: 'normal', sha256: bold, byte_length: 100 }
  ] } };
  assert.equal(compareFontBindings(snapshot, browser).status, 'passed');
  browser.observed.font_faces[0].sha256 = bold; browser.observed.font_faces[1].sha256 = regular;
  const result = compareFontBindings(snapshot, browser);
  assert.equal(result.status, 'failed'); assert.equal(result.errors.length, 2);
  assert.deepEqual(browser.observed.font_faces.map(f => f.sha256).sort(), [regular, bold]);
});
for (const [name, mutate] of [
  ['radius', b => { b.nodes.action.paint.radius = 5; }],
  ['vertical radius', b => { b.nodes.action.paint.radius_y = 5; }],
  ['flat fill', b => { b.nodes.action.paint.html_fill = [129, 128, 128]; }],
  ['catalog path', b => { b.nodes.icon.paint.asset_canonical.path = 'M9 9'; }],
  ['CSS stroke override', b => { b.nodes.icon.paint.computed_styles[0].strokeWidth = '3px'; }],
  ['icon transform with unchanged tagged node', b => { b.nodes.icon.paint.bounds = { x: 1, y: 0, width: 12, height: 12 }; }]
]) test(`paint gate rejects actual ${name} mutation`, () => {
  const { native, browser } = paintFixture(); mutate(browser);
  assert.equal(comparePaint(native, browser).status, 'failed');
});
test('coverage classifier accepts only scoped edge differences after all independent gates pass', () => {
  const { nativeImage, browserImage, native, browser, gates } = coverageFixture();
  const result = classifyEdgeRaster(encodePng(nativeImage), encodePng(browserImage), native, browser, gates);
  assert.equal(result.status, 'passed'); assert.equal(result.accepted_edge_pixels, 24);
  assert.equal(result.forbidden_pixels, 0); assert.equal(result.exact_rgba, false);
});
test('coverage solves the common per-channel UNORM interval instead of fixing one exact channel', () => {
  const f = coverageFixture();
  f.native.nodes.shape.paint.html_background = [23, 23, 22]; f.native.nodes.shape.paint.html_fill = [170, 170, 178];
  for (let y = 0; y < 11; y++) for (let x = 0; x < 11; x++) {
    const interior = x >= 3 && x <= 7 && y >= 3 && y <= 7, edge = x >= 2 && x <= 8 && y >= 2 && y <= 8 && !interior;
    for (const [image, coverageColor] of [[f.nativeImage, [122, 122, 127]], [f.browserImage, [93, 93, 95]]]) {
      const rgb = interior ? [170, 170, 178] : edge ? coverageColor : [23, 23, 22];
      for (let c = 0; c < 3; c++) image.pixels[(y * 11 + x) * 4 + c] = rgb[c];
    }
  }
  // c=.67 gives native RGB error <=.51; c=.47 gives browser error <=.91.
  // Both are within the SAME existing one-UNORM bound in every channel.
  const result = classifyEdgeRaster(encodePng(f.nativeImage), encodePng(f.browserImage), f.native, f.browser, f.gates);
  assert.equal(result.status, 'passed', JSON.stringify(result.errors));
});
for (const [name, mutate] of [
  ['flat interior corruption', f => { f.browserImage.pixels[(5 * 11 + 5) * 4] = 254; }],
  ['flat background corruption', f => { f.browserImage.pixels[0] = 1; }],
  ['foreign hue at an edge', f => { f.browserImage.pixels[(2 * 11 + 2) * 4] = 0; }],
  ['unverified source identity', f => { f.gates.source_validation.status = 'failed'; }],
  ['one pixel glyph shift rejected by geometry', f => { f.gates.geometry.status = 'failed'; }],
  ['wrong radius or path rejected by paint', f => { f.gates.paint.status = 'failed'; }],
  ['unknown renderer scope', f => { delete f.native.context.os_build; }]
]) test(`coverage criterion rejects ${name}`, () => {
  const f = coverageFixture(); mutate(f);
  assert.equal(classifyEdgeRaster(encodePng(f.nativeImage), encodePng(f.browserImage), f.native, f.browser, f.gates).status, 'failed');
});

function interactionFixture() {
  const snapshot = { colors_rgba: { rail: [0, 0, 0, 1], hover: [1, 1, 1, 1] } };
  const facts = {
    pointer_activation: { before: 0, after: 1 }, keyboard_enter: { before: 1, after: 2 }, keyboard_space: { before: 2, after: 3 },
    disabled_rejection: { before: 3, after: 3 }, loading_rejection: { before: 3, after: 3 },
    tab_traversal: { before: 'one', after: 'two', returned: 'one' }, focus_restoration: { before: 'one', after: 'one' },
    hover_release: { rest: [0, 0, 0, 255], hover: [255, 255, 255, 255], released: [0, 0, 0, 255] }, stable_slots: {}
  };
  for (const state of ['rest', 'disabled', 'loading', 'hover', 'focused']) facts.stable_slots[state] = { icon: { x: 8, y: 6, width: 12, height: 12 }, label: { x: 24, y: 6 } };
  const native = { reference_hash: 'ref', contract_hash: 'contract', context: { native_build_hash: 'build' }, interaction: { schema_version: 1, status: 'passed', reference_hash: 'ref', contract_hash: 'contract', native_build_hash: 'build',
    cases: Object.entries(facts).map(([name, observed]) => ({ name, status: 'passed', observed, evidence: { 'interaction-rest.png': 'a'.repeat(64) } })) } };
  return { native, snapshot };
}
test('interaction policy requires actual case facts and frame evidence, not a passed flag', () => {
  const { native, snapshot } = interactionFixture(); assert.equal(verifyInteraction(native, snapshot).status, 'passed');
  native.interaction.cases = []; assert.equal(verifyInteraction(native, snapshot).status, 'failed');
  native.interaction = { status: 'untested' }; assert.equal(verifyInteraction(native, snapshot).status, 'untested');
});
for (const [name, mutate] of [
  ['missing activation', n => { n.interaction.cases[0].observed.after = 0; }],
  ['disabled activation', n => { n.interaction.cases.find(c => c.name === 'disabled_rejection').observed.after++; }],
  ['sticky hover after release', n => { n.interaction.cases.find(c => c.name === 'hover_release').observed.released = [255, 255, 255, 255]; }],
  ['wrong hover palette', n => { n.interaction.cases.find(c => c.name === 'hover_release').observed.hover = [254, 255, 255, 255]; }],
  ['lost focus restoration', n => { n.interaction.cases.find(c => c.name === 'focus_restoration').observed.after = 'wrong'; }],
  ['stuck tab focus', n => { n.interaction.cases.find(c => c.name === 'tab_traversal').observed.after = 'one'; }],
  ['loading slot shift', n => { n.interaction.cases.find(c => c.name === 'stable_slots').observed.loading.icon.x++; }],
  ['missing evidence', n => { n.interaction.cases[0].evidence = {}; }],
  ['stale native build', n => { n.interaction.native_build_hash = 'old-build'; }]
]) test(`interaction policy rejects ${name}`, () => {
  const { native, snapshot } = interactionFixture(); mutate(native);
  assert.equal(verifyInteraction(native, snapshot).status, 'failed');
});
test('inert interaction scope is derived from authored controls and actions', () => {
  const { native, snapshot } = interactionFixture();
  native.interaction.scope = 'inert'; native.interaction.cases = [];
  const document = { actions: [], root: { id: 'text', kind: 'text', children: [] } };
  assert.equal(verifyInteraction(native, snapshot, document).status, 'passed');
  document.root.kind = 'dense_action';
  assert.equal(verifyInteraction(native, snapshot, document).status, 'failed');
});
test('all enabled authored actions require observed native routes and callback count changes', () => {
  const { native, snapshot } = interactionFixture();
  const document = { actions: ['first', 'second'], root: { kind: 'column', children: [
    { id: 'one', kind: 'dense_action', state: 'rest', action: 'first', icon: 'play' },
    { id: 'two', kind: 'dense_action', state: 'rest', action: 'second', icon: 'play' }
  ] } };
  native.interaction.action_routes = [ { node_id: 'one', expected: 'first', observed: 'first', before: 3, after: 4 }, { node_id: 'two', expected: 'second', observed: 'second', before: 4, after: 5 } ];
  assert.equal(verifyInteraction(native, snapshot, document).status, 'passed');
  native.interaction.action_routes[1].observed = 'first';
  assert.equal(verifyInteraction(native, snapshot, document).status, 'failed');
});
test('iconless stable-slot scope permits null slots only when every authored control lacks an icon', () => {
  const { native, snapshot } = interactionFixture();
  const document = { actions: ['run'], root: { id: 'button', kind: 'dense_action', state: 'rest', action: 'run', icon: null } };
  native.interaction.action_routes = [{ node_id: 'button', expected: 'run', observed: 'run', before: 3, after: 4 }];
  for (const state of Object.values(native.interaction.cases.find(c => c.name === 'stable_slots').observed)) state.icon = null;
  assert.equal(verifyInteraction(native, snapshot, document).status, 'passed');
  document.root.icon = 'play';
  assert.equal(verifyInteraction(native, snapshot, document).status, 'failed');
});

test('equal opaque background elision preserves semantic paint and radius audit', () => {
  const { native, browser } = paintFixture();
  Object.assign(native.nodes.action.paint, { html_background: [128,128,128], background_rgba: [128/255,128/255,128/255,1], fill_rgba: [128/255,128/255,128/255,.5] });
  Object.assign(browser.nodes.action.paint, { html_fill: undefined, fill_none: true, paint_elision: 'opaque-equal-background' });
  assert.equal(comparePaint(native,browser).status,'passed');
  native.nodes.action.paint.fill_rgba=[0,0,0,1];
  assert.equal(comparePaint(native,browser).status,'failed');
  native.nodes.action.paint.fill_rgba=[128/255,128/255,128/255,.5];
  browser.nodes.action.paint.radius=5;
  assert.equal(comparePaint(native,browser).status,'failed');
  browser.nodes.action.paint.radius=4;
  native.nodes.action.paint.html_background=[127,128,128];
  assert.equal(comparePaint(native,browser).status,'failed');
  native.nodes.action.paint.html_background=[128,128,128];
  native.nodes.action.paint.background_rgba[3]=.5;
  assert.equal(comparePaint(native,browser).status,'failed');
  native.nodes.action.paint.background_rgba[3]=1;
  browser.nodes.action.paint.fill_none=false;
  assert.equal(comparePaint(native,browser).status,'failed');
});
