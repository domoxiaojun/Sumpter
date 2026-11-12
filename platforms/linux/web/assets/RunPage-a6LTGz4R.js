import{m as me,j as t,I as q,r as u,e as z,a as lt,b as _e,c as he,d as we,i as ct,f as ut,g as dt,p as mt,h as fe,k as ht,n as ft,l as pt,u as gt,o as vt,q as xt,s as Re,t as yt,v as Ee,w as pe,x as ge,y as L,z as W,A as ye,B as Z,C as re,D as De,E as ke,F as Fe,G as Ve,H as $e,J as qe,K as We,L as He,M as Ge,N as bt,O as jt,P as Xe,Q as St,R as _t,S as wt,T as Rt,U as Et,V as Nt}from"./index-BS5GiJGM.js";import{S as Ct}from"./StatusBadge-CcK6y1b6.js";import{D as Tt}from"./DataTable-DsiyfUDX.js";import{P as Ne}from"./outputSpeed-C9Z-ZRmV.js";import{E as Mt,e as ve,a as Pt,b as Ut,c as At,C as zt}from"./EventInspector-D-tYS3s-.js";import{c as It}from"./clipboard-BKe_f5Aw.js";import"./requestPolicy-D6HDy5_v.js";function Ce(e,s=[],i=e?.requestID){const n=new Map;for(const r of[...e?.events||[],...s])r.requestID===i&&n.set(r.id,me(n.get(r.id),r));const a=[...n.values()].sort((r,o)=>Number(r.seq||0)-Number(o.seq||0));return{...e,requestID:i,events:a.slice(0,512),truncated:!!(e?.truncated||a.length>512)}}function Bt(e){return e?.events?.some(s=>s.kind==="client"&&["inFlight","in_flight"].includes(s.phase))||!1}function Lt({read:e,onValue:s,enabled:i,interval:n=1e3,schedule:a=setTimeout,unschedule:r=clearTimeout,now:o=Date.now}){let c=!1,f=!0,d,h,p=-1/0;function w(g=!1){if(c||h||!g&&(!f||!i())||d!=null)return;const b=g?0:Math.max(0,n-(o()-p));d=a(()=>{d=void 0,y(g)},b)}async function y(g){if(c||h||!g&&!i())return;const b=new AbortController;h=b,p=o();try{const v=await e(b.signal);if(c||b.signal.aborted||!g&&!i())return;f=Bt(v),s(v)}catch(v){[401,403,404].includes(v?.status)&&(f=!1)}finally{h=void 0,!c&&f&&i()&&(d=a(()=>{d=void 0,y(!1)},n))}}return{refresh:w,synchronize(){i()?w():(d!=null&&r(d),d=void 0)},dispose(){c=!0,d!=null&&r(d),h?.abort()}}}function ae({label:e,value:s,detail:i,icon:n,accent:a="var(--primary)"}){return t.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[t.jsxs("div",{className:"metric-head",children:[t.jsx("span",{className:"metric-label",children:e}),n&&t.jsx("div",{className:"metric-icon-box",children:t.jsx(q,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),t.jsx("div",{className:"metric-value",children:s}),i&&t.jsx("div",{className:"metric-detail",children:i})]})}const Ot=`#version 300 es
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
}`,Te=1920*1080*4;let Dt=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=Vt();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(s,i,n,a,r=0,o=0,c=2,f=Te,d=[]){if(s?.nodeType===1)this.parentElement=s;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=s.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const w=this.ownerDocument.createElement("style");w.innerHTML=Ft,w.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(w)}const h=this.ownerDocument.createElement("canvas");this.canvasElement=h,this.parentElement.prepend(h),this.fragmentShader=i,this.providedUniforms=n,this.mipmaps=d,this.currentFrame=o,this.minPixelRatio=c,this.maxPixelCount=f;const p=h.getContext("webgl2",a);if(!p)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=p,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(r),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const s=kt(this.gl,Ot,this.fragmentShader);s&&(this.program=s)};setupPositionAttribute=()=>{const s=this.gl.getAttribLocation(this.program,"a_position"),i=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,i);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(s),this.gl.vertexAttribPointer(s,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const s={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([i,n])=>{if(s[i]=this.gl.getUniformLocation(this.program,i),n instanceof HTMLImageElement){const a=`${i}AspectRatio`;s[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=s};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([s])=>{if(s?.borderBoxSize[0]){const i=s.devicePixelContentBoxSize?.[0];i!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=i.inlineSize,this.parentDevicePixelHeight=i.blockSize),this.parentWidth=s.borderBoxSize[0].inlineSize,this.parentHeight=s.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const s=this.ownerDocument.defaultView;s?.IntersectionObserver&&(this.intersectionObserver=new s.IntersectionObserver(([i])=>{this.isInViewport=i?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let s=0,i=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const h=Math.max(1,this.minPixelRatio/n);s=this.parentDevicePixelWidth*h*a,i=this.parentDevicePixelHeight*h*a}else{let h=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const p=$t(this.ownerDocument);h*=Math.max(1,p)}s=Math.round(this.parentWidth)*h,i=Math.round(this.parentHeight)*h}const r=Math.sqrt(this.maxPixelCount)/Math.sqrt(s*i),o=Math.min(1,r),c=Math.round(s*o),f=Math.round(i*o),d=c/Math.round(this.parentWidth);(this.canvasElement.width!==c||this.canvasElement.height!==f||this.renderScale!==d)&&(this.renderScale=d,this.canvasElement.width=c,this.canvasElement.height=f,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=s=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const i=s-this.lastRenderTime;this.lastRenderTime=s,this.currentSpeed!==0&&(this.currentFrame+=i*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(s,i)=>{if(!i.complete||i.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${s} must be fully loaded`);const n=this.textures.get(s);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(s)||this.textureUnitMap.set(s,this.textureUnitMap.size);const a=this.textureUnitMap.get(s);this.gl.activeTexture(this.gl.TEXTURE0+a);const r=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,r),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,i),this.mipmaps.includes(s)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||r===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(s,r);const c=this.uniformLocations[s];if(c){this.gl.uniform1i(c,a);const f=`${s}AspectRatio`,d=this.uniformLocations[f];if(d){const h=i.naturalWidth/i.naturalHeight;this.gl.uniform1f(d,h)}}};areUniformValuesEqual=(s,i)=>s===i?!0:Array.isArray(s)&&Array.isArray(i)&&s.length===i.length?s.every((n,a)=>this.areUniformValuesEqual(n,i[a])):!1;setUniformValues=s=>{this.gl.useProgram(this.program),Object.entries(s).forEach(([i,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[i],a))return;this.uniformCache[i]=a;const r=this.uniformLocations[i];if(!r){console.warn(`Uniform location for ${i} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(i,n);else if(Array.isArray(n)){let o=null,c=null;if(n[0]!==void 0&&Array.isArray(n[0])){const f=n[0].length;if(n.every(d=>d.length===f))o=n.flat(),c=f;else{console.warn(`All child arrays must be the same length for ${i}`);return}}else o=n,c=o.length;switch(c){case 2:this.gl.uniform2fv(r,o);break;case 3:this.gl.uniform3fv(r,o);break;case 4:this.gl.uniform4fv(r,o);break;case 9:this.gl.uniformMatrix3fv(r,!1,o);break;case 16:this.gl.uniformMatrix4fv(r,!1,o);break;default:console.warn(`Unsupported uniform array length: ${c}`)}}else typeof n=="number"?this.gl.uniform1f(r,n):typeof n=="boolean"?this.gl.uniform1i(r,n?1:0):console.warn(`Unsupported uniform type for ${i}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=s=>{this.currentFrame=s,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(s=1)=>{this.speed=s,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=s=>{this.currentSpeed=s,this.rafId===null&&s!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&s===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(s=Te)=>{this.maxPixelCount=s,this.handleResize()};setMinPixelRatio=(s=2)=>{this.minPixelRatio=s,this.handleResize()};setUniforms=s=>{this.setUniformValues(s),this.providedUniforms={...this.providedUniforms,...s},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(s=>{this.gl.deleteTexture(s)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function Me(e,s,i){const n=e.createShader(s);return n?(e.shaderSource(n,i),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function kt(e,s,i){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(s=s.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),i=i.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const r=Me(e,e.VERTEX_SHADER,s),o=Me(e,e.FRAGMENT_SHADER,i);if(!r||!o)return null;const c=e.createProgram();return c?(e.attachShader(c,r),e.attachShader(c,o),e.linkProgram(c),e.getProgramParameter(c,e.LINK_STATUS)?(e.detachShader(c,r),e.detachShader(c,o),e.deleteShader(r),e.deleteShader(o),c):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(c)),e.deleteProgram(c),e.deleteShader(r),e.deleteShader(o),null)):null}const Ft=`@layer paper-shaders {
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
}`;function Vt(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function $t(e){const s=visualViewport?.scale??1,i=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=s*i+n,r=outerWidth/a,o=Math.round(100*r);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:r}const qt={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},Wt={none:0,contain:1,cover:2},Ht=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,Gt=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,Xt=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,Pe={maxColorCount:10},Yt=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${Pe.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${Ht}
${Gt}
${Xt}

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

  for (int i = 0; i < ${Pe.maxColorCount}; i++) {
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
`;function Kt(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:ie;if(typeof e!="string")return ie;let s,i,n,a=1;if(e.startsWith("#"))[s,i,n,a]=Qt(e);else if(e.startsWith("rgb")){const r=Zt(e);if(r===null)return ie;[s,i,n,a]=r}else if(e.startsWith("hsl")){const r=Jt(e);if(r===null)return ie;[s,i,n,a]=es(r)}else return console.error("Unsupported color format",e),ie;return[le(s,0,1),le(i,0,1),le(n,0,1),le(a,0,1)]}function Qt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(r=>r+r).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),ie;const s=parseInt(e.slice(0,2),16)/255,i=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[s,i,n,a]}function Zt(e){const s=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return s?[parseInt(s[1]??"0")/255,parseInt(s[2]??"0")/255,parseInt(s[3]??"0")/255,s[4]===void 0?1:parseFloat(s[4])]:null}function Jt(e){const s=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return s?[parseInt(s[1]??"0"),parseInt(s[2]??"0"),parseInt(s[3]??"0"),s[4]===void 0?1:parseFloat(s[4])]:null}function es(e){const[s,i,n,a]=e,r=s/360,o=i/100,c=n/100;let f,d,h;if(i===0)f=d=h=c;else{const p=(g,b,v)=>(v<0&&(v+=1),v>1&&(v-=1),v<.16666666666666666?g+(b-g)*6*v:v<.5?b:v<.6666666666666666?g+(b-g)*(.6666666666666666-v)*6:g),w=c<.5?c*(1+o):c+o-c*o,y=2*c-w;f=p(y,w,r+1/3),d=p(y,w,r),h=p(y,w,r-1/3)}return[f,d,h,a]}const le=(e,s,i)=>Math.min(Math.max(e,s),i),ie=[.5,.5,.5,1],ts="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function ss(e){const s=u.useRef(void 0),i=u.useCallback(n=>{const a=e.map(r=>{if(r!=null){if(typeof r=="function"){const o=r,c=o(n);return typeof c=="function"?c:()=>{o(null)}}return r.current=n,()=>{r.current=null}}});return()=>{a.forEach(r=>r?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{s.current&&(s.current(),s.current=void 0),n!=null&&(s.current=i(n))},e)}function Ue(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const s=e.naturalWidth/e.naturalHeight;e.width=Math.round(s>1?1024*s:1024),e.height=Math.round(s>1?1024:1024/s)}}async function Ae(e){const s={},i=[],n=r=>{try{return r.startsWith("/")||new URL(r),!0}catch{return!1}},a=r=>{try{return r.startsWith("/")?!1:new URL(r,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([r,o])=>{if(typeof o=="string"){const c=o||ts;if(!n(c)){console.warn(`Uniform "${r}" has invalid URL "${c}". Skipping image loading.`);return}const f=new Promise((d,h)=>{const p=new Image;a(c)&&(p.crossOrigin="anonymous"),p.onload=()=>{Ue(p),s[r]=p,d()},p.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${c}`),h()},p.src=c});i.push(f)}else if(o instanceof HTMLImageElement){const c=o.decode().then(()=>{Ue(o),s[r]=o});i.push(c)}else s[r]=o}),await Promise.all(i),s}const Ye=u.forwardRef(function({fragmentShader:s,uniforms:i,webGlContextAttributes:n,speed:a=0,frame:r=0,width:o,height:c,minPixelRatio:f,maxPixelCount:d,mipmaps:h,style:p,...w},y){const[g,b]=u.useState(!1),v=u.useRef(null),_=u.useRef(null),C=u.useRef(n);u.useEffect(()=>((async()=>{const I=await Ae(i);v.current&&!_.current&&(_.current=new Dt(v.current,s,I,C.current,a,r,f,d,h),b(!0))})(),()=>{_.current?.dispose(),_.current=null}),[s]),u.useEffect(()=>{let D=!1;return(async()=>{const H=await Ae(i);D||_.current?.setUniforms(H)})(),()=>{D=!0}},[i,g]),u.useEffect(()=>{_.current?.setSpeed(a)},[a,g]),u.useEffect(()=>{_.current?.setMaxPixelCount(d)},[d,g]),u.useEffect(()=>{_.current?.setMinPixelRatio(f)},[f,g]),u.useEffect(()=>{_.current?.setFrame(r)},[r,g]);const O=ss([v,y]);return t.jsx("div",{ref:O,style:o!==void 0||c!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof c=="string"&&isNaN(+c)===!1?+c:c,...p}:p,...w})});Ye.displayName="ShaderMount";function is(e,s){if(Object.keys(e).length!==Object.keys(s).length)return!1;for(const i in e){if(i==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(s.colors);if(!n||!a){if(Object.is(e.colors,s.colors)===!1)return!1;continue}if(e.colors?.length!==s.colors?.length||!e.colors?.every((r,o)=>r===s.colors?.[o]))return!1;continue}if(Object.is(e[i],s[i])===!1)return!1}return!0}const P={params:{...qt,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},rs=u.memo(function({speed:s=P.params.speed,frame:i=P.params.frame,colors:n=P.params.colors,distortion:a=P.params.distortion,swirl:r=P.params.swirl,grainMixer:o=P.params.grainMixer,grainOverlay:c=P.params.grainOverlay,fit:f=P.params.fit,rotation:d=P.params.rotation,scale:h=P.params.scale,originX:p=P.params.originX,originY:w=P.params.originY,offsetX:y=P.params.offsetX,offsetY:g=P.params.offsetY,worldWidth:b=P.params.worldWidth,worldHeight:v=P.params.worldHeight,..._}){const C={u_colors:n.map(Kt),u_colorsCount:n.length,u_distortion:a,u_swirl:r,u_grainMixer:o,u_grainOverlay:c,u_fit:Wt[f],u_rotation:d,u_scale:h,u_offsetX:y,u_offsetY:g,u_originX:p,u_originY:w,u_worldWidth:b,u_worldHeight:v};return t.jsx(Ye,{..._,speed:s,frame:i,fragmentShader:Yt,uniforms:C})},is);function Ke(){const[e,s]=u.useState(()=>document.documentElement.dataset.theme==="light"),[i,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const r=new MutationObserver(()=>s(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),c=()=>n(o.matches);return r.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",c),()=>{r.disconnect(),o.removeEventListener("change",c)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return t.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:t.jsx(rs,{className:"telemetry-live-mesh",colors:a,speed:i?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function ze(e,s,i=""){if(!s)return[];const n=new Map((s.events||[]).map(r=>[r.id,r])),a=Math.min(...(s.events||[]).map(r=>Number(r.seq||0)));return(e||[]).filter(r=>{if(i&&r.kind!==i||z(r))return!1;if(s.snapshotChangeSeq!=null)return Number(r.changeSeq||0)>Number(s.snapshotChangeSeq);const o=n.get(r.id);return o?Number(r.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(r.seq||0)>=a})}function ns(e,s,i){const n=new Map((e||[]).filter(a=>!z(a)).map(a=>[a.id,a]));for(const a of s||[]){const r=n.get(a.id);(!r||Number(a.changeSeq||0)>=Number(r.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,r)=>Number(r.timestamp||0)-Number(a.timestamp||0)||Number(r.seq||0)-Number(a.seq||0)).slice(0,i)}function Ie(e,s){return!s||e?.kind===s}function Be(e,s){return!s||pt(e)===s}function as({recentEvents:e=[],eventKind:s="",outcome:i="",resetGeneration:n=0,onRecovered:a,autoRefresh:r=!0,refreshInterval:o=15,range:c="all",scopeKey:f=""}={}){const[d,h]=u.useState(1),[p,w]=u.useState(lt),[y,g]=u.useState(null),[b,v]=u.useState("loading"),[_,C]=u.useState(!0),[O,D]=u.useState(!1),I=u.useRef(!1),H=u.useRef(null),[ee,G]=u.useState(null),[J,te]=u.useState(0),V=u.useRef(0),M=u.useRef(null),X=u.useRef(null),se=`${c}:${f}:${s||"all"}:${i||"all"}:${p}:${Number(n||0)}`,Y=_e(c,y),N=Y?.filters?.from??he(c).from,K=Y?.filters?.to,F=u.useMemo(()=>e.filter(j=>we(j,{from:N,to:K})),[e,N,K]);u.useEffect(()=>{I.current=!1,H.current=null,M.current=null,v("loading"),C(!0),g(null),G(null),h(1)},[se]),u.useEffect(()=>{const j=++V.current,S=new AbortController,R=_e(c,M.current),T=M.current&&!R?1:Math.max(1,Number(d)||1),U=I.current;I.current=!1,C(!U),D(!0),G(null);const ne=async(E=!0)=>{const Q=await fe.getRuntimeEventPage({page:E?T:1,pageSize:p,kind:s||void 0,outcome:i||void 0,...ht(he(c),E?R:null),snapshotSeq:E?R?.snapshotSeq:void 0,snapshotChangeSeq:E?R?.snapshotChangeSeq:void 0,historyGeneration:E?R?.historyGeneration:void 0,signal:S.signal});return ft(Q,{page:E?T:1,pageSize:p})};return(async()=>{try{let E;try{E=await ne(!0)}catch(Q){if(!ct(Q))throw Q;E=await ne(!1),j===V.current&&(h(1),a?.(ut(Q)))}if(j!==V.current)return;if(!E){v("legacy"),g(null);return}M.current=E,g(E),v("page"),E.page!==d&&h(E.page)}catch(E){if(j!==V.current||E?.name==="AbortError")return;if([400,404,405].includes(Number(E?.status))){v("legacy"),g(null);return}G(E?.message||"加载事件历史失败")}finally{j===V.current&&(C(!1),D(!1))}})(),()=>S.abort()},[c,f,s,i,a,d,p,J,n]);const ue=u.useCallback(j=>{const S=dt(j,y?.totalPages||1);I.current=!1,h(S)},[y?.totalPages]),de=u.useCallback(j=>{I.current=!1;const S=mt(j);M.current=null,w(S),h(1)},[]),m=u.useMemo(()=>b==="page"&&d!==1||i?[]:F.filter(j=>Ie(j,s)&&z(j)),[s,i,b,d,F]),$=u.useMemo(()=>{const j=(b==="page"&&d===1?ze(F,y,s):[]).filter(T=>Be(T,i)),S=b==="page"?ns(y?.events||[],j,p):F,R=new Set(m.map(T=>T.id));return S.filter(T=>Ie(T,s)&&we(T,{from:N,to:K})&&Be(T,i)&&!R.has(T.id)&&!z(T))},[s,i,m,b,d,p,F,y,N,K]),B=u.useMemo(()=>[...m,...$],[m,$]);return u.useEffect(()=>{if(b!=="page"||d!==1||O||M.current==null)return;const j=ze(F,M.current,s);if(!j.length||X.current!=null)return;const S=Math.max(...j.map(U=>Number(U.changeSeq||0))),R=`${s}:${M.current.snapshotChangeSeq??"legacy"}:${S}`,T=H.current;if(!(T?.signature===R&&Date.now()-T.at<5e3))return X.current=window.setTimeout(()=>{X.current=null,H.current={signature:R,at:Date.now()},I.current=!0,M.current=null,te(U=>U+1)},220),()=>{X.current!=null&&(window.clearTimeout(X.current),X.current=null)}},[s,O,b,d,F]),u.useEffect(()=>{if(!r||d!==1||b!=="page")return;const j=window.setInterval(()=>{document.hidden||O||(I.current=!0,M.current=null,te(S=>S+1))},Math.max(1,Number(o)||15)*1e3);return()=>window.clearInterval(j)},[r,d,b,o,O]),{mode:b,loading:_,error:ee,events:B,liveEvents:m,persistedEvents:$,liveEventCount:m.length,page:d,pageSize:p,totalCount:y?.totalCount??B.length,totalPages:y?.totalPages??(B.length?1:0),hasNext:!!y?.hasNext,hasPrevious:!!y?.hasPrevious,snapshotSeq:y?.snapshotSeq??null,snapshotChangeSeq:y?.snapshotChangeSeq??null,historyGeneration:y?.historyGeneration??null,setPage:ue,setPageSize:de,retry:()=>{I.current=!1,te(j=>j+1)}}}const xe="today";function os(e=new Date){return{range:xe,granularity:"hour",filters:he(xe,e)}}function ls(e){const s=e?.totals,i=s?.tokens;if(!i||typeof i!="object")return null;const n=r=>r!=null&&r!==""&&Number.isFinite(Number(r))&&Number(r)>=0?Number(r):null,a=n(i.observedRequests??i.observed_requests);return a==null?null:{input:n(i.inputTokens??i.input_tokens),output:n(i.outputTokens??i.output_tokens),observed:a,clientRequests:n(s.clientRequests??s.client_requests),clientSuccesses:n(s.clientSuccesses??s.client_successes),clientFailures:n(s.clientFailures??s.client_failures),upstreamAttempts:n(s.upstreamAttempts??s.upstream_attempts),failovers:n(s.failovers)}}function Le(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天 · ${L(e.observed)} 个请求有用量`:"今天暂无可用用量":"正在读取今天的用量"}const oe=e=>e==null?"—":L(e);function Qe(e){const s=De(e);if(!s?.applicable)return null;if(s.name&&s.name!=="unidentified_project"){if(s.source==="workspace_local"&&s.localUser)return`${s.name} 本地(${s.localUser})`;const i=s.source==="client_declared"?"（客户端声明）":"";return`项目: ${s.name}${i}`}return s.label}function Ze(e){const s=Z(e,"requestPurpose"),i=typeof s=="object"?s?.kind||s?.type||s?.name:s;if(!i)return null;if(i==="standard")return"主请求";if(i==="websearch")return"WebSearch";if(i==="webfetch")return"WebFetch";const n=Z(e,"requestKind");return n!=null?wt(n):Rt(e)}function cs(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!W(e))return"不适用（通知事件）";if(z(e))return"进行中";const s=W(e);return s==="succeeded"?"成功":s==="failed"?"失败":s==="cancelled"?e.failureKind==="client_disconnected"?"连接中断":"已取消":Et(e).replace(/\s*\([^)]*\)\s*/g,"")}function us(e){return String(e?.kind||"").toLowerCase()==="notify"&&!W(e)?"不适用（通知事件）":z(e)?"进行中":Z(e,"phase")==="completed"||W(e)?"已完成":Nt(e).replace(/\s*\([^)]*\)\s*/g,"")}function Je(e){return String(e?.kind||"").toLowerCase()==="notify"&&!W(e)?"不适用（通知事件）":`${Xe(e)} · ${us(e)}`}function ce(e){return[Qe(e),He(e.kind),Ge(e),ve(e)?`代理: ${ve(e)}`:null,Ze(e),e.sourceIP].filter(Boolean).join(" · ")}function et({event:e,compact:s=!1}){const i=_t(e),n=z(e)||i==="live";return t.jsxs("span",{className:`telemetry-outcome-text ${i}${s?" is-compact":""}`,children:[n&&t.jsx("i",{className:"telemetry-live-pill-dot","aria-hidden":"true"}),cs(e)]})}function tt({event:e,live:s=!1}){const i=ce(e),n=Qe(e),a=e.kind==="notify"?[]:bt(e);return t.jsxs("span",{className:s?"telemetry-live-request":"telemetry-primary-cell",children:[t.jsxs("span",{className:"telemetry-event-head-row",children:[t.jsx("span",{className:`mono-cell${s?" telemetry-live-time":" telemetry-event-time"}`,children:Ve(e.timestamp)}),n&&t.jsx("span",{className:"telemetry-event-project-pill",title:n,children:n})]}),t.jsx("span",{className:`telemetry-event-meta${s?" telemetry-live-request-summary":""}`,title:i,children:[He(e.kind),Ge(e),ve(e),Ze(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&t.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[t.jsx(q,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function st({event:e,live:s=!1}){if(e.kind==="notify")return t.jsx("span",{children:e.hookEvent||"通知"});const i=qe(e),n=String(Z(e,"clientModel","client_model")||"").trim(),a=n&&i!=="—"&&n!==i,r=Number(Z(e,"attemptNumber","attempt_number")||0),o=r>1||re(e);return t.jsxs("span",{className:s?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[t.jsxs("span",{className:"telemetry-route-model-line",children:[t.jsx("strong",{className:"telemetry-event-model",children:i}),a&&t.jsxs("span",{className:"telemetry-route-mapped-badge mono-cell",title:`客户端模型 ${n} → 路由模型 ${i}`,children:["← ",n]})]}),t.jsxs("span",{className:"telemetry-route-meta",children:[t.jsx("span",{children:We(e)}),o&&t.jsx("span",{className:"telemetry-route-retry-badge",children:r>1?`重试 #${r}`:"故障转移"})]})]})}function it({event:e,live:s=!1}){return e.kind==="notify"?t.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):t.jsxs("span",{className:s?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[t.jsxs("span",{className:"telemetry-result-primary-row",children:[t.jsx(et,{event:e}),t.jsx("span",{className:"telemetry-result-detail mono-cell",title:Je(e),children:jt(e)?Xe(e):"HTTP —"})]}),t.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:St(e)})]})}function rt({event:e}){if(e.kind!=="client"||z(e))return null;const s=Z(e,"outputTokensPerSecond","output_tokens_per_second"),i=s==null?NaN:Number(s),n=W(e)==="succeeded"&&Number.isFinite(i)&&i>=0;return t.jsx("span",{className:"telemetry-speed-pill",title:n?"根据上游有效输出区间估算的速度":"无法可靠测量输出速度",children:n?`${i.toFixed(1)} tok/s`:"—"})}function nt({event:e}){if(e.kind==="notify")return t.jsx("span",{className:"telemetry-duration-cell",children:"—"});const s=z(e),{ttfbMS:i,durationMS:n}=ye(e),a=i>=5e3,r=Number.isFinite(n)&&n>0?n:Number.isFinite(i)&&i>0?i:0,o=Number.isFinite(i)&&i>0&&r>0?Math.min(100,Math.max(10,Math.round(i/r*100))):0,c=r>0?Math.max(0,100-o):0;return t.jsxs("span",{className:"mono-cell telemetry-duration-cell telemetry-inline-latency",title:a?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:[t.jsxs("span",{className:"telemetry-inline-latency-top",children:[t.jsx("span",{className:`telemetry-inline-latency-nums${a?" is-slow":""}${s?" is-live":""}`,children:$e(e)}),t.jsx(rt,{event:e})]}),!s&&r>0&&t.jsxs("span",{className:"telemetry-latency-track","aria-hidden":"true",children:[o>0&&t.jsx("i",{className:`telemetry-latency-ttfb${a?" is-slow":""}`,style:{width:`${o}%`}}),c>0&&t.jsx("i",{className:"telemetry-latency-stream",style:{width:`${c}%`}})]})]})}function ds({events:e}){if(!e?.length)return null;const s=e.filter(v=>String(v?.kind||"").toLowerCase()!=="notify");if(!s.length)return null;let i=0,n=0,a=0,r=0,o=0,c=0,f=0,d=0,h=0;for(const v of s){const _=W(v);_==="succeeded"?i+=1:_==="failed"?n+=1:_==="cancelled"&&(a+=1);const C=ye(v);Number.isFinite(C.ttfbMS)&&C.ttfbMS>=0&&(r+=C.ttfbMS,o+=1),Number.isFinite(C.durationMS)&&C.durationMS>=0&&(c+=C.durationMS,f+=1);const O=Number(Z(v,"inputTokens","input_tokens")||0),D=Number(Z(v,"outputTokens","output_tokens")||0);Number.isFinite(O)&&O>0&&(d+=O),Number.isFinite(D)&&D>0&&(d+=D),re(v)&&(h+=1)}const p=i+n+a,w=p>0?`${(i/p*100).toFixed(1)}%`:"—",y=p>0?Math.round(i/p*100):0,g=p>0?Math.round(n/p*100):0,b=p>0?Math.max(0,100-y-g):0;return t.jsxs("div",{className:"telemetry-overview-strip",role:"region","aria-label":"本页事件遥测摘要",children:[t.jsxs("div",{className:"telemetry-overview-item",children:[t.jsxs("div",{className:"telemetry-overview-head",children:[t.jsx("span",{className:"telemetry-overview-label",children:"本页请求健康度"}),t.jsx("strong",{className:"mono-cell telemetry-overview-value",children:w})]}),t.jsxs("div",{className:"telemetry-health-bar","aria-hidden":"true",children:[y>0&&t.jsx("i",{className:"telemetry-health-seg-good",style:{width:`${y}%`}}),g>0&&t.jsx("i",{className:"telemetry-health-seg-fail",style:{width:`${g}%`}}),b>0&&t.jsx("i",{className:"telemetry-health-seg-warn",style:{width:`${b}%`}})]}),t.jsxs("div",{className:"telemetry-overview-sub",children:[t.jsxs("span",{children:["成功 ",L(i)]}),t.jsxs("span",{children:["失败 ",L(n)," · 取消 ",L(a)]})]})]}),t.jsxs("div",{className:"telemetry-overview-item",children:[t.jsxs("div",{className:"telemetry-overview-head",children:[t.jsx("span",{className:"telemetry-overview-label",children:"本页首字节均值"}),t.jsx("strong",{className:"mono-cell telemetry-overview-value",children:o>0?pe(r/o):"—"})]}),t.jsxs("div",{className:"telemetry-overview-sub",children:[t.jsx("span",{children:"蓝青段代表首字节响应"}),t.jsxs("span",{children:[L(o)," 条已采样"]})]})]}),t.jsxs("div",{className:"telemetry-overview-item",children:[t.jsxs("div",{className:"telemetry-overview-head",children:[t.jsx("span",{className:"telemetry-overview-label",children:"本页完成耗时均值"}),t.jsx("strong",{className:"mono-cell telemetry-overview-value",children:f>0?pe(c/f):"—"})]}),t.jsxs("div",{className:"telemetry-overview-sub",children:[t.jsx("span",{children:"紫绿段代表流式生成"}),t.jsxs("span",{children:[L(f)," 条已采样"]})]})]}),t.jsxs("div",{className:"telemetry-overview-item",children:[t.jsxs("div",{className:"telemetry-overview-head",children:[t.jsx("span",{className:"telemetry-overview-label",children:"本页 Token 与切换"}),t.jsx("strong",{className:"mono-cell telemetry-overview-value",children:d>0?ge(d):"—"})]}),t.jsxs("div",{className:"telemetry-overview-sub",children:[t.jsx("span",{children:"本页输入+输出累计"}),t.jsxs("span",{children:["故障转移 ",L(h)," 次"]})]})]})]})}function be({event:e}){if(e.kind==="notify")return t.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const s=Pt(e),i=z(e);return t.jsxs("span",{className:`telemetry-usage-cell${s?"":" is-empty"}`,title:i?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[t.jsx("span",{className:"telemetry-usage-tokens",children:Ut(e)}),t.jsxs("span",{className:"telemetry-usage-secondary",children:[t.jsx("span",{children:i&&!s?"上游尚未返回":At(e)}),(!i||s)&&t.jsx(zt,{event:e}),i&&s&&t.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function at({event:e,live:s=!1}){const i=s||z(e),n=ke(e)||(e.outcome==="failed"?Fe(e):"");return t.jsxs("span",{className:`${i?"telemetry-live-message":"telemetry-event-message"}${re(e)?" is-warning":W(e)==="failed"?" is-error":""}`,title:n||void 0,children:[re(e)&&t.jsx("strong",{children:"故障转移 · "}),n||(i?"请求进行中，等待最终结果":"-")]})}function ms({events:e,selectedEventID:s,onSelect:i}){return e?.length?t.jsxs("section",{className:"telemetry-live-group","aria-label":`${L(e.length)} 个进行中请求`,children:[t.jsx(Ke,{}),t.jsxs("div",{className:"telemetry-live-columns",children:[t.jsxs("span",{className:"telemetry-live-heading-cell",children:[t.jsx("i",{"aria-hidden":"true"}),"进行中 · ",L(e.length)]}),t.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),t.jsx("span",{"aria-hidden":"true",children:"结果"}),t.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),t.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",t.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),t.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),t.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=s===n.id;return t.jsx("div",{role:"listitem",children:t.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>i(n.id),children:[t.jsx(tt,{event:n,live:!0}),t.jsx(st,{event:n,live:!0}),t.jsx(it,{event:n,live:!0}),t.jsx(nt,{event:n}),t.jsx(be,{event:n}),t.jsx(at,{event:n,live:!0})]})},n.id)})})]}):null}function Oe({events:e,selectedEventID:s,onSelect:i,loading:n,live:a=!1}){return e?.length?t.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&t.jsx(Ke,{}),e.map(r=>{const o=De(r),c=W(r),f=ke(r)||(r.outcome==="failed"?Fe(r):""),d=s===r.id;return t.jsx("article",{role:"listitem",children:t.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${d?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":d,onClick:()=>i(r.id),onKeyDown:h=>{(h.key==="Enter"||h.key===" ")&&(h.preventDefault(),i(r.id))},children:[t.jsxs("div",{className:"telemetry-mobile-heading",children:[t.jsx("span",{className:"mono-cell telemetry-mobile-time",children:Ve(r.timestamp)}),t.jsx(et,{event:r,compact:!0}),t.jsxs("span",{className:`mono-cell telemetry-mobile-duration${a||z(r)?" is-live":""}`,style:{color:c==="failed"?"var(--status-critical)":void 0},title:ye(r).ttfbMS>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:[t.jsx("span",{children:$e(r)}),t.jsx(rt,{event:r})]})]}),t.jsxs("div",{className:"telemetry-mobile-route",children:[t.jsx("strong",{className:"mono-cell",children:r.kind==="notify"?r.hookEvent||"通知":qe(r)}),r.kind!=="notify"&&t.jsx("small",{children:We(r)})]}),t.jsxs("div",{className:"telemetry-mobile-usage",children:[t.jsx("span",{className:"telemetry-mobile-status",children:Je(r)}),r.kind!=="notify"&&t.jsx(be,{event:r})]}),t.jsxs("div",{className:"telemetry-mobile-context",children:[t.jsx("span",{className:"telemetry-mobile-request-summary",title:ce(r),children:ce(r)||o.label}),f&&t.jsxs("p",{className:`telemetry-mobile-message${c==="failed"?" is-error":re(r)?" is-warning":""}`,children:[t.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),re(r)&&t.jsx("strong",{children:"故障转移 · "}),f]})]})]})},r.id)})]}):a?null:t.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:t.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function js(){const{status:e,runtime:s,runtimeEventDetail:i,config:n,secretStatus:a,toggleProxy:r,autoRefresh:o,runRefreshInterval:c,loadRuntimeEvent:f,addToast:d}=gt(),[h,p]=u.useState("client"),[w,y]=u.useState(""),[g,b]=u.useState("desc"),[v,_]=u.useState(null),C=u.useCallback(l=>{_(x=>x===l?null:l)},[]),[O,D]=u.useState({}),I=(l,x)=>D(k=>k[l]===x?k:{...k,[l]:x}),[H,ee]=u.useState(null),G=H?.events||[],J=u.useRef(null),te=u.useRef(o);te.current=o;const[V,M]=u.useState("none"),[,X]=u.useState(0),se=e?.running,Y=s?.recentEvents||[],[N,K]=u.useState(null),[F,ue]=u.useState(()=>vt().from);u.useEffect(()=>xt(ue),[]),u.useEffect(()=>{_(null),ee(null)},[F]);const de=u.useCallback(l=>d(l,"info"),[d]),m=as({range:xe,scopeKey:F,recentEvents:Y,autoRefresh:o,refreshInterval:c,eventKind:h==="all"?"":h,outcome:w,resetGeneration:s?.resetGeneration,onRecovered:de});u.useEffect(()=>{if(m.loading||V==="none")return;const l=window.setTimeout(()=>M("none"),280);return()=>window.clearTimeout(l)},[m.loading,V]),u.useEffect(()=>{let l=!1;K(null);const x=()=>{document.hidden||fe.getRuntimeTrends(os()).then(A=>{if(l)return;const Se=ls(A);K(Se?{status:"ready",...Se}:{status:"error"})}).catch(()=>{l||K(A=>A?.status==="ready"?A:{status:"error"})})};if(x(),!o)return()=>{l=!0};const k=window.setInterval(x,Math.max(15,Number(c)||15)*1e3);return()=>{l=!0,window.clearInterval(k)}},[o,c,s?.resetGeneration,F]),u.useEffect(()=>{if(!Y.some(x=>z(x)))return;const l=window.setInterval(()=>X(x=>x+1),1e3);return()=>window.clearInterval(l)},[Y]);const $=u.useMemo(()=>Re(m.liveEvents||[],g),[m.liveEvents,g]),B=u.useMemo(()=>Re(m.persistedEvents||m.events.filter(l=>!z(l)),g),[m.events,m.persistedEvents,g]),j=u.useMemo(()=>{const l=new Map;for(const x of[...Y,...m.events])l.set(x.id,me(l.get(x.id),x));return[...l.values()]},[m.events,Y]),S=v,R=S&&(i?.event?.id===S?i.event:j.find(l=>l.id===S)||G.find(l=>l.id===S)||B.find(l=>l.id===S)),T=u.useMemo(()=>{const l=new Map;for(const A of[...G,...j])l.set(A.id,me(l.get(A.id),A));const x=[...l.values()],k=yt(x,S);return i?.event?.id!==S?k:k.map(A=>A.id===S?i.event:A)},[j,S,G,i]),U=R?Ee(R):"",ne=Math.max(Number(j.find(l=>l.id===S)?.changeSeq||0),Number(G.find(l=>l.id===S)?.changeSeq||0));u.useEffect(()=>{if(ee(null),!U||U==="-")return;const l=Lt({read:k=>fe.getRuntimeRequestChain(U,{signal:k}),onValue:k=>ee(A=>Ce(k,A?.requestID===U?A.events:[])),enabled:()=>te.current&&!document.hidden});J.current=l;const x=()=>l.synchronize();return document.addEventListener("visibilitychange",x),l.refresh(!0),()=>{l.dispose(),J.current===l&&(J.current=null),document.removeEventListener("visibilitychange",x)}},[U,s?.resetGeneration]),u.useEffect(()=>{J.current?.synchronize()},[o]),u.useEffect(()=>{if(!o||document.hidden||!U)return;const l=j.filter(x=>Ee(x)===U);ee(x=>x?.requestID===U?Ce(x,l):x),J.current?.refresh()},[o,j,U]),u.useEffect(()=>{if(!S){f(null);return}f(S,ne).catch(l=>{d(`加载事件详情失败: ${l.message}`,"error")})},[d,S,ne,f]);const E=(l,x="内容")=>It(l,x,d),Q=l=>{_(null);const x=Number(l);Number.isFinite(x)&&x!==m.page&&M(x>m.page?"forward":"backward"),m.setPage(l)},je=l=>{_(null),M("replace"),m.setPageSize(l)},ot=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:l=>t.jsx(tt,{event:l})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:l=>t.jsx(st,{event:l})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:l=>t.jsx(it,{event:l})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:l=>t.jsx(nt,{event:l})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:l=>t.jsx(be,{event:l})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:l=>t.jsx(at,{event:l})}];return t.jsxs("div",{className:"page-stack run-page",children:[t.jsxs("div",{className:"page-header",children:[t.jsxs("div",{className:"page-title-group",children:[t.jsxs("h1",{className:"page-title",children:[t.jsx(q,{name:"play",size:24,style:{color:"var(--primary)"}}),t.jsx("span",{children:"运行"}),t.jsx(Ct,{status:e})]}),t.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),t.jsx("div",{className:"page-actions",children:t.jsxs("button",{type:"button",className:`btn ${se?"btn-danger":"btn-primary"}`,onClick:r,children:[t.jsx(q,{name:se?"stop":"play",size:16}),t.jsx("span",{children:se?"停止运行":"启动运行"})]})})]}),e?.lastError&&t.jsxs("div",{className:"run-error-banner",children:[t.jsx(q,{name:"warning",size:20}),t.jsxs("div",{children:[t.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),t.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:t.jsxs("div",{className:"glass-panel run-hero-panel",children:[t.jsxs("div",{className:"run-hero-head",children:[t.jsxs("div",{className:"run-hero-heading",children:[t.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),t.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(se?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),t.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),t.jsxs("dl",{className:"run-detail-grid",children:[t.jsxs("div",{className:"run-detail-listener",children:[t.jsx("dt",{children:"HTTP 监听地址"}),t.jsxs("dd",{className:"run-listener-row",children:[t.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),t.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>E(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[t.jsx(q,{name:"copy",size:14}),t.jsx("span",{children:"复制"})]})]}),e?.listener?.httpsPort!=null&&t.jsxs(t.Fragment,{children:[t.jsx("dt",{children:"HTTPS 监听地址"}),t.jsxs("dd",{className:"run-listener-row",children:[t.jsxs("span",{className:"mono-cell run-listener-address",children:[e.listener.host,":",e.listener.httpsPort]}),t.jsx("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",title:"复制 HTTPS 地址",onClick:()=>E(`https://${e.listener.host.includes(":")?`[${e.listener.host}]`:e.listener.host}:${e.listener.httpsPort}`,"HTTPS 地址"),children:t.jsx(q,{name:"copy",size:14})})]})]})]}),t.jsxs("div",{children:[t.jsx("dt",{children:"配置版本"}),t.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),t.jsxs("div",{children:[t.jsx("dt",{children:"认证状态"}),t.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),t.jsxs("div",{children:[t.jsx("dt",{children:"运行时长"}),t.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?pe(e.uptimeSeconds*1e3):"-"})]}),t.jsxs("div",{children:[t.jsx("dt",{children:"调度策略"}),t.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),t.jsxs("div",{className:"run-metric-grid","aria-label":"今天用量与当前入口",children:[t.jsx(ae,{label:"输入 Token",value:N?.status==="ready"&&N.observed>0&&N.input!=null?ge(N.input):"—",detail:Le(N),icon:"file-down",accent:"var(--primary)"}),t.jsx(ae,{label:"输出 Token",value:N?.status==="ready"&&N.observed>0&&N.output!=null?ge(N.output):"—",detail:Le(N),icon:"file-up",accent:"var(--status-warning)"}),t.jsx(ae,{label:"可调度入口",value:L(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--accent-cyan)"}),t.jsx(ae,{label:"客户端请求",value:oe(N?.clientRequests),detail:`今天 · 成功 ${oe(N?.clientSuccesses)} / 失败 ${oe(N?.clientFailures)}`,icon:"move-diagonal",accent:"var(--status-good)"}),t.jsx(ae,{label:"上游尝试",value:oe(N?.upstreamAttempts),detail:`今天 · 含重试 · 故障转移 ${oe(N?.failovers)}`,icon:"network",accent:"var(--accent-indigo)"})]}),t.jsxs("div",{className:"glass-panel run-events-panel",children:[t.jsxs("div",{className:"panel-header",children:[t.jsxs("div",{className:"panel-title-group",children:[t.jsxs("div",{className:"panel-title",children:[t.jsx(q,{name:"activity",size:18,style:{color:"var(--primary)"}}),t.jsx("span",{children:"今日事件与遥测日志"})]}),t.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),t.jsxs("div",{className:"telemetry-toolbar",children:[t.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(l=>t.jsx("button",{type:"button",className:`telemetry-filter-button${h===l.id?" active":""}`,onClick:()=>{_(null),M("replace"),p(l.id)},"aria-pressed":h===l.id,children:l.label},l.id))}),t.jsx("div",{className:"telemetry-filter-group telemetry-outcome-filter-group",role:"group","aria-label":"事件结果状态筛选",children:[{id:"",label:"全部状态"},{id:"succeeded",label:"成功"},{id:"failed",label:"失败"},{id:"cancelled",label:"已取消"}].map(l=>t.jsx("button",{type:"button",className:`telemetry-filter-button${w===l.id?" active":""}`,onClick:()=>{_(null),M("replace"),y(l.id)},"aria-pressed":w===l.id,children:l.label},l.id||"all-outcomes"))}),m.mode!=="page"&&t.jsxs("span",{className:"panel-hint",children:[L(B.length)," 条持久事件"]}),t.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{b(l=>l==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${g==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[t.jsx(q,{name:"sort",size:14}),t.jsxs("span",{children:[m.mode==="page"?"本页":"",g==="desc"?"最新优先":"最早优先"]})]})]})]}),t.jsx(ds,{events:B}),t.jsxs("div",{className:"panel-body",style:{padding:0},children:[m.error&&t.jsxs("div",{className:"event-page-error",role:"alert",children:[t.jsxs("span",{children:["读取事件历史失败：",m.error]}),t.jsx("button",{type:"button",className:"btn btn-secondary",onClick:m.retry,children:"重试"})]}),t.jsxs("div",{className:`event-page-stage event-page-stage-${m.loading?"loading":"ready"}`,"data-page":m.page,"data-page-direction":V,"aria-busy":m.loading,children:[m.mode==="page"?t.jsx(Ne,{compact:!0,page:m.page,pageSize:m.pageSize,totalCount:m.totalCount,totalPages:m.totalPages,itemCount:B.length,liveItemCount:m.liveEventCount,loading:m.loading,onPageChange:Q,onPageSizeChange:je,ariaLabel:"持久事件分页"}):null,t.jsxs("div",{className:"event-page-content",children:[$.length>0&&t.jsxs("div",{className:"telemetry-live-section",children:[t.jsx("div",{className:"telemetry-live-desktop",children:t.jsx(ms,{events:$,selectedEventID:R?.id,onSelect:C})}),t.jsxs("div",{className:"telemetry-live-mobile",children:[t.jsxs("div",{className:"telemetry-live-heading",children:[t.jsxs("span",{children:[t.jsx("i",{"aria-hidden":"true"}),"进行中 · ",L($.length)]}),t.jsx("small",{children:"上游暂计"})]}),t.jsx(Oe,{events:$,selectedEventID:R?.id,onSelect:C,live:!0})]})]}),t.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[t.jsx("div",{className:"telemetry-desktop-table",children:t.jsx(Tt,{className:"telemetry-table",columns:ot,data:B,keyField:"id",ariaLabel:m.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:g,onSortChange:(l,x)=>b(x),onRowClick:l=>C(l.id),activeRowKey:R?.id,emptyText:m.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),t.jsx(Oe,{events:B,selectedEventID:R?.id,onSelect:C,loading:m.loading})]}),!R&&(B.length>0||$.length>0)&&t.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"}),m.mode==="page"&&B.length>=15&&m.totalPages>1?t.jsx("div",{className:"telemetry-bottom-pagination",children:t.jsx(Ne,{compact:!0,page:m.page,pageSize:m.pageSize,totalCount:m.totalCount,totalPages:m.totalPages,itemCount:B.length,liveItemCount:m.liveEventCount,loading:m.loading,onPageChange:Q,onPageSizeChange:je,ariaLabel:"持久事件底部分页"})}):null]})]}),m.mode==="legacy"?t.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),R&&t.jsx(Mt,{event:R,chain:T,chainTruncated:H?.truncated,onSelect:_,onClose:()=>_(null),requestSummary:ce(R),expanded:O,onToggle:I,onCopy:E,loading:i?.event?.id!==R.id&&R.detailsOmitted===!0})]})]})}export{js as RunPage};
