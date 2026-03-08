const crypto = require('node:crypto');

const { parseControlCommand, renderControlResponse } = require('./commands');
const { isAuthorized, parsePairCommand, unauthorizedHint, validatePairRequest } = require('./feishu/auth');
const { renderOutboundMessage } = require('./feishu/render');
const { buildSessionRoute, normalizeFeishuEvent } = require('./feishu/session');

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
      const response = await this.coreClient.controlSession({
        session_key: route.sessionKey,
        parent_session_key: route.parentSessionKey,
        ...controlCommand,
      });
      await this.replyMessage(inbound.messageId, renderControlResponse(response));
      return { handled: true, kind: 'control', sessionKey: route.sessionKey };
    }

    const turnId = `turn_${crypto.randomUUID()}`;
    this.turnContexts.set(turnId, {
      replyToMessageId: inbound.messageId,
      slotStates: new Map(),
      openId: inbound.openId,
      route,
      createdAt: Date.now(),
    });

    await this.coreClient.submitTurn({
      turn_id: turnId,
      session_key: route.sessionKey,
      parent_session_key: route.parentSessionKey,
      text: inbound.text,
    });

    return { accepted: true, turnId, sessionKey: route.sessionKey };
  }

  async handleCoreEvent(event) {
    const context = this.turnContexts.get(event.turn_id);
    if (!context) {
      return { ignored: true, reason: 'missing_turn_context' };
    }

    const rendered = renderOutboundMessage(event.message);
    const slotKey = event.slot;
    const slotState = context.slotStates.get(slotKey);

    if (rendered.transport === 'cardkit') {
      if (slotState && slotState.cardId) {
        const nextSequence = (slotState.sequence || 0) + 2;
        await this.feishuClient.updateCard(slotState.cardId, rendered.card, nextSequence);
        context.slotStates.set(slotKey, {
          ...slotState,
          sequence: nextSequence,
        });
        return { updated: true, messageId: slotState.messageId, cardId: slotState.cardId };
      }

      const sent = await this.feishuClient.replyCardKitToMessage(context.replyToMessageId, rendered.card);
      context.slotStates.set(slotKey, {
        messageId: sent.messageId,
        cardId: sent.cardId,
        sequence: 1,
      });
      return { sent: true, messageId: sent.messageId, cardId: sent.cardId };
    }

    const existingMessageId = slotState && slotState.messageId;
    if (existingMessageId && rendered.msg_type === 'interactive') {
      await this.feishuClient.updateMessage(existingMessageId, rendered);
      return { updated: true, messageId: existingMessageId };
    }

    const sentMessageId = await this.feishuClient.replyToMessage(context.replyToMessageId, rendered);
    if (sentMessageId) {
      context.slotStates.set(slotKey, {
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
}

module.exports = {
  GatewayService,
};
