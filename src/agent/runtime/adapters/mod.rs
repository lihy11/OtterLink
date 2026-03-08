pub mod claude_code;
pub mod codex;

use anyhow::{anyhow, Result};

#[derive(Clone)]
pub struct AcpAdapterSpec {
    pub id: &'static str,
    pub default_command: &'static str,
    pub session_mode: Option<&'static str>,
}

pub fn for_id(adapter: &str) -> Result<AcpAdapterSpec> {
    match adapter {
        "codex" => Ok(codex::spec()),
        "claude_code" => Ok(claude_code::spec()),
        other => Err(anyhow!("unsupported ACP_ADAPTER: {}", other)),
    }
}

pub fn default_command(adapter: &str) -> Result<String> {
    Ok(for_id(adapter)?.default_command.to_string())
}
