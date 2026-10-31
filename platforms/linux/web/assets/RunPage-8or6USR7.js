import{m as ie,j as i,I as W,r as u,v as Ye,c as Ke,e as F,a as Qe,i as Ze,b as Je,d as et,p as tt,f as re,n as it,u as rt,o as ce,g as st,h as ue,k as nt,l as de,q as I,s as Se,t as G,w as je,x as Re,y as Ee,z as we,A as Ne,B as Ue,C as Ce,D as J,E as Te,F as Ae,G as at,H as ot,J as lt,K as Pe,L as ct,M as ze,N as ut,O as dt,P as ht}from"./index-FN7sN93u.js";import{S as mt,c as ft}from"./clipboard-CvAu6I_e.js";import{D as pt}from"./DataTable-DezFD_M5.js";import{P as gt}from"./outputSpeed-B9ZC5Rn3.js";import{E as vt,e as se,a as xt,b as bt,c as yt,C as _t}from"./EventInspector-Vhm4ivIx.js";import"./requestPolicy-CjaOnvV0.js";function he(e,t=[],s=e?.requestID){const n=new Map;for(const r of[...e?.events||[],...t])r.requestID===s&&n.set(r.id,ie(n.get(r.id),r));const a=[...n.values()].sort((r,o)=>Number(r.seq||0)-Number(o.seq||0));return{...e,requestID:s,events:a.slice(0,512),truncated:!!(e?.truncated||a.length>512)}}function St(e){return e?.events?.some(t=>t.kind==="client"&&["inFlight","in_flight"].includes(t.phase))||!1}function jt({read:e,onValue:t,enabled:s,interval:n=1e3,schedule:a=setTimeout,unschedule:r=clearTimeout,now:o=Date.now}){let l=!1,h=!0,m,d,g=-1/0;function S(b=!1){if(l||d||!b&&(!h||!s())||m!=null)return;const j=b?0:Math.max(0,n-(o()-g));m=a(()=>{m=void 0,y(b)},j)}async function y(b){if(l||d||!b&&!s())return;const j=new AbortController;d=j,g=o();try{const p=await e(j.signal);if(l||j.signal.aborted||!b&&!s())return;h=St(p),t(p)}catch(p){[401,403,404].includes(p?.status)&&(h=!1)}finally{d=void 0,!l&&h&&s()&&(m=a(()=>{m=void 0,y(!1)},n))}}return{refresh:S,synchronize(){s()?S():(m!=null&&r(m),m=void 0)},dispose(){l=!0,m!=null&&r(m),d?.abort()}}}function Q({label:e,value:t,detail:s,icon:n,accent:a="var(--primary)"}){return i.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[i.jsxs("div",{className:"metric-head",children:[i.jsx("span",{className:"metric-label",children:e}),n&&i.jsx("div",{className:"metric-icon-box",children:i.jsx(W,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),i.jsx("div",{className:"metric-value",children:t}),s&&i.jsx("div",{className:"metric-detail",children:s})]})}function Rt({observation:e,autoRefresh:t}){const s=u.useRef(null);s.current=Ye(s.current,e,t);const n=Ke(s.current);return i.jsxs("section",{className:"responses-connections-panel","aria-label":"当前 Responses WebSocket",children:[i.jsxs("div",{className:"responses-connections-heading",children:[i.jsx("strong",{children:"当前 Responses WebSocket"}),!t&&i.jsx("span",{children:"已暂停刷新"}),n.error&&i.jsx("span",{className:"responses-connections-error",role:"status",children:n.error})]}),n.message?i.jsx("p",{children:n.message}):i.jsx("dl",{className:"responses-connections-values",children:n.rows.map(([a,r])=>i.jsxs("div",{children:[i.jsx("dt",{children:a}),i.jsx("dd",{children:r})]},a))}),i.jsx("p",{children:"当前进程内的连接快照；已进入转发不代表模型正在生成。"})]})}const Et=`#version 300 es
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
}`,me=1920*1080*4;let wt=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=Ct();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(t,s,n,a,r=0,o=0,l=2,h=me,m=[]){if(t?.nodeType===1)this.parentElement=t;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=t.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const S=this.ownerDocument.createElement("style");S.innerHTML=Ut,S.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(S)}const d=this.ownerDocument.createElement("canvas");this.canvasElement=d,this.parentElement.prepend(d),this.fragmentShader=s,this.providedUniforms=n,this.mipmaps=m,this.currentFrame=o,this.minPixelRatio=l,this.maxPixelCount=h;const g=d.getContext("webgl2",a);if(!g)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=g,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(r),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const t=Nt(this.gl,Et,this.fragmentShader);t&&(this.program=t)};setupPositionAttribute=()=>{const t=this.gl.getAttribLocation(this.program,"a_position"),s=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,s);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(t),this.gl.vertexAttribPointer(t,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const t={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([s,n])=>{if(t[s]=this.gl.getUniformLocation(this.program,s),n instanceof HTMLImageElement){const a=`${s}AspectRatio`;t[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=t};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([t])=>{if(t?.borderBoxSize[0]){const s=t.devicePixelContentBoxSize?.[0];s!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=s.inlineSize,this.parentDevicePixelHeight=s.blockSize),this.parentWidth=t.borderBoxSize[0].inlineSize,this.parentHeight=t.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const t=this.ownerDocument.defaultView;t?.IntersectionObserver&&(this.intersectionObserver=new t.IntersectionObserver(([s])=>{this.isInViewport=s?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let t=0,s=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const d=Math.max(1,this.minPixelRatio/n);t=this.parentDevicePixelWidth*d*a,s=this.parentDevicePixelHeight*d*a}else{let d=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const g=Tt(this.ownerDocument);d*=Math.max(1,g)}t=Math.round(this.parentWidth)*d,s=Math.round(this.parentHeight)*d}const r=Math.sqrt(this.maxPixelCount)/Math.sqrt(t*s),o=Math.min(1,r),l=Math.round(t*o),h=Math.round(s*o),m=l/Math.round(this.parentWidth);(this.canvasElement.width!==l||this.canvasElement.height!==h||this.renderScale!==m)&&(this.renderScale=m,this.canvasElement.width=l,this.canvasElement.height=h,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=t=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const s=t-this.lastRenderTime;this.lastRenderTime=t,this.currentSpeed!==0&&(this.currentFrame+=s*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(t,s)=>{if(!s.complete||s.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${t} must be fully loaded`);const n=this.textures.get(t);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(t)||this.textureUnitMap.set(t,this.textureUnitMap.size);const a=this.textureUnitMap.get(t);this.gl.activeTexture(this.gl.TEXTURE0+a);const r=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,r),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,s),this.mipmaps.includes(t)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||r===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(t,r);const l=this.uniformLocations[t];if(l){this.gl.uniform1i(l,a);const h=`${t}AspectRatio`,m=this.uniformLocations[h];if(m){const d=s.naturalWidth/s.naturalHeight;this.gl.uniform1f(m,d)}}};areUniformValuesEqual=(t,s)=>t===s?!0:Array.isArray(t)&&Array.isArray(s)&&t.length===s.length?t.every((n,a)=>this.areUniformValuesEqual(n,s[a])):!1;setUniformValues=t=>{this.gl.useProgram(this.program),Object.entries(t).forEach(([s,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[s],a))return;this.uniformCache[s]=a;const r=this.uniformLocations[s];if(!r){console.warn(`Uniform location for ${s} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(s,n);else if(Array.isArray(n)){let o=null,l=null;if(n[0]!==void 0&&Array.isArray(n[0])){const h=n[0].length;if(n.every(m=>m.length===h))o=n.flat(),l=h;else{console.warn(`All child arrays must be the same length for ${s}`);return}}else o=n,l=o.length;switch(l){case 2:this.gl.uniform2fv(r,o);break;case 3:this.gl.uniform3fv(r,o);break;case 4:this.gl.uniform4fv(r,o);break;case 9:this.gl.uniformMatrix3fv(r,!1,o);break;case 16:this.gl.uniformMatrix4fv(r,!1,o);break;default:console.warn(`Unsupported uniform array length: ${l}`)}}else typeof n=="number"?this.gl.uniform1f(r,n):typeof n=="boolean"?this.gl.uniform1i(r,n?1:0):console.warn(`Unsupported uniform type for ${s}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=t=>{this.currentFrame=t,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(t=1)=>{this.speed=t,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=t=>{this.currentSpeed=t,this.rafId===null&&t!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&t===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(t=me)=>{this.maxPixelCount=t,this.handleResize()};setMinPixelRatio=(t=2)=>{this.minPixelRatio=t,this.handleResize()};setUniforms=t=>{this.setUniformValues(t),this.providedUniforms={...this.providedUniforms,...t},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(t=>{this.gl.deleteTexture(t)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function fe(e,t,s){const n=e.createShader(t);return n?(e.shaderSource(n,s),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function Nt(e,t,s){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(t=t.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),s=s.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const r=fe(e,e.VERTEX_SHADER,t),o=fe(e,e.FRAGMENT_SHADER,s);if(!r||!o)return null;const l=e.createProgram();return l?(e.attachShader(l,r),e.attachShader(l,o),e.linkProgram(l),e.getProgramParameter(l,e.LINK_STATUS)?(e.detachShader(l,r),e.detachShader(l,o),e.deleteShader(r),e.deleteShader(o),l):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(l)),e.deleteProgram(l),e.deleteShader(r),e.deleteShader(o),null)):null}const Ut=`@layer paper-shaders {
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
}`;function Ct(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function Tt(e){const t=visualViewport?.scale??1,s=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=t*s+n,r=outerWidth/a,o=Math.round(100*r);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:r}const At={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},Pt={none:0,contain:1,cover:2},zt=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,Mt=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,Bt=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,pe={maxColorCount:10},It=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${pe.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${zt}
${Mt}
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

  for (int i = 0; i < ${pe.maxColorCount}; i++) {
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
`;function Ot(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:Y;if(typeof e!="string")return Y;let t,s,n,a=1;if(e.startsWith("#"))[t,s,n,a]=Lt(e);else if(e.startsWith("rgb")){const r=Dt(e);if(r===null)return Y;[t,s,n,a]=r}else if(e.startsWith("hsl")){const r=Vt(e);if(r===null)return Y;[t,s,n,a]=kt(r)}else return console.error("Unsupported color format",e),Y;return[Z(t,0,1),Z(s,0,1),Z(n,0,1),Z(a,0,1)]}function Lt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(r=>r+r).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),Y;const t=parseInt(e.slice(0,2),16)/255,s=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[t,s,n,a]}function Dt(e){const t=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0")/255,parseInt(t[2]??"0")/255,parseInt(t[3]??"0")/255,t[4]===void 0?1:parseFloat(t[4])]:null}function Vt(e){const t=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0"),parseInt(t[2]??"0"),parseInt(t[3]??"0"),t[4]===void 0?1:parseFloat(t[4])]:null}function kt(e){const[t,s,n,a]=e,r=t/360,o=s/100,l=n/100;let h,m,d;if(s===0)h=m=d=l;else{const g=(b,j,p)=>(p<0&&(p+=1),p>1&&(p-=1),p<.16666666666666666?b+(j-b)*6*p:p<.5?j:p<.6666666666666666?b+(j-b)*(.6666666666666666-p)*6:b),S=l<.5?l*(1+o):l+o-l*o,y=2*l-S;h=g(y,S,r+1/3),m=g(y,S,r),d=g(y,S,r-1/3)}return[h,m,d,a]}const Z=(e,t,s)=>Math.min(Math.max(e,t),s),Y=[.5,.5,.5,1],Ft="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function qt(e){const t=u.useRef(void 0),s=u.useCallback(n=>{const a=e.map(r=>{if(r!=null){if(typeof r=="function"){const o=r,l=o(n);return typeof l=="function"?l:()=>{o(null)}}return r.current=n,()=>{r.current=null}}});return()=>{a.forEach(r=>r?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{t.current&&(t.current(),t.current=void 0),n!=null&&(t.current=s(n))},e)}function ge(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const t=e.naturalWidth/e.naturalHeight;e.width=Math.round(t>1?1024*t:1024),e.height=Math.round(t>1?1024:1024/t)}}async function ve(e){const t={},s=[],n=r=>{try{return r.startsWith("/")||new URL(r),!0}catch{return!1}},a=r=>{try{return r.startsWith("/")?!1:new URL(r,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([r,o])=>{if(typeof o=="string"){const l=o||Ft;if(!n(l)){console.warn(`Uniform "${r}" has invalid URL "${l}". Skipping image loading.`);return}const h=new Promise((m,d)=>{const g=new Image;a(l)&&(g.crossOrigin="anonymous"),g.onload=()=>{ge(g),t[r]=g,m()},g.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${l}`),d()},g.src=l});s.push(h)}else if(o instanceof HTMLImageElement){const l=o.decode().then(()=>{ge(o),t[r]=o});s.push(l)}else t[r]=o}),await Promise.all(s),t}const Me=u.forwardRef(function({fragmentShader:t,uniforms:s,webGlContextAttributes:n,speed:a=0,frame:r=0,width:o,height:l,minPixelRatio:h,maxPixelCount:m,mipmaps:d,style:g,...S},y){const[b,j]=u.useState(!1),p=u.useRef(null),w=u.useRef(null),$=u.useRef(n);u.useEffect(()=>((async()=>{const O=await ve(s);p.current&&!w.current&&(w.current=new wt(p.current,t,O,$.current,a,r,h,m,d),j(!0))})(),()=>{w.current?.dispose(),w.current=null}),[t]),u.useEffect(()=>{let q=!1;return(async()=>{const A=await ve(s);q||w.current?.setUniforms(A)})(),()=>{q=!0}},[s,b]),u.useEffect(()=>{w.current?.setSpeed(a)},[a,b]),u.useEffect(()=>{w.current?.setMaxPixelCount(m)},[m,b]),u.useEffect(()=>{w.current?.setMinPixelRatio(h)},[h,b]),u.useEffect(()=>{w.current?.setFrame(r)},[r,b]);const H=qt([p,y]);return i.jsx("div",{ref:H,style:o!==void 0||l!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof l=="string"&&isNaN(+l)===!1?+l:l,...g}:g,...S})});Me.displayName="ShaderMount";function Wt(e,t){if(Object.keys(e).length!==Object.keys(t).length)return!1;for(const s in e){if(s==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(t.colors);if(!n||!a){if(Object.is(e.colors,t.colors)===!1)return!1;continue}if(e.colors?.length!==t.colors?.length||!e.colors?.every((r,o)=>r===t.colors?.[o]))return!1;continue}if(Object.is(e[s],t[s])===!1)return!1}return!0}const N={params:{...At,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},$t=u.memo(function({speed:t=N.params.speed,frame:s=N.params.frame,colors:n=N.params.colors,distortion:a=N.params.distortion,swirl:r=N.params.swirl,grainMixer:o=N.params.grainMixer,grainOverlay:l=N.params.grainOverlay,fit:h=N.params.fit,rotation:m=N.params.rotation,scale:d=N.params.scale,originX:g=N.params.originX,originY:S=N.params.originY,offsetX:y=N.params.offsetX,offsetY:b=N.params.offsetY,worldWidth:j=N.params.worldWidth,worldHeight:p=N.params.worldHeight,...w}){const $={u_colors:n.map(Ot),u_colorsCount:n.length,u_distortion:a,u_swirl:r,u_grainMixer:o,u_grainOverlay:l,u_fit:Pt[h],u_rotation:m,u_scale:d,u_offsetX:y,u_offsetY:b,u_originX:g,u_originY:S,u_worldWidth:j,u_worldHeight:p};return i.jsx(Me,{...w,speed:t,frame:s,fragmentShader:It,uniforms:$})},Wt);function Be(){const[e,t]=u.useState(()=>document.documentElement.dataset.theme==="light"),[s,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const r=new MutationObserver(()=>t(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),l=()=>n(o.matches);return r.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",l),()=>{r.disconnect(),o.removeEventListener("change",l)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return i.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:i.jsx($t,{className:"telemetry-live-mesh",colors:a,speed:s?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function xe(e,t,s=""){if(!t)return[];const n=new Map((t.events||[]).map(r=>[r.id,r])),a=Math.min(...(t.events||[]).map(r=>Number(r.seq||0)));return(e||[]).filter(r=>{if(s&&r.kind!==s||F(r))return!1;if(t.snapshotChangeSeq!=null)return Number(r.changeSeq||0)>Number(t.snapshotChangeSeq);const o=n.get(r.id);return o?Number(r.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(r.seq||0)>=a})}function Ht(e,t,s){const n=new Map((e||[]).filter(a=>!F(a)).map(a=>[a.id,a]));for(const a of t||[]){const r=n.get(a.id);(!r||Number(a.changeSeq||0)>=Number(r.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,r)=>Number(r.seq||0)-Number(a.seq||0)).slice(0,s)}function be(e,t){return!t||e?.kind===t}function Gt({recentEvents:e=[],eventKind:t="",resetGeneration:s=0,onRecovered:n}={}){const[a,r]=u.useState(1),[o,l]=u.useState(Qe),[h,m]=u.useState(null),[d,g]=u.useState("loading"),[S,y]=u.useState(!0),[b,j]=u.useState(!1),p=u.useRef(!1),w=u.useRef(null),[$,H]=u.useState(null),[q,O]=u.useState(0),A=u.useRef(0),U=u.useRef(null),L=u.useRef(null),K=`${t||"all"}:${o}:${Number(s||0)}`;u.useEffect(()=>{p.current=!1,w.current=null,U.current=null,g("loading"),y(!0),m(null),H(null),r(1)},[K]),u.useEffect(()=>{const _=++A.current,P=new AbortController,f=Math.max(1,Number(a)||1),R=U.current,z=p.current;p.current=!1,y(!z),j(!0),H(null);const V=async(v=!0)=>{const E=await re.getRuntimeEventPage({page:v?f:1,pageSize:o,kind:t||void 0,snapshotSeq:v?R?.snapshotSeq:void 0,snapshotChangeSeq:v?R?.snapshotChangeSeq:void 0,historyGeneration:v?R?.historyGeneration:void 0,signal:P.signal});return it(E,{page:v?f:1,pageSize:o})};return(async()=>{try{let v;try{v=await V(!0)}catch(E){if(!Ze(E))throw E;v=await V(!1),_===A.current&&(r(1),n?.(Je(E)))}if(_!==A.current)return;if(!v){g("legacy"),m(null);return}U.current=v,m(v),g("page"),v.page!==f&&r(v.page)}catch(v){if(_!==A.current||v?.name==="AbortError")return;if([400,404,405].includes(Number(v?.status))){g("legacy"),m(null);return}H(v?.message||"加载事件历史失败")}finally{_===A.current&&(y(!1),j(!1))}})(),()=>P.abort()},[t,n,a,o,q,s]);const X=u.useCallback(_=>{const P=et(_,h?.totalPages||1);p.current=!1,r(P)},[h?.totalPages]),te=u.useCallback(_=>{p.current=!1;const P=tt(_);U.current=null,l(P),r(1)},[]),B=u.useMemo(()=>d==="page"&&a!==1?[]:e.filter(_=>be(_,t)&&F(_)),[t,d,a,e]),D=u.useMemo(()=>{const _=d==="page"&&a===1?xe(e,h,t):[],P=d==="page"?Ht(h?.events||[],_,o):e,f=new Set(B.map(R=>R.id));return P.filter(R=>be(R,t)&&!f.has(R.id)&&!F(R))},[t,B,d,a,o,e,h]),C=u.useMemo(()=>[...B,...D],[B,D]);return u.useEffect(()=>{if(d!=="page"||a!==1||b||U.current==null)return;const _=xe(e,U.current,t);if(!_.length||L.current!=null)return;const P=Math.max(..._.map(z=>Number(z.changeSeq||0))),f=`${t}:${U.current.snapshotChangeSeq??"legacy"}:${P}`,R=w.current;if(!(R?.signature===f&&Date.now()-R.at<5e3))return L.current=window.setTimeout(()=>{L.current=null,w.current={signature:f,at:Date.now()},p.current=!0,U.current=null,O(z=>z+1)},220),()=>{L.current!=null&&(window.clearTimeout(L.current),L.current=null)}},[t,b,d,a,e]),{mode:d,loading:S,error:$,events:C,liveEvents:B,persistedEvents:D,liveEventCount:B.length,page:a,pageSize:o,totalCount:h?.totalCount??C.length,totalPages:h?.totalPages??(C.length?1:0),hasNext:!!h?.hasNext,hasPrevious:!!h?.hasPrevious,snapshotSeq:h?.snapshotSeq??null,snapshotChangeSeq:h?.snapshotChangeSeq??null,historyGeneration:h?.historyGeneration??null,setPage:X,setPageSize:te,retry:()=>{p.current=!1,O(_=>_+1)}}}function Xt(e){const t=e?.totals?.tokens;if(!t||typeof t!="object")return null;const s=Number(t.observedRequests??t.observed_requests);if(!Number.isFinite(s)||s<0)return null;const n=a=>{const r=Number(a);return Number.isFinite(r)&&r>=0?r:null};return{input:n(t.inputTokens??t.input_tokens),output:n(t.outputTokens??t.output_tokens),observed:s}}function ye(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天累计 · ${I(e.observed)} 个请求有用量`:"暂无可用用量":"正在读取今天的用量"}function Ie(e){const t=Se(e);if(!t?.applicable)return null;if(t.name&&t.name!=="unidentified_project"){if(t.source==="workspace_local"&&t.localUser)return`${t.name} 本地(${t.localUser})`;const s=t.source==="client_declared"?"（客户端声明）":"";return`项目: ${t.name}${s}`}return t.label}function Oe(e){const t=ze(e,"requestPurpose"),s=typeof t=="object"?t?.kind||t?.type||t?.name:t;return s?s==="standard"?"主请求":s==="websearch"?"WebSearch":s==="webfetch"?"WebFetch":ut(e):null}function Yt(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!G(e))return"不适用（通知事件）";if(F(e))return"进行中";const t=G(e);return t==="succeeded"?"成功":t==="failed"?"失败":t==="cancelled"?e.failureKind==="client_disconnected"?"连接中断":"已取消":dt(e).replace(/\s*\([^)]*\)\s*/g,"")}function Kt(e){return String(e?.kind||"").toLowerCase()==="notify"&&!G(e)?"不适用（通知事件）":F(e)?"进行中":ze(e,"phase")==="completed"||G(e)?"已完成":ht(e).replace(/\s*\([^)]*\)\s*/g,"")}function Le(e){return String(e?.kind||"").toLowerCase()==="notify"&&!G(e)?"不适用（通知事件）":`${Pe(e)} · ${Kt(e)}`}function ee(e){return[Ie(e),Te(e.kind),Ae(e),se(e)?`代理: ${se(e)}`:null,Oe(e),e.sourceIP].filter(Boolean).join(" · ")}function De({event:e,compact:t=!1}){return i.jsx("span",{className:`telemetry-outcome-text ${ct(e)}${t?" is-compact":""}`,children:Yt(e)})}function Ve({event:e,live:t=!1}){const s=ee(e),n=Ie(e),a=e.kind==="notify"?[]:at(e);return i.jsxs("span",{className:t?"telemetry-live-request":"telemetry-primary-cell",children:[i.jsx("span",{className:`mono-cell${t?" telemetry-live-time":" telemetry-event-time"}`,children:Ee(e.timestamp)}),n&&i.jsx("span",{className:"telemetry-event-meta",title:n,children:n}),i.jsx("span",{className:`telemetry-event-meta${t?" telemetry-live-request-summary":""}`,title:s,children:[Te(e.kind),Ae(e),se(e),Oe(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&i.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[i.jsx(W,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function ke({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{children:e.hookEvent||"通知"}):i.jsxs("span",{className:t?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[i.jsx("strong",{className:"telemetry-event-model",children:Ue(e)}),i.jsx("span",{className:"telemetry-route-meta",children:Ce(e)})]})}function Fe({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):i.jsxs("span",{className:t?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[i.jsx(De,{event:e}),i.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:ot(e)}),i.jsx("span",{className:"telemetry-result-detail",title:Le(e),children:lt(e)?Pe(e):"HTTP —"})]})}function qe({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-duration-cell",children:"—"});const t=Ne(e)>=5e3;return i.jsx("span",{className:"mono-cell telemetry-duration-cell",title:t?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:we(e)})}function ne({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const t=xt(e),s=F(e);return i.jsxs("span",{className:`telemetry-usage-cell${t?"":" is-empty"}`,title:s?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[i.jsx("span",{className:"telemetry-usage-tokens",children:bt(e)}),i.jsxs("span",{className:"telemetry-usage-secondary",children:[i.jsx("span",{children:s&&!t?"上游尚未返回":yt(e)}),(!s||t)&&i.jsx(_t,{event:e}),s&&t&&i.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function We({event:e,live:t=!1}){const s=je(e)||(e.outcome==="failed"?Re(e):"");return i.jsxs("span",{className:`${t?"telemetry-live-message":"telemetry-event-message"}${J(e)?" is-warning":G(e)==="failed"?" is-error":""}`,title:s||void 0,children:[J(e)&&i.jsx("strong",{children:"故障转移 · "}),s||(t?"请求进行中，等待最终结果":"-")]})}function Qt({events:e,selectedEventID:t,onSelect:s}){return e?.length?i.jsxs("section",{className:"telemetry-live-group","aria-label":`${I(e.length)} 个进行中请求`,children:[i.jsx(Be,{}),i.jsxs("div",{className:"telemetry-live-columns",children:[i.jsxs("span",{className:"telemetry-live-heading-cell",children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",I(e.length)]}),i.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),i.jsx("span",{"aria-hidden":"true",children:"结果"}),i.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),i.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",i.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),i.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),i.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=t===n.id;return i.jsx("div",{role:"listitem",children:i.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>s(n.id),children:[i.jsx(Ve,{event:n,live:!0}),i.jsx(ke,{event:n,live:!0}),i.jsx(Fe,{event:n,live:!0}),i.jsx(qe,{event:n}),i.jsx(ne,{event:n}),i.jsx(We,{event:n,live:!0})]})},n.id)})})]}):null}function _e({events:e,selectedEventID:t,onSelect:s,loading:n,live:a=!1}){return e?.length?i.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&i.jsx(Be,{}),e.map(r=>{const o=Se(r),l=G(r),h=je(r)||(r.outcome==="failed"?Re(r):""),m=t===r.id;return i.jsx("article",{role:"listitem",children:i.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${m?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":m,onClick:()=>s(r.id),onKeyDown:d=>{(d.key==="Enter"||d.key===" ")&&(d.preventDefault(),s(r.id))},children:[i.jsxs("div",{className:"telemetry-mobile-heading",children:[i.jsx("span",{className:"mono-cell telemetry-mobile-time",children:Ee(r.timestamp)}),i.jsx(De,{event:r,compact:!0}),i.jsx("span",{className:"mono-cell telemetry-mobile-duration",style:{color:l==="failed"?"var(--status-critical)":void 0},title:Ne(r)>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:we(r)})]}),i.jsxs("div",{className:"telemetry-mobile-route",children:[i.jsx("strong",{className:"mono-cell",children:r.kind==="notify"?r.hookEvent||"通知":Ue(r)}),r.kind!=="notify"&&i.jsx("small",{children:Ce(r)})]}),i.jsxs("div",{className:"telemetry-mobile-usage",children:[i.jsx("span",{className:"telemetry-mobile-status",children:Le(r)}),r.kind!=="notify"&&i.jsx(ne,{event:r})]}),i.jsxs("div",{className:"telemetry-mobile-context",children:[i.jsx("span",{className:"telemetry-mobile-request-summary",title:ee(r),children:ee(r)||o.label}),h&&i.jsxs("p",{className:`telemetry-mobile-message${l==="failed"?" is-error":J(r)?" is-warning":""}`,children:[i.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),J(r)&&i.jsx("strong",{children:"故障转移 · "}),h]})]})]})},r.id)})]}):a?null:i.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:i.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function ni(){const{status:e,runtime:t,runtimeEventDetail:s,config:n,secretStatus:a,toggleProxy:r,connectionsObservation:o,autoRefresh:l,runRefreshInterval:h,loadRuntimeEvent:m,addToast:d}=rt(),[g,S]=u.useState("client"),[y,b]=u.useState("desc"),[j,p]=u.useState(null),[w,$]=u.useState({}),H=(c,x)=>$(M=>M[c]===x?M:{...M,[c]:x}),[q,O]=u.useState(null),A=q?.events||[],U=u.useRef(null),L=u.useRef(l);L.current=l;const[K,X]=u.useState("none"),[,te]=u.useState(0),B=e?.running,D=t?.recentEvents||[],[C,_]=u.useState(null),P=u.useCallback(c=>d(c,"info"),[d]),f=Gt({recentEvents:D,eventKind:g==="all"?"":g,resetGeneration:t?.resetGeneration,onRecovered:P});u.useEffect(()=>{if(f.loading||K==="none")return;const c=window.setTimeout(()=>X("none"),280);return()=>window.clearTimeout(c)},[f.loading,K]),u.useEffect(()=>{let c=!1;const x=()=>{document.hidden||re.getRuntimeTrends({range:"today",granularity:"hour"}).then(T=>{if(c)return;const le=Xt(T);_(le?{status:"ready",...le}:{status:"error",input:null,output:null,observed:0})}).catch(()=>{c||_(T=>T?.status==="ready"?T:{status:"error",input:null,output:null,observed:0})})};if(x(),!l)return()=>{c=!0};const M=window.setInterval(x,Math.max(15,Number(h)||15)*1e3);return()=>{c=!0,window.clearInterval(M)}},[l,h,t?.resetGeneration]),u.useEffect(()=>{if(!D.some(x=>F(x)))return;const c=window.setInterval(()=>te(x=>x+1),1e3);return()=>window.clearInterval(c)},[D]);const R=u.useMemo(()=>ce(f.liveEvents||[],y),[f.liveEvents,y]),z=u.useMemo(()=>ce(f.persistedEvents||f.events.filter(c=>!F(c)),y),[f.events,f.persistedEvents,y]),V=u.useMemo(()=>{const c=new Map;for(const x of[...D,...f.events])c.set(x.id,ie(c.get(x.id),x));return[...c.values()]},[f.events,D]),v=j,E=v&&(s?.event?.id===v?s.event:V.find(c=>c.id===v)||A.find(c=>c.id===v)||z.find(c=>c.id===v)),$e=u.useMemo(()=>{const c=new Map;for(const T of[...A,...V])c.set(T.id,ie(c.get(T.id),T));const x=[...c.values()],M=st(x,v);return s?.event?.id!==v?M:M.map(T=>T.id===v?s.event:T)},[V,v,A,s]),k=E?ue(E):"",ae=Math.max(Number(V.find(c=>c.id===v)?.changeSeq||0),Number(A.find(c=>c.id===v)?.changeSeq||0));u.useEffect(()=>{if(O(null),!k||k==="-")return;const c=jt({read:M=>re.getRuntimeRequestChain(k,{signal:M}),onValue:M=>O(T=>he(M,T?.requestID===k?T.events:[])),enabled:()=>L.current&&!document.hidden});U.current=c;const x=()=>c.synchronize();return document.addEventListener("visibilitychange",x),c.refresh(!0),()=>{c.dispose(),U.current===c&&(U.current=null),document.removeEventListener("visibilitychange",x)}},[k,t?.resetGeneration]),u.useEffect(()=>{U.current?.synchronize()},[l]),u.useEffect(()=>{if(!l||document.hidden||!k)return;const c=V.filter(x=>ue(x)===k);O(x=>x?.requestID===k?he(x,c):x),U.current?.refresh()},[l,V,k]),u.useEffect(()=>{if(!v){m(null);return}m(v,ae).catch(c=>{d(`加载事件详情失败: ${c.message}`,"error")})},[d,v,ae,m]);const oe=(c,x="内容")=>ft(c,x,d),He=c=>{p(null);const x=Number(c);Number.isFinite(x)&&x!==f.page&&X(x>f.page?"forward":"backward"),f.setPage(c)},Ge=c=>{p(null),X("replace"),f.setPageSize(c)},Xe=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:c=>i.jsx(Ve,{event:c})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:c=>i.jsx(ke,{event:c})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:c=>i.jsx(Fe,{event:c})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:c=>i.jsx(qe,{event:c})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(ne,{event:c})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(We,{event:c})}];return i.jsxs("div",{className:"page-stack run-page",children:[i.jsxs("div",{className:"page-header",children:[i.jsxs("div",{className:"page-title-group",children:[i.jsxs("h1",{className:"page-title",children:[i.jsx(W,{name:"play",size:24,style:{color:"var(--primary)"}}),i.jsx("span",{children:"运行"}),i.jsx(mt,{status:e})]}),i.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),i.jsx("div",{className:"page-actions",children:i.jsxs("button",{type:"button",className:`btn ${B?"btn-danger":"btn-primary"}`,onClick:r,children:[i.jsx(W,{name:B?"stop":"play",size:16}),i.jsx("span",{children:B?"停止运行":"启动运行"})]})})]}),e?.lastError&&i.jsxs("div",{className:"run-error-banner",children:[i.jsx(W,{name:"warning",size:20}),i.jsxs("div",{children:[i.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),i.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:i.jsxs("div",{className:"glass-panel run-hero-panel",children:[i.jsxs("div",{className:"run-hero-head",children:[i.jsxs("div",{className:"run-hero-heading",children:[i.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),i.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(B?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),i.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),i.jsxs("dl",{className:"run-detail-grid",children:[i.jsxs("div",{className:"run-detail-listener",children:[i.jsx("dt",{children:"监听地址"}),i.jsxs("dd",{className:"run-listener-row",children:[i.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),i.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>oe(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[i.jsx(W,{name:"copy",size:14}),i.jsx("span",{children:"复制"})]})]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"配置版本"}),i.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"认证状态"}),i.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"运行时长"}),i.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?nt(e.uptimeSeconds*1e3):"-"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"调度策略"}),i.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),i.jsxs("div",{className:"run-metric-grid",children:[i.jsx(Q,{label:"输入 Token",value:C?.status==="ready"&&C.observed>0&&C.input!=null?de(C.input):"—",detail:ye(C),icon:"file-down",accent:"var(--primary)"}),i.jsx(Q,{label:"输出 Token",value:C?.status==="ready"&&C.observed>0&&C.output!=null?de(C.output):"—",detail:ye(C),icon:"file-up",accent:"var(--status-warning)"}),i.jsx(Q,{label:"可调度入口",value:I(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--primary)"}),i.jsx(Q,{label:"客户端请求",value:I(t?.clientRequests||0),detail:`端到端 · 成功 ${I(t?.clientSuccesses||0)} / 失败 ${I(t?.clientFailures||0)}`,icon:"move-diagonal",accent:"var(--primary)"}),i.jsx(Q,{label:"上游尝试",value:I(t?.upstreamAttempts||0),detail:`含重试 · 故障转移 ${I(t?.failovers||0)}`,icon:"network",accent:"var(--primary)"})]}),i.jsx(Rt,{observation:o,autoRefresh:l}),i.jsxs("div",{className:"glass-panel run-events-panel",children:[i.jsxs("div",{className:"panel-header",children:[i.jsxs("div",{className:"panel-title-group",children:[i.jsxs("div",{className:"panel-title",children:[i.jsx(W,{name:"activity",size:18,style:{color:"var(--primary)"}}),i.jsx("span",{children:"最近事件与遥测日志"})]}),i.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),i.jsxs("div",{className:"telemetry-toolbar",children:[i.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(c=>i.jsx("button",{type:"button",className:`telemetry-filter-button${g===c.id?" active":""}`,onClick:()=>{p(null),X("replace"),S(c.id)},"aria-pressed":g===c.id,children:c.label},c.id))}),f.mode!=="page"&&i.jsxs("span",{className:"panel-hint",children:[I(z.length)," 条持久事件"]}),i.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{b(c=>c==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${y==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[i.jsx(W,{name:"sort",size:14}),i.jsxs("span",{children:[f.mode==="page"?"本页":"",y==="desc"?"最新优先":"最早优先"]})]})]})]}),i.jsxs("div",{className:"panel-body",style:{padding:0},children:[f.error&&i.jsxs("div",{className:"event-page-error",role:"alert",children:[i.jsxs("span",{children:["读取事件历史失败：",f.error]}),i.jsx("button",{type:"button",className:"btn btn-secondary",onClick:f.retry,children:"重试"})]}),i.jsxs("div",{className:`event-page-stage event-page-stage-${f.loading?"loading":"ready"}`,"data-page":f.page,"data-page-direction":K,"aria-busy":f.loading,children:[f.mode==="page"?i.jsx(gt,{compact:!0,page:f.page,pageSize:f.pageSize,totalCount:f.totalCount,totalPages:f.totalPages,itemCount:z.length,liveItemCount:f.liveEventCount,loading:f.loading,onPageChange:He,onPageSizeChange:Ge,ariaLabel:"持久事件分页"}):null,i.jsxs("div",{className:"event-page-content",children:[R.length>0&&i.jsxs("div",{className:"telemetry-live-section",children:[i.jsx("div",{className:"telemetry-live-desktop",children:i.jsx(Qt,{events:R,selectedEventID:E?.id,onSelect:p})}),i.jsxs("div",{className:"telemetry-live-mobile",children:[i.jsxs("div",{className:"telemetry-live-heading",children:[i.jsxs("span",{children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",I(R.length)]}),i.jsx("small",{children:"上游暂计"})]}),i.jsx(_e,{events:R,selectedEventID:E?.id,onSelect:p,live:!0})]})]}),i.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[i.jsx("div",{className:"telemetry-desktop-table",children:i.jsx(pt,{className:"telemetry-table",columns:Xe,data:z,keyField:"id",ariaLabel:f.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:y,onSortChange:(c,x)=>b(x),onRowClick:c=>p(c.id),activeRowKey:E?.id,emptyText:f.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),i.jsx(_e,{events:z,selectedEventID:E?.id,onSelect:p,loading:f.loading})]}),!E&&(z.length>0||R.length>0)&&i.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"})]})]}),f.mode==="legacy"?i.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),E&&i.jsx(vt,{event:E,chain:$e,chainTruncated:q?.truncated,onSelect:p,requestSummary:ee(E),expanded:w,onToggle:H,onCopy:oe,loading:s?.event?.id!==E.id&&E.detailsOmitted===!0})]})]})}export{ni as RunPage};
