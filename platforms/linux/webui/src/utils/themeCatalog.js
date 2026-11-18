// 统一管理 WebUI 的经典黑白主题与 Catppuccin 官方四风味、八强调色目录与解析纯函数。

export const THEME_MODES = [
  {
    id: 'system',
    label: '跟随系统',
    shortLabel: '跟随系统',
    description: '随系统深浅自动切换纯白 / 曜石双色',
    group: 'classic',
    scheme: 'system',
    previewClass: 'theme-preview-system',
  },
  {
    id: 'light',
    label: '纯白浅色',
    shortLabel: '纯白浅色',
    description: '净白画布搭配现代钴蓝',
    group: 'classic',
    scheme: 'light',
    previewClass: 'theme-preview-light',
  },
  {
    id: 'dark',
    label: '曜石深色',
    shortLabel: '曜石深色',
    description: '中性炭黑搭配现代钴蓝',
    group: 'classic',
    scheme: 'dark',
    previewClass: 'theme-preview-dark',
  },
  {
    id: 'catppuccin-latte',
    label: 'Latte · 拿铁',
    shortLabel: 'Latte',
    description: 'Catppuccin 柔和燕麦浅色',
    group: 'catppuccin',
    scheme: 'light',
    previewClass: 'theme-preview-latte',
  },
  {
    id: 'catppuccin-frappe',
    label: 'Frappé · 星冰乐',
    shortLabel: 'Frappé',
    description: 'Catppuccin 雾感柔暗色',
    group: 'catppuccin',
    scheme: 'dark',
    previewClass: 'theme-preview-frappe',
  },
  {
    id: 'catppuccin-macchiato',
    label: 'Macchiato · 玛奇朵',
    shortLabel: 'Macchiato',
    description: 'Catppuccin 均衡靛夜暗色',
    group: 'catppuccin',
    scheme: 'dark',
    previewClass: 'theme-preview-macchiato',
  },
  {
    id: 'catppuccin-mocha',
    label: 'Mocha · 摩卡',
    shortLabel: 'Mocha',
    description: 'Catppuccin 深邃旗舰暗色',
    group: 'catppuccin',
    scheme: 'dark',
    previewClass: 'theme-preview-mocha',
  },
];

export const CATPPUCCIN_ACCENTS = [
  {
    id: 'mauve',
    label: '丁香紫 (Mauve)',
    shortLabel: '丁香紫',
    swatchClass: 'theme-swatch-mauve',
    hex: {
      'catppuccin-latte': '#8839ef',
      'catppuccin-frappe': '#ca9ee6',
      'catppuccin-macchiato': '#c6a0f6',
      'catppuccin-mocha': '#cba6f7',
    },
  },
  {
    id: 'blue',
    label: '晴空蓝 (Blue)',
    shortLabel: '晴空蓝',
    swatchClass: 'theme-swatch-blue',
    hex: {
      'catppuccin-latte': '#1e66f5',
      'catppuccin-frappe': '#8caaee',
      'catppuccin-macchiato': '#8aadf4',
      'catppuccin-mocha': '#89b4fa',
    },
  },
  {
    id: 'sapphire',
    label: '宝石蓝 (Sapphire)',
    shortLabel: '宝石蓝',
    swatchClass: 'theme-swatch-sapphire',
    hex: {
      'catppuccin-latte': '#209fb5',
      'catppuccin-frappe': '#85c1dc',
      'catppuccin-macchiato': '#7dc4e4',
      'catppuccin-mocha': '#74c7ec',
    },
  },
  {
    id: 'teal',
    label: '青碧色 (Teal)',
    shortLabel: '青碧色',
    swatchClass: 'theme-swatch-teal',
    hex: {
      'catppuccin-latte': '#179299',
      'catppuccin-frappe': '#81c8be',
      'catppuccin-macchiato': '#8bd5ca',
      'catppuccin-mocha': '#94e2d5',
    },
  },
  {
    id: 'green',
    label: '鼠尾草绿 (Green)',
    shortLabel: '鼠尾草绿',
    swatchClass: 'theme-swatch-green',
    hex: {
      'catppuccin-latte': '#40a02b',
      'catppuccin-frappe': '#a6d189',
      'catppuccin-macchiato': '#a6da95',
      'catppuccin-mocha': '#a6e3a1',
    },
  },
  {
    id: 'peach',
    label: '蜜桃橙 (Peach)',
    shortLabel: '蜜桃橙',
    swatchClass: 'theme-swatch-peach',
    hex: {
      'catppuccin-latte': '#fe640b',
      'catppuccin-frappe': '#ef9f76',
      'catppuccin-macchiato': '#f5a97f',
      'catppuccin-mocha': '#fab387',
    },
  },
  {
    id: 'pink',
    label: '樱花粉 (Pink)',
    shortLabel: '樱花粉',
    swatchClass: 'theme-swatch-pink',
    hex: {
      'catppuccin-latte': '#ea76cb',
      'catppuccin-frappe': '#f4b8e4',
      'catppuccin-macchiato': '#f5bde6',
      'catppuccin-mocha': '#f5c2e7',
    },
  },
  {
    id: 'lavender',
    label: '薰衣草 (Lavender)',
    shortLabel: '薰衣草',
    swatchClass: 'theme-swatch-lavender',
    hex: {
      'catppuccin-latte': '#7287fd',
      'catppuccin-frappe': '#babbf1',
      'catppuccin-macchiato': '#b4befe',
      'catppuccin-mocha': '#b4befe',
    },
  },
];

const THEME_MODE_IDS = new Set(THEME_MODES.map((item) => item.id));
const ACCENT_MAP = new Map(CATPPUCCIN_ACCENTS.map((item) => [item.id, item]));
const THEME_MODE_MAP = new Map(THEME_MODES.map((item) => [item.id, item]));

const PALETTE_THEME_COLORS = {
  'default-light': '#fafafa',
  'default-dark': '#0b0b0d',
  'catppuccin-latte': '#eff1f5',
  'catppuccin-frappe': '#303446',
  'catppuccin-macchiato': '#24273a',
  'catppuccin-mocha': '#1e1e2e',
};

const CATPPUCCIN_FLAVOR_TOKENS = {
  'catppuccin-latte': {
    base: '#eff1f5',
    mantle: '#e6e9ef',
    crust: '#dce0e8',
    sapphire: '#209fb5',
    lavender: '#7287fd',
    sky: '#04a5e5',
  },
  'catppuccin-frappe': {
    base: '#303446',
    mantle: '#292c3c',
    crust: '#232634',
    sapphire: '#85c1dc',
    lavender: '#babbf1',
    sky: '#99d1db',
  },
  'catppuccin-macchiato': {
    base: '#24273a',
    mantle: '#1e2030',
    crust: '#181926',
    sapphire: '#7dc4e4',
    lavender: '#b4befe',
    sky: '#91d7e3',
  },
  'catppuccin-mocha': {
    base: '#1e1e2e',
    mantle: '#181825',
    crust: '#11111b',
    sapphire: '#74c7ec',
    lavender: '#b4befe',
    sky: '#89dceb',
  },
};

export function normalizeThemeMode(raw) {
  return THEME_MODE_IDS.has(raw) ? raw : 'dark';
}

export function normalizeCatppuccinAccent(raw) {
  return ACCENT_MAP.has(raw) ? raw : 'mauve';
}

export function isCatppuccinMode(mode) {
  return typeof mode === 'string' && mode.startsWith('catppuccin-');
}

export function isCatppuccinDarkMode(mode) {
  return mode === 'catppuccin-frappe'
    || mode === 'catppuccin-macchiato'
    || mode === 'catppuccin-mocha';
}

export function getThemeModeMeta(mode) {
  return THEME_MODE_MAP.get(normalizeThemeMode(mode));
}

export function getCatppuccinAccentMeta(accent) {
  return ACCENT_MAP.get(normalizeCatppuccinAccent(accent));
}

export function resolveThemeState(themeMode, catppuccinAccent, prefersDark = true) {
  const mode = normalizeThemeMode(themeMode);
  const accent = normalizeCatppuccinAccent(catppuccinAccent);
  const meta = THEME_MODE_MAP.get(mode);
  const scheme = meta.scheme === 'system' ? (prefersDark ? 'dark' : 'light') : meta.scheme;
  const isCatppuccin = isCatppuccinMode(mode);
  const palette = isCatppuccin
    ? mode
    : (scheme === 'dark' ? 'default-dark' : 'default-light');

  return {
    themeMode: mode,
    accent,
    scheme,
    palette,
    isCatppuccin,
    themeColorHex: PALETTE_THEME_COLORS[palette],
  };
}

export function nextThemeModeOnToggle(themeMode, scheme, lastCatppuccinDark = 'catppuccin-mocha') {
  const mode = normalizeThemeMode(themeMode);
  if (isCatppuccinMode(mode)) {
    if (mode === 'catppuccin-latte') {
      return isCatppuccinDarkMode(lastCatppuccinDark) ? lastCatppuccinDark : 'catppuccin-mocha';
    }
    return 'catppuccin-latte';
  }
  return scheme === 'dark' ? 'light' : 'dark';
}

export function nextModeForAccentSelection(themeMode, scheme) {
  const mode = normalizeThemeMode(themeMode);
  if (isCatppuccinMode(mode)) return mode;
  return scheme === 'light' ? 'catppuccin-latte' : 'catppuccin-mocha';
}

export function resolveLoginShaderColors(palette, accent) {
  if (palette === 'default-dark') {
    return ['#2563eb', '#18181b', '#09090b', '#0ea5e9'];
  }
  if (palette === 'default-light') {
    return ['#93c5fd', '#f4f4f5', '#ffffff', '#bae6fd'];
  }
  const flavor = CATPPUCCIN_FLAVOR_TOKENS[palette] ?? CATPPUCCIN_FLAVOR_TOKENS['catppuccin-mocha'];
  const accentMeta = getCatppuccinAccentMeta(accent);
  const accentHex = accentMeta.hex[palette] ?? accentMeta.hex['catppuccin-mocha'];
  if (palette === 'catppuccin-latte') {
    return [accentHex, flavor.mantle, flavor.base, flavor.sapphire];
  }
  return [accentHex, flavor.mantle, flavor.crust, flavor.lavender];
}

export function resolveLiveSurfaceColors(palette, accent) {
  if (palette === 'default-light') {
    return ['#60a5fa', '#2563eb', '#a1a1aa', '#38bdf8'];
  }
  if (palette === 'default-dark') {
    return ['#3b82f6', '#1d4ed8', '#3f3f46', '#0ea5e9'];
  }
  const flavor = CATPPUCCIN_FLAVOR_TOKENS[palette] ?? CATPPUCCIN_FLAVOR_TOKENS['catppuccin-mocha'];
  const accentMeta = getCatppuccinAccentMeta(accent);
  const accentHex = accentMeta.hex[palette] ?? accentMeta.hex['catppuccin-mocha'];
  return [flavor.sapphire, accentHex, flavor.lavender, flavor.sky];
}
