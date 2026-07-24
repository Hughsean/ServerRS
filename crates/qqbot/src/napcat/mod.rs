//! NapCat/OneBot 11 协议适配。

pub mod api;
mod error;
mod event;
pub mod listener;
pub mod message_parser;

pub use api::{HistoryMessage, HistorySender, NapCatApiClient};
pub use error::NapCatError;
pub use event::{
    GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent, MessageSegment,
    NapCatConnectionObserver, NapCatEvent, NapCatEventHandler, PokeEvent, PrivateMessageEvent,
    SenderInfo,
};
pub use listener::NapCatListener;
