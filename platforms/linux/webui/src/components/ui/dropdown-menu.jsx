import { createContext, useCallback, useContext, useState } from 'react';
import { Check } from 'lucide-react';
import { DropdownMenu as DropdownMenuPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

const DropdownMenuStateContext = createContext(null);

export function DropdownMenu({ open: openProp, defaultOpen = false, onOpenChange, ...props }) {
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen);
  const isControlled = openProp !== undefined;
  const open = isControlled ? openProp : uncontrolledOpen;

  const handleOpenChange = useCallback((nextOpen) => {
    if (!isControlled) setUncontrolledOpen(nextOpen);
    onOpenChange?.(nextOpen);
  }, [isControlled, onOpenChange]);

  return (
    <DropdownMenuStateContext.Provider value={{ open, setOpen: handleOpenChange }}>
      <DropdownMenuPrimitive.Root
        data-slot="dropdown-menu"
        open={open}
        onOpenChange={handleOpenChange}
        {...props}
      />
    </DropdownMenuStateContext.Provider>
  );
}

export function DropdownMenuTrigger({ onClick, disabled, ...props }) {
  const menuState = useContext(DropdownMenuStateContext);
  return (
    <DropdownMenuPrimitive.Trigger
      data-slot="dropdown-menu-trigger"
      disabled={disabled}
      onClick={(event) => {
        onClick?.(event);
        // Radix DropdownMenuTrigger 仅监听 pointerdown 与键盘按键；
        // 读屏器虚拟光标或编程式 .click() 触发的合成 click（detail === 0）在此补齐切换。
        if (!event.defaultPrevented && !disabled && event.detail === 0 && menuState) {
          menuState.setOpen(!menuState.open);
        }
      }}
      {...props}
    />
  );
}

export function DropdownMenuContent({ className, sideOffset = 6, align = 'end', ...props }) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        align={align}
        className={cn(
          'surface-popover z-50 min-w-[11rem] overflow-hidden rounded-xl border border-border/85 bg-popover/95 p-1.5 text-popover-foreground backdrop-blur-md',
          'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 duration-150',
          className,
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

export function DropdownMenuItem({ className, variant = 'default', ...props }) {
  return (
    <DropdownMenuPrimitive.Item
      data-slot="dropdown-menu-item"
      data-variant={variant}
      className={cn(
        'relative flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-2 text-xs font-medium outline-hidden select-none transition-colors',
        'focus:bg-accent focus:text-accent-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/15',
        'data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:size-3.5 [&_svg]:shrink-0 [&_svg]:text-muted-foreground',
        className,
      )}
      {...props}
    />
  );
}

export function DropdownMenuLabel({ className, ...props }) {
  return (
    <DropdownMenuPrimitive.Label
      data-slot="dropdown-menu-label"
      className={cn('px-2.5 py-1.5 text-[11px] font-semibold tracking-wide text-muted-foreground select-none', className)}
      {...props}
    />
  );
}

export function DropdownMenuRadioGroup({ ...props }) {
  return <DropdownMenuPrimitive.RadioGroup data-slot="dropdown-menu-radio-group" {...props} />;
}

export function DropdownMenuRadioItem({ className, children, ...props }) {
  return (
    <DropdownMenuPrimitive.RadioItem
      data-slot="dropdown-menu-radio-item"
      className={cn(
        'relative flex cursor-pointer items-center gap-2 rounded-lg py-1.5 pr-2.5 pl-7 text-xs font-medium outline-hidden select-none transition-colors',
        'focus:bg-accent focus:text-accent-foreground data-[state=checked]:text-foreground',
        'data-disabled:pointer-events-none data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <span className="pointer-events-none absolute left-2 flex size-3.5 items-center justify-center">
        <DropdownMenuPrimitive.ItemIndicator>
          <Check className="size-3.5 text-primary" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.RadioItem>
  );
}

export function DropdownMenuSeparator({ className, ...props }) {
  return (
    <DropdownMenuPrimitive.Separator
      data-slot="dropdown-menu-separator"
      className={cn('-mx-1 my-1 h-px bg-border/80', className)}
      {...props}
    />
  );
}
