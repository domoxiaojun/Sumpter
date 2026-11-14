// 与 CLI 冒烟和交互式浏览器验收共用的真实 DOM 行为断言；不读取 React 内部状态。
export async function runInteractionChecks({ open, click, focus, press, fill, evaluate, waitFor, setViewport, check }) {
  const dialog = '[data-slot="dialog-content"]';
  const commandInput = '[aria-controls="command-palette-results"][role="combobox"]';
  const commandTrigger = 'button[title="打开快捷指令面板 (⌘K)"]';
  const sidebarTrigger = '[data-slot="sidebar-trigger"]';
  const locked = `document.body.hasAttribute('data-scroll-locked') && getComputedStyle(document.body).overflow === 'hidden'`;
  const unlocked = `!document.body.hasAttribute('data-scroll-locked') && getComputedStyle(document.body).overflow !== 'hidden'`;
  const selectedVisible = `(() => {
    const input = document.querySelector('${commandInput}');
    const option = input && document.getElementById(input.getAttribute('aria-activedescendant'));
    const list = document.getElementById('command-palette-results');
    if (!option || !list) return false;
    const a = option.getBoundingClientRect(), b = list.getBoundingClientRect();
    return a.top >= b.top - 1 && a.bottom <= b.bottom + 1 && document.activeElement === input;
  })()`;
  const released = async (label) => check(await waitFor(unlocked), `${label}关闭后释放背景滚动锁`);
  const closedDialog = () => waitFor(`!document.querySelector('${dialog}')`);

  await setViewport(1440, 1000);
  await open('providers');
  await click('button[aria-label="编辑入口「Anthropic 官方直连」"]');
  check(await waitFor(locked), '入口编辑弹窗在 Admin CSP 下锁住背景');
  check(await evaluate(`getComputedStyle(document.body).marginRight === '0px' && getComputedStyle(document.documentElement).scrollbarGutter === 'stable'`), '滚动条槽位保持稳定且没有重复补偿边距');
  await focus(`${dialog} [data-slot="dialog-close"]`);
  const before = await evaluate('window.scrollY');
  await press('End');
  await press('PageDown');
  check(await waitFor(`window.scrollY === ${before}`), '弹窗中 End / PageDown 不滚动背景页面');

  // 两层弹窗同时存在时，关闭上层不能提前解除下层的滚动锁。
  await press('k', { modifiers: 2 });
  check(await waitFor(`Number(document.body.getAttribute('data-scroll-locked')) >= 2`), '命令面板与编辑弹窗保留嵌套锁');
  await click(`${dialog}[aria-label="命令面板"] [data-slot="dialog-close"]`);
  check(await waitFor(`document.querySelectorAll('${dialog}').length === 1 && (${locked})`), '关闭上层后编辑弹窗仍锁住背景');
  await click(`${dialog} [data-slot="dialog-close"]`);
  await closedDialog();
  await released('编辑弹窗');

  await click('button[aria-haspopup="menu"]');
  check(await waitFor(locked), '用户菜单在 Admin CSP 下锁住背景');
  await press('Escape');
  await released('用户菜单');

  await click('[role="combobox"][aria-label="运行刷新间隔"]');
  check(await waitFor(locked), '刷新间隔选择器在 Admin CSP 下锁住背景');
  check(await evaluate(`getComputedStyle(document.querySelector('[data-radix-select-viewport]')).scrollbarWidth === 'none'`), '选择器使用静态滚动条样式');
  await press('Escape');
  await released('选择器');

  await click(sidebarTrigger);
  check(await waitFor(`document.querySelector('[data-slot="sidebar"]')?.dataset.state === 'collapsed'`), '侧栏可以收起');
  check(await evaluate(`(() => {
    const buttons = [...document.querySelectorAll('[data-slot="sidebar-menu-button"]')];
    return buttons.length === 9 && buttons.every(button => button.getAttribute('aria-label')?.trim() && !button.hasAttribute('tooltip'));
  })()`), '收起侧栏的九个导航保留名称且不透传伪 tooltip 属性');
  check(await evaluate(`Boolean(document.querySelector('button[aria-label="Sumpter · 运行"]'))`), '品牌按钮保留运行页名称');
  await focus('[data-slot="sidebar-menu-button"][aria-label="入口库"]');
  check(await waitFor(`document.querySelector('[role="tooltip"]')?.textContent === '入口库'`), '收起侧栏的焦点导航显示真实提示');
  await click(sidebarTrigger);

  await click(commandTrigger);
  await waitFor(`Boolean(document.querySelector('${commandInput}'))`);
  await focus(commandInput);
  for (let i = 0; i < 10; i += 1) await press('ArrowDown');
  check(await waitFor(selectedVisible), '方向键选择下方命令时选中项滚动到可见区域，焦点仍在搜索框');
  await press('ArrowDown');
  await press('ArrowDown');
  check(await waitFor(`${selectedVisible} && document.querySelector('${commandInput}').getAttribute('aria-activedescendant') === 'command-nav-run'`), '从末项向下循环回首项并滚动到顶部');
  await press('ArrowUp');
  check(await waitFor(`${selectedVisible} && document.querySelector('${commandInput}').getAttribute('aria-activedescendant') === 'command-act-theme'`), '从首项向上循环回末项并滚动到底部');
  await fill(commandInput, '安全');
  check(await waitFor(`${selectedVisible} && document.querySelector('${commandInput}').getAttribute('aria-activedescendant') === 'command-nav-security'`), '筛选后重置选中项和滚动位置');
  await evaluate(`document.querySelector('${commandInput}').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, isComposing: true}))`);
  check(await evaluate(`Boolean(document.querySelector('${commandInput}')) && location.hash === '#providers'`), '中文输入法组合阶段 Enter 不执行指令');
  await fill(commandInput, '');
  await setViewport(568, 320);
  check(await waitFor(`(() => {
    const box = document.querySelector('${dialog}').getBoundingClientRect();
    return box.top >= 15 && box.bottom <= innerHeight - 15;
  })()`), '小窗口中命令面板完整位于视口内');
  await focus(commandInput);
  await press('ArrowUp');
  check(await waitFor(selectedVisible), '小窗口中末项可见且搜索框继续持有焦点');
  await click(`${dialog} [data-slot="dialog-close"]`);
  await closedDialog();
  await released('命令面板');

  await setViewport(390, 844);
  await waitFor(`!document.querySelector('[data-slot="sidebar"]')`);
  await click(sidebarTrigger);
  check(await waitFor(`Boolean(document.querySelector('[data-slot="sheet-content"]')) && (${locked})`), '移动端导航抽屉锁住背景');
  await click('button[aria-label="关闭侧抽屉"]');
  await released('移动端导航抽屉');
  await setViewport(1440, 1000);
}
