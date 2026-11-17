import { test } from 'node:test';
import assert from 'node:assert/strict';
import { scanSource, render } from './audit-jmap.mjs';

const file = 'upstream/bulwark/lib/example.ts';
test('AST scan ignores comments and distinguishes references from call tuples', () => {
  const scan = scanSource(`// ['Fake/get', {}, '0']\nconst note = 'Email/get'; const calls = [['Email/query', {filter: {inMailbox: box}, sort: [{property:'size', isAscending:false}], '#ids': {resultOf:'q',name:'Email/query',path:'/ids'}}, 'q']];`, file);
  assert.equal(scan.calls.length, 1);
  assert.equal(scan.calls[0].method, 'Email/query');
  assert.ok(!scan.refs.some((r) => r.method === 'Fake/get'));
  assert.equal(scan.refs.filter((r) => r.method === 'Email/get').length, 1);
  assert.ok(scan.calls[0].fields.some((r) => r.path === 'filter.inMailbox' && r.dynamic));
  assert.ok(scan.calls[0].fields.some((r) => r.path === 'sort[0].property' && r.expression === "'size'"));
  assert.ok(scan.calls[0].fields.some((r) => r.path === '#ids.resultOf'));
});

test('finite union methods and constant property lists are resolved without executing source', () => {
  const scan = scanSource("const PROPERTIES = ['id', 'subject'] as const; async function changes(type: 'Email' | 'Mailbox') { return request([[`${type}/changes`, { properties: PROPERTIES, ...extra }, '0']]); }", file);
  assert.deepEqual(scan.calls.map((r) => r.method), ['Email/changes', 'Mailbox/changes']);
  assert.deepEqual(scan.dynamic[0].resolvedMethods, ['Email/changes', 'Mailbox/changes']);
  assert.ok(scan.calls[0].fields.some((r) => r.path === 'properties[1]' && r.expression === "'subject'"));
  assert.ok(scan.calls[0].fields.some((r) => r.expression === '...extra' && r.dynamic));
});

test('unresolved methods, request expressions and transport targets remain explicit', () => {
  const scan = scanSource("function send(method: string, args: unknown) { client.request([[method, args, '0']]); fetch(apiUrl); new EventSource(events); }", file);
  assert.equal(scan.calls.length, 0);
  assert.deepEqual(scan.dynamic[0].resolvedMethods, []);
  assert.equal(scan.requests.length, 1);
  assert.deepEqual(scan.transports.map((r) => r.transport), ['fetch', 'EventSource']);
  assert.equal(scan.transports[0].targetExpression, 'apiUrl');
});

test('TSX parsing and syntax failures do not silently omit files', () => {
  assert.equal(scanSource("const element = <div/>; const call = ['Email/get', {}, '0'];", 'view.tsx').calls.length, 1);
  assert.throws(() => scanSource('const broken = {;', file));
});

test('new methods, missing server references and new dynamic contexts fail review', () => {
  const data = { files: [file], refs: [{method:'Email/get'}], calls: [], dynamic: [], requests: [], transports: [] };
  const review = {methods:{}, dynamicReviews:[],transportReviews:[],priorities:[]};
  const server = {repository:'https://example.test',commit:'pinned',files:[]};
  assert.throws(() => render(data, review, server, 'upstream'), /Nicht bewertete/);
  review.methods['Email/get'] = {status:'partial',local:[{file,line:1}],tests:[],stalwart:['missing.rs'],limitations:'Partial only'};
  assert.throws(() => render(data, review, server, 'upstream'), /Unbekannter Stalwart/);
  data.dynamic.push({file,owner:'newFunction'});
  assert.throws(() => render(data, review, server, 'upstream'), /Dynamischer Pfad/);
});

test('non-protocol exclusions cannot hide an actual call', () => {
  const data = {files:[],refs:[{method:'Widget/custom'}],calls:[{method:'Widget/custom'}],dynamic:[],requests:[],transports:[]};
  assert.throws(() => render(data,{nonProtocol:{'Widget/custom':'label'},methods:{},dynamicReviews:[]},{},'upstream'), /Nicht-Protokoll/);
});

test('inventory expressions are stable across Windows and Unix line endings', () => {
  const source = "const calls = [['Email/get', {\n  properties: ['id']\n}, '0']];";
  assert.deepEqual(scanSource(source, file), scanSource(source.replaceAll('\n', '\r\n'), file));
});
