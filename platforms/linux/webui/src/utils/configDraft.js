import { clone } from './helpers.js';

// 草稿的版本必须与开始编辑时的文档绑定，不能在点击保存时换成新版本。
export function configDraftWrite(config, sourceDocument) {
  if (!sourceDocument?.generation || !sourceDocument?.config) {
    throw new Error('配置草稿缺少编辑基线，请重新打开编辑器');
  }
  return { generation: sourceDocument.generation, config: clone(config) };
}

export function requireDraftGeneration(sourceDocument, latestDocument) {
  if (sourceDocument && sourceDocument.generation !== latestDocument?.generation) {
    const error = new Error('配置已被其他操作修改；草稿已保留，请重新载入后合并修改');
    error.status = 409;
    error.code = 'generation_conflict';
    throw error;
  }
}
