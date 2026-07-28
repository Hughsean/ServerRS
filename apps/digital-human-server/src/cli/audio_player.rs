//! 本机音频播放边界。
//!
//! 音频地址是短时签名能力链接，禁止输出或持久化；此模块仅校验后交给系统默认关联程序。

use std::process::Command;

use reqwest::Url;

/// 播放器抽象，使命令流程可在不启动外部程序的情况下测试。
pub trait AudioPlayer: Send + Sync {
    fn play(&self, audio_url: &str) -> Result<(), String>;
}

/// 使用系统默认关联程序播放远程音频。
pub struct SystemAudioPlayer;

impl AudioPlayer for SystemAudioPlayer {
    fn play(&self, audio_url: &str) -> Result<(), String> {
        let command = opener_command_for(std::env::consts::OS, audio_url)?;
        command
            .spawn()
            .map(|_| ())
            .map_err(|_| "无法启动本机音频播放器".into())
    }
}

struct OpenerCommand {
    program: &'static str,
    args: Vec<String>,
}

impl OpenerCommand {
    fn spawn(&self) -> std::io::Result<std::process::Child> {
        Command::new(self.program).args(&self.args).spawn()
    }
}

fn opener_command_for(os: &str, audio_url: &str) -> Result<OpenerCommand, String> {
    validate_audio_url(audio_url)?;
    let args = match os {
        // `cmd /C` 会将 URL 查询串中的 `&` 解释成命令分隔符。`start` 接受
        // 空标题占位，直接把 URL 作为独立参数交给 cmd，避免手工拼接命令字符串。
        "windows" => vec![
            String::from("/C"),
            String::from("start"),
            String::from(""),
            format!("\"{audio_url}\""),
        ],
        "macos" => vec![String::from(audio_url)],
        _ => vec![String::from(audio_url)],
    };
    let program = match os {
        "windows" => "cmd",
        "macos" => "open",
        _ => "xdg-open",
    };
    Ok(OpenerCommand { program, args })
}

fn validate_audio_url(audio_url: &str) -> Result<(), String> {
    let url = Url::parse(audio_url).map_err(|_| "音频地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err("音频地址无效".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_audio_urls_without_credentials() {
        assert!(validate_audio_url("https://example.com/audio?signature=a").is_ok());
        assert!(validate_audio_url("http://127.0.0.1:8080/audio").is_ok());
    }

    #[test]
    fn rejects_unsafe_or_malformed_audio_urls() {
        for url in [
            "file:///tmp/audio.wav",
            "https://user@example.com/audio",
            "https://user:secret@example.com/audio",
            "not a url",
        ] {
            assert!(validate_audio_url(url).is_err(), "应拒绝 {url}");
        }
    }

    #[test]
    fn builds_platform_specific_opener_arguments() {
        let url = "https://example.com/audio?expires=1&signature=x";
        let windows = opener_command_for("windows", url).unwrap();
        assert_eq!(windows.program, "cmd");
        assert_eq!(
            windows.args,
            vec![
                String::from("/C"),
                String::from("start"),
                String::from(""),
                format!("\"{url}\""),
            ]
        );

        let macos = opener_command_for("macos", url).unwrap();
        assert_eq!(macos.program, "open");
        assert_eq!(macos.args, vec![String::from(url)]);

        let linux = opener_command_for("linux", url).unwrap();
        assert_eq!(linux.program, "xdg-open");
        assert_eq!(linux.args, vec![String::from(url)]);
    }
}
