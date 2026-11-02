import{m as se,j as i,I as W,r as u,v as Ye,c as Ke,e as V,a as Qe,i as Ze,b as Je,d as et,p as tt,f as ne,n as it,u as rt,o as de,g as st,h as he,k as nt,l as me,q as O,s as Re,t as Y,w as Ee,x as we,y as Ne,z as Ue,A as Ce,B as Te,C as te,D as Ae,E as Me,F as at,G as ot,H as lt,J as Pe,K as ct,L as ut,M as ze,N as dt,O as ht,P as mt}from"./index-B3usRTUb.js";import{S as ft,c as pt}from"./clipboard-B9K1bszW.js";import{D as gt}from"./DataTable-BrL4R5-O.js";import{P as vt}from"./outputSpeed-CA2u9pZm.js";import{E as xt,e as ae,a as bt,b as yt,c as St,C as _t}from"./EventInspector-Dc1cw6oP.js";import"./requestPolicy-CjaOnvV0.js";function fe(e,t=[],r=e?.requestID){const n=new Map;for(const s of[...e?.events||[],...t])s.requestID===r&&n.set(s.id,se(n.get(s.id),s));const a=[...n.values()].sort((s,o)=>Number(s.seq||0)-Number(o.seq||0));return{...e,requestID:r,events:a.slice(0,512),truncated:!!(e?.truncated||a.length>512)}}function jt(e){return e?.events?.some(t=>t.kind==="client"&&["inFlight","in_flight"].includes(t.phase))||!1}function Rt({read:e,onValue:t,enabled:r,interval:n=1e3,schedule:a=setTimeout,unschedule:s=clearTimeout,now:o=Date.now}){let l=!1,m=!0,f,d,g=-1/0;function b(y=!1){if(l||d||!y&&(!m||!r())||f!=null)return;const j=y?0:Math.max(0,n-(o()-g));f=a(()=>{f=void 0,S(y)},j)}async function S(y){if(l||d||!y&&!r())return;const j=new AbortController;d=j,g=o();try{const v=await e(j.signal);if(l||j.signal.aborted||!y&&!r())return;m=jt(v),t(v)}catch(v){[401,403,404].includes(v?.status)&&(m=!1)}finally{d=void 0,!l&&m&&r()&&(f=a(()=>{f=void 0,S(!1)},n))}}return{refresh:b,synchronize(){r()?b():(f!=null&&s(f),f=void 0)},dispose(){l=!0,f!=null&&s(f),d?.abort()}}}function J({label:e,value:t,detail:r,icon:n,accent:a="var(--primary)"}){return i.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[i.jsxs("div",{className:"metric-head",children:[i.jsx("span",{className:"metric-label",children:e}),n&&i.jsx("div",{className:"metric-icon-box",children:i.jsx(W,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),i.jsx("div",{className:"metric-value",children:t}),r&&i.jsx("div",{className:"metric-detail",children:r})]})}function Et({observation:e,autoRefresh:t}){const r=u.useRef(null);r.current=Ye(r.current,e,t);const n=Ke(r.current);return i.jsxs("section",{className:"responses-connections-panel","aria-label":"当前 Responses WebSocket",children:[i.jsxs("div",{className:"responses-connections-heading",children:[i.jsx("strong",{children:"当前 Responses WebSocket"}),!t&&i.jsx("span",{children:"已暂停刷新"}),n.error&&i.jsx("span",{className:"responses-connections-error",role:"status",children:n.error})]}),n.message?i.jsx("p",{children:n.message}):i.jsx("dl",{className:"responses-connections-values",children:n.rows.map(([a,s])=>i.jsxs("div",{children:[i.jsx("dt",{children:a}),i.jsx("dd",{children:s})]},a))}),i.jsx("p",{children:"当前进程内的连接快照；已进入转发不代表模型正在生成。"})]})}const wt=`#version 300 es
precision mediump float;

layout(location = 0) in vec4 a_position;

uniform vec2 u_resolution;
uniform float u_pixelRatio;
uniform float u_imageAspectRatio;
uniform float u_originX;
uniform float u_originY;
uniform float u_worldWidth;
uniform float u_worldHeight;
uniform float u_fit;
uniform float u_scale;
uniform float u_rotation;
uniform float u_offsetX;
uniform float u_offsetY;

out vec2 v_objectUV;
out vec2 v_objectBoxSize;
out vec2 v_responsiveUV;
out vec2 v_responsiveBoxGivenSize;
out vec2 v_patternUV;
out vec2 v_patternBoxSize;
out vec2 v_imageUV;

vec3 getBoxSize(float boxRatio, vec2 givenBoxSize) {
  vec2 box = vec2(0.);
  // fit = none
  box.x = boxRatio * min(givenBoxSize.x / boxRatio, givenBoxSize.y);
  float noFitBoxWidth = box.x;
  if (u_fit == 1.) { // fit = contain
    box.x = boxRatio * min(u_resolution.x / boxRatio, u_resolution.y);
  } else if (u_fit == 2.) { // fit = cover
    box.x = boxRatio * max(u_resolution.x / boxRatio, u_resolution.y);
  }
  box.y = box.x / boxRatio;
  return vec3(box, noFitBoxWidth);
}

void main() {
  gl_Position = a_position;

  vec2 uv = gl_Position.xy * .5;
  vec2 boxOrigin = vec2(.5 - u_originX, u_originY - .5);
  vec2 givenBoxSize = vec2(u_worldWidth, u_worldHeight);
  givenBoxSize = max(givenBoxSize, vec2(1.)) * u_pixelRatio;
  float r = u_rotation * 3.14159265358979323846 / 180.;
  mat2 graphicRotation = mat2(cos(r), sin(r), -sin(r), cos(r));
  vec2 graphicOffset = vec2(-u_offsetX, u_offsetY);


  // ===================================================

  float fixedRatio = 1.;
  vec2 fixedRatioBoxGivenSize = vec2(
  (u_worldWidth == 0.) ? u_resolution.x : givenBoxSize.x,
  (u_worldHeight == 0.) ? u_resolution.y : givenBoxSize.y
  );

  v_objectBoxSize = getBoxSize(fixedRatio, fixedRatioBoxGivenSize).xy;
  vec2 objectWorldScale = u_resolution.xy / v_objectBoxSize;

  v_objectUV = uv;
  v_objectUV *= objectWorldScale;
  v_objectUV += boxOrigin * (objectWorldScale - 1.);
  v_objectUV += graphicOffset;
  v_objectUV /= u_scale;
  v_objectUV = graphicRotation * v_objectUV;

  // ===================================================

  v_responsiveBoxGivenSize = vec2(
  (u_worldWidth == 0.) ? u_resolution.x : givenBoxSize.x,
  (u_worldHeight == 0.) ? u_resolution.y : givenBoxSize.y
  );
  float responsiveRatio = v_responsiveBoxGivenSize.x / v_responsiveBoxGivenSize.y;
  vec2 responsiveBoxSize = getBoxSize(responsiveRatio, v_responsiveBoxGivenSize).xy;
  vec2 responsiveBoxScale = u_resolution.xy / responsiveBoxSize;

  #ifdef ADD_HELPERS
  v_responsiveHelperBox = uv;
  v_responsiveHelperBox *= responsiveBoxScale;
  v_responsiveHelperBox += boxOrigin * (responsiveBoxScale - 1.);
  #endif

  v_responsiveUV = uv;
  v_responsiveUV *= responsiveBoxScale;
  v_responsiveUV += boxOrigin * (responsiveBoxScale - 1.);
  v_responsiveUV += graphicOffset;
  v_responsiveUV /= u_scale;
  v_responsiveUV.x *= responsiveRatio;
  v_responsiveUV = graphicRotation * v_responsiveUV;
  v_responsiveUV.x /= responsiveRatio;

  // ===================================================

  float patternBoxRatio = givenBoxSize.x / givenBoxSize.y;
  vec2 patternBoxGivenSize = vec2(
  (u_worldWidth == 0.) ? u_resolution.x : givenBoxSize.x,
  (u_worldHeight == 0.) ? u_resolution.y : givenBoxSize.y
  );
  patternBoxRatio = patternBoxGivenSize.x / patternBoxGivenSize.y;

  vec3 boxSizeData = getBoxSize(patternBoxRatio, patternBoxGivenSize);
  v_patternBoxSize = boxSizeData.xy;
  float patternBoxNoFitBoxWidth = boxSizeData.z;
  vec2 patternBoxScale = u_resolution.xy / v_patternBoxSize;

  v_patternUV = uv;
  v_patternUV += graphicOffset / patternBoxScale;
  v_patternUV += boxOrigin;
  v_patternUV -= boxOrigin / patternBoxScale;
  v_patternUV *= u_resolution.xy;
  v_patternUV /= u_pixelRatio;
  if (u_fit > 0.) {
    v_patternUV *= (patternBoxNoFitBoxWidth / v_patternBoxSize.x);
  }
  v_patternUV /= u_scale;
  v_patternUV = graphicRotation * v_patternUV;
  v_patternUV += boxOrigin / patternBoxScale;
  v_patternUV -= boxOrigin;
  // x100 is a default multiplier between vertex and fragmant shaders
  // we use it to avoid UV presision issues
  v_patternUV *= .01;

  // ===================================================

  vec2 imageBoxSize;
  if (u_fit == 1.) { // contain
    imageBoxSize.x = min(u_resolution.x / u_imageAspectRatio, u_resolution.y) * u_imageAspectRatio;
  } else if (u_fit == 2.) { // cover
    imageBoxSize.x = max(u_resolution.x / u_imageAspectRatio, u_resolution.y) * u_imageAspectRatio;
  } else {
    imageBoxSize.x = min(10.0, 10.0 / u_imageAspectRatio * u_imageAspectRatio);
  }
  imageBoxSize.y = imageBoxSize.x / u_imageAspectRatio;
  vec2 imageBoxScale = u_resolution.xy / imageBoxSize;

  v_imageUV = uv;
  v_imageUV *= imageBoxScale;
  v_imageUV += boxOrigin * (imageBoxScale - 1.);
  v_imageUV += graphicOffset;
  v_imageUV /= u_scale;
  v_imageUV.x *= u_imageAspectRatio;
  v_imageUV = graphicRotation * v_imageUV;
  v_imageUV.x /= u_imageAspectRatio;

  v_imageUV += .5;
  v_imageUV.y = 1. - v_imageUV.y;
}`,pe=1920*1080*4;let Nt=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=Tt();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(t,r,n,a,s=0,o=0,l=2,m=pe,f=[]){if(t?.nodeType===1)this.parentElement=t;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=t.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const b=this.ownerDocument.createElement("style");b.innerHTML=Ct,b.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(b)}const d=this.ownerDocument.createElement("canvas");this.canvasElement=d,this.parentElement.prepend(d),this.fragmentShader=r,this.providedUniforms=n,this.mipmaps=f,this.currentFrame=o,this.minPixelRatio=l,this.maxPixelCount=m;const g=d.getContext("webgl2",a);if(!g)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=g,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(s),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const t=Ut(this.gl,wt,this.fragmentShader);t&&(this.program=t)};setupPositionAttribute=()=>{const t=this.gl.getAttribLocation(this.program,"a_position"),r=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,r);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(t),this.gl.vertexAttribPointer(t,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const t={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([r,n])=>{if(t[r]=this.gl.getUniformLocation(this.program,r),n instanceof HTMLImageElement){const a=`${r}AspectRatio`;t[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=t};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([t])=>{if(t?.borderBoxSize[0]){const r=t.devicePixelContentBoxSize?.[0];r!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=r.inlineSize,this.parentDevicePixelHeight=r.blockSize),this.parentWidth=t.borderBoxSize[0].inlineSize,this.parentHeight=t.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const t=this.ownerDocument.defaultView;t?.IntersectionObserver&&(this.intersectionObserver=new t.IntersectionObserver(([r])=>{this.isInViewport=r?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let t=0,r=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const d=Math.max(1,this.minPixelRatio/n);t=this.parentDevicePixelWidth*d*a,r=this.parentDevicePixelHeight*d*a}else{let d=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const g=At(this.ownerDocument);d*=Math.max(1,g)}t=Math.round(this.parentWidth)*d,r=Math.round(this.parentHeight)*d}const s=Math.sqrt(this.maxPixelCount)/Math.sqrt(t*r),o=Math.min(1,s),l=Math.round(t*o),m=Math.round(r*o),f=l/Math.round(this.parentWidth);(this.canvasElement.width!==l||this.canvasElement.height!==m||this.renderScale!==f)&&(this.renderScale=f,this.canvasElement.width=l,this.canvasElement.height=m,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=t=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const r=t-this.lastRenderTime;this.lastRenderTime=t,this.currentSpeed!==0&&(this.currentFrame+=r*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(t,r)=>{if(!r.complete||r.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${t} must be fully loaded`);const n=this.textures.get(t);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(t)||this.textureUnitMap.set(t,this.textureUnitMap.size);const a=this.textureUnitMap.get(t);this.gl.activeTexture(this.gl.TEXTURE0+a);const s=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,s),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,r),this.mipmaps.includes(t)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||s===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(t,s);const l=this.uniformLocations[t];if(l){this.gl.uniform1i(l,a);const m=`${t}AspectRatio`,f=this.uniformLocations[m];if(f){const d=r.naturalWidth/r.naturalHeight;this.gl.uniform1f(f,d)}}};areUniformValuesEqual=(t,r)=>t===r?!0:Array.isArray(t)&&Array.isArray(r)&&t.length===r.length?t.every((n,a)=>this.areUniformValuesEqual(n,r[a])):!1;setUniformValues=t=>{this.gl.useProgram(this.program),Object.entries(t).forEach(([r,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[r],a))return;this.uniformCache[r]=a;const s=this.uniformLocations[r];if(!s){console.warn(`Uniform location for ${r} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(r,n);else if(Array.isArray(n)){let o=null,l=null;if(n[0]!==void 0&&Array.isArray(n[0])){const m=n[0].length;if(n.every(f=>f.length===m))o=n.flat(),l=m;else{console.warn(`All child arrays must be the same length for ${r}`);return}}else o=n,l=o.length;switch(l){case 2:this.gl.uniform2fv(s,o);break;case 3:this.gl.uniform3fv(s,o);break;case 4:this.gl.uniform4fv(s,o);break;case 9:this.gl.uniformMatrix3fv(s,!1,o);break;case 16:this.gl.uniformMatrix4fv(s,!1,o);break;default:console.warn(`Unsupported uniform array length: ${l}`)}}else typeof n=="number"?this.gl.uniform1f(s,n):typeof n=="boolean"?this.gl.uniform1i(s,n?1:0):console.warn(`Unsupported uniform type for ${r}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=t=>{this.currentFrame=t,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(t=1)=>{this.speed=t,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=t=>{this.currentSpeed=t,this.rafId===null&&t!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&t===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(t=pe)=>{this.maxPixelCount=t,this.handleResize()};setMinPixelRatio=(t=2)=>{this.minPixelRatio=t,this.handleResize()};setUniforms=t=>{this.setUniformValues(t),this.providedUniforms={...this.providedUniforms,...t},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(t=>{this.gl.deleteTexture(t)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function ge(e,t,r){const n=e.createShader(t);return n?(e.shaderSource(n,r),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function Ut(e,t,r){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(t=t.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),r=r.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const s=ge(e,e.VERTEX_SHADER,t),o=ge(e,e.FRAGMENT_SHADER,r);if(!s||!o)return null;const l=e.createProgram();return l?(e.attachShader(l,s),e.attachShader(l,o),e.linkProgram(l),e.getProgramParameter(l,e.LINK_STATUS)?(e.detachShader(l,s),e.detachShader(l,o),e.deleteShader(s),e.deleteShader(o),l):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(l)),e.deleteProgram(l),e.deleteShader(s),e.deleteShader(o),null)):null}const Ct=`@layer paper-shaders {
  :where([data-paper-shader]) {
    isolation: isolate;
    position: relative;

    & canvas {
      contain: strict;
      display: block;
      position: absolute;
      inset: 0;
      z-index: -1;
      width: 100%;
      height: 100%;
      border-radius: inherit;
      corner-shape: inherit;
    }
  }
}`;function Tt(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function At(e){const t=visualViewport?.scale??1,r=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=t*r+n,s=outerWidth/a,o=Math.round(100*s);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:s}const Mt={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},Pt={none:0,contain:1,cover:2},zt=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,It=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,Bt=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,ve={maxColorCount:10},Ot=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${ve.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${zt}
${It}
${Bt}

float valueNoise(vec2 st) {
  vec2 i = floor(st);
  vec2 f = fract(st);
  float a = hash21(i);
  float b = hash21(i + vec2(1.0, 0.0));
  float c = hash21(i + vec2(0.0, 1.0));
  float d = hash21(i + vec2(1.0, 1.0));
  vec2 u = f * f * (3.0 - 2.0 * f);
  float x1 = mix(a, b, u.x);
  float x2 = mix(c, d, u.x);
  return mix(x1, x2, u.y);
}

float noise(vec2 n, vec2 seedOffset) {
  return valueNoise(n + seedOffset);
}

vec2 getPosition(int i, float t) {
  float a = float(i) * .37;
  float b = .6 + fract(float(i) / 3.) * .9;
  float c = .8 + fract(float(i + 1) / 4.);

  float x = sin(t * b + a);
  float y = cos(t * c + a * 1.5);

  return .5 + .5 * vec2(x, y);
}

void main() {
  vec2 uv = v_objectUV;
  uv += .5;
  vec2 grainUV = uv * 1000.;

  float grain = noise(grainUV, vec2(0.));
  float mixerGrain = .4 * u_grainMixer * (grain - .5);

  const float firstFrameOffset = 41.5;
  float t = .5 * (u_time + firstFrameOffset);

  float radius = smoothstep(0., 1., length(uv - .5));
  float center = 1. - radius;
  for (float i = 1.; i <= 2.; i++) {
    uv.x += u_distortion * center / i * sin(t + i * .4 * smoothstep(.0, 1., uv.y)) * cos(.2 * t + i * 2.4 * smoothstep(.0, 1., uv.y));
    uv.y += u_distortion * center / i * cos(t + i * 2. * smoothstep(.0, 1., uv.x));
  }

  vec2 uvRotated = uv;
  uvRotated -= vec2(.5);
  float angle = 3. * u_swirl * radius;
  uvRotated = rotate(uvRotated, -angle);
  uvRotated += vec2(.5);

  vec3 color = vec3(0.);
  float opacity = 0.;
  float totalWeight = 0.;

  for (int i = 0; i < ${ve.maxColorCount}; i++) {
    if (i >= int(u_colorsCount)) break;

    vec2 pos = getPosition(i, t) + mixerGrain;
    vec3 colorFraction = u_colors[i].rgb * u_colors[i].a;
    float opacityFraction = u_colors[i].a;

    float dist = length(uvRotated - pos);

    dist = pow(dist, 3.5);
    float weight = 1. / (dist + 1e-3);
    color += colorFraction * weight;
    opacity += opacityFraction * weight;
    totalWeight += weight;
  }

  color /= max(1e-4, totalWeight);
  opacity /= max(1e-4, totalWeight);

  float grainOverlay = valueNoise(rotate(grainUV, 1.) + vec2(3.));
  grainOverlay = mix(grainOverlay, valueNoise(rotate(grainUV, 2.) + vec2(-1.)), .5);
  grainOverlay = pow(grainOverlay, 1.3);

  float grainOverlayV = grainOverlay * 2. - 1.;
  vec3 grainOverlayColor = vec3(step(0., grainOverlayV));
  float grainOverlayStrength = u_grainOverlay * abs(grainOverlayV);
  grainOverlayStrength = pow(grainOverlayStrength, .8);
  color = mix(color, grainOverlayColor, .35 * grainOverlayStrength);

  opacity += .5 * grainOverlayStrength;
  opacity = clamp(opacity, 0., 1.);

  fragColor = vec4(color, opacity);
}
`;function Lt(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:Z;if(typeof e!="string")return Z;let t,r,n,a=1;if(e.startsWith("#"))[t,r,n,a]=Dt(e);else if(e.startsWith("rgb")){const s=kt(e);if(s===null)return Z;[t,r,n,a]=s}else if(e.startsWith("hsl")){const s=Vt(e);if(s===null)return Z;[t,r,n,a]=Ft(s)}else return console.error("Unsupported color format",e),Z;return[ee(t,0,1),ee(r,0,1),ee(n,0,1),ee(a,0,1)]}function Dt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(s=>s+s).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),Z;const t=parseInt(e.slice(0,2),16)/255,r=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[t,r,n,a]}function kt(e){const t=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0")/255,parseInt(t[2]??"0")/255,parseInt(t[3]??"0")/255,t[4]===void 0?1:parseFloat(t[4])]:null}function Vt(e){const t=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0"),parseInt(t[2]??"0"),parseInt(t[3]??"0"),t[4]===void 0?1:parseFloat(t[4])]:null}function Ft(e){const[t,r,n,a]=e,s=t/360,o=r/100,l=n/100;let m,f,d;if(r===0)m=f=d=l;else{const g=(y,j,v)=>(v<0&&(v+=1),v>1&&(v-=1),v<.16666666666666666?y+(j-y)*6*v:v<.5?j:v<.6666666666666666?y+(j-y)*(.6666666666666666-v)*6:y),b=l<.5?l*(1+o):l+o-l*o,S=2*l-b;m=g(S,b,s+1/3),f=g(S,b,s),d=g(S,b,s-1/3)}return[m,f,d,a]}const ee=(e,t,r)=>Math.min(Math.max(e,t),r),Z=[.5,.5,.5,1],qt="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function $t(e){const t=u.useRef(void 0),r=u.useCallback(n=>{const a=e.map(s=>{if(s!=null){if(typeof s=="function"){const o=s,l=o(n);return typeof l=="function"?l:()=>{o(null)}}return s.current=n,()=>{s.current=null}}});return()=>{a.forEach(s=>s?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{t.current&&(t.current(),t.current=void 0),n!=null&&(t.current=r(n))},e)}function xe(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const t=e.naturalWidth/e.naturalHeight;e.width=Math.round(t>1?1024*t:1024),e.height=Math.round(t>1?1024:1024/t)}}async function be(e){const t={},r=[],n=s=>{try{return s.startsWith("/")||new URL(s),!0}catch{return!1}},a=s=>{try{return s.startsWith("/")?!1:new URL(s,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([s,o])=>{if(typeof o=="string"){const l=o||qt;if(!n(l)){console.warn(`Uniform "${s}" has invalid URL "${l}". Skipping image loading.`);return}const m=new Promise((f,d)=>{const g=new Image;a(l)&&(g.crossOrigin="anonymous"),g.onload=()=>{xe(g),t[s]=g,f()},g.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${l}`),d()},g.src=l});r.push(m)}else if(o instanceof HTMLImageElement){const l=o.decode().then(()=>{xe(o),t[s]=o});r.push(l)}else t[s]=o}),await Promise.all(r),t}const Ie=u.forwardRef(function({fragmentShader:t,uniforms:r,webGlContextAttributes:n,speed:a=0,frame:s=0,width:o,height:l,minPixelRatio:m,maxPixelCount:f,mipmaps:d,style:g,...b},S){const[y,j]=u.useState(!1),v=u.useRef(null),w=u.useRef(null),U=u.useRef(n);u.useEffect(()=>((async()=>{const L=await be(r);v.current&&!w.current&&(w.current=new Nt(v.current,t,L,U.current,a,s,m,f,d),j(!0))})(),()=>{w.current?.dispose(),w.current=null}),[t]),u.useEffect(()=>{let F=!1;return(async()=>{const q=await be(r);F||w.current?.setUniforms(q)})(),()=>{F=!0}},[r,y]),u.useEffect(()=>{w.current?.setSpeed(a)},[a,y]),u.useEffect(()=>{w.current?.setMaxPixelCount(f)},[f,y]),u.useEffect(()=>{w.current?.setMinPixelRatio(m)},[m,y]),u.useEffect(()=>{w.current?.setFrame(s)},[s,y]);const H=$t([v,S]);return i.jsx("div",{ref:H,style:o!==void 0||l!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof l=="string"&&isNaN(+l)===!1?+l:l,...g}:g,...b})});Ie.displayName="ShaderMount";function Wt(e,t){if(Object.keys(e).length!==Object.keys(t).length)return!1;for(const r in e){if(r==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(t.colors);if(!n||!a){if(Object.is(e.colors,t.colors)===!1)return!1;continue}if(e.colors?.length!==t.colors?.length||!e.colors?.every((s,o)=>s===t.colors?.[o]))return!1;continue}if(Object.is(e[r],t[r])===!1)return!1}return!0}const N={params:{...Mt,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},Ht=u.memo(function({speed:t=N.params.speed,frame:r=N.params.frame,colors:n=N.params.colors,distortion:a=N.params.distortion,swirl:s=N.params.swirl,grainMixer:o=N.params.grainMixer,grainOverlay:l=N.params.grainOverlay,fit:m=N.params.fit,rotation:f=N.params.rotation,scale:d=N.params.scale,originX:g=N.params.originX,originY:b=N.params.originY,offsetX:S=N.params.offsetX,offsetY:y=N.params.offsetY,worldWidth:j=N.params.worldWidth,worldHeight:v=N.params.worldHeight,...w}){const U={u_colors:n.map(Lt),u_colorsCount:n.length,u_distortion:a,u_swirl:s,u_grainMixer:o,u_grainOverlay:l,u_fit:Pt[m],u_rotation:f,u_scale:d,u_offsetX:S,u_offsetY:y,u_originX:g,u_originY:b,u_worldWidth:j,u_worldHeight:v};return i.jsx(Ie,{...w,speed:t,frame:r,fragmentShader:Ot,uniforms:U})},Wt);function Be(){const[e,t]=u.useState(()=>document.documentElement.dataset.theme==="light"),[r,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const s=new MutationObserver(()=>t(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),l=()=>n(o.matches);return s.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",l),()=>{s.disconnect(),o.removeEventListener("change",l)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return i.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:i.jsx(Ht,{className:"telemetry-live-mesh",colors:a,speed:r?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function ye(e,t,r=""){if(!t)return[];const n=new Map((t.events||[]).map(s=>[s.id,s])),a=Math.min(...(t.events||[]).map(s=>Number(s.seq||0)));return(e||[]).filter(s=>{if(r&&s.kind!==r||V(s))return!1;if(t.snapshotChangeSeq!=null)return Number(s.changeSeq||0)>Number(t.snapshotChangeSeq);const o=n.get(s.id);return o?Number(s.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(s.seq||0)>=a})}function Gt(e,t,r){const n=new Map((e||[]).filter(a=>!V(a)).map(a=>[a.id,a]));for(const a of t||[]){const s=n.get(a.id);(!s||Number(a.changeSeq||0)>=Number(s.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,s)=>Number(s.timestamp||0)-Number(a.timestamp||0)||Number(s.seq||0)-Number(a.seq||0)).slice(0,r)}function Se(e,t){return!t||e?.kind===t}function Xt({recentEvents:e=[],eventKind:t="",resetGeneration:r=0,onRecovered:n,autoRefresh:a=!0,refreshInterval:s=15}={}){const[o,l]=u.useState(1),[m,f]=u.useState(Qe),[d,g]=u.useState(null),[b,S]=u.useState("loading"),[y,j]=u.useState(!0),[v,w]=u.useState(!1),U=u.useRef(!1),H=u.useRef(null),[F,L]=u.useState(null),[q,D]=u.useState(0),$=u.useRef(0),M=u.useRef(null),I=u.useRef(null),re=`${t||"all"}:${m}:${Number(r||0)}`;u.useEffect(()=>{U.current=!1,H.current=null,M.current=null,S("loading"),j(!0),g(null),L(null),l(1)},[re]),u.useEffect(()=>{const _=++$.current,h=new AbortController,T=Math.max(1,Number(o)||1),R=M.current,P=U.current;U.current=!1,j(!P),w(!0),L(null);const E=async(x=!0)=>{const X=await ne.getRuntimeEventPage({page:x?T:1,pageSize:m,kind:t||void 0,snapshotSeq:x?R?.snapshotSeq:void 0,snapshotChangeSeq:x?R?.snapshotChangeSeq:void 0,historyGeneration:x?R?.historyGeneration:void 0,signal:h.signal});return it(X,{page:x?T:1,pageSize:m})};return(async()=>{try{let x;try{x=await E(!0)}catch(X){if(!Ze(X))throw X;x=await E(!1),_===$.current&&(l(1),n?.(Je(X)))}if(_!==$.current)return;if(!x){S("legacy"),g(null);return}M.current=x,g(x),S("page"),x.page!==T&&l(x.page)}catch(x){if(_!==$.current||x?.name==="AbortError")return;if([400,404,405].includes(Number(x?.status))){S("legacy"),g(null);return}L(x?.message||"加载事件历史失败")}finally{_===$.current&&(j(!1),w(!1))}})(),()=>h.abort()},[t,n,o,m,q,r]);const K=u.useCallback(_=>{const h=et(_,d?.totalPages||1);U.current=!1,l(h)},[d?.totalPages]),G=u.useCallback(_=>{U.current=!1;const h=tt(_);M.current=null,f(h),l(1)},[]),B=u.useMemo(()=>b==="page"&&o!==1?[]:e.filter(_=>Se(_,t)&&V(_)),[t,b,o,e]),C=u.useMemo(()=>{const _=b==="page"&&o===1?ye(e,d,t):[],h=b==="page"?Gt(d?.events||[],_,m):e,T=new Set(B.map(R=>R.id));return h.filter(R=>Se(R,t)&&!T.has(R.id)&&!V(R))},[t,B,b,o,m,e,d]),Q=u.useMemo(()=>[...B,...C],[B,C]);return u.useEffect(()=>{if(b!=="page"||o!==1||v||M.current==null)return;const _=ye(e,M.current,t);if(!_.length||I.current!=null)return;const h=Math.max(..._.map(P=>Number(P.changeSeq||0))),T=`${t}:${M.current.snapshotChangeSeq??"legacy"}:${h}`,R=H.current;if(!(R?.signature===T&&Date.now()-R.at<5e3))return I.current=window.setTimeout(()=>{I.current=null,H.current={signature:T,at:Date.now()},U.current=!0,M.current=null,D(P=>P+1)},220),()=>{I.current!=null&&(window.clearTimeout(I.current),I.current=null)}},[t,v,b,o,e]),u.useEffect(()=>{if(!a||o!==1||b!=="page")return;const _=window.setInterval(()=>{document.hidden||v||(U.current=!0,M.current=null,D(h=>h+1))},Math.max(1,Number(s)||15)*1e3);return()=>window.clearInterval(_)},[a,o,b,s,v]),{mode:b,loading:y,error:F,events:Q,liveEvents:B,persistedEvents:C,liveEventCount:B.length,page:o,pageSize:m,totalCount:d?.totalCount??Q.length,totalPages:d?.totalPages??(Q.length?1:0),hasNext:!!d?.hasNext,hasPrevious:!!d?.hasPrevious,snapshotSeq:d?.snapshotSeq??null,snapshotChangeSeq:d?.snapshotChangeSeq??null,historyGeneration:d?.historyGeneration??null,setPage:K,setPageSize:G,retry:()=>{U.current=!1,D(_=>_+1)}}}function Yt(e){const t=e?.totals?.tokens;if(!t||typeof t!="object")return null;const r=Number(t.observedRequests??t.observed_requests);if(!Number.isFinite(r)||r<0)return null;const n=a=>{const s=Number(a);return Number.isFinite(s)&&s>=0?s:null};return{input:n(t.inputTokens??t.input_tokens),output:n(t.outputTokens??t.output_tokens),observed:r}}function _e(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天累计 · ${O(e.observed)} 个请求有用量`:"暂无可用用量":"正在读取今天的用量"}function Oe(e){const t=Re(e);if(!t?.applicable)return null;if(t.name&&t.name!=="unidentified_project"){if(t.source==="workspace_local"&&t.localUser)return`${t.name} 本地(${t.localUser})`;const r=t.source==="client_declared"?"（客户端声明）":"";return`项目: ${t.name}${r}`}return t.label}function Le(e){const t=ze(e,"requestPurpose"),r=typeof t=="object"?t?.kind||t?.type||t?.name:t;return r?r==="standard"?"主请求":r==="websearch"?"WebSearch":r==="webfetch"?"WebFetch":dt(e):null}function Kt(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!Y(e))return"不适用（通知事件）";if(V(e))return"进行中";const t=Y(e);return t==="succeeded"?"成功":t==="failed"?"失败":t==="cancelled"?e.failureKind==="client_disconnected"?"连接中断":"已取消":ht(e).replace(/\s*\([^)]*\)\s*/g,"")}function Qt(e){return String(e?.kind||"").toLowerCase()==="notify"&&!Y(e)?"不适用（通知事件）":V(e)?"进行中":ze(e,"phase")==="completed"||Y(e)?"已完成":mt(e).replace(/\s*\([^)]*\)\s*/g,"")}function De(e){return String(e?.kind||"").toLowerCase()==="notify"&&!Y(e)?"不适用（通知事件）":`${Pe(e)} · ${Qt(e)}`}function ie(e){return[Oe(e),Ae(e.kind),Me(e),ae(e)?`代理: ${ae(e)}`:null,Le(e),e.sourceIP].filter(Boolean).join(" · ")}function ke({event:e,compact:t=!1}){return i.jsx("span",{className:`telemetry-outcome-text ${ut(e)}${t?" is-compact":""}`,children:Kt(e)})}function Ve({event:e,live:t=!1}){const r=ie(e),n=Oe(e),a=e.kind==="notify"?[]:at(e);return i.jsxs("span",{className:t?"telemetry-live-request":"telemetry-primary-cell",children:[i.jsx("span",{className:`mono-cell${t?" telemetry-live-time":" telemetry-event-time"}`,children:Ne(e.timestamp)}),n&&i.jsx("span",{className:"telemetry-event-meta",title:n,children:n}),i.jsx("span",{className:`telemetry-event-meta${t?" telemetry-live-request-summary":""}`,title:r,children:[Ae(e.kind),Me(e),ae(e),Le(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&i.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[i.jsx(W,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function Fe({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{children:e.hookEvent||"通知"}):i.jsxs("span",{className:t?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[i.jsx("strong",{className:"telemetry-event-model",children:Ce(e)}),i.jsx("span",{className:"telemetry-route-meta",children:Te(e)})]})}function qe({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):i.jsxs("span",{className:t?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[i.jsx(ke,{event:e}),i.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:ot(e)}),i.jsx("span",{className:"telemetry-result-detail",title:De(e),children:lt(e)?Pe(e):"HTTP —"})]})}function $e({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-duration-cell",children:"—"});const t=ct(e).ttfbMS>=5e3;return i.jsx("span",{className:"mono-cell telemetry-duration-cell",title:t?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:Ue(e)})}function oe({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const t=bt(e),r=V(e);return i.jsxs("span",{className:`telemetry-usage-cell${t?"":" is-empty"}`,title:r?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[i.jsx("span",{className:"telemetry-usage-tokens",children:yt(e)}),i.jsxs("span",{className:"telemetry-usage-secondary",children:[i.jsx("span",{children:r&&!t?"上游尚未返回":St(e)}),(!r||t)&&i.jsx(_t,{event:e}),r&&t&&i.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function We({event:e,live:t=!1}){const r=Ee(e)||(e.outcome==="failed"?we(e):"");return i.jsxs("span",{className:`${t?"telemetry-live-message":"telemetry-event-message"}${te(e)?" is-warning":Y(e)==="failed"?" is-error":""}`,title:r||void 0,children:[te(e)&&i.jsx("strong",{children:"故障转移 · "}),r||(t?"请求进行中，等待最终结果":"-")]})}function Zt({events:e,selectedEventID:t,onSelect:r}){return e?.length?i.jsxs("section",{className:"telemetry-live-group","aria-label":`${O(e.length)} 个进行中请求`,children:[i.jsx(Be,{}),i.jsxs("div",{className:"telemetry-live-columns",children:[i.jsxs("span",{className:"telemetry-live-heading-cell",children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",O(e.length)]}),i.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),i.jsx("span",{"aria-hidden":"true",children:"结果"}),i.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),i.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",i.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),i.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),i.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=t===n.id;return i.jsx("div",{role:"listitem",children:i.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>r(n.id),children:[i.jsx(Ve,{event:n,live:!0}),i.jsx(Fe,{event:n,live:!0}),i.jsx(qe,{event:n,live:!0}),i.jsx($e,{event:n}),i.jsx(oe,{event:n}),i.jsx(We,{event:n,live:!0})]})},n.id)})})]}):null}function je({events:e,selectedEventID:t,onSelect:r,loading:n,live:a=!1}){return e?.length?i.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&i.jsx(Be,{}),e.map(s=>{const o=Re(s),l=Y(s),m=Ee(s)||(s.outcome==="failed"?we(s):""),f=t===s.id;return i.jsx("article",{role:"listitem",children:i.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${f?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":f,onClick:()=>r(s.id),onKeyDown:d=>{(d.key==="Enter"||d.key===" ")&&(d.preventDefault(),r(s.id))},children:[i.jsxs("div",{className:"telemetry-mobile-heading",children:[i.jsx("span",{className:"mono-cell telemetry-mobile-time",children:Ne(s.timestamp)}),i.jsx(ke,{event:s,compact:!0}),i.jsx("span",{className:"mono-cell telemetry-mobile-duration",style:{color:l==="failed"?"var(--status-critical)":void 0},title:eventTTFBMS(s)>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:Ue(s)})]}),i.jsxs("div",{className:"telemetry-mobile-route",children:[i.jsx("strong",{className:"mono-cell",children:s.kind==="notify"?s.hookEvent||"通知":Ce(s)}),s.kind!=="notify"&&i.jsx("small",{children:Te(s)})]}),i.jsxs("div",{className:"telemetry-mobile-usage",children:[i.jsx("span",{className:"telemetry-mobile-status",children:De(s)}),s.kind!=="notify"&&i.jsx(oe,{event:s})]}),i.jsxs("div",{className:"telemetry-mobile-context",children:[i.jsx("span",{className:"telemetry-mobile-request-summary",title:ie(s),children:ie(s)||o.label}),m&&i.jsxs("p",{className:`telemetry-mobile-message${l==="failed"?" is-error":te(s)?" is-warning":""}`,children:[i.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),te(s)&&i.jsx("strong",{children:"故障转移 · "}),m]})]})]})},s.id)})]}):a?null:i.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:i.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function ai(){const{status:e,runtime:t,runtimeEventDetail:r,config:n,secretStatus:a,toggleProxy:s,connectionsObservation:o,autoRefresh:l,runRefreshInterval:m,loadRuntimeEvent:f,addToast:d}=rt(),[g,b]=u.useState("client"),[S,y]=u.useState("desc"),[j,v]=u.useState(null),[w,U]=u.useState({}),H=(c,p)=>U(z=>z[c]===p?z:{...z,[c]:p}),[F,L]=u.useState(null),q=F?.events||[],D=u.useRef(null),$=u.useRef(l);$.current=l;const[M,I]=u.useState("none"),[,re]=u.useState(0),K=e?.running,G=t?.recentEvents||[],B=u.useMemo(()=>{const c=o?.value?.active;return!Array.isArray(c)||g==="upstream"?[]:c.map(p=>({id:`websocket:${p.requestID}`,requestID:p.requestID,kind:"client",phase:"inFlight",statusCode:101,outcome:null,transport:"websocket",clientModel:p.model||null,effectiveModel:p.model||null,timestamp:p.startedAt,durationMS:0,ttfbMS:null,streamTrace:{websocketTrace:{stage:p.stage,handshakeStatus:p.stage==="relay"?101:null}},failover:!1,detailsOmitted:!1}))},[o,g]),[C,Q]=u.useState(null),_=u.useCallback(c=>d(c,"info"),[d]),h=Xt({recentEvents:G,autoRefresh:l,refreshInterval:m,eventKind:g==="all"?"":g,resetGeneration:t?.resetGeneration,onRecovered:_});u.useEffect(()=>{if(h.loading||M==="none")return;const c=window.setTimeout(()=>I("none"),280);return()=>window.clearTimeout(c)},[h.loading,M]),u.useEffect(()=>{let c=!1;const p=()=>{document.hidden||ne.getRuntimeTrends({range:"today",granularity:"hour"}).then(A=>{if(c)return;const ue=Yt(A);Q(ue?{status:"ready",...ue}:{status:"error",input:null,output:null,observed:0})}).catch(()=>{c||Q(A=>A?.status==="ready"?A:{status:"error",input:null,output:null,observed:0})})};if(p(),!l)return()=>{c=!0};const z=window.setInterval(p,Math.max(15,Number(m)||15)*1e3);return()=>{c=!0,window.clearInterval(z)}},[l,m,t?.resetGeneration]),u.useEffect(()=>{if(![...G,...B].some(p=>V(p)))return;const c=window.setInterval(()=>re(p=>p+1),1e3);return()=>window.clearInterval(c)},[G,B]);const T=u.useMemo(()=>de([...h.liveEvents||[],...B],S),[h.liveEvents,S,B]),R=u.useMemo(()=>de(h.persistedEvents||h.events.filter(c=>!V(c)),S),[h.events,h.persistedEvents,S]),P=u.useMemo(()=>{const c=new Map;for(const p of[...G,...h.events])c.set(p.id,se(c.get(p.id),p));return[...c.values()]},[h.events,G]),E=j,x=E&&(r?.event?.id===E?r.event:P.find(c=>c.id===E)||q.find(c=>c.id===E)||R.find(c=>c.id===E)),X=u.useMemo(()=>{const c=new Map;for(const A of[...q,...P])c.set(A.id,se(c.get(A.id),A));const p=[...c.values()],z=st(p,E);return r?.event?.id!==E?z:z.map(A=>A.id===E?r.event:A)},[P,E,q,r]),k=x?he(x):"",le=Math.max(Number(P.find(c=>c.id===E)?.changeSeq||0),Number(q.find(c=>c.id===E)?.changeSeq||0));u.useEffect(()=>{if(L(null),!k||k==="-")return;const c=Rt({read:z=>ne.getRuntimeRequestChain(k,{signal:z}),onValue:z=>L(A=>fe(z,A?.requestID===k?A.events:[])),enabled:()=>$.current&&!document.hidden});D.current=c;const p=()=>c.synchronize();return document.addEventListener("visibilitychange",p),c.refresh(!0),()=>{c.dispose(),D.current===c&&(D.current=null),document.removeEventListener("visibilitychange",p)}},[k,t?.resetGeneration]),u.useEffect(()=>{D.current?.synchronize()},[l]),u.useEffect(()=>{if(!l||document.hidden||!k)return;const c=P.filter(p=>he(p)===k);L(p=>p?.requestID===k?fe(p,c):p),D.current?.refresh()},[l,P,k]),u.useEffect(()=>{if(!E){f(null);return}f(E,le).catch(c=>{d(`加载事件详情失败: ${c.message}`,"error")})},[d,E,le,f]);const ce=(c,p="内容")=>pt(c,p,d),He=c=>{v(null);const p=Number(c);Number.isFinite(p)&&p!==h.page&&I(p>h.page?"forward":"backward"),h.setPage(c)},Ge=c=>{v(null),I("replace"),h.setPageSize(c)},Xe=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:c=>i.jsx(Ve,{event:c})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:c=>i.jsx(Fe,{event:c})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:c=>i.jsx(qe,{event:c})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:c=>i.jsx($e,{event:c})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(oe,{event:c})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(We,{event:c})}];return i.jsxs("div",{className:"page-stack run-page",children:[i.jsxs("div",{className:"page-header",children:[i.jsxs("div",{className:"page-title-group",children:[i.jsxs("h1",{className:"page-title",children:[i.jsx(W,{name:"play",size:24,style:{color:"var(--primary)"}}),i.jsx("span",{children:"运行"}),i.jsx(ft,{status:e})]}),i.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),i.jsx("div",{className:"page-actions",children:i.jsxs("button",{type:"button",className:`btn ${K?"btn-danger":"btn-primary"}`,onClick:s,children:[i.jsx(W,{name:K?"stop":"play",size:16}),i.jsx("span",{children:K?"停止运行":"启动运行"})]})})]}),e?.lastError&&i.jsxs("div",{className:"run-error-banner",children:[i.jsx(W,{name:"warning",size:20}),i.jsxs("div",{children:[i.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),i.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:i.jsxs("div",{className:"glass-panel run-hero-panel",children:[i.jsxs("div",{className:"run-hero-head",children:[i.jsxs("div",{className:"run-hero-heading",children:[i.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),i.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(K?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),i.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),i.jsxs("dl",{className:"run-detail-grid",children:[i.jsxs("div",{className:"run-detail-listener",children:[i.jsx("dt",{children:"监听地址"}),i.jsxs("dd",{className:"run-listener-row",children:[i.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),i.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>ce(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[i.jsx(W,{name:"copy",size:14}),i.jsx("span",{children:"复制"})]})]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"配置版本"}),i.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"认证状态"}),i.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"运行时长"}),i.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?nt(e.uptimeSeconds*1e3):"-"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"调度策略"}),i.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),i.jsxs("div",{className:"run-metric-grid",children:[i.jsx(J,{label:"输入 Token",value:C?.status==="ready"&&C.observed>0&&C.input!=null?me(C.input):"—",detail:_e(C),icon:"file-down",accent:"var(--primary)"}),i.jsx(J,{label:"输出 Token",value:C?.status==="ready"&&C.observed>0&&C.output!=null?me(C.output):"—",detail:_e(C),icon:"file-up",accent:"var(--status-warning)"}),i.jsx(J,{label:"可调度入口",value:O(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--primary)"}),i.jsx(J,{label:"客户端请求",value:O(t?.clientRequests||0),detail:`端到端 · 成功 ${O(t?.clientSuccesses||0)} / 失败 ${O(t?.clientFailures||0)}`,icon:"move-diagonal",accent:"var(--primary)"}),i.jsx(J,{label:"上游尝试",value:O(t?.upstreamAttempts||0),detail:`含重试 · 故障转移 ${O(t?.failovers||0)}`,icon:"network",accent:"var(--primary)"})]}),i.jsx(Et,{observation:o,autoRefresh:l}),i.jsxs("div",{className:"glass-panel run-events-panel",children:[i.jsxs("div",{className:"panel-header",children:[i.jsxs("div",{className:"panel-title-group",children:[i.jsxs("div",{className:"panel-title",children:[i.jsx(W,{name:"activity",size:18,style:{color:"var(--primary)"}}),i.jsx("span",{children:"最近事件与遥测日志"})]}),i.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),i.jsxs("div",{className:"telemetry-toolbar",children:[i.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(c=>i.jsx("button",{type:"button",className:`telemetry-filter-button${g===c.id?" active":""}`,onClick:()=>{v(null),I("replace"),b(c.id)},"aria-pressed":g===c.id,children:c.label},c.id))}),h.mode!=="page"&&i.jsxs("span",{className:"panel-hint",children:[O(R.length)," 条持久事件"]}),i.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{y(c=>c==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${S==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[i.jsx(W,{name:"sort",size:14}),i.jsxs("span",{children:[h.mode==="page"?"本页":"",S==="desc"?"最新优先":"最早优先"]})]})]})]}),i.jsxs("div",{className:"panel-body",style:{padding:0},children:[h.error&&i.jsxs("div",{className:"event-page-error",role:"alert",children:[i.jsxs("span",{children:["读取事件历史失败：",h.error]}),i.jsx("button",{type:"button",className:"btn btn-secondary",onClick:h.retry,children:"重试"})]}),i.jsxs("div",{className:`event-page-stage event-page-stage-${h.loading?"loading":"ready"}`,"data-page":h.page,"data-page-direction":M,"aria-busy":h.loading,children:[h.mode==="page"?i.jsx(vt,{compact:!0,page:h.page,pageSize:h.pageSize,totalCount:h.totalCount,totalPages:h.totalPages,itemCount:R.length,liveItemCount:h.liveEventCount,loading:h.loading,onPageChange:He,onPageSizeChange:Ge,ariaLabel:"持久事件分页"}):null,i.jsxs("div",{className:"event-page-content",children:[T.length>0&&i.jsxs("div",{className:"telemetry-live-section",children:[i.jsx("div",{className:"telemetry-live-desktop",children:i.jsx(Zt,{events:T,selectedEventID:x?.id,onSelect:v})}),i.jsxs("div",{className:"telemetry-live-mobile",children:[i.jsxs("div",{className:"telemetry-live-heading",children:[i.jsxs("span",{children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",O(T.length)]}),i.jsx("small",{children:"上游暂计"})]}),i.jsx(je,{events:T,selectedEventID:x?.id,onSelect:v,live:!0})]})]}),i.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[i.jsx("div",{className:"telemetry-desktop-table",children:i.jsx(gt,{className:"telemetry-table",columns:Xe,data:R,keyField:"id",ariaLabel:h.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:S,onSortChange:(c,p)=>y(p),onRowClick:c=>v(c.id),activeRowKey:x?.id,emptyText:h.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),i.jsx(je,{events:R,selectedEventID:x?.id,onSelect:v,loading:h.loading})]}),!x&&(R.length>0||T.length>0)&&i.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"})]})]}),h.mode==="legacy"?i.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),x&&i.jsx(xt,{event:x,chain:X,chainTruncated:F?.truncated,onSelect:v,requestSummary:ie(x),expanded:w,onToggle:H,onCopy:ce,loading:r?.event?.id!==x.id&&x.detailsOmitted===!0})]})]})}export{ai as RunPage};
