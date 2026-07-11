//! 斜杠命令分发与处理。
//!
//! Session 是 REPL 贯穿全程的状态,定义在此处供 repl 与 commands 共享。

use std::io::Write;
use std::sync::Arc;

use crate::cli::client::ApiClient;
use crate::cli::error::CliError;
use crate::cli::render;

/// REPL 全局状态。
pub struct Session {
    pub client: Arc<ApiClient>,
    pub conversation_id: Option<u64>,
    pub personalization_enabled: bool,
    pub username: String,
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
            let resp = session.client.chat_send(&text).await?;
            session.conversation_id = Some(resp.conversation_id);
            writeln!(
                out,
                "{}",
                render::assistant_reply(&resp.reply, &resp.tool_calls)
            )?;
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
            if !confirm(out, "将遗忘全部对话/记忆/画像,不可逆。确认? (y/N): ")? {
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
            if !confirm(out, "将重置个性化(关闭+失效画像)。确认? (y/N): ")? {
                writeln!(out, "已取消")?;
                return Ok(false);
            }
            session.client.persona_reset().await?;
            session.personalization_enabled = false;
            writeln!(out, "已重置个性化")?;
        }
        Command::Unknown(name) => {
            writeln!(out, "未知命令 {name},输入 /help 查看可用命令")?;
        }
    }
    Ok(false)
}

/// 交互式二次确认。从 stdin 读一行,仅 "y"/"Y" 为确认。
fn confirm<W: Write>(out: &mut W, prompt: &str) -> Result<bool, CliError> {
    use std::io::BufRead;
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
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
}
