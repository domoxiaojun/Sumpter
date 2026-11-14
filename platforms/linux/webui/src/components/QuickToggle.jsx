import { cn } from '../lib/utils.js';
import { Switch } from './ui/switch.jsx';

export function QuickToggle({ checked, onChange, disabled = false, label = '', title = '', ariaLabel = '' }) {
  const accessibleLabel = ariaLabel || title || `${label || '状态'}${checked ? '（已启用）' : '（已停用）'}`;
  return (
    <label
      className={cn(
        'inline-flex items-center gap-2 select-none',
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer',
      )}
      title={title}
    >
      <Switch
        checked={Boolean(checked)}
        onCheckedChange={(value) => !disabled && onChange && onChange(value)}
        disabled={disabled}
        aria-label={accessibleLabel}
      />
      {label && <span className="text-xs font-medium text-foreground">{label}</span>}
    </label>
  );
}
