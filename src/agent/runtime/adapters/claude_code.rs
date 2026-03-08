use super::AcpAdapterSpec;

pub fn spec() -> AcpAdapterSpec {
    AcpAdapterSpec {
        id: "claude_code",
        default_command: "claude-code-acp",
        session_mode: Some("bypassPermissions"),
    }
}
