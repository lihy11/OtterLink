function parsePairCommand(text) {
  const parts = String(text || '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) {
    return null;
  }
  const command = parts[0].toLowerCase();
  if (!['配对', 'pair', '/pair'].includes(command)) {
    return null;
  }
  return parts[1] || null;
}

function isAuthorized(config, pairings, openId) {
  const allowListed = config.allowFromOpenIds.has(openId);
  const paired = pairings.isPaired(openId);

  switch (config.feishuAuthMode) {
    case 'off':
      return true;
    case 'pair':
      return paired;
    case 'allow_from':
      return allowListed;
    case 'pair_or_allow_from':
      return paired || allowListed;
    default:
      return false;
  }
}

function validatePairRequest(config, openId, providedToken) {
  if (config.allowFromOpenIds.has(openId)) {
    return { ok: true };
  }

  if (config.pairAuthToken) {
    return providedToken === config.pairAuthToken
      ? { ok: true }
      : { ok: false, message: '配对失败：口令错误。请使用“配对 <口令>”。' };
  }

  if (config.feishuAuthMode === 'allow_from') {
    return { ok: false, message: '当前只允许白名单用户，不开放自助配对。' };
  }

  return { ok: false, message: '当前服务未配置 PAIR_AUTH_TOKEN，无法自助配对。' };
}

function unauthorizedHint(mode) {
  switch (mode) {
    case 'pair':
      return '当前机器人需要先配对。请私聊发送“配对 <口令>”。';
    case 'allow_from':
      return '当前机器人只响应白名单用户。';
    case 'pair_or_allow_from':
      return '当前机器人只响应已配对或白名单用户。请发送“配对 <口令>”或联系管理员。';
    default:
      return '未授权。';
  }
}

module.exports = {
  isAuthorized,
  parsePairCommand,
  unauthorizedHint,
  validatePairRequest,
};
