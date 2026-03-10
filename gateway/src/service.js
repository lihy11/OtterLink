const crypto = require('node:crypto');

const { parseControlCommand, renderControlResponse, renderHistoryOverview, renderRuntimeHelp } = require('./commands');
const { isAuthorized, parsePairCommand, unauthorizedHint, validatePairRequest } = require('./feishu/auth');
const { renderCardMarkdown, renderOutboundMessage } = require('./feishu/render');
const { buildSessionRoute, normalizeFeishuEvent } = require('./feishu/session');

const CARD_HEARTBEAT_INTERVAL_MS = 5 * 60 * 1000;

class GatewayService {
  constructor({ config, feishuClient, coreClient, pairings, logger = console }) {
    this.config = config;
    this.feishuClient = feishuClient;
    this.coreClient = coreClient;
    this.pairings = pairings;
    this.logger = logger;
    this.turnContexts = new Map();
  }

  async handleFeishuEvent(payload) {
    const inbound = normalizeFeishuEvent(payload);
    this.logger.log(
      '[gateway] inbound feishu event',
      JSON.stringify({
        message_id: inbound.messageId,
        chat_id: inbound.chatId,
        chat_type: inbound.chatType,
        open_id: inbound.openId,
        text: inbound.text,
        raw_content: payload?.message?.content || null,
      }),
    );
    if (!inbound.messageId || !inbound.chatId || !inbound.openId) {
      return { ignored: true, reason: 'missing_identity' };
    }

    const pairToken = parsePairCommand(inbound.text);
    if (inbound.chatType === 'p2p' && pairToken !== null) {
      return this.handlePairCommand(inbound, pairToken);
    }

    if (!isAuthorized(this.config, this.pairings, inbound.openId)) {
      if (inbound.chatType === 'p2p') {
        await this.replyText(inbound.messageId, unauthorizedHint(this.config.feishuAuthMode));
      }
      return { ignored: true, reason: 'unauthorized' };
    }

    const route = buildSessionRoute(inbound);
    const controlCommand = parseControlCommand(inbound.text);
    if (controlCommand) {
      if (controlCommand.local_action === 'runtime_help') {
        await this.replyMessage(inbound.messageId, renderRuntimeHelp());
        return { handled: true, kind: 'control_help', sessionKey: route.sessionKey };
      }
      if (controlCommand.local_action === 'runtime_invalid') {
        await this.replyText(inbound.messageId, `Runtime 命令错误：${controlCommand.message}`);
        return { handled: true, kind: 'control_invalid', sessionKey: route.sessionKey };
      }
      try {
        const response = await this.coreClient.controlSession({
          session_key: route.sessionKey,
          parent_session_key: route.parentSessionKey,
          ...controlCommand,
        });
        await this.replyMessage(inbound.messageId, renderControlResponse(response));
        if (response.history_overview && response.history_overview.turns && response.history_overview.turns.length > 0) {
          await this.replyMessage(inbound.messageId, renderHistoryOverview(response.history_overview));
        }
        return { handled: true, kind: 'control', sessionKey: route.sessionKey };
      } catch (error) {
        await this.replyText(inbound.messageId, `Runtime 控制失败：${extractCoreError(error)}`);
        return { handled: true, kind: 'control_error', sessionKey: route.sessionKey };
      }
    }

    const turnId = `turn_${crypto.randomUUID()}`;
    this.turnContexts.set(turnId, {
      replyToMessageId: inbound.messageId,
      slotStates: new Map(),
      openId: inbound.openId,
      route,
      createdAt: Date.now(),
      cardFallbackSlots: new Set(),
    });

    try {
      await this.coreClient.submitTurn({
        turn_id: turnId,
        session_key: route.sessionKey,
        parent_session_key: route.parentSessionKey,
        text: inbound.text,
      });
    } catch (error) {
      this.turnContexts.delete(turnId);
      await this.replyText(inbound.messageId, `无法开始本轮：${extractCoreError(error)}`);
      return { ignored: true, reason: 'turn_rejected', sessionKey: route.sessionKey };
    }

    return { accepted: true, turnId, sessionKey: route.sessionKey };
  }

  async handleCoreEvent(event) {
    const context = this.turnContexts.get(event.turn_id);
    if (!context) {
      return { ignored: true, reason: 'missing_turn_context' };
    }

    const slotKey = event.slot;
    const rendered = shouldUsePlainMessageTransport(context, slotKey, event.message)
      ? renderFallbackMessage(event.message)
      : renderOutboundMessage(event.message);
    const slotState = context.slotStates.get(slotKey);

    if (rendered.transport === 'cardkit') {
      try {
        if (slotState && slotState.cardId) {
          const nextSequence = (slotState.sequence || 0) + 2;
          await this.feishuClient.updateCard(slotState.cardId, rendered.card, nextSequence);
          this.updateSlotState(context, slotKey, {
            ...slotState,
            sequence: nextSequence,
            lastCard: rendered.card,
          });
          this.scheduleCardHeartbeat(event.turn_id, slotKey);
          return { updated: true, messageId: slotState.messageId, cardId: slotState.cardId };
        }

        const sent = await this.feishuClient.replyCardKitToMessage(context.replyToMessageId, rendered.card);
        this.updateSlotState(context, slotKey, {
          messageId: sent.messageId,
          cardId: sent.cardId,
          sequence: 1,
          lastCard: rendered.card,
          heartbeatCount: 0,
        });
        this.scheduleCardHeartbeat(event.turn_id, slotKey);
        return { sent: true, messageId: sent.messageId, cardId: sent.cardId };
      } catch (error) {
        this.logger.error?.('[gateway] card delivery failed, falling back to message transport', error);
        return this.fallbackSlotToMessages(
          context,
          slotKey,
          event.message,
          '卡片更新失败，已切换为普通消息继续回传。',
        );
      }
    }

    this.clearHeartbeat(context, slotKey);
    const existingMessageId = slotKey === 'progress' ? null : slotState && slotState.messageId;
    if (existingMessageId && rendered.msg_type === 'interactive') {
      await this.feishuClient.updateMessage(existingMessageId, rendered);
      return { updated: true, messageId: existingMessageId };
    }

    const sentMessageId = await this.feishuClient.replyToMessage(context.replyToMessageId, rendered);
    if (sentMessageId) {
      this.updateSlotState(context, slotKey, {
        messageId: sentMessageId,
        cardId: null,
        sequence: 0,
      });
    }
    return { sent: true, messageId: sentMessageId };
  }

  async handleNotify(request) {
    const openId = request.open_id || this.pairings.firstPaired();
    if (!openId) {
      throw new Error('missing open_id and no paired user available');
    }
    const rendered = request.title
      ? renderOutboundMessage({ kind: 'post', title: request.title, text: request.text || '' })
      : renderOutboundMessage({ kind: 'text', text: request.text || '' });
    if (rendered.transport === 'cardkit') {
      const sent = await this.feishuClient.sendCardKitToOpenId(openId, rendered.card);
      return sent.messageId;
    }
    return this.feishuClient.sendToOpenId(openId, rendered);
  }

  async handlePairCommand(inbound, pairToken) {
    const verdict = validatePairRequest(this.config, inbound.openId, pairToken);
    if (!verdict.ok) {
      await this.replyText(inbound.messageId, verdict.message);
      return { ignored: true, reason: 'pair_rejected' };
    }

    await this.pairings.pair(inbound.openId);
    await this.replyText(inbound.messageId, `配对成功：${inbound.openId}`);
    return { paired: true, openId: inbound.openId };
  }

  async replyText(messageId, text) {
    return this.replyMessage(messageId, { kind: 'text', text });
  }

  async replyMessage(messageId, message) {
    const rendered = renderOutboundMessage(message);
    if (rendered.transport === 'cardkit') {
      const sent = await this.feishuClient.replyCardKitToMessage(messageId, rendered.card);
      return sent.messageId;
    }
    return this.feishuClient.replyToMessage(messageId, rendered);
  }

  updateSlotState(context, slotKey, nextState) {
    const prev = context.slotStates.get(slotKey);
    if (prev && prev.heartbeatTimer && prev.heartbeatTimer !== nextState.heartbeatTimer) {
      clearTimeout(prev.heartbeatTimer);
    }
    context.slotStates.set(slotKey, {
      heartbeatCount: 0,
      ...prev,
      ...nextState,
    });
  }

  scheduleCardHeartbeat(turnId, slotKey) {
    const context = this.turnContexts.get(turnId);
    if (!context || context.cardFallbackSlots.has(slotKey)) {
      return;
    }
    const slotState = context.slotStates.get(slotKey);
    if (!slotState || !slotState.cardId || !slotState.lastCard || !slotState.lastCard.config?.streaming_mode) {
      return;
    }
    if (slotState.heartbeatTimer) {
      clearTimeout(slotState.heartbeatTimer);
    }
    const heartbeatTimer = setTimeout(() => {
      this.sendCardHeartbeat(turnId, slotKey).catch((error) => {
        this.logger.error?.('[gateway] card heartbeat failed', error);
      });
    }, CARD_HEARTBEAT_INTERVAL_MS);
    heartbeatTimer.unref?.();
    this.updateSlotState(context, slotKey, { heartbeatTimer });
  }

  clearHeartbeat(context, slotKey) {
    const slotState = context.slotStates.get(slotKey);
    if (!slotState || !slotState.heartbeatTimer) {
      return;
    }
    clearTimeout(slotState.heartbeatTimer);
    context.slotStates.set(slotKey, {
      ...slotState,
      heartbeatTimer: null,
    });
  }

  async sendCardHeartbeat(turnId, slotKey) {
    const context = this.turnContexts.get(turnId);
    if (!context || context.cardFallbackSlots.has(slotKey)) {
      return;
    }
    const slotState = context.slotStates.get(slotKey);
    if (!slotState || !slotState.cardId || !slotState.lastCard) {
      return;
    }
    const heartbeatCount = (slotState.heartbeatCount || 0) + 1;
    const heartbeatCard = prependHeartbeat(slotState.lastCard, heartbeatCount);
    try {
      const nextSequence = (slotState.sequence || 0) + 2;
      await this.feishuClient.updateCard(slotState.cardId, heartbeatCard, nextSequence);
      this.updateSlotState(context, slotKey, {
        sequence: nextSequence,
        lastCard: heartbeatCard,
        heartbeatCount,
      });
      this.scheduleCardHeartbeat(turnId, slotKey);
    } catch (error) {
      await this.fallbackSlotToMessages(
        context,
        slotKey,
        { kind: 'card', card: heartbeatCard },
        `卡片流式更新已超时，已切换为普通消息继续回传。`,
      );
    }
  }

  async fallbackSlotToMessages(context, slotKey, message, reasonText) {
    context.cardFallbackSlots.add(slotKey);
    this.clearHeartbeat(context, slotKey);
    if (reasonText) {
      await this.feishuClient.replyToMessage(context.replyToMessageId, {
        msg_type: 'text',
        content: { text: reasonText },
      });
    }
    const rendered = renderFallbackMessage(message);
    const sentMessageId = await this.feishuClient.replyToMessage(context.replyToMessageId, rendered);
    this.updateSlotState(context, slotKey, {
      messageId: sentMessageId,
      cardId: null,
      sequence: 0,
      lastCard: null,
      heartbeatTimer: null,
    });
    return { sent: true, messageId: sentMessageId, fallback: true };
  }
}

function shouldUsePlainMessageTransport(context, slotKey, message) {
  if (slotKey === 'progress') {
    return true;
  }
  return context.cardFallbackSlots.has(slotKey) || message.kind !== 'card';
}

function renderFallbackMessage(message) {
  switch (message.kind) {
    case 'text':
      return { transport: 'message', msg_type: 'text', content: { text: message.text } };
    case 'post':
      return {
        transport: 'message',
        msg_type: 'text',
        content: { text: `${message.title}\n\n${message.text}`.trim() },
      };
    case 'card':
      return {
        transport: 'message',
        msg_type: 'text',
        content: { text: renderCardMarkdown(message.card) || '[empty card]' },
      };
    case 'raw':
      return {
        transport: 'message',
        msg_type: 'text',
        content: { text: JSON.stringify(message.content) },
      };
    default:
      return {
        transport: 'message',
        msg_type: 'text',
        content: { text: '[unsupported message]' },
      };
  }
}

function prependHeartbeat(cardkitCard, heartbeatCount) {
  const elements = Array.isArray(cardkitCard?.body?.elements) ? cardkitCard.body.elements : [];
  if (elements.length === 0) {
    return cardkitCard;
  }
  const first = elements[0];
  if (first.tag !== 'markdown') {
    return cardkitCard;
  }
  const nextCard = structuredClone(cardkitCard);
  nextCard.body.elements[0].content = [
    `**正在等待-${heartbeatCount}**`,
    '',
    first.content || '',
  ].join('\n');
  if (nextCard.config?.summary) {
    nextCard.config.summary.content = `正在等待-${heartbeatCount}`;
  }
  return nextCard;
}

function extractCoreError(error) {
  const text = error?.message || String(error);
  const marker = ' error=';
  const index = text.indexOf(marker);
  if (index >= 0) {
    return text.slice(index + marker.length).trim();
  }
  return text;
}

module.exports = {
  GatewayService,
};
