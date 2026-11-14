import{c as m,j as n,aX as u,y as p}from"./index-UFkUhBrB.js";/**
 * @license lucide-react v1.72.0 - ISC
 *
 * This source code is licensed under the ISC license.
 * See the LICENSE file in the root directory of this source tree.
 */const t={name:"pencil",size:24,node:[["path",{d:"M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z",key:"1a8usu"}],["path",{d:"m15 5 4 4",key:"1mk7zo"}]]};t.node;const d=m(t);function f({checked:s,onChange:c,disabled:e=!1,label:a="",title:o="",ariaLabel:r=""}){const i=r||o||`${a||"状态"}${s?"（已启用）":"（已停用）"}`;return n.jsxs("label",{className:p("inline-flex items-center gap-2 select-none",e?"cursor-not-allowed opacity-60":"cursor-pointer"),title:o,children:[n.jsx(u,{checked:!!s,onCheckedChange:l=>!e&&c&&c(l),disabled:e,"aria-label":i}),a&&n.jsx("span",{className:"text-xs font-medium text-foreground",children:a})]})}export{d as P,f as Q};
