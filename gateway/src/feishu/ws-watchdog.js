function shouldRestartWsClient(reconnectInfo, now = Date.now(), stallTimeoutMs = 180000) {
  if (!reconnectInfo || !Number.isFinite(reconnectInfo.nextConnectTime) || reconnectInfo.nextConnectTime <= 0) {
    return false;
  }
  if (reconnectInfo.nextConnectTime > now - stallTimeoutMs) {
    return false;
  }
  if (Number.isFinite(reconnectInfo.lastConnectTime) && reconnectInfo.lastConnectTime > reconnectInfo.nextConnectTime) {
    return false;
  }
  return true;
}

module.exports = {
  shouldRestartWsClient,
};
