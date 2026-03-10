use super::AcpAdapterSpec;

pub fn spec() -> AcpAdapterSpec {
    AcpAdapterSpec {
        id: "codex",
        default_command: "npx -y -p @zed-industries/codex-acp@0.9.2 -p @zed-industries/codex-acp-linux-x64@0.9.2 codex-acp -c approval_policy=never -c sandbox_mode=\"danger-full-access\"",
        session_mode: None,
    }
}
