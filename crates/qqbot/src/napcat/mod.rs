//! NapCat/OneBot 11 协议适配。

pub mod api;
mod error;
mod event;
pub mod listener;
pub mod message_parser;

pub use error::NapCatError;
pub use event::{
    GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent, MessageSegment,
    NapCatEvent, NapCatEventHandler, PokeEvent, SenderInfo,
};
pub use listener::NapCatListener;
