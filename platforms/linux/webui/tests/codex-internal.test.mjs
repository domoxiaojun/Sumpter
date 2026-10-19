import assert from 'node:assert/strict';
import test from 'node:test';
import { codexAgentRoleLabel, codexMetadataSummary, formatWebSocketTrace, friendlyEventMessage } from '../src/utils/helpers.js';

test('internal tasks and request purposes remain distinct from spawned agents', () => {
  assert.equal(codexAgentRoleLabel({threadSource:'memory_consolidation',isSubagent:true}), '记忆任务');
  assert.equal(codexAgentRoleLabel({subagentHeader:'guardian',isSubagent:true}), 'Guardian 安全审查');
  assert.match(codexAgentRoleLabel({subagentKind:'thread_spawn'}), /子代理/);
  const summary = codexMetadataSummary({requestKind:'prewarm',historyInitialization:'cold_resume',analyticsEnabled:false,mcpAttribution:{status:'complete',sourceCount:2}});
  assert.match(summary,/连接预热/);
  assert.match(summary,/历史初始化 cold_resume/);
  assert.match(summary,/客户端分析 关闭/);
  assert.match(summary,/MCP 归因 complete · 来源 2/);
});

test('connection details expose logical counts, correlation and observation limits', () => {
  const summary = formatWebSocketTrace({stage:'relay',requests:{counts:{prewarm:1,turn:2},recent:[{sequence:3,kind:'turn',turnId:'t2',responseId:'r2',terminal:'response.interrupted'}],dropped:0,unobservedFrames:1,unassociatedEvents:2}});
  assert.match(summary,/连接预热 1/);
  assert.match(summary,/正式回合 2/);
  assert.match(summary,/turn=t2 · response=r2 · response.interrupted/);
  assert.match(summary,/未观测帧 1 · 未关联事件 2/);
  assert.equal(friendlyEventMessage({outcome:'cancelled',message:'websocket_unused_prewarm_connection_closed',streamTrace:{websocketTrace:{stage:'awaiting_first_message'}}}), '预热连接已关闭（未发送业务请求）');
});
