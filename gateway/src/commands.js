function parseControlCommand(text) {
  const parts = String(text || '').trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) {
    return null;
  }

  const [head, action, ...rest] = parts;
  const normalizedHead = head.toLowerCase();
  const normalizedAction = (action || '').toLowerCase();

  if (['/runtime', 'runtime', '/rt', '/rumtime', 'rumtime', '会话'].includes(normalizedHead)) {
    if (!normalizedAction || ['help', '帮助'].includes(normalizedAction)) {
      return { local_action: 'runtime_help' };
    }
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
      if (rest.length === 0) {
        return invalidRuntimeCommand('缺少 agent 名称。用法：`/runtime use <claude|codex>`');
      }
      return {
        action: 'use_agent',
        agent_kind: rest.join(' ') || null,
      };
    }
    if (['pick', '选择'].includes(normalizedAction)) {
      if (rest.length === 0) {
        return invalidRuntimeCommand('缺少会话 ID。用法：`/runtime pick <short_id>`');
      }
      return {
        action: 'switch_runtime',
        runtime_selector: rest.join(' ') || null,
      };
    }
    if (['cwd', '工作区', '目录'].includes(normalizedAction)) {
      if (rest.length === 0) {
        return invalidRuntimeCommand('缺少路径。用法：`/runtime cwd <path>`');
      }
      return {
        action: 'set_workspace',
        workspace_path: rest.join(' ') || null,
      };
    }
    if (['stop', 'cancel', '停止', '中断'].includes(normalizedAction)) {
      return {
        action: 'stop_runtime',
      };
    }
    if (['proxy', '代理'].includes(normalizedAction)) {
      if (rest.length === 0) {
        return invalidRuntimeCommand('缺少代理模式。用法：`/runtime proxy <default|on|off> [proxy_url]`');
      }
      const [mode, ...proxyRest] = rest;
      if (looksLikeProxyUrl(mode)) {
        return {
          action: 'set_proxy',
          proxy_mode: 'on',
          proxy_url: mode,
        };
      }
      return {
        action: 'set_proxy',
        proxy_mode: mode || null,
        proxy_url: proxyRest.join(' ') || null,
      };
    }

    return invalidRuntimeCommand(`未知命令：\`${action}\`。请使用 \`/runtime help\` 查看支持的子命令。`);
  }

  return null;
}

function invalidRuntimeCommand(message) {
  return {
    local_action: 'runtime_invalid',
    message,
  };
}

function looksLikeProxyUrl(value) {
  const text = String(value || '').trim();
  return /^(https?|socks5):\/\//i.test(text) || /^\[(https?|socks5):\/\/.+\]\((https?|socks5):\/\/.+\)$/i.test(text);
}

function renderControlResponse(response) {
  const blocks = [{ kind: 'markdown', text: `📣 **Runtime 控制结果**\n${response.message}` }];

  if (response.selector) {
    blocks.push({ kind: 'divider' });
    blocks.push({ kind: 'markdown', text: formatSelectorSummary(response.selector, response.active_runtime) });
  }

  if (response.active_runtime) {
    blocks.push({ kind: 'divider' });
    blocks.push({
      kind: 'markdown',
      text: `👉 **当前已选会话**：\`${runtimeDisplayId(response.active_runtime)}\`${response.active_runtime.tag ? ` · ${escapeCell(response.active_runtime.tag)}` : ''}`,
    });
  }

  if (response.runtimes && response.runtimes.length > 0) {
    blocks.push({ kind: 'divider' });
    blocks.push({
      kind: 'markdown',
      text: [
        `📋 **会话列表** · 共 ${response.runtimes.length} 个`,
        '',
        formatRuntimeMarkdownTable(response.runtimes),
      ].join('\n'),
    });
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

function renderHistoryOverview(historyOverview) {
  const rows = historyOverview.turns.map((turn, index) => {
    const lines = [`${index + 1}.`];
    if (turn.user_text) {
      lines.push(`- user: ${turn.user_text}`);
    }
    if (turn.assistant_text) {
      lines.push(`- assistant: ${turn.assistant_text}`);
    }
    return lines.join('\n');
  });

  return {
    kind: 'card',
    card: {
      title: '历史概览',
      theme: 'grey',
      wide_screen_mode: true,
      update_multi: false,
      blocks: [
        {
          kind: 'markdown',
          text: `🕘 **最近 ${historyOverview.turns.length} 轮历史记录** · Session \`${String(historyOverview.runtime_session_ref || '').slice(0, 8)}\``,
        },
        { kind: 'divider' },
        {
          kind: 'markdown',
          text: rows.join('\n\n'),
        },
      ],
    },
  };
}

function formatSelectorSummary(selector, activeRuntime) {
  const lines = [
    '🎛️ **当前选择器**',
    `- Agent: \`${selector.agent_kind}\``,
    `- CWD: \`${selector.workspace_path}\``,
    `- Proxy: ${formatProxySummary(selector)}`,
    `- Session: ${selector.has_selected_runtime && activeRuntime ? `\`${runtimeDisplayId(activeRuntime)}\`` : '未选择'}`,
  ];
  return lines.join('\n');
}

function renderRuntimeHelp() {
  return {
    kind: 'card',
    card: {
      title: 'Runtime Help',
      theme: 'grey',
      wide_screen_mode: true,
      update_multi: false,
      blocks: [
        { kind: 'markdown', text: '📚 **当前支持的 Runtime 命令**' },
        { kind: 'divider' },
        {
          kind: 'markdown',
          text: [
            '`/runtime help`',
            '`/runtime show`',
            '`/runtime list`',
            '`/runtime load [workspace]`',
            '`/runtime use <claude|codex>`',
            '`/runtime pick <short_id>`',
            '`/runtime new <label>`',
            '`/runtime cwd <path>`',
            '`/runtime stop`',
            '`/runtime proxy <default|on|off> [proxy_url]`',
          ].join('\n'),
        },
        { kind: 'divider' },
        {
          kind: 'markdown',
          text: [
            '中文别名：',
            '`会话 帮助`',
            '`会话 查看`',
            '`会话 列表`',
            '`会话 加载 [workspace]`',
            '`会话 切换 <claude|codex>`',
            '`会话 选择 <短ID>`',
            '`会话 新建 <名称>`',
            '`会话 工作区 <路径>`',
            '`会话 停止`',
            '`会话 代理 <default|on|off> [proxy_url]`',
          ].join('\n'),
        },
      ],
    },
  };
}

function formatRuntimeMarkdownTable(runtimes) {
  const rows = [
    '| 状态 | Tag | 短ID | Prompt |',
    '| --- | --- | --- | --- |',
  ];
  runtimes.forEach((runtime) => {
    rows.push(
      `| ${runtime.is_active ? '👉' : ''} | ${escapeTableCell(runtime.tag || '-')} | ${escapeTableCell(runtimeDisplayId(runtime))} | ${escapeTableCell(runtime.prompt_preview || shortLabel(runtime.label) || '-')} |`,
    );
  });
  return rows.join('\n');
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

function escapeTableCell(value) {
  return escapeCell(value).replace(/\|/g, '\\|');
}

function formatProxySummary(selector) {
  if (selector.proxy_mode === 'on') {
    return selector.proxy_url ? `\`on · ${selector.proxy_url}\`` : '`on`';
  }
  if (selector.proxy_mode === 'off') {
    return '`off`';
  }
  return selector.proxy_url ? `\`default · ${selector.proxy_url}\`` : '`default`';
}

module.exports = {
  parseControlCommand,
  renderControlResponse,
  renderHistoryOverview,
  renderRuntimeHelp,
  runtimeDisplayId,
  invalidRuntimeCommand,
  looksLikeProxyUrl,
};
