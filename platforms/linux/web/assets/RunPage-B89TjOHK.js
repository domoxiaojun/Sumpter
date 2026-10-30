import{j as i,I as F,r as u,v as We,c as $e,e as O,a as qe,i as He,b as Ge,d as Xe,p as Ye,f as te,n as Ke,u as Qe,o as ae,m as Ze,g as Je,h as et,k as tt,l as oe,q as z,s as ve,t as H,w as xe,x as be,y as ye,z as _e,A as je,B as Se,C as Re,D as J,E as Ee,F as we,G as it,H as st,J as rt,K as Ne,L as nt,M as Ue,N as at,O as ot,P as lt}from"./index-DJU8O27W.js";import{S as ct,c as ut}from"./clipboard-BXwHSjHk.js";import{D as dt}from"./DataTable-D5J1cLhD.js";import{P as ht}from"./outputSpeed-DqS2XNyd.js";import{E as mt,e as ie,a as ft,b as pt,c as gt,C as vt}from"./EventInspector-EUJLOqeD.js";function K({label:e,value:t,detail:s,icon:n,accent:a="var(--primary)"}){return i.jsxs("div",{className:"metric-card",style:{"--card-accent":a},children:[i.jsxs("div",{className:"metric-head",children:[i.jsx("span",{className:"metric-label",children:e}),n&&i.jsx("div",{className:"metric-icon-box",children:i.jsx(F,{name:n,size:18,"aria-hidden":"true",focusable:"false"})})]}),i.jsx("div",{className:"metric-value",children:t}),s&&i.jsx("div",{className:"metric-detail",children:s})]})}function xt({observation:e,autoRefresh:t}){const s=u.useRef(null);s.current=We(s.current,e,t);const n=$e(s.current);return i.jsxs("section",{className:"responses-connections-panel","aria-label":"当前 Responses WebSocket",children:[i.jsxs("div",{className:"responses-connections-heading",children:[i.jsx("strong",{children:"当前 Responses WebSocket"}),!t&&i.jsx("span",{children:"已暂停刷新"}),n.error&&i.jsx("span",{className:"responses-connections-error",role:"status",children:n.error})]}),n.message?i.jsx("p",{children:n.message}):i.jsx("dl",{className:"responses-connections-values",children:n.rows.map(([a,r])=>i.jsxs("div",{children:[i.jsx("dt",{children:a}),i.jsx("dd",{children:r})]},a))}),i.jsx("p",{children:"当前进程内的连接快照；已进入转发不代表模型正在生成。"})]})}const bt=`#version 300 es
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
}`,le=1920*1080*4;let yt=class{parentElement;canvasElement;gl;program=null;uniformLocations={};fragmentShader;rafId=null;lastRenderTime=0;currentFrame=0;speed=0;currentSpeed=0;providedUniforms;mipmaps=[];hasBeenDisposed=!1;resolutionChanged=!0;textures=new Map;minPixelRatio;maxPixelCount;isSafari=St();uniformCache={};textureUnitMap=new Map;ownerDocument;constructor(t,s,n,a,r=0,o=0,c=2,h=le,f=[]){if(t?.nodeType===1)this.parentElement=t;else throw new Error("Paper Shaders: parent element must be an HTMLElement");if(this.ownerDocument=t.ownerDocument,!this.ownerDocument.querySelector("style[data-paper-shader]")){const w=this.ownerDocument.createElement("style");w.innerHTML=jt,w.setAttribute("data-paper-shader",""),this.ownerDocument.head.prepend(w)}const d=this.ownerDocument.createElement("canvas");this.canvasElement=d,this.parentElement.prepend(d),this.fragmentShader=s,this.providedUniforms=n,this.mipmaps=f,this.currentFrame=o,this.minPixelRatio=c,this.maxPixelCount=h;const g=d.getContext("webgl2",a);if(!g)throw new Error("Paper Shaders: WebGL is not supported in this browser");this.gl=g,this.initProgram(),this.setupPositionAttribute(),this.setupUniforms(),this.setUniformValues(this.providedUniforms),this.setupResizeObserver(),visualViewport?.addEventListener("resize",this.handleVisualViewportChange),this.setupIntersectionObserver(),this.setSpeed(r),this.parentElement.setAttribute("data-paper-shader",""),this.parentElement.paperShaderMount=this,this.ownerDocument.addEventListener("visibilitychange",this.handleDocumentVisibilityChange)}initProgram=()=>{const t=_t(this.gl,bt,this.fragmentShader);t&&(this.program=t)};setupPositionAttribute=()=>{const t=this.gl.getAttribLocation(this.program,"a_position"),s=this.gl.createBuffer();this.gl.bindBuffer(this.gl.ARRAY_BUFFER,s);const n=[-1,-1,1,-1,-1,1,-1,1,1,-1,1,1];this.gl.bufferData(this.gl.ARRAY_BUFFER,new Float32Array(n),this.gl.STATIC_DRAW),this.gl.enableVertexAttribArray(t),this.gl.vertexAttribPointer(t,2,this.gl.FLOAT,!1,0,0)};setupUniforms=()=>{const t={u_time:this.gl.getUniformLocation(this.program,"u_time"),u_pixelRatio:this.gl.getUniformLocation(this.program,"u_pixelRatio"),u_resolution:this.gl.getUniformLocation(this.program,"u_resolution")};Object.entries(this.providedUniforms).forEach(([s,n])=>{if(t[s]=this.gl.getUniformLocation(this.program,s),n instanceof HTMLImageElement){const a=`${s}AspectRatio`;t[a]=this.gl.getUniformLocation(this.program,a)}}),this.uniformLocations=t};renderScale=1;parentWidth=0;parentHeight=0;parentDevicePixelWidth=0;parentDevicePixelHeight=0;devicePixelsSupported=!1;intersectionObserver=null;isInViewport=!0;resizeObserver=null;setupResizeObserver=()=>{this.resizeObserver=new ResizeObserver(([t])=>{if(t?.borderBoxSize[0]){const s=t.devicePixelContentBoxSize?.[0];s!==void 0&&(this.devicePixelsSupported=!0,this.parentDevicePixelWidth=s.inlineSize,this.parentDevicePixelHeight=s.blockSize),this.parentWidth=t.borderBoxSize[0].inlineSize,this.parentHeight=t.borderBoxSize[0].blockSize}this.handleResize()}),this.resizeObserver.observe(this.parentElement)};setupIntersectionObserver=()=>{const t=this.ownerDocument.defaultView;t?.IntersectionObserver&&(this.intersectionObserver=new t.IntersectionObserver(([s])=>{this.isInViewport=s?.isIntersecting??!0,this.updateCurrentSpeed()}),this.intersectionObserver.observe(this.parentElement))};handleVisualViewportChange=()=>{this.resizeObserver?.disconnect(),this.setupResizeObserver()};handleResize=()=>{let t=0,s=0;const n=Math.max(1,window.devicePixelRatio),a=visualViewport?.scale??1;if(this.devicePixelsSupported){const d=Math.max(1,this.minPixelRatio/n);t=this.parentDevicePixelWidth*d*a,s=this.parentDevicePixelHeight*d*a}else{let d=Math.max(n,this.minPixelRatio)*a;if(this.isSafari){const g=Rt(this.ownerDocument);d*=Math.max(1,g)}t=Math.round(this.parentWidth)*d,s=Math.round(this.parentHeight)*d}const r=Math.sqrt(this.maxPixelCount)/Math.sqrt(t*s),o=Math.min(1,r),c=Math.round(t*o),h=Math.round(s*o),f=c/Math.round(this.parentWidth);(this.canvasElement.width!==c||this.canvasElement.height!==h||this.renderScale!==f)&&(this.renderScale=f,this.canvasElement.width=c,this.canvasElement.height=h,this.resolutionChanged=!0,this.gl.viewport(0,0,this.gl.canvas.width,this.gl.canvas.height),this.render(performance.now()))};render=t=>{if(this.hasBeenDisposed)return;if(this.program===null){console.warn("Tried to render before program or gl was initialized");return}const s=t-this.lastRenderTime;this.lastRenderTime=t,this.currentSpeed!==0&&(this.currentFrame+=s*this.currentSpeed),this.gl.clear(this.gl.COLOR_BUFFER_BIT),this.gl.useProgram(this.program),this.gl.uniform1f(this.uniformLocations.u_time,this.currentFrame*.001),this.resolutionChanged&&(this.gl.uniform2f(this.uniformLocations.u_resolution,this.gl.canvas.width,this.gl.canvas.height),this.gl.uniform1f(this.uniformLocations.u_pixelRatio,this.renderScale),this.resolutionChanged=!1),this.gl.drawArrays(this.gl.TRIANGLES,0,6),this.currentSpeed!==0?this.requestRender():this.rafId=null};requestRender=()=>{this.rafId!==null&&cancelAnimationFrame(this.rafId),this.rafId=requestAnimationFrame(this.render)};setTextureUniform=(t,s)=>{if(!s.complete||s.naturalWidth===0)throw new Error(`Paper Shaders: image for uniform ${t} must be fully loaded`);const n=this.textures.get(t);n&&this.gl.deleteTexture(n),this.textureUnitMap.has(t)||this.textureUnitMap.set(t,this.textureUnitMap.size);const a=this.textureUnitMap.get(t);this.gl.activeTexture(this.gl.TEXTURE0+a);const r=this.gl.createTexture();this.gl.bindTexture(this.gl.TEXTURE_2D,r),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_S,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_WRAP_T,this.gl.CLAMP_TO_EDGE),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MAG_FILTER,this.gl.LINEAR),this.gl.texImage2D(this.gl.TEXTURE_2D,0,this.gl.RGBA,this.gl.RGBA,this.gl.UNSIGNED_BYTE,s),this.mipmaps.includes(t)&&(this.gl.generateMipmap(this.gl.TEXTURE_2D),this.gl.texParameteri(this.gl.TEXTURE_2D,this.gl.TEXTURE_MIN_FILTER,this.gl.LINEAR_MIPMAP_LINEAR));const o=this.gl.getError();if(o!==this.gl.NO_ERROR||r===null){console.error("Paper Shaders: WebGL error when uploading texture:",o);return}this.textures.set(t,r);const c=this.uniformLocations[t];if(c){this.gl.uniform1i(c,a);const h=`${t}AspectRatio`,f=this.uniformLocations[h];if(f){const d=s.naturalWidth/s.naturalHeight;this.gl.uniform1f(f,d)}}};areUniformValuesEqual=(t,s)=>t===s?!0:Array.isArray(t)&&Array.isArray(s)&&t.length===s.length?t.every((n,a)=>this.areUniformValuesEqual(n,s[a])):!1;setUniformValues=t=>{this.gl.useProgram(this.program),Object.entries(t).forEach(([s,n])=>{let a=n;if(n instanceof HTMLImageElement&&(a=`${n.src.slice(0,200)}|${n.naturalWidth}x${n.naturalHeight}`),this.areUniformValuesEqual(this.uniformCache[s],a))return;this.uniformCache[s]=a;const r=this.uniformLocations[s];if(!r){console.warn(`Uniform location for ${s} not found`);return}if(n instanceof HTMLImageElement)this.setTextureUniform(s,n);else if(Array.isArray(n)){let o=null,c=null;if(n[0]!==void 0&&Array.isArray(n[0])){const h=n[0].length;if(n.every(f=>f.length===h))o=n.flat(),c=h;else{console.warn(`All child arrays must be the same length for ${s}`);return}}else o=n,c=o.length;switch(c){case 2:this.gl.uniform2fv(r,o);break;case 3:this.gl.uniform3fv(r,o);break;case 4:this.gl.uniform4fv(r,o);break;case 9:this.gl.uniformMatrix3fv(r,!1,o);break;case 16:this.gl.uniformMatrix4fv(r,!1,o);break;default:console.warn(`Unsupported uniform array length: ${c}`)}}else typeof n=="number"?this.gl.uniform1f(r,n):typeof n=="boolean"?this.gl.uniform1i(r,n?1:0):console.warn(`Unsupported uniform type for ${s}: ${typeof n}`)})};getCurrentFrame=()=>this.currentFrame;setFrame=t=>{this.currentFrame=t,this.lastRenderTime=performance.now(),this.render(performance.now())};setSpeed=(t=1)=>{this.speed=t,this.updateCurrentSpeed()};updateCurrentSpeed=()=>{this.setCurrentSpeed(this.ownerDocument.hidden||!this.isInViewport?0:this.speed)};setCurrentSpeed=t=>{this.currentSpeed=t,this.rafId===null&&t!==0&&(this.lastRenderTime=performance.now(),this.rafId=requestAnimationFrame(this.render)),this.rafId!==null&&t===0&&(cancelAnimationFrame(this.rafId),this.rafId=null)};setMaxPixelCount=(t=le)=>{this.maxPixelCount=t,this.handleResize()};setMinPixelRatio=(t=2)=>{this.minPixelRatio=t,this.handleResize()};setUniforms=t=>{this.setUniformValues(t),this.providedUniforms={...this.providedUniforms,...t},this.render(performance.now())};handleDocumentVisibilityChange=()=>{this.updateCurrentSpeed()};dispose=()=>{this.hasBeenDisposed=!0,this.rafId!==null&&(cancelAnimationFrame(this.rafId),this.rafId=null),this.gl&&this.program&&(this.textures.forEach(t=>{this.gl.deleteTexture(t)}),this.textures.clear(),this.gl.deleteProgram(this.program),this.program=null,this.gl.bindBuffer(this.gl.ARRAY_BUFFER,null),this.gl.bindBuffer(this.gl.ELEMENT_ARRAY_BUFFER,null),this.gl.bindRenderbuffer(this.gl.RENDERBUFFER,null),this.gl.bindFramebuffer(this.gl.FRAMEBUFFER,null),this.gl.getError()),this.resizeObserver&&(this.resizeObserver.disconnect(),this.resizeObserver=null),this.intersectionObserver&&(this.intersectionObserver.disconnect(),this.intersectionObserver=null),visualViewport?.removeEventListener("resize",this.handleVisualViewportChange),this.ownerDocument.removeEventListener("visibilitychange",this.handleDocumentVisibilityChange),this.uniformLocations={},this.canvasElement.remove(),delete this.parentElement.paperShaderMount}};function ce(e,t,s){const n=e.createShader(t);return n?(e.shaderSource(n,s),e.compileShader(n),e.getShaderParameter(n,e.COMPILE_STATUS)?n:(console.error("An error occurred compiling the shaders: "+e.getShaderInfoLog(n)),e.deleteShader(n),null)):null}function _t(e,t,s){const n=e.getShaderPrecisionFormat(e.FRAGMENT_SHADER,e.MEDIUM_FLOAT),a=n?n.precision:null;a&&a<23&&(t=t.replace(/precision\s+(lowp|mediump)\s+float;/g,"precision highp float;"),s=s.replace(/precision\s+(lowp|mediump)\s+float/g,"precision highp float").replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g,"$1 highp $3"));const r=ce(e,e.VERTEX_SHADER,t),o=ce(e,e.FRAGMENT_SHADER,s);if(!r||!o)return null;const c=e.createProgram();return c?(e.attachShader(c,r),e.attachShader(c,o),e.linkProgram(c),e.getProgramParameter(c,e.LINK_STATUS)?(e.detachShader(c,r),e.detachShader(c,o),e.deleteShader(r),e.deleteShader(o),c):(console.error("Unable to initialize the shader program: "+e.getProgramInfoLog(c)),e.deleteProgram(c),e.deleteShader(r),e.deleteShader(o),null)):null}const jt=`@layer paper-shaders {
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
}`;function St(){const e=navigator.userAgent.toLowerCase();return e.includes("safari")&&!e.includes("chrome")&&!e.includes("android")}function Rt(e){const t=visualViewport?.scale??1,s=visualViewport?.width??window.innerWidth,n=window.innerWidth-e.documentElement.clientWidth,a=t*s+n,r=outerWidth/a,o=Math.round(100*r);return o%5===0?o/100:o===33?1/3:o===67?2/3:o===133?4/3:r}const Et={fit:"contain",scale:1,rotation:0,offsetX:0,offsetY:0,originX:.5,originY:.5,worldWidth:0,worldHeight:0},wt={none:0,contain:1,cover:2},Nt=`
#define TWO_PI 6.28318530718
#define PI 3.14159265358979323846
`,Ut=`
vec2 rotate(vec2 uv, float th) {
  return mat2(cos(th), sin(th), -sin(th), cos(th)) * uv;
}
`,At=`
  float hash21(vec2 p) {
    p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
  }
`,ue={maxColorCount:10},Ct=`#version 300 es
precision mediump float;

uniform float u_time;

uniform vec4 u_colors[${ue.maxColorCount}];
uniform float u_colorsCount;

uniform float u_distortion;
uniform float u_swirl;
uniform float u_grainMixer;
uniform float u_grainOverlay;

in vec2 v_objectUV;
out vec4 fragColor;

${Nt}
${Ut}
${At}

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

  for (int i = 0; i < ${ue.maxColorCount}; i++) {
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
`;function Tt(e){if(Array.isArray(e))return e.length===4?e:e.length===3?[...e,1]:X;if(typeof e!="string")return X;let t,s,n,a=1;if(e.startsWith("#"))[t,s,n,a]=Pt(e);else if(e.startsWith("rgb")){const r=zt(e);if(r===null)return X;[t,s,n,a]=r}else if(e.startsWith("hsl")){const r=Mt(e);if(r===null)return X;[t,s,n,a]=Bt(r)}else return console.error("Unsupported color format",e),X;return[Z(t,0,1),Z(s,0,1),Z(n,0,1),Z(a,0,1)]}function Pt(e){if(e=e.replace(/^#/,""),(e.length===3||e.length===4)&&(e=e.split("").map(r=>r+r).join("")),e.length===6&&(e=e+"ff"),!/^[0-9a-f]{8}$/i.test(e))return console.warn("Invalid hex color"),X;const t=parseInt(e.slice(0,2),16)/255,s=parseInt(e.slice(2,4),16)/255,n=parseInt(e.slice(4,6),16)/255,a=parseInt(e.slice(6,8),16)/255;return[t,s,n,a]}function zt(e){const t=e.match(/^rgba?\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0")/255,parseInt(t[2]??"0")/255,parseInt(t[3]??"0")/255,t[4]===void 0?1:parseFloat(t[4])]:null}function Mt(e){const t=e.match(/^hsla?\s*\(\s*(\d+)\s*,\s*(\d+)%\s*,\s*(\d+)%\s*(?:,\s*([0-9.]+))?\s*\)$/i);return t?[parseInt(t[1]??"0"),parseInt(t[2]??"0"),parseInt(t[3]??"0"),t[4]===void 0?1:parseFloat(t[4])]:null}function Bt(e){const[t,s,n,a]=e,r=t/360,o=s/100,c=n/100;let h,f,d;if(s===0)h=f=d=c;else{const g=(j,C,p)=>(p<0&&(p+=1),p>1&&(p-=1),p<.16666666666666666?j+(C-j)*6*p:p<.5?C:p<.6666666666666666?j+(C-j)*(.6666666666666666-p)*6:j),w=c<.5?c*(1+o):c+o-c*o,_=2*c-w;h=g(_,w,r+1/3),f=g(_,w,r),d=g(_,w,r-1/3)}return[h,f,d,a]}const Z=(e,t,s)=>Math.min(Math.max(e,t),s),X=[.5,.5,.5,1],It="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";function Ot(e){const t=u.useRef(void 0),s=u.useCallback(n=>{const a=e.map(r=>{if(r!=null){if(typeof r=="function"){const o=r,c=o(n);return typeof c=="function"?c:()=>{o(null)}}return r.current=n,()=>{r.current=null}}});return()=>{a.forEach(r=>r?.())}},e);return u.useMemo(()=>e.every(n=>n==null)?null:n=>{t.current&&(t.current(),t.current=void 0),n!=null&&(t.current=s(n))},e)}function de(e){if(e.naturalWidth<1024&&e.naturalHeight<1024){if(e.naturalWidth<1||e.naturalHeight<1)return;const t=e.naturalWidth/e.naturalHeight;e.width=Math.round(t>1?1024*t:1024),e.height=Math.round(t>1?1024:1024/t)}}async function he(e){const t={},s=[],n=r=>{try{return r.startsWith("/")||new URL(r),!0}catch{return!1}},a=r=>{try{return r.startsWith("/")?!1:new URL(r,window.location.origin).origin!==window.location.origin}catch{return!1}};return Object.entries(e).forEach(([r,o])=>{if(typeof o=="string"){const c=o||It;if(!n(c)){console.warn(`Uniform "${r}" has invalid URL "${c}". Skipping image loading.`);return}const h=new Promise((f,d)=>{const g=new Image;a(c)&&(g.crossOrigin="anonymous"),g.onload=()=>{de(g),t[r]=g,f()},g.onerror=()=>{console.error(`Could not set uniforms. Failed to load image at ${c}`),d()},g.src=c});s.push(h)}else if(o instanceof HTMLImageElement){const c=o.decode().then(()=>{de(o),t[r]=o});s.push(c)}else t[r]=o}),await Promise.all(s),t}const Ae=u.forwardRef(function({fragmentShader:t,uniforms:s,webGlContextAttributes:n,speed:a=0,frame:r=0,width:o,height:c,minPixelRatio:h,maxPixelCount:f,mipmaps:d,style:g,...w},_){const[j,C]=u.useState(!1),p=u.useRef(null),R=u.useRef(null),W=u.useRef(n);u.useEffect(()=>((async()=>{const L=await he(s);p.current&&!R.current&&(R.current=new yt(p.current,t,L,W.current,a,r,h,f,d),C(!0))})(),()=>{R.current?.dispose(),R.current=null}),[t]),u.useEffect(()=>{let B=!1;return(async()=>{const P=await he(s);B||R.current?.setUniforms(P)})(),()=>{B=!0}},[s,j]),u.useEffect(()=>{R.current?.setSpeed(a)},[a,j]),u.useEffect(()=>{R.current?.setMaxPixelCount(f)},[f,j]),u.useEffect(()=>{R.current?.setMinPixelRatio(h)},[h,j]),u.useEffect(()=>{R.current?.setFrame(r)},[r,j]);const $=Ot([p,_]);return i.jsx("div",{ref:$,style:o!==void 0||c!==void 0?{width:typeof o=="string"&&isNaN(+o)===!1?+o:o,height:typeof c=="string"&&isNaN(+c)===!1?+c:c,...g}:g,...w})});Ae.displayName="ShaderMount";function Lt(e,t){if(Object.keys(e).length!==Object.keys(t).length)return!1;for(const s in e){if(s==="colors"){const n=Array.isArray(e.colors),a=Array.isArray(t.colors);if(!n||!a){if(Object.is(e.colors,t.colors)===!1)return!1;continue}if(e.colors?.length!==t.colors?.length||!e.colors?.every((r,o)=>r===t.colors?.[o]))return!1;continue}if(Object.is(e[s],t[s])===!1)return!1}return!0}const N={params:{...Et,speed:1,frame:0,colors:["#e0eaff","#241d9a","#f75092","#9f50d3"],distortion:.8,swirl:.1,grainMixer:0,grainOverlay:0}},Vt=u.memo(function({speed:t=N.params.speed,frame:s=N.params.frame,colors:n=N.params.colors,distortion:a=N.params.distortion,swirl:r=N.params.swirl,grainMixer:o=N.params.grainMixer,grainOverlay:c=N.params.grainOverlay,fit:h=N.params.fit,rotation:f=N.params.rotation,scale:d=N.params.scale,originX:g=N.params.originX,originY:w=N.params.originY,offsetX:_=N.params.offsetX,offsetY:j=N.params.offsetY,worldWidth:C=N.params.worldWidth,worldHeight:p=N.params.worldHeight,...R}){const W={u_colors:n.map(Tt),u_colorsCount:n.length,u_distortion:a,u_swirl:r,u_grainMixer:o,u_grainOverlay:c,u_fit:wt[h],u_rotation:f,u_scale:d,u_offsetX:_,u_offsetY:j,u_originX:g,u_originY:w,u_worldWidth:C,u_worldHeight:p};return i.jsx(Ae,{...R,speed:t,frame:s,fragmentShader:Ct,uniforms:W})},Lt);function Ce(){const[e,t]=u.useState(()=>document.documentElement.dataset.theme==="light"),[s,n]=u.useState(()=>window.matchMedia("(prefers-reduced-motion: reduce)").matches);u.useEffect(()=>{const r=new MutationObserver(()=>t(document.documentElement.dataset.theme==="light")),o=window.matchMedia("(prefers-reduced-motion: reduce)"),c=()=>n(o.matches);return r.observe(document.documentElement,{attributes:!0,attributeFilter:["data-theme"]}),o.addEventListener("change",c),()=>{r.disconnect(),o.removeEventListener("change",c)}},[]);const a=e?["#68bcc9","#8b96dc","#c898c2","#79b9c3"]:["#78c5d2","#9ca8e8","#d2a6ce","#89c9cf"];return i.jsx("span",{className:"telemetry-live-light","aria-hidden":"true",children:i.jsx(Vt,{className:"telemetry-live-mesh",colors:a,speed:s?0:.075,distortion:.42,swirl:.12,grainMixer:.015,grainOverlay:0,minPixelRatio:1,maxPixelCount:15e5})})}function me(e,t,s=""){if(!t)return[];const n=new Map((t.events||[]).map(r=>[r.id,r])),a=Math.min(...(t.events||[]).map(r=>Number(r.seq||0)));return(e||[]).filter(r=>{if(s&&r.kind!==s||O(r))return!1;if(t.snapshotChangeSeq!=null)return Number(r.changeSeq||0)>Number(t.snapshotChangeSeq);const o=n.get(r.id);return o?Number(r.changeSeq||0)>Number(o.changeSeq||0):!Number.isFinite(a)||Number(r.seq||0)>=a})}function Dt(e,t,s){const n=new Map((e||[]).filter(a=>!O(a)).map(a=>[a.id,a]));for(const a of t||[]){const r=n.get(a.id);(!r||Number(a.changeSeq||0)>=Number(r.changeSeq||0))&&n.set(a.id,a)}return[...n.values()].sort((a,r)=>Number(r.seq||0)-Number(a.seq||0)).slice(0,s)}function fe(e,t){return!t||e?.kind===t}function kt({recentEvents:e=[],eventKind:t="",resetGeneration:s=0,onRecovered:n}={}){const[a,r]=u.useState(1),[o,c]=u.useState(qe),[h,f]=u.useState(null),[d,g]=u.useState("loading"),[w,_]=u.useState(!0),[j,C]=u.useState(!1),p=u.useRef(!1),R=u.useRef(null),[W,$]=u.useState(null),[B,L]=u.useState(0),P=u.useRef(0),U=u.useRef(null),V=u.useRef(null),G=`${t||"all"}:${o}:${Number(s||0)}`;u.useEffect(()=>{p.current=!1,R.current=null,U.current=null,g("loading"),_(!0),f(null),$(null),r(1)},[G]),u.useEffect(()=>{const b=++P.current,E=new AbortController,A=Math.max(1,Number(a)||1),v=U.current,S=p.current;p.current=!1,_(!S),C(!0),$(null);const Q=async(y=!0)=>{const D=await te.getRuntimeEventPage({page:y?A:1,pageSize:o,kind:t||void 0,snapshotSeq:y?v?.snapshotSeq:void 0,snapshotChangeSeq:y?v?.snapshotChangeSeq:void 0,historyGeneration:y?v?.historyGeneration:void 0,signal:E.signal});return Ke(D,{page:y?A:1,pageSize:o})};return(async()=>{try{let y;try{y=await Q(!0)}catch(D){if(!He(D))throw D;y=await Q(!1),b===P.current&&(r(1),n?.(Ge(D)))}if(b!==P.current)return;if(!y){g("legacy"),f(null);return}U.current=y,f(y),g("page"),y.page!==A&&r(y.page)}catch(y){if(b!==P.current||y?.name==="AbortError")return;if([400,404,405].includes(Number(y?.status))){g("legacy"),f(null);return}$(y?.message||"加载事件历史失败")}finally{b===P.current&&(_(!1),C(!1))}})(),()=>E.abort()},[t,n,a,o,B,s]);const q=u.useCallback(b=>{const E=Xe(b,h?.totalPages||1);p.current=!1,r(E)},[h?.totalPages]),T=u.useCallback(b=>{p.current=!1;const E=Ye(b);U.current=null,c(E),r(1)},[]),I=u.useMemo(()=>d==="page"&&a!==1?[]:e.filter(b=>fe(b,t)&&O(b)),[t,d,a,e]),Y=u.useMemo(()=>{const b=d==="page"&&a===1?me(e,h,t):[],E=d==="page"?Dt(h?.events||[],b,o):e,A=new Set(I.map(v=>v.id));return E.filter(v=>fe(v,t)&&!A.has(v.id)&&!O(v))},[t,I,d,a,o,e,h]),m=u.useMemo(()=>[...I,...Y],[I,Y]);return u.useEffect(()=>{if(d!=="page"||a!==1||j||U.current==null)return;const b=me(e,U.current,t);if(!b.length||V.current!=null)return;const E=Math.max(...b.map(S=>Number(S.changeSeq||0))),A=`${t}:${U.current.snapshotChangeSeq??"legacy"}:${E}`,v=R.current;if(!(v?.signature===A&&Date.now()-v.at<5e3))return V.current=window.setTimeout(()=>{V.current=null,R.current={signature:A,at:Date.now()},p.current=!0,U.current=null,L(S=>S+1)},220),()=>{V.current!=null&&(window.clearTimeout(V.current),V.current=null)}},[t,j,d,a,e]),{mode:d,loading:w,error:W,events:m,liveEvents:I,persistedEvents:Y,liveEventCount:I.length,page:a,pageSize:o,totalCount:h?.totalCount??m.length,totalPages:h?.totalPages??(m.length?1:0),hasNext:!!h?.hasNext,hasPrevious:!!h?.hasPrevious,snapshotSeq:h?.snapshotSeq??null,snapshotChangeSeq:h?.snapshotChangeSeq??null,historyGeneration:h?.historyGeneration??null,setPage:q,setPageSize:T,retry:()=>{p.current=!1,L(b=>b+1)}}}function Ft(e){const t=e?.totals?.tokens;if(!t||typeof t!="object")return null;const s=Number(t.observedRequests??t.observed_requests);if(!Number.isFinite(s)||s<0)return null;const n=a=>{const r=Number(a);return Number.isFinite(r)&&r>=0?r:null};return{input:n(t.inputTokens??t.input_tokens),output:n(t.outputTokens??t.output_tokens),observed:s}}function pe(e){return e?e.status==="error"?"今天用量暂不可用":e.observed>0?`今天累计 · ${z(e.observed)} 个请求有用量`:"暂无可用用量":"正在读取今天的用量"}function Te(e){const t=ve(e);if(!t?.applicable)return null;if(t.name&&t.name!=="unidentified_project"){if(t.source==="workspace_local"&&t.localUser)return`${t.name} 本地(${t.localUser})`;const s=t.source==="client_declared"?"（客户端声明）":"";return`项目: ${t.name}${s}`}return t.label}function Pe(e){const t=Ue(e,"requestPurpose"),s=typeof t=="object"?t?.kind||t?.type||t?.name:t;return s?s==="standard"?"主请求":s==="websearch"?"WebSearch":s==="webfetch"?"WebFetch":at(e):null}function Wt(e){if(String(e?.kind||"").toLowerCase()==="notify"&&!H(e))return"不适用（通知事件）";if(O(e))return"进行中";const t=H(e);return t==="succeeded"?"成功":t==="failed"?"失败":t==="cancelled"?"已取消":ot(e).replace(/\s*\([^)]*\)\s*/g,"")}function $t(e){return String(e?.kind||"").toLowerCase()==="notify"&&!H(e)?"不适用（通知事件）":O(e)?"进行中":Ue(e,"phase")==="completed"||H(e)?"已完成":lt(e).replace(/\s*\([^)]*\)\s*/g,"")}function ze(e){return String(e?.kind||"").toLowerCase()==="notify"&&!H(e)?"不适用（通知事件）":`${Ne(e)} · ${$t(e)}`}function ee(e){return[Te(e),Ee(e.kind),we(e),ie(e)?`代理: ${ie(e)}`:null,Pe(e),e.sourceIP].filter(Boolean).join(" · ")}function Me({event:e,compact:t=!1}){return i.jsx("span",{className:`telemetry-outcome-text ${nt(e)}${t?" is-compact":""}`,children:Wt(e)})}function Be({event:e,live:t=!1}){const s=ee(e),n=Te(e),a=e.kind==="notify"?[]:it(e);return i.jsxs("span",{className:t?"telemetry-live-request":"telemetry-primary-cell",children:[i.jsx("span",{className:`mono-cell${t?" telemetry-live-time":" telemetry-event-time"}`,children:ye(e.timestamp)}),n&&i.jsx("span",{className:"telemetry-event-meta",title:n,children:n}),i.jsx("span",{className:`telemetry-event-meta${t?" telemetry-live-request-summary":""}`,title:s,children:[Ee(e.kind),we(e),ie(e),Pe(e),e.sourceIP].filter(Boolean).join(" · ")}),a.length>0&&i.jsxs("span",{className:"telemetry-event-meta telemetry-event-tools",title:`调用工具：${a.join("、")}`,children:[i.jsx(F,{name:"wrench",size:11,"aria-hidden":"true"}),a.join("、")]})]})}function Ie({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{children:e.hookEvent||"通知"}):i.jsxs("span",{className:t?"telemetry-live-route":"telemetry-cell-stack telemetry-route-cell",children:[i.jsx("strong",{className:"telemetry-event-model",children:Se(e)}),i.jsx("span",{className:"telemetry-route-meta",children:Re(e)})]})}function Oe({event:e,live:t=!1}){return e.kind==="notify"?i.jsx("span",{className:"telemetry-cell-stack",children:"通知事件"}):i.jsxs("span",{className:t?"telemetry-live-result":"telemetry-cell-stack telemetry-result-cell",children:[i.jsx(Me,{event:e}),i.jsx("span",{className:"telemetry-result-detail telemetry-result-transport",children:st(e)}),i.jsx("span",{className:"telemetry-result-detail",title:ze(e),children:rt(e)?Ne(e):"HTTP —"})]})}function Le({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-duration-cell",children:"—"});const t=je(e)>=5e3;return i.jsx("span",{className:"mono-cell telemetry-duration-cell",title:t?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:_e(e)})}function se({event:e}){if(e.kind==="notify")return i.jsx("span",{className:"telemetry-usage-cell is-empty",children:"—"});const t=ft(e),s=O(e);return i.jsxs("span",{className:`telemetry-usage-cell${t?"":" is-empty"}`,title:s?"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。":"输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。",children:[i.jsx("span",{className:"telemetry-usage-tokens",children:pt(e)}),i.jsxs("span",{className:"telemetry-usage-secondary",children:[i.jsx("span",{children:s&&!t?"上游尚未返回":gt(e)}),(!s||t)&&i.jsx(vt,{event:e}),s&&t&&i.jsx("small",{className:"telemetry-usage-provisional",children:"暂计"})]})]})}function Ve({event:e,live:t=!1}){const s=xe(e)||(e.outcome==="failed"?be(e):"");return i.jsxs("span",{className:`${t?"telemetry-live-message":"telemetry-event-message"}${J(e)?" is-warning":H(e)==="failed"?" is-error":""}`,title:s||void 0,children:[J(e)&&i.jsx("strong",{children:"故障转移 · "}),s||(t?"请求进行中，等待最终结果":"-")]})}function qt({events:e,selectedEventID:t,onSelect:s}){return e?.length?i.jsxs("section",{className:"telemetry-live-group","aria-label":`${z(e.length)} 个进行中请求`,children:[i.jsx(Ce,{}),i.jsxs("div",{className:"telemetry-live-columns",children:[i.jsxs("span",{className:"telemetry-live-heading-cell",children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",z(e.length)]}),i.jsx("span",{"aria-hidden":"true",children:"模型 / 路由"}),i.jsx("span",{"aria-hidden":"true",children:"结果"}),i.jsx("span",{"aria-hidden":"true",children:"首字节 → 总耗时"}),i.jsxs("span",{"aria-hidden":"true",children:["Token 用量 ",i.jsx("small",{title:"仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。",children:"上游暂计"})]}),i.jsx("span",{"aria-hidden":"true",children:"事件状态"})]}),i.jsx("div",{className:"telemetry-live-list",role:"list","aria-label":"进行中请求列表",children:e.map(n=>{const a=t===n.id;return i.jsx("div",{role:"listitem",children:i.jsxs("button",{type:"button",className:`telemetry-live-event runtime-row-enter${a?" active":""}`,"aria-pressed":a,onClick:()=>s(n.id),children:[i.jsx(Be,{event:n,live:!0}),i.jsx(Ie,{event:n,live:!0}),i.jsx(Oe,{event:n,live:!0}),i.jsx(Le,{event:n}),i.jsx(se,{event:n}),i.jsx(Ve,{event:n,live:!0})]})},n.id)})})]}):null}function ge({events:e,selectedEventID:t,onSelect:s,loading:n,live:a=!1}){return e?.length?i.jsxs("div",{className:"responsive-data-card-list telemetry-mobile-list","data-live":a,role:"list","aria-label":a?"进行中请求列表":"请求事件列表",children:[a&&i.jsx(Ce,{}),e.map(r=>{const o=ve(r),c=H(r),h=xe(r)||(r.outcome==="failed"?be(r):""),f=t===r.id;return i.jsx("article",{role:"listitem",children:i.jsxs("div",{className:`responsive-data-card telemetry-mobile-card${f?" is-selected":""}`,role:"button",tabIndex:0,"aria-pressed":f,onClick:()=>s(r.id),onKeyDown:d=>{(d.key==="Enter"||d.key===" ")&&(d.preventDefault(),s(r.id))},children:[i.jsxs("div",{className:"telemetry-mobile-heading",children:[i.jsx("span",{className:"mono-cell telemetry-mobile-time",children:ye(r.timestamp)}),i.jsx(Me,{event:r,compact:!0}),i.jsx("span",{className:"mono-cell telemetry-mobile-duration",style:{color:c==="failed"?"var(--status-critical)":void 0},title:je(r)>=5e3?"首字节 → 总耗时；首字节超过 5 秒，上游可能排队中":"首字节 → 总耗时",children:_e(r)})]}),i.jsxs("div",{className:"telemetry-mobile-route",children:[i.jsx("strong",{className:"mono-cell",children:r.kind==="notify"?r.hookEvent||"通知":Se(r)}),r.kind!=="notify"&&i.jsx("small",{children:Re(r)})]}),i.jsxs("div",{className:"telemetry-mobile-usage",children:[i.jsx("span",{className:"telemetry-mobile-status",children:ze(r)}),r.kind!=="notify"&&i.jsx(se,{event:r})]}),i.jsxs("div",{className:"telemetry-mobile-context",children:[i.jsx("span",{className:"telemetry-mobile-request-summary",title:ee(r),children:ee(r)||o.label}),h&&i.jsxs("p",{className:`telemetry-mobile-message${c==="failed"?" is-error":J(r)?" is-warning":""}`,children:[i.jsx("span",{className:"telemetry-state-label",children:"事件状态"}),J(r)&&i.jsx("strong",{children:"故障转移 · "}),h]})]})]})},r.id)})]}):a?null:i.jsx("div",{className:"responsive-data-card-list telemetry-mobile-list",children:i.jsx("div",{className:"responsive-card-empty",children:n?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})})}function Zt(){const{status:e,runtime:t,runtimeEventDetail:s,config:n,secretStatus:a,toggleProxy:r,connectionsObservation:o,autoRefresh:c,runRefreshInterval:h,loadRuntimeEvent:f,addToast:d}=Qe(),[g,w]=u.useState("client"),[_,j]=u.useState("desc"),[C,p]=u.useState(null),[R,W]=u.useState({}),$=(l,x)=>W(k=>k[l]===x?k:{...k,[l]:x}),[B,L]=u.useState([]),[P,U]=u.useState("none"),[,V]=u.useState(0),G=e?.running,q=t?.recentEvents||[],[T,I]=u.useState(null),Y=u.useCallback(l=>d(l,"info"),[d]),m=kt({recentEvents:q,eventKind:g==="all"?"":g,resetGeneration:t?.resetGeneration,onRecovered:Y});u.useEffect(()=>{if(m.loading||P==="none")return;const l=window.setTimeout(()=>U("none"),280);return()=>window.clearTimeout(l)},[m.loading,P]),u.useEffect(()=>{let l=!1;const x=()=>{document.hidden||te.getRuntimeTrends({range:"today",granularity:"hour"}).then(M=>{if(l)return;const ne=Ft(M);I(ne?{status:"ready",...ne}:{status:"error",input:null,output:null,observed:0})}).catch(()=>{l||I(M=>M?.status==="ready"?M:{status:"error",input:null,output:null,observed:0})})};if(x(),!c)return()=>{l=!0};const k=window.setInterval(x,Math.max(15,Number(h)||15)*1e3);return()=>{l=!0,window.clearInterval(k)}},[c,h,t?.resetGeneration]),u.useEffect(()=>{if(!q.some(x=>O(x)))return;const l=window.setInterval(()=>V(x=>x+1),1e3);return()=>window.clearInterval(l)},[q]);const b=u.useMemo(()=>ae(m.liveEvents||[],_),[m.liveEvents,_]),E=u.useMemo(()=>ae(m.persistedEvents||m.events.filter(l=>!O(l)),_),[m.events,m.persistedEvents,_]),A=u.useMemo(()=>{const l=new Map;for(const x of[...q,...m.events])l.set(x.id,Ze(l.get(x.id),x));return[...l.values()]},[m.events,q]),v=C,S=v&&(s?.event?.id===v?s.event:B.find(l=>l.id===v)||A.find(l=>l.id===v)||E.find(l=>l.id===v)),Q=u.useMemo(()=>{const l=new Map;for(const M of[...A,...B])l.set(M.id,M);const x=[...l.values()],k=Je(x,v);return s?.event?.id!==v?k:k.map(M=>M.id===v?s.event:M)},[A,v,B,s]),y=S?et(S):"",D=Number(A.find(l=>l.id===v)?.changeSeq||0);u.useEffect(()=>{if(L([]),!y||y==="-")return;const l=new AbortController;return te.getRuntimeRequestChain(y,{signal:l.signal}).then(x=>{Array.isArray(x?.events)&&L(x.events)}).catch(()=>{}),()=>l.abort()},[y]),u.useEffect(()=>{if(!v){f(null);return}f(v,D).catch(l=>{d(`加载事件详情失败: ${l.message}`,"error")})},[d,v,D,f]);const re=(l,x="内容")=>ut(l,x,d),De=l=>{p(null);const x=Number(l);Number.isFinite(x)&&x!==m.page&&U(x>m.page?"forward":"backward"),m.setPage(l)},ke=l=>{p(null),U("replace"),m.setPageSize(l)},Fe=[{key:"timestamp",title:"请求",type:"time",width:"220px",minWidth:"220px",sortable:!0,render:l=>i.jsx(Be,{event:l})},{key:"model",title:"模型 / 路由",type:"text",width:"180px",minWidth:"180px",render:l=>i.jsx(Ie,{event:l})},{key:"outcome",title:"结果",type:"status",width:"104px",minWidth:"104px",render:l=>i.jsx(Oe,{event:l})},{key:"duration",title:"首字节 → 总耗时",type:"text",width:"156px",minWidth:"156px",render:l=>i.jsx(Le,{event:l})},{key:"usage",title:"Token 用量",type:"text",width:"220px",minWidth:"220px",render:l=>i.jsx(se,{event:l})},{key:"message",title:"事件状态",type:"text",width:"220px",minWidth:"220px",render:l=>i.jsx(Ve,{event:l})}];return i.jsxs("div",{className:"page-stack run-page",children:[i.jsxs("div",{className:"page-header",children:[i.jsxs("div",{className:"page-title-group",children:[i.jsxs("h1",{className:"page-title",children:[i.jsx(F,{name:"play",size:24,style:{color:"var(--primary)"}}),i.jsx("span",{children:"运行"}),i.jsx(ct,{status:e})]}),i.jsx("p",{className:"page-subtitle",children:"启动状态、监听地址、统计和最近事件。"})]}),i.jsx("div",{className:"page-actions",children:i.jsxs("button",{type:"button",className:`btn ${G?"btn-danger":"btn-primary"}`,onClick:r,children:[i.jsx(F,{name:G?"stop":"play",size:16}),i.jsx("span",{children:G?"停止运行":"启动运行"})]})})]}),e?.lastError&&i.jsxs("div",{className:"run-error-banner",children:[i.jsx(F,{name:"warning",size:20}),i.jsxs("div",{children:[i.jsx("strong",{children:"最近错误告警："})," ",e.lastError]})]}),i.jsx("div",{className:"run-hero-grid run-hero-grid-single",children:i.jsxs("div",{className:"glass-panel run-hero-panel",children:[i.jsxs("div",{className:"run-hero-head",children:[i.jsxs("div",{className:"run-hero-heading",children:[i.jsx("span",{className:"run-hero-kicker",children:"服务监听状态"}),i.jsx("p",{className:"run-hero-description",children:e?.health?.headline||(G?"代理服务正常响应客户端请求":"服务当前处于休眠停止状态")})]}),i.jsxs("span",{className:"mono-cell run-hero-version",children:["v",e?.version||"0.2.0"," · ",e?.generation||"-"]})]}),i.jsxs("dl",{className:"run-detail-grid",children:[i.jsxs("div",{className:"run-detail-listener",children:[i.jsx("dt",{children:"监听地址"}),i.jsxs("dd",{className:"run-listener-row",children:[i.jsx("span",{className:"mono-cell run-listener-address",children:e?.listener?`${e.listener.host}:${e.listener.port}`:"127.0.0.1:57878"}),i.jsxs("button",{type:"button",className:"btn btn-ghost btn-compact run-copy-address",onClick:()=>re(`http://${e?.listener?.host||"127.0.0.1"}:${e?.listener?.port||57878}`,"监听地址"),title:"复制监听地址",children:[i.jsx(F,{name:"copy",size:14}),i.jsx("span",{children:"复制"})]})]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"配置版本"}),i.jsxs("dd",{children:["v",n?.schemaVersion||7]})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"认证状态"}),i.jsx("dd",{className:a?.inboundAuthToken?.configured?"run-detail-good":void 0,children:a?.inboundAuthToken?.configured?`已启用 · 尾号 ${a.inboundAuthToken.last4}`:"未启用 · 本机免密"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"运行时长"}),i.jsx("dd",{className:"mono-cell",children:e?.uptimeSeconds?tt(e.uptimeSeconds*1e3):"-"})]}),i.jsxs("div",{children:[i.jsx("dt",{children:"调度策略"}),i.jsx("dd",{children:"优先级 + 会话粘性绑定"})]})]})]})}),i.jsxs("div",{className:"run-metric-grid",children:[i.jsx(K,{label:"输入 Token",value:T?.status==="ready"&&T.observed>0&&T.input!=null?oe(T.input):"—",detail:pe(T),icon:"file-down",accent:"var(--primary)"}),i.jsx(K,{label:"输出 Token",value:T?.status==="ready"&&T.observed>0&&T.output!=null?oe(T.output):"—",detail:pe(T),icon:"file-up",accent:"var(--status-warning)"}),i.jsx(K,{label:"可调度入口",value:z(e?.endpoints??n?.endpoints?.length??0),detail:"按优先级形成 Provider 候选序列",icon:"git-fork",accent:"var(--primary)"}),i.jsx(K,{label:"客户端请求",value:z(t?.clientRequests||0),detail:`端到端 · 成功 ${z(t?.clientSuccesses||0)} / 失败 ${z(t?.clientFailures||0)}`,icon:"move-diagonal",accent:"var(--primary)"}),i.jsx(K,{label:"上游尝试",value:z(t?.upstreamAttempts||0),detail:`含重试 · 故障转移 ${z(t?.failovers||0)}`,icon:"network",accent:"var(--primary)"})]}),i.jsx(xt,{observation:o,autoRefresh:c}),i.jsxs("div",{className:"glass-panel run-events-panel",children:[i.jsxs("div",{className:"panel-header",children:[i.jsxs("div",{className:"panel-title-group",children:[i.jsxs("div",{className:"panel-title",children:[i.jsx(F,{name:"activity",size:18,style:{color:"var(--primary)"}}),i.jsx("span",{children:"最近事件与遥测日志"})]}),i.jsx("span",{className:"panel-hint",children:"客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。"})]}),i.jsxs("div",{className:"telemetry-toolbar",children:[i.jsx("div",{className:"telemetry-filter-group",role:"group","aria-label":"事件类型筛选",children:[{id:"client",label:"客户端"},{id:"upstream",label:"上游"},{id:"all",label:"全部"}].map(l=>i.jsx("button",{type:"button",className:`telemetry-filter-button${g===l.id?" active":""}`,onClick:()=>{p(null),U("replace"),w(l.id)},"aria-pressed":g===l.id,children:l.label},l.id))}),m.mode!=="page"&&i.jsxs("span",{className:"panel-hint",children:[z(E.length)," 条持久事件"]}),i.jsxs("button",{type:"button",className:"btn btn-ghost telemetry-sort-button",onClick:()=>{j(l=>l==="desc"?"asc":"desc")},"aria-label":`切换当前页事件时间排序，当前为${_==="desc"?"最新优先":"最早优先"}`,title:"按当前页事件时间稳定排序",children:[i.jsx(F,{name:"sort",size:14}),i.jsxs("span",{children:[m.mode==="page"?"本页":"",_==="desc"?"最新优先":"最早优先"]})]})]})]}),i.jsxs("div",{className:"panel-body",style:{padding:0},children:[m.error&&i.jsxs("div",{className:"event-page-error",role:"alert",children:[i.jsxs("span",{children:["读取事件历史失败：",m.error]}),i.jsx("button",{type:"button",className:"btn btn-secondary",onClick:m.retry,children:"重试"})]}),i.jsxs("div",{className:`event-page-stage event-page-stage-${m.loading?"loading":"ready"}`,"data-page":m.page,"data-page-direction":P,"aria-busy":m.loading,children:[m.mode==="page"?i.jsx(ht,{compact:!0,page:m.page,pageSize:m.pageSize,totalCount:m.totalCount,totalPages:m.totalPages,itemCount:E.length,liveItemCount:m.liveEventCount,loading:m.loading,onPageChange:De,onPageSizeChange:ke,ariaLabel:"持久事件分页"}):null,i.jsxs("div",{className:"event-page-content",children:[b.length>0&&i.jsxs("div",{className:"telemetry-live-section",children:[i.jsx("div",{className:"telemetry-live-desktop",children:i.jsx(qt,{events:b,selectedEventID:S?.id,onSelect:p})}),i.jsxs("div",{className:"telemetry-live-mobile",children:[i.jsxs("div",{className:"telemetry-live-heading",children:[i.jsxs("span",{children:[i.jsx("i",{"aria-hidden":"true"}),"进行中 · ",z(b.length)]}),i.jsx("small",{children:"上游暂计"})]}),i.jsx(ge,{events:b,selectedEventID:S?.id,onSelect:p,live:!0})]})]}),i.jsxs("section",{className:"telemetry-history-group","aria-label":"历史事件",children:[i.jsx("div",{className:"telemetry-desktop-table",children:i.jsx(dt,{className:"telemetry-table",columns:Fe,data:E,keyField:"id",ariaLabel:m.mode==="page"?"SQLite 持久事件分页列表":"最近请求事件列表",tableMinWidth:"1100px",sortKey:"timestamp",sortDirection:_,onSortChange:(l,x)=>j(x),onRowClick:l=>p(l.id),activeRowKey:S?.id,emptyText:m.loading?"正在读取事件历史…":"当前筛选条件下暂无请求事件"})}),i.jsx(ge,{events:E,selectedEventID:S?.id,onSelect:p,loading:m.loading})]}),!S&&(E.length>0||b.length>0)&&i.jsx("div",{className:"telemetry-selection-hint",role:"status",children:"选择一条事件查看请求链路与诊断详情"})]})]}),m.mode==="legacy"?i.jsx("div",{className:"event-page-compatibility",role:"status",children:"当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。"}):null]}),S&&i.jsx(mt,{event:S,chain:Q,onSelect:p,requestSummary:ee(S),expanded:R,onToggle:$,onCopy:re,loading:s?.event?.id!==S.id&&S.detailsOmitted===!0})]})]})}export{Zt as RunPage};
