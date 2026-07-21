// ServerRS CLI 客户端入口。
//
// 启动流程:加载配置 -> 恢复或登录 token -> chat/open 开启会话 -> 进入 REPL。
// CLI 模块以文件目录形式放在 src/cli/,不挂载到 lib.rs,避免与后端集成测试互相干扰。

#[path = "../cli/mod.rs"]
mod cli;

use std::io::Write;

use cli::auth;
use cli::client::ApiClient;
use cli::config::CliConfig;
use cli::error::CliError;
use cli::render;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 简单参数解析:支持 --url <url> 与 -h/--help
    let mut url_arg = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--url" => url_arg = args.next(),
            "-h" | "--help" => {
                println!(
                    "用法: cli [--url <server_url>]\n环境变量 SERVERRS_CLI_URL 也可指定服务端地址。"
                );
                return Ok(());
            }
            other => {
                eprintln!("未知参数: {other}\n用法: cli [--url <server_url>]");
                std::process::exit(1);
            }
        }
    }

    let config = CliConfig::load(url_arg).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let client = ApiClient::with_reqwest(&config, None)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // 恢复缓存 token;若无则交互登录。登录成功后 token 已写入 client 内部。
    let cached =
        auth::load_token_cache(&config).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    if let Some(token) = cached {
        // 将缓存 token 注入 client,后续请求会自动带 Bearer。
        client.set_token_external(token);
    } else {
        login_flow(&config, &client)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    }

    // 验证 token:调 /auth/me。401 且 refresh 失败则重新登录。
    let me = match client.me().await {
        Ok(u) => u,
        Err(e) if e.is_relogin_required() => {
            eprintln!("登录已过期,请重新登录");
            login_flow(&config, &client)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            client
                .me()
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        }
        Err(e) => return Err(Box::new(e) as Box<dyn std::error::Error>),
    };

    // 开启会话
    let opened = client
        .chat_open()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let mut session = cli::repl::build_session(
        client,
        Some(opened.conversation.id),
        opened.personalization_enabled,
        me.username,
    );

    println!(
        "{}",
        render::banner(
            &config.base_url,
            &session.username,
            session.personalization_enabled
        )
    );

    if let Err(e) = cli::repl::run(&mut session).await {
        if e.is_relogin_required() {
            eprintln!("登录已过期,请重新启动 cli 登录。");
        }
        return Err(Box::new(e) as Box<dyn std::error::Error>);
    }
    Ok(())
}

/// 交互登录并保存缓存。
async fn login_flow(config: &CliConfig, client: &ApiClient) -> Result<(), CliError> {
    use std::io::BufRead;
    let mut username = String::new();
    print!("用户名: ");
    std::io::stdout().flush().map_err(CliError::Io)?;
    std::io::stdin()
        .lock()
        .read_line(&mut username)
        .map_err(CliError::Io)?;
    let username = username.trim().to_string();

    // 密码输入:用 rpassword 隐藏回显。
    let password = rpassword::prompt_password("密码: ")
        .map_err(|e| CliError::Auth(format!("读取密码失败: {e}")))?;

    let cache = client.login(&username, &password).await?;
    auth::save_token_cache(config, &cache)?;
    Ok(())
}
