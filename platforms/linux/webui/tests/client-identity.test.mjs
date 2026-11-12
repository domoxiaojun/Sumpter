import assert from 'node:assert/strict';
import test from 'node:test';
import { parseIdentityAliases, parseRelayEndpoints, identityAliasesText, validateIdentityBindings } from '../src/utils/clientIdentity.js';
import { CLIENT_CATALOG, CLIENT_VARIANTS } from '../src/utils/clientRegistry.js';
import { clientAttributionSummary, clientKindLabel } from '../src/utils/helpers.js';

test('exact identity mappings reject malformed, duplicate and excessive rules', () => {
  assert.deepEqual(parseIdentityAliases('custom-cli=codex\nmy-sdk=open_code'), {'custom-cli':'codex','my-sdk':'open_code'});
  for (const value of ['name=not-a-client','bad product=codex','name=codex\nname=pi','name=codex=pi',Array.from({length:129},(_,i)=>`p${i}=codex`).join('\n')]) assert.throws(()=>parseIdentityAliases(value));
  const special = parseIdentityAliases('__proto__=codex');
  assert.equal(Object.getPrototypeOf(special),Object.prototype);
  assert.equal(special.__proto__,'codex');
  assert.equal(identityAliasesText(special),'__proto__=codex');
});

test('token binding validation uses stable IDs and the complete shared variant catalog', () => {
  assert.doesNotThrow(()=>validateIdentityBindings({[`token-${'a'.repeat(64)}`]:{client:'custom-sdk',variant:'work_cca',relaySource:'office'}}));
  for (const b of [{'bad id':{client:'codex'}},{a:{client:'bad/label'}},{a:{client:'codex',variant:'invalid'}},{a:{client:'codex',relaySource:'bad source'}}]) assert.throws(()=>validateIdentityBindings(b));
  assert.ok(CLIENT_VARIANTS.includes('app_server_sdk'));
  for (const product of CLIENT_CATALOG) assert.equal(clientKindLabel(product.id),product.name);
});

test('relay configuration retains endpoint IDs without accepting arbitrary source labels', () => {
  const id=`Endpoint-${'a'.repeat(80)}`;
  assert.deepEqual(parseRelayEndpoints(`${id}=home`),{[id]:'home'});
  for (const value of ['target=bad source','target=home\ntarget=office',`${'x'.repeat(129)}=home`]) assert.throws(()=>parseRelayEndpoints(value));
});

test('identity settings survive complete config read-write conversion', async () => {
  globalThis.window={location:{search:'?mock=1',assign(){}}};
  const {fromWireConfig,toWireConfig}=await import('../src/services/api.js');
  const clientIdentity={tokenBindings:{legacy:{client:'custom-sdk',variant:'sdk'}},reportingAliases:{'custom-sdk':'codex'},uaAliases:{'custom-cli':'open_code'},relayEndpoints:{remote:'home'}};
  const document={generation:7,config:{schemaVersion:7,listener:{port:57878,clientIdentity},endpoints:[]}};
  const decoded=fromWireConfig(document);
  assert.deepEqual(toWireConfig(decoded).listener.clientIdentity,clientIdentity);
  assert.deepEqual(document.config.listener.clientIdentity,clientIdentity);
});

test('identity display separates raw product, engine, host, relay and reporting labels', () => {
  const label=clientAttributionSummary({identity:{rawProduct:'openai',product:'custom-sdk',host:'vscode',engine:'codex',relaySource:'office',reportedAs:'codex'},evidence:[{field:'clientKind',source:'token_binding',rule:'server-config',value:'custom-sdk'}],conflicts:[],issues:[]});
  for (const fragment of ['原始产品：openai','产品：custom-sdk','宿主：vscode','引擎：codex','中继：office','统计归属：codex','Token 绑定']) assert.ok(label.includes(fragment),label);
});
