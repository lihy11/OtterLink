class FeishuClient {
  constructor({ appId, appSecret, fetchImpl = fetch, baseUrl = 'https://open.feishu.cn' }) {
    this.appId = appId;
    this.appSecret = appSecret;
    this.fetchImpl = fetchImpl;
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.cachedToken = null;
  }

  async getTenantAccessToken() {
    if (this.cachedToken && this.cachedToken.expiresAt > Date.now() + 30_000) {
      return this.cachedToken.value;
    }

    const response = await this.fetchImpl(`${this.baseUrl}/open-apis/auth/v3/tenant_access_token/internal`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ app_id: this.appId, app_secret: this.appSecret }),
    });
    const payload = await response.json();
    if (!response.ok || payload.code !== 0 || !payload.tenant_access_token) {
      throw new Error(`tenant token failed status=${response.status} code=${payload.code} msg=${payload.msg}`);
    }

    this.cachedToken = {
      value: payload.tenant_access_token,
      expiresAt: Date.now() + (payload.expire || 7200) * 1000,
    };
    return this.cachedToken.value;
  }

  async replyToMessage(messageId, rendered) {
    const payload = await this.request(
      `/open-apis/im/v1/messages/${messageId}/reply`,
      {
        method: 'POST',
        body: {
          msg_type: rendered.msg_type,
          content: JSON.stringify(rendered.content),
        },
      },
    );
    return payload.data && payload.data.message_id ? payload.data.message_id : null;
  }

  async reactToMessage(messageId, emojiType = 'OK') {
    await this.request(`/open-apis/im/v1/messages/${messageId}/reactions`, {
      method: 'POST',
      body: {
        reaction_type: {
          emoji_type: emojiType,
        },
      },
    });
    return messageId;
  }

  async replyCardKitToMessage(messageId, cardkitCard) {
    const cardId = await this.createCard(cardkitCard);
    const payload = await this.request(
      `/open-apis/im/v1/messages/${messageId}/reply`,
      {
        method: 'POST',
        body: {
          msg_type: 'interactive',
          content: JSON.stringify({
            type: 'card',
            data: { card_id: cardId },
          }),
        },
      },
    );
    return {
      messageId: payload.data && payload.data.message_id ? payload.data.message_id : null,
      cardId,
    };
  }

  async updateMessage(messageId, rendered) {
    await this.request(`/open-apis/im/v1/messages/${messageId}`, {
      method: 'PATCH',
      body: {
        content: JSON.stringify(rendered.content),
      },
    });
    return messageId;
  }

  async updateCard(cardId, cardkitCard, sequence = 1) {
    const markdown = cardkitCard?.body?.elements?.[0]?.content || '';
    await this.request(`/open-apis/cardkit/v1/cards/${cardId}/elements/content/content`, {
      method: 'PUT',
      body: {
        content: markdown,
        sequence,
        uuid: `s_${cardId}_${sequence}`,
      },
    });

    await this.request(`/open-apis/cardkit/v1/cards/${cardId}/settings`, {
      method: 'PATCH',
      body: {
        settings: JSON.stringify({
          config: {
            streaming_mode: Boolean(cardkitCard?.config?.streaming_mode),
            summary: {
              content: cardkitCard?.config?.summary?.content || '[Generating...]',
            },
          },
        }),
        sequence: sequence + 1,
        uuid: `c_${cardId}_${sequence + 1}`,
      },
    });
    return cardId;
  }

  async sendToOpenId(openId, rendered) {
    const payload = await this.request('/open-apis/im/v1/messages?receive_id_type=open_id', {
      method: 'POST',
      body: {
        receive_id: openId,
        msg_type: rendered.msg_type,
        content: JSON.stringify(rendered.content),
      },
    });
    return payload.data && payload.data.message_id ? payload.data.message_id : null;
  }

  async sendCardKitToOpenId(openId, cardkitCard) {
    const cardId = await this.createCard(cardkitCard);
    const payload = await this.request('/open-apis/im/v1/messages?receive_id_type=open_id', {
      method: 'POST',
      body: {
        receive_id: openId,
        msg_type: 'interactive',
        content: JSON.stringify({
          type: 'card',
          data: { card_id: cardId },
        }),
      },
    });
    return {
      messageId: payload.data && payload.data.message_id ? payload.data.message_id : null,
      cardId,
    };
  }

  async createCard(cardkitCard) {
    const payload = await this.request('/open-apis/cardkit/v1/cards', {
      method: 'POST',
      body: {
        type: 'card_json',
        data: JSON.stringify(cardkitCard),
      },
    });
    return payload.data && payload.data.card_id ? payload.data.card_id : null;
  }

  async request(path, { method, body }) {
    const token = await this.getTenantAccessToken();
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify(body),
    });
    const payload = await response.json();
    if (!response.ok || payload.code !== 0) {
      throw new Error(`feishu request failed status=${response.status} code=${payload.code} msg=${payload.msg}`);
    }
    return payload;
  }
}

module.exports = {
  FeishuClient,
};
