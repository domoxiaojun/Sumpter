import{m as re,j as i,I as W,r as u,e as k,a as Ge,i as Xe,b as Ye,c as Ke,p as Qe,d as se,n as Ze,u as Je,o as ue,g as et,f as de,h as tt,k as he,l as I,q as Re,s as G,t as je,v as Ee,w as we,x as Ne,y as Ue,z as Te,A as te,B as Ce,C as Ae,D as it,E as rt,F as st,G as Me,H as nt,J as at,K as ne,L as ot,M as lt,N as ct,O as ut}from"./index-C4sDZsyg.js";import{S as dt,c as ht}from"./clipboard-BbYIj7ol.js";import{D as mt}from"./DataTable-B6UgUC1j.js";import{P as ft}from"./outputSpeed-CItiRGLh.js";import{E as pt,e as ae,a as gt,b as vt,c as xt,C as bt}from"./EventInspector-e38xvkWE.js";import"./requestPolicy-CjaOnvV0.js";function me(e,t=[],s=e?.requestID){const n=new Map;for(const r of[...e?.events||[],...t])r.requestID===s&&n.set(r.id,re(n.get(r.id),r));const a=[...n.values()].sort((r,o)=>Number(r.seq||0)-Number(o.seq||0));return{...e,requestID:s,events:a.slice(0,512),truncated:!!(e?.truncated||a.length>512)}}function yt(e){return e?.events?.some(t=>t.kind==="client"&&["inFlight","in_flight"].includes(t.phase))||!1}function _t({read:e,onValue:t,enabled:s,interval:n=1e3,schedule:a=setTimeout,unschedule:r=clearTimeout,now:o=Date.now}){let c=!1,m=!0,h,d,v=-1/0;function p(R=!1){if(c||d||!R&&(!m||!s())||h!=null)return;const _=R?0:Math.max(0,n-(o()-v));h=a(()=>{h=void 0,E(R)},_)}async function E(R){if(c||d||!R&&!s())return;const _=new AbortController;d=_,v=o();try{const x=await e(_.signal);if(c||_.signal.aborted||!R&&!s())return;m=yt(x),t(x)}catch(x){[401,403,404].includes(x?.status)&&(m=!1)}finally{d=void 0,!c&&m&&s()&&(h=a(()=>{h=void 0,E(!1)},n))}}return{refresh:p,synchronize(){s()?p():(h!=null&&r(h),h=void 0)},dispose(){c=!0,h!=null&&r(h),d?.abort()}}}function Z({label:e,value:t,detail:s,icon:n,accent:a="var(--primary)"}){return i.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[i.jsxs("div",{className:"metric-head",children:[i.jsx("span",{className:"metric-label",children:e}),n&&i.jsx("div",{className:"metric-icon-box",children:i.jsx(W,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),i.jsx("div",{className:"metric-value",children:t}),s&&i.jsx("div",{className:"metric-detail",children:s})]})}const St=`#version 300 es
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
}`,fe=1920*1080*4;let Rt=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=wt();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(t,s,n,a,r=0,o=0,c=2,m=fe,h=[]){if(t?.nodeType===1)this.parentElement=t;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=t.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const p=this.ownerDocument.createElement("style");p.innerHTML=Et,p.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(p)}const d=this.ownerDocument.createElement("canvas");this.canvasElement=d,this.parentElement.prepend(d),this.fragmentShader=s,this.providedUniforms=n,this.mipmaps=h,this.currentFrame=o,this.minPixelRatio=c,this.maxPixelCount=m;const v=d.getContext("webgl2",a);if(!v)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=v,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(r),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const t=jt(this.gl,St,this.fragmentShader);t&&(this.program=t)};setupPositionAttribute=()=>{const t=this.gl.getAttribLocation(this.program,"a_position"),s=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,s);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(t),this.gl.vertexAttribPointer(t,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const t={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([s,n])=>{if(t[s]=this.gl.getUniformLocation(this.program,s),n instanceof HTMLImageElement){const a=`${s}AspectRatio`;t[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=t};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([t])=>{if(t?.borderBoxSize[0]){const s=t.devicePixelContentBoxSize?.[0];s!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=s.inlineSize,this.parentDevicePixelHeight=s.blockSize),this.parentWidth=t.borderBoxSize[0].inlineSize,this.parentHeight=t.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const t=this.ownerDocument.defaultView;t?.IntersectionObserver&&(this.intersectionObserver=new t.IntersectionObserver(([s])=>{this.isInViewport=s?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let t=0,s=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const d=Math.max(1,this.minPixelRatio/n);t=this.parentDevicePixelWidth*d*a,s=this.parentDevicePixelHeight*d*a}else{let d=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const v=Nt(this.ownerDocument);d*=Math.max(1,v)}t=Math.round(this.parentWidth)*d,s=Math.round(this.parentHeight)*d}const r=Math.sqrt(this.maxPixelCount)/Math.sqrt(t*s),o=Math.min(1,r),c=Math.round(t*o),m=Math.round(s*o),h=c/Math.round(this.parentWidth);(this.canvasElement.width!==c||this.canvasElement.height!==m||this.renderScale!==h)&&(this.renderScale=h,this.canvasElement.width=c,this.canvasElement.height=m,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=t=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const s=t-this.lastRenderTime;this.lastRenderTime=t,this.currentSpeed!==0&&(this.currentFrame+=s*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(t,s)=>{if(!s.complete||s.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${t} must be fully loaded`);const n=this.textures.get(t);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(t)||this.textureUnitMap.set(t,this.textureUnitMap.size);const a=this.textureUnitMap.get(t);this.gl.activeTexture(this.gl.TEXTURE0+a);const r=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,r),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,s),this.mipmaps.includes(t)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||r===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(t,r);const c=this.uniformLocations[t];if(c){this.gl.uniform1i(c,a);const m=`${t}AspectRatio`,h=this.uniformLocations[m];if(h){const d=s.naturalWidth/s.naturalHeight;this.gl.uniform1f(h,d)}}};areUniformValuesEqual=(t,s)=>t===s?!0:Array.isArray(t)&&Array.isArray(s)&&t.length===s.length?t.every((n,a)=>this.areUniformValuesEqual(n,s[a])):!1;setUniformValues=t=>{this.gl.useProgram(this.program),Object.entries(t).forEach(([s,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[s],a))return;this.uniformCache[s]=a;const r=this.uniformLocations[s];if(!r){console.warn(`Uniform location for ${s} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(s,n);else if(Array.isArray(n)){let o=null,c=null;if(n[0]!==void 0&&Array.isArray(n[0])){const m=n[0].length;if(n.every(h=>h.length===m))o=n.flat(),c=m;else{console.warn(`All child arrays must be the same length for ${s}`);return}}else o=n,c=o.length;switch(c){case 2:this.gl.uniform2fv(r,o);break;case 3:this.gl.uniform3fv(r,o);break;case 4:this.gl.uniform4fv(r,o);break;case 9:this.gl.uniformMatrix3fv(r,!1,o);break;case 16:this.gl.uniformMatrix4fv(r,!1,o);break;default:console.warn(`Unsupported uniform array length: ${c}`)}}else typeof n=="number"?this.gl.uniform1f(r,n):typeof n=="boolean"?this.gl.uniform1i(r,n?1:0):console.warn(`Unsupported uniform type for ${s}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=t=>{this.currentFrame=t,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(t=1)=>{this.speed=t,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=t=>{this.currentSpeed=t,this.rafId===null&&t!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&t===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(t=fe)=>{this.maxPixelCount=t,this.handleResize()};setMinPixelRatio=(t=2)=>{this.minPixelRatio=t,this.handleResize()};setUniforms=t=>{this.setUniformValues(t),this.providedUniforms={...this.providedUniforms,...t},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(t=>{this.gl.deleteTexture(t)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function pe(e,t,s){const n=e.createShader(t);return n?(e.shaderSource(n,s),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function jt(e,t,s){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(t=t.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),s=s.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const r=pe(e,e.VERTEX_SHADER,t),o=pe(e,e.FRAGMENT_SHADER,s);if(!r||!o)return null;const c=e.createProgram();return c?(e.attachShader(c,r),e.attachShader(c,o),e.linkProgram(c),e.getProgramParameter(c,e.LINK_STATUS)?(e.detachShader(c,r),e.detachShader(c,o),e.deleteShader(r),e.deleteShader(o),c):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(c)),e.deleteProgram(c),e.deleteShader(r),e.deleteShader(o),null)):null}const Et=`@layer paper-shaders {
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
}`;function wt(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function Nt(e){const t=visualViewport?.scale??1,s=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=t*s+n,r=outerWidth/a,o=Math.round(100*r);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:r}const Ut={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},Tt={none:0,contain:1,cover:2},Ct=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,At=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,Mt=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,ge={maxColorCount:10},Pt=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${ge.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${Ct}
${At}
${Mt}

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

  for (int i = 0; i < ${ge.maxColorCount}; i++) {
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
`;function zt(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:K;if(typeof e!="string")return K;let t,s,n,a=1;if(e.startsWith("#"))[t,s,n,a]=Bt(e);else if(e.startsWith("rgb")){const r=It(e);if(r===null)return K;[t,s,n,a]=r}else if(e.startsWith("hsl")){const r=Lt(e);if(r===null)return K;[t,s,n,a]=Ot(r)}else return console.error("Unsupported color format",e),K;return[ee(t,0,1),ee(s,0,1),ee(n,0,1),ee(a,0,1)]}function Bt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(r=>r+r).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),K;const t=parseInt(e.slice(0,2),16)/255,s=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[t,s,n,a]}function It(e){const t=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0")/255,parseInt(t[2]??"0")/255,parseInt(t[3]??"0")/255,t[4]===void 0?1:parseFloat(t[4])]:null}function Lt(e){const t=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0"),parseInt(t[2]??"0"),parseInt(t[3]??"0"),t[4]===void 0?1:parseFloat(t[4])]:null}function Ot(e){const[t,s,n,a]=e,r=t/360,o=s/100,c=n/100;let m,h,d;if(s===0)m=h=d=c;else{const v=(R,_,x)=>(x<0&&(x+=1),x>1&&(x-=1),x<.16666666666666666?R+(_-R)*6*x:x<.5?_:x<.6666666666666666?R+(_-R)*(.6666666666666666-x)*6:R),p=c<.5?c*(1+o):c+o-c*o,E=2*c-p;m=v(E,p,r+1/3),h=v(E,p,r),d=v(E,p,r-1/3)}return[m,h,d,a]}const ee=(e,t,s)=>Math.min(Math.max(e,t),s),K=[.5,.5,.5,1],Dt="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function Vt(e){const t=u.useRef(void 0),s=u.useCallback(n=>{const a=e.map(r=>{if(r!=null){if(typeof r=="function"){const o=r,c=o(n);return typeof c=="function"?c:()=>{o(null)}}return r.current=n,()=>{r.current=null}}});return()=>{a.forEach(r=>r?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{t.current&&(t.current(),t.current=void 0),n!=null&&(t.current=s(n))},e)}function ve(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const t=e.naturalWidth/e.naturalHeight;e.width=Math.round(t>1?1024*t:1024),e.height=Math.round(t>1?1024:1024/t)}}async function xe(e){const t={},s=[],n=r=>{try{return r.startsWith("/")||new URL(r),!0}catch{return!1}},a=r=>{try{return r.startsWith("/")?!1:new URL(r,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([r,o])=>{if(typeof o=="string"){const c=o||Dt;if(!n(c)){console.warn(`Uniform "${r}" has invalid URL "${c}". Skipping image loading.`);return}const m=new Promise((h,d)=>{const v=new Image;a(c)&&(v.crossOrigin="anonymous"),v.onload=()=>{ve(v),t[r]=v,h()},v.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${c}`),d()},v.src=c});s.push(m)}else if(o instanceof HTMLImageElement){const c=o.decode().then(()=>{ve(o),t[r]=o});s.push(c)}else t[r]=o}),await Promise.all(s),t}const Pe=u.forwardRef(function({fragmentShader:t,uniforms:s,webGlContextAttributes:n,speed:a=0,frame:r=0,width:o,height:c,minPixelRatio:m,maxPixelCount:h,mipmaps:d,style:v,...p},E){const[R,_]=u.useState(!1),x=u.useRef(null),N=u.useRef(null),C=u.useRef(n);u.useEffect(()=>((async()=>{const B=await xe(s);x.current&&!N.current&&(N.current=new Rt(x.current,t,B,C.current,a,r,m,h,d),_(!0))})(),()=>{N.current?.dispose(),N.current=null}),[t]),u.useEffect(()=>{let L=!1;return(async()=>{const O=await xe(s);L||N.current?.setUniforms(O)})(),()=>{L=!0}},[s,R]),u.useEffect(()=>{N.current?.setSpeed(a)},[a,R]),u.useEffect(()=>{N.current?.setMaxPixelCount(h)},[h,R]),u.useEffect(()=>{N.current?.setMinPixelRatio(m)},[m,R]),u.useEffect(()=>{N.current?.setFrame(r)},[r,R]);const F=Vt([x,E]);return i.jsx("div",{ref:F,style:o!==void 0||c!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof c=="string"&&isNaN(+c)===!1?+c:c,...v}:v,...p})});Pe.displayName="ShaderMount";function kt(e,t){if(Object.keys(e).length!==Object.keys(t).length)return!1;for(const s in e){if(s==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(t.colors);if(!n||!a){if(Object.is(e.colors,t.colors)===!1)return!1;continue}if(e.colors?.length!==t.colors?.length||!e.colors?.every((r,o)=>r===t.colors?.[o]))return!1;continue}if(Object.is(e[s],t[s])===!1)return!1}return!0}const U={params:{...Ut,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},Ft=u.memo(function({speed:t=U.params.speed,frame:s=U.params.frame,colors:n=U.params.colors,distortion:a=U.params.distortion,swirl:r=U.params.swirl,grainMixer:o=U.params.grainMixer,grainOverlay:c=U.params.grainOverlay,fit:m=U.params.fit,rotation:h=U.params.rotation,scale:d=U.params.scale,originX:v=U.params.originX,originY:p=U.params.originY,offsetX:E=U.params.offsetX,offsetY:R=U.params.offsetY,worldWidth:_=U.params.worldWidth,worldHeight:x=U.params.worldHeight,...N}){const C={u_colors:n.map(zt),u_colorsCount:n.length,u_distortion:a,u_swirl:r,u_grainMixer:o,u_grainOverlay:c,u_fit:Tt[m],u_rotation:h,u_scale:d,u_offsetX:E,u_offsetY:R,u_originX:v,u_originY:p,u_worldWidth:_,u_worldHeight:x};return i.jsx(Pe,{...N,speed:t,frame:s,fragmentShader:Pt,uniforms:C})},kt);function ze(){const[e,t]=u.useState(()=>document.documentElement.dataset.theme==="light"),[s,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const r=new MutationObserver(()=>t(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),c=()=>n(o.matches);return r.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",c),()=>{r.disconnect(),o.removeEventListener("change",c)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return i.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:i.jsx(Ft,{className:"telemetry-live-mesh",colors:a,speed:s?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function be(e,t,s=""){if(!t)return[];const n=new Map((t.events||[]).map(r=>[r.id,r])),a=Math.min(...(t.events||[]).map(r=>Number(r.seq||0)));return(e||[]).filter(r=>{if(s&&r.kind!==s||k(r))return!1;if(t.snapshotChangeSeq!=null)return Number(r.changeSeq||0)>Number(t.snapshotChangeSeq);const o=n.get(r.id);return o?Number(r.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(r.seq||0)>=a})}function qt(e,t,s){const n=new Map((e||[]).filter(a=>!k(a)).map(a=>[a.id,a]));for(const a of t||[]){const r=n.get(a.id);(!r||Number(a.changeSeq||0)>=Number(r.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,r)=>Number(r.timestamp||0)-Number(a.timestamp||0)||Number(r.seq||0)-Number(a.seq||0)).slice(0,s)}function ye(e,t){return!t||e?.kind===t}function $t({recentEvents:e=[],eventKind:t="",resetGeneration:s=0,onRecovered:n,autoRefresh:a=!0,refreshInterval:r=15}={}){const[o,c]=u.useState(1),[m,h]=u.useState(Ge),[d,v]=u.useState(null),[p,E]=u.useState("loading"),[R,_]=u.useState(!0),[x,N]=u.useState(!1),C=u.useRef(!1),F=u.useRef(null),[L,B]=u.useState(null),[O,X]=u.useState(0),D=u.useRef(0),A=u.useRef(null),q=u.useRef(null),Y=`${t||"all"}:${m}:${Number(s||0)}`;u.useEffect(()=>{C.current=!1,F.current=null,A.current=null,E("loading"),_(!0),v(null),B(null),c(1)},[Y]),u.useEffect(()=>{const S=++D.current,j=new AbortController,T=Math.max(1,Number(o)||1),b=A.current,w=C.current;C.current=!1,_(!w),N(!0),B(null);const J=async(y=!0)=>{const $=await se.getRuntimeEventPage({page:y?T:1,pageSize:m,kind:t||void 0,snapshotSeq:y?b?.snapshotSeq:void 0,snapshotChangeSeq:y?b?.snapshotChangeSeq:void 0,historyGeneration:y?b?.historyGeneration:void 0,signal:j.signal});return Ze($,{page:y?T:1,pageSize:m})};return(async()=>{try{let y;try{y=await J(!0)}catch($){if(!Xe($))throw $;y=await J(!1),S===D.current&&(c(1),n?.(Ye($)))}if(S!==D.current)return;if(!y){E("legacy"),v(null);return}A.current=y,v(y),E("page"),y.page!==T&&c(y.page)}catch(y){if(S!==D.current||y?.name==="AbortError")return;if([400,404,405].includes(Number(y?.status))){E("legacy"),v(null);return}B(y?.message||"加载事件历史失败")}finally{S===D.current&&(_(!1),N(!1))}})(),()=>j.abort()},[t,n,o,m,O,s]);const H=u.useCallback(S=>{const j=Ke(S,d?.totalPages||1);C.current=!1,c(j)},[d?.totalPages]),P=u.useCallback(S=>{C.current=!1;const j=Qe(S);A.current=null,h(j),c(1)},[]),V=u.useMemo(()=>p==="page"&&o!==1?[]:e.filter(S=>ye(S,t)&&k(S)),[t,p,o,e]),Q=u.useMemo(()=>{const S=p==="page"&&o===1?be(e,d,t):[],j=p==="page"?qt(d?.events||[],S,m):e,T=new Set(V.map(b=>b.id));return j.filter(b=>ye(b,t)&&!T.has(b.id)&&!k(b))},[t,V,p,o,m,e,d]),f=u.useMemo(()=>[...V,...Q],[V,Q]);return u.useEffect(()=>{if(p!=="page"||o!==1||x||A.current==null)return;const S=be(e,A.current,t);if(!S.length||q.current!=null)return;const j=Math.max(...S.map(w=>Number(w.changeSeq||0))),T=`${t}:${A.current.snapshotChangeSeq??"legacy"}:${j}`,b=F.current;if(!(b?.signature===T&&Date.now()-b.at<5e3))return q.current=window.setTimeout(()=>{q.current=null,F.current={signature:T,at:Date.now()},C.current=!0,A.current=null,X(w=>w+1)},220),()=>{q.current!=null&&(window.clearTimeout(q.current),q.current=null)}},[t,x,p,o,e]),u.useEffect(()=>{if(!a||o!==1||p!=="page")return;const S=window.setInterval(()=>{document.hidden||x||(C.current=!0,A.current=null,X(j=>j+1))},Math.max(1,Number(r)||15)*1e3);return()=>window.clearInterval(S)},[a,o,p,r,x]),{mode:p,loading:R,error:L,events:f,liveEvents:V,persistedEvents:Q,liveEventCount:V.length,page:o,pageSize:m,totalCount:d?.totalCount??f.length,totalPages:d?.totalPages??(f.length?1:0),hasNext:!!d?.hasNext,hasPrevious:!!d?.hasPrevious,snapshotSeq:d?.snapshotSeq??null,snapshotChangeSeq:d?.snapshotChangeSeq??null,historyGeneration:d?.historyGeneration??null,setPage:H,setPageSize:P,retry:()=>{C.current=!1,X(S=>S+1)}}}function Wt(e){const t=e?.totals?.tokens;if(!t||typeof t!="object")return null;const s=Number(t.observedRequests??t.observed_requests);if(!Number.isFinite(s)||s<0)return null;const n=a=>{const r=Number(a);return Number.isFinite(r)&&r>=0?r:null};return{input:n(t.inputTokens??t.input_tokens),output:n(t.outputTokens??t.output_tokens),observed:s}}function _e(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天累计 · ${I(e.observed)} 个请求有用量`:"暂无可用用量":"正在读取今天的用量"}function Be(e){const t=Re(e);if(!t?.applicable)return null;if(t.name&&t.name!=="unidentified_project"){if(t.source==="workspace_local"&&t.localUser)return`${t.name} 本地(${t.localUser})`;const s=t.source==="client_declared"?"（客户端声明）":"";return`项目: ${t.name}${s}`}return t.label}function Ie(e){const t=ne(e,"requestPurpose"),s=typeof t=="object"?t?.kind||t?.type||t?.name:t;if(!s)return null;if(s==="standard")return"主请求";if(s==="websearch")return"WebSearch";if(s==="webfetch")return"WebFetch";const n=ne(e,"requestKind");return n!=null?ot(n):lt(e)}function Ht(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!G(e))return"不适用（通知事件）";if(k(e))return"进行中";const t=G(e);return t==="succeeded"?"成功":t==="failed"?"失败":t==="cancelled"?e.failureKind==="client_disconnected"?"连接中断":"已取消":ct(e).replace(/\s*\([^)]*\)\s*/g,"")}function Gt(e){return String(e?.kind||"").toLowerCase()==="notify"&&!G(e)?"不适用（通知事件）":k(e)?"进行中":ne(e,"phase")==="completed"||G(e)?"已完成":ut(e).replace(/\s*\([^)]*\)\s*/g,"")}function Le(e){return String(e?.kind||"").toLowerCase()==="notify"&&!G(e)?"不适用（通知事件）":`${Me(e)} · ${Gt(e)}`}function ie(e){return[Be(e),Ce(e.kind),Ae(e),ae(e)?`代理: ${ae(e)}`:null,Ie(e),e.sourceIP].filter(Boolean).join(" · ")}function Oe({event:e,compact:t=!1}){return i.jsx("span",{className:`telemetry-outcome-text ${at(e)}${t?" is-compact":""}`,children:Ht(e)})}function De({event:e,live:t=!1}){const s=ie(e),n=Be(e),a=e.kind==="notify"?[]:it(e);return i.jsxs("span",{className:t?"telemetry-live-request":"telemetry-primary-cell",children:[i.jsx("span",{className:`mono-cell${t?" telemetry-live-time":" telemetry-event-time"}`,children:we(e.timestamp)}),n&&i.jsx("span",{className:"telemetry-event-meta",title:n,children:n}),i.jsx("span",{className:`telemetry-event-meta${t?" telemetry-live-request-summary":""}`,title:s,children:[Ce(e.kind),Ae(e),ae(e),Ie(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&i.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[i.jsx(W,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function Ve({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{children:e.hookEvent||"通知"}):i.jsxs("span",{className:t?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[i.jsx("strong",{className:"telemetry-event-model",children:Ue(e)}),i.jsx("span",{className:"telemetry-route-meta",children:Te(e)})]})}function ke({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):i.jsxs("span",{className:t?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[i.jsx(Oe,{event:e}),i.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:rt(e)}),i.jsx("span",{className:"telemetry-result-detail",title:Le(e),children:st(e)?Me(e):"HTTP —"})]})}function Fe({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-duration-cell",children:"—"});const t=nt(e).ttfbMS>=5e3;return i.jsx("span",{className:"mono-cell telemetry-duration-cell",title:t?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:Ne(e)})}function oe({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const t=gt(e),s=k(e);return i.jsxs("span",{className:`telemetry-usage-cell${t?"":" is-empty"}`,title:s?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[i.jsx("span",{className:"telemetry-usage-tokens",children:vt(e)}),i.jsxs("span",{className:"telemetry-usage-secondary",children:[i.jsx("span",{children:s&&!t?"上游尚未返回":xt(e)}),(!s||t)&&i.jsx(bt,{event:e}),s&&t&&i.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function qe({event:e,live:t=!1}){const s=je(e)||(e.outcome==="failed"?Ee(e):"");return i.jsxs("span",{className:`${t?"telemetry-live-message":"telemetry-event-message"}${te(e)?" is-warning":G(e)==="failed"?" is-error":""}`,title:s||void 0,children:[te(e)&&i.jsx("strong",{children:"故障转移 · "}),s||(t?"请求进行中，等待最终结果":"-")]})}function Xt({events:e,selectedEventID:t,onSelect:s}){return e?.length?i.jsxs("section",{className:"telemetry-live-group","aria-label":`${I(e.length)} 个进行中请求`,children:[i.jsx(ze,{}),i.jsxs("div",{className:"telemetry-live-columns",children:[i.jsxs("span",{className:"telemetry-live-heading-cell",children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",I(e.length)]}),i.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),i.jsx("span",{"aria-hidden":"true",children:"结果"}),i.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),i.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",i.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),i.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),i.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=t===n.id;return i.jsx("div",{role:"listitem",children:i.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>s(n.id),children:[i.jsx(De,{event:n,live:!0}),i.jsx(Ve,{event:n,live:!0}),i.jsx(ke,{event:n,live:!0}),i.jsx(Fe,{event:n}),i.jsx(oe,{event:n}),i.jsx(qe,{event:n,live:!0})]})},n.id)})})]}):null}function Se({events:e,selectedEventID:t,onSelect:s,loading:n,live:a=!1}){return e?.length?i.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&i.jsx(ze,{}),e.map(r=>{const o=Re(r),c=G(r),m=je(r)||(r.outcome==="failed"?Ee(r):""),h=t===r.id;return i.jsx("article",{role:"listitem",children:i.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${h?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":h,onClick:()=>s(r.id),onKeyDown:d=>{(d.key==="Enter"||d.key===" ")&&(d.preventDefault(),s(r.id))},children:[i.jsxs("div",{className:"telemetry-mobile-heading",children:[i.jsx("span",{className:"mono-cell telemetry-mobile-time",children:we(r.timestamp)}),i.jsx(Oe,{event:r,compact:!0}),i.jsx("span",{className:"mono-cell telemetry-mobile-duration",style:{color:c==="failed"?"var(--status-critical)":void 0},title:eventTTFBMS(r)>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:Ne(r)})]}),i.jsxs("div",{className:"telemetry-mobile-route",children:[i.jsx("strong",{className:"mono-cell",children:r.kind==="notify"?r.hookEvent||"通知":Ue(r)}),r.kind!=="notify"&&i.jsx("small",{children:Te(r)})]}),i.jsxs("div",{className:"telemetry-mobile-usage",children:[i.jsx("span",{className:"telemetry-mobile-status",children:Le(r)}),r.kind!=="notify"&&i.jsx(oe,{event:r})]}),i.jsxs("div",{className:"telemetry-mobile-context",children:[i.jsx("span",{className:"telemetry-mobile-request-summary",title:ie(r),children:ie(r)||o.label}),m&&i.jsxs("p",{className:`telemetry-mobile-message${c==="failed"?" is-error":te(r)?" is-warning":""}`,children:[i.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),te(r)&&i.jsx("strong",{children:"故障转移 · "}),m]})]})]})},r.id)})]}):a?null:i.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:i.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function ii(){const{status:e,runtime:t,runtimeEventDetail:s,config:n,secretStatus:a,toggleProxy:r,autoRefresh:o,runRefreshInterval:c,loadRuntimeEvent:m,addToast:h}=Je(),[d,v]=u.useState("client"),[p,E]=u.useState("desc"),[R,_]=u.useState(null),[x,N]=u.useState({}),C=(l,g)=>N(z=>z[l]===g?z:{...z,[l]:g}),[F,L]=u.useState(null),B=F?.events||[],O=u.useRef(null),X=u.useRef(o);X.current=o;const[D,A]=u.useState("none"),[,q]=u.useState(0),Y=e?.running,H=t?.recentEvents||[],[P,V]=u.useState(null),Q=u.useCallback(l=>h(l,"info"),[h]),f=$t({recentEvents:H,autoRefresh:o,refreshInterval:c,eventKind:d==="all"?"":d,resetGeneration:t?.resetGeneration,onRecovered:Q});u.useEffect(()=>{if(f.loading||D==="none")return;const l=window.setTimeout(()=>A("none"),280);return()=>window.clearTimeout(l)},[f.loading,D]),u.useEffect(()=>{let l=!1;const g=()=>{document.hidden||se.getRuntimeTrends({range:"today",granularity:"hour"}).then(M=>{if(l)return;const ce=Wt(M);V(ce?{status:"ready",...ce}:{status:"error",input:null,output:null,observed:0})}).catch(()=>{l||V(M=>M?.status==="ready"?M:{status:"error",input:null,output:null,observed:0})})};if(g(),!o)return()=>{l=!0};const z=window.setInterval(g,Math.max(15,Number(c)||15)*1e3);return()=>{l=!0,window.clearInterval(z)}},[o,c,t?.resetGeneration]),u.useEffect(()=>{if(!H.some(g=>k(g)))return;const l=window.setInterval(()=>q(g=>g+1),1e3);return()=>window.clearInterval(l)},[H]);const S=u.useMemo(()=>ue(f.liveEvents||[],p),[f.liveEvents,p]),j=u.useMemo(()=>ue(f.persistedEvents||f.events.filter(l=>!k(l)),p),[f.events,f.persistedEvents,p]),T=u.useMemo(()=>{const l=new Map;for(const g of[...H,...f.events])l.set(g.id,re(l.get(g.id),g));return[...l.values()]},[f.events,H]),b=R,w=b&&(s?.event?.id===b?s.event:T.find(l=>l.id===b)||B.find(l=>l.id===b)||j.find(l=>l.id===b)),J=u.useMemo(()=>{const l=new Map;for(const M of[...B,...T])l.set(M.id,re(l.get(M.id),M));const g=[...l.values()],z=et(g,b);return s?.event?.id!==b?z:z.map(M=>M.id===b?s.event:M)},[T,b,B,s]),y=w?de(w):"",$=Math.max(Number(T.find(l=>l.id===b)?.changeSeq||0),Number(B.find(l=>l.id===b)?.changeSeq||0));u.useEffect(()=>{if(L(null),!y||y==="-")return;const l=_t({read:z=>se.getRuntimeRequestChain(y,{signal:z}),onValue:z=>L(M=>me(z,M?.requestID===y?M.events:[])),enabled:()=>X.current&&!document.hidden});O.current=l;const g=()=>l.synchronize();return document.addEventListener("visibilitychange",g),l.refresh(!0),()=>{l.dispose(),O.current===l&&(O.current=null),document.removeEventListener("visibilitychange",g)}},[y,t?.resetGeneration]),u.useEffect(()=>{O.current?.synchronize()},[o]),u.useEffect(()=>{if(!o||document.hidden||!y)return;const l=T.filter(g=>de(g)===y);L(g=>g?.requestID===y?me(g,l):g),O.current?.refresh()},[o,T,y]),u.useEffect(()=>{if(!b){m(null);return}m(b,$).catch(l=>{h(`加载事件详情失败: ${l.message}`,"error")})},[h,b,$,m]);const le=(l,g="内容")=>ht(l,g,h),$e=l=>{_(null);const g=Number(l);Number.isFinite(g)&&g!==f.page&&A(g>f.page?"forward":"backward"),f.setPage(l)},We=l=>{_(null),A("replace"),f.setPageSize(l)},He=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:l=>i.jsx(De,{event:l})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:l=>i.jsx(Ve,{event:l})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:l=>i.jsx(ke,{event:l})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:l=>i.jsx(Fe,{event:l})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:l=>i.jsx(oe,{event:l})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:l=>i.jsx(qe,{event:l})}];return i.jsxs("div",{className:"page-stack run-page",children:[i.jsxs("div",{className:"page-header",children:[i.jsxs("div",{className:"page-title-group",children:[i.jsxs("h1",{className:"page-title",children:[i.jsx(W,{name:"play",size:24,style:{color:"var(--primary)"}}),i.jsx("span",{children:"运行"}),i.jsx(dt,{status:e})]}),i.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),i.jsx("div",{className:"page-actions",children:i.jsxs("button",{type:"button",className:`btn ${Y?"btn-danger":"btn-primary"}`,onClick:r,children:[i.jsx(W,{name:Y?"stop":"play",size:16}),i.jsx("span",{children:Y?"停止运行":"启动运行"})]})})]}),e?.lastError&&i.jsxs("div",{className:"run-error-banner",children:[i.jsx(W,{name:"warning",size:20}),i.jsxs("div",{children:[i.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),i.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:i.jsxs("div",{className:"glass-panel run-hero-panel",children:[i.jsxs("div",{className:"run-hero-head",children:[i.jsxs("div",{className:"run-hero-heading",children:[i.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),i.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(Y?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),i.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),i.jsxs("dl",{className:"run-detail-grid",children:[i.jsxs("div",{className:"run-detail-listener",children:[i.jsx("dt",{children:"监听地址"}),i.jsxs("dd",{className:"run-listener-row",children:[i.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),i.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>le(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[i.jsx(W,{name:"copy",size:14}),i.jsx("span",{children:"复制"})]})]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"配置版本"}),i.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"认证状态"}),i.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"运行时长"}),i.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?tt(e.uptimeSeconds*1e3):"-"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"调度策略"}),i.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),i.jsxs("div",{className:"run-metric-grid",children:[i.jsx(Z,{label:"输入 Token",value:P?.status==="ready"&&P.observed>0&&P.input!=null?he(P.input):"—",detail:_e(P),icon:"file-down",accent:"var(--primary)"}),i.jsx(Z,{label:"输出 Token",value:P?.status==="ready"&&P.observed>0&&P.output!=null?he(P.output):"—",detail:_e(P),icon:"file-up",accent:"var(--status-warning)"}),i.jsx(Z,{label:"可调度入口",value:I(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--primary)"}),i.jsx(Z,{label:"客户端请求",value:I(t?.clientRequests||0),detail:`端到端 · 成功 ${I(t?.clientSuccesses||0)} / 失败 ${I(t?.clientFailures||0)}`,icon:"move-diagonal",accent:"var(--primary)"}),i.jsx(Z,{label:"上游尝试",value:I(t?.upstreamAttempts||0),detail:`含重试 · 故障转移 ${I(t?.failovers||0)}`,icon:"network",accent:"var(--primary)"})]}),i.jsxs("div",{className:"glass-panel run-events-panel",children:[i.jsxs("div",{className:"panel-header",children:[i.jsxs("div",{className:"panel-title-group",children:[i.jsxs("div",{className:"panel-title",children:[i.jsx(W,{name:"activity",size:18,style:{color:"var(--primary)"}}),i.jsx("span",{children:"最近事件与遥测日志"})]}),i.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),i.jsxs("div",{className:"telemetry-toolbar",children:[i.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(l=>i.jsx("button",{type:"button",className:`telemetry-filter-button${d===l.id?" active":""}`,onClick:()=>{_(null),A("replace"),v(l.id)},"aria-pressed":d===l.id,children:l.label},l.id))}),f.mode!=="page"&&i.jsxs("span",{className:"panel-hint",children:[I(j.length)," 条持久事件"]}),i.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{E(l=>l==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${p==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[i.jsx(W,{name:"sort",size:14}),i.jsxs("span",{children:[f.mode==="page"?"本页":"",p==="desc"?"最新优先":"最早优先"]})]})]})]}),i.jsxs("div",{className:"panel-body",style:{padding:0},children:[f.error&&i.jsxs("div",{className:"event-page-error",role:"alert",children:[i.jsxs("span",{children:["读取事件历史失败：",f.error]}),i.jsx("button",{type:"button",className:"btn btn-secondary",onClick:f.retry,children:"重试"})]}),i.jsxs("div",{className:`event-page-stage event-page-stage-${f.loading?"loading":"ready"}`,"data-page":f.page,"data-page-direction":D,"aria-busy":f.loading,children:[f.mode==="page"?i.jsx(ft,{compact:!0,page:f.page,pageSize:f.pageSize,totalCount:f.totalCount,totalPages:f.totalPages,itemCount:j.length,liveItemCount:f.liveEventCount,loading:f.loading,onPageChange:$e,onPageSizeChange:We,ariaLabel:"持久事件分页"}):null,i.jsxs("div",{className:"event-page-content",children:[S.length>0&&i.jsxs("div",{className:"telemetry-live-section",children:[i.jsx("div",{className:"telemetry-live-desktop",children:i.jsx(Xt,{events:S,selectedEventID:w?.id,onSelect:_})}),i.jsxs("div",{className:"telemetry-live-mobile",children:[i.jsxs("div",{className:"telemetry-live-heading",children:[i.jsxs("span",{children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",I(S.length)]}),i.jsx("small",{children:"上游暂计"})]}),i.jsx(Se,{events:S,selectedEventID:w?.id,onSelect:_,live:!0})]})]}),i.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[i.jsx("div",{className:"telemetry-desktop-table",children:i.jsx(mt,{className:"telemetry-table",columns:He,data:j,keyField:"id",ariaLabel:f.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:p,onSortChange:(l,g)=>E(g),onRowClick:l=>_(l.id),activeRowKey:w?.id,emptyText:f.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),i.jsx(Se,{events:j,selectedEventID:w?.id,onSelect:_,loading:f.loading})]}),!w&&(j.length>0||S.length>0)&&i.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"})]})]}),f.mode==="legacy"?i.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),w&&i.jsx(pt,{event:w,chain:J,chainTruncated:F?.truncated,onSelect:_,requestSummary:ie(w),expanded:x,onToggle:C,onCopy:le,loading:s?.event?.id!==w.id&&w.detailsOmitted===!0})]})]})}export{ii as RunPage};
