import { useEffect, useState } from 'react';
import { MeshGradient } from '@paper-design/shaders-react';
import { resolveLiveSurfaceColors } from '../utils/themeCatalog.js';

function readDomTheme() {
  const { palette, theme, accent } = document.documentElement.dataset;
  return {
    palette: palette || (theme === 'light' ? 'default-light' : 'default-dark'),
    accent: accent || 'mauve',
  };
}

export function LiveSurfaceLight() {
  const [domTheme, setDomTheme] = useState(readDomTheme);
  const [reduced, setReduced] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  useEffect(() => {
    const observer = new MutationObserver(() => setDomTheme(readDomTheme()));
    const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
    const updateMotion = () => setReduced(motion.matches);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme', 'data-palette', 'data-accent'],
    });
    motion.addEventListener('change', updateMotion);
    return () => { observer.disconnect(); motion.removeEventListener('change', updateMotion); };
  }, []);
  // Keep the colors clear; the edge mask and a single opacity layer control
  // intensity without turning the whole request surface into a gray wash.
  const colors = resolveLiveSurfaceColors(domTheme.palette, domTheme.accent);
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
