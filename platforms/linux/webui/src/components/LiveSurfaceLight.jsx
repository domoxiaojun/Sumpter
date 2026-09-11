import React, { useEffect, useState } from 'react';
import { MeshGradient } from '@paper-design/shaders-react';

export function LiveSurfaceLight() {
  const [light, setLight] = useState(() => document.documentElement.dataset.theme === 'light');
  const [reduced, setReduced] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  useEffect(() => {
    const theme = new MutationObserver(() => setLight(document.documentElement.dataset.theme === 'light'));
    const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
    const updateMotion = () => setReduced(motion.matches);
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    motion.addEventListener('change', updateMotion);
    return () => { theme.disconnect(); motion.removeEventListener('change', updateMotion); };
  }, []);
  // Keep the colors clear; the edge mask and a single opacity layer control
  // intensity without turning the whole request surface into a gray wash.
  const colors = light
    ? ['#68bcc9', '#8b96dc', '#c898c2', '#79b9c3']
    : ['#78c5d2', '#9ca8e8', '#d2a6ce', '#89c9cf'];
  return (
    <span className="telemetry-live-light" aria-hidden="true">
      <MeshGradient
        className="telemetry-live-mesh"
        colors={colors}
        speed={reduced ? 0 : 0.075}
        distortion={0.42}
        swirl={0.12}
        grainMixer={0.015}
        grainOverlay={0}
        minPixelRatio={1}
        maxPixelCount={1500000}
      />
    </span>
  );
}
