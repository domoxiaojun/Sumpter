// daemon 的 Admin CSP 是 style-src 'self'，运行时注入的 <style> 会被拦截。
// 图表系列颜色改为容器上的 CSS 自定义属性：React 通过 CSSOM 写入，不受该策略限制。
export function chartColorVariables(config) {
  const variables = {};
  for (const [key, item] of Object.entries(config || {})) {
    const light = item?.theme?.light || item?.color || item?.theme?.dark;
    const dark = item?.theme?.dark || item?.color || item?.theme?.light;
    if (!light) continue;
    // 根元素按主题声明了 color-scheme，light-dark() 无需脚本即可随主题切换。
    variables[`--color-${key}`] = light === dark ? light : `light-dark(${light}, ${dark})`;
  }
  return variables;
}
