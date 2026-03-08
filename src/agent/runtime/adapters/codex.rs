use super::AcpAdapterSpec;

pub fn spec() -> AcpAdapterSpec {
    AcpAdapterSpec {
        id: "codex",
        default_command: "npx -y @zed-industries/codex-acp@0.9.4 -c approval_policy=never -c sandbox_mode=\"danger-full-access\"",
        session_mode: None,
    }
}
