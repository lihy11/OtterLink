const test = require('node:test');
const assert = require('node:assert/strict');

const { renderCardKitCard, renderOutboundMessage } = require('../src/feishu/render');

test('renderOutboundMessage converts standard card to cardkit transport payload', () => {
  const rendered = renderOutboundMessage({
    kind: 'card',
    card: {
      title: 'Progress',
      theme: 'blue',
      wide_screen_mode: true,
      update_multi: true,
      blocks: [
        { kind: 'markdown', text: 'hello' },
        { kind: 'divider' },
      ],
    },
  });

  assert.equal(rendered.transport, 'cardkit');
  assert.equal(rendered.card.body.elements[0].tag, 'markdown');
  assert.match(rendered.card.body.elements[0].content, /Progress/);
  assert.match(rendered.card.body.elements[0].content, /hello/);
});

test('renderCardKitCard keeps title and streaming config in body-only cardkit payload', () => {
  const progress = renderCardKitCard({
    title: 'Progress',
    theme: 'grey',
    wide_screen_mode: true,
    update_multi: true,
    blocks: [{ kind: 'markdown', text: 'running' }],
  });
  const todo = renderCardKitCard({
    title: 'Todo',
    theme: 'orange',
    wide_screen_mode: true,
    update_multi: true,
    blocks: [{ kind: 'markdown', text: 'todo' }],
  });

  assert.equal(progress.body.elements[0].tag, 'markdown');
  assert.match(progress.body.elements[0].content, /Progress/);
  assert.match(todo.body.elements[0].content, /Todo/);
  assert.equal(progress.config.streaming_mode, true);
});
