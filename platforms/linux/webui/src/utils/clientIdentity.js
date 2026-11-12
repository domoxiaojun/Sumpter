import { CLIENT_CATALOG, CLIENT_VARIANTS } from './clientRegistry.js';
export const validProduct = value => /^[a-z0-9._-]{1,64}$/.test(value);
export function parseIdentityAliases(text) {
  const result = {};
  for (const line of text.split('\n').map(v=>v.trim()).filter(Boolean)) {
    const parts=line.split('=');
    if(parts.length!==2 || !validProduct(parts[0]) || !CLIENT_CATALOG.some(c=>c.id===parts[1])) throw new Error('映射每行填写 产品标识=客户端ID，目标须为目录中的客户端');
    if(Object.hasOwn(result,parts[0])) throw new Error('产品标识不能重复');
    Object.defineProperty(result,parts[0],{value:parts[1],enumerable:true});
  }
  if(Object.keys(result).length>128) throw new Error('映射最多 128 条');
  return result;
}
export const identityAliasesText = map => Object.entries(map || {}).map(([k,v])=>`${k}=${v}`).join('\n');
export function validateIdentityBindings(bindings) {
  if(Object.keys(bindings).length>129) throw new Error('客户端绑定最多 129 条');
  for(const [id,binding] of Object.entries(bindings)) {
    if(!/^[A-Za-z0-9_-]{1,128}$/.test(id)) throw new Error('Token ID 无效');
    if(binding.variant && !CLIENT_VARIANTS.includes(binding.variant)) throw new Error('未知客户端入口形态');
    if(!validProduct(binding.client)) throw new Error('客户端标识须为 1–64 个小写字母、数字、点、横线或下划线');
    if(binding.relaySource && !validProduct(binding.relaySource)) throw new Error('中继来源标识无效');
  }
}
export function parseRelayEndpoints(text) {
  const entries=text.split('\n').map(v=>v.trim()).filter(Boolean).map(line=>line.split('='));
  if(entries.length>128 || entries.some(([id,value,...rest])=>rest.length || !id || new TextEncoder().encode(id).length>128 || !validProduct(value))) throw new Error('中继目标每行填写 入口ID=来源标识，最多 128 条');
  if(new Set(entries.map(([id])=>id)).size!==entries.length) throw new Error('中继入口不能重复');
  return Object.fromEntries(entries);
}
