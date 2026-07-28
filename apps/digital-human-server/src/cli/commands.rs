//! 斜杠命令分发与处理。
//!
//! Session 是 REPL 贯穿全程的状态,定义在此处供 repl 与 commands 共享。

use std::io::Write;
use std::sync::Arc;

use crate::cli::audio_player::AudioPlayer;
use crate::cli::client::ApiClient;
use crate::cli::dto::{
    ChatSuspendedResponse, ChatTurnResponse, ChatTurnWithAudioResponse, PendingApprovalItem,
};
use crate::cli::error::CliError;
use crate::cli::render;

/// 确认回调:展示提示并等待用户确认,仅 "y"/"Y" 视为确认。
/// 生产实现读 stdin;测试注入闭包,避免阻塞。
pub type ConfirmFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// REPL 全局状态。
pub struct Session {
    pub client: Arc<ApiClient>,
    pub conversation_id: Option<u64>,
    pub personalization_enabled: bool,
    pub username: String,
    /// 最近一次明确保存的待审批任务(收到 202 或 resume 再次暂停时更新)。
    /// `/approve`、`/reject` 不带参数时只能使用这一项,绝不能任意选择服务器列表第一项。
    pub pending_approval: Option<PendingApprovalItem>,
    pub confirm: ConfirmFn,
    pub audio_player: Arc<dyn AudioPlayer>,
}

/// 解析后的命令。`Text` 表示非斜杠输入,作为对话消息发送。
#[derive(Debug)]
pub enum Command {
    Text(String),
    Help,
    Quit,
    History {
        limit: u64,
    },
    Clear,
    Reopen,
    Forget,
    Memories {
        mem_type: Option<String>,
        limit: usize,
    },
    Persona,
    Profile,
    Rebuild,
    Reset,
    Approvals {
        limit: u32,
    },
    Approve {
        checkpoint_id: Option<String>,
    },
    Reject {
        checkpoint_id: Option<String>,
    },
    Unknown(String),
}

/// 解析一行输入为命令。
pub fn parse(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Text(String::new()); // 空输入,发送时会被忽略
    }
    if !trimmed.starts_with('/') {
        return Command::Text(trimmed.to_string());
    }
    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    match cmd {
        "/help" => Command::Help,
        "/quit" | "/exit" => Command::Quit,
        "/history" => Command::History {
            limit: rest.first().and_then(|s| s.parse().ok()).unwrap_or(20),
        },
        "/clear" => Command::Clear,
        "/reopen" => Command::Reopen,
        "/forget" => Command::Forget,
        "/memories" => {
            let mem_type = rest.first().map(|s| s.to_string());
            let limit = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
            Command::Memories { mem_type, limit }
        }
        "/persona" => Command::Persona,
        "/profile" => Command::Profile,
        "/rebuild" => Command::Rebuild,
        "/reset" => Command::Reset,
        "/approvals" => Command::Approvals {
            limit: rest.first().and_then(|s| s.parse().ok()).unwrap_or(20),
        },
        "/approve" => Command::Approve {
            checkpoint_id: rest.first().map(|s| s.to_string()),
        },
        "/reject" => Command::Reject {
            checkpoint_id: rest.first().map(|s| s.to_string()),
        },
        other => Command::Unknown(other.to_string()),
    }
}

/// 处理命令,输出结果到 writer。返回 true 表示应退出 REPL。
pub async fn handle_command<W: Write>(
    session: &mut Session,
    cmd: Command,
    out: &mut W,
) -> Result<bool, CliError> {
    match cmd {
        Command::Text(text) => {
            if text.is_empty() {
                return Ok(false);
            }
            let turn = session.client.chat_send_with_audio(&text).await?;
            handle_audio_turn(session, turn, out).await?;
        }
        Command::Help => {
            writeln!(out, "{}", render::help())?;
        }
        Command::Quit => {
            return Ok(true);
        }
        Command::History { limit } => {
            let resp = session.client.chat_history(limit).await?;
            writeln!(out, "{}", render::history(&resp.messages))?;
        }
        Command::Clear => {
            session.client.transcript_clear().await?;
            writeln!(out, "已清空当前会话转写(记忆和画像保留)")?;
        }
        Command::Reopen => {
            let resp = session.client.chat_open().await?;
            session.conversation_id = Some(resp.conversation.id);
            session.personalization_enabled = resp.personalization_enabled;
            writeln!(
                out,
                "已重新开启会话,conversation_id = {}",
                resp.conversation.id
            )?;
        }
        Command::Forget => {
            if !(session.confirm)("将遗忘全部对话/记忆/画像,不可逆。确认? (y/N): ")
            {
                writeln!(out, "已取消")?;
                return Ok(false);
            }
            let resp = session.client.forget().await?;
            writeln!(
                out,
                "已遗忘: 禁用 {} 条记忆,画像已失效",
                resp.memories_disabled
            )?;
            // forget 后旧会话已清空,自动重新 open
            let reopened = session.client.chat_open().await?;
            session.conversation_id = Some(reopened.conversation.id);
            session.personalization_enabled = reopened.personalization_enabled;
            writeln!(
                out,
                "已开启新会话,conversation_id = {}",
                reopened.conversation.id
            )?;
        }
        Command::Memories { mem_type, limit } => {
            let resp = session
                .client
                .chat_memories(mem_type.as_deref(), limit)
                .await?;
            writeln!(
                out,
                "{}",
                render::memories_table(&resp.memories, resp.total_active)
            )?;
        }
        Command::Persona => {
            let resp = session.client.chat_persona().await?;
            writeln!(out, "{}", render::persona(&resp))?;
        }
        Command::Profile => {
            let resp = session.client.user_profile().await?;
            writeln!(out, "{}", render::user_profile(&resp))?;
        }
        Command::Rebuild => {
            let resp = session.client.persona_rebuild().await?;
            session.personalization_enabled = true;
            writeln!(out, "画像已重建,snapshot_id = {}", resp.snapshot_id)?;
        }
        Command::Reset => {
            if !(session.confirm)("将重置个性化(关闭+失效画像)。确认? (y/N): ") {
                writeln!(out, "已取消")?;
                return Ok(false);
            }
            session.client.persona_reset().await?;
            session.personalization_enabled = false;
            writeln!(out, "已重置个性化")?;
        }
        Command::Approvals { limit } => {
            let resp = session.client.list_pending_approvals(limit).await?;
            writeln!(out, "{}", render::approvals_list(&resp.items))?;
        }
        Command::Approve { checkpoint_id } => {
            decide(session, checkpoint_id, true, out).await?;
        }
        Command::Reject { checkpoint_id } => {
            decide(session, checkpoint_id, false, out).await?;
        }
        Command::Unknown(name) => {
            writeln!(out, "未知命令 {name},输入 /help 查看可用命令")?;
        }
    }
    Ok(false)
}

/// 处理聊天联合响应:正常完成直接渲染;暂停则保存待审批并提示。
async fn handle_turn<W: Write>(
    session: &mut Session,
    turn: ChatTurnResponse,
    out: &mut W,
) -> Result<(), CliError> {
    match turn {
        ChatTurnResponse::Completed(resp) => {
            session.conversation_id = Some(resp.conversation_id);
            writeln!(
                out,
                "{}",
                render::assistant_reply(&resp.reply, &resp.tool_calls)
            )?;
        }
        ChatTurnResponse::Suspended(suspended) => {
            session.conversation_id = Some(suspended.conversation_id);
            let item = refresh_pending_item(session, &suspended).await;
            session.pending_approval = Some(item.clone());
            writeln!(out, "{}", render::suspended_notice(&item))?;
        }
    }
    Ok(())
}

/// 处理带音频的首次聊天响应：只有成功完成的轮次才请求本机播放。
async fn handle_audio_turn<W: Write>(
    session: &mut Session,
    turn: ChatTurnWithAudioResponse,
    out: &mut W,
) -> Result<(), CliError> {
    match turn {
        ChatTurnWithAudioResponse::Completed(response) => {
            session.conversation_id = Some(response.conversation_id);
            writeln!(
                out,
                "{}",
                render::assistant_reply(&response.reply, &response.tool_calls)
            )?;
            if session
                .audio_player
                .play(&response.audio.audio_url)
                .is_err()
            {
                writeln!(out, "语音回复已生成，但本机自动播放失败。")?;
            }
        }
        ChatTurnWithAudioResponse::Suspended(suspended) => {
            session.conversation_id = Some(suspended.conversation_id);
            let item = refresh_pending_item(session, &suspended).await;
            session.pending_approval = Some(item.clone());
            writeln!(out, "{}", render::suspended_notice(&item))?;
        }
    }
    Ok(())
}

/// 用详情接口补全 202 响应缺失的创建/过期时间;详情失败时降级为占位值。
async fn refresh_pending_item(
    session: &Session,
    suspended: &ChatSuspendedResponse,
) -> PendingApprovalItem {
    match session
        .client
        .get_checkpoint(&suspended.checkpoint_id)
        .await
    {
        Ok(item) => item,
        Err(_) => PendingApprovalItem {
            status: suspended.status.clone(),
            checkpoint_id: suspended.checkpoint_id.clone(),
            run_id: suspended.run_id.clone(),
            conversation_id: suspended.conversation_id,
            reason: suspended.reason.clone(),
            created_at: String::new(),
            expires_at: String::new(),
            approval: suspended.approval.clone(),
        },
    }
}

/// `/approve` 与 `/reject` 的共用流程。
async fn decide<W: Write>(
    session: &mut Session,
    checkpoint_id: Option<String>,
    approve: bool,
    out: &mut W,
) -> Result<(), CliError> {
    let verb = if approve { "批准" } else { "拒绝" };

    // 1. 确定目标:显式 ID 先查详情拿合法 approval_id;无 ID 只能用 Session
    //    里最近一次明确保存的待审批项,绝不能任意选择服务器列表第一项。
    let target = match checkpoint_id {
        Some(id) => match session.client.get_checkpoint(&id).await {
            Ok(item) => item,
            Err(error) if is_gone_or_consumed(&error) => {
                clear_saved_pending(session, id.trim());
                writeln!(
                    out,
                    "Checkpoint 已过期、已消费或被其他恢复请求处理。请用 /approvals 重新查询。"
                )?;
                return Ok(());
            }
            Err(error) => return Err(error),
        },
        None => match session.pending_approval.clone() {
            Some(item) => item,
            None => {
                writeln!(
                    out,
                    "没有最近保存的待审批任务。请先 /approvals 查看,再 /{verb} <checkpoint_id>。"
                )?;
                return Ok(());
            }
        },
    };

    // 2. 执行前再次展示工具摘要并要求显式确认,绝不自动批准。
    writeln!(out, "{}", render::approval_summary(&target))?;
    let prompt = format!("确认{verb}以上工具调用? (y/N): ");
    if !(session.confirm)(&prompt) {
        writeln!(out, "已取消")?;
        return Ok(());
    }

    // 3. 恢复运行;结果复用统一的联合响应处理逻辑。
    let decision = if approve { "approve" } else { "reject" };
    let turn = match session
        .client
        .resume_checkpoint(
            &target.checkpoint_id,
            &target.approval.approval_id,
            decision,
        )
        .await
    {
        Ok(turn) => turn,
        Err(error) if is_gone_or_consumed(&error) => {
            clear_saved_pending(session, &target.checkpoint_id);
            writeln!(
                out,
                "Checkpoint 已过期、已消费或被其他恢复请求处理。请用 /approvals 重新查询。"
            )?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    // resume 成功:完成的轮次清理对应待审批;再次暂停时 handle_turn 会替换为新项。
    if matches!(turn, ChatTurnResponse::Completed(_)) {
        clear_saved_pending(session, &target.checkpoint_id);
    }
    handle_turn(session, turn, out).await
}

/// 404/409 都表示 Checkpoint 已不可恢复(过期、已消费或被并发恢复处理)。
fn is_gone_or_consumed(error: &CliError) -> bool {
    matches!(error, CliError::Api { status, .. } if *status == 404 || *status == 409)
}

fn clear_saved_pending(session: &mut Session, checkpoint_id: &str) {
    if session
        .pending_approval
        .as_ref()
        .is_some_and(|item| item.checkpoint_id == checkpoint_id)
    {
        session.pending_approval = None;
    }
}

/// 交互式二次确认的生产实现:从 stdin 读一行,仅 "y"/"Y" 为确认。
pub fn stdin_confirm(prompt: &str) -> bool {
    use std::io::BufRead;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map(|_| line.trim().eq_ignore_ascii_case("y"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_text() {
        match parse("你好") {
            Command::Text(t) => assert_eq!(t, "你好"),
            other => panic!("期望 Text,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_empty_as_text_empty() {
        match parse("   ") {
            Command::Text(t) => assert!(t.is_empty()),
            other => panic!("期望空 Text,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_help_and_quit() {
        assert!(matches!(parse("/help"), Command::Help));
        assert!(matches!(parse("/quit"), Command::Quit));
        assert!(matches!(parse("/exit"), Command::Quit));
    }

    #[test]
    fn parse_history_default_limit() {
        match parse("/history") {
            Command::History { limit } => assert_eq!(limit, 20),
            other => panic!("期望 History,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_history_custom_limit() {
        match parse("/history 50") {
            Command::History { limit } => assert_eq!(limit, 50),
            other => panic!("期望 History,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_history_invalid_limit_falls_back() {
        match parse("/history abc") {
            Command::History { limit } => assert_eq!(limit, 20),
            other => panic!("期望 History,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_memories_with_type_and_limit() {
        match parse("/memories fact 50") {
            Command::Memories { mem_type, limit } => {
                assert_eq!(mem_type.as_deref(), Some("fact"));
                assert_eq!(limit, 50);
            }
            other => panic!("期望 Memories,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_memories_defaults() {
        match parse("/memories") {
            Command::Memories { mem_type, limit } => {
                assert!(mem_type.is_none());
                assert_eq!(limit, 20);
            }
            other => panic!("期望 Memories,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_command() {
        match parse("/foobar") {
            Command::Unknown(name) => assert_eq!(name, "/foobar"),
            other => panic!("期望 Unknown,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_approvals_default_and_custom_limit() {
        match parse("/approvals") {
            Command::Approvals { limit } => assert_eq!(limit, 20),
            other => panic!("期望 Approvals,得到 {other:?}"),
        }
        match parse("/approvals 50") {
            Command::Approvals { limit } => assert_eq!(limit, 50),
            other => panic!("期望 Approvals,得到 {other:?}"),
        }
    }

    #[test]
    fn parse_approve_and_reject_with_optional_id() {
        match parse("/approve") {
            Command::Approve { checkpoint_id } => assert!(checkpoint_id.is_none()),
            other => panic!("期望 Approve,得到 {other:?}"),
        }
        match parse("/approve 2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760") {
            Command::Approve { checkpoint_id } => {
                assert_eq!(
                    checkpoint_id.as_deref(),
                    Some("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760")
                )
            }
            other => panic!("期望 Approve,得到 {other:?}"),
        }
        match parse("/reject 2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760") {
            Command::Reject { checkpoint_id } => assert!(checkpoint_id.is_some()),
            other => panic!("期望 Reject,得到 {other:?}"),
        }
    }
}

/// 审批闭环的流程测试:mock HTTP 后端,验证暂停保存、确认、恢复与状态清理。
#[cfg(test)]
mod approval_flow_tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use reqwest::{Method, StatusCode};

    use super::*;
    use crate::cli::audio_player::{AudioPlayer, SystemAudioPlayer};
    use crate::cli::auth::TokenCache;
    use crate::cli::client::HttpBackend;
    use crate::cli::config::CliConfig;

    const CP1: &str = "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760";
    const CP2: &str = "3bb282b3-f4ad-41a6-bf1b-bf5c51fdc760";
    const RUN: &str = "90b4891f-cf68-4c1a-ad83-32d9d8494d18";
    const APPROVAL: &str = "02f941ab-0fb8-4c44-999c-9ff896ef415a";

    struct MockBackend {
        responses: Mutex<VecDeque<(StatusCode, String)>>,
        calls: Mutex<Vec<(Method, String, Option<String>)>>,
    }

    impl MockBackend {
        fn new(responses: Vec<(StatusCode, String)>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(vec![]),
            }
        }

        fn calls(&self) -> Vec<(Method, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HttpBackend for MockBackend {
        async fn execute(
            &self,
            method: Method,
            url: String,
            _auth_header: Option<String>,
            body: Option<String>,
        ) -> Result<(StatusCode, String), CliError> {
            self.calls.lock().unwrap().push((method, url, body));
            let mut responses = self.responses.lock().unwrap();
            Ok(responses.pop_front().expect("MockBackend 无更多预设响应"))
        }
    }

    struct MockAudioPlayer {
        result: Result<(), String>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl MockAudioPlayer {
        fn succeeds() -> Self {
            Self {
                result: Ok(()),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn fails() -> Self {
            Self {
                result: Err("failed".into()),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl AudioPlayer for MockAudioPlayer {
        fn play(&self, _audio_url: &str) -> Result<(), String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.result.clone()
        }
    }

    fn session(
        backend: Arc<MockBackend>,
        confirm: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Session {
        session_with_player(backend, confirm, Arc::new(SystemAudioPlayer))
    }

    fn session_with_player(
        backend: Arc<MockBackend>,
        confirm: impl Fn(&str) -> bool + Send + Sync + 'static,
        audio_player: Arc<dyn AudioPlayer>,
    ) -> Session {
        let config = CliConfig {
            base_url: "http://test".into(),
            token_path: std::path::PathBuf::from("/tmp/x"),
        };
        let client = ApiClient::new(
            &config,
            backend,
            Some(TokenCache {
                access_token: "t".into(),
                refresh_token: "r".into(),
            }),
        );
        Session {
            client: Arc::new(client),
            conversation_id: Some(9),
            personalization_enabled: false,
            username: "tester".into(),
            pending_approval: None,
            confirm: Arc::new(confirm),
            audio_player,
        }
    }

    fn suspended_json(checkpoint_id: &str) -> String {
        format!(
            r#"{{"status":"suspended","conversation_id":9,"checkpoint_id":"{checkpoint_id}","run_id":"{RUN}","reason":"approval","approval":{{"approval_id":"{APPROVAL}","prompt":"模型请求执行受控工具，请确认是否允许。","tool_calls":[{{"id":"call-1","name":"fetch_web_content","arguments":{{"url":"https://example.com"}}}}]}}}}"#
        )
    }

    fn pending_item_json(checkpoint_id: &str) -> String {
        format!(
            r#"{{"status":"pending","checkpoint_id":"{checkpoint_id}","run_id":"{RUN}","conversation_id":9,"reason":"approval","created_at":"2026-07-22T01:00:00+00:00","expires_at":"2026-07-23T01:00:00+00:00","approval":{{"approval_id":"{APPROVAL}","prompt":"模型请求执行受控工具，请确认是否允许。","tool_calls":[{{"id":"call-1","name":"fetch_web_content","arguments":{{"url":"https://example.com"}}}}]}}}}"#
        )
    }

    fn completed_json() -> String {
        r#"{"conversation_id":9,"reply":"已完成","tool_calls":[]}"#.into()
    }

    fn completed_audio_json() -> String {
        r#"{"conversationId":9,"reply":"已完成","toolCalls":[],"audio":{"audioUrl":"https://example.com/audio?signature=x","format":"wav","sampleRate":24000,"channels":1,"sampleBits":16}}"#
            .into()
    }

    #[tokio::test]
    async fn text_with_completed_audio_reply_renders_and_starts_playback() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            completed_audio_json(),
        )]));
        let player = Arc::new(MockAudioPlayer::succeeds());
        let mut session = session_with_player(backend.clone(), |_| true, player.clone());
        let mut out = Vec::new();

        handle_command(&mut session, Command::Text("你好".into()), &mut out)
            .await
            .unwrap();

        assert!(String::from_utf8(out).unwrap().contains("已完成"));
        assert_eq!(player.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            backend.calls()[0]
                .1
                .ends_with("/api/v1/chat/messages-with-audio")
        );
    }

    #[tokio::test]
    async fn audio_player_failure_does_not_hide_completed_reply() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            completed_audio_json(),
        )]));
        let player = Arc::new(MockAudioPlayer::fails());
        let mut session = session_with_player(backend, |_| true, player);
        let mut out = Vec::new();

        handle_command(&mut session, Command::Text("你好".into()), &mut out)
            .await
            .unwrap();

        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("已完成"));
        assert!(output.contains("本机自动播放失败"));
    }

    #[tokio::test]
    async fn text_with_completed_reply_keeps_pending_state_untouched() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            completed_audio_json(),
        )]));
        let mut session = session(backend, |_| true);
        let mut out = Vec::new();

        handle_command(&mut session, Command::Text("你好".into()), &mut out)
            .await
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已完成"));
        assert!(session.pending_approval.is_none());
    }

    #[tokio::test]
    async fn text_with_suspended_saves_pending_and_never_auto_approves() {
        let backend = Arc::new(MockBackend::new(vec![
            (StatusCode::ACCEPTED, suspended_json(CP1)),
            (StatusCode::OK, pending_item_json(CP1)),
        ]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |_| {
            panic!("收到 suspended 后绝不能自动触发确认/批准");
        });
        let mut out = Vec::new();

        handle_command(&mut session, Command::Text("抓取网页".into()), &mut out)
            .await
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(CP1));
        assert!(text.contains("2026-07-23T01:00:00"));
        assert!(text.contains("fetch_web_content"));
        assert!(text.contains("/approve"));

        let saved = session.pending_approval.expect("suspended 必须保存待审批");
        assert_eq!(saved.checkpoint_id, CP1);
        assert_eq!(saved.approval.approval_id, APPROVAL);

        // 只发送了消息和详情查询,没有调用 resume
        let calls = backend_ref.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.ends_with("/api/v1/chat/messages-with-audio"));
        assert!(
            calls[1]
                .1
                .ends_with(&format!("/api/v1/chat/checkpoints/{CP1}"))
        );
    }

    #[tokio::test]
    async fn approvals_lists_pending_from_server() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            format!(r#"{{"items":[{}]}}"#, pending_item_json(CP1)),
        )]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |_| true);
        let mut out = Vec::new();

        handle_command(&mut session, Command::Approvals { limit: 20 }, &mut out)
            .await
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("待审批任务 (1)"));
        assert!(text.contains(CP1));
        let calls = backend_ref.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]
                .1
                .contains("/api/v1/chat/checkpoints/pending?limit=20")
        );
        assert_eq!(calls[0].0, Method::GET);
    }

    #[tokio::test]
    async fn approve_without_saved_pending_requires_an_explicit_id() {
        let backend = Arc::new(MockBackend::new(vec![]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |_| true);
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: None,
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("没有最近保存的待审批任务"));
        // 绝不能任意选择服务器列表第一项:不允许发起任何请求
        assert!(backend_ref.calls().is_empty());
    }

    #[tokio::test]
    async fn approve_with_id_fetches_detail_confirms_and_resumes() {
        let backend = Arc::new(MockBackend::new(vec![
            (StatusCode::OK, pending_item_json(CP1)),
            (StatusCode::OK, completed_json()),
        ]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |prompt| {
            assert!(prompt.contains("确认批准"));
            true
        });
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: Some(CP1.into()),
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已完成"));

        let calls = backend_ref.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, Method::GET);
        assert!(
            calls[0]
                .1
                .ends_with(&format!("/api/v1/chat/checkpoints/{CP1}"))
        );
        assert_eq!(calls[1].0, Method::POST);
        assert!(
            calls[1]
                .1
                .ends_with(&format!("/api/v1/chat/checkpoints/{CP1}/resume"))
        );
        let body = calls[1].2.as_deref().unwrap();
        assert!(body.contains(&format!("\"approval_id\":\"{APPROVAL}\"")));
        assert!(body.contains("\"decision\":\"approve\""));
    }

    #[tokio::test]
    async fn approve_cancelled_by_user_never_calls_resume() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            pending_item_json(CP1),
        )]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |_| false);
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: Some(CP1.into()),
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已取消"));
        assert!(text.contains("fetch_web_content"));
        let calls = backend_ref.calls();
        assert_eq!(calls.len(), 1, "取消确认后不得调用 resume");
    }

    #[tokio::test]
    async fn approve_uses_saved_pending_and_clears_it_after_completion() {
        let backend = Arc::new(MockBackend::new(vec![(StatusCode::OK, completed_json())]));
        let mut session = session(backend, |_| true);
        session.pending_approval = Some(serde_json::from_str(&pending_item_json(CP1)).unwrap());
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: None,
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已完成"));
        assert!(
            session.pending_approval.is_none(),
            "完成后必须清理待审批状态"
        );
    }

    #[tokio::test]
    async fn reject_with_saved_pending_sends_reject_decision() {
        let backend = Arc::new(MockBackend::new(vec![(StatusCode::OK, completed_json())]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |prompt| {
            assert!(prompt.contains("确认拒绝"));
            true
        });
        session.pending_approval = Some(serde_json::from_str(&pending_item_json(CP1)).unwrap());
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Reject {
                checkpoint_id: None,
            },
            &mut out,
        )
        .await
        .unwrap();

        let calls = backend_ref.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]
                .2
                .as_deref()
                .unwrap()
                .contains("\"decision\":\"reject\"")
        );
        assert!(session.pending_approval.is_none());
    }

    #[tokio::test]
    async fn resume_that_suspends_again_replaces_saved_pending() {
        let backend = Arc::new(MockBackend::new(vec![
            (StatusCode::ACCEPTED, suspended_json(CP2)),
            (StatusCode::OK, pending_item_json(CP2)),
        ]));
        let mut session = session(backend, |_| true);
        session.pending_approval = Some(serde_json::from_str(&pending_item_json(CP1)).unwrap());
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: None,
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(CP2));
        let saved = session.pending_approval.expect("再次暂停必须替换待审批");
        assert_eq!(saved.checkpoint_id, CP2);
    }

    #[tokio::test]
    async fn resume_404_shows_actionable_hint_and_clears_saved_pending() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::NOT_FOUND,
            r#"{"code":"NOT_FOUND","message":"Checkpoint 不存在或已失效"}"#.into(),
        )]));
        let mut session = session(backend, |_| true);
        session.pending_approval = Some(serde_json::from_str(&pending_item_json(CP1)).unwrap());
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: None,
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已过期、已消费或被其他恢复请求处理"));
        assert!(text.contains("/approvals"));
        assert!(session.pending_approval.is_none());
    }

    #[tokio::test]
    async fn approve_with_unknown_id_404_does_not_touch_saved_pending() {
        let backend = Arc::new(MockBackend::new(vec![(
            StatusCode::NOT_FOUND,
            r#"{"code":"NOT_FOUND","message":"Checkpoint 不存在或已失效"}"#.into(),
        )]));
        let mut session = session(backend, |_| true);
        session.pending_approval = Some(serde_json::from_str(&pending_item_json(CP1)).unwrap());
        let mut out = Vec::new();

        handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: Some(CP2.into()),
            },
            &mut out,
        )
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("已过期、已消费或被其他恢复请求处理"));
        // 404 的是另一个 ID,已保存的 CP1 待审批必须保留
        assert_eq!(
            session.pending_approval.as_ref().unwrap().checkpoint_id,
            CP1
        );
    }

    #[tokio::test]
    async fn invalid_checkpoint_id_is_rejected_before_any_request() {
        let backend = Arc::new(MockBackend::new(vec![]));
        let backend_ref = backend.clone();
        let mut session = session(backend, |_| true);
        let mut out = Vec::new();

        let result = handle_command(
            &mut session,
            Command::Approve {
                checkpoint_id: Some("not-a-uuid".into()),
            },
            &mut out,
        )
        .await;

        assert!(result.is_err());
        assert!(backend_ref.calls().is_empty());
    }
}
