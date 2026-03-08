function renderOutboundMessage(message) {
  switch (message.kind) {
    case 'text':
      return { transport: 'message', msg_type: 'text', content: { text: message.text } };
    case 'post':
      return {
        transport: 'message',
        msg_type: 'post',
        content: {
          zh_cn: {
            title: message.title,
            content: [[{ tag: 'text', text: message.text }]],
          },
        },
      };
    case 'card':
      return {
        transport: 'cardkit',
        card: renderCardKitCard(message.card),
      };
    case 'raw':
      return {
        transport: 'message',
        msg_type: message.msg_type,
        content: message.content,
      };
    default:
      throw new Error(`unsupported outbound message kind=${message.kind}`);
  }
}

function renderCardKitCard(card) {
  const markdown = renderCardMarkdown(card);
  return {
    schema: '2.0',
    config: {
      streaming_mode: Boolean(card.update_multi),
      summary: {
        content: summarizeMarkdown(markdown),
      },
      streaming_config: {
        print_frequency_ms: { default: 80 },
        print_step: { default: 4 },
      },
    },
    body: {
      elements: [
        {
          tag: 'markdown',
          content: markdown,
          element_id: 'content',
        },
      ],
    },
  };
}

function renderCardMarkdown(card) {
  const parts = [];
  if (card.title) {
    parts.push(`**${card.title}**`);
  }
  for (const block of card.blocks) {
    if (block.kind === 'divider') {
      parts.push('---');
    } else if (block.kind === 'markdown') {
      parts.push(block.text);
    }
  }
  return parts.join('\n\n').trim();
}

function summarizeMarkdown(markdown) {
  const compact = String(markdown || '')
    .replace(/[#>*`|]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (compact.length <= 120) {
    return compact || '[Generating...]';
  }
  return `${compact.slice(0, 117)}...`;
}

module.exports = {
  renderCardKitCard,
  renderCardMarkdown,
  renderOutboundMessage,
};
