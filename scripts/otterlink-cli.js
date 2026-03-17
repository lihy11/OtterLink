#!/usr/bin/env node

const fs = require('node:fs');
const fsp = require('node:fs/promises');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const readline = require('node:readline/promises');
const { stdin, stdout } = require('node:process');
const crypto = require('node:crypto');

const ACP_PACKAGES = {
  claude_code: {
    label: 'claude_code',
    globalCommand: 'claude-code-acp',
    installArgs: ['install', '-g', '@zed-industries/claude-code-acp@0.16.2'],
    verifyArgs: ['-lc', 'npx -y @zed-industries/claude-code-acp@0.16.2 --help >/dev/null 2>&1'],
  },
  codex: {
    label: 'codex',
    globalCommand: 'codex-acp',
    installArgs: [
      'install',
      '-g',
      '@zed-industries/codex-acp@0.9.2',
      '@zed-industries/codex-acp-linux-x64@0.9.2',
    ],
    verifyArgs: [
      '-lc',
      'npx -y -p @zed-industries/codex-acp@0.9.2 -p @zed-industries/codex-acp-linux-x64@0.9.2 codex-acp --help >/dev/null 2>&1',
    ],
  },
};

async function main() {
  const rootDir = resolveRootDir();
  const args = process.argv.slice(2);
  const command = args[0] || 'help';
  const envFile = resolveEnvFile(rootDir, getOptionValue(args, '--env-file'));

  switch (command) {
    case 'help':
    case '--help':
    case '-h':
      printHelp(rootDir, envFile);
      return;
    case 'configure':
      await configureCommand({ rootDir, envFile, nonInteractive: args.includes('--non-interactive') });
      return;
    case 'install-acp':
      await installAcpCommand({
        agent: args[1] || 'all',
        ifMissing: args.includes('--if-missing'),
      });
      return;
    case 'doctor':
      await doctorCommand({ rootDir, envFile });
      return;
    case 'start':
      runRepoScript(rootDir, 'scripts/start-longconn.sh', envFile);
      return;
    case 'stop':
      runRepoScript(rootDir, 'scripts/stop-longconn.sh', envFile);
      return;
    case 'restart':
      runRepoScript(rootDir, 'scripts/stop-longconn.sh', envFile);
      runRepoScript(rootDir, 'scripts/start-longconn.sh', envFile);
      return;
    case 'status':
      await statusCommand({ rootDir, envFile });
      return;
    default:
      throw new Error(`unsupported command: ${command}`);
  }
}

function resolveRootDir() {
  return process.env.REMOTEAGENT_ROOT || path.resolve(__dirname, '..');
}

function resolveEnvFile(rootDir, explicitPath) {
  if (explicitPath) {
    return path.resolve(explicitPath.replace(/^~(?=\/|$)/, os.homedir()));
  }
  return process.env.REMOTEAGENT_ENV_FILE || path.join(rootDir, '.run', 'feishu.env');
}

function getOptionValue(args, name) {
  const index = args.indexOf(name);
  if (index === -1) {
    return null;
  }
  return args[index + 1] || null;
}

function printHelp(rootDir, envFile) {
  console.log(`OtterLink CLI

Project root: ${rootDir}
Env file: ${envFile}

Usage:
  otterlink configure [--env-file PATH]
  otterlink install-acp [claude_code|codex|all] [--if-missing]
  otterlink doctor [--env-file PATH]
  otterlink start [--env-file PATH]
  otterlink stop [--env-file PATH]
  otterlink restart [--env-file PATH]
  otterlink status [--env-file PATH]
`);
}

async function configureCommand({ rootDir, envFile, nonInteractive }) {
  if (nonInteractive) {
    throw new Error('configure currently requires an interactive terminal');
  }

  const current = loadEnvFile(envFile);
  const defaults = buildConfigDefaults(rootDir, current);
  const rl = readline.createInterface({ input: stdin, output: stdout });
  try {
    console.log(`Configuring OtterLink with env file: ${envFile}\n`);
    const next = { ...defaults };

    next.APP_ID = await promptText(rl, 'Feishu APP_ID', defaults.APP_ID);
    next.APP_SECRET = await promptText(rl, 'Feishu APP_SECRET', defaults.APP_SECRET, { secret: true });
    next.FEISHU_DISABLE_WS = (await promptYesNo(
      rl,
      'Enable Feishu WebSocket long connection?',
      defaults.FEISHU_DISABLE_WS !== '1',
    ))
      ? '0'
      : '1';
    next.FEISHU_AUTH_MODE = await promptChoice(
      rl,
      'Feishu auth mode',
      ['off', 'pair', 'allow_from', 'pair_or_allow_from'],
      defaults.FEISHU_AUTH_MODE,
    );
    next.ALLOW_FROM_OPEN_IDS = await promptText(
      rl,
      'Allow-list open_ids (comma separated, optional)',
      defaults.ALLOW_FROM_OPEN_IDS,
    );
    next.PAIR_AUTH_TOKEN = await promptText(
      rl,
      'Pair token (optional)',
      defaults.PAIR_AUTH_TOKEN,
      { allowEmpty: true },
    );
    next.BIND = await promptText(rl, 'Gateway bind address', defaults.BIND);
    next.CORE_BIND = await promptText(rl, 'Core bind address', defaults.CORE_BIND);
    next.CORE_BASE_URL = await promptText(rl, 'Core base URL', defaults.CORE_BASE_URL);
    next.GATEWAY_EVENT_URL = await promptText(
      rl,
      'Gateway event callback URL',
      defaults.GATEWAY_EVENT_URL,
    );
    next.RUNTIME_MODE = await promptChoice(
      rl,
      'Runtime mode',
      ['acp', 'acp_fallback', 'exec_json'],
      defaults.RUNTIME_MODE,
    );
    next.ACP_ADAPTER = await promptChoice(
      rl,
      'Default ACP adapter',
      ['claude_code', 'codex'],
      defaults.ACP_ADAPTER,
    );
    next.CODEX_WORKDIR = await promptText(rl, 'Default workspace', defaults.CODEX_WORKDIR);
    next.ACP_PROXY_URL = await promptText(
      rl,
      'Default proxy URL (optional)',
      defaults.ACP_PROXY_URL,
      { allowEmpty: true },
    );
    next.CLAUDE_CODE_DEFAULT_PROXY_MODE = await promptChoice(
      rl,
      'Default proxy for claude_code',
      ['off', 'on'],
      defaults.CLAUDE_CODE_DEFAULT_PROXY_MODE,
    );
    next.CODEX_DEFAULT_PROXY_MODE = await promptChoice(
      rl,
      'Default proxy for codex',
      ['on', 'off'],
      defaults.CODEX_DEFAULT_PROXY_MODE,
    );
    next.PAIR_STORE_PATH = await promptText(rl, 'Pair store path', defaults.PAIR_STORE_PATH);
    next.STATE_DB_PATH = await promptText(rl, 'State DB path', defaults.STATE_DB_PATH);
    next.TODO_EVENT_LOG_PATH = await promptText(
      rl,
      'Todo event log path',
      defaults.TODO_EVENT_LOG_PATH,
    );
    next.CLAUDE_HOME_DIR = await promptText(rl, 'Claude home dir', defaults.CLAUDE_HOME_DIR);
    next.CODEX_HOME_DIR = await promptText(rl, 'Codex home dir', defaults.CODEX_HOME_DIR);

    next.CORE_INGEST_TOKEN = defaults.CORE_INGEST_TOKEN || randomToken();
    next.GATEWAY_EVENT_TOKEN = defaults.GATEWAY_EVENT_TOKEN || randomToken();
    next.BRIDGE_INGEST_TOKEN = defaults.BRIDGE_INGEST_TOKEN || randomToken();
    next.BRIDGE_NOTIFY_TOKEN = defaults.BRIDGE_NOTIFY_TOKEN || randomToken();
    next.CODEX_BIN = defaults.CODEX_BIN;
    next.CODEX_SKIP_GIT_REPO_CHECK = defaults.CODEX_SKIP_GIT_REPO_CHECK;
    next.RENDER_MIN_UPDATE_MS = defaults.RENDER_MIN_UPDATE_MS;
    next.ACP_AGENT_CMD = defaults.ACP_AGENT_CMD;

    ensureParentDir(next.PAIR_STORE_PATH);
    ensureParentDir(next.STATE_DB_PATH);
    ensureParentDir(next.TODO_EVENT_LOG_PATH);
    ensureDir(path.dirname(envFile));
    ensureDir(next.CODEX_WORKDIR);

    await fsp.writeFile(envFile, renderEnvFile(next), 'utf8');
    console.log(`\nSaved configuration to ${envFile}`);

    if (await promptYesNo(rl, 'Scan ACP runtimes now?', true)) {
      await doctorAcpOnly();
    }
    if (await promptYesNo(rl, 'Install missing ACP runtimes now?', true)) {
      await installAcpCommand({ agent: 'all', ifMissing: true });
    }
  } finally {
    rl.close();
  }
}

function buildConfigDefaults(rootDir, current) {
  const bind = current.BIND || '127.0.0.1:1127';
  const coreBind = current.CORE_BIND || '127.0.0.1:7211';
  return {
    APP_ID: current.APP_ID || '',
    APP_SECRET: current.APP_SECRET || '',
    BIND: bind,
    CORE_BIND: coreBind,
    CORE_BASE_URL: current.CORE_BASE_URL || `http://${coreBind}`,
    GATEWAY_EVENT_URL:
      current.GATEWAY_EVENT_URL || `http://${bind}/internal/gateway/event`,
    CORE_INGEST_TOKEN: current.CORE_INGEST_TOKEN || '',
    GATEWAY_EVENT_TOKEN: current.GATEWAY_EVENT_TOKEN || '',
    BRIDGE_INGEST_TOKEN: current.BRIDGE_INGEST_TOKEN || '',
    BRIDGE_NOTIFY_TOKEN: current.BRIDGE_NOTIFY_TOKEN || '',
    FEISHU_AUTH_MODE: current.FEISHU_AUTH_MODE || 'off',
    FEISHU_DISABLE_WS: current.FEISHU_DISABLE_WS || '0',
    PAIR_AUTH_TOKEN: current.PAIR_AUTH_TOKEN || '',
    ALLOW_FROM_OPEN_IDS: current.ALLOW_FROM_OPEN_IDS || '',
    PAIR_STORE_PATH: preferredConfigPath(
      current.PAIR_STORE_PATH,
      path.join(rootDir, '.run', 'pairings.json'),
    ),
    STATE_DB_PATH: preferredConfigPath(
      current.STATE_DB_PATH,
      path.join(rootDir, '.run', 'state.db'),
    ),
    TODO_EVENT_LOG_PATH: preferredConfigPath(
      current.TODO_EVENT_LOG_PATH,
      path.join(rootDir, '.run', 'todo-events.jsonl'),
    ),
    CLAUDE_HOME_DIR: preferredConfigPath(
      current.CLAUDE_HOME_DIR,
      path.join(os.homedir(), '.claude'),
    ),
    CODEX_HOME_DIR: preferredConfigPath(
      current.CODEX_HOME_DIR,
      path.join(os.homedir(), '.codex'),
    ),
    RUNTIME_MODE: current.RUNTIME_MODE || 'acp_fallback',
    ACP_ADAPTER: current.ACP_ADAPTER || 'claude_code',
    ACP_AGENT_CMD: current.ACP_AGENT_CMD || '',
    ACP_PROXY_URL: current.ACP_PROXY_URL || '',
    CLAUDE_CODE_DEFAULT_PROXY_MODE:
      current.CLAUDE_CODE_DEFAULT_PROXY_MODE || 'off',
    CODEX_DEFAULT_PROXY_MODE: current.CODEX_DEFAULT_PROXY_MODE || 'on',
    CODEX_BIN: current.CODEX_BIN || 'codex',
    CODEX_WORKDIR: preferredConfigPath(
      current.CODEX_WORKDIR,
      path.join(rootDir, 'workspace'),
    ),
    CODEX_SKIP_GIT_REPO_CHECK: current.CODEX_SKIP_GIT_REPO_CHECK || 'true',
    RENDER_MIN_UPDATE_MS: current.RENDER_MIN_UPDATE_MS || '700',
  };
}

function preferredConfigPath(currentValue, fallbackPath) {
  const fallback = expandHomePath(fallbackPath);
  const candidate = expandHomePath(currentValue);
  if (!candidate) {
    return fallback;
  }
  return canWriteTargetPath(candidate) ? candidate : fallback;
}

function expandHomePath(value) {
  if (!value) {
    return '';
  }
  return path.resolve(String(value).replace(/^~(?=\/|$)/, os.homedir()));
}

function canWriteTargetPath(targetPath) {
  const candidate = expandHomePath(targetPath);
  let probe = candidate;
  const { W_OK } = fs.constants;

  while (!fs.existsSync(probe)) {
    const parent = path.dirname(probe);
    if (parent === probe) {
      return false;
    }
    probe = parent;
  }

  try {
    fs.accessSync(probe, W_OK);
    return true;
  } catch {
    return false;
  }
}

async function doctorCommand({ rootDir, envFile }) {
  const env = buildConfigDefaults(rootDir, loadEnvFile(envFile));
  console.log(`project root: ${rootDir}`);
  console.log(`env file: ${envFile}`);
  console.log(`app id configured: ${env.APP_ID ? 'yes' : 'no'}`);
  console.log(`default adapter: ${env.ACP_ADAPTER}`);
  console.log(`default proxy url: ${env.ACP_PROXY_URL || '(empty)'}`);
  console.log(`claude_code default proxy: ${env.CLAUDE_CODE_DEFAULT_PROXY_MODE}`);
  console.log(`codex default proxy: ${env.CODEX_DEFAULT_PROXY_MODE}`);
  console.log('');

  await doctorAcpOnly();
  console.log('');
  await statusCommand({ rootDir, envFile });
}

async function doctorAcpOnly() {
  for (const agent of Object.keys(ACP_PACKAGES)) {
    const result = await detectAcp(agent);
    console.log(
      `${agent}: global=${result.globalCommand ? 'yes' : 'no'}, npx=${result.npxReady ? 'yes' : 'no'}, install_needed=${result.installNeeded ? 'yes' : 'no'}`,
    );
  }
}

async function installAcpCommand({ agent, ifMissing }) {
  const targets = normalizeAgentTargets(agent);
  ensureCommandExists('npm');
  for (const target of targets) {
    const detection = await detectAcp(target);
    if (ifMissing && !detection.installNeeded) {
      console.log(`${target}: already available, skipping install`);
      continue;
    }
    console.log(`${target}: installing ACP runtime with npm`);
    runChecked('npm', ACP_PACKAGES[target].installArgs);
    const verified = await detectAcp(target);
    if (verified.installNeeded) {
      throw new Error(`${target}: install finished but runtime is still unavailable`);
    }
    console.log(`${target}: install complete`);
  }
}

function normalizeAgentTargets(agent) {
  if (!agent || agent === 'all') {
    return ['claude_code', 'codex'];
  }
  if (!ACP_PACKAGES[agent]) {
    throw new Error(`unsupported agent target: ${agent}`);
  }
  return [agent];
}

async function detectAcp(agent) {
  const spec = ACP_PACKAGES[agent];
  const globalCommand = commandExists(spec.globalCommand);
  const npxReady = await canRun('bash', spec.verifyArgs);
  return {
    globalCommand,
    npxReady,
    installNeeded: !globalCommand && !npxReady,
  };
}

async function statusCommand({ rootDir, envFile }) {
  const env = buildConfigDefaults(rootDir, loadEnvFile(envFile));
  const runDir = path.join(rootDir, '.run');
  const gatewayPid = readPid(path.join(runDir, 'gateway.pid'));
  const rustPid = readPid(path.join(runDir, 'rust.pid'));

  console.log(`gateway pid: ${formatPidStatus(gatewayPid)}`);
  console.log(`core pid: ${formatPidStatus(rustPid)}`);

  const gatewayHealth = await fetchHealth(`http://${env.BIND}/healthz`);
  const coreHealth = await fetchHealth(`http://${env.CORE_BIND}/healthz`);
  console.log(`gateway health: ${gatewayHealth}`);
  console.log(`core health: ${coreHealth}`);
}

function formatPidStatus(pid) {
  if (!pid) {
    return 'not running';
  }
  return processExists(pid) ? `${pid} (running)` : `${pid} (stale pid file)`;
}

function readPid(filePath) {
  if (!fs.existsSync(filePath)) {
    return null;
  }
  const raw = fs.readFileSync(filePath, 'utf8').trim();
  return raw ? Number(raw) : null;
}

async function fetchHealth(url) {
  return new Promise((resolve) => {
    const req = http.get(url, { timeout: 1500 }, (res) => {
      res.resume();
      resolve(res.statusCode === 200 ? 'ok' : `http_${res.statusCode}`);
    });
    req.on('timeout', () => {
      req.destroy();
      resolve('timeout');
    });
    req.on('error', () => resolve('down'));
  });
}

function loadEnvFile(envFile) {
  if (!fs.existsSync(envFile)) {
    return {};
  }
  const raw = fs.readFileSync(envFile, 'utf8');
  const values = {};
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) {
      continue;
    }
    const index = trimmed.indexOf('=');
    if (index === -1) {
      continue;
    }
    const key = trimmed
      .slice(0, index)
      .trim()
      .replace(/^export\s+/, '');
    const value = trimmed.slice(index + 1).trim();
    values[key] = stripQuotes(value);
  }
  return values;
}

function renderEnvFile(env) {
  const lines = [
    '# Gateway',
    formatEnvLine('APP_ID', env.APP_ID),
    formatEnvLine('APP_SECRET', env.APP_SECRET),
    formatEnvLine('BIND', env.BIND),
    formatEnvLine('CORE_BASE_URL', env.CORE_BASE_URL),
    formatEnvLine('CORE_INGEST_TOKEN', env.CORE_INGEST_TOKEN),
    formatEnvLine('GATEWAY_EVENT_TOKEN', env.GATEWAY_EVENT_TOKEN),
    formatEnvLine('BRIDGE_INGEST_TOKEN', env.BRIDGE_INGEST_TOKEN),
    formatEnvLine('BRIDGE_NOTIFY_TOKEN', env.BRIDGE_NOTIFY_TOKEN),
    formatEnvLine('FEISHU_AUTH_MODE', env.FEISHU_AUTH_MODE),
    formatEnvLine('FEISHU_DISABLE_WS', env.FEISHU_DISABLE_WS),
    formatEnvLine('PAIR_AUTH_TOKEN', env.PAIR_AUTH_TOKEN),
    formatEnvLine('ALLOW_FROM_OPEN_IDS', env.ALLOW_FROM_OPEN_IDS),
    formatEnvLine('PAIR_STORE_PATH', env.PAIR_STORE_PATH),
    '',
    '# Rust core',
    formatEnvLine('CORE_BIND', env.CORE_BIND),
    formatEnvLine('GATEWAY_EVENT_URL', env.GATEWAY_EVENT_URL),
    formatEnvLine('STATE_DB_PATH', env.STATE_DB_PATH),
    formatEnvLine('TODO_EVENT_LOG_PATH', env.TODO_EVENT_LOG_PATH),
    formatEnvLine('CLAUDE_HOME_DIR', env.CLAUDE_HOME_DIR),
    formatEnvLine('CODEX_HOME_DIR', env.CODEX_HOME_DIR),
    formatEnvLine('RENDER_MIN_UPDATE_MS', env.RENDER_MIN_UPDATE_MS),
    '',
    '# Agent runtime',
    formatEnvLine('RUNTIME_MODE', env.RUNTIME_MODE),
    formatEnvLine('ACP_ADAPTER', env.ACP_ADAPTER),
    formatEnvLine('ACP_AGENT_CMD', env.ACP_AGENT_CMD),
    formatEnvLine('ACP_PROXY_URL', env.ACP_PROXY_URL),
    formatEnvLine('CLAUDE_CODE_DEFAULT_PROXY_MODE', env.CLAUDE_CODE_DEFAULT_PROXY_MODE),
    formatEnvLine('CODEX_DEFAULT_PROXY_MODE', env.CODEX_DEFAULT_PROXY_MODE),
    formatEnvLine('CODEX_BIN', env.CODEX_BIN),
    formatEnvLine('CODEX_WORKDIR', env.CODEX_WORKDIR),
    formatEnvLine('CODEX_SKIP_GIT_REPO_CHECK', env.CODEX_SKIP_GIT_REPO_CHECK),
    '',
  ];
  return `${lines.join('\n')}\n`;
}

function formatEnvLine(key, value) {
  return `${key}=${quoteEnvValue(value)}`;
}

function quoteEnvValue(value) {
  const stringValue = String(value ?? '');
  if (/^[A-Za-z0-9_./:@,-]*$/.test(stringValue)) {
    return stringValue;
  }
  return `'${stringValue.replace(/'/g, `'\\''`)}'`;
}

function stripQuotes(value) {
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    return value.slice(1, -1);
  }
  return value;
}

function ensureParentDir(filePath) {
  ensureDir(path.dirname(filePath));
}

function ensureDir(dirPath) {
  fs.mkdirSync(dirPath, { recursive: true });
}

function randomToken() {
  return crypto.randomBytes(24).toString('hex');
}

async function promptText(rl, label, currentValue, options = {}) {
  const suffix = currentValue ? ` [${options.secret ? maskValue(currentValue) : currentValue}]` : '';
  const answer = await rl.question(`${label}${suffix}: `);
  if (!answer.trim()) {
    return options.allowEmpty ? '' : currentValue;
  }
  return answer.trim();
}

async function promptChoice(rl, label, choices, currentValue) {
  const display = choices.join('/');
  while (true) {
    const answer = await rl.question(`${label} (${display}) [${currentValue}]: `);
    const normalized = (answer.trim() || currentValue).trim();
    if (choices.includes(normalized)) {
      return normalized;
    }
    console.log(`Unsupported value: ${normalized}`);
  }
}

async function promptYesNo(rl, label, currentValue) {
  const current = currentValue ? 'Y/n' : 'y/N';
  const answer = await rl.question(`${label} [${current}]: `);
  const normalized = answer.trim().toLowerCase();
  if (!normalized) {
    return currentValue;
  }
  if (['y', 'yes'].includes(normalized)) {
    return true;
  }
  if (['n', 'no'].includes(normalized)) {
    return false;
  }
  console.log('Please answer yes or no.');
  return promptYesNo(rl, label, currentValue);
}

function maskValue(value) {
  if (value.length <= 6) {
    return '*'.repeat(value.length);
  }
  return `${value.slice(0, 3)}***${value.slice(-3)}`;
}

function runRepoScript(rootDir, relativeScript, envFile) {
  const scriptPath = path.join(rootDir, relativeScript);
  runChecked(scriptPath, [], {
    cwd: rootDir,
    env: { ...process.env, REMOTEAGENT_ENV_FILE: envFile, REMOTEAGENT_ROOT: rootDir },
  });
}

function runChecked(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || process.cwd(),
    env: options.env || process.env,
    stdio: 'inherit',
  });
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

function commandExists(command) {
  const result = spawnSync('bash', ['-lc', `command -v ${shellEscape(command)} >/dev/null 2>&1`], {
    stdio: 'ignore',
  });
  return result.status === 0;
}

function ensureCommandExists(command) {
  if (!commandExists(command)) {
    throw new Error(`missing required command: ${command}`);
  }
}

async function canRun(command, args) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { stdio: 'ignore' });
    child.on('error', () => resolve(false));
    child.on('exit', (code) => resolve(code === 0));
  });
}

function shellEscape(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

main().catch((error) => {
  console.error(error.message || String(error));
  process.exit(1);
});
