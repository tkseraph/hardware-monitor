export type SourceState = 'ok' | 'warming_up' | 'not_implemented' | 'unverified'
  | 'unsupported' | 'not_applicable' | 'permission_required' | 'error' | 'stale';

const messages: Record<SourceState, [string, string]> = {
  ok: ['实时读数', 'Live reading'],
  warming_up: ['正在建立采样基线', 'Establishing a sampling baseline'],
  not_implemented: ['此平台的采集尚未实现', 'Collection is not implemented on this platform'],
  unverified: ['此数据来源尚未验证', 'This source has not been verified'],
  unsupported: ['当前设备或系统不支持此指标', 'This metric is unsupported by the device or system'],
  not_applicable: ['此指标不适用于当前设备', 'This metric does not apply to this device'],
  permission_required: ['读取此指标需要额外权限', 'Reading this metric requires additional permission'],
  error: ['本次读取失败', 'The latest read failed'],
  stale: ['读数已过期', 'The reading is stale'],
};

export function sourceMessage(state: SourceState, chinese: boolean): string {
  return (messages[state] ?? messages.unverified)[chinese ? 0 : 1];
}

export function freshnessThreshold(intervalMs: number): number {
  return Number.isFinite(intervalMs) && intervalMs > 0 ? Math.max(15, intervalMs / 1000 * 3) : 15;
}
