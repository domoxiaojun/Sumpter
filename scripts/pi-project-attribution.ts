/** Opt in only on Sumpter providers with headers: { "X-Sumpter-Client": "pi" }. */
import { execFile } from "node:child_process";
import { basename } from "node:path";
import { userInfo } from "node:os";
import { promisify } from "node:util";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const exec = promisify(execFile);
const attributionHeaders = [
  "x-sumpter-project", "x-sumpter-workspace", "x-sumpter-user",
  "x-sumpter-git-remote", "x-sumpter-session-id", "x-sumpter-attribution-encoding",
];

async function git(cwd: string, args: string[]): Promise<string | undefined> {
  try {
    const { stdout } = await exec("git", ["-C", cwd, ...args], {
      timeout: 1000, maxBuffer: 16384, encoding: "utf8",
    });
    return stdout.trim() || undefined;
  } catch {
    return undefined;
  }
}

export function sanitizeRemote(remote: string | undefined): string | undefined {
  if (!remote || /[\u0000-\u001f\u007f]/u.test(remote)) return undefined;
  try {
    const url = new URL(remote);
    if (!["https:", "http:", "ssh:", "git:"].includes(url.protocol)) return undefined;
    url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    return url.toString();
  } catch {
    // SCP-style Git remotes: discard user information and query/fragment.
    const match = /^(?:[^@\s]+@)?([a-z\d.-]+):([^?#\s]+)(?:[?#].*)?$/iu.exec(remote);
    return match ? `ssh://${match[1]}/${match[2]}` : undefined;
  }
}

export default function (pi: ExtensionAPI) {
  pi.on("before_provider_headers", async (event, ctx) => {
    const markers = Object.entries(event.headers)
      .filter(([name]) => name.toLowerCase() === "x-sumpter-client");
    if (!markers.length || markers.some(([, value]) => value?.trim().toLowerCase() !== "pi")) return;

    // Clear previous values including alternate casing; failures must not
    // leave another session's metadata on reused headers.
    for (const name of Object.keys(event.headers)) {
      if (attributionHeaders.includes(name.toLowerCase())) event.headers[name] = null;
    }
    const session = ctx.sessionManager.getSessionId();
    if (session && session.length <= 256 && /^[\x21-\x7e]+$/u.test(session)) {
      event.headers["x-sumpter-session-id"] = session;
    }
    const workspace = await git(ctx.cwd, ["rev-parse", "--show-toplevel"]) || ctx.cwd;
    const remote = sanitizeRemote(await git(workspace, ["config", "--get", "remote.origin.url"]));
    let user: string | undefined;
    try { user = userInfo().username; } catch { /* Optional observation. */ }
    const values: Record<string, string | undefined> = {
      "x-sumpter-project": basename(workspace),
      "x-sumpter-workspace": workspace,
      "x-sumpter-user": user,
      "x-sumpter-git-remote": remote,
    };
    event.headers["x-sumpter-attribution-encoding"] = "uri-v1";
    for (const [name, value] of Object.entries(values)) {
      if (value && Buffer.byteLength(value, "utf8") <= 4096 && !/[\u0000-\u001f\u007f]/u.test(value)) {
        event.headers[name] = encodeURIComponent(value);
      }
    }
  });
}
