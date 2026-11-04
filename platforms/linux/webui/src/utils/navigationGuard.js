// Read live component state without persisting drafts (which may contain secrets).
export function createNavigationGuard({ confirmDiscard, notifyBusy }) {
  const guards = new Set();
  const states = () => [...guards].map(read => read());
  return {
    register(read) {
      guards.add(read);
      return () => guards.delete(read);
    },
    shouldBlockUnload() {
      return states().some(state => state.dirty || state.saving);
    },
    canLeave() {
      const current = states();
      if (current.some(state => state.saving)) {
        notifyBusy();
        return false;
      }
      return !current.some(state => state.dirty)
        || confirmDiscard('当前有未保存的更改。离开会放弃这些更改，是否继续？');
    },
  };
}
