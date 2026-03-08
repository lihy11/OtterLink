class CoreClient {
  constructor({ baseUrl, token = '', fetchImpl = fetch }) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.token = token;
    this.fetchImpl = fetchImpl;
  }

  async submitTurn(turnRequest) {
    return this.post('/internal/core/turn', turnRequest, 'core submit');
  }

  async controlSession(controlRequest) {
    return this.post('/internal/core/control', controlRequest, 'core control');
  }

  async post(path, payload, label) {
    const headers = { 'content-type': 'application/json' };
    if (this.token) {
      headers['x-core-ingest-token'] = this.token;
    }

    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
    });
    const body = await response.json();
    if (!response.ok) {
      throw new Error(`${label} failed status=${response.status} error=${body.error || 'unknown'}`);
    }
    return body;
  }
}

module.exports = {
  CoreClient,
};
