const fs = require('node:fs/promises');
const path = require('node:path');

class PairingStore {
  constructor(filePath, pairedOpenIds = new Set()) {
    this.filePath = filePath;
    this.pairedOpenIds = pairedOpenIds;
  }

  static async load(filePath) {
    try {
      const raw = await fs.readFile(filePath, 'utf8');
      return new PairingStore(filePath, parseStore(raw));
    } catch (error) {
      if (error.code === 'ENOENT') {
        return new PairingStore(filePath, new Set());
      }
      throw error;
    }
  }

  isPaired(openId) {
    return this.pairedOpenIds.has(openId);
  }

  async pair(openId) {
    this.pairedOpenIds.add(openId);
    await this.save();
  }

  firstPaired() {
    return this.pairedOpenIds.values().next().value || null;
  }

  snapshot() {
    return Array.from(this.pairedOpenIds).sort();
  }

  async save() {
    await fs.mkdir(path.dirname(this.filePath), { recursive: true });
    const body = JSON.stringify({ open_ids: this.snapshot() }, null, 2);
    await fs.writeFile(this.filePath, body);
  }
}

function parseStore(raw) {
  const parsed = JSON.parse(raw);
  if (Array.isArray(parsed)) {
    return new Set(parsed.filter(Boolean));
  }
  if (parsed && Array.isArray(parsed.open_ids)) {
    return new Set(parsed.open_ids.filter(Boolean));
  }
  if (parsed && typeof parsed === 'object') {
    return new Set(Object.values(parsed).filter(Boolean));
  }
  return new Set();
}

module.exports = {
  PairingStore,
  parseStore,
};
