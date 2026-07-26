//! NapCat/OneBot 11 协议适配。

pub mod api;
pub mod capabilities;
mod error;
pub mod event;
pub mod heartbeat;
pub mod listener;
pub mod message_parser;
pub mod segments;

pub use api::{
    FriendInfoData, HistoryMessage, HistorySender, MAX_RESPONSE_BYTES, NapCatApiClient,
    RecentContactData, StatusData, VersionInfoData,
};
pub use capabilities::{ApiAvailability, CapabilitySnapshot};
pub use error::NapCatError;
pub use event::{
    GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent, MessageSegment,
    NapCatConnectionObserver, NapCatEvent, NapCatEventHandler, PokeEvent, PrivateMessageEvent,
    RichKind, SenderInfo,
};
pub use heartbeat::{
    HeartbeatConfig, HeartbeatDeadline, HeartbeatState, LifecycleState, MetaEvent, parse_meta_event,
};
pub use listener::NapCatListener;
