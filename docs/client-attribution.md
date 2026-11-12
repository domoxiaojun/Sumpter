# 客户端身份与请求归因

服务端在入站时生成版本 2 的客户端观测，保留产品、宿主、引擎、入口、角色、请求用途与证据。规则只影响观测，不改变鉴权、路由或协议正文。历史记录不回写；未记录与已识别但证据不足的 unknown 分开显示。

## 免脚本接入

在安全页“客户端识别”中，为独立入站 Token 绑定客户端标识与可选入口。仅鉴权成功匹配到的启用 Token 使用绑定；多个客户端共用 Token 时留空。原生身份优先，绑定只填补身份不足，矛盾记录为冲突。Token 名称或凭据文本不参与推断。客户端只填写代理地址、Token 和模型，无需安装采集器。

项目仅由请求中可靠的工作区信息提供。Token 绑定不会生成项目、用户、会话或主代理身份。没有工作区时保留未知，不承诺替代脚本的本地项目采集能力。

## 统一目录与原生信号

[客户端目录](../crates/sumpter-core/src/client_registry.rs)统一维护 21 类产品及原生能力，使用 node scripts/maintenance/sync-client-registry.mjs 生成 Web 与 Swift 的目录副本。精确 UA 产品别名可扩展现有产品识别；不使用正则或任意子串。普通 SDK 和浏览器只保留有界产品标签，不推断背后的应用。

原生会话支持 Codex、Claude Code、Grok、Pi affinity，以及 x-opencode-session、x-conversation-id、x-deepseek-harness-session-id。优先使用原生会话，不从提示词、IP、token 数量或模型推测归属。

Claude Code 的 x-claude-code-agent-id 与 x-claude-code-parent-agent-id 分别记录代理及父代理 ID；缺失标记不证明主代理。Codex 的宿主应用和实际引擎可分别记录，例如 VS Code 承载 Codex；宿主使用 Codex 不等于宿主产品消失。父级、分叉和压缩不是同一概念。

Codex 原生标题、记忆、审查与压缩名称按产品映射到统一请求用途，原始 canonical 元数据继续保留。角色、用途、产品及入口相互独立。同级证据冲突保留 unknown；专用接口和原生任务不被弱指纹覆盖。

## 配置

设置位于 listener.clientIdentity，所有字段可省略，默认关闭：

```json
{
  "tokenBindings": {
    "work-client": { "client": "codex", "variant": "desktop" },
    "relay-token": { "client": "sumpter", "relaySource": "office" }
  },
  "uaAliases": { "custom-cli": "open_code" },
  "reportingAliases": { "my-bridge": "codex" },
  "relayEndpoints": { "remote-sumpter-endpoint-id": "home" }
}
```

Token 绑定键为已配置 Token 的稳定 ID；默认 Token 使用 legacy。客户端标识为 1–64 字节小写字母、数字、点、横线或下划线，未知产品保留标签。别名目标必须为目录中的稳定客户端 ID，每类映射最多 128 条。

统计映射在查询时应用到分组、筛选、分面和导出筛选，不修改原始事件。事件明细保留 clientKind / clientAttribution，额外的 reportedClientKind 标明统计归属。Token 绑定和中继补充的产品以 product 保存，原始 UA 产品以 rawProduct 保留。删除映射即可恢复独立统计；不会改变权限、预算或路由。展示时不把应用运行时或 SDK 标签当作已证实的 agent。

## 可信中继

接收方仅对绑定了 relaySource 的已认证专用 Token 接受 X-Sumpter-Forwarded-Client 和 X-Sumpter-Forwarded-Session，并要求 X-Sumpter-Relay-Source 与配置的 relaySource 精确一致。中继名称来自接收方配置，不信任请求自报的机器名称或 UA。会话补充以 relay:来源:会话ID 命名，且不覆盖原生会话；不同来源分开。

发送方只有 relayEndpoints 中明确列出的目标入口才附加转发标签（值为与接收方约定的来源标识），其他上游继续剥离全部 X-Sumpter-*。请仅列出另一台 Sumpter 的入口；不要给普通供应商开启。标签长度有界，不携带正文、凭据或绝对路径。持有专用中继 Token 的发送方可以声明客户端身份，故不可与普通客户端共用。

本轮不增加提示词采集、正文摘要或本地会话扫描：当前归因使用显式标识，无需文本指纹来猜测所有权。

## 归因脚本退役

仓库不再提供 Shell 包装器、Pi 扩展、Gemini 包装器、安装器、脚本下载路由或双端安装管理界面。历史 X-Sumpter-* 声明与已有事件保持可读兼容，出站仍剥离私有头。

升级不会自动编辑其他电脑的 Shell 配置、删除已安装私有文件或修改客户端设置。已安装过旧采集器的用户需要在客户端主机移除对应 Shell 启动注册及显式加载的 Pi 扩展，并重启旧客户端进程；先核对备份和现有连接设置，不要删除整个配置目录。Gemini 改用自身的 GOOGLE_GEMINI_BASE_URL 与 GEMINI_API_KEY。

## 验证边界

共享规则和双 adapter 合同覆盖身份兜底、原生标记、冲突、隔离及出站剥离。源码检查、行为测试、打包和真实客户端流量分别验收；合成样例不能证明所有产品版本都发送同样标记。
