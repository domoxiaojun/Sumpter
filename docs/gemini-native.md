# Gemini Developer API 原生接入

入口协议选择 `Gemini Developer API`，API 地址填写 `https://generativelanguage.googleapis.com`，API Key 填 Google AI Studio / Gemini Developer API 的有效密钥。出站使用 `x-goog-api-key`；如果配置的是 OAuth access token，API Key 字段使用 `Bearer <access token>`。Sumpter 不负责 OAuth 登录或令牌自动刷新。

客户端连接 Sumpter 时，`x-goog-api-key` 或 `Authorization: Bearer` 填的是 Sumpter 入站 Token。Sumpter 剥离它，再注入所选入口的 Google 凭据。入站不接受 query 中的 `key`，避免把入站凭据放入 URL；官方文档中的 query key 示例接入本代理时应改为请求头。

## 原生能力范围

原生请求在 Gemini 入口间使用原始协议。字段是否被上游模型接受，由 Google 的模型能力、API 版本和账户权限决定。跨协议转换只支持现有桥能准确表达的字段，不能把本表理解为所有字段都能转换到 Anthropic / OpenAI。

| 能力 | Sumpter 的处理 |
| --- | --- |
| 模型发现 | `GET /v1beta/models?pageSize=1000`，读取全部 `nextPageToken` 分页；内部目录将 `models/<id>` 规范化为 `<id>`；不把发现的模型自动加入授权模型组 |
| 模型列表和详情原生调用 | `/v1beta/models` 与 `/v1beta/models/<id>` 原生响应；使用下述项目资源入口选择规则 |
| 文本与多模态生成 | `generateContent` 保留原生正文与响应，包括图片、音频、视频、PDF、`inlineData`、`fileData`、生成模态与模型专用选项 |
| 流式生成 | `streamGenerateContent?alt=sse` 保留 SSE；省略 `alt=sse` 时保留 Google 默认 JSON 数组响应，仅使用 Gemini 原生入口，不做数组流的跨协议转换 |
| 思考和工具 | 原生保留 `thinkingConfig`、`thoughtSignature`、函数调用 ID / 参数 / 结果、并行工具调用、多模态工具结果以及 Google Search / URL Context / Code Execution 等原生工具字段 |
| 结构化输出和控制项 | 原生保留 JSON Schema、`generationConfig`、`safetySettings`、`systemInstruction`、`cachedContent` 和其他 Google 字段 |
| 计数与嵌入 | `countTokens`、`embedContent`、`batchEmbedContents`；显式模型映射同时改写辅助请求中已有的模型字段，用户文本与工具参数不参与替换 |
| 批处理与预测 | `batchGenerateContent`、`asyncBatchEmbedContent`、`predict`、`predictLongRunning`；批任务和 operation 查询/取消请求保留原生协议 |
| 文件与缓存资源 | `/v1beta/files`、`/upload/v1beta/files`、`/download/v1beta/files`、`/v1beta/cachedContents` 及其子路径；保留请求方法、上传控制头、二进制内容和更新参数 |
| 其他项目资源 | `batches`、`operations`、`fileSearchStores`、`corpora`、`generatedFiles`、`auth_tokens` 原生转发 |
| Interactions API | 创建、查询、删除和取消请求保留 `agent`、`previous_interaction_id`、`store`、`background` 等状态字段；原生 SSE 按 Interaction 状态观察终止与用量 |
| Live WebSocket | `/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent` 及 `v1alpha` / `v1` 版本；使用 Gemini 凭据，原样转发 setup、工具、音视频及控制帧 |
| 用量与终止 | 观察原生 prompt、工具输入、候选输出、思考与缓存命中；思考计入输出总量并单列，缓存属于输入子集；保留 finishReason、拦截和错误，不把 HTTP 200 自动当成完整回答 |

模型目录获取有总超时、总响应大小、模型数量和分页次数上限。后续页失败、重复页 Token 或超限会使本次刷新失败，不把部分目录当成完整目录。

`/v1alpha` 和 `/v1beta` 原生资源路径独立于现有 OpenAI 接口。`/v1/models` 和 `/v1/files` 继续使用既有 OpenAI 路由语义。要使用 Google 的 stable v1 资源接口，可以将 Gemini 入口 API 地址设为 `https://generativelanguage.googleapis.com/v1`，客户端仍走 `/v1beta/...`，Sumpter 将使用入口指定的版本。`/v1/interactions` 等没有 OpenAI 冲突的 Google 资源也可直接调用。

## 项目资源与多入口

文件、缓存、批任务、Interaction ID 和 Live 会话属于 Google 项目及凭据面，不能按普通聊天的失败转移逻辑随机切换项目。

- 资源请求只选择显式固定为 `gemini` 的入口；`auto` 入口不作为项目资源的默认归属。
- 只有一个符合条件的 Gemini 入口时可直接调用。有多个入口时，客户端请求增加 `x-sumpter-endpoint-id: <入口 ID>`；该私有头会在出站前剥离。后续资源调用应始终使用同一个入口 ID。
- 带模型的缓存创建和 Interactions 请求仍经过模型映射/模型组路由；模型字段遵循显式映射。没有模型字段的资源调用使用项目资源路由。
- 文件接口需入口或映射显式声明 `capabilities: ["files"]`；Live 需显式声明 `capabilities: ["live"]`。
- 资源写操作不会在失败后自动重放；项目资源路由只保留所选入口，避免把创建、删除或上传重试到其他项目。

分段上传的 `x-goog-upload-*` 控制头和正文会保留。Google 返回的同源 `x-goog-upload-url` 会改为通过 Sumpter 继续上传，并保留不透明的上传会话参数。Sumpter 以入站 `Host` 构造回传地址；HTTPS 反向代理应配置 `trustedProxyCIDRs` 并发送 `X-Forwarded-Proto: https`。代理若另加外部 URL 路径前缀，需要在反向代理处相应处理回传上传 URL。

单次 HTTP 请求正文沿用 Sumpter 的 128 MiB 上限，大文件应使用不超过该上限的续传分块。响应正文继续转发；统计观察器沿用有界观察策略，超大响应的用量和终止信息可能无法完整观察。

Live 的 `setup.model` 保留 Google 原始模型名，不使用聊天别名改写。代理入站仍需要上述请求头凭据；Google 浏览器临时凭据不自动成为 Sumpter 入站 Token。WebSocket 的连接结果、帧转发与实际模型会话成功分开判断，连接统计不代表已具备逐轮 Live Token 结算。

Vertex AI 的 project/location 路径、服务账户登录与令牌刷新属于独立的接口面，本次 Gemini Developer API 适配不将它们冒充为相同协议。已有任意路径透传继续按原有约束工作。

## 官方依据与验证

本实现对照以下 Google 官方资料：

- [Models：列表、模型资源名与分页](https://aistudio.google.com/api-reference/models/list-models)
- [API Key 与鉴权](https://aistudio.google.com/docs/api-key)
- [生成内容、原生响应与用量字段](https://aistudio.google.com/api-reference/generate-content)
- [嵌入和批量嵌入](https://aistudio.google.com/api-reference/embeddings)
- [Files API](https://aistudio.google.com/api-reference/files)
- [缓存资源](https://aistudio.google.com/api-reference/caching)
- [Batch API](https://aistudio.google.com/docs/batch-api)
- [Interactions API](https://aistudio.google.com/docs/interactions-overview)
- [Interactions SSE 事件与用量](https://aistudio.google.com/docs/streaming)
- [Live WebSocket](https://aistudio.google.com/api-reference/live)

共享合同测试覆盖 Linux / macOS adapter 的原生 HTTP 请求与 Live 握手，目录测试覆盖分页、鉴权、版本前缀、失败和模型名规范化；Swift 目录维护对应回归。测试夹具使用合成凭据和本地模拟上游。源码实现、测试通过、真实 Google API 验收、客户端验收和正式部署是分别核对的事项，实际执行记录见本轮任务交付。
