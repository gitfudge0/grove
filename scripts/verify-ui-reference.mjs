#!/usr/bin/env node
/** Paired reference verification. Geometry accepts only floating-point serialization noise.
 * Raster comparison is exact RGBA; different text antialiasing is a failure pending explicit
 * characterization, never a blanket tolerance. This tool does not certify native interaction.
 */
import { readFile, writeFile, mkdir, realpath } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { inflateSync, deflateSync } from 'node:zlib';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

// Typed native geometry is f32; serde shortest decimals and browser doubles
// must denote exactly the same f32. Endpoint-difference advances remain f64.
const sameF32 = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.fround(a) === Math.fround(b);
const sameF64 = (a, b) => Number.isFinite(a) && Number.isFinite(b) && a === b;
// Mirror the observed native backend's f32 blend and single f64 UNORM encode.
const nativeComposite = (fg, bg) => {
  if (![fg, bg].every(value => Array.isArray(value) && value.length === 4 && value.every(Number.isFinite))) return null;
  const f = Math.fround, alpha = f(fg[3]);
  return [0,1,2].map(i => {
    const scaled = f(f(f(fg[i]) * alpha) + f(f(bg[i]) * f(1 - alpha))) * 255;
    const floor = Math.floor(scaled), fraction = scaled - floor;
    return Math.max(0, Math.min(255, fraction === .5 ? floor + (floor % 2) : Math.round(scaled)));
  });
};
const VERIFIER_VERSION = 1;
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const EXECUTED_VERIFIER_HASH = sha256(await readFile(fileURLToPath(import.meta.url)));
const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object' ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
const same = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
// Native paint is Float32. serde's typed f32 formatter and JSON Value formatter
// may print the same Float32 differently; compare exact underlying f32 identity.
const sameNativeColor = (a, b) => Array.isArray(a) && Array.isArray(b) && a.length === 4 && b.length === 4 && a.every((value, index) => Number.isFinite(value) && Number.isFinite(b[index]) && Math.fround(value) === Math.fround(b[index]));
const isObject = value => value !== null && typeof value === 'object' && !Array.isArray(value);

export function compareMeasurements(native, browser) {
  const errors = [];
  const fail = (where, expected, actual) => errors.push({ where, expected, actual });
  const number = (where, expected, actual) => {
    if (!sameF32(expected, actual))
      fail(where, expected, actual);
  };
  if (native.schema_version !== 1 || browser.schema_version !== 1)
    fail('schema_version', 1, [native.schema_version, browser.schema_version]);
  for (const field of ['reference_hash', 'contract_hash']) {
    if (typeof native[field] !== 'string' || !native[field] || native[field] !== browser[field])
      fail(field, native[field], browser[field]);
  }
  if (!isObject(native.context) || !isObject(browser.context)) {
    fail('context', 'native and browser contexts', null);
    return { status: 'failed', criterion: 'exact native f32 identity; exact f64 glyph advances', errors };
  }
  for (const field of ['theme', 'motion', 'locale', 'platform', 'native_engine'])
    if (native.context[field] !== browser.context[field]) fail(`context.${field}`, native.context[field], browser.context[field]);
  const referenceContext = native.reference_context ?? native.context;
  if (referenceContext.native_build_hash !== undefined && referenceContext.native_build_hash !== browser.context.native_build_hash)
    fail('context.native_build_hash', referenceContext.native_build_hash, browser.context.native_build_hash);
  for (const field of ['theme', 'motion', 'locale', 'platform', 'native_engine', 'rem', 'scale'])
    if (referenceContext[field] !== native.context[field]) fail(`reference_context.${field}`, referenceContext[field], native.context[field]);
  for (const field of ['rem', 'scale']) number(`context.${field}`, native.context[field], browser.context[field]);
  for (const field of ['width', 'height'])
    number(`context.viewport.${field}`, native.context.viewport?.[field], browser.context.viewport?.[field]);
  const expectedFonts = native.context.font_hashes;
  if (!isObject(expectedFonts) || !Object.keys(expectedFonts).length || !same(expectedFonts, browser.context.font_hashes))
    fail('context.font_hashes', expectedFonts, browser.context.font_hashes);
  const observed = browser.observed;
  if (!isObject(observed) || !observed.browser_engine || observed.fonts_ready !== true || !Array.isArray(observed.fonts) || !observed.fonts.length)
    fail('observed', 'actual browser engine and loaded font faces', observed);
  if (observed?.font_errors?.length) fail('observed.font_errors', [], observed.font_errors);
  if (browser.duplicates?.length) fail('duplicate node IDs', [], browser.duplicates);
  if (browser.errors?.length) fail('browser measurement errors', [], browser.errors);
  if (!isObject(native.nodes) || !isObject(browser.nodes) || !Object.keys(native.nodes).length) {
    fail('nodes', 'nonempty keyed native/browser measurements', null);
    return { status: 'failed', criterion: 'exact native f32 identity; exact f64 glyph advances', errors };
  }
  const nativeIds = Object.keys(native.nodes).sort(), browserIds = Object.keys(browser.nodes).sort();
  if (!same(nativeIds, browserIds)) fail('node IDs', nativeIds, browserIds);
  for (const id of nativeIds) {
    const expected = native.nodes[id], actual = browser.nodes[id];
    if (!actual) continue;
    if (expected.kind !== actual.kind) fail(`${id}.kind`, expected.kind, actual.kind);
    for (const field of ['x', 'y', 'width', 'height']) number(`${id}.bounds.${field}`, expected.bounds?.[field], actual.bounds?.[field]);
    if (Boolean(expected.text) !== Boolean(actual.text)) fail(`${id}.text`, Boolean(expected.text), Boolean(actual.text));
    if (expected.text && actual.text) {
      for (const [label, measurement] of [['native', expected.text], ['browser', actual.text]]) {
        if (!measurement.bounds || !Number.isFinite(measurement.baseline) || !Array.isArray(measurement.glyphs) || !Array.isArray(measurement.lines))
          fail(`${id}.text.${label}.measurement_completeness`, 'inner bounds, actual baseline, glyph advances and line boxes', measurement);
        else {
          if (measurement.glyphs.map(glyph => glyph.text).join('') !== measurement.content)
            fail(`${id}.text.${label}.glyph_content`, measurement.content, measurement.glyphs.map(glyph => glyph.text).join(''));
          for (const [index, line] of measurement.lines.entries())
            for (const field of ['y', 'height', 'baseline'])
              if (!Number.isFinite(line[field])) fail(`${id}.text.${label}.lines[${index}].${field}`, 'measured finite value', line[field]);
        }
      }
      for (const field of ['content', 'font_family', 'font_style'])
        if (expected.text[field] !== actual.text[field]) fail(`${id}.text.${field}`, expected.text[field], actual.text[field]);
      if (expected.text.source_content !== undefined && expected.text.source_content !== actual.text.source_content)
        fail(`${id}.text.source_content`, expected.text.source_content, actual.text.source_content);
      for (const field of ['font_size', 'font_weight', 'line_height']) number(`${id}.text.${field}`, expected.text[field], actual.text[field]);
      if (!Array.isArray(expected.text.lines) || !Array.isArray(actual.text.lines) || expected.text.lines.length !== actual.text.lines.length)
        fail(`${id}.text.lines`, expected.text.lines, actual.text.lines);
      else expected.text.lines.forEach((line, index) => {
        const observedLine = actual.text.lines[index];
        if (line.text !== observedLine.text) fail(`${id}.text.lines[${index}].text`, line.text, observedLine.text);
        number(`${id}.text.lines[${index}].width`, line.width, observedLine.width);
        for (const field of ['y', 'height', 'baseline']) number(`${id}.text.lines[${index}].${field}`, line[field], observedLine[field]);
      });
      if (expected.text.baseline !== undefined) number(`${id}.text.baseline`, expected.text.baseline, actual.text.baseline);
      if (expected.text.bounds) for (const field of ['x', 'y', 'width', 'height'])
        number(`${id}.text.bounds.${field}`, expected.text.bounds[field], actual.text.bounds?.[field]);
      if (expected.text.glyphs) {
        if (!Array.isArray(actual.text.glyphs) || expected.text.glyphs.length !== actual.text.glyphs.length)
          fail(`${id}.text.glyphs`, expected.text.glyphs, actual.text.glyphs);
        else expected.text.glyphs.forEach((glyph, index) => {
          if (glyph.text !== actual.text.glyphs[index].text) fail(`${id}.text.glyphs[${index}].text`, glyph.text, actual.text.glyphs[index].text);
          for (const field of ['x', 'y', 'width', 'height'])
            if (field === 'width') {
              if (!sameF64(glyph[field], actual.text.glyphs[index][field])) fail(`${id}.text.glyphs[${index}].${field}`, glyph[field], actual.text.glyphs[index][field]);
            } else number(`${id}.text.glyphs[${index}].${field}`, glyph[field], actual.text.glyphs[index][field]);
        });
      }
    }
  }
  return { status: errors.length ? 'failed' : 'passed', criterion: 'exact native f32 identity; exact f64 glyph advances', errors };
}

/** The authored document, never the candidate capture, owns semantic expectations. */
export function auditSource(document, snapshot, native) {
  const errors = [], expected = new Map();
  const fail = (where, expected, actual) => errors.push({ where, expected, actual });
  const numeric = (where, expected, actual) => {
    if (!sameF32(expected, actual)) fail(where, expected, actual);
  };
  if (!document?.root || !snapshot?.tokens || !snapshot?.dense_action || !isObject(native?.nodes))
    return { status: 'failed', errors: [{ where: 'source', expected: 'typed authored document, design-system snapshot and native nodes', actual: 'missing source' }] };
  if (document.contract_hash !== snapshot.contract_hash || native.contract_hash !== snapshot.contract_hash)
    fail('source.contract_hash', snapshot.contract_hash, [document.contract_hash, native.contract_hash]);
  const ratio = document.fixture.rem / snapshot.rem_base;
  if (!Number.isFinite(ratio) || ratio <= 0) fail('source.rem_base', 'positive exported native rem base', snapshot.rem_base);
  function add(id, record) {
    if (!id || expected.has(id)) fail('source.node_id', 'unique nonempty identity', id);
    expected.set(id, record);
  }
  function walk(node) {
    const allowed = ['row', 'column', 'surface', 'text', 'icon', 'dense_action', 'checkout_row'];
    if (!allowed.includes(node.kind)) fail(`${node.id}.source.kind`, allowed, node.kind);
    add(node.id, { kind: node.kind, source: node });
    const originalText = node.text_binding ? document.values?.[node.text_binding] : node.text;
    if (node.kind === 'text') expected.get(node.id).text = { source: originalText, size: snapshot.tokens[node.text_role] * ratio, color: node.color ?? 'foreground', truncate: node.layout.text_policy === 'truncate' };
    if (node.kind === 'icon') expected.get(node.id).icon = node.icon;
    if (['dense_action', 'checkout_row'].includes(node.kind)) {
      const action = node.kind === 'dense_action';
      const color = action ? node.state === 'disabled' ? 'supporting_disabled' : node.state === 'loading' ? 'supporting_loading' : 'supporting' : node.color ?? 'foreground';
      add(`${node.id}-label`, { kind: 'text', source: node, text: { source: originalText, size: (action ? snapshot.dense_action.text_size : snapshot.checkout_row?.text_size) * ratio, color, truncate: !action } });
      const loadingIcon = snapshot.dense_action.loading_icon;
      if (action && node.state === 'loading' && (typeof loadingIcon !== 'string' || !loadingIcon || !snapshot.icons?.[loadingIcon]))
        fail(`${node.id}.source.loading_icon`, 'registered loading identity from the native dense action recipe', loadingIcon);
      if (node.icon) add(`${node.id}-icon`, { kind: 'icon', source: node, icon: action && node.state === 'loading' ? loadingIcon : node.icon, color: action ? color : 'supporting', icon_size: action ? snapshot.tokens.icon_sm : snapshot.checkout_row?.icon_size });
    }
    for (const child of node.children ?? []) walk(child);
  }
  walk(document.root);
  const expectedIds = [...expected.keys()].sort(), actualIds = Object.keys(native.nodes).sort();
  if (!same(expectedIds, actualIds)) fail('source.node_ids', expectedIds, actualIds);
  for (const [id, spec] of expected) {
    const actual = native.nodes[id]; if (!actual) continue;
    if (actual.kind !== spec.kind) fail(`${id}.source.kind`, spec.kind, actual.kind);
    if (spec.text) {
      if (typeof spec.text.source !== 'string') { fail(`${id}.source.binding`, 'resolved authored text value', spec.text.source); continue; }
      const text = actual.text;
      if (!text) { fail(`${id}.source.text`, spec.text.source, null); continue; }
      if ((text.source_content ?? text.content) !== spec.text.source) fail(`${id}.source.full_text`, spec.text.source, text.source_content ?? text.content);
      const displayed = text.content;
      const validDisplay = displayed === spec.text.source || spec.text.truncate && typeof displayed === 'string' && displayed.endsWith('…') && spec.text.source.startsWith(displayed.slice(0, -1));
      if (!validDisplay) fail(`${id}.source.displayed_text`, spec.text.source, displayed);
      if (text.font_family !== snapshot.ui_family) fail(`${id}.source.font_family`, snapshot.ui_family, text.font_family);
      if (text.font_style !== 'normal') fail(`${id}.source.font_style`, 'normal', text.font_style);
      numeric(`${id}.source.font_size`, spec.text.size, text.font_size);
      numeric(`${id}.source.font_weight`, 400, text.font_weight);
      if (actual.paint?.kind !== 'text') fail(`${id}.source.paint`, 'text', actual.paint?.kind);
      else {
        const role = snapshot.colors_rgba?.[spec.text.color];
        if (!sameNativeColor(role, actual.paint.foreground_rgba)) fail(`${id}.source.foreground`, role, actual.paint.foreground_rgba);
        const face = snapshot.font_faces?.find(face => face.family === snapshot.ui_family && face.weight === 400 && face.style === 'normal');
        const fontHash = face?.sha256;
        if (!fontHash || actual.paint.font_asset_sha256 !== fontHash) fail(`${id}.source.font_asset`, fontHash, actual.paint.font_asset_sha256);
      }
    }
    if (spec.icon) {
      const svg = snapshot.icons?.[spec.icon];
      if (!svg || actual.paint?.kind !== 'svg' || actual.paint.asset_svg !== svg || actual.paint.asset_sha256 !== sha256(svg))
        fail(`${id}.source.icon`, spec.icon, actual.paint?.asset_sha256);
      const role = snapshot.colors_rgba?.[spec.color ?? 'foreground'];
      if (!sameNativeColor(role, actual.paint?.foreground_rgba)) fail(`${id}.source.icon_foreground`, role, actual.paint?.foreground_rgba);
      numeric(`${id}.source.icon_width`, (spec.icon_size ?? snapshot.tokens.icon_sm) * ratio, actual.bounds?.width);
      numeric(`${id}.source.icon_height`, (spec.icon_size ?? snapshot.tokens.icon_sm) * ratio, actual.bounds?.height);
    }
    if (spec.kind === 'dense_action') {
      const state = spec.source.state;
      const role = state === 'disabled' ? 'rail_disabled' : state === 'loading' ? 'rail_loading' : ['hover', 'focus'].includes(state) ? 'hover' : 'rail';
      if (actual.paint?.kind !== 'rounded_rect') fail(`${id}.source.paint`, 'rounded_rect', actual.paint?.kind);
      else {
        numeric(`${id}.source.radius`, snapshot.dense_action.radius * ratio, actual.paint.radius);
        if (!sameNativeColor(actual.paint.fill_rgba, snapshot.colors_rgba?.[role])) fail(`${id}.source.fill`, snapshot.colors_rgba?.[role], actual.paint.fill_rgba);
      }
      numeric(`${id}.source.height`, snapshot.dense_action.height * ratio, actual.bounds.height);
    }
    if (spec.kind === 'surface' && (actual.paint?.kind !== 'surface' || actual.paint.radius !== 0 || !sameNativeColor(actual.paint.fill_rgba, snapshot.colors_rgba?.rail)))
      fail(`${id}.source.surface`, 'native rail surface with zero radius', actual.paint);
  }
  return { status: errors.length ? 'failed' : 'passed', errors };
}

export function comparePaint(native, browser) {
  const errors = [], fail = (where, expected, actual) => errors.push({ where, expected, actual });
  const numeric = (where, expected, actual) => {
    if (!sameF32(expected, actual)) fail(where, expected, actual);
  };
  let paints = 0;
  for (const [id, node] of Object.entries(native.nodes ?? {})) {
    if (!['dense_action', 'surface', 'text', 'icon'].includes(node.kind)) continue;
    const expected = node.paint, actual = browser.nodes?.[id]?.paint;
    if (!expected || !actual) { fail(`${id}.paint`, 'native and independently observed browser paint', null); continue; }
    paints++;
    if (expected.kind !== actual.kind) fail(`${id}.paint.kind`, expected.kind, actual.kind);
    const elision = actual.paint_elision;
    if (elision !== undefined && elision !== null) {
      const legal = expected.kind === 'rounded_rect' && elision === 'opaque-equal-background'
        && actual.fill_none === true && Array.isArray(expected.html_fill) && expected.html_fill.length === 3
        && expected.html_fill.every(value => Number.isInteger(value) && value >= 0 && value <= 255)
        && same(expected.html_fill, expected.html_background)
        && same(expected.html_fill, nativeComposite(expected.fill_rgba, expected.background_rgba))
        && same(expected.html_background, nativeComposite(expected.background_rgba, expected.background_rgba))
        && Array.isArray(expected.background_rgba) && expected.background_rgba[3] === 1
        && Array.isArray(expected.fill_rgba) && expected.fill_rgba.length === 4
        && expected.fill_rgba.every(Number.isFinite) && expected.fill_rgba[3] >= 0 && expected.fill_rgba[3] <= 1;
      if (!legal) fail(`${id}.paint.elision`, 'audited opaque equal-background rounded paint only', { expected, actual });
    } else if (!same(expected.html_fill, actual.html_fill)) fail(`${id}.paint.html_fill`, expected.html_fill, actual.html_fill);
    if (!Array.isArray(expected.html_background) || expected.html_background.length !== 3) fail(`${id}.paint.html_background`, 'native UNORM parent background', expected.html_background);
    if (['rounded_rect', 'surface'].includes(expected.kind)) {
      numeric(`${id}.paint.radius`, expected.radius, actual.radius);
      numeric(`${id}.paint.radius_y`, expected.radius, actual.radius_y);
    } else if (expected.kind === 'svg') {
      if (!expected.asset_svg || sha256(expected.asset_svg) !== expected.asset_sha256) fail(`${id}.paint.asset_sha256`, 'SHA256 of native catalog SVG', expected.asset_sha256);
      if (!same(actual.asset_canonical, actual.expected_asset_canonical)) fail(`${id}.paint.asset_geometry`, actual.expected_asset_canonical, actual.asset_canonical);
      if (!same(actual.computed_styles, actual.expected_computed_styles)) fail(`${id}.paint.asset_styles`, actual.expected_computed_styles, actual.computed_styles);
      for (const field of ['x', 'y', 'width', 'height']) numeric(`${id}.paint.svg_bounds.${field}`, node.bounds[field], actual.bounds?.[field]);
    }
  }
  if (!paints) fail('paint', 'at least one validated drawable primitive', null);
  return { status: errors.length ? 'failed' : 'passed', errors };
}

export function compareFontBindings(snapshot, browser) {
  const errors = [], expected = snapshot?.font_faces, actual = browser.observed?.font_faces;
  if (!Array.isArray(expected) || !expected.length || !Array.isArray(actual))
    return { status: 'failed', errors: [{ where: 'font_manifest', expected: 'native face catalog and observed CSSFontFaceRule payloads', actual: null }] };
  const identity = face => `${face.family}|${face.weight}|${face.style}`;
  const expectedIds = expected.map(identity), actualIds = actual.map(identity);
  if (new Set(expectedIds).size !== expectedIds.length || new Set(actualIds).size !== actualIds.length || !same([...expectedIds].sort(), [...actualIds].sort()))
    errors.push({ where: 'font_manifest.face_identities', expected: expectedIds, actual: actualIds });
  const bindings = [];
  for (const face of expected) {
    const observed = actual.find(candidate => identity(candidate) === identity(face));
    if (face.sha256 !== snapshot.font_hashes?.[face.path] || !observed || face.sha256 !== observed.sha256 || !Number.isInteger(observed.byte_length) || observed.byte_length <= 0)
      errors.push({ where: `font_manifest.${identity(face)}`, expected: { path: face.path, sha256: face.sha256 }, actual: observed });
    else bindings.push({ family: face.family, weight: face.weight, style: face.style, path: face.path, sha256: observed.sha256, byte_length: observed.byte_length });
  }
  return { status: errors.length ? 'failed' : 'passed', bindings, errors };
}

export function verifyInteraction(native, snapshot, document) {
  const record = native.interaction;
  if (!record || record.status === 'untested') return { status: 'untested', reason: 'No actual native interaction facts were captured.' };
  const errors = [], fail = (where, expected, actual) => errors.push({ where, expected, actual });
  const cases = record.cases ?? [], required = ['pointer_activation', 'keyboard_enter', 'keyboard_space', 'disabled_rejection', 'loading_rejection', 'tab_traversal', 'hover_release', 'focus_restoration', 'stable_slots'].sort();
  if (record.schema_version !== 1 || record.status !== 'passed') fail('interaction.schema', 'version 1 native facts with passed producer checks', record.status);
  for (const [field, expected] of [['reference_hash', native.reference_hash], ['contract_hash', native.contract_hash], ['native_build_hash', native.context?.native_build_hash]])
    if (!expected || record[field] !== expected) fail(`interaction.${field}`, expected, record[field]);
  const controls = [];
  const collect = node => { if (node.kind === 'dense_action') controls.push(node); for (const child of node.children ?? []) collect(child); };
  if (document?.root) collect(document.root);
  if (record.scope === 'inert') {
    if (!document?.root || controls.length || !Array.isArray(document.actions) || document.actions.length || !Array.isArray(cases) || cases.length || record.action_routes && (!Array.isArray(record.action_routes) || record.action_routes.length))
      fail('interaction.inert_scope', 'authored document has no controls/actions and no interaction cases', { controls: controls.map(c => c.id), actions: document?.actions, cases });
    return { status: errors.length ? 'failed' : 'passed', scope: 'inert', method: 'independent authored-source audit: no interactive components; actual paired native rendering still required', cases, errors };
  }
  if (!Array.isArray(cases) || !same(cases.map(c => c.name).sort(), required)) fail('interaction.cases', required, Array.isArray(cases) ? cases.map(c => c.name) : cases);
  const integer = value => Number.isInteger(value) && value >= 0;
  const handle = value => typeof value === 'string' && value.length > 0 || integer(value);
  const rgba = value => Array.isArray(value) && value.length === 4 && value.every(v => Number.isInteger(v) && v >= 0 && v <= 255);
  const roundEven = value => { const floor = Math.floor(value); return value - floor === 0.5 ? floor % 2 ? floor + 1 : floor : Math.round(value); };
  const unorm = value => Array.isArray(value) && value.length === 4 ? value.map(channel => roundEven(channel * 255)) : null;
  if (document?.root) {
    const enabled = controls.filter(control => !['disabled', 'loading'].includes(control.state));
    const routes = record.action_routes;
    if (!Array.isArray(routes) || !same(routes.map(route => route.node_id).sort(), enabled.map(control => control.id).sort()))
      fail('interaction.action_routes.nodes', enabled.map(control => control.id), routes);
    for (const route of Array.isArray(routes) ? routes : []) {
      const target = enabled.find(control => control.id === route.node_id);
      if (!target || typeof target.action !== 'string' || route.expected !== target.action || route.observed !== target.action || !integer(route.before) || route.after !== route.before + 1)
        fail(`interaction.action_routes.${route.node_id}`, { expected: target?.action, delta: 1 }, route);
    }
  }
  for (const entry of Array.isArray(cases) ? cases : []) {
    const facts = entry.observed ?? {};
    if (entry.status !== 'passed') fail(`interaction.${entry.name}.status`, 'passed', entry.status);
    if (!isObject(entry.evidence) || !Object.keys(entry.evidence).length || Object.entries(entry.evidence).some(([name, hash]) => !/^interaction-[a-z0-9-]+\.png$/.test(name) || !/^[0-9a-f]{64}$/.test(hash)))
      fail(`interaction.${entry.name}.evidence`, 'attributable native frame PNG hashes', entry.evidence);
    if (['pointer_activation', 'keyboard_enter', 'keyboard_space', 'disabled_rejection', 'loading_rejection'].includes(entry.name)) {
      const delta = entry.name.endsWith('rejection') ? 0 : 1;
      if (!integer(facts.before) || !integer(facts.after) || facts.after !== facts.before + delta) fail(`interaction.${entry.name}.activation_count`, `after = before + ${delta}`, facts);
    } else if (entry.name === 'tab_traversal') {
      const returnTarget = facts.before === null ? facts.firstFocused : facts.before;
      if (!handle(facts.after) || !handle(returnTarget) || facts.after === facts.before || facts.returned !== returnTarget) fail('interaction.tab_traversal.focus', 'distinct next focus and restoration to original/first focus', facts);
    } else if (entry.name === 'focus_restoration') {
      if (!handle(facts.before) || facts.before !== facts.after) fail('interaction.focus_restoration.focus', 'same nonempty native focus identity', facts);
    } else if (entry.name === 'hover_release') {
      if (!rgba(facts.rest) || !rgba(facts.hover) || !rgba(facts.released) || !same(facts.rest, facts.released) || same(facts.rest, facts.hover)) fail('interaction.hover_release.pixels', 'actual distinct hover and restored rest pixels', facts);
      if (!same(facts.rest, unorm(snapshot?.colors_rgba?.rail)) || !same(facts.hover, unorm(snapshot?.colors_rgba?.hover))) fail('interaction.hover_release.design_system', 'actual rest/hover palette roles', facts);
    } else if (entry.name === 'stable_slots') {
      const groups = facts.groups;
      const primary = controls.find(control => control.state === 'rest');
      const expectedSamples = controls.map(control => `${control.id}:${control.state}`);
      if (primary) expectedSamples.push(`${primary.id}:hover`, `${primary.id}:focused`);
      if (!document?.root || !Array.isArray(groups) || !groups.length) {
        fail('interaction.stable_slots.groups', 'authored icon-treatment groups with actual samples', groups);
        continue;
      }
      const samples = [], treatments = new Set();
      for (const group of groups) {
        if (typeof group.has_icon !== 'boolean' || treatments.has(group.has_icon) || !Array.isArray(group.samples) || !group.samples.length) {
          fail('interaction.stable_slots.group', 'unique icon treatment with nonempty samples', group);
          continue;
        }
        treatments.add(group.has_icon);
        let rest;
        for (const sample of group.samples) {
          samples.push(`${sample.node_id}:${sample.state}`);
          const control = controls.find(control => control.id === sample.node_id);
          const transient = ['hover', 'focused'].includes(sample.state);
          if (!control || Boolean(control.icon) !== group.has_icon || (transient ? control !== primary : sample.state !== control.state))
            fail('interaction.stable_slots.source', 'sample identity/state/icon treatment matches authored control', sample);
          const slot = sample.slots;
          const values = (group.has_icon ? ['x', 'y', 'width', 'height'].map(key => slot?.icon?.[key]) : []).concat(['x', 'y'].map(key => slot?.label?.[key]));
          if (values.some(value => !Number.isFinite(value)) || group.has_icon && (values[2] <= 0 || values[3] <= 0) || !group.has_icon && slot?.icon !== null)
            fail('interaction.stable_slots.bounds', 'finite actual slots with authored icon presence and positive icon size', sample);
          if (rest && values.some((value, index) => !sameF32(value, rest[index])))
            fail('interaction.stable_slots.drift', rest, values);
          rest ??= values;
        }
      }
      if (!same(samples.sort(), expectedSamples.sort())) fail('interaction.stable_slots.samples', expectedSamples, samples);
    }
  }
  return { status: errors.length ? 'failed' : 'passed', method: 'actual GPUI pointer/key/focus facts and frame evidence; no browser inference', cases, errors };
}

// Minimal lossless screenshot PNG reader/writer. Supports the noninterlaced 8-bit
// RGB/RGBA formats produced by GPUI and Chromium; rejects other formats explicitly.
const PNG_SIGNATURE = Buffer.from([137,80,78,71,13,10,26,10]);
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
export function decodePng(bytes) {
  if (!Buffer.isBuffer(bytes) || !bytes.subarray(0, 8).equals(PNG_SIGNATURE)) throw new Error('Not a PNG screenshot');
  let offset = 8, width, height, channels, seenEnd = false;
  const parts = [];
  while (offset + 12 <= bytes.length) {
    const length = bytes.readUInt32BE(offset), type = bytes.toString('ascii', offset + 4, offset + 8);
    if (offset + length + 12 > bytes.length) throw new Error('Truncated PNG chunk');
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    if (crc32(bytes.subarray(offset + 4, offset + 8 + length)) !== bytes.readUInt32BE(offset + 8 + length)) throw new Error('PNG checksum mismatch');
    if (type === 'IHDR') {
      if (width !== undefined || length !== 13) throw new Error('Invalid PNG header');
      width = data.readUInt32BE(0); height = data.readUInt32BE(4);
      channels = data[9] === 6 ? 4 : data[9] === 2 ? 3 : 0;
      if (!width || !height || width * height > 64_000_000 || data[8] !== 8 || !channels || data[10] || data[11] || data[12])
        throw new Error('Unsupported screenshot PNG format/dimensions');
    } else if (type === 'IDAT') parts.push(data);
    else if (type === 'IEND') { seenEnd = true; break; }
    offset += length + 12;
  }
  if (!width || !seenEnd || !parts.length) throw new Error('Incomplete PNG screenshot');
  const stride = width * channels;
  const raw = inflateSync(Buffer.concat(parts), { maxOutputLength: (stride + 1) * height });
  if (raw.length !== (stride + 1) * height) throw new Error('PNG scanline size mismatch');
  const decoded = Buffer.alloc(stride * height);
  const paeth = (a, b, c) => { const p = a + b - c, x = Math.abs(p - a), y = Math.abs(p - b), z = Math.abs(p - c); return x <= y && x <= z ? a : y <= z ? b : c; };
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    if (filter > 4) throw new Error('Unsupported PNG filter');
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? decoded[y * stride + x - channels] : 0;
      const above = y ? decoded[(y - 1) * stride + x] : 0;
      const corner = y && x >= channels ? decoded[(y - 1) * stride + x - channels] : 0;
      const prediction = [0, left, above, Math.floor((left + above) / 2), paeth(left, above, corner)][filter];
      decoded[y * stride + x] = (raw[y * (stride + 1) + x + 1] + prediction) & 255;
    }
  }
  const pixels = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    decoded.copy(pixels, i * 4, i * channels, i * channels + 3);
    pixels[i * 4 + 3] = channels === 4 ? decoded[i * channels + 3] : 255;
  }
  return { width, height, pixels };
}
function chunk(type, bytes) {
  const body = Buffer.concat([Buffer.from(type), bytes]), result = Buffer.alloc(body.length + 8);
  result.writeUInt32BE(bytes.length); body.copy(result, 4); result.writeUInt32BE(crc32(body), result.length - 4); return result;
}
export function encodePng({ width, height, pixels }) {
  if (!Number.isInteger(width) || !Number.isInteger(height) || width <= 0 || height <= 0 || pixels.length !== width * height * 4) throw new Error('Invalid RGBA dimensions');
  const header = Buffer.alloc(13); header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 6;
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) pixels.copy(rows, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([PNG_SIGNATURE, chunk('IHDR', header), chunk('IDAT', deflateSync(rows)), chunk('IEND', Buffer.alloc(0))]);
}
export function compareRasters(nativePng, browserPng) {
  const native = decodePng(nativePng), browser = decodePng(browserPng);
  if (native.width !== browser.width || native.height !== browser.height)
    return { result: { status: 'failed', criterion: 'exact RGBA equality', errors: ['Screenshot dimensions differ'], native_size: [native.width, native.height], browser_size: [browser.width, browser.height] } };
  let differing = 0;
  const diff = Buffer.alloc(native.pixels.length), overlay = Buffer.alloc(native.pixels.length);
  for (let i = 0; i < native.pixels.length; i += 4) {
    const equal = native.pixels.subarray(i, i + 4).equals(browser.pixels.subarray(i, i + 4));
    if (!equal) differing++;
    diff[i] = equal ? 0 : 255; diff[i + 1] = 0; diff[i + 2] = equal ? 0 : 255; diff[i + 3] = 255;
    for (let c = 0; c < 3; c++) overlay[i + c] = Math.round((native.pixels[i + c] + browser.pixels[i + c]) / 2);
    overlay[i + 3] = 255;
  }
  return { result: { status: differing ? 'failed' : 'passed', criterion: 'exact RGBA equality; no antialiasing tolerance authorized', differing_pixels: differing, total_pixels: native.width * native.height }, diff: encodePng({ ...native, pixels: diff }), overlay: encodePng({ ...native, pixels: overlay }) };
}

/** A bounded backend-coverage criterion, never a global similarity threshold.
 * Source, geometry and independently observed paint must already match. Every
 * differing sample must be a valid foreground/background coverage color within
 * one physical pixel of an edge in BOTH images, inside that validated primitive.
 * Flat interiors remain byte-exact. Engine/font/style identities scope the result.
 */
export function classifyEdgeRaster(nativePng, browserPng, native, browser, gates) {
  const criterion = 'paint-identity edge coverage v1: exact flat interiors; <=1 physical px from edges in both rasters; UNORM coverage quantization only';
  if (['geometry', 'source_validation', 'paint'].some(gate => gates[gate]?.status !== 'passed'))
    return { status: 'failed', criterion, errors: ['Source, geometry and paint must independently pass before edge characterization.'] };
  if (!native.context?.os_build || !native.context?.renderer || !browser.observed?.browser_version)
    return { status: 'failed', criterion, errors: ['Missing exact OS/native renderer/browser version scope.'] };
  const a = decodePng(nativePng), b = decodePng(browserPng);
  if (a.width !== b.width || a.height !== b.height) return { status: 'failed', criterion, errors: ['Screenshot dimensions differ.'] };
  const scale = native.context.scale;
  const pixelEqual = (image, i, j) => image.pixels.subarray(i * 4, i * 4 + 4).equals(image.pixels.subarray(j * 4, j * 4 + 4));
  const edgeMap = image => {
    const map = new Uint8Array(image.width * image.height);
    for (let y = 0; y < image.height; y++) for (let x = 0; x < image.width; x++) {
      const i = y * image.width + x;
      for (const [dx, dy] of [[-1, 0], [1, 0], [0, -1], [0, 1]]) {
        const nx = x + dx, ny = y + dy;
        if (nx >= 0 && nx < image.width && ny >= 0 && ny < image.height && !pixelEqual(image, i, ny * image.width + nx)) { map[i] = 1; break; }
      }
    }
    return map;
  };
  const ae = edgeMap(a), be = edgeMap(b);
  const nearEdge = (map, x, y) => {
    for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
      const nx = x + dx, ny = y + dy;
      if (nx >= 0 && nx < a.width && ny >= 0 && ny < a.height && map[ny * a.width + nx]) return true;
    }
    return false;
  };
  const flat = (image, x, y) => {
    const i = y * image.width + x;
    for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
      const nx = x + dx, ny = y + dy;
      if (nx >= 0 && nx < image.width && ny >= 0 && ny < image.height && !pixelEqual(image, i, ny * image.width + nx)) return false;
    }
    return true;
  };
  const validCoverage = (pixel, bg, fg) => {
    if (pixel[3] !== 255 || !bg || !fg) return false;
    // Integer framebuffer rounding is bounded by one UNORM sample, independently
    // of the observed difference between the two images. This is not an RGB delta.
    // Solve |P - (B + c(F-B))| <= 1 in EACH channel and intersect with c in [0,1].
    // Pinning c to one channel's exact sample would falsely reject valid rounding
    // in that channel, including one-UNORM endpoint quantization.
    let low = 0, high = 1;
    for (let channel = 0; channel < 3; channel++) {
      const delta = fg[channel] - bg[channel];
      if (!delta) { if (Math.abs(pixel[channel] - bg[channel]) > 1) return false; continue; }
      const a = (pixel[channel] - bg[channel] - 1) / delta, b = (pixel[channel] - bg[channel] + 1) / delta;
      low = Math.max(low, Math.min(a, b)); high = Math.min(high, Math.max(a, b));
      if (low > high) return false;
    }
    return low <= high;
  };
  const scopes = [];
  for (const [id, node] of Object.entries(native.nodes)) {
    if (!node.paint) continue;
    const observed = browser.nodes[id];
    const boxes = node.paint.kind === 'text' ? observed.text.observed_clusters?.map(cluster => cluster.ink_bounds) ?? [observed.text.bounds] : [observed.paint?.bounds ?? observed.bounds];
    scopes.push({ id, kind: node.paint.kind, foreground: node.paint.html_fill, background: node.paint.html_background, boxes });
  }
  scopes.sort((x, y) => (x.kind === 'text' ? 0 : x.kind === 'svg' ? 1 : 2) - (y.kind === 'text' ? 0 : y.kind === 'svg' ? 1 : 2));
  const errors = [], per_primitive = {}; let differing = 0, accepted = 0, rejected = 0;
  for (let y = 0; y < a.height; y++) for (let x = 0; x < a.width; x++) {
    const i = (y * a.width + x) * 4, ap = [...a.pixels.subarray(i, i + 4)], bp = [...b.pixels.subarray(i, i + 4)];
    if (same(ap, bp)) continue;
    differing++;
    let reason = null, owner;
    if (flat(a, x, y) && flat(b, x, y)) reason = 'flat interior differs';
    else if (!nearEdge(ae, x, y) || !nearEdge(be, x, y)) reason = 'outside paired edge band';
    else {
      const candidates = scopes.filter(scope => scope.boxes.some(box => box && x + 0.5 >= box.x * scale - 1 && x + 0.5 < (box.x + box.width) * scale + 1 && y + 0.5 >= box.y * scale - 1 && y + 0.5 < (box.y + box.height) * scale + 1));
      owner = candidates.find(scope => validCoverage(ap, scope.background, scope.foreground) && validCoverage(bp, scope.background, scope.foreground));
      if (!owner) reason = candidates.length ? 'not valid declared foreground/background coverage' : 'outside validated drawable primitives';
    }
    if (reason) { rejected++; if (errors.length < 100) errors.push({ x, y, reason, native: ap, browser: bp }); }
    else { accepted++; per_primitive[owner.id] = (per_primitive[owner.id] ?? 0) + 1; }
  }
  return { status: rejected ? 'failed' : 'passed', criterion, exact_rgba: differing === 0, differing_pixels: differing, accepted_edge_pixels: accepted, forbidden_pixels: rejected, per_primitive, errors,
    scope: { native_engine: native.context.native_engine, native_renderer: native.context.renderer, os_build: native.context.os_build, browser_engine: browser.observed.browser_engine, browser_version: browser.observed.browser_version, font_hashes: native.context.font_hashes } };
}

async function loadPlaywright() {
  const require = createRequire(import.meta.url), candidates = ['playwright', 'playwright-core'];
  if (process.env.GROVE_PLAYWRIGHT_MODULE) candidates.unshift(process.env.GROVE_PLAYWRIGHT_MODULE);
  const globalRoot = spawnSync('npm', ['root', '-g'], { encoding: 'utf8' });
  if (globalRoot.status === 0) {
    const root = globalRoot.stdout.trim();
    candidates.push(path.join(root, 'playwright'), path.join(root, 'agent-browser/node_modules/playwright'), path.join(root, 'agent-browser/node_modules/playwright-core'));
  }
  // npm's configured prefix may differ from the prefix of the installed CLI.
  for (const bin of (process.env.PATH ?? '').split(path.delimiter)) try {
    candidates.push(path.dirname(await realpath(path.join(bin, 'playwright'))));
  } catch {}
  for (const candidate of candidates) try { const module = require(candidate); if (module.chromium) return module; } catch {}
  throw new Error('Playwright is unavailable. Set GROVE_PLAYWRIGHT_MODULE to an installed playwright/playwright-core module; no browser or dependency is downloaded automatically.');
}

/** Executed in Chromium. No native measurement values influence DOM measurement. */
export function measureBrowserDocument({ expectedSvgSources = {}, measuredReference = false } = {}) {
  const metadata = window.__GROVE_REFERENCE__;
  const errors = [], duplicates = [], nodes = {};
  const rect = r => ({ x: r.x, y: r.y, width: r.width, height: r.height });
  const fontFamily = value => value.split(',')[0].trim().replace(/^['"]|['"]$/g, '');
  const color = value => {
    const match = /^rgba?\(([^)]+)\)$/.exec(value);
    if (!match) { errors.push(`Unsupported computed color: ${value}`); return null; }
    const channels = match[1].split(',').map(Number);
    if (channels.length === 3) channels.push(1);
    if (channels.length !== 4 || channels.some(v => !Number.isFinite(v)) || channels[3] !== 1) errors.push(`Nonopaque or invalid computed paint: ${value}`);
    return channels;
  };
  const ancestry = element => {
    for (let current = element; current instanceof Element; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (style.opacity !== '1' || style.filter !== 'none' || style.mixBlendMode !== 'normal' || style.visibility !== 'visible' ||
          ['clipPath', 'maskImage', 'backdropFilter'].some(property => style[property] && style[property] !== 'none'))
        errors.push(`${element.getAttribute('data-node') ?? element.getAttribute('data-text') ?? 'paint'}: unsupported ancestor opacity/filter/blend/visibility/clip/mask`);
    }
  };
  const canonicalSvg = (element, root = true) => ({ tag: element.localName,
    attributes: [...element.attributes].filter(a => !(root && ['x', 'y', 'width', 'height', 'style', 'data-icon', 'xmlns'].includes(a.name))).map(a => [a.name, a.value]).sort((a, b) => a[0].localeCompare(b[0])),
    children: [...element.childNodes].flatMap(child => child.nodeType === Node.ELEMENT_NODE ? [canonicalSvg(child, false)] : child.nodeType === Node.TEXT_NODE && child.textContent.trim() ? [child.textContent.trim()] : []) });
  const svgStyles = element => [element, ...element.querySelectorAll('*')].filter(e => !['style', 'title', 'desc'].includes(e.localName)).map(e => {
    const style = getComputedStyle(e); return Object.fromEntries(['fill', 'stroke', 'strokeWidth', 'strokeLinecap', 'strokeLinejoin', 'strokeMiterlimit', 'fillRule', 'clipRule', 'opacity', 'fillOpacity', 'strokeOpacity', 'filter', 'clipPath', 'maskImage', 'backdropFilter', 'vectorEffect', 'paintOrder', 'mixBlendMode', 'visibility', 'transform'].map(k => [k, style[k]]));
  });
  const unitMatrix = (element, id) => {
    const m = element.getScreenCTM();
    if (!m || [m.a - 1, m.b, m.c, m.d - 1, m.e, m.f].some(value => Math.abs(value) > 0.00001)) errors.push(`${id}: unsupported nonidentity glyph/layout transform`);
  };
  for (const element of document.querySelectorAll('[data-node]')) {
    const id = element.getAttribute('data-node');
    if (!id || Object.hasOwn(nodes, id)) { duplicates.push(id); continue; }
    const entry = { kind: element.getAttribute('data-kind'), bounds: rect(element.getBoundingClientRect()) };
    const targets = [...document.querySelectorAll('[data-text]')].filter(item => item.getAttribute('data-text') === id);
    if (targets.length > 1) errors.push(`${id}: duplicate data-text targets`);
    const target = targets[0];
    if (target) {
      if (target.getAttribute('data-layout-mode') === 'native-measured') {
        const lineboxes = [...target.querySelectorAll('rect[data-linebox]')];
        const textboxes = [...target.querySelectorAll('rect[data-textbox]')];
        const clusters = [...target.querySelectorAll('text[data-glyph]')];
        if (lineboxes.length !== 1 || textboxes.length !== 1 || !clusters.length) {
          errors.push(`${id}: measured SVG requires exactly one observed linebox/textbox and nonempty shaped clusters`);
          nodes[id] = entry; continue;
        }
        const line = rect(lineboxes[0].getBoundingClientRect());
        const observedClusters = clusters.map(cluster => {
          const allowedAttributes = new Set(['data-glyph', 'x', 'y', 'xml:space']);
          if ([...cluster.attributes].some(attribute => !allowedAttributes.has(attribute.name) && !attribute.name.startsWith('aria-')) || cluster.getAttribute('xml:space') !== 'preserve')
            errors.push(`${id}: unsupported SVG glyph attributes; stretching, rotation and per-glyph styles are forbidden`);
          for (const name of ['x', 'y']) if (!/^-?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.test(cluster.getAttribute(name) ?? '')) errors.push(`${id}: glyph ${name} requires one absolute numeric coordinate`);
          unitMatrix(cluster, id);
          ancestry(cluster);
          if (!cluster.getNumberOfChars()) throw new Error(`${id}: SVG cluster has no rendered characters`);
          const origin = cluster.getStartPositionOfChar(0), matrix = cluster.getScreenCTM();
          if (!matrix) throw new Error(`${id}: SVG cluster is not rendered`);
          const start = new DOMPoint(origin.x, origin.y).matrixTransform(matrix);
          const style = getComputedStyle(cluster);
          return { text: cluster.textContent, x: start.x, baseline: start.y,
            font_family: fontFamily(style.fontFamily), font_style: style.fontStyle, font_size: Number.parseFloat(style.fontSize), font_weight: Number.parseFloat(style.fontWeight),
            paint_style: Object.fromEntries(['fill', 'stroke', 'fillOpacity', 'opacity', 'filter', 'fontStyle', 'fontKerning', 'fontFeatureSettings', 'fontVariantLigatures', 'letterSpacing', 'wordSpacing', 'textTransform', 'textAnchor', 'direction', 'writingMode'].map(key => [key, style[key]])),
            natural_advance: cluster.getComputedTextLength(), ink_bounds: rect(cluster.getBoundingClientRect()) };
        });
        const first = observedClusters[0], content = observedClusters.map(cluster => cluster.text).join('');
        if (observedClusters.some(cluster => cluster.font_family !== first.font_family || cluster.font_style !== first.font_style || cluster.font_size !== first.font_size || cluster.font_weight !== first.font_weight || cluster.baseline !== first.baseline || JSON.stringify(cluster.paint_style) !== JSON.stringify(first.paint_style)))
          errors.push(`${id}: unsupported mixed SVG font/baseline runs`);
        if (observedClusters.some((cluster, index) => index && cluster.x < observedClusters[index - 1].x))
          errors.push(`${id}: unsupported nonmonotonic SVG cluster order`);
        entry.text = { content, font_family: first.font_family, font_style: first.font_style, font_size: first.font_size, font_weight: first.font_weight,
          source_content: target.getAttribute('aria-label') ?? element.getAttribute('aria-label') ?? content,
          line_height: line.height, bounds: rect(textboxes[0].getBoundingClientRect()), baseline: first.baseline,
          lines: [{ text: content, width: line.width, y: line.y, height: line.height, baseline: first.baseline }],
          glyphs: observedClusters.map((cluster, index) => ({ text: cluster.text, x: cluster.x, y: line.y,
            width: (observedClusters[index + 1]?.x ?? line.x + line.width) - cluster.x, height: line.height })),
          observed_clusters: observedClusters, layout_mode: 'native-measured' };
        nodes[id] = entry; continue;
      }
      const style = getComputedStyle(target), selections = [], lines = [];
      const walker = document.createTreeWalker(target, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        const node = walker.currentNode;
        let offset = 0;
        for (const char of node.textContent) {
          const range = document.createRange(); range.setStart(node, offset); offset += char.length; range.setEnd(node, offset);
          const r = range.getBoundingClientRect();
          selections.push({ text: char, ...rect(r) });
          let line = lines.find(item => item.y === r.y);
          if (!line) { line = { text: '', x: r.x, y: r.y, right: r.right }; lines.push(line); }
          line.text += char; line.x = Math.min(line.x, r.x); line.right = Math.max(line.right, r.right);
        }
      }
      // A zero-width inline baseline probe measures the actual line baseline. It is
      // removed before the screenshot and cannot provide any guessed text metrics.
      const probe = document.createElement('span');
      probe.style.cssText = 'display:inline-block;width:0;height:0;padding:0;margin:0;border:0;vertical-align:baseline';
      target.append(probe); const baseline = probe.getBoundingClientRect().y; probe.remove();
      const lh = Number.parseFloat(style.lineHeight);
      if (!Number.isFinite(lh)) errors.push(`${id}: explicit line-height required`);
      const textBounds = rect(target.getBoundingClientRect());
      // Both backends compare logical advance boxes, not platform-specific ink or
      // selection rectangles. Preserve the raw Range rectangles for diagnosis.
      const logicalLines = lines.map(({ text, x, right }, index) => ({ text, width: right - x, y: textBounds.y + index * lh, height: lh, baseline: baseline - (lines.length - index - 1) * lh }));
      const glyphs = selections.map((selection, index) => {
        const lineIndex = lines.findIndex(line => line.y === selection.y);
        const next = selections[index + 1], line = lines[lineIndex];
        return { text: selection.text, x: selection.x, y: textBounds.y + lineIndex * lh, width: (next?.y === selection.y ? next.x : line.right) - selection.x, height: lh };
      });
      entry.text = { content: target.textContent, font_family: fontFamily(style.fontFamily), font_style: style.fontStyle, font_size: Number.parseFloat(style.fontSize), font_weight: Number.parseFloat(style.fontWeight), line_height: lh, lines: logicalLines, baseline, glyphs, bounds: textBounds, selection_rects: selections };
    }
    nodes[id] = entry;
  }
  for (const [id, entry] of Object.entries(nodes)) {
    const element = [...document.querySelectorAll('[data-node]')].find(e => e.getAttribute('data-node') === id);
    const target = [...document.querySelectorAll('[data-text]')].find(e => e.getAttribute('data-text') === id);
    if (['dense_action', 'surface'].includes(entry.kind) && element instanceof SVGRectElement) {
      const style = getComputedStyle(element); ancestry(element); unitMatrix(element, id);
      const radius = Number.parseFloat(style.rx), radiusY = style.ry === 'auto' ? radius : Number.parseFloat(style.ry);
      if (style.fillOpacity !== '1' || style.stroke !== 'none') errors.push(`${id}: unsupported rect stroke/fill opacity`);
      entry.paint = { kind: entry.kind === 'surface' ? 'surface' : 'rounded_rect', html_fill: style.fill === 'none' ? null : color(style.fill)?.slice(0, 3), fill_none: style.fill === 'none', paint_elision: element.getAttribute('data-paint-elision'), radius, radius_y: radiusY };
    } else if (entry.text && target) {
      const glyph = target.querySelector('text[data-glyph]') ?? target, style = getComputedStyle(glyph); ancestry(glyph);
      const shaping = Object.fromEntries(['fontStyle', 'fontKerning', 'fontFeatureSettings', 'fontVariantLigatures', 'letterSpacing', 'wordSpacing', 'textTransform', 'textAnchor', 'direction', 'writingMode'].map(k => [k, style[k]]));
      const expected = { fontStyle: 'normal', fontKerning: 'auto', fontFeatureSettings: 'normal', fontVariantLigatures: 'normal', letterSpacing: 'normal', wordSpacing: '0px', textTransform: 'none', textAnchor: 'start', direction: 'ltr', writingMode: 'horizontal-tb' };
      if (Object.keys(expected).some(key => shaping[key] !== expected[key])) errors.push(`${id}: unsupported text shaping style ${JSON.stringify(shaping)}`);
      if (style.fillOpacity !== '1' || style.stroke !== 'none') errors.push(`${id}: unsupported text stroke/fill opacity`);
      entry.paint = { kind: 'text', html_fill: color(style.fill)?.slice(0, 3), shaping };
    } else if (entry.kind === 'icon') {
      const wrapper = [...document.querySelectorAll('[data-icon]')].find(e => e.getAttribute('data-icon') === id);
      const svg = wrapper?.localName === 'svg' ? wrapper : wrapper?.querySelector('svg');
      if (!svg || !expectedSvgSources[id]) { errors.push(`${id}: missing observed SVG/catalog source`); continue; }
      ancestry(svg);
      const parsed = new DOMParser().parseFromString(expectedSvgSources[id], 'image/svg+xml');
      if (parsed.querySelector('parsererror')) { errors.push(`${id}: invalid native SVG catalog asset`); continue; }
      const host = document.createElement('span'); host.style.cssText = 'position:fixed;left:-10000px;top:0;width:0;height:0;visibility:hidden;pointer-events:none';
      const shadow = host.attachShadow({ mode: 'closed' }), expectedSvg = document.importNode(parsed.documentElement, true);
      expectedSvg.style.color = getComputedStyle(svg).color; expectedSvg.style.visibility = 'visible';
      shadow.append(expectedSvg); document.body.append(host);
      const actualCanonical = canonicalSvg(svg), expectedCanonical = canonicalSvg(expectedSvg);
      // Visibility belongs to the hidden diagnostic host, not the trusted asset.
      const actualStyles = svgStyles(svg), expectedStyles = svgStyles(expectedSvg);
      host.remove();
      const view = svg.viewBox.baseVal, matrix = svg.getScreenCTM(), allocated = entry.bounds;
      const corners = matrix ? [[view.x, view.y], [view.x + view.width, view.y], [view.x, view.y + view.height], [view.x + view.width, view.y + view.height]].map(([x, y]) => new DOMPoint(x, y).matrixTransform(matrix)) : [];
      const actualViewport = corners.length ? { x: Math.min(...corners.map(p => p.x)), y: Math.min(...corners.map(p => p.y)), width: Math.max(...corners.map(p => p.x)) - Math.min(...corners.map(p => p.x)), height: Math.max(...corners.map(p => p.y)) - Math.min(...corners.map(p => p.y)) } : null;
      if (!view.width || !view.height || Math.abs(allocated.width / view.width - allocated.height / view.height) > 0.00001 || !matrix ||
          [matrix.a - allocated.width / view.width, matrix.b, matrix.c, matrix.d - allocated.height / view.height, matrix.e - (allocated.x - view.x * allocated.width / view.width), matrix.f - (allocated.y - view.y * allocated.height / view.height)].some(value => Math.abs(value) > 0.00001))
        errors.push(`${id}: unsupported catalog SVG viewport/ancestor transform`);
      entry.paint = { kind: 'svg', html_fill: color(getComputedStyle(svg).color)?.slice(0, 3), bounds: actualViewport, ink_bounds: rect(svg.getBoundingClientRect()),
        asset_canonical: actualCanonical, expected_asset_canonical: expectedCanonical, computed_styles: actualStyles, expected_computed_styles: expectedStyles };
    }
  }
  if (measuredReference || document.querySelector('[data-layout-mode="native-measured"]')) {
    for (const child of document.body.children) if (!['svg', 'style', 'script'].includes(child.localName)) errors.push('Unowned body rendering outside measured SVG');
    for (const primitive of document.querySelectorAll('svg rect,svg path,svg circle,svg ellipse,svg polygon,svg polyline,svg line,svg text,svg image,svg use,svg foreignObject')) {
      if (primitive.closest('[data-icon]')) continue;
      if (primitive.matches('text[data-glyph]') && primitive.closest('g[data-layout-mode="native-measured"]')) continue;
      if (primitive.matches('rect[data-node],rect[data-linebox],rect[data-textbox]')) {
        const style = getComputedStyle(primitive);
        if (['dense_action', 'surface'].includes(primitive.getAttribute('data-kind'))) continue;
        if (style.fill === 'none' && style.stroke === 'none') continue;
      }
      errors.push(`Unowned painted SVG primitive: ${primitive.localName}`);
    }
  }
  if (!metadata) errors.push('Missing __GROVE_REFERENCE__ provenance');
  const faces = [...document.fonts].map(f => ({ family: fontFamily(f.family), weight: f.weight, style: f.style, status: f.status }));
  const cssFaces = [];
  const inspectRules = rules => {
    for (const rule of rules) {
      if (rule instanceof CSSFontFaceRule) {
        const source = rule.style.getPropertyValue('src'), urls = [...source.matchAll(/url\(\s*['"]?(data:[^;,]+;base64,([A-Za-z0-9+/=]+))['"]?\s*\)/g)];
        if (urls.length !== 1 || /local\(/i.test(source)) errors.push('Font faces require one bundled data URL and no local fallback');
        const declaredWeight = rule.style.fontWeight || 'normal';
        cssFaces.push({ family: fontFamily(rule.style.fontFamily), weight: declaredWeight === 'normal' ? 400 : declaredWeight === 'bold' ? 700 : Number.parseFloat(declaredWeight), style: rule.style.fontStyle || 'normal', source_base64: urls[0]?.[2] ?? null });
      } else if (rule.cssRules) inspectRules(rule.cssRules);
    }
  };
  for (const sheet of document.styleSheets) try { inspectRules(sheet.cssRules); } catch (error) { errors.push(`Cannot inspect font stylesheet: ${error.message}`); }
  return { schema_version: 1, reference_hash: metadata?.reference_hash, contract_hash: metadata?.contract_hash, context: { ...metadata?.context, viewport: { width: window.innerWidth, height: window.innerHeight }, scale: window.devicePixelRatio, rem: Number.parseFloat(getComputedStyle(document.documentElement).fontSize), locale: navigator.language, motion: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'reduced' : 'normal' }, observed: { browser_engine: navigator.userAgent, fonts_ready: document.fonts.status === 'loaded', fonts: faces, font_faces: cssFaces, font_errors: faces.filter(f => f.status !== 'loaded') }, nodes, duplicates, errors };
}

async function captureBrowser(directory, native, html, measuredReference) {
  const playwright = await loadPlaywright();
  const launch = { headless: true };
  if (process.env.GROVE_CHROME_EXECUTABLE) launch.executablePath = process.env.GROVE_CHROME_EXECUTABLE;
  else if (process.platform === 'darwin') launch.channel = 'chrome';
  const browser = await playwright.chromium.launch(launch);
  try {
    const context = await browser.newContext({ viewport: native.context.viewport, deviceScaleFactor: native.context.scale, reducedMotion: 'reduce', locale: native.context.locale, colorScheme: 'no-preference' });
    const page = await context.newPage(), requestErrors = [];
    await page.route('**/*', route => {
      const url = route.request().url();
      if (url.startsWith('data:') || url === 'about:blank') return route.continue();
      requestErrors.push(`External resource rejected: ${url}`); return route.abort();
    });
    page.on('pageerror', error => requestErrors.push(error.message));
    await page.setContent(html, { waitUntil: 'load' });
    await page.evaluate(async () => {
      await document.fonts.ready;
      await Promise.all([...document.fonts].map(face => face.load()));
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    });
    const expectedSvgSources = Object.fromEntries(Object.entries(native.nodes).filter(([, n]) => n.paint?.kind === 'svg').map(([id, n]) => [id, n.paint.asset_svg]));
    const measured = await page.evaluate(measureBrowserDocument, { expectedSvgSources, measuredReference });
    measured.observed.browser_version = browser.version();
    for (const face of measured.observed.font_faces) {
      const bytes = face.source_base64 ? Buffer.from(face.source_base64, 'base64') : Buffer.alloc(0);
      face.sha256 = sha256(bytes); face.byte_length = bytes.length;
      delete face.source_base64;
    }
    measured.errors.push(...requestErrors);
    const hashes = [...html.matchAll(/url\(\s*['"]?data:(?:font\/[^;,]+|application\/(?:font[^;,]*|octet-stream|x-font[^;,]*));base64,([A-Za-z0-9+/=]+)['"]?\s*\)/g)].map(match => sha256(Buffer.from(match[1], 'base64')));
    for (const [name, hash] of Object.entries(native.context.font_hashes))
      if (!hashes.includes(hash)) measured.errors.push(`Bundled font payload missing or changed: ${name}`);
    await page.screenshot({ path: path.join(directory, 'browser.png'), animations: 'disabled' });
    await context.close();
    return measured;
  } finally { await browser.close(); }
}

export async function verifyDirectory(directory) {
  directory = path.resolve(directory);
  await mkdir(directory, { recursive: true });
  // Invalidate any previous acceptance before reading inputs or launching a browser.
  await writeFile(path.join(directory, 'certificate.json'), JSON.stringify({ schema_version: 1, status: 'failed', reason: 'Verification in progress or interrupted; no current acceptance.' }, null, 2) + '\n');
  const nativeBytes = await readFile(path.join(directory, 'native.json'));
  const htmlBytes = await readFile(path.join(directory, 'reference.html'));
  const native = JSON.parse(nativeBytes), html = htmlBytes.toString('utf8');
  let measured, geometry, raster = { status: 'untested', criterion: 'exact RGBA equality' }, rasterArtifacts;
  let source_validation = { status: 'untested', errors: [] }, paint = { status: 'untested', errors: [] };
  let font_bindings = { status: 'untested', errors: [] };
  let snapshot, authoredDocument;
  const artifacts = { 'native.json': sha256(nativeBytes), 'reference.html': sha256(htmlBytes) };
  try {
    // Frozen identity selects the measured workflow even when no text exists.
    // Partial or malformed authored evidence must fail rather than fall back to
    // the browser-only geometry harness.
    const readOptional = async name => {
      try { return JSON.parse(await readFile(path.join(directory, name), 'utf8')); }
      catch (error) { if (error.code === 'ENOENT') return null; throw error; }
    };
    const manifest = await readOptional('manifest.json');
    const referenceLock = await readOptional('reference-lock.json');
    const measuredReference = manifest !== null || referenceLock !== null;
    measured = await captureBrowser(directory, native, html, measuredReference);
    await writeFile(path.join(directory, 'browser.json'), JSON.stringify(measured, null, 2) + '\n');
    geometry = compareMeasurements(native, measured);
    if (measuredReference || Object.values(measured.nodes).some(node => node.text?.layout_mode === 'native-measured')) {
      const payloadBytes = await readFile(path.join(directory, 'measured-layout.json'));
      const payload = JSON.parse(payloadBytes);
      artifacts['measured-layout.json'] = sha256(payloadBytes);
      const document = JSON.parse(await readFile(path.join(directory, 'document.json')));
      authoredDocument = document;
      snapshot = JSON.parse(await readFile(path.join(directory, 'snapshot.json')));
      const candidateAudit = auditSource(document, snapshot, native), baselineAudit = auditSource(document, snapshot, payload);
      source_validation = { status: 'passed', errors: [...candidateAudit.errors, ...baselineAudit.errors.map(error => ({ ...error, where: `baseline.${error.where}` }))] };
      if (manifest?.schema_version !== 1 || manifest?.render_mode !== 'native-measured-svg' || manifest.reference_hash !== payload.reference_hash || manifest.contract_hash !== payload.contract_hash)
        source_validation.errors.push({ where: 'manifest.identity', expected: 'native-measured-svg authored identity', actual: manifest });
      font_bindings = compareFontBindings(snapshot, measured);
      source_validation.errors.push(...font_bindings.errors);
      for (const field of ['reference_hash', 'contract_hash'])
        if (payload[field] !== native[field]) source_validation.errors.push({ where: `measured-layout.${field}`, expected: native[field], actual: payload[field] });
      if (!same(payload.context, native.reference_context)) source_validation.errors.push({ where: 'reference_context', expected: payload.context, actual: native.reference_context });
      for (const filename of ['document.json', 'generated.rs', 'manifest.json', 'snapshot.json'])
        artifacts[filename] = sha256(await readFile(path.join(directory, filename)));
      const lockBytes = await readFile(path.join(directory, 'reference-lock.json')), lock = JSON.parse(lockBytes);
      artifacts['reference-lock.json'] = sha256(lockBytes);
      if (lock.schema_version !== 1 || lock.render_mode !== 'native-measured-svg' || lock.reference_hash !== payload.reference_hash || lock.contract_hash !== payload.contract_hash || !same(lock.context, payload.context))
        source_validation.errors.push({ where: 'reference-lock.identity', expected: 'immutable authored baseline identity/context', actual: lock });
      const lockedFiles = ['document.json', 'snapshot.json', 'generated.rs', 'manifest.json', 'reference.html', 'measured-layout.json'].sort();
      if (!same(Object.keys(lock.artifacts ?? {}).sort(), lockedFiles)) source_validation.errors.push({ where: 'reference-lock.artifacts', expected: lockedFiles, actual: Object.keys(lock.artifacts ?? {}) });
      for (const filename of lockedFiles) if (lock.artifacts?.[filename] !== artifacts[filename])
        source_validation.errors.push({ where: `reference-lock.${filename}`, expected: lock.artifacts?.[filename], actual: artifacts[filename] });
      if (source_validation.errors.length) source_validation.status = 'failed';
      paint = comparePaint(native, measured);
    }
    const nativePng = await readFile(path.join(directory, 'native.png')), browserPng = await readFile(path.join(directory, 'browser.png'));
    artifacts['native.png'] = sha256(nativePng); artifacts['browser.png'] = sha256(browserPng);
    artifacts['browser.json'] = sha256(await readFile(path.join(directory, 'browser.json')));
    rasterArtifacts = compareRasters(nativePng, browserPng);
    raster = rasterArtifacts.result;
    if (raster.status === 'failed') {
      const edgeCoverage = classifyEdgeRaster(nativePng, browserPng, native, measured, { geometry, source_validation, paint });
      raster = { ...raster, status: edgeCoverage.status, exact_rgba: false, edge_coverage: edgeCoverage,
        criterion: edgeCoverage.status === 'passed' ? edgeCoverage.criterion : raster.criterion };
    }
    const nativeImage = decodePng(nativePng);
    if (nativeImage.width !== Math.round(native.context.viewport.width * native.context.scale) || nativeImage.height !== Math.round(native.context.viewport.height * native.context.scale))
      geometry.errors.push({ where: 'native screenshot scale', expected: [native.context.viewport.width * native.context.scale, native.context.viewport.height * native.context.scale], actual: [nativeImage.width, nativeImage.height] });
    if (geometry.errors.length) geometry.status = 'failed';
    if (rasterArtifacts.diff) { await writeFile(path.join(directory, 'diff.png'), rasterArtifacts.diff); artifacts['diff.png'] = sha256(rasterArtifacts.diff); }
    if (rasterArtifacts.overlay) { await writeFile(path.join(directory, 'overlay.png'), rasterArtifacts.overlay); artifacts['overlay.png'] = sha256(rasterArtifacts.overlay); }
  } catch (error) {
    geometry = { status: 'failed', errors: [{ where: 'capture', actual: error.message }] };
  }
  const geometryBytes = JSON.stringify(geometry, null, 2) + '\n';
  await writeFile(path.join(directory, 'geometry-diff.json'), geometryBytes);
  artifacts['geometry-diff.json'] = sha256(geometryBytes);
  const rasterBytes = JSON.stringify(raster, null, 2) + '\n';
  await writeFile(path.join(directory, 'raster-diff.json'), rasterBytes);
  artifacts['raster-diff.json'] = sha256(rasterBytes);
  const interaction = verifyInteraction(native, snapshot, authoredDocument);
  if (interaction.status === 'passed') {
    for (const entry of interaction.cases) for (const [filename, expectedHash] of Object.entries(entry.evidence)) try {
      const bytes = await readFile(path.join(directory, filename)), actualHash = sha256(bytes);
      decodePng(bytes);
      artifacts[filename] = actualHash;
      if (actualHash !== expectedHash) interaction.errors.push({ where: `interaction.${entry.name}.${filename}`, expected: expectedHash, actual: actualHash });
    } catch (error) { interaction.errors.push({ where: `interaction.${entry.name}.${filename}`, expected: 'actual native frame evidence', actual: error.message }); }
    if (interaction.errors.length) interaction.status = 'failed';
  }
  const verifierIntegrity = sha256(await readFile(fileURLToPath(import.meta.url))) === EXECUTED_VERIFIER_HASH;
  const outcomes = [source_validation, paint, geometry, raster, interaction, { status: verifierIntegrity ? 'passed' : 'failed' }];
  const status = outcomes.every(result => result.status === 'passed') ? 'certified' : outcomes.some(result => result.status === 'failed') ? 'failed' : 'measured';
  const certificate = { schema_version: 1, reference_hash: native.reference_hash, contract_hash: native.contract_hash, context: native.context, reference_context: native.reference_context ?? null, native_hash: sha256(nativeBytes), html_hash: sha256(htmlBytes), artifacts, verifier: { version: VERIFIER_VERSION, sha256: EXECUTED_VERIFIER_HASH, status: verifierIntegrity ? 'passed' : 'failed', reason: verifierIntegrity ? 'source unchanged throughout verification' : 'source changed while verification was running' }, browser_engine: measured?.observed.browser_engine ?? null, browser_version: measured?.observed.browser_version ?? null, source_validation, font_bindings, paint, geometry, raster, interaction, status };
  await writeFile(path.join(directory, 'certificate.json'), JSON.stringify(certificate, null, 2) + '\n');
  return certificate;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 3) { console.error('Usage: node scripts/verify-ui-reference.mjs <native-capture-directory>'); process.exitCode = 2; }
  else try {
    const certificate = await verifyDirectory(process.argv[2]);
    console.log(JSON.stringify({ status: certificate.status, geometry: certificate.geometry.status, raster: certificate.raster.status, interaction: certificate.interaction.status, certificate: path.join(path.resolve(process.argv[2]), 'certificate.json') }, null, 2));
    if (certificate.status === 'failed') process.exitCode = 1;
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
