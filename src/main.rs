use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    feishu_acp_bridge_demo::run().await
}
