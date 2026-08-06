//! QQ 开放平台协议适配器。
//!
//! 本 crate 只负责凭据换取、HTTP API、Gateway 会话和类型化协议事件；不包含个人秘书
//! 业务、数据库访问或 Agent 工具。每个实例只服务一个 AppID，避免跨 Bot OpenID 污染。

mod api;
mod credentials;
mod gateway;
mod token;

pub use api::{
    QqApiError, QqMessageReceipt, QqOpenPlatformClient, QqOpenPlatformEndpoints, QqTarget,
};
pub use credentials::{QqBotCredentials, QqCredentialsError};
pub use gateway::{
    GatewayEventHandlerT, GatewayRunError, GatewaySession, GatewaySessionStoreT, QqGatewayClient,
    QqGatewayEvent, QqGatewayEventKind,
};
pub use token::TokenStatus;
