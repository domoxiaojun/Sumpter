// Sumpter Linux Status for Scriptable
//
// 使用方法：
// 1. 在 iPhone 安装 Scriptable，新建脚本并粘贴本文件全部内容。
// 2. 先在 Scriptable 内手动运行一次，填写 Linux Web Admin 的 HTTPS 地址和管理员凭据。
// 3. 在桌面添加 Scriptable 小组件，选择本脚本；支持小号和中号。
//
// 安全边界：
// - 只调用登录、会话检查和只读状态接口，不会启停代理或修改配置。
// - 管理员密码和会话 Cookie 只保存在 iOS Keychain，不写进脚本。
// - 为避免明文传输管理员密码，仅接受 HTTPS 管理地址。

const SCRIPT_NAME = "Sumpter Linux Status";
const API_PREFIX = "/admin/api";
const SESSION_COOKIE_NAME = "kekulv_admin_session";
const SETTINGS_KEY = "kekulv.scriptable.status.settings.v1";
const PASSWORD_KEY = "kekulv.scriptable.status.password.v1";
const COOKIE_KEY = "kekulv.scriptable.status.cookie.v1";
const REFRESH_MINUTES = 5;
const REQUEST_TIMEOUT_SECONDS = 15;

await main();
Script.complete();

async function main() {
  const settings = await settingsForThisRun();
  if (!settings) {
    if (config.runsInWidget) {
      Script.setWidget(buildMessageWidget(
        "需要首次配置",
        "请在 Scriptable 中手动运行一次本脚本。",
        "setup"
      ));
    }
    return;
  }

  try {
    const snapshot = await fetchSnapshot(settings);
    const widget = buildStatusWidget(snapshot.status, snapshot.inFlight, settings);
    if (config.runsInWidget) {
      Script.setWidget(widget);
    } else {
      await widget.presentMedium();
    }
  } catch (error) {
    const widget = buildMessageWidget(
      error.title || "无法读取状态",
      error.userMessage || "请检查网络、管理地址和登录凭据。",
      error.kind || "network",
      settings
    );
    if (config.runsInWidget) {
      Script.setWidget(widget);
    } else {
      await widget.presentMedium();
    }
  }
}

async function settingsForThisRun() {
  const saved = loadSettings();
  if (config.runsInWidget) return saved;
  if (!saved) return promptForSettings(null);

  const menu = new Alert();
  menu.title = SCRIPT_NAME;
  menu.message = `当前地址：${saved.baseURL}`;
  menu.addAction("预览状态");
  menu.addAction("修改连接配置");
  menu.addDestructiveAction("清除本机配置");
  menu.addCancelAction("取消");
  const action = await menu.presentSheet();

  if (action === 0) return saved;
  if (action === 1) return promptForSettings(saved);
  if (action === 2) {
    clearStoredConfiguration();
    const alert = new Alert();
    alert.title = "已清除";
    alert.message = "管理地址、用户名、密码和登录会话已从 iOS Keychain 删除。";
    alert.addAction("好");
    await alert.presentAlert();
  }
  return null;
}

async function promptForSettings(current) {
  const alert = new Alert();
  alert.title = current ? "修改连接配置" : "连接 Sumpter Linux";
  alert.message = "请填写能从 iPhone 访问的 HTTPS Web Admin 地址。密码只保存到 iOS Keychain。";
  alert.addTextField("https://admin.example.com", current?.baseURL || "");
  alert.addTextField("管理员用户名", current?.username || "kkl");
  alert.addSecureTextField(current ? "管理员密码（留空表示不修改）" : "管理员密码", "");
  alert.addAction("保存");
  alert.addCancelAction("取消");

  const action = await alert.presentAlert();
  if (action < 0) return null;

  let baseURL;
  try {
    baseURL = normalizeBaseURL(alert.textFieldValue(0));
  } catch (error) {
    await showInputError(error.message);
    return promptForSettings(current);
  }

  const username = alert.textFieldValue(1).trim();
  const newPassword = alert.textFieldValue(2);
  const oldPassword = keychainGet(PASSWORD_KEY);
  const password = newPassword || oldPassword;

  if (!username) {
    await showInputError("管理员用户名不能为空。");
    return promptForSettings(current);
  }
  if (!password) {
    await showInputError("首次配置必须填写管理员密码。");
    return promptForSettings(current);
  }

  const next = { baseURL, username };
  if (!current || current.baseURL !== baseURL || current.username !== username || newPassword) {
    keychainRemove(COOKIE_KEY);
  }
  Keychain.set(SETTINGS_KEY, JSON.stringify(next));
  Keychain.set(PASSWORD_KEY, password);
  return next;
}

async function showInputError(message) {
  const alert = new Alert();
  alert.title = "配置无效";
  alert.message = message;
  alert.addAction("返回修改");
  await alert.presentAlert();
}

function normalizeBaseURL(input) {
  let value = String(input || "").trim();
  value = value.replace(/\/+$/, "");
  value = value.replace(/\/admin(?:\/.*)?$/i, "");
  if (!/^https:\/\/[^/?#]+$/i.test(value)) {
    throw new Error("请输入纯 HTTPS 站点地址，例如 https://admin.example.com；不要填写 /admin/ 后面的路径。");
  }
  return value;
}

function loadSettings() {
  const raw = keychainGet(SETTINGS_KEY);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    if (!value.baseURL || !value.username) return null;
    return value;
  } catch (_) {
    return null;
  }
}

function clearStoredConfiguration() {
  keychainRemove(SETTINGS_KEY);
  keychainRemove(PASSWORD_KEY);
  keychainRemove(COOKIE_KEY);
}

function keychainGet(key) {
  return Keychain.contains(key) ? Keychain.get(key) : null;
}

function keychainRemove(key) {
  if (Keychain.contains(key)) Keychain.remove(key);
}

async function fetchSnapshot(settings) {
  await ensureAuthenticated(settings);
  let statusResponse = await sendJSON(settings, "/status");

  // daemon 可能刚好重启并清空了内存会话；重新登录一次后再读。
  if (statusResponse.statusCode === 401) {
    keychainRemove(COOKIE_KEY);
    await login(settings);
    statusResponse = await sendJSON(settings, "/status");
  }

  if (statusResponse.statusCode !== 200 || !statusResponse.data) {
    throw apiError(statusResponse, "状态接口暂时不可用。");
  }

  let eventsResponse = await sendJSON(settings, "/runtime/events?limit=200&kind=client");
  if (eventsResponse.statusCode === 401) {
    keychainRemove(COOKIE_KEY);
    await login(settings);
    eventsResponse = await sendJSON(settings, "/runtime/events?limit=200&kind=client");
  }
  if (eventsResponse.statusCode !== 200 || !eventsResponse.data) {
    throw apiError(eventsResponse, "进行中请求接口暂时不可用。");
  }

  const events = Array.isArray(eventsResponse.data.events) ? eventsResponse.data.events : [];
  return {
    status: statusResponse.data,
    // 只显示客户端请求本身；upstream 事件是内部重试，不单独占一个小组件条目。
    inFlight: events.filter((event) => isInFlight(event) && String(event.kind || "") === "client"),
  };
}

function isInFlight(event) {
  const phase = String(event?.phase || "");
  return phase === "inFlight" || phase === "in_flight";
}

async function ensureAuthenticated(settings) {
  if (keychainGet(COOKIE_KEY)) {
    const session = await sendJSON(settings, "/auth/session");
    if (session.statusCode === 200 && session.data?.authenticated) return;
    keychainRemove(COOKIE_KEY);
  }
  await login(settings);
}

async function login(settings) {
  const password = keychainGet(PASSWORD_KEY);
  if (!password) {
    throw userError("需要登录", "请在 Scriptable 中手动运行脚本并重新填写管理员密码。", "setup");
  }

  const response = await sendJSON(settings, "/auth/login", {
    method: "POST",
    body: { username: settings.username, password },
    includeCookie: false,
  });

  if (response.statusCode === 401) {
    throw userError("登录失败", "管理员用户名或密码不正确，请手动运行脚本修改配置。", "auth");
  }
  if (response.statusCode !== 200 || !response.data?.authenticated) {
    throw apiError(response, "Linux Web Admin 拒绝了登录请求。");
  }
  if (!keychainGet(COOKIE_KEY)) {
    throw userError("无法保存会话", "Scriptable 没有收到登录 Cookie，请检查 HTTPS 反向代理是否保留 Set-Cookie。", "auth");
  }
}

async function sendJSON(settings, path, options = {}) {
  const request = new Request(`${settings.baseURL}${API_PREFIX}${path}`);
  request.method = options.method || "GET";
  request.timeoutInterval = REQUEST_TIMEOUT_SECONDS;
  request.headers = { Accept: "application/json" };

  const cookie = options.includeCookie === false ? null : keychainGet(COOKIE_KEY);
  if (cookie) request.headers.Cookie = `${SESSION_COOKIE_NAME}=${cookie}`;
  if (options.body !== undefined) {
    request.headers["Content-Type"] = "application/json";
    request.body = JSON.stringify(options.body);
  }

  let text;
  try {
    text = await request.loadString();
  } catch (_) {
    throw userError("无法连接 Linux", "请求超时、网络不可达，或 HTTPS 证书不受 iPhone 信任。", "network");
  }

  updateSessionCookie(request.response);
  const statusCode = Number(request.response?.statusCode || 0);
  let data = null;
  if (text && text.trim()) {
    try {
      data = JSON.parse(text);
    } catch (_) {
      throw userError("响应格式错误", "管理地址返回的不是 Sumpter Admin JSON，请检查反向代理路径。", "network");
    }
  }
  return { statusCode, data };
}

function updateSessionCookie(response) {
  const cookies = Array.isArray(response?.cookies) ? response.cookies : [];
  const cookieObject = cookies.find((cookie) => cookie.name === SESSION_COOKIE_NAME);
  if (cookieObject) {
    storeCookieValue(String(cookieObject.value || ""));
    return;
  }

  const headers = response?.headers || {};
  const entry = Object.entries(headers).find(([name]) => name.toLowerCase() === "set-cookie");
  if (!entry) return;
  const match = String(entry[1]).match(/(?:^|[,\s])kekulv_admin_session=([^;]*)/i);
  if (match) storeCookieValue(match[1]);
}

function storeCookieValue(value) {
  if (value) Keychain.set(COOKIE_KEY, value);
  else keychainRemove(COOKIE_KEY);
}

function apiError(response, fallback) {
  if (response.statusCode === 401) {
    return userError("登录已失效", "请手动运行脚本重新登录。", "auth");
  }
  if (response.statusCode === 404) {
    return userError("接口不存在", "请确认地址指向当前 Sumpter Linux Web Admin。", "network");
  }
  return userError("无法读取状态", fallback, "network");
}

function userError(title, message, kind) {
  const error = new Error(message);
  error.title = title;
  error.userMessage = message;
  error.kind = kind;
  return error;
}

function buildStatusWidget(status, inFlight, settings) {
  const widget = new ListWidget();
  applyWidgetBase(widget);
  widget.url = `${settings.baseURL}/admin/`;
  widget.refreshAfterDate = new Date(Date.now() + REFRESH_MINUTES * 60 * 1000);

  const family = config.widgetFamily || "medium";
  const compact = family === "small";
  const appearance = statusAppearance(status, inFlight);

  const header = widget.addStack();
  header.centerAlignContent();
  const symbol = namedSymbol("server.rack", "desktopcomputer");
  const icon = header.addImage(symbol.image);
  icon.imageSize = new Size(17, 17);
  icon.tintColor = appearance.color;
  header.addSpacer(7);
  const title = header.addText("Sumpter Linux");
  title.font = Font.semiboldSystemFont(14);
  title.textColor = palette().primary;
  header.addSpacer();
  const version = header.addText(`v${status.version || "-"}`);
  version.font = Font.mediumMonospacedSystemFont(10);
  version.textColor = palette().muted;

  widget.addSpacer(compact ? 12 : 14);
  const stateRow = widget.addStack();
  stateRow.centerAlignContent();
  const dot = stateRow.addText("●");
  dot.font = Font.systemFont(11);
  dot.textColor = appearance.color;
  stateRow.addSpacer(6);
  const state = stateRow.addText(appearance.label);
  state.font = Font.boldSystemFont(compact ? 18 : 20);
  state.textColor = palette().primary;
  state.lineLimit = 1;
  state.minimumScaleFactor = 0.75;

  widget.addSpacer(4);
  const subtitle = widget.addText(`代理在线 ${formatUptime(status.uptimeSeconds)} · ${status.endpoints ?? status.providers ?? 0} 个入口`);
  subtitle.font = Font.systemFont(11);
  subtitle.textColor = palette().secondary;
  subtitle.lineLimit = 1;

  widget.addSpacer(compact ? 13 : 16);
  if (!inFlight.length) {
    widget.addSpacer(compact ? 14 : 18);
    const empty = widget.addText(status.running ? "当前无进行中请求" : "代理已停止");
    empty.font = Font.semiboldSystemFont(compact ? 14 : 17);
    empty.textColor = palette().primary;
    empty.lineLimit = 2;
    widget.addSpacer(5);
    const emptyHint = widget.addText(status.running ? "有新请求时会在这里显示" : "点按打开 Web Admin");
    emptyHint.font = Font.systemFont(10);
    emptyHint.textColor = palette().muted;
  } else if (compact) {
    widget.addSpacer(14);
    const first = inFlight[0];
    const model = widget.addText(eventModel(first));
    model.font = Font.boldSystemFont(15);
    model.textColor = palette().primary;
    model.lineLimit = 1;
    model.minimumScaleFactor = 0.65;
    widget.addSpacer(4);
    const elapsed = widget.addText(`${formatElapsed(first)} · ${inFlight.length > 1 ? `另有 ${inFlight.length - 1} 个` : "进行中"}`);
    elapsed.font = Font.systemFont(10);
    elapsed.textColor = palette().secondary;
  } else {
    widget.addSpacer(11);
    const visible = inFlight.slice(0, 3);
    visible.forEach((event, index) => {
      if (index > 0) widget.addSpacer(6);
      addLiveRequestRow(widget, event);
    });
    if (inFlight.length > visible.length) {
      widget.addSpacer(6);
      const more = widget.addText(`还有 ${inFlight.length - visible.length} 个进行中请求`);
      more.font = Font.systemFont(10);
      more.textColor = palette().muted;
    }
  }

  widget.addSpacer();
  const footer = widget.addStack();
  footer.centerAlignContent();
  const detail = status.lastError ? "有最近错误 · 点按查看" : `刷新 ${formatClock(new Date())}`;
  const footerText = footer.addText(detail);
  footerText.font = Font.systemFont(9);
  footerText.textColor = status.lastError ? palette().warning : palette().muted;
  return widget;
}

function addLiveRequestRow(widget, event) {
  const row = widget.addStack();
  row.layoutHorizontally();
  row.backgroundColor = palette().card;
  row.cornerRadius = 8;
  row.setPadding(6, 8, 6, 8);
  const text = row.addStack();
  text.layoutVertically();
  const model = text.addText(eventModel(event));
  model.font = Font.semiboldSystemFont(11);
  model.textColor = palette().primary;
  model.lineLimit = 1;
  model.minimumScaleFactor = 0.65;
  text.addSpacer(2);
  const context = text.addText(`${eventEndpoint(event)} · ${eventClient(event)}`);
  context.font = Font.systemFont(9);
  context.textColor = palette().muted;
  context.lineLimit = 1;
  context.minimumScaleFactor = 0.55;
  row.addSpacer();
  const elapsed = row.addText(formatElapsed(event));
  elapsed.font = Font.monospacedSystemFont(10);
  elapsed.textColor = palette().accent;
  elapsed.lineLimit = 1;
}

function buildMessageWidget(titleText, message, kind, settings = null) {
  const widget = new ListWidget();
  applyWidgetBase(widget);
  widget.url = settings ? `${settings.baseURL}/admin/` : URLScheme.forRunningScript();
  widget.refreshAfterDate = new Date(Date.now() + REFRESH_MINUTES * 60 * 1000);

  const color = kind === "setup" ? palette().accent : kind === "auth" ? palette().warning : palette().danger;
  const symbolName = kind === "setup" ? "gearshape.fill" : kind === "auth" ? "lock.trianglebadge.exclamationmark" : "wifi.exclamationmark";
  const symbol = namedSymbol(symbolName, "gearshape.fill");
  const icon = widget.addImage(symbol.image);
  icon.imageSize = new Size(23, 23);
  icon.tintColor = color;
  widget.addSpacer(12);

  const title = widget.addText(titleText);
  title.font = Font.boldSystemFont(17);
  title.textColor = palette().primary;
  title.lineLimit = 1;
  widget.addSpacer(6);
  const body = widget.addText(message);
  body.font = Font.systemFont(11);
  body.textColor = palette().secondary;
  body.lineLimit = 3;
  widget.addSpacer();
  const hint = widget.addText(settings ? "点按打开 Web Admin" : "点按后手动运行脚本完成配置");
  hint.font = Font.systemFont(9);
  hint.textColor = palette().muted;
  return widget;
}

function applyWidgetBase(widget) {
  widget.backgroundColor = palette().background;
  widget.setPadding(15, 15, 13, 15);
}

function namedSymbol(name, fallbackName) {
  return SFSymbol.named(name) || SFSymbol.named(fallbackName);
}

function statusAppearance(status, inFlight) {
  if (!status.running) return { label: "代理已停止", color: palette().muted };
  if (inFlight.length) return { label: `${inFlight.length} 个请求进行中`, color: palette().accent };
  return { label: "无请求进行中", color: palette().good };
}

function eventModel(event) {
  return cleanEventText(event?.effectiveModel || event?.clientModel || event?.upstreamModel) || "模型待定";
}

function eventEndpoint(event) {
  return cleanEventText(event?.endpointName || event?.endpointID) || "入口待定";
}

function eventClient(event) {
  const purpose = cleanEventText(event?.requestPurpose);
  const client = cleanEventText(event?.clientKind);
  return purpose || client || "客户端请求";
}

function cleanEventText(value) {
  if (value && typeof value === "object") {
    return cleanEventText(value.kind || value.type || value.name);
  }
  return String(value || "").replace(/[\r\n\t]+/g, " ").trim();
}

function formatElapsed(event) {
  // RuntimeEvent timestamp 使用 Apple epoch 秒数（2001-01-01 起）。
  const timestamp = Number(event?.timestamp);
  const appleNow = Date.now() / 1000 - 978307200;
  const seconds = Math.max(0, Math.floor(appleNow - (Number.isFinite(timestamp) ? timestamp : appleNow)));
  if (seconds < 60) return `${seconds}秒`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}分${seconds % 60}秒`;
  return `${Math.floor(seconds / 3600)}小时${Math.floor((seconds % 3600) / 60)}分`;
}

function formatUptime(seconds) {
  let remaining = Math.max(0, Math.floor(Number(seconds) || 0));
  const days = Math.floor(remaining / 86400);
  remaining %= 86400;
  const hours = Math.floor(remaining / 3600);
  const minutes = Math.floor((remaining % 3600) / 60);
  if (days > 0) return `${days}天${hours}小时`;
  if (hours > 0) return `${hours}小时${minutes}分`;
  return `${minutes}分钟`;
}

function formatClock(date) {
  const formatter = new DateFormatter();
  formatter.locale = "zh_CN";
  formatter.dateFormat = "HH:mm";
  return formatter.string(date);
}

function palette() {
  return {
    background: Color.dynamic(new Color("#F7F8FA"), new Color("#111318")),
    card: Color.dynamic(new Color("#EDEFF3"), new Color("#20242B")),
    primary: Color.dynamic(new Color("#171A21"), new Color("#F4F5F7")),
    secondary: Color.dynamic(new Color("#525967"), new Color("#B6BBC5")),
    muted: Color.dynamic(new Color("#7C8491"), new Color("#858D99")),
    accent: new Color("#3978F6"),
    good: new Color("#2AA775"),
    warning: new Color("#D89024"),
    danger: new Color("#D95454"),
  };
}
