#!/usr/bin/env node
// Actual browser harness checks against a hand-calculated rectangle fixture.
// This is NOT GPUI rendering evidence and cannot certify a component.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { compareFontBindings, encodePng, sha256, verifyDirectory } from './verify-ui-reference.mjs';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const output = process.argv[2] ? path.resolve(process.argv[2]) : await mkdtemp(path.join(tmpdir(), 'grove-reference-browser-'));
await mkdir(output, { recursive: true });
const font = await readFile(path.join(root, 'assets/fonts/DMSans-Regular.ttf'));
const context = { platform: process.platform, native_engine: 'hand-calculated-browser-harness-fixture-not-GPUI', theme: 'grove-dark', rem: 16, scale: 2, viewport: { width: 64, height: 32 }, font_hashes: { 'fonts/DMSans-Regular.ttf': sha256(font) }, motion: 'reduced', locale: 'en-US' };
const native = { schema_version: 1, reference_hash: 'browser-harness-reference', contract_hash: 'browser-harness-contract', context, nodes: { root: { kind: 'surface', bounds: { x: 0, y: 0, width: 20, height: 10 } } } };
const metadata = { schema_version: 1, reference_hash: native.reference_hash, contract_hash: native.contract_hash, context };
const valid = `<!doctype html><html style="font-size:16px"><style>@font-face{font-family:'DM Sans';src:url(data:font/ttf;base64,${font.toString('base64')})}*{box-sizing:border-box}html,body{margin:0;padding:0;background:#000}body{font-family:'DM Sans'}[data-node]{width:20px;height:10px;background:#000}</style><body><div data-node="root" data-kind="surface"></div><script>window.__GROVE_REFERENCE__=${JSON.stringify(metadata)}</script></body></html>`;
const pixels = Buffer.alloc(128 * 64 * 4);
for (let i = 3; i < pixels.length; i += 4) pixels[i] = 255;
const png = encodePng({ width: 128, height: 64, pixels });
const cases = [
  ['valid', valid, 'passed', 'passed'],
  ['shifted-inset', valid.replace('width:20px', 'transform:translateX(1px);width:20px'), 'failed', 'passed'],
  ['shrink', valid.replace('width:20px', 'width:19px'), 'failed', 'passed'],
  ['root-rem-reset', valid.replace('font-size:16px', 'font-size:20px'), 'failed', 'passed'],
  ['duplicate-id', valid.replace('</div>', '</div><div data-node="root" data-kind="surface"></div>'), 'failed', 'passed'],
  ['stale-contract', valid.replace('"contract_hash":"browser-harness-contract"', '"contract_hash":"stale"'), 'failed', 'passed'],
  ['raster-fill', valid.replaceAll('background:#000', 'background:#f00'), 'passed', 'failed'],
  ['restored-valid', valid, 'passed', 'passed']
];
for (const [name, html, geometry, raster] of cases) {
  const directory = path.join(output, name); await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, 'reference.html'), html);
  await writeFile(path.join(directory, 'native.json'), JSON.stringify(native));
  await writeFile(path.join(directory, 'native.png'), png);
  const certificate = await verifyDirectory(directory);
  assert.equal(certificate.geometry.status, geometry, `${name}: ${JSON.stringify(certificate.geometry)}`);
  assert.equal(certificate.raster.status, raster, `${name}: ${JSON.stringify(certificate.raster)}`);
  assert.equal(certificate.interaction.status, 'untested');
  assert.notEqual(certificate.status, 'certified');
  for (const [filename, hash] of Object.entries(certificate.artifacts))
    assert.equal(sha256(await readFile(path.join(directory, filename))), hash, `${name}: evidence hash ${filename}`);
  assert.equal(certificate.verifier.sha256, sha256(await readFile(path.join(root, 'scripts/verify-ui-reference.mjs'))));
  console.log(`${name}: geometry=${geometry}, raster=${raster}, native interaction=untested`);
}
// SVG path observes independently assigned cluster origins and logical lineboxes;
// the expected values below are hand-calculated, never read from page metadata.
const svgNative = { ...native, reference_context: context, nodes: { label: { kind: 'text', bounds: { x: 8.125, y: 5, width: 4, height: 12 }, text: {
  content: 'I', font_family: 'DM Sans', font_style: 'normal', font_size: 12, font_weight: 400, line_height: 12,
  bounds: { x: 8.125, y: 5, width: 4, height: 12 }, baseline: 15.5,
  lines: [{ text: 'I', width: 3.25, y: 5, height: 12, baseline: 15.5 }],
  glyphs: [{ text: 'I', x: 8.125, y: 5, width: 3.25, height: 12 }]
} } } };
const svgBody = '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32" viewBox="0 0 64 32"><rect data-node="label" data-kind="text" x="8.125" y="5" width="4" height="12" fill="none"/><g data-text="label" data-layout-mode="native-measured" style="font-family:\'DM Sans\';font-size:12px;font-weight:400;white-space:pre;fill:#000"><rect data-textbox="0" x="8.125" y="5" width="4" height="12" fill="none"/><rect data-linebox="0" x="8.125" y="5" width="3.25" height="12" fill="none"/><text data-glyph="0" x="8.125" y="15.5" xml:space="preserve">I</text></g></svg>';
const svgValid = valid.replace('[data-node]{width:20px;height:10px;background:#000}', '').replace('<div data-node="root" data-kind="surface"></div>', svgBody);
for (const [name, html, expected] of [
  ['svg-valid', svgValid, 'passed'],
  ['svg-cluster-drift', svgValid.replace('data-glyph="0" x="8.125"', 'data-glyph="0" x="8.625"'), 'failed'],
  ['svg-linebox-drift', svgValid.replace('width="3.25"', 'width="4.25"'), 'failed'],
  ['svg-font-weight', svgValid.replace('font-weight:400', 'font-weight:700'), 'failed'],
  ['svg-transform-only', svgValid.replace('data-glyph="0"', 'data-glyph="0" transform="translate(1 0)"'), 'failed'],
  ['svg-text-length-stretch', svgValid.replace('data-glyph="0"', 'data-glyph="0" textLength="2" lengthAdjust="spacingAndGlyphs"'), 'failed'],
  ['svg-ancestor-clip', svgValid.replace('style="font-family:', 'style="clip-path:inset(1px);font-family:'), 'failed'],
  ['svg-ancestor-mask', svgValid.replace('style="font-family:', 'style="mask-image:linear-gradient(black,transparent);font-family:'), 'failed'],
  ['svg-unowned-path', svgValid.replace('</svg>', '<path d="M0 0L1 1" stroke="black"/></svg>'), 'failed'],
  ['svg-restored-valid', svgValid, 'passed']
]) {
  const directory = path.join(output, name); await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, 'reference.html'), html);
  await writeFile(path.join(directory, 'native.json'), JSON.stringify(svgNative));
  await writeFile(path.join(directory, 'measured-layout.json'), JSON.stringify(svgNative));
  for (const filename of ['document.json', 'manifest.json', 'snapshot.json'])
    await writeFile(path.join(directory, filename), JSON.stringify({ browser_harness_only: true, native_rendering: false }));
  await writeFile(path.join(directory, 'generated.rs'), '// Browser harness; not compiled GPUI evidence.\n');
  const lockedFiles = ['document.json', 'snapshot.json', 'generated.rs', 'manifest.json', 'reference.html', 'measured-layout.json'];
  const lockArtifacts = {};
  for (const filename of lockedFiles) lockArtifacts[filename] = sha256(await readFile(path.join(directory, filename)));
  await writeFile(path.join(directory, 'reference-lock.json'), JSON.stringify({ schema_version: 1, reference_hash: svgNative.reference_hash, contract_hash: svgNative.contract_hash, render_mode: 'native-measured-svg', context, artifacts: lockArtifacts }));
  await writeFile(path.join(directory, 'native.png'), png);
  const certificate = await verifyDirectory(directory);
  assert.equal(certificate.geometry.status, expected, `${name}: ${JSON.stringify(certificate.geometry)}`);
  assert.notEqual(certificate.status, 'certified');
  console.log(`${name}: geometry=${expected}; native interaction=untested`);
}
const lockMutationDirectory = path.join(output, 'svg-restored-valid');
await writeFile(path.join(lockMutationDirectory, 'document.json'), (await readFile(path.join(lockMutationDirectory, 'document.json'), 'utf8')) + '\n');
const byteChanged = await verifyDirectory(lockMutationDirectory);
assert.equal(byteChanged.source_validation.status, 'failed');
assert.ok(byteChanged.source_validation.errors.some(error => error.where === 'reference-lock.document.json'));
console.log('Frozen document whitespace edits invalidate the byte hash lock.');

const catalog = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 4L20 20"/></svg>';
const styledNative = structuredClone(svgNative);
styledNative.nodes.label.paint = { kind: 'text', html_fill: [0, 0, 0], html_background: [0, 0, 0] };
styledNative.nodes.action = { kind: 'dense_action', bounds: { x: 40, y: 0, width: 20, height: 24 }, paint: { kind: 'rounded_rect', radius: 4, html_fill: [0, 0, 0], html_background: [0, 0, 0], fill_rgba: [0,0,0,1], background_rgba: [0,0,0,1] } };
styledNative.nodes.icon = { kind: 'icon', bounds: { x: 0, y: 15, width: 12, height: 12 }, paint: { kind: 'svg', asset_svg: catalog, asset_sha256: sha256(catalog), html_fill: [0, 0, 0], html_background: [0, 0, 0] } };
const icon = catalog.replace('<svg ', '<svg data-icon="icon" x="0" y="15" width="12" height="12" style="color:rgb(0,0,0)" ');
const styled = svgValid.replace('</svg>', `<rect data-node="action" data-kind="dense_action" x="40" y="0" width="20" height="24" rx="4" fill="rgb(0,0,0)"/><rect data-node="icon" data-kind="icon" x="0" y="15" width="12" height="12" fill="none"/>${icon}</svg>`);
for (const [name, html, paintResult, expectedError] of [
  ['paint-valid', styled, 'passed', null],
  ['paint-radius', styled.replace('rx="4"', 'rx="5"'), 'failed', 'action.paint.radius'],
  ['paint-path', styled.replace('M4 4L20 20', 'M4 4L18 20'), 'failed', 'icon.paint.asset_geometry'],
  ['paint-fill', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="rgb(1,0,0)"'), 'failed', 'action.paint.html_fill'],
  ['paint-elision-valid', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="none" data-paint-elision="opaque-equal-background"'), 'passed', null],
  ['paint-elision-wrong-painted-fill', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="rgb(1,0,0)" data-paint-elision="opaque-equal-background"'), 'failed', 'action.paint.elision'],
  ['paint-elision-forged-equal-projection', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="none" data-paint-elision="opaque-equal-background"'), 'failed', 'action.paint.elision'],
  ['paint-elision-unequal-background', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="none" data-paint-elision="opaque-equal-background"'), 'failed', 'action.paint.elision'],
  ['paint-elision-radius', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="5" fill="none" data-paint-elision="opaque-equal-background"'), 'failed', 'action.paint.radius'],
  ['paint-elision-unknown-flag', styled.replace('rx="4" fill="rgb(0,0,0)"', 'rx="4" fill="none" data-paint-elision="wrong"'), 'failed', 'action.paint.elision'],
  ['paint-restored-valid', styled, 'passed', null]
]) {
  const directory = path.join(output, name); await mkdir(directory, { recursive: true });
  const paintNative = structuredClone(styledNative);
  if (name === 'paint-elision-forged-equal-projection') paintNative.nodes.action.paint.fill_rgba = [1,0,0,1];
  if (name === 'paint-elision-unequal-background') paintNative.nodes.action.paint.html_fill = [1,0,0];
  await writeFile(path.join(directory, 'reference.html'), html);
  await writeFile(path.join(directory, 'native.json'), JSON.stringify(paintNative));
  await writeFile(path.join(directory, 'measured-layout.json'), JSON.stringify(styledNative));
  for (const filename of ['document.json', 'manifest.json', 'snapshot.json']) await writeFile(path.join(directory, filename), JSON.stringify({ browser_harness_only: true, native_rendering: false }));
  await writeFile(path.join(directory, 'generated.rs'), '// Browser harness; not compiled GPUI evidence.\n');
  const lockedFiles = ['document.json', 'snapshot.json', 'generated.rs', 'manifest.json', 'reference.html', 'measured-layout.json'], artifacts = {};
  for (const filename of lockedFiles) artifacts[filename] = sha256(await readFile(path.join(directory, filename)));
  await writeFile(path.join(directory, 'reference-lock.json'), JSON.stringify({ schema_version: 1, reference_hash: styledNative.reference_hash, contract_hash: styledNative.contract_hash, render_mode: 'native-measured-svg', context, artifacts }));
  await writeFile(path.join(directory, 'native.png'), png);
  const certificate = await verifyDirectory(directory);
  assert.equal(certificate.geometry.status, 'passed', `${name}: ${JSON.stringify(certificate.geometry)}`);
  assert.equal(certificate.paint.status, paintResult, `${name}: ${JSON.stringify(certificate.paint)}`);
  if (expectedError) assert.ok(certificate.paint.errors.some(error => error.where === expectedError), JSON.stringify(certificate.paint));
  assert.notEqual(certificate.status, 'certified');
  console.log(`${name}: observed paint=${paintResult}; no GPUI certification`);
}
const bold = await readFile(path.join(root, 'assets/fonts/DMSans-Bold.ttf'));
const faceSnapshot = { font_hashes: { regular: sha256(font), bold: sha256(bold) }, font_faces: [
  { path: 'regular', family: 'DM Sans', weight: 400, style: 'normal', sha256: sha256(font) },
  { path: 'bold', family: 'DM Sans', weight: 700, style: 'normal', sha256: sha256(bold) }
] };
const fontNative = structuredClone(svgNative);
fontNative.context.font_hashes['fonts/DMSans-Bold.ttf'] = sha256(bold); fontNative.reference_context = fontNative.context;
const faceMetadata = { ...metadata, context: fontNative.context };
const faceValid = svgValid.replace(JSON.stringify(metadata), JSON.stringify(faceMetadata)).replace('</style>', `@font-face{font-family:'DM Sans';font-weight:700;src:url(data:font/ttf;base64,${bold.toString('base64')})}</style>`);
const faceSwapped = faceValid.replace(font.toString('base64'), '__REGULAR_PAYLOAD__').replace(bold.toString('base64'), font.toString('base64')).replace('__REGULAR_PAYLOAD__', bold.toString('base64'));
for (const [name, html, expected] of [['face-valid', faceValid, 'passed'], ['face-swapped-payloads', faceSwapped, 'failed'], ['face-restored-valid', faceValid, 'passed']]) {
  const directory = path.join(output, name); await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, 'reference.html'), html);
  await writeFile(path.join(directory, 'native.json'), JSON.stringify(fontNative));
  await writeFile(path.join(directory, 'measured-layout.json'), JSON.stringify(fontNative));
  for (const filename of ['document.json', 'manifest.json', 'snapshot.json']) await writeFile(path.join(directory, filename), JSON.stringify({ browser_harness_only: true, native_rendering: false }));
  await writeFile(path.join(directory, 'generated.rs'), '// Browser harness; not compiled GPUI evidence.\n');
  const files = ['document.json', 'snapshot.json', 'generated.rs', 'manifest.json', 'reference.html', 'measured-layout.json'], artifacts = {};
  for (const filename of files) artifacts[filename] = sha256(await readFile(path.join(directory, filename)));
  await writeFile(path.join(directory, 'reference-lock.json'), JSON.stringify({ schema_version: 1, reference_hash: fontNative.reference_hash, contract_hash: fontNative.contract_hash, render_mode: 'native-measured-svg', context: fontNative.context, artifacts }));
  await writeFile(path.join(directory, 'native.png'), png);
  const certificate = await verifyDirectory(directory), observed = JSON.parse(await readFile(path.join(directory, 'browser.json')));
  assert.equal(certificate.geometry.status, 'passed', `${name}: ${JSON.stringify(certificate.geometry)}`);
  assert.equal(observed.nodes.label.text.font_weight, 400);
  assert.deepEqual(observed.observed.font_faces.map(face => face.sha256).sort(), Object.values(faceSnapshot.font_hashes).sort());
  assert.equal(compareFontBindings(faceSnapshot, observed).status, expected);
  assert.notEqual(certificate.status, 'certified');
  console.log(`${name}: actual CSS face payload identity=${expected}, computed weight remains400`);
}
const interrupted = path.join(output, 'restored-valid');
await rm(path.join(interrupted, 'native.json'));
await assert.rejects(() => verifyDirectory(interrupted), /ENOENT/);
assert.equal(JSON.parse(await readFile(path.join(interrupted, 'certificate.json'))).status, 'failed');
console.log('Missing input invalidates previous acceptance before failing.');
console.log(`Browser harness checks passed. Evidence: ${output}. No GPUI parity established.`);
