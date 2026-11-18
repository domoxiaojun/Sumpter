import { Monitor, Moon, Palette, Sun } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';
import {
  CATPPUCCIN_ACCENTS,
  THEME_MODES,
  getCatppuccinAccentMeta,
  getThemeModeMeta,
  isCatppuccinMode,
} from '../utils/themeCatalog.js';
import { Button } from './ui/button.jsx';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from './ui/dropdown-menu.jsx';

const CLASSIC_MODES = THEME_MODES.filter((item) => item.group === 'classic');
const CATPPUCCIN_MODES = THEME_MODES.filter((item) => item.group === 'catppuccin');

function renderThemeIcon(themeMode) {
  if (isCatppuccinMode(themeMode)) {
    return <Palette className="size-4 text-primary" />;
  }
  if (themeMode === 'system') {
    return <Monitor className="size-4" />;
  }
  if (themeMode === 'light') {
    return <Sun className="size-4" />;
  }
  return <Moon className="size-4" />;
}

export function ThemePicker({ triggerClassName }) {
  const {
    themeMode,
    setThemeMode,
    catppuccinAccent,
    selectCatppuccinAccent,
  } = useAppControls();

  const currentMode = getThemeModeMeta(themeMode);
  const currentAccent = getCatppuccinAccentMeta(catppuccinAccent);
  const isCatppuccin = isCatppuccinMode(themeMode);
  const title = isCatppuccin
    ? `当前主题：Catppuccin ${currentMode.shortLabel} · ${currentAccent.shortLabel}`
    : `当前主题：${currentMode.label}`;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="icon"
          className={cn('size-8 rounded-xl border-border/80 bg-card/70 shadow-none', triggerClassName)}
          title={title}
          aria-label="选择界面主题"
        >
          {renderThemeIcon(themeMode)}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64 p-1.5">
        <DropdownMenuRadioGroup value={themeMode} onValueChange={setThemeMode}>
          <DropdownMenuLabel>经典黑白（钴蓝）</DropdownMenuLabel>
          {CLASSIC_MODES.map((mode) => (
            <DropdownMenuRadioItem key={mode.id} value={mode.id} data-theme-option={mode.id}>
              <span className="truncate">{mode.label}</span>
              <span className="ml-auto flex shrink-0 items-center gap-1" aria-hidden="true">
                <span className={cn('size-3.5 rounded-full border border-border/80', mode.previewClass)} />
                {mode.id !== 'system' && (
                  <span
                    className={cn(
                      'size-2.5 rounded-full',
                      mode.id === 'light' ? 'theme-accent-dot-light' : 'theme-accent-dot-dark',
                    )}
                  />
                )}
              </span>
            </DropdownMenuRadioItem>
          ))}

          <DropdownMenuSeparator />
          <DropdownMenuLabel>Catppuccin 调色板</DropdownMenuLabel>
          {CATPPUCCIN_MODES.map((mode) => (
            <DropdownMenuRadioItem key={mode.id} value={mode.id} data-theme-option={mode.id}>
              <span className="truncate">{mode.label}</span>
              <span className="ml-auto flex shrink-0 items-center gap-1" aria-hidden="true">
                <span className={cn('size-3.5 rounded-full border border-border/80', mode.previewClass)} />
                <span className={cn('size-2.5 rounded-full', `theme-accent-dot-${mode.id}`)} />
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>

        <DropdownMenuSeparator />
        <div className="px-2 pt-1 pb-1.5">
          <div className="mb-1.5 flex items-center justify-between text-[11px]">
            <span className="font-semibold text-muted-foreground">Catppuccin 强调色</span>
            <span className="font-medium text-foreground">
              {isCatppuccin ? currentAccent.shortLabel : `${currentAccent.shortLabel}（点击启用）`}
            </span>
          </div>
          <div
            role="radiogroup"
            aria-label="Catppuccin 强调色"
            className="grid grid-cols-8 gap-1"
          >
            {CATPPUCCIN_ACCENTS.map((accent) => {
              const selected = catppuccinAccent === accent.id;
              return (
                <button
                  key={accent.id}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  aria-label={`强调色：${accent.label}`}
                  title={accent.label}
                  onClick={() => selectCatppuccinAccent(accent.id)}
                  className={cn(
                    'flex size-6 items-center justify-center rounded-full transition-transform outline-none focus-visible:ring-2 focus-visible:ring-ring',
                    selected && isCatppuccin
                      ? 'ring-2 ring-primary ring-offset-2 ring-offset-popover'
                      : selected
                        ? 'ring-1 ring-border ring-offset-1 ring-offset-popover'
                        : 'hover:scale-110',
                  )}
                >
                  <span
                    className={cn(
                      'size-4 rounded-full border border-black/10 dark:border-white/15',
                      accent.swatchClass,
                    )}
                    aria-hidden="true"
                  />
                </button>
              );
            })}
          </div>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
