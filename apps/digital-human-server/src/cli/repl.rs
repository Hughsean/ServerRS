//! REPL 主循环:rustyline 读行 + 异步命令处理。
//!
//! rustyline::readline 是阻塞调用,在 tokio runtime 里用
//! spawn_blocking 包装,避免阻塞 reactor。命令处理仍在 async 上下文。
//! 提示符通过 channel 在每次读行前传给读行线程,使其能动态显示个性化状态。

use std::sync::Arc;

use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, Result as RlResult};

use crate::cli::client::ApiClient;
use crate::cli::commands::{self, Session};
use crate::cli::error::CliError;
use crate::cli::render;

/// 运行 REPL。返回当用户退出或 EOF。
pub async fn run(session: &mut Session) -> Result<(), CliError> {
    // 读行线程接收当前提示符,返回读到的行。
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<RlResult<Option<String>>>(8);
    // 提示符通道:主循环 -> 读行线程,每次读行前发送当前提示符。
    let (prompt_tx, prompt_rx) = std::sync::mpsc::channel::<String>();

    // 读行线程:阻塞读 rustyline,提示符从 prompt_rx 取。
    tokio::task::spawn_blocking(move || -> RlResult<()> {
        let mut rl = DefaultEditor::new()?;
        while let Ok(prompt) = prompt_rx.recv() {
            let line = rl.readline(&prompt);
            match &line {
                Ok(l) => {
                    if !l.trim().is_empty() {
                        let _ = rl.add_history_entry(l);
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C: 继续读下一行,不退出
                }
                Err(ReadlineError::Eof) => {
                    let _ = line_tx.blocking_send(Ok(None));
                    break;
                }
                Err(_) => {
                    let _ = line_tx.blocking_send(Ok(None));
                    break;
                }
            }
            if line_tx.blocking_send(line.map(Some)).is_err() {
                break;
            }
        }
        Ok(())
    });

    let mut out = std::io::stdout();

    loop {
        // 发送当前提示符给读行线程
        let prompt = render::prompt(session.personalization_enabled);
        if prompt_tx.send(prompt).is_err() {
            break; // 读行线程已退出
        }
        let line_result = match line_rx.recv().await {
            Some(r) => r,
            None => break,
        };
        let line = match line_result {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(ReadlineError::Interrupted) => {
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("输入错误: {e}");
                break;
            }
        };

        let cmd = commands::parse(&line);
        if matches!(cmd, commands::Command::Text(ref t) if t.is_empty()) {
            continue;
        }
        match commands::handle_command(session, cmd, &mut out).await {
            Ok(quit) => {
                if quit {
                    break;
                }
            }
            Err(e) => {
                if e.is_relogin_required() {
                    eprintln!("登录已过期,请重新登录");
                    return Err(e);
                }
                eprintln!("{e}");
            }
        }
    }
    Ok(())
}

/// 构造 Session 的便捷方法。
pub fn build_session(
    client: ApiClient,
    conversation_id: Option<u64>,
    personalization_enabled: bool,
    username: String,
) -> Session {
    Session {
        client: Arc::new(client),
        conversation_id,
        personalization_enabled,
        username,
        pending_approval: None,
        confirm: Arc::new(commands::stdin_confirm),
        audio_player: Arc::new(crate::cli::audio_player::SystemAudioPlayer),
    }
}
