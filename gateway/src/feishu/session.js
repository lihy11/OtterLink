function normalizeFeishuEvent(payload) {
  const event = payload && payload.event ? payload.event : payload;
  const sender = event && event.sender ? event.sender : {};
  const message = event && event.message ? event.message : {};
  const content = parseMessageContent(message.content);

  return {
    openId: sender.sender_id && sender.sender_id.open_id ? sender.sender_id.open_id : '',
    messageId: message.message_id || '',
    chatId: message.chat_id || '',
    chatType: message.chat_type || '',
    threadId: message.thread_id || null,
    messageType: message.message_type || '',
    text: content.text || '',
    raw: event,
  };
}

function parseMessageContent(value) {
  if (!value) {
    return {};
  }
  if (typeof value === 'string') {
    try {
      return JSON.parse(value);
    } catch (_error) {
      return {};
    }
  }
  return value;
}

function buildSessionRoute(message) {
  if (message.threadId) {
    return {
      sessionKey: `feishu:thread:${message.chatId}:${message.threadId}`,
      parentSessionKey: `feishu:chat:${message.chatId}`,
    };
  }
  if (message.chatType === 'p2p') {
    return {
      sessionKey: `feishu:p2p:${message.openId}`,
      parentSessionKey: null,
    };
  }
  return {
    sessionKey: `feishu:chat:${message.chatId}`,
    parentSessionKey: null,
  };
}

module.exports = {
  buildSessionRoute,
  normalizeFeishuEvent,
  parseMessageContent,
};
