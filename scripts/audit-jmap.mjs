import { parse } from '@babel/parser';
import { readFileSync, readdirSync, writeFileSync, existsSync } from 'node:fs';
import { resolve, relative } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const methodPattern = /^(?:x:)?[A-Z][A-Za-z0-9]{0,39}\/[a-z][A-Za-z0-9]{0,39}$/;
const excluded = /(^|\/)(__tests__|tests|test|e2e|fixtures|__fixtures__|node_modules|\.next|\.git|dist)(\/|$)|\.(test|spec)\.[cm]?[jt]sx?$/;
export function sourceFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (excluded.test(path.replaceAll('\\', '/'))) return [];
    return entry.isDirectory() ? sourceFiles(path) : /\.[cm]?[jt]sx?$/.test(entry.name) ? [path] : [];
  }).sort();
}
function visit(node, fn, parent = null, owner = '<module>') {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const child of node) visit(child, fn, parent, owner); return; }
  if (typeof node.type !== 'string') return;
  if (/Function|Method/.test(node.type)) owner = node.id?.name || node.key?.name || node.key?.value || owner;
  fn(node, parent, owner);
  for (const [key, child] of Object.entries(node)) {
    if (!['loc', 'tokens', 'comments', 'extra'].includes(key)) visit(child, fn, node, owner);
  }
}
function keyOf(node) { return node?.name ?? node?.value; }
export function scanSource(source, file) {
  source = source.replaceAll('\r\n', '\n');
  const ast = parse(source, { sourceType: 'unambiguous', allowReturnOutsideFunction: true, plugins: [...(/\.[cm]?tsx?$/.test(file) ? ['typescript'] : []), ...(/\.[jt]sx$/.test(file) ? ['jsx'] : [])], errorRecovery: false });
  const refs = [], calls = [], dynamic = [], requests = [], transports = [];
  const symbols = new Map();
  const types = new Map();
  const parameters = new Map();
  visit(ast, (node, parent, owner) => {
    if (node.type === 'TSTypeAliasDeclaration') types.set(node.id.name, node.typeAnnotation);
    if (node.type === 'Identifier' && node.typeAnnotation) parameters.set(`${owner}:${node.name}`, node.typeAnnotation.typeAnnotation);
    if (node.type === 'VariableDeclarator' && node.id?.type === 'Identifier' && node.init) symbols.set(node.id.name, node.init);
  });
  const layer = file.includes('/app/api/dev-jmap/') ? 'development-mock' : file.includes('/app/api/') ? 'web-server' : file.includes('/scripts/') ? 'tooling' : 'client-or-shared';
  const loc = (node) => ({ file, layer, line: node.loc.start.line, column: node.loc.start.column + 1 });
  const expression = (node) => node ? source.slice(node.start, node.end) : '';
  function values(node, owner, seen = new Set()) {
    if (!node || seen.has(node)) return [];
    seen = new Set([...seen, node]);
    if (node.type === 'StringLiteral') return [node.value];
    if (node.type === 'TSLiteralType') return values(node.literal, owner, seen);
    if (node.type === 'TSUnionType') return node.types.flatMap((item) => values(item, owner, seen));
    if (node.type === 'TSTypeReference') return values(types.get(node.typeName.name), owner, seen);
    if (node.type === 'Identifier') return values(parameters.get(`${owner}:${node.name}`) || symbols.get(node.name), owner, seen);
    if (node.type === 'TemplateLiteral') {
      let result = [node.quasis[0].value.cooked];
      for (let i = 0; i < node.expressions.length; i++) {
        const choices = values(node.expressions[i], owner, seen);
        if (!choices.length || choices.length * result.length > 100) return [];
        result = result.flatMap((prefix) => choices.map((choice) => prefix + choice + node.quasis[i + 1].value.cooked));
      }
      return result;
    }
    return [];
  }

  function shape(node, seen = new Set(), prefix = '') {
    if (!node) return [];
    if (['TSAsExpression', 'TSSatisfiesExpression', 'TSNonNullExpression'].includes(node.type)) return shape(node.expression, seen, prefix);
    if (['StringLiteral', 'NumericLiteral', 'BooleanLiteral', 'NullLiteral'].includes(node.type)) return [{ path: prefix, expression: expression(node), dynamic: false }];
    if (node.type === 'Identifier' && symbols.has(node.name) && !seen.has(node.name)) {
      return shape(symbols.get(node.name), new Set([...seen, node.name]), prefix);
    }
    if (node.type === 'ObjectExpression') return node.properties.flatMap((property) => {
      if (property.type === 'SpreadElement') return [{ path: prefix || '<args>', expression: expression(property), dynamic: true }];
      const key = keyOf(property.key);
      if (key === undefined || property.computed) return [{ path: prefix || '<args>', expression: expression(property), dynamic: true }];
      const path = prefix ? `${prefix}.${key}` : String(key);
      return [{ path, expression: expression(property.value).slice(0, 400), dynamic: !['StringLiteral', 'NumericLiteral', 'BooleanLiteral', 'NullLiteral'].includes(property.value?.type) }, ...shape(property.value, seen, path)];
    });
    if (node.type === 'ArrayExpression') return node.elements.flatMap((item, index) => shape(item, seen, `${prefix}[${index}]`));
    return [];
  }
  visit(ast, (node, parent, owner) => {
    if (node.type === 'StringLiteral' && methodPattern.test(node.value)) refs.push({ method: node.value, ...loc(node), owner });
    if (node.type === 'ArrayExpression' && node.elements.length >= 2) {
      const [first, args] = node.elements;
      const resolved = values(first, owner).filter((name) => methodPattern.test(name));
      if (first?.type !== 'StringLiteral' && resolved.length) {
        for (const method of resolved) {
          refs.push({ method, ...loc(node), owner, derived: true });
          calls.push({ method, ...loc(node), owner, derived: true, argumentExpression: expression(args).slice(0, 600), fields: shape(args) });
        }
      }
      if (first?.type === 'StringLiteral' && methodPattern.test(first.value) && args && !['StringLiteral', 'NullLiteral'].includes(args.type)) {
        calls.push({ method: first.value, ...loc(node), owner, argumentExpression: expression(args).slice(0, 600), fields: shape(args) });
      } else if ((first?.type === 'TemplateLiteral' && /\/(get|set|query|changes|copy|import|parse)/.test(expression(first))) || (first?.type === 'Identifier' && /^(method|methodName|jmapMethod)$/.test(first.name) && args)) {
        dynamic.push({ ...loc(node), owner, expression: expression(node).slice(0, 500), resolvedMethods: resolved, reason: resolved.length ? 'Endliche Typunion/Template statisch aufgeloest.' : 'Dynamischer Methodenname; statisch nicht vollstaendig aufloesbar.' });
      }
    }
    if ((node.type === 'CallExpression' || node.type === 'OptionalCallExpression') && /^(request|jmapRequest|requestJmap|stalwartJmap)$/.test(keyOf(node.callee?.property) || node.callee?.name || '')) {
      requests.push({ ...loc(node), owner, argumentExpression: expression(node.arguments[0]).slice(0, 300), inlineArray: node.arguments[0]?.type === 'ArrayExpression' });
    }
    const transport = node.type === 'NewExpression' ? node.callee?.name : (node.type === 'CallExpression' || node.type === 'OptionalCallExpression') ? (node.callee?.name || keyOf(node.callee?.property)) : null;
    if (['fetch', 'EventSource', 'WebSocket', 'XMLHttpRequest', 'sendBeacon'].includes(transport)) transports.push({ ...loc(node), owner, transport, targetExpression: expression(node.arguments?.[0]).slice(0, 500) });
    if (node.type === 'TemplateLiteral' && /^`(?:x:)?(?:\$\{[^}]+\}|[A-Z][A-Za-z0-9]*)\/(?:get|set|query|changes|queryChanges|copy|import|parse|validate)`$/.test(expression(node)) && node.expressions.length && parent?.type !== 'ArrayExpression') {
      dynamic.push({ ...loc(node), owner, expression: expression(node).slice(0, 500), reason: 'Zusammengesetzter Methodenname/Template: manuelle Pruefung.' });
    }
  });
  return { refs, calls, dynamic, requests, transports };
}

export function inventory(base = root) {
  const upstream = resolve(base, 'upstream/bulwark');
  const files = sourceFiles(upstream);
  const result = { files: [], refs: [], calls: [], dynamic: [], requests: [], transports: [] };
  for (const path of files) {
    const file = relative(base, path).replaceAll('\\', '/');
    let scan;
    try { scan = scanSource(readFileSync(path, 'utf8'), file); } catch (error) { throw new Error(`${file}: ${error.message}`); }
    result.files.push(file);
    for (const key of ['refs', 'calls', 'dynamic', 'requests', 'transports']) result[key].push(...scan[key]);
  }
  return result;
}
function sourceLink(file, line = 1) { return `[${file}:${line}](../../${file}#L${line})`; }
export function render(data, review, reference, commit) {
  const counts = Object.fromEntries(['partial', 'fallback', 'missing', 'reference-only'].map((s) => [s, 0]));
  const nonProtocol = review.nonProtocol || {};
  const nonProtocolReferences = data.refs.filter((r) => Object.hasOwn(nonProtocol, r.method)).map((r) => ({ ...r, reason: nonProtocol[r.method] }));
  if (data.calls.some((r) => Object.hasOwn(nonProtocol, r.method))) throw new Error('Als Nicht-Protokoll bewerteter Name erscheint in einem Aufruftupel.');
  const methods = [...new Set(data.refs.map((r) => r.method))].filter((method) => !Object.hasOwn(nonProtocol, method)).sort();
  for (const site of data.dynamic) {
    const context = `${site.file}:${site.owner}`;
    if (!review.dynamicReviews.some((r) => r.context === context)) throw new Error(`Dynamischer Pfad nicht bewertet: ${context}`);
  }
  const rows = methods.map((method) => {
    const entry = review.methods[method];
    if (!entry) throw new Error(`Nicht bewertete Methode: ${method}`);
    if (!['partial', 'fallback', 'missing', 'reference-only'].includes(entry.status) || !entry.limitations || !entry.local.length || !entry.stalwart.length) throw new Error(`Unvollstaendige Bewertung: ${method}`);
    counts[entry.status] = (counts[entry.status] || 0) + 1;
    const refs = data.refs.filter((r) => r.method === method);
    const calls = data.calls.filter((r) => r.method === method);
    return { method, ...entry, occurrences: refs, callSites: calls, stalwart: entry.stalwart.map((path) => {
      const file = reference.files.find((item) => item.path === path);
      if (!file) throw new Error(`Unbekannter Stalwart-Pfad: ${path}`);
      return { ...file, url: `${reference.repository}/blob/${reference.commit}/${path}` };
    }) };
  });
  for (const method of Object.keys(review.methods)) if (!methods.includes(method)) throw new Error(`Veraltete Methodenbewertung: ${method}`);
  const report = { schemaVersion: 1, nonProtocolReferences, bulwarkCommit: commit, stalwartCommit: reference.commit,
    scope: 'Alle JS/TS-Quellen im fixierten Bulwark-Submodul ohne Tests/Fixtures/Buildausgaben; Serverrouten, Entwicklungsmock und Werkzeuge sind enthalten und je Fundstelle markiert. Statische Methodennamen, Aufruftupel und dynamische Requeststellen; kein Beweis fuer Laufzeit-Erreichbarkeit oder vollstaendige dynamische Werte.',
    dynamicReviews: review.dynamicReviews, transportReviews: review.transportReviews,
    transportSites: data.transports,
    summary: { sourceFiles: data.files.length, methods: rows.length, tupleSites: data.calls.length, dynamicSites: data.dynamic.length, requestSites: data.requests.length, transportSites: data.transports.length, statuses: counts },
    sourceFiles: data.files, methods: rows, dynamicMethodSites: data.dynamic, requestSites: data.requests };
  const lines = ['# JMAP-Kompatibilitaetsmatrix', '', '<!-- Generiert: npm run audit:jmap; Bewertungen: review.json. Nicht von Hand bearbeiten. -->', '',
    `Bulwark: \`${commit}\`. Stalwart-Referenz: \`${reference.commit}\`.`, '',
    `${data.files.length} Quelldateien; ${rows.length} Methodennamen; ${data.calls.length} statische Aufruftupel; ${data.dynamic.length} dynamische Stellen; ${data.requests.length} Requeststellen; ${data.transports.length} HTTP/Push-Transportstellen.`, '',
    '## Aussage und Grenzen', '', report.scope, '',
    'Ein Literal kann ein Aufruf, eine Antwortpruefung oder ein Protokollbeispiel sein. Aufruftupel sind syntaktische Kandidaten, kein Laufzeitnachweis. Vollstaendige Fundstellen, Argumentfelder (einschliesslich properties/filter/sort und Referenzen) sowie dynamische Ausdruecke stehen in inventory.json. Dynamische Werte, externe Plugins, HTTP-/Blob-/Push-Nebenpfade und serverseitige Erreichbarkeit benoetigen gesonderte Pruefung.', '',
    'partial = Teilumfang; fallback = Ersatzantwort/Neuladen statt Methodensemantik; missing = kein Desktoppfad; reference-only = reine Referenz ohne gefundenes Aufruftupel. Testbelege nennen vorhandene Tests, nicht die Garantie aller Argumentkombinationen. Ein Stalwart-Quellpfad ist eine Referenz, kein Nachweis fuer aktivierte Serverfunktionen.', '',
    '## Methoden', '', '| Methode | Stand | Lokaler Pfad / Grenze | Stalwart | Tests |', '|---|---|---|---|---|'];
  for (const row of rows) {
    const local = row.local.map((r) => sourceLink(r.file, r.line)).join(', ');
    const server = row.stalwart.map((r) => `[${r.path.split('/').slice(-2).join('/')}](${r.url})`).join(', ');
    const tests = row.tests.map((r) => `${sourceLink(r.file, r.line)} (${r.name})`).join(', ') || 'Kein dedizierter Nachweis';
    lines.push(`| ${row.method} | ${row.status} | ${local}${local ? '<br>' : ''}${row.limitations.replaceAll('|', '/')} | ${server} | ${tests} |`);
  }
  lines.push('', '## Dynamische Methodenstellen', '', '| Quelle | Kontext | Ausdruck |', '|---|---|---|');
  for (const row of data.dynamic) lines.push(`| ${sourceLink(row.file, row.line)} | ${row.owner} | \`${row.expression.replaceAll('`', "'").replaceAll('|', '/').replaceAll(/\s+/g, ' ').slice(0, 220)}\` |`);
  lines.push('', '## Ausgeschlossene Nicht-Protokollnamen', '', ...Object.entries(nonProtocol).map(([name, reason]) => `- ${name}: ${reason}`));
  lines.push('', '## Gepruefte dynamische Pfade', '', ...review.dynamicReviews.map((r) => `- ${r.context}: ${r.review}`), '', '## HTTP, Blob und Push', '', ...review.transportReviews.map((r) => `- ${r.path}: ${r.review}`));
  lines.push('', '## Prioritaeten', '', ...review.priorities.map((r, i) => `${i + 1}. ${r}`), '', '## Reproduzieren', '', '`npm run audit:jmap` schreibt Matrix und JSON. `npm run check:jmap` prueft Drift ohne Schreiben. Eine neue Methode benoetigt eine explizite Bewertung in review.json. Der Generator verwendet ausschliesslich lokale Quellen und die festgehaltene Stalwart-Pfadliste; kein Serverzugriff und keine Zugangsdaten.', '');
  return { report, markdown: lines.join('\n') };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const data = inventory();
  if (process.argv.includes('--scan')) {
    writeFileSync(resolve(root, '.cache/jmap-scan.json'), JSON.stringify(data, null, 2) + '\n');
  } else {
    const review = JSON.parse(readFileSync(resolve(root, 'docs/jmap/review.json'), 'utf8'));
    for (const entry of Object.values(review.methods)) for (const ref of [...entry.local, ...entry.tests]) {
      const source = readFileSync(resolve(root, ref.file), 'utf8');
      if (ref.anchor) {
        const index = source.indexOf(ref.anchor);
        if (index < 0) throw new Error(`Quellanker fehlt: ${ref.file}: ${ref.anchor}`);
        ref.line = source.slice(0, index).split('\n').length;
      }
      if (!Number.isInteger(ref.line) || ref.line < 1 || ref.line > source.split('\n').length) throw new Error(`Ungueltiger Quellbeleg: ${ref.file}:${ref.line}`);
      if (ref.name && !source.includes(ref.name)) throw new Error(`Test fehlt: ${ref.name}`);
    }
    const reference = JSON.parse(readFileSync(resolve(root, 'docs/jmap/stalwart-reference.json'), 'utf8'));
    const commit = execFileSync('git', ['-C', resolve(root, 'upstream/bulwark'), 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
    const { report, markdown } = render(data, review, reference, commit);
    const localFiles = [...new Set(Object.values(review.methods).flatMap((entry) => [...entry.local, ...entry.tests].map((r) => r.file)).concat(['src-tauri/src/replica.rs', 'src-tauri/src/lib.rs', 'src-tauri/src/sync.rs', 'src-tauri/src/push.rs', 'src-tauri/src/uploads.rs', 'src/desktop/blobs.ts', 'vite.config.ts', 'scripts/prepare-bulwark.mjs', 'tests/desktop/bulwark.spec.ts']))].sort();
    report.localSourceDigests = Object.fromEntries(localFiles.map((file) => [file, createHash('sha256').update(readFileSync(resolve(root, file), 'utf8').replaceAll('\r\n', '\n')).digest('hex')]));
    for (const [name, content] of [['inventory.json', JSON.stringify(report, null, 2) + '\n'], ['matrix.md', markdown]]) {
      const path = resolve(root, 'docs/jmap', name);
      if (process.argv.includes('--check')) {
        if (!existsSync(path) || readFileSync(path, 'utf8') !== content) throw new Error(`JMAP-Abgleich veraltet: ${name}; npm run audit:jmap ausfuehren.`);
      } else writeFileSync(path, content);
    }
    console.log(JSON.stringify(report.summary));
  }
}
