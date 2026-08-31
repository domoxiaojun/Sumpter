/**
 * Sumpter Linux 进行中请求查看器（TSX/TypeScript 脚本）
 *
 * 运行环境：Node.js 18+ + `tsx`，不是 iOS Scriptable 的直接脚本格式。
 *
 * 单文件配置：直接修改下面 CONFIG 即可运行。不要把填好密码的文件提交到 Git。
 * 环境变量 KEKULV_ADMIN_URL、KEKULV_ADMIN_USERNAME、KEKULV_ADMIN_PASSWORD
 * 存在时优先于 CONFIG，适合正式部署或密码管理器注入。
 *
 * 示例：
 *   npx tsx platforms/linux/integrations/tsx/kekulv-inflight.tsx
 *   npx tsx platforms/linux/integrations/tsx/kekulv-inflight.tsx --json
 */

const APPLE_EPOCH_OFFSET_SECONDS = 978_307_200;
const SESSION_COOKIE_NAME = "kekulv_admin_session";
const API_PREFIX = "/admin/api";
const REQUEST_TIMEOUT_MS = 15_000;

const CONFIG = {
  // 改成你的 Admin 地址，例如 http://192.168.1.20:57879 或 https://admin.example.com。
  adminURL: "https://YOUR-ADMIN-DOMAIN",
  username: "kkl",
  // 建议留空并使用环境变量；若只在本机保存，可直接填入密码。
  password: "",
};

type JsonObject = Record<string, unknown>;

type RuntimeEvent = {
  id?: string;
  kind?: string;
  phase?: string;
  timestamp?: number;
  effectiveModel?: string;
  clientModel?: string;
  upstreamModel?: string;
  endpointName?: string;
  endpointID?: string;
  clientKind?: string | JsonObject;
  requestPurpose?: string | JsonObject;
};

type LinuxStatus = {
  running?: boolean;
  version?: string;
  uptimeSeconds?: number;
  endpoints?: number;
  providers?: number;
};

type ApiResponse<T> = {
  status: number;
  data: T | null;
  headers: Headers;
};

const adminURL = normalizeAdminURL(
  process.env.KEKULV_ADMIN_URL || CONFIG.adminURL,
);
const username = configuredValue(
  "KEKULV_ADMIN_USERNAME",
  CONFIG.username,
  "管理员用户名",
);
const password = configuredValue(
  "KEKULV_ADMIN_PASSWORD",
  CONFIG.password,
  "管理员密码",
);
const jsonOutput = process.argv.includes("--json");

async function main(): Promise<void> {
  const cookie = await login();
  const statusResponse = await requestJSON<LinuxStatus>("/status", { cookie });
  if (statusResponse.status !== 200 || !statusResponse.data) {
    throw new Error(`读取 /status 失败（HTTP ${statusResponse.status}）。`);
  }

  const eventResponse = await requestJSON<{ events?: RuntimeEvent[] }>(
    "/runtime/events?limit=200&kind=client",
    { cookie },
  );
  if (eventResponse.status !== 200 || !eventResponse.data) {
    throw new Error(`读取进行中请求失败（HTTP ${eventResponse.status}）。`);
  }

  const inFlight = (eventResponse.data.events ?? []).filter(
    (event) => event.kind === "client" && isInFlight(event),
  );

  if (jsonOutput) {
    console.log(JSON.stringify({ status: statusResponse.data, inFlight }, null, 2));
    return;
  }

  printHumanReadable(statusResponse.data, inFlight);
}

async function login(): Promise<string> {
  const response = await requestJSON<JsonObject>("/auth/login", {
    method: "POST",
    body: { username, password },
  });
  if (response.status === 401) {
    throw new Error("Linux Admin 登录失败：用户名或密码不正确。");
  }
  if (response.status !== 200 || !response.data) {
    throw new Error(`Linux Admin 登录失败（HTTP ${response.status}）。`);
  }

  const cookie = extractSessionCookie(response.headers);
  if (!cookie) {
    throw new Error("登录成功但没有收到会话 Cookie，请检查 HTTPS 反向代理的 Set-Cookie 转发。");
  }
  return cookie;
}

async function requestJSON<T>(
  path: string,
  options: { method?: string; body?: JsonObject; cookie?: string } = {},
): Promise<ApiResponse<T>> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (options.cookie) headers.Cookie = `${SESSION_COOKIE_NAME}=${options.cookie}`;
    if (options.body) {
      headers["Content-Type"] = "application/json";
    }

    const response = await fetch(`${adminURL}${API_PREFIX}${path}`, {
      method: options.method ?? "GET",
      headers,
      body: options.body ? JSON.stringify(options.body) : undefined,
      signal: controller.signal,
    });
    const text = await response.text();
    let data: T | null = null;
    if (text.trim()) {
      try {
        data = JSON.parse(text) as T;
      } catch {
        throw new Error(`Admin 返回了非 JSON 响应（HTTP ${response.status}）。`);
      }
    }
    return { status: response.status, data, headers: response.headers };
  } catch (error) {
    if (error instanceof Error && error.name === "AbortError") {
      throw new Error("连接 Linux Admin 超时。");
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

function extractSessionCookie(headers: Headers): string | null {
  const withSetCookie = headers as Headers & { getSetCookie?: () => string[] };
  const values = typeof withSetCookie.getSetCookie === "function"
    ? withSetCookie.getSetCookie()
    : [headers.get("set-cookie") ?? ""];
  for (const value of values) {
    const match = value.match(new RegExp(`${SESSION_COOKIE_NAME}=([^;]*)`, "i"));
    if (match?.[1]) return match[1];
  }
  return null;
}

function isInFlight(event: RuntimeEvent): boolean {
  return event.phase === "inFlight" || event.phase === "in_flight";
}

function printHumanReadable(status: LinuxStatus, events: RuntimeEvent[]): void {
  const state = status.running ? "运行中" : "代理已停止";
  const version = status.version ? `v${status.version}` : "版本未知";
  const uptime = formatUptime(status.uptimeSeconds);
  const endpointCount = status.endpoints ?? status.providers ?? 0;

  console.log(`Sumpter Linux · ${state} · ${version}`);
  console.log(`在线 ${uptime} · ${endpointCount} 个入口`);
  console.log(`进行中的客户端请求：${events.length}`);

  if (!events.length) {
    console.log("当前没有进行中的请求。");
    return;
  }

  for (const [index, event] of events.entries()) {
    console.log(`${index + 1}. ${eventModel(event)} · ${eventEndpoint(event)} · ${eventContext(event)} · ${formatElapsed(event)}`);
  }
}

function eventModel(event: RuntimeEvent): string {
  return cleanText(event.effectiveModel ?? event.clientModel ?? event.upstreamModel) || "模型待定";
}

function eventEndpoint(event: RuntimeEvent): string {
  return cleanText(event.endpointName ?? event.endpointID) || "入口待定";
}

function eventContext(event: RuntimeEvent): string {
  const purpose = cleanText(event.requestPurpose);
  const client = cleanText(event.clientKind);
  if (purpose === "standard") return "主请求";
  if (purpose === "websearch") return "WebSearch";
  if (purpose === "webfetch") return "WebFetch";
  return purpose || client || "客户端请求";
}

function cleanText(value: unknown): string {
  if (value && typeof value === "object") {
    const object = value as JsonObject;
    return cleanText(object.kind ?? object.type ?? object.name);
  }
  return String(value ?? "").replace(/[\r\n\t]+/g, " ").trim();
}

function formatElapsed(event: RuntimeEvent): string {
  const timestamp = Number(event.timestamp);
  const now = Date.now() / 1000 - APPLE_EPOCH_OFFSET_SECONDS;
  const seconds = Math.max(0, Math.floor(now - (Number.isFinite(timestamp) ? timestamp : now)));
  if (seconds < 60) return `${seconds}秒`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}分${seconds % 60}秒`;
  return `${Math.floor(seconds / 3600)}小时${Math.floor((seconds % 3600) / 60)}分`;
}

function formatUptime(value: number | undefined): string {
  let seconds = Math.max(0, Math.floor(Number(value) || 0));
  const days = Math.floor(seconds / 86400);
  seconds %= 86400;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days) return `${days}天${hours}小时`;
  if (hours) return `${hours}小时${minutes}分`;
  return `${minutes}分钟`;
}

function configuredValue(name: string, fallback: string, label: string): string {
  const value = process.env[name] ?? fallback;
  if (!value) {
    throw new Error(`请在此文件顶部 CONFIG 填写${label}，或设置环境变量 ${name}。`);
  }
  return value;
}

function normalizeAdminURL(value: string | undefined): string {
  const input = String(value ?? "").trim().replace(/\/+$/, "");
  if (!/^https?:\/\/[^/?#]+$/i.test(input)) {
    throw new Error("KEKULV_ADMIN_URL 必须是 http:// 或 https:// 地址，例如 http://192.168.1.20:57879。");
  }
  return input;
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`错误：${message}`);
  process.exitCode = 1;
});
