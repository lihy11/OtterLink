const http = require('node:http');
const Lark = require('@larksuiteoapi/node-sdk');

const { loadConfig } = require('./config');
const { CoreClient } = require('./core-client');
const { FeishuClient } = require('./feishu/client');
const { GatewayService } = require('./service');
const { PairingStore } = require('./store/pairings');

async function main() {
  const config = loadConfig();
  if (!config.appId || !config.appSecret) {
    throw new Error('missing APP_ID or APP_SECRET');
  }

  const pairings = await PairingStore.load(config.pairStorePath);
  const feishuClient = new FeishuClient({
    appId: config.appId,
    appSecret: config.appSecret,
  });
  const coreClient = new CoreClient({
    baseUrl: config.coreBaseUrl,
    token: config.coreIngestToken,
  });
  const service = new GatewayService({ config, feishuClient, coreClient, pairings });

  const server = http.createServer(async (req, res) => {
    try {
      if (req.method === 'GET' && req.url === '/healthz') {
        return respondJson(res, 200, { ok: true });
      }

      const body = await readJson(req);
      if (req.method === 'POST' && req.url === '/internal/gateway/event') {
        if (!authorize(req, config.gatewayEventToken, 'x-gateway-event-token')) {
          return respondJson(res, 401, { error: 'unauthorized' });
        }
        const result = await service.handleCoreEvent(body);
        return respondJson(res, 200, result);
      }

      if (req.method === 'POST' && req.url === '/internal/notify') {
        if (!authorize(req, config.notifyToken, 'x-notify-token')) {
          return respondJson(res, 401, { error: 'unauthorized' });
        }
        const messageId = await service.handleNotify(body);
        return respondJson(res, 200, { ok: true, message_id: messageId || null });
      }

      if (req.method === 'POST' && req.url === '/internal/feishu/event') {
        if (!authorize(req, config.bridgeIngestToken, 'x-bridge-token')) {
          return respondJson(res, 401, { error: 'unauthorized' });
        }
        const result = await service.handleFeishuEvent(body);
        return respondJson(res, 200, result);
      }

      return respondJson(res, 404, { error: 'not_found' });
    } catch (error) {
      console.error('[gateway] request failed', error);
      return respondJson(res, 500, { error: error.message || String(error) });
    }
  });

  const [host, port] = config.bind.split(':');
  server.listen(Number(port), host, () => {
    console.log(`[gateway] listening on ${config.bind}`);
  });

  let wsClient = null;
  if (!config.disableWs) {
    const dispatcher = new Lark.EventDispatcher({}).register({
      'im.message.receive_v1': async (data) => {
        try {
          await service.handleFeishuEvent(data);
        } catch (error) {
          console.error('[gateway] feishu event failed', error);
        }
        return {};
      },
    });

    wsClient = new Lark.WSClient({
      appId: config.appId,
      appSecret: config.appSecret,
      loggerLevel: Lark.LoggerLevel.info,
    });
    wsClient.start({ eventDispatcher: dispatcher });
    console.log('[gateway] Feishu WS client started');
  }

  const shutdown = () => {
    if (wsClient) {
      wsClient.close();
    }
    server.close(() => process.exit(0));
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

function authorize(req, expected, headerName) {
  if (!expected) {
    return true;
  }
  return req.headers[headerName] === expected;
}

function respondJson(res, status, payload) {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(payload));
}

function readJson(req) {
  return new Promise((resolve, reject) => {
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
    });
    req.on('end', () => {
      if (!body) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(body));
      } catch (error) {
        reject(error);
      }
    });
    req.on('error', reject);
  });
}

main().catch((error) => {
  console.error('[gateway] fatal', error);
  process.exit(1);
});
