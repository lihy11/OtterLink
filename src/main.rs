use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    otterlink::run().await
}
