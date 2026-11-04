import { useLayoutEffect, useRef } from 'react';
import { useAppControls } from '../context/AppContext.jsx';

export function useUnsavedChanges(dirty, saving = false) {
  const { registerNavigationGuard } = useAppControls();
  const state = useRef({ dirty, saving });
  state.current = { dirty, saving };
  useLayoutEffect(() => registerNavigationGuard(() => state.current), [registerNavigationGuard]);
}
