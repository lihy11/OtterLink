use crate::core::models::{CardBlock, CardTheme, OutboundMessage, StandardCard};

pub fn text_message(text: impl Into<String>) -> OutboundMessage {
    OutboundMessage::Text { text: text.into() }
}

pub fn card_message(
    title: impl Into<String>,
    theme: CardTheme,
    update_multi: bool,
    blocks: Vec<CardBlock>,
) -> OutboundMessage {
    OutboundMessage::Card {
        card: StandardCard {
            title: title.into(),
            theme,
            wide_screen_mode: true,
            update_multi,
            blocks,
        },
    }
}
