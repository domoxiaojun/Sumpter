import{m as ae,j as i,I as W,r as u,e as I,a as et,i as tt,b as it,c as st,p as rt,d as oe,n as nt,f as at,u as ot,o as xe,g as lt,h as ye,k as le,l as ce,q as C,s as H,t as de,v as G,w as ee,x as Pe,y as Ae,z as ze,A as Ie,B as Be,C as Le,D as Oe,E as ke,F as De,G as ct,H as ut,J as Fe,K as dt,L as ht,M as mt,N as ft,O as pt,P as gt}from"./index-B5O58rj8.js";import{S as vt,c as xt}from"./clipboard-CCioxF4W.js";import{D as yt}from"./DataTable-BwuWGjPI.js";import{P as be}from"./outputSpeed-BTqzSVn0.js";import{E as bt,e as ue,a as jt,b as St,c as _t,C as wt}from"./EventInspector-C_ipZfbU.js";import"./requestPolicy-CjaOnvV0.js";function je(e,t=[],s=e?.requestID){const n=new Map;for(const r of[...e?.events||[],...t])r.requestID===s&&n.set(r.id,ae(n.get(r.id),r));const a=[...n.values()].sort((r,o)=>Number(r.seq||0)-Number(o.seq||0));return{...e,requestID:s,events:a.slice(0,512),truncated:!!(e?.truncated||a.length>512)}}function Rt(e){return e?.events?.some(t=>t.kind==="client"&&["inFlight","in_flight"].includes(t.phase))||!1}function Et({read:e,onValue:t,enabled:s,interval:n=1e3,schedule:a=setTimeout,unschedule:r=clearTimeout,now:o=Date.now}){let l=!1,m=!0,d,p,f=-1/0;function S(g=!1){if(l||p||!g&&(!m||!s())||d!=null)return;const R=g?0:Math.max(0,n-(o()-f));d=a(()=>{d=void 0,j(g)},R)}async function j(g){if(l||p||!g&&!s())return;const R=new AbortController;p=R,f=o();try{const v=await e(R.signal);if(l||R.signal.aborted||!g&&!s())return;m=Rt(v),t(v)}catch(v){[401,403,404].includes(v?.status)&&(m=!1)}finally{p=void 0,!l&&m&&s()&&(d=a(()=>{d=void 0,j(!1)},n))}}return{refresh:S,synchronize(){s()?S():(d!=null&&r(d),d=void 0)},dispose(){l=!0,d!=null&&r(d),p?.abort()}}}function ie({label:e,value:t,detail:s,icon:n,accent:a="var(--primary)"}){return i.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[i.jsxs("div",{className:"metric-head",children:[i.jsx("span",{className:"metric-label",children:e}),n&&i.jsx("div",{className:"metric-icon-box",children:i.jsx(W,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),i.jsx("div",{className:"metric-value",children:t}),s&&i.jsx("div",{className:"metric-detail",children:s})]})}const Nt=`#version 300 es
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
}`,Se=1920*1080*4;let Ct=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=Tt();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(t,s,n,a,r=0,o=0,l=2,m=Se,d=[]){if(t?.nodeType===1)this.parentElement=t;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=t.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const S=this.ownerDocument.createElement("style");S.innerHTML=Ut,S.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(S)}const p=this.ownerDocument.createElement("canvas");this.canvasElement=p,this.parentElement.prepend(p),this.fragmentShader=s,this.providedUniforms=n,this.mipmaps=d,this.currentFrame=o,this.minPixelRatio=l,this.maxPixelCount=m;const f=p.getContext("webgl2",a);if(!f)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=f,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(r),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const t=Mt(this.gl,Nt,this.fragmentShader);t&&(this.program=t)};setupPositionAttribute=()=>{const t=this.gl.getAttribLocation(this.program,"a_position"),s=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,s);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(t),this.gl.vertexAttribPointer(t,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const t={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([s,n])=>{if(t[s]=this.gl.getUniformLocation(this.program,s),n instanceof HTMLImageElement){const a=`${s}AspectRatio`;t[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=t};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([t])=>{if(t?.borderBoxSize[0]){const s=t.devicePixelContentBoxSize?.[0];s!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=s.inlineSize,this.parentDevicePixelHeight=s.blockSize),this.parentWidth=t.borderBoxSize[0].inlineSize,this.parentHeight=t.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const t=this.ownerDocument.defaultView;t?.IntersectionObserver&&(this.intersectionObserver=new t.IntersectionObserver(([s])=>{this.isInViewport=s?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let t=0,s=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const p=Math.max(1,this.minPixelRatio/n);t=this.parentDevicePixelWidth*p*a,s=this.parentDevicePixelHeight*p*a}else{let p=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const f=Pt(this.ownerDocument);p*=Math.max(1,f)}t=Math.round(this.parentWidth)*p,s=Math.round(this.parentHeight)*p}const r=Math.sqrt(this.maxPixelCount)/Math.sqrt(t*s),o=Math.min(1,r),l=Math.round(t*o),m=Math.round(s*o),d=l/Math.round(this.parentWidth);(this.canvasElement.width!==l||this.canvasElement.height!==m||this.renderScale!==d)&&(this.renderScale=d,this.canvasElement.width=l,this.canvasElement.height=m,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=t=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const s=t-this.lastRenderTime;this.lastRenderTime=t,this.currentSpeed!==0&&(this.currentFrame+=s*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(t,s)=>{if(!s.complete||s.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${t} must be fully loaded`);const n=this.textures.get(t);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(t)||this.textureUnitMap.set(t,this.textureUnitMap.size);const a=this.textureUnitMap.get(t);this.gl.activeTexture(this.gl.TEXTURE0+a);const r=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,r),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,s),this.mipmaps.includes(t)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||r===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(t,r);const l=this.uniformLocations[t];if(l){this.gl.uniform1i(l,a);const m=`${t}AspectRatio`,d=this.uniformLocations[m];if(d){const p=s.naturalWidth/s.naturalHeight;this.gl.uniform1f(d,p)}}};areUniformValuesEqual=(t,s)=>t===s?!0:Array.isArray(t)&&Array.isArray(s)&&t.length===s.length?t.every((n,a)=>this.areUniformValuesEqual(n,s[a])):!1;setUniformValues=t=>{this.gl.useProgram(this.program),Object.entries(t).forEach(([s,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[s],a))return;this.uniformCache[s]=a;const r=this.uniformLocations[s];if(!r){console.warn(`Uniform location for ${s} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(s,n);else if(Array.isArray(n)){let o=null,l=null;if(n[0]!==void 0&&Array.isArray(n[0])){const m=n[0].length;if(n.every(d=>d.length===m))o=n.flat(),l=m;else{console.warn(`All child arrays must be the same length for ${s}`);return}}else o=n,l=o.length;switch(l){case 2:this.gl.uniform2fv(r,o);break;case 3:this.gl.uniform3fv(r,o);break;case 4:this.gl.uniform4fv(r,o);break;case 9:this.gl.uniformMatrix3fv(r,!1,o);break;case 16:this.gl.uniformMatrix4fv(r,!1,o);break;default:console.warn(`Unsupported uniform array length: ${l}`)}}else typeof n=="number"?this.gl.uniform1f(r,n):typeof n=="boolean"?this.gl.uniform1i(r,n?1:0):console.warn(`Unsupported uniform type for ${s}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=t=>{this.currentFrame=t,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(t=1)=>{this.speed=t,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=t=>{this.currentSpeed=t,this.rafId===null&&t!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&t===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(t=Se)=>{this.maxPixelCount=t,this.handleResize()};setMinPixelRatio=(t=2)=>{this.minPixelRatio=t,this.handleResize()};setUniforms=t=>{this.setUniformValues(t),this.providedUniforms={...this.providedUniforms,...t},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(t=>{this.gl.deleteTexture(t)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function _e(e,t,s){const n=e.createShader(t);return n?(e.shaderSource(n,s),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function Mt(e,t,s){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(t=t.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),s=s.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const r=_e(e,e.VERTEX_SHADER,t),o=_e(e,e.FRAGMENT_SHADER,s);if(!r||!o)return null;const l=e.createProgram();return l?(e.attachShader(l,r),e.attachShader(l,o),e.linkProgram(l),e.getProgramParameter(l,e.LINK_STATUS)?(e.detachShader(l,r),e.detachShader(l,o),e.deleteShader(r),e.deleteShader(o),l):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(l)),e.deleteProgram(l),e.deleteShader(r),e.deleteShader(o),null)):null}const Ut=`@layer paper-shaders {
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
}`;function Tt(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function Pt(e){const t=visualViewport?.scale??1,s=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=t*s+n,r=outerWidth/a,o=Math.round(100*r);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:r}const At={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},zt={none:0,contain:1,cover:2},It=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,Bt=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,Lt=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,we={maxColorCount:10},Ot=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${we.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${It}
${Bt}
${Lt}

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

  for (int i = 0; i < ${we.maxColorCount}; i++) {
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
`;function kt(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:J;if(typeof e!="string")return J;let t,s,n,a=1;if(e.startsWith("#"))[t,s,n,a]=Dt(e);else if(e.startsWith("rgb")){const r=Ft(e);if(r===null)return J;[t,s,n,a]=r}else if(e.startsWith("hsl")){const r=Vt(e);if(r===null)return J;[t,s,n,a]=$t(r)}else return console.error("Unsupported color format",e),J;return[se(t,0,1),se(s,0,1),se(n,0,1),se(a,0,1)]}function Dt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(r=>r+r).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),J;const t=parseInt(e.slice(0,2),16)/255,s=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[t,s,n,a]}function Ft(e){const t=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0")/255,parseInt(t[2]??"0")/255,parseInt(t[3]??"0")/255,t[4]===void 0?1:parseFloat(t[4])]:null}function Vt(e){const t=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0"),parseInt(t[2]??"0"),parseInt(t[3]??"0"),t[4]===void 0?1:parseFloat(t[4])]:null}function $t(e){const[t,s,n,a]=e,r=t/360,o=s/100,l=n/100;let m,d,p;if(s===0)m=d=p=l;else{const f=(g,R,v)=>(v<0&&(v+=1),v>1&&(v-=1),v<.16666666666666666?g+(R-g)*6*v:v<.5?R:v<.6666666666666666?g+(R-g)*(.6666666666666666-v)*6:g),S=l<.5?l*(1+o):l+o-l*o,j=2*l-S;m=f(j,S,r+1/3),d=f(j,S,r),p=f(j,S,r-1/3)}return[m,d,p,a]}const se=(e,t,s)=>Math.min(Math.max(e,t),s),J=[.5,.5,.5,1],qt="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function Wt(e){const t=u.useRef(void 0),s=u.useCallback(n=>{const a=e.map(r=>{if(r!=null){if(typeof r=="function"){const o=r,l=o(n);return typeof l=="function"?l:()=>{o(null)}}return r.current=n,()=>{r.current=null}}});return()=>{a.forEach(r=>r?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{t.current&&(t.current(),t.current=void 0),n!=null&&(t.current=s(n))},e)}function Re(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const t=e.naturalWidth/e.naturalHeight;e.width=Math.round(t>1?1024*t:1024),e.height=Math.round(t>1?1024:1024/t)}}async function Ee(e){const t={},s=[],n=r=>{try{return r.startsWith("/")||new URL(r),!0}catch{return!1}},a=r=>{try{return r.startsWith("/")?!1:new URL(r,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([r,o])=>{if(typeof o=="string"){const l=o||qt;if(!n(l)){console.warn(`Uniform "${r}" has invalid URL "${l}". Skipping image loading.`);return}const m=new Promise((d,p)=>{const f=new Image;a(l)&&(f.crossOrigin="anonymous"),f.onload=()=>{Re(f),t[r]=f,d()},f.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${l}`),p()},f.src=l});s.push(m)}else if(o instanceof HTMLImageElement){const l=o.decode().then(()=>{Re(o),t[r]=o});s.push(l)}else t[r]=o}),await Promise.all(s),t}const Ve=u.forwardRef(function({fragmentShader:t,uniforms:s,webGlContextAttributes:n,speed:a=0,frame:r=0,width:o,height:l,minPixelRatio:m,maxPixelCount:d,mipmaps:p,style:f,...S},j){const[g,R]=u.useState(!1),v=u.useRef(null),b=u.useRef(null),E=u.useRef(n);u.useEffect(()=>((async()=>{const K=await Ee(s);v.current&&!b.current&&(b.current=new Ct(v.current,t,K,E.current,a,r,m,d,p),R(!0))})(),()=>{b.current?.dispose(),b.current=null}),[t]),u.useEffect(()=>{let z=!1;return(async()=>{const $=await Ee(s);z||b.current?.setUniforms($)})(),()=>{z=!0}},[s,g]),u.useEffect(()=>{b.current?.setSpeed(a)},[a,g]),u.useEffect(()=>{b.current?.setMaxPixelCount(d)},[d,g]),u.useEffect(()=>{b.current?.setMinPixelRatio(m)},[m,g]),u.useEffect(()=>{b.current?.setFrame(r)},[r,g]);const M=Wt([v,j]);return i.jsx("div",{ref:M,style:o!==void 0||l!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof l=="string"&&isNaN(+l)===!1?+l:l,...f}:f,...S})});Ve.displayName="ShaderMount";function Ht(e,t){if(Object.keys(e).length!==Object.keys(t).length)return!1;for(const s in e){if(s==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(t.colors);if(!n||!a){if(Object.is(e.colors,t.colors)===!1)return!1;continue}if(e.colors?.length!==t.colors?.length||!e.colors?.every((r,o)=>r===t.colors?.[o]))return!1;continue}if(Object.is(e[s],t[s])===!1)return!1}return!0}const U={params:{...At,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},Gt=u.memo(function({speed:t=U.params.speed,frame:s=U.params.frame,colors:n=U.params.colors,distortion:a=U.params.distortion,swirl:r=U.params.swirl,grainMixer:o=U.params.grainMixer,grainOverlay:l=U.params.grainOverlay,fit:m=U.params.fit,rotation:d=U.params.rotation,scale:p=U.params.scale,originX:f=U.params.originX,originY:S=U.params.originY,offsetX:j=U.params.offsetX,offsetY:g=U.params.offsetY,worldWidth:R=U.params.worldWidth,worldHeight:v=U.params.worldHeight,...b}){const E={u_colors:n.map(kt),u_colorsCount:n.length,u_distortion:a,u_swirl:r,u_grainMixer:o,u_grainOverlay:l,u_fit:zt[m],u_rotation:d,u_scale:p,u_offsetX:j,u_offsetY:g,u_originX:f,u_originY:S,u_worldWidth:R,u_worldHeight:v};return i.jsx(Ve,{...b,speed:t,frame:s,fragmentShader:Ot,uniforms:E})},Ht);function $e(){const[e,t]=u.useState(()=>document.documentElement.dataset.theme==="light"),[s,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const r=new MutationObserver(()=>t(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),l=()=>n(o.matches);return r.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",l),()=>{r.disconnect(),o.removeEventListener("change",l)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return i.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:i.jsx(Gt,{className:"telemetry-live-mesh",colors:a,speed:s?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function Ne(e,t,s=""){if(!t)return[];const n=new Map((t.events||[]).map(r=>[r.id,r])),a=Math.min(...(t.events||[]).map(r=>Number(r.seq||0)));return(e||[]).filter(r=>{if(s&&r.kind!==s||I(r))return!1;if(t.snapshotChangeSeq!=null)return Number(r.changeSeq||0)>Number(t.snapshotChangeSeq);const o=n.get(r.id);return o?Number(r.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(r.seq||0)>=a})}function Xt(e,t,s){const n=new Map((e||[]).filter(a=>!I(a)).map(a=>[a.id,a]));for(const a of t||[]){const r=n.get(a.id);(!r||Number(a.changeSeq||0)>=Number(r.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,r)=>Number(r.timestamp||0)-Number(a.timestamp||0)||Number(r.seq||0)-Number(a.seq||0)).slice(0,s)}function Ce(e,t){return!t||e?.kind===t}function Me(e,t){return!t||at(e)===t}function Yt({recentEvents:e=[],eventKind:t="",outcome:s="",resetGeneration:n=0,onRecovered:a,autoRefresh:r=!0,refreshInterval:o=15}={}){const[l,m]=u.useState(1),[d,p]=u.useState(et),[f,S]=u.useState(null),[j,g]=u.useState("loading"),[R,v]=u.useState(!0),[b,E]=u.useState(!1),M=u.useRef(!1),z=u.useRef(null),[K,$]=u.useState(null),[te,q]=u.useState(0),D=u.useRef(0),B=u.useRef(null),F=u.useRef(null),X=`${t||"all"}:${s||"all"}:${d}:${Number(n||0)}`;u.useEffect(()=>{M.current=!1,z.current=null,B.current=null,g("loading"),v(!0),S(null),$(null),m(1)},[X]),u.useEffect(()=>{const w=++D.current,h=new AbortController,P=Math.max(1,Number(l)||1),_=B.current,O=M.current;M.current=!1,v(!O),E(!0),$(null);const N=async(y=!0)=>{const Y=await oe.getRuntimeEventPage({page:y?P:1,pageSize:d,kind:t||void 0,outcome:s||void 0,snapshotSeq:y?_?.snapshotSeq:void 0,snapshotChangeSeq:y?_?.snapshotChangeSeq:void 0,historyGeneration:y?_?.historyGeneration:void 0,signal:h.signal});return nt(Y,{page:y?P:1,pageSize:d})};return(async()=>{try{let y;try{y=await N(!0)}catch(Y){if(!tt(Y))throw Y;y=await N(!1),w===D.current&&(m(1),a?.(it(Y)))}if(w!==D.current)return;if(!y){g("legacy"),S(null);return}B.current=y,S(y),g("page"),y.page!==P&&m(y.page)}catch(y){if(w!==D.current||y?.name==="AbortError")return;if([400,404,405].includes(Number(y?.status))){g("legacy"),S(null);return}$(y?.message||"加载事件历史失败")}finally{w===D.current&&(v(!1),E(!1))}})(),()=>h.abort()},[t,s,a,l,d,te,n]);const ne=u.useCallback(w=>{const h=st(w,f?.totalPages||1);M.current=!1,m(h)},[f?.totalPages]),Q=u.useCallback(w=>{M.current=!1;const h=rt(w);B.current=null,p(h),m(1)},[]),L=u.useMemo(()=>j==="page"&&l!==1||s?[]:e.filter(w=>Ce(w,t)&&I(w)),[t,s,j,l,e]),T=u.useMemo(()=>{const w=(j==="page"&&l===1?Ne(e,f,t):[]).filter(_=>Me(_,s)),h=j==="page"?Xt(f?.events||[],w,d):e,P=new Set(L.map(_=>_.id));return h.filter(_=>Ce(_,t)&&Me(_,s)&&!P.has(_.id)&&!I(_))},[t,s,L,j,l,d,e,f]),Z=u.useMemo(()=>[...L,...T],[L,T]);return u.useEffect(()=>{if(j!=="page"||l!==1||b||B.current==null)return;const w=Ne(e,B.current,t);if(!w.length||F.current!=null)return;const h=Math.max(...w.map(O=>Number(O.changeSeq||0))),P=`${t}:${B.current.snapshotChangeSeq??"legacy"}:${h}`,_=z.current;if(!(_?.signature===P&&Date.now()-_.at<5e3))return F.current=window.setTimeout(()=>{F.current=null,z.current={signature:P,at:Date.now()},M.current=!0,B.current=null,q(O=>O+1)},220),()=>{F.current!=null&&(window.clearTimeout(F.current),F.current=null)}},[t,b,j,l,e]),u.useEffect(()=>{if(!r||l!==1||j!=="page")return;const w=window.setInterval(()=>{document.hidden||b||(M.current=!0,B.current=null,q(h=>h+1))},Math.max(1,Number(o)||15)*1e3);return()=>window.clearInterval(w)},[r,l,j,o,b]),{mode:j,loading:R,error:K,events:Z,liveEvents:L,persistedEvents:T,liveEventCount:L.length,page:l,pageSize:d,totalCount:f?.totalCount??Z.length,totalPages:f?.totalPages??(Z.length?1:0),hasNext:!!f?.hasNext,hasPrevious:!!f?.hasPrevious,snapshotSeq:f?.snapshotSeq??null,snapshotChangeSeq:f?.snapshotChangeSeq??null,historyGeneration:f?.historyGeneration??null,setPage:ne,setPageSize:Q,retry:()=>{M.current=!1,q(w=>w+1)}}}function Kt(e){const t=e?.totals?.tokens;if(!t||typeof t!="object")return null;const s=Number(t.observedRequests??t.observed_requests);if(!Number.isFinite(s)||s<0)return null;const n=a=>{const r=Number(a);return Number.isFinite(r)&&r>=0?r:null};return{input:n(t.inputTokens??t.input_tokens),output:n(t.outputTokens??t.output_tokens),observed:s}}function Ue(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天累计 · ${C(e.observed)} 个请求有用量`:"暂无可用用量":"正在读取今天的用量"}function qe(e){const t=Pe(e);if(!t?.applicable)return null;if(t.name&&t.name!=="unidentified_project"){if(t.source==="workspace_local"&&t.localUser)return`${t.name} 本地(${t.localUser})`;const s=t.source==="client_declared"?"（客户端声明）":"";return`项目: ${t.name}${s}`}return t.label}function We(e){const t=G(e,"requestPurpose"),s=typeof t=="object"?t?.kind||t?.type||t?.name:t;if(!s)return null;if(s==="standard")return"主请求";if(s==="websearch")return"WebSearch";if(s==="webfetch")return"WebFetch";const n=G(e,"requestKind");return n!=null?mt(n):ft(e)}function Qt(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!H(e))return"不适用（通知事件）";if(I(e))return"进行中";const t=H(e);return t==="succeeded"?"成功":t==="failed"?"失败":t==="cancelled"?e.failureKind==="client_disconnected"?"连接中断":"已取消":pt(e).replace(/\s*\([^)]*\)\s*/g,"")}function Zt(e){return String(e?.kind||"").toLowerCase()==="notify"&&!H(e)?"不适用（通知事件）":I(e)?"进行中":G(e,"phase")==="completed"||H(e)?"已完成":gt(e).replace(/\s*\([^)]*\)\s*/g,"")}function He(e){return String(e?.kind||"").toLowerCase()==="notify"&&!H(e)?"不适用（通知事件）":`${Fe(e)} · ${Zt(e)}`}function re(e){return[qe(e),ke(e.kind),De(e),ue(e)?`代理: ${ue(e)}`:null,We(e),e.sourceIP].filter(Boolean).join(" · ")}function Ge({event:e,compact:t=!1}){const s=ht(e),n=I(e)||s==="live";return i.jsxs("span",{className:`telemetry-outcome-text ${s}${t?" is-compact":""}`,children:[n&&i.jsx("i",{className:"telemetry-live-pill-dot","aria-hidden":"true"}),Qt(e)]})}function Xe({event:e,live:t=!1}){const s=re(e),n=qe(e),a=e.kind==="notify"?[]:ct(e);return i.jsxs("span",{className:t?"telemetry-live-request":"telemetry-primary-cell",children:[i.jsxs("span",{className:"telemetry-event-head-row",children:[i.jsx("span",{className:`mono-cell${t?" telemetry-live-time":" telemetry-event-time"}`,children:Ie(e.timestamp,{smartDate:!0})}),n&&i.jsx("span",{className:"telemetry-event-project-pill",title:n,children:n})]}),i.jsx("span",{className:`telemetry-event-meta${t?" telemetry-live-request-summary":""}`,title:s,children:[ke(e.kind),De(e),ue(e),We(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&i.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[i.jsx(W,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function Ye({event:e,live:t=!1}){if(e.kind==="notify")return i.jsx("span",{children:e.hookEvent||"通知"});const s=Le(e),n=String(G(e,"clientModel","client_model")||"").trim(),a=n&&s!=="—"&&n!==s,r=Number(G(e,"attemptNumber","attempt_number")||0),o=r>1||ee(e);return i.jsxs("span",{className:t?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[i.jsxs("span",{className:"telemetry-route-model-line",children:[i.jsx("strong",{className:"telemetry-event-model",children:s}),a&&i.jsxs("span",{className:"telemetry-route-mapped-badge mono-cell",title:`客户端模型 ${n} → 路由模型 ${s}`,children:["← ",n]})]}),i.jsxs("span",{className:"telemetry-route-meta",children:[i.jsx("span",{children:Oe(e)}),o&&i.jsx("span",{className:"telemetry-route-retry-badge",children:r>1?`重试 #${r}`:"故障转移"})]})]})}function Ke({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):i.jsxs("span",{className:t?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[i.jsxs("span",{className:"telemetry-result-primary-row",children:[i.jsx(Ge,{event:e}),i.jsx("span",{className:"telemetry-result-detail mono-cell",title:He(e),children:ut(e)?Fe(e):"HTTP —"})]}),i.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:dt(e)})]})}function Qe({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-duration-cell",children:"—"});const t=I(e),{ttfbMS:s,durationMS:n}=de(e),a=s>=5e3,r=Number.isFinite(n)&&n>0?n:Number.isFinite(s)&&s>0?s:0,o=Number.isFinite(s)&&s>0&&r>0?Math.min(100,Math.max(10,Math.round(s/r*100))):0,l=r>0?Math.max(0,100-o):0,m=Number(G(e,"outputTokensPerSecond","output_tokens_per_second")),d=Number.isFinite(m)&&m>0;return i.jsxs("span",{className:"mono-cell telemetry-duration-cell telemetry-inline-latency",title:a?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:[i.jsxs("span",{className:"telemetry-inline-latency-top",children:[i.jsx("span",{className:`telemetry-inline-latency-nums${a?" is-slow":""}${t?" is-live":""}`,children:Be(e)}),d&&i.jsxs("span",{className:"telemetry-speed-pill",children:[m.toFixed(1)," tok/s"]})]}),!t&&r>0&&i.jsxs("span",{className:"telemetry-latency-track","aria-hidden":"true",children:[o>0&&i.jsx("i",{className:`telemetry-latency-ttfb${a?" is-slow":""}`,style:{width:`${o}%`}}),l>0&&i.jsx("i",{className:"telemetry-latency-stream",style:{width:`${l}%`}})]})]})}function Jt({events:e}){if(!e?.length)return null;const t=e.filter(v=>String(v?.kind||"").toLowerCase()!=="notify");if(!t.length)return null;let s=0,n=0,a=0,r=0,o=0,l=0,m=0,d=0,p=0;for(const v of t){const b=H(v);b==="succeeded"?s+=1:b==="failed"?n+=1:b==="cancelled"&&(a+=1);const E=de(v);Number.isFinite(E.ttfbMS)&&E.ttfbMS>=0&&(r+=E.ttfbMS,o+=1),Number.isFinite(E.durationMS)&&E.durationMS>=0&&(l+=E.durationMS,m+=1);const M=Number(G(v,"inputTokens","input_tokens")||0),z=Number(G(v,"outputTokens","output_tokens")||0);Number.isFinite(M)&&M>0&&(d+=M),Number.isFinite(z)&&z>0&&(d+=z),ee(v)&&(p+=1)}const f=s+n+a,S=f>0?`${(s/f*100).toFixed(1)}%`:"—",j=f>0?Math.round(s/f*100):0,g=f>0?Math.round(n/f*100):0,R=f>0?Math.max(0,100-j-g):0;return i.jsxs("div",{className:"telemetry-overview-strip",role:"region","aria-label":"本页事件遥测摘要",children:[i.jsxs("div",{className:"telemetry-overview-item",children:[i.jsxs("div",{className:"telemetry-overview-head",children:[i.jsx("span",{className:"telemetry-overview-label",children:"本页请求健康度"}),i.jsx("strong",{className:"mono-cell telemetry-overview-value",children:S})]}),i.jsxs("div",{className:"telemetry-health-bar","aria-hidden":"true",children:[j>0&&i.jsx("i",{className:"telemetry-health-seg-good",style:{width:`${j}%`}}),g>0&&i.jsx("i",{className:"telemetry-health-seg-fail",style:{width:`${g}%`}}),R>0&&i.jsx("i",{className:"telemetry-health-seg-warn",style:{width:`${R}%`}})]}),i.jsxs("div",{className:"telemetry-overview-sub",children:[i.jsxs("span",{children:["成功 ",C(s)]}),i.jsxs("span",{children:["失败 ",C(n)," · 取消 ",C(a)]})]})]}),i.jsxs("div",{className:"telemetry-overview-item",children:[i.jsxs("div",{className:"telemetry-overview-head",children:[i.jsx("span",{className:"telemetry-overview-label",children:"本页首字节均值"}),i.jsx("strong",{className:"mono-cell telemetry-overview-value",children:o>0?le(r/o):"—"})]}),i.jsxs("div",{className:"telemetry-overview-sub",children:[i.jsx("span",{children:"蓝青段代表首字节响应"}),i.jsxs("span",{children:[C(o)," 条已采样"]})]})]}),i.jsxs("div",{className:"telemetry-overview-item",children:[i.jsxs("div",{className:"telemetry-overview-head",children:[i.jsx("span",{className:"telemetry-overview-label",children:"本页完成耗时均值"}),i.jsx("strong",{className:"mono-cell telemetry-overview-value",children:m>0?le(l/m):"—"})]}),i.jsxs("div",{className:"telemetry-overview-sub",children:[i.jsx("span",{children:"紫绿段代表流式生成"}),i.jsxs("span",{children:[C(m)," 条已采样"]})]})]}),i.jsxs("div",{className:"telemetry-overview-item",children:[i.jsxs("div",{className:"telemetry-overview-head",children:[i.jsx("span",{className:"telemetry-overview-label",children:"本页 Token 与切换"}),i.jsx("strong",{className:"mono-cell telemetry-overview-value",children:d>0?ce(d):"—"})]}),i.jsxs("div",{className:"telemetry-overview-sub",children:[i.jsx("span",{children:"本页输入+输出累计"}),i.jsxs("span",{children:["故障转移 ",C(p)," 次"]})]})]})]})}function he({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const t=jt(e),s=I(e);return i.jsxs("span",{className:`telemetry-usage-cell${t?"":" is-empty"}`,title:s?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[i.jsx("span",{className:"telemetry-usage-tokens",children:St(e)}),i.jsxs("span",{className:"telemetry-usage-secondary",children:[i.jsx("span",{children:s&&!t?"上游尚未返回":_t(e)}),(!s||t)&&i.jsx(wt,{event:e}),s&&t&&i.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function Ze({event:e,live:t=!1}){const s=t||I(e),n=Ae(e)||(e.outcome==="failed"?ze(e):"");return i.jsxs("span",{className:`${s?"telemetry-live-message":"telemetry-event-message"}${ee(e)?" is-warning":H(e)==="failed"?" is-error":""}`,title:n||void 0,children:[ee(e)&&i.jsx("strong",{children:"故障转移 · "}),n||(s?"请求进行中，等待最终结果":"-")]})}function ei({events:e,selectedEventID:t,onSelect:s}){return e?.length?i.jsxs("section",{className:"telemetry-live-group","aria-label":`${C(e.length)} 个进行中请求`,children:[i.jsx($e,{}),i.jsxs("div",{className:"telemetry-live-columns",children:[i.jsxs("span",{className:"telemetry-live-heading-cell",children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",C(e.length)]}),i.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),i.jsx("span",{"aria-hidden":"true",children:"结果"}),i.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),i.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",i.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),i.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),i.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=t===n.id;return i.jsx("div",{role:"listitem",children:i.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>s(n.id),children:[i.jsx(Xe,{event:n,live:!0}),i.jsx(Ye,{event:n,live:!0}),i.jsx(Ke,{event:n,live:!0}),i.jsx(Qe,{event:n}),i.jsx(he,{event:n}),i.jsx(Ze,{event:n,live:!0})]})},n.id)})})]}):null}function Te({events:e,selectedEventID:t,onSelect:s,loading:n,live:a=!1}){return e?.length?i.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&i.jsx($e,{}),e.map(r=>{const o=Pe(r),l=H(r),m=Ae(r)||(r.outcome==="failed"?ze(r):""),d=t===r.id;return i.jsx("article",{role:"listitem",children:i.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${d?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":d,onClick:()=>s(r.id),onKeyDown:p=>{(p.key==="Enter"||p.key===" ")&&(p.preventDefault(),s(r.id))},children:[i.jsxs("div",{className:"telemetry-mobile-heading",children:[i.jsx("span",{className:"mono-cell telemetry-mobile-time",children:Ie(r.timestamp)}),i.jsx(Ge,{event:r,compact:!0}),i.jsx("span",{className:`mono-cell telemetry-mobile-duration${a||I(r)?" is-live":""}`,style:{color:l==="failed"?"var(--status-critical)":void 0},title:de(r).ttfbMS>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:Be(r)})]}),i.jsxs("div",{className:"telemetry-mobile-route",children:[i.jsx("strong",{className:"mono-cell",children:r.kind==="notify"?r.hookEvent||"通知":Le(r)}),r.kind!=="notify"&&i.jsx("small",{children:Oe(r)})]}),i.jsxs("div",{className:"telemetry-mobile-usage",children:[i.jsx("span",{className:"telemetry-mobile-status",children:He(r)}),r.kind!=="notify"&&i.jsx(he,{event:r})]}),i.jsxs("div",{className:"telemetry-mobile-context",children:[i.jsx("span",{className:"telemetry-mobile-request-summary",title:re(r),children:re(r)||o.label}),m&&i.jsxs("p",{className:`telemetry-mobile-message${l==="failed"?" is-error":ee(r)?" is-warning":""}`,children:[i.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),ee(r)&&i.jsx("strong",{children:"故障转移 · "}),m]})]})]})},r.id)})]}):a?null:i.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:i.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function li(){const{status:e,runtime:t,runtimeEventDetail:s,config:n,secretStatus:a,toggleProxy:r,autoRefresh:o,runRefreshInterval:l,loadRuntimeEvent:m,addToast:d}=ot(),[p,f]=u.useState("client"),[S,j]=u.useState(""),[g,R]=u.useState("desc"),[v,b]=u.useState(null),E=u.useCallback(c=>{b(x=>x===c?null:c)},[]),[M,z]=u.useState({}),K=(c,x)=>z(k=>k[c]===x?k:{...k,[c]:x}),[$,te]=u.useState(null),q=$?.events||[],D=u.useRef(null),B=u.useRef(o);B.current=o;const[F,X]=u.useState("none"),[,ne]=u.useState(0),Q=e?.running,L=t?.recentEvents||[],[T,Z]=u.useState(null),w=u.useCallback(c=>d(c,"info"),[d]),h=Yt({recentEvents:L,autoRefresh:o,refreshInterval:l,eventKind:p==="all"?"":p,outcome:S,resetGeneration:t?.resetGeneration,onRecovered:w});u.useEffect(()=>{if(h.loading||F==="none")return;const c=window.setTimeout(()=>X("none"),280);return()=>window.clearTimeout(c)},[h.loading,F]),u.useEffect(()=>{let c=!1;const x=()=>{document.hidden||oe.getRuntimeTrends({range:"today",granularity:"hour"}).then(A=>{if(c)return;const ve=Kt(A);Z(ve?{status:"ready",...ve}:{status:"error",input:null,output:null,observed:0})}).catch(()=>{c||Z(A=>A?.status==="ready"?A:{status:"error",input:null,output:null,observed:0})})};if(x(),!o)return()=>{c=!0};const k=window.setInterval(x,Math.max(15,Number(l)||15)*1e3);return()=>{c=!0,window.clearInterval(k)}},[o,l,t?.resetGeneration]),u.useEffect(()=>{if(!L.some(x=>I(x)))return;const c=window.setInterval(()=>ne(x=>x+1),1e3);return()=>window.clearInterval(c)},[L]);const P=u.useMemo(()=>xe(h.liveEvents||[],g),[h.liveEvents,g]),_=u.useMemo(()=>xe(h.persistedEvents||h.events.filter(c=>!I(c)),g),[h.events,h.persistedEvents,g]),O=u.useMemo(()=>{const c=new Map;for(const x of[...L,...h.events])c.set(x.id,ae(c.get(x.id),x));return[...c.values()]},[h.events,L]),N=v,y=N&&(s?.event?.id===N?s.event:O.find(c=>c.id===N)||q.find(c=>c.id===N)||_.find(c=>c.id===N)),Y=u.useMemo(()=>{const c=new Map;for(const A of[...q,...O])c.set(A.id,ae(c.get(A.id),A));const x=[...c.values()],k=lt(x,N);return s?.event?.id!==N?k:k.map(A=>A.id===N?s.event:A)},[O,N,q,s]),V=y?ye(y):"",me=Math.max(Number(O.find(c=>c.id===N)?.changeSeq||0),Number(q.find(c=>c.id===N)?.changeSeq||0));u.useEffect(()=>{if(te(null),!V||V==="-")return;const c=Et({read:k=>oe.getRuntimeRequestChain(V,{signal:k}),onValue:k=>te(A=>je(k,A?.requestID===V?A.events:[])),enabled:()=>B.current&&!document.hidden});D.current=c;const x=()=>c.synchronize();return document.addEventListener("visibilitychange",x),c.refresh(!0),()=>{c.dispose(),D.current===c&&(D.current=null),document.removeEventListener("visibilitychange",x)}},[V,t?.resetGeneration]),u.useEffect(()=>{D.current?.synchronize()},[o]),u.useEffect(()=>{if(!o||document.hidden||!V)return;const c=O.filter(x=>ye(x)===V);te(x=>x?.requestID===V?je(x,c):x),D.current?.refresh()},[o,O,V]),u.useEffect(()=>{if(!N){m(null);return}m(N,me).catch(c=>{d(`加载事件详情失败: ${c.message}`,"error")})},[d,N,me,m]);const fe=(c,x="内容")=>xt(c,x,d),pe=c=>{b(null);const x=Number(c);Number.isFinite(x)&&x!==h.page&&X(x>h.page?"forward":"backward"),h.setPage(c)},ge=c=>{b(null),X("replace"),h.setPageSize(c)},Je=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:c=>i.jsx(Xe,{event:c})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:c=>i.jsx(Ye,{event:c})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:c=>i.jsx(Ke,{event:c})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:c=>i.jsx(Qe,{event:c})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(he,{event:c})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:c=>i.jsx(Ze,{event:c})}];return i.jsxs("div",{className:"page-stack run-page",children:[i.jsxs("div",{className:"page-header",children:[i.jsxs("div",{className:"page-title-group",children:[i.jsxs("h1",{className:"page-title",children:[i.jsx(W,{name:"play",size:24,style:{color:"var(--primary)"}}),i.jsx("span",{children:"运行"}),i.jsx(vt,{status:e})]}),i.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),i.jsx("div",{className:"page-actions",children:i.jsxs("button",{type:"button",className:`btn ${Q?"btn-danger":"btn-primary"}`,onClick:r,children:[i.jsx(W,{name:Q?"stop":"play",size:16}),i.jsx("span",{children:Q?"停止运行":"启动运行"})]})})]}),e?.lastError&&i.jsxs("div",{className:"run-error-banner",children:[i.jsx(W,{name:"warning",size:20}),i.jsxs("div",{children:[i.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),i.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:i.jsxs("div",{className:"glass-panel run-hero-panel",children:[i.jsxs("div",{className:"run-hero-head",children:[i.jsxs("div",{className:"run-hero-heading",children:[i.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),i.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(Q?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),i.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),i.jsxs("dl",{className:"run-detail-grid",children:[i.jsxs("div",{className:"run-detail-listener",children:[i.jsx("dt",{children:"监听地址"}),i.jsxs("dd",{className:"run-listener-row",children:[i.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),i.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>fe(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[i.jsx(W,{name:"copy",size:14}),i.jsx("span",{children:"复制"})]})]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"配置版本"}),i.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"认证状态"}),i.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"运行时长"}),i.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?le(e.uptimeSeconds*1e3):"-"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"调度策略"}),i.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),i.jsxs("div",{className:"run-metric-grid",children:[i.jsx(ie,{label:"输入 Token",value:T?.status==="ready"&&T.observed>0&&T.input!=null?ce(T.input):"—",detail:Ue(T),icon:"file-down",accent:"var(--primary)"}),i.jsx(ie,{label:"输出 Token",value:T?.status==="ready"&&T.observed>0&&T.output!=null?ce(T.output):"—",detail:Ue(T),icon:"file-up",accent:"var(--status-warning)"}),i.jsx(ie,{label:"可调度入口",value:C(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--accent-cyan)"}),i.jsx(ie,{label:"客户端请求",value:C(t?.clientRequests||0),detail:`端到端 · 成功 ${C(t?.clientSuccesses||0)} / 失败 ${C(t?.clientFailures||0)}`,icon:"move-diagonal",accent:"var(--status-good)"}),i.jsx(ie,{label:"上游尝试",value:C(t?.upstreamAttempts||0),detail:`含重试 · 故障转移 ${C(t?.failovers||0)}`,icon:"network",accent:"var(--accent-indigo)"})]}),i.jsxs("div",{className:"glass-panel run-events-panel",children:[i.jsxs("div",{className:"panel-header",children:[i.jsxs("div",{className:"panel-title-group",children:[i.jsxs("div",{className:"panel-title",children:[i.jsx(W,{name:"activity",size:18,style:{color:"var(--primary)"}}),i.jsx("span",{children:"最近事件与遥测日志"})]}),i.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),i.jsxs("div",{className:"telemetry-toolbar",children:[i.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(c=>i.jsx("button",{type:"button",className:`telemetry-filter-button${p===c.id?" active":""}`,onClick:()=>{b(null),X("replace"),f(c.id)},"aria-pressed":p===c.id,children:c.label},c.id))}),i.jsx("div",{className:"telemetry-filter-group telemetry-outcome-filter-group",role:"group","aria-label":"事件结果状态筛选",children:[{id:"",label:"全部状态"},{id:"succeeded",label:"成功"},{id:"failed",label:"失败"},{id:"cancelled",label:"已取消"}].map(c=>i.jsx("button",{type:"button",className:`telemetry-filter-button${S===c.id?" active":""}`,onClick:()=>{b(null),X("replace"),j(c.id)},"aria-pressed":S===c.id,children:c.label},c.id||"all-outcomes"))}),h.mode!=="page"&&i.jsxs("span",{className:"panel-hint",children:[C(_.length)," 条持久事件"]}),i.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{R(c=>c==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${g==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[i.jsx(W,{name:"sort",size:14}),i.jsxs("span",{children:[h.mode==="page"?"本页":"",g==="desc"?"最新优先":"最早优先"]})]})]})]}),i.jsx(Jt,{events:_}),i.jsxs("div",{className:"panel-body",style:{padding:0},children:[h.error&&i.jsxs("div",{className:"event-page-error",role:"alert",children:[i.jsxs("span",{children:["读取事件历史失败：",h.error]}),i.jsx("button",{type:"button",className:"btn btn-secondary",onClick:h.retry,children:"重试"})]}),i.jsxs("div",{className:`event-page-stage event-page-stage-${h.loading?"loading":"ready"}`,"data-page":h.page,"data-page-direction":F,"aria-busy":h.loading,children:[h.mode==="page"?i.jsx(be,{compact:!0,page:h.page,pageSize:h.pageSize,totalCount:h.totalCount,totalPages:h.totalPages,itemCount:_.length,liveItemCount:h.liveEventCount,loading:h.loading,onPageChange:pe,onPageSizeChange:ge,ariaLabel:"持久事件分页"}):null,i.jsxs("div",{className:"event-page-content",children:[P.length>0&&i.jsxs("div",{className:"telemetry-live-section",children:[i.jsx("div",{className:"telemetry-live-desktop",children:i.jsx(ei,{events:P,selectedEventID:y?.id,onSelect:E})}),i.jsxs("div",{className:"telemetry-live-mobile",children:[i.jsxs("div",{className:"telemetry-live-heading",children:[i.jsxs("span",{children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",C(P.length)]}),i.jsx("small",{children:"上游暂计"})]}),i.jsx(Te,{events:P,selectedEventID:y?.id,onSelect:E,live:!0})]})]}),i.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[i.jsx("div",{className:"telemetry-desktop-table",children:i.jsx(yt,{className:"telemetry-table",columns:Je,data:_,keyField:"id",ariaLabel:h.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:g,onSortChange:(c,x)=>R(x),onRowClick:c=>E(c.id),activeRowKey:y?.id,emptyText:h.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),i.jsx(Te,{events:_,selectedEventID:y?.id,onSelect:E,loading:h.loading})]}),!y&&(_.length>0||P.length>0)&&i.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"}),h.mode==="page"&&_.length>=15&&h.totalPages>1?i.jsx("div",{className:"telemetry-bottom-pagination",children:i.jsx(be,{compact:!0,page:h.page,pageSize:h.pageSize,totalCount:h.totalCount,totalPages:h.totalPages,itemCount:_.length,liveItemCount:h.liveEventCount,loading:h.loading,onPageChange:pe,onPageSizeChange:ge,ariaLabel:"持久事件底部分页"})}):null]})]}),h.mode==="legacy"?i.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),y&&i.jsx(bt,{event:y,chain:Y,chainTruncated:$?.truncated,onSelect:b,onClose:()=>b(null),requestSummary:re(y),expanded:M,onToggle:K,onCopy:fe,loading:s?.event?.id!==y.id&&y.detailsOmitted===!0})]})]})}export{li as RunPage};
