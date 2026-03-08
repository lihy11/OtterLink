function parseControlCommand(text) {
  const parts = String(text || '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) {
    return null;
  }

  const [head, action, ...rest] = parts;
  const normalizedHead = head.toLowerCase();
  const normalizedAction = (action || '').toLowerCase();

  if (['/runtime', 'runtime', '/rt', '会话'].includes(normalizedHead)) {
    if (['show', '当前', '查看'].includes(normalizedAction)) {
      return { action: 'show_runtime' };
    }
    if (['list', '列表'].includes(normalizedAction)) {
      return { action: 'list_runtimes' };
    }
    if (['load', 'import', '加载'].includes(normalizedAction)) {
      return {
        action: 'load_runtimes',
        workspace_path: rest.join(' ') || null,
      };
    }
    if (['new', '新建', '创建'].includes(normalizedAction)) {
      return {
        action: 'create_runtime',
        label: rest.join(' ') || null,
      };
    }
    if (['use', 'switch', '切换'].includes(normalizedAction)) {
      return {
        action: 'switch_runtime',
        runtime_selector: rest.join(' ') || null,
      };
    }
  }

  if (['/workspace', 'workspace', '/ws', '工作区'].includes(normalizedHead)) {
    if (['show', '当前', '查看'].includes(normalizedAction)) {
      return { action: 'show_runtime' };
    }
    if (['set', '设置'].includes(normalizedAction)) {
      return {
        action: 'set_workspace',
        workspace_path: rest.join(' ') || null,
      };
    }
  }

  return null;
}

function renderControlResponse(response) {
  const blocks = [{ kind: 'markdown', text: `📣 **Runtime 控制结果**\n${response.message}` }];

  if (response.active_runtime) {
    blocks.push({ kind: 'divider' });
    blocks.push({ kind: 'markdown', text: '👉 **当前运行会话**' });
    blocks.push(...formatRuntimeCardRows([response.active_runtime]));
  }

  if (response.runtimes && response.runtimes.length > 0) {
    blocks.push({ kind: 'divider' });
    blocks.push({ kind: 'markdown', text: `📋 **会话列表** · 共 ${response.runtimes.length} 个` });
    blocks.push(...formatRuntimeCardRows(response.runtimes));
  }

  return {
    kind: 'card',
    card: {
      title: 'Runtime 控制',
      theme: 'grey',
      wide_screen_mode: true,
      update_multi: false,
      blocks,
    },
  };
}

function formatRuntimeCardRows(runtimes) {
  const blocks = [];
  runtimes.forEach((runtime, index) => {
    if (index > 0) {
      blocks.push({ kind: 'divider' });
    }
    blocks.push({
      kind: 'markdown',
      text: [
        `${runtime.is_active ? '👉' : '•'} **${shortLabel(runtime.label)}**`,
        `\`${runtime.agent_kind}\` · #${escapeCell(runtime.tag || '-') } · \`${runtimeDisplayId(runtime)}\``,
        runtime.prompt_preview ? `📝 ${escapeCell(runtime.prompt_preview)}` : '📝 -',
        `📁 ${shortPath(runtime.workspace_path)}`,
      ].join('\n'),
    });
  });
  return blocks;
}

function runtimeDisplayId(runtime) {
  const source = runtime.runtime_session_ref || runtime.runtime_id || '';
  return source.slice(0, 8);
}

function shortLabel(value) {
  return String(value || '').replace(/^claude_code-/, '').replace(/^codex-/, '');
}

function shortPath(value) {
  const text = String(value || '');
  return text.length > 48 ? `...${text.slice(-45)}` : text;
}

function escapeCell(value) {
  return String(value || '').replace(/\s+/g, ' ').trim() || '-';
}

module.exports = {
  parseControlCommand,
  renderControlResponse,
  runtimeDisplayId,
};
