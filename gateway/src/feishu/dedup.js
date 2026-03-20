const DEFAULT_TTL_MS = 10 * 60 * 1000;

class MessageDeduper {
  constructor({ ttlMs = DEFAULT_TTL_MS } = {}) {
    this.ttlMs = ttlMs;
    this.seen = new Map();
  }

  isDuplicate(messageId, now = Date.now()) {
    this.prune(now);
    if (!messageId) {
      return false;
    }
    if (this.seen.has(messageId)) {
      return true;
    }
    this.seen.set(messageId, now);
    return false;
  }

  prune(now = Date.now()) {
    if (this.ttlMs <= 0 || this.seen.size === 0) {
      return;
    }
    for (const [messageId, ts] of this.seen) {
      if (now - ts > this.ttlMs) {
        this.seen.delete(messageId);
      }
    }
  }
}

module.exports = {
  DEFAULT_TTL_MS,
  MessageDeduper,
};
