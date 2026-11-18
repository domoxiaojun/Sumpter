import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CATPPUCCIN_ACCENTS,
  THEME_MODES,
  isCatppuccinMode,
  nextModeForAccentSelection,
  nextThemeModeOnToggle,
  normalizeCatppuccinAccent,
  normalizeThemeMode,
  resolveLiveSurfaceColors,
  resolveLoginShaderColors,
  resolveThemeState,
} from '../src/utils/themeCatalog.js';

test('主题与强调色目录完整保留原黑白钴蓝与 Catppuccin 4 风味 8 强调色', () => {
  assert.deepEqual(
    THEME_MODES.map((item) => item.id),
    [
      'system',
      'light',
      'dark',
      'catppuccin-latte',
      'catppuccin-frappe',
      'catppuccin-macchiato',
      'catppuccin-mocha',
    ],
  );
  assert.deepEqual(
    CATPPUCCIN_ACCENTS.map((item) => item.id),
    ['mauve', 'blue', 'sapphire', 'teal', 'green', 'peach', 'pink', 'lavender'],
  );
  assert.equal(normalizeThemeMode('unknown-theme'), 'dark');
  assert.equal(normalizeCatppuccinAccent('unknown-accent'), 'mauve');
  assert.equal(isCatppuccinMode('catppuccin-latte'), true);
  assert.equal(isCatppuccinMode('system'), false);
});

test('主题解析正确处理跟随系统与各个风味的明暗极性和元色', () => {
  assert.deepEqual(resolveThemeState('system', 'mauve', true), {
    themeMode: 'system',
    accent: 'mauve',
    scheme: 'dark',
    palette: 'default-dark',
    isCatppuccin: false,
    themeColorHex: '#0b0b0d',
  });
  assert.deepEqual(resolveThemeState('system', 'mauve', false), {
    themeMode: 'system',
    accent: 'mauve',
    scheme: 'light',
    palette: 'default-light',
    isCatppuccin: false,
    themeColorHex: '#fafafa',
  });
  assert.deepEqual(resolveThemeState('catppuccin-latte', 'blue', true), {
    themeMode: 'catppuccin-latte',
    accent: 'blue',
    scheme: 'light',
    palette: 'catppuccin-latte',
    isCatppuccin: true,
    themeColorHex: '#eff1f5',
  });
  assert.deepEqual(resolveThemeState('catppuccin-frappe', 'peach', false), {
    themeMode: 'catppuccin-frappe',
    accent: 'peach',
    scheme: 'dark',
    palette: 'catppuccin-frappe',
    isCatppuccin: true,
    themeColorHex: '#303446',
  });
  assert.deepEqual(resolveThemeState('catppuccin-macchiato', 'pink', false), {
    themeMode: 'catppuccin-macchiato',
    accent: 'pink',
    scheme: 'dark',
    palette: 'catppuccin-macchiato',
    isCatppuccin: true,
    themeColorHex: '#24273a',
  });
  assert.deepEqual(resolveThemeState('catppuccin-mocha', 'teal', false), {
    themeMode: 'catppuccin-mocha',
    accent: 'teal',
    scheme: 'dark',
    palette: 'catppuccin-mocha',
    isCatppuccin: true,
    themeColorHex: '#1e1e2e',
  });
});

test('选中强调色与命令面板明暗切换逻辑符合预期', () => {
  assert.equal(nextModeForAccentSelection('light', 'light'), 'catppuccin-latte');
  assert.equal(nextModeForAccentSelection('dark', 'dark'), 'catppuccin-mocha');
  assert.equal(nextModeForAccentSelection('system', 'dark'), 'catppuccin-mocha');
  assert.equal(nextModeForAccentSelection('catppuccin-frappe', 'dark'), 'catppuccin-frappe');
  assert.equal(nextModeForAccentSelection('catppuccin-macchiato', 'dark'), 'catppuccin-macchiato');

  assert.equal(nextThemeModeOnToggle('dark', 'dark'), 'light');
  assert.equal(nextThemeModeOnToggle('light', 'light'), 'dark');
  assert.equal(nextThemeModeOnToggle('system', 'light'), 'dark');
  assert.equal(nextThemeModeOnToggle('catppuccin-frappe', 'dark', 'catppuccin-frappe'), 'catppuccin-latte');
  assert.equal(nextThemeModeOnToggle('catppuccin-latte', 'light', 'catppuccin-frappe'), 'catppuccin-frappe');
});

test('MeshGradient 着色器配色对默认黑白钴蓝原样保留且 Catppuccin 全部合法', () => {
  assert.deepEqual(resolveLoginShaderColors('default-dark', 'mauve'), ['#2563eb', '#18181b', '#09090b', '#0ea5e9']);
  assert.deepEqual(resolveLoginShaderColors('default-light', 'mauve'), ['#93c5fd', '#f4f4f5', '#ffffff', '#bae6fd']);
  assert.deepEqual(resolveLiveSurfaceColors('default-light', 'mauve'), ['#60a5fa', '#2563eb', '#a1a1aa', '#38bdf8']);
  assert.deepEqual(resolveLiveSurfaceColors('default-dark', 'mauve'), ['#3b82f6', '#1d4ed8', '#3f3f46', '#0ea5e9']);

  for (const palette of ['catppuccin-latte', 'catppuccin-frappe', 'catppuccin-macchiato', 'catppuccin-mocha']) {
    for (const accent of CATPPUCCIN_ACCENTS) {
      for (const colors of [resolveLoginShaderColors(palette, accent.id), resolveLiveSurfaceColors(palette, accent.id)]) {
        assert.equal(colors.length, 4);
        for (const color of colors) {
          assert.match(color, /^#[0-9a-f]{6}$/);
        }
      }
    }
  }
});
