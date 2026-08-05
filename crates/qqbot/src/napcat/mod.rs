//! NapCat/OneBot 11 协议适配。

mod api;
pub mod capabilities;
mod error;
pub mod event;
pub mod heartbeat;
pub mod listener;
pub mod message_parser;
pub mod segments;

pub use api::{
    FriendHistoryQuery, FriendInfoData, GroupHistoryQuery, GroupInfoData, HistoryMessage,
    HistoryReadDirection, HistorySender, MAX_RESPONSE_BYTES, NapCatCapabilityReadT,
    NapCatDirectoryReadT, NapCatHistoryReadT, NapCatReadOnlyClient, RecentContactData, StatusData,
    VersionInfoData,
};
pub use capabilities::{ApiAvailability, CapabilitySnapshot};
pub use error::NapCatError;
pub use event::{
    FriendRecallEvent, GroupMemberDecreaseEvent, GroupMemberIncreaseEvent, GroupMessageEvent,
    GroupRecallEvent, MessageSegment, NapCatConnectionObserver, NapCatEvent, NapCatEventHandler,
    PokeEvent, PrivateMessageEvent, RichKind, SenderInfo,
};
pub use heartbeat::{
    HeartbeatConfig, HeartbeatDeadline, HeartbeatState, LifecycleState, MetaEvent, parse_meta_event,
};
pub use listener::NapCatListener;
