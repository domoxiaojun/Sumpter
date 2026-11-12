//! Shared product catalog. Observation capabilities never grant permissions.
use crate::events::ClientKind;
use serde::Serialize;
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub ua_products: &'static [&'static str],
    pub native_session_headers: &'static [&'static str],
    pub native_roles: bool,
    pub native_tasks: bool,
}
macro_rules! catalog {
    ($(($kind:ident, $id:literal, $name:literal, [$($ua:literal),*], [$($session:literal),*], $roles:literal, $tasks:literal)),* $(,)?) => {
        pub const CLIENTS: &[ClientKind] = &[$(ClientKind::$kind),*];
        pub const CATALOG: &[ClientDescriptor] = &[$(ClientDescriptor { id:$id, name:$name,
            ua_products:&[$($ua),*], native_session_headers:&[$($session),*], native_roles:$roles, native_tasks:$tasks }),*];
    }
}
catalog! {
(Codex,"codex","Codex",["codex","codex_cli_rs","codex-cli","codex-tui","codex_tui","codex-exec","codex_exec","codex-vscode","codex_vscode","acp-extension-codex"],["session-id","session_id"],true,true),
(ClaudeCode,"claude_code","Claude Code",["claude-cli","claude-code","claude-user"],["x-claude-code-session-id"],true,false),
(GeminiCli,"gemini_cli","Gemini CLI",["geminicli"],[],false,false),
(GrokBuild,"grok_build","Grok Build",["grok-shell","grok-pager","grok-desktop"],["x-grok-session-id","x-grok-conv-id"],false,false),
(Pi,"pi","pi",["pi","pi-coding-agent"],["x-session-affinity"],false,false),
(Cursor,"cursor","Cursor",["cursor","cursor-agent","cursor-cli"],[],false,false),
(Cline,"cline","Cline",["cline","cline-cli"],[],false,false),
(RooCode,"roo_code","Roo Code",["roo-code","roo_cline","roocode"],[],false,false),
(Continue,"continue","Continue",["continue","continue-cli"],[],false,false),
(Aider,"aider","Aider",["aider"],[],false,false),
(OpenCode,"open_code","OpenCode",["opencode","opencode-cli"],["x-opencode-session"],false,false),
(Goose,"goose","Goose",["goose"],[],false,false),
(OpenWebUi,"open_web_ui","Open WebUI",["open-webui","openwebui"],[],false,false),
(Windsurf,"windsurf","Windsurf",["windsurf","codeium"],[],false,false),
(KiloCode,"kilo_code","Kilo Code",["kilo-code","kilocode"],[],false,false),
(LiteLlm,"lite_llm","LiteLLM",["litellm"],[],false,false),
(Ollama,"ollama","Ollama",["ollama"],[],false,false),
(LmStudio,"lm_studio","LM Studio",["lm-studio","lmstudio"],[],false,false),
(Chatbox,"chatbox","Chatbox",["chatbox"],[],false,false),
(DeepSeekHarness,"deepseek_harness","DeepSeek Harness",["deepseek-harness"],["x-deepseek-harness-session-id"],true,true),
(WorkBuddy,"workbuddy","WorkBuddy",["workbuddy"],["x-conversation-id"],false,false),
}
pub fn product(ua: &str) -> Option<String> {
    let token = ua.trim().split('/').next()?.split_whitespace().next()?;
    let token = token.to_ascii_lowercase();
    valid_product(&token).then_some(token)
}
pub fn valid_product(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
pub fn kind_for_product(product: &str) -> Option<ClientKind> {
    CATALOG
        .iter()
        .position(|d| d.ua_products.contains(&product))
        .map(|index| CLIENTS[index])
}

pub const VARIANTS: &[&str] = &[
    "cli",
    "tui",
    "desktop",
    "ide",
    "vscode",
    "web",
    "sdk",
    "backend",
    "exec",
    "work_desktop",
    "work_web",
    "work_mobile",
    "work_cca",
    "chatgpt_cca",
    "chatgpt_atlas",
    "chatgpt_desktop",
    "chatgpt_android_remote",
    "chatgpt_ios_remote",
    "app_server",
    "app_server_sdk",
    "mcp_server",
    "unknown",
];
