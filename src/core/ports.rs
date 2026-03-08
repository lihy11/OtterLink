use anyhow::Result;
use async_trait::async_trait;

use crate::protocol::CoreOutboundEvent;

#[async_trait]
pub trait TurnEventSink: Send + Sync {
    async fn publish(&self, event: &CoreOutboundEvent) -> Result<()>;
}
