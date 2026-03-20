use anyhow::{anyhow, Result};

use crate::{
    core::{
        message_builder::{card_message, text_message},
        models::{CardBlock, CardTheme, OutboundMessage},
    },
    protocol::{
        ControlAction, CoreControlRequest, CoreControlResponse, RuntimeHistoryOverview, RuntimeSummary,
    },
};

pub enum ParsedInboundMessage {
    Turn,
    Help,
    Invalid { message: String },
    Control(CoreControlRequest),
}

pub fn parse_inbound_message(
    text: &str,
    session_key: String,
    parent_session_key: Option<String>,
) -> ParsedInboundMessage {
    let parts = text.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        return ParsedInboundMessage::Turn;
    }

    let head = parts[0].to_lowercase();
    if !matches!(head.as_str(), "/ot" | "ot" | "会话") {
        return ParsedInboundMessage::Turn;
    }

    let action = parts.get(1).map(|value| value.to_lowercase()).unwrap_or_default();
    let rest = parts.iter().skip(2).copied().collect::<Vec<_>>().join(" ");

    if action.is_empty() || matches!(action.as_str(), "help" | "帮助") {
        return ParsedInboundMessage::Help;
    }

    let request = |action: ControlAction| CoreControlRequest {
        session_key: session_key.clone(),
        parent_session_key: parent_session_key.clone(),
        action,
        runtime_selector: None,
        workspace_path: None,
        label: None,
        agent_kind: None,
        proxy_mode: None,
        proxy_url: None,
    };

    match action.as_str() {
        "show" | "当前" | "查看" => ParsedInboundMessage::Control(request(ControlAction::ShowRuntime)),
        "list" | "列表" => ParsedInboundMessage::Control(request(ControlAction::ListRuntimes)),
        "load" | "import" | "加载" => ParsedInboundMessage::Control(CoreControlRequest {
            workspace_path: if rest.is_empty() { None } else { Some(rest) },
            ..request(ControlAction::LoadRuntimes)
        }),
        "new" | "新建" | "创建" => ParsedInboundMessage::Control(CoreControlRequest {
            label: if rest.is_empty() { None } else { Some(rest) },
            ..request(ControlAction::CreateRuntime)
        }),
        "use" | "switch" | "切换" => {
            if rest.is_empty() {
                ParsedInboundMessage::Invalid {
                    message: "缺少 agent 名称。用法：`/ot use <claude|codex>`".to_string(),
                }
            } else {
                ParsedInboundMessage::Control(CoreControlRequest {
                    agent_kind: Some(rest),
                    ..request(ControlAction::UseAgent)
                })
            }
        }
        "pick" | "选择" => {
            if rest.is_empty() {
                ParsedInboundMessage::Invalid {
                    message: "缺少会话 ID。用法：`/ot pick <short_id>`".to_string(),
                }
            } else {
                ParsedInboundMessage::Control(CoreControlRequest {
                    runtime_selector: Some(rest),
                    ..request(ControlAction::SwitchRuntime)
                })
            }
        }
        "cwd" | "工作区" | "目录" => {
            if rest.is_empty() {
                ParsedInboundMessage::Invalid {
                    message: "缺少路径。用法：`/ot cwd <path>`".to_string(),
                }
            } else {
                ParsedInboundMessage::Control(CoreControlRequest {
                    workspace_path: Some(rest),
                    ..request(ControlAction::SetWorkspace)
                })
            }
        }
        "stop" | "cancel" | "停止" | "中断" => {
            ParsedInboundMessage::Control(request(ControlAction::StopRuntime))
        }
        "proxy" | "代理" => {
            let mut proxy_parts = parts.iter().skip(2).copied().collect::<Vec<_>>();
            if proxy_parts.is_empty() {
                return ParsedInboundMessage::Invalid {
                    message: "缺少代理模式。用法：`/ot proxy <default|on|off> [proxy_url]`".to_string(),
                };
            }

            let mode = proxy_parts.remove(0).to_string();
            if looks_like_proxy_url(&mode) {
                return ParsedInboundMessage::Control(CoreControlRequest {
                    proxy_mode: Some("on".to_string()),
                    proxy_url: Some(mode),
                    ..request(ControlAction::SetProxy)
                });
            }

            ParsedInboundMessage::Control(CoreControlRequest {
                proxy_mode: Some(mode),
                proxy_url: if proxy_parts.is_empty() {
                    None
                } else {
                    Some(proxy_parts.join(" "))
                },
                ..request(ControlAction::SetProxy)
            })
        }
        _ => ParsedInboundMessage::Invalid {
            message: format!("未知命令：`{}`。请使用 `/ot help` 查看支持的子命令。", action),
        },
    }
}

pub fn render_runtime_help() -> OutboundMessage {
    card_message(
        "Runtime Help",
        CardTheme::Grey,
        false,
        vec![
            CardBlock::Markdown {
                text: "📚 **当前支持的 Runtime 命令**".to_string(),
            },
            CardBlock::Divider,
            CardBlock::Markdown {
                text: [
                    "`/ot help`",
                    "`/ot show`",
                    "`/ot list`",
                    "`/ot load [workspace]`",
                    "`/ot use <claude|codex>`",
                    "`/ot pick <short_id>`",
                    "`/ot new <label>`",
                    "`/ot cwd <path>`",
                    "`/ot stop`",
                    "`/ot proxy <default|on|off> [proxy_url]`",
                ]
                .join("\n"),
            },
            CardBlock::Divider,
            CardBlock::Markdown {
                text: [
                    "中文别名：",
                    "`会话 帮助`",
                    "`会话 查看`",
                    "`会话 列表`",
                    "`会话 加载 [workspace]`",
                    "`会话 切换 <claude|codex>`",
                    "`会话 选择 <短ID>`",
                    "`会话 新建 <名称>`",
                    "`会话 工作区 <路径>`",
                    "`会话 停止`",
                    "`会话 代理 <default|on|off> [proxy_url]`",
                ]
                .join("\n"),
            },
        ],
    )
}

pub fn render_invalid_runtime_command(message: &str) -> OutboundMessage {
    text_message(format!("Runtime 命令错误：{}", message))
}

pub fn render_control_response(response: &CoreControlResponse) -> Vec<OutboundMessage> {
    let mut blocks = vec![CardBlock::Markdown {
        text: format!("📣 **Runtime 控制结果**\n{}", response.message),
    }];

    if let Some(selector) = response.selector.as_ref() {
        blocks.push(CardBlock::Divider);
        blocks.push(CardBlock::Markdown {
            text: format_selector_summary(
                selector.agent_kind.as_str(),
                selector.workspace_path.as_str(),
                selector.proxy_mode.as_str(),
                selector.proxy_url.as_deref(),
                response.active_runtime.as_ref(),
            ),
        });
    }

    if let Some(active_runtime) = response.active_runtime.as_ref() {
        blocks.push(CardBlock::Divider);
        blocks.push(CardBlock::Markdown {
            text: format!(
                "👉 **当前已选会话**：`{}`{}",
                runtime_display_id(active_runtime),
                active_runtime
                    .tag
                    .as_deref()
                    .map(|tag| format!(" · {}", escape_cell(tag)))
                    .unwrap_or_default()
            ),
        });
    }

    if !response.runtimes.is_empty() {
        blocks.push(CardBlock::Divider);
        blocks.push(CardBlock::Markdown {
            text: format!(
                "📋 **会话列表** · 共 {} 个\n\n{}",
                response.runtimes.len(),
                format_runtime_markdown_table(&response.runtimes)
            ),
        });
    }

    let mut messages = vec![card_message("Runtime 控制", CardTheme::Grey, false, blocks)];
    if let Some(history) = response.history_overview.as_ref() {
        messages.push(render_history_overview(history));
    }
    messages
}

fn render_history_overview(history: &RuntimeHistoryOverview) -> OutboundMessage {
    let rows = history
        .turns
        .iter()
        .enumerate()
        .map(|(index, turn)| {
            let mut lines = vec![format!("{}.", index + 1)];
            if !turn.user_text.is_empty() {
                lines.push(format!("- user: {}", turn.user_text));
            }
            if !turn.assistant_text.is_empty() {
                lines.push(format!("- assistant: {}", turn.assistant_text));
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    card_message(
        "历史概览",
        CardTheme::Grey,
        false,
        vec![
            CardBlock::Markdown {
                text: format!(
                    "🕘 **最近 {} 轮历史记录** · Session `{}`",
                    history.turns.len(),
                    history.runtime_session_ref.chars().take(8).collect::<String>()
                ),
            },
            CardBlock::Divider,
            CardBlock::Markdown { text: rows },
        ],
    )
}

fn format_selector_summary(
    agent_kind: &str,
    workspace_path: &str,
    proxy_mode: &str,
    proxy_url: Option<&str>,
    active_runtime: Option<&RuntimeSummary>,
) -> String {
    [
        "🎛️ **当前选择器**".to_string(),
        format!("- Agent: `{}`", agent_kind),
        format!("- CWD: `{}`", workspace_path),
        format!("- Proxy: {}", format_proxy_summary(proxy_mode, proxy_url)),
        format!(
            "- Session: {}",
            active_runtime
                .map(|runtime| format!("`{}`", runtime_display_id(runtime)))
                .unwrap_or_else(|| "未选择".to_string())
        ),
    ]
    .join("\n")
}

fn format_proxy_summary(proxy_mode: &str, proxy_url: Option<&str>) -> String {
    match proxy_url {
        Some(url) if !url.is_empty() => format!("{} · {}", proxy_mode, short_path(url)),
        _ => proxy_mode.to_string(),
    }
}

fn format_runtime_markdown_table(runtimes: &[RuntimeSummary]) -> String {
    let mut rows = vec![
        "| 状态 | Tag | 短ID | Prompt |".to_string(),
        "| --- | --- | --- | --- |".to_string(),
    ];
    for runtime in runtimes {
        let prompt = runtime
            .prompt_preview
            .clone()
            .unwrap_or_else(|| short_label(&runtime.label));
        rows.push(format!(
            "| {} | {} | {} | {} |",
            if runtime.is_active { "👉" } else { "" },
            escape_cell(runtime.tag.as_deref().unwrap_or("-")),
            escape_cell(&runtime_display_id(runtime)),
            escape_cell(&prompt),
        ));
    }
    rows.join("\n")
}

fn short_label(value: &str) -> String {
    value
        .strip_prefix("claude_code-")
        .or_else(|| value.strip_prefix("codex-"))
        .unwrap_or(value)
        .to_string()
}

fn short_path(value: &str) -> String {
    if value.chars().count() > 48 {
        let tail = value.chars().rev().take(45).collect::<String>();
        format!("...{}", tail.chars().rev().collect::<String>())
    } else {
        value.to_string()
    }
}

fn runtime_display_id(runtime: &RuntimeSummary) -> String {
    runtime
        .runtime_session_ref
        .as_deref()
        .unwrap_or(runtime.runtime_id.as_str())
        .chars()
        .take(8)
        .collect()
}

fn escape_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br/>")
}

fn looks_like_proxy_url(value: &str) -> bool {
    let text = value.trim();
    text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("socks5://")
        || (text.starts_with("[http://")
            || text.starts_with("[https://")
            || text.starts_with("[socks5://"))
}

pub fn parse_runtime_command_or_err(
    text: &str,
    session_key: String,
    parent_session_key: Option<String>,
) -> Result<ParsedInboundMessage> {
    let parsed = parse_inbound_message(text, session_key, parent_session_key);
    match parsed {
        ParsedInboundMessage::Invalid { message } if message.is_empty() => {
            Err(anyhow!("invalid runtime command"))
        }
        other => Ok(other),
    }
}
