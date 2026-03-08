use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoEntry {
    pub content: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutboundMessage {
    Text { text: String },
    Post { title: String, text: String },
    Card { card: StandardCard },
    Raw { msg_type: String, content: Value },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandardCard {
    pub title: String,
    pub theme: CardTheme,
    pub wide_screen_mode: bool,
    pub update_multi: bool,
    pub blocks: Vec<CardBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardTheme {
    Blue,
    Green,
    Grey,
    Orange,
    Red,
    Wathet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardBlock {
    Markdown { text: String },
    Divider,
}
