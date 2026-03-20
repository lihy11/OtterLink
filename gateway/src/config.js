const path = require('node:path');

function parseList(value) {
  return new Set(
    String(value || '')
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean),
  );
}

function loadConfig(env = process.env) {
  return {
    bind: env.BIND || '127.0.0.1:1127',
    coreBaseUrl: env.CORE_BASE_URL || 'http://127.0.0.1:7211',
    coreIngestToken: env.CORE_INGEST_TOKEN || '',
    gatewayEventToken: env.GATEWAY_EVENT_TOKEN || env.BRIDGE_NOTIFY_TOKEN || '',
    bridgeIngestToken: env.BRIDGE_INGEST_TOKEN || '',
    notifyToken: env.BRIDGE_NOTIFY_TOKEN || '',
    appId: env.APP_ID || '',
    appSecret: env.APP_SECRET || '',
    feishuAuthMode: env.FEISHU_AUTH_MODE || 'off',
    pairAuthToken: env.PAIR_AUTH_TOKEN || '',
    allowFromOpenIds: parseList(env.ALLOW_FROM_OPEN_IDS),
    pairStorePath: env.PAIR_STORE_PATH || path.join(process.cwd(), '.run', 'pairings.json'),
    disableWs: matchesTrue(env.FEISHU_DISABLE_WS),
    feishuDedupTtlMs: parseNonNegativeInt(env.FEISHU_DEDUP_TTL_MS, 10 * 60 * 1000),
    feishuWsWatchdogIntervalMs: parsePositiveInt(env.FEISHU_WS_WATCHDOG_INTERVAL_MS, 15000),
    feishuWsStallTimeoutMs: parsePositiveInt(env.FEISHU_WS_STALL_TIMEOUT_MS, 120000),
    feishuWsIdleRestartMs: parseNonNegativeInt(env.FEISHU_WS_IDLE_RESTART_MS, 0),
  };
}

function matchesTrue(value) {
  return ['1', 'true', 'TRUE', 'yes', 'YES'].includes(String(value || ''));
}

function parsePositiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function parseNonNegativeInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}

module.exports = {
  loadConfig,
  parseList,
  matchesTrue,
};
