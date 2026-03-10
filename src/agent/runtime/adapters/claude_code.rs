use super::AcpAdapterSpec;

pub fn spec() -> AcpAdapterSpec {
    AcpAdapterSpec {
        id: "claude_code",
        default_command: "npx -y @zed-industries/claude-code-acp@0.16.2",
        session_mode: Some("bypassPermissions"),
    }
}
