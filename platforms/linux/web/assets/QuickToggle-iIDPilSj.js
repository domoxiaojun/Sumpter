import{c as u,j as a,aX as p,y as x}from"./index-PBs66drB.js";/**
 * @license lucide-react v1.72.0 - ISC
 *
 * This source code is licensed under the ISC license.
 * See the LICENSE file in the root directory of this source tree.
 */const t={name:"plus",size:24,node:[["path",{d:"M5 12h14",key:"1ays0h"}],["path",{d:"M12 5v14",key:"s699le"}]]};t.node;const h=u(t);function d({checked:n,onChange:o,disabled:e=!1,label:s="",title:c="",ariaLabel:r=""}){const i=r||c||`${s||"状态"}${n?"（已启用）":"（已停用）"}`;return a.jsxs("label",{className:x("inline-flex items-center gap-2 select-none",e?"cursor-not-allowed opacity-60":"cursor-pointer"),title:c,children:[a.jsx(p,{checked:!!n,onCheckedChange:l=>!e&&o&&o(l),disabled:e,"aria-label":i}),s&&a.jsx("span",{className:"text-xs font-medium text-foreground",children:s})]})}export{h as P,d as Q};
