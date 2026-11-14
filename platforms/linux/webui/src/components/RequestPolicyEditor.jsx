import { useContext, useRef, useState } from 'react';
import { ModalBusyContext } from '../context/ModalContext.js';
import { useAppControls } from '../context/AppContext.jsx';
import { DEFAULT_LIMITS, requestPolicy, validateRequestPolicy } from '../utils/requestPolicy.js';
import { Button } from './ui/button.jsx';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';

const SELECT_CLS = 'h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground shadow-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50';

export function RequestPolicyEditor() {
  const { config, configDoc, saveConfig, closeModal, addToast } = useAppControls();
  const [draft, setDraft] = useState(() => requestPolicy(config.requestPolicy));
  const [sourceDocument] = useState(() => configDoc);
  const [endpointID, setEndpointID] = useState('');
  const [model, setModel] = useState('*');
  const [protocol, setProtocol] = useState('all');
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const setModalBusy = useContext(ModalBusyContext);

  const change = (edit) => setDraft((prior) => {
    const next = structuredClone(prior);
    edit(next);
    return next;
  });

  const endpoint = draft.endpoints[endpointID] || { limits: DEFAULT_LIMITS, models: {} };
  const ensureEndpoint = (next) => (next.endpoints[endpointID] ||= { limits: { ...DEFAULT_LIMITS }, models: {} });
  const profiles = protocol === 'all' ? endpoint.models : endpoint.protocolModels?.[protocol] || {};
  const ensureProfiles = (next) => {
    const p = ensureEndpoint(next);
    if (protocol === 'all') return p.models;
    p.protocolModels ||= {};
    p.protocolModels[protocol] ||= {};
    return p.protocolModels[protocol];
  };

  const numberField = (label, value, onChange, min, max) => (
    <label className="flex flex-col gap-1.5 text-xs">
      <span className="font-medium text-muted-foreground">{label}</span>
      <Input
        type="number"
        min={min}
        max={max}
        step="1"
        className="h-9 font-mono text-xs tabular-nums"
        value={value}
        onChange={(event) => onChange(event.target.value === '' ? '' : Number(event.target.value))}
      />
    </label>
  );

  const limitsGrid = (value, update) => (
    <div className="grid gap-3 sm:grid-cols-2">
      {numberField('并发上限（0 = 不限）', value.maxConcurrent, (v) => update('maxConcurrent', v), 0, 10000)}
      {numberField('排队数量（0 = 不排队）', value.maxQueued, (v) => update('maxQueued', v), 0, 10000)}
      {numberField('排队时限（秒）', value.queueTimeoutSeconds, (v) => update('queueTimeoutSeconds', v), 1, 300)}
      {numberField('每分钟上游尝试（0 = 不限）', value.requestsPerMinute, (v) => update('requestsPerMinute', v), 0, 1000000)}
    </div>
  );

  const save = async (event) => {
    event.preventDefault();
    if (savingRef.current) return;
    try {
      validateRequestPolicy(draft);
      savingRef.current = true;
      setSaving(true);
      setModalBusy(true);
      await saveConfig({ ...structuredClone(sourceDocument.config), requestPolicy: draft }, {}, sourceDocument);
      closeModal();
      addToast('请求策略已保存', 'success');
    } catch (error) {
      addToast(error.message, 'error');
    } finally {
      savingRef.current = false;
      setSaving(false);
      setModalBusy(false);
    }
  };

  return (
    <form onSubmit={save} className="space-y-4">
      <fieldset disabled={saving} className="space-y-4">
        <p className="text-xs leading-relaxed text-muted-foreground">
          适用于所有客户端的 HTTP/SSE 请求。并发名额保持到上游响应结束；重试也计入每分钟额度。WebSocket 不按连接数等同为请求数。
        </p>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="space-y-0.5">
            <h3 className="text-sm font-semibold text-foreground">HTTP 入站保护</h3>
            <p className="text-[11px] text-muted-foreground">
              安全额度必须为正整数。入站总预算至少等于原始体与解压上限之和；读体期限不限制正常响应或 SSE 长流。
            </p>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            {numberField('HTTP 单请求体（MiB）', draft.ingress.maxBodyMib, (v) => change((next) => { next.ingress.maxBodyMib = v; }), 1, 1024)}
            {numberField('解压输出上限（MiB）', draft.ingress.maxDecodedBodyMib, (v) => change((next) => { next.ingress.maxDecodedBodyMib = v; }), 1, 1024)}
            {numberField('入站缓冲总预算（MiB）', draft.ingress.maxBufferedMib, (v) => change((next) => { next.ingress.maxBufferedMib = v; }), 1, 16384)}
            {numberField('同时读体、解压、解析', draft.ingress.maxPreprocessing, (v) => change((next) => { next.ingress.maxPreprocessing = v; }), 1, 64)}
            {numberField('同时解压', draft.ingress.maxDecompressions, (v) => change((next) => { next.ingress.maxDecompressions = v; }), 1, 64)}
            {numberField('读体空闲时限（秒）', draft.ingress.bodyIdleTimeoutSeconds, (v) => change((next) => { next.ingress.bodyIdleTimeoutSeconds = v; }), 1, 3600)}
            {numberField('读体总时限（秒）', draft.ingress.bodyReadTimeoutSeconds, (v) => change((next) => { next.ingress.bodyReadTimeoutSeconds = v; }), 1, 86400)}
          </div>
        </section>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="space-y-0.5">
            <h3 className="text-sm font-semibold text-foreground">WebSocket 保护</h3>
            <p className="text-[11px] text-muted-foreground">保存后新请求和连接生效，降低共享额度不强杀已有工作。</p>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            {numberField('单帧上限（MiB）', draft.websocket.maxFrameMib, (v) => change((next) => { next.websocket.maxFrameMib = v; }), 1, 1024)}
            {numberField('累计分片消息上限（MiB）', draft.websocket.maxMessageMib, (v) => change((next) => { next.websocket.maxMessageMib = v; }), 1, 1024)}
            {numberField('业务连接上限', draft.websocket.maxConnections, (v) => change((next) => { next.websocket.maxConnections = v; }), 1, 1024)}
            {numberField('首个业务消息期限（秒）', draft.websocket.firstMessageTimeoutSeconds, (v) => change((next) => { next.websocket.firstMessageTimeoutSeconds = v; }), 1, 3600)}
          </div>
        </section>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold text-foreground">全局请求限制</h3>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 px-2.5 text-xs"
              onClick={() => change((next) => {
                next.maxTotalAttempts = 6;
                next.global = { ...next.global, maxConcurrent: 8, maxQueued: 16, queueTimeoutSeconds: 15 };
              })}
            >
              填入交互预设
            </Button>
          </div>
          {limitsGrid(draft.global, (k, v) => change((next) => { next.global[k] = v; }))}
          <div className="grid gap-3 sm:grid-cols-2">
            {numberField('一次请求最多尝试（0 = 沿用原有重试限制）', draft.maxTotalAttempts, (v) => change((next) => { next.maxTotalAttempts = v; }), 0, 10000)}
            {numberField('代理响应等待上限（秒，0 = 不限）', draft.responseWaitTimeoutSeconds, (v) => change((next) => { next.responseWaitTimeoutSeconds = v; }), 0, 86400)}
          </div>
          <p className="text-[11px] text-muted-foreground">
            涵盖排队、所有入口尝试、退避与缓冲；超时返回 504。0 保持原策略；已开始交付的长回答不受影响。
          </p>
        </section>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="space-y-0.5">
            <h3 className="text-sm font-semibold text-foreground">连续无响应保护</h3>
            <p className="text-[11px] text-muted-foreground">
              同一会话、同一正文连续连接失败或首响应超时后停止向该入口重发；收到 HTTP 响应即清零。
            </p>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            {numberField('同一入口连续无响应上限（0 = 关闭）', draft.maxConsecutiveNoResponse, (v) => change((next) => { next.maxConsecutiveNoResponse = v; }), 0, 100)}
            {numberField('相同请求拦截时长（秒）', draft.noResponseCooldownSeconds, (v) => change((next) => { next.noResponseCooldownSeconds = v; }), 1, 3600)}
          </div>
        </section>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="space-y-0.5">
            <h3 className="text-sm font-semibold text-foreground">Responses 流保护</h3>
            <p className="text-[11px] text-muted-foreground">
              仅在流错误重试开启时生效；前奏模式提交输出后不再透明重放，达到额度恢复透传。
            </p>
          </div>
          <label className="flex flex-col gap-1.5 text-xs">
            <span className="font-medium text-muted-foreground">流式策略</span>
            <select
              className={SELECT_CLS}
              value={draft.streamProtection}
              onChange={(e) => change((next) => { next.streamProtection = e.target.value; })}
            >
              <option value="complete">完整缓冲：支持中途断流恢复</option>
              <option value="prelude">前奏保护：更早开始输出</option>
            </select>
          </label>
          <div className="grid gap-3 sm:grid-cols-2">
            {numberField('单流缓冲（MiB）', draft.streamBufferMib, (v) => change((next) => { next.streamBufferMib = v; }), 1, 256)}
            {numberField('所有流总缓冲（MiB）', draft.maxBufferedMib, (v) => change((next) => { next.maxBufferedMib = v; }), 1, 4096)}
            {numberField('单流响应块上限', draft.streamBufferChunks, (v) => change((next) => { next.streamBufferChunks = v; }), 16, 65536)}
            {numberField('缓冲时限（秒，0 = 不限）', draft.streamBufferSeconds, (v) => change((next) => { next.streamBufferSeconds = v; }), 0, 3600)}
          </div>
        </section>

        <section className="space-y-3 rounded-xl border bg-muted/15 p-3.5">
          <div className="space-y-0.5">
            <h3 className="text-sm font-semibold text-foreground">入口限制与兼容声明</h3>
            <p className="text-[11px] text-muted-foreground">
              明确不支持时跳过该入口，全部候选不支持则拒绝请求；不会静默删除工具或改写正文。
            </p>
          </div>
          <label className="flex flex-col gap-1.5 text-xs">
            <Label className="text-xs font-medium text-muted-foreground">入口</Label>
            <select
              className={SELECT_CLS}
              value={endpointID}
              onChange={(e) => setEndpointID(e.target.value)}
            >
              <option value="">选择入口</option>
              {config.endpoints.map((e) => (
                <option value={e.id} key={e.id}>{e.name || e.id}</option>
              ))}
            </select>
          </label>
          {endpointID && (
            <div className="space-y-3 pt-1">
              {limitsGrid(endpoint.limits, (k, v) => change((next) => { ensureEndpoint(next).limits[k] = v; }))}
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="flex flex-col gap-1.5 text-xs">
                  <span className="font-medium text-muted-foreground">上游模型（精确 ID 或 *）</span>
                  <Input
                    className="h-9 font-mono text-xs"
                    value={model}
                    onChange={(e) => setModel(e.target.value)}
                  />
                </label>
                <label className="flex flex-col gap-1.5 text-xs">
                  <span className="font-medium text-muted-foreground">兼容声明适用协议</span>
                  <select
                    className={SELECT_CLS}
                    value={protocol}
                    onChange={(e) => setProtocol(e.target.value)}
                  >
                    <option value="all">所有协议的默认声明</option>
                    <option value="anthropic">Anthropic Messages</option>
                    <option value="openai">Chat Completions</option>
                    <option value="openai-responses">Responses</option>
                    <option value="gemini">Gemini</option>
                  </select>
                </label>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                {[
                  ['strictTools', '严格工具'],
                  ['grammarTools', '语法工具'],
                  ['developerRole', 'Developer 角色'],
                  ['structuredOutput', '结构化输出'],
                ].map(([key, label]) => (
                  <label className="flex flex-col gap-1.5 text-xs" key={key}>
                    <span className="font-medium text-muted-foreground">{label}</span>
                    <select
                      className={SELECT_CLS}
                      disabled={!model.trim()}
                      value={profiles[model.trim()]?.[key] == null ? 'unknown' : String(profiles[model.trim()][key])}
                      onChange={(e) => change((next) => {
                        const p = ensureProfiles(next);
                        p[model.trim()] ||= {};
                        p[model.trim()][key] = e.target.value === 'unknown' ? null : e.target.value === 'true';
                      })}
                    >
                      <option value="unknown">未知 / 保持原样</option>
                      <option value="true">支持</option>
                      <option value="false">不支持</option>
                    </select>
                  </label>
                ))}
              </div>
            </div>
          )}
        </section>

        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="outline" onClick={closeModal}>
            取消
          </Button>
          <Button type="submit" disabled={saving}>
            {saving ? '保存中…' : '保存请求策略'}
          </Button>
        </div>
      </fieldset>
    </form>
  );
}
