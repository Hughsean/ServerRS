//! 终端渲染:纯格式化函数,无 IO 副作用,便于单元测试。

use crate::cli::dto::*;

/// 分隔线长度。
const SEP: &str = "────────────────────────────";

/// 助手回复渲染。无工具调用时不显示工具行。
pub fn assistant_reply(reply: &str, tool_calls: &[ChatToolCallItem]) -> String {
    let mut s = format!("{SEP}\n{reply}\n");
    if !tool_calls.is_empty() {
        s.push_str(&format!("\n{}\n", tool_calls_line(tool_calls)));
    }
    s.push_str(SEP);
    s
}

/// 工具调用行,如 `[工具调用: get_weather(合肥)]`。
pub fn tool_calls_line(tool_calls: &[ChatToolCallItem]) -> String {
    tool_calls
        .iter()
        .map(|tc| {
            let args = if tc.arguments.is_null() {
                String::new()
            } else {
                // 简化:取字符串值或原始 JSON
                tc.arguments
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| tc.arguments.to_string())
            };
            format!("[工具调用: {}({})]", tc.name, args)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 历史消息渲染。content 为后端的 JSON 结构,提取 text 字段。
pub fn history(messages: &[ChatMessageItem]) -> String {
    messages
        .iter()
        .map(|m| {
            let role = match m.sender_role.as_str() {
                "user" => "你",
                "assistant" => "助手",
                other => other,
            };
            let text = extract_message_text(&m.content);
            // 时间取前 19 位(去掉亚秒),如 2026-07-11T14:30:00
            let time = m.created_at.chars().take(19).collect::<String>();
            format!("[{time}] {role}: {text}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 从消息 content JSON 提取文本。
fn extract_message_text(content: &serde_json::Value) -> String {
    content
        .get("text")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| content.to_string())
}

/// 记忆表格渲染。
pub fn memories_table(memories: &[ChatMemoryItem], total_active: usize) -> String {
    let mut s = String::from("ID    类型          置信度  强化  内容\n");
    for m in memories {
        s.push_str(&format!(
            "{:<5} {:<12} {:<6.2} {:<5} {}\n",
            m.memory_id, m.memory_type, m.confidence, m.reinforce_count, m.content
        ));
    }
    s.push_str(&format!("共 {} 条活跃记忆", total_active));
    s
}

/// 画像快照渲染。
pub fn persona(resp: &ChatPersonaResponse) -> String {
    if !resp.has_active_persona {
        return format!(
            "画像状态: {}\n暂无活跃画像快照,可输入 /rebuild 构建",
            if resp.personalization_enabled {
                "已启用"
            } else {
                "未启用"
            }
        );
    }
    let generated = resp
        .generated_at
        .as_deref()
        .map(|t| format!("生成于 {}", t.chars().take(19).collect::<String>()))
        .unwrap_or_default();
    let sum = &resp.snapshot_summary;
    format!(
        "画像状态: {} ({})\n沟通偏好: {} 条 | 稳定事实: {} 条 | 重复话题: {} 条 | 目标: {} 条 | 敏感上下文: {} 条",
        if resp.personalization_enabled {
            "已启用"
        } else {
            "未启用"
        },
        generated,
        sum.communication_preferences_count,
        sum.stable_facts_count,
        sum.recurring_topics_count,
        sum.goals_count,
        sum.sensitive_context_count,
    )
}

/// 手填画像渲染。空字段显示"未设置"。
pub fn user_profile(p: &UserProfileResponse) -> String {
    let items = [
        ("兴趣爱好", &p.interests),
        ("性格特征", &p.personality_traits),
        ("交互偏好", &p.interaction_preferences),
        ("情绪倾向", &p.emotional_tendency),
        ("学习记录", &p.learning_records),
    ];
    items
        .iter()
        .map(|(label, field)| {
            let val = field
                .as_ref()
                .filter(|v| !v.is_empty())
                .map(|v| v.join(", "))
                .unwrap_or_else(|| "(未设置)".into());
            format!("{label}: {val}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 启动横幅。
pub fn banner(base_url: &str, username: &str, personalization_enabled: bool) -> String {
    format!(
        "ServerRS CLI 客户端  (连接 {base_url})\n已登录: {username}  | 个性化: {}\n输入 /help 查看命令,Ctrl+D 或 /quit 退出",
        if personalization_enabled {
            "开"
        } else {
            "关"
        }
    )
}

// ── 工具审批收件箱 ──

/// 收到 202 suspended 时的提示。不自动批准,只展示信息并引导用户决策。
pub fn suspended_notice(item: &PendingApprovalItem) -> String {
    format!(
        "{SEP}\n⚠ 助手请求执行受控工具,运行已暂停,等待你的审批\n{}\n使用 /approve 批准执行,或 /reject 拒绝。\n{SEP}",
        approval_summary(item)
    )
}

/// 待审批任务列表。
pub fn approvals_list(items: &[PendingApprovalItem]) -> String {
    if items.is_empty() {
        return "没有待审批的任务。".into();
    }
    let mut s = format!("待审批任务 ({}):\n", items.len());
    for item in items {
        let tools = item
            .approval
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!(
            "  {} 工具: {} | 过期: {}\n",
            item.checkpoint_id,
            tools,
            short_time(&item.expires_at)
        ));
    }
    s.push_str("使用 /approve <checkpoint_id> 或 /reject <checkpoint_id> 处理。");
    s
}

/// 单个待审批任务的工具摘要(批准/拒绝前展示)。
pub fn approval_summary(item: &PendingApprovalItem) -> String {
    let mut s = format!(
        "Checkpoint: {}\n审批提示: {}\n工具调用:",
        item.checkpoint_id, item.approval.prompt
    );
    if item.approval.tool_calls.is_empty() {
        s.push_str("\n  (无)");
    }
    for call in &item.approval.tool_calls {
        let args = if call.arguments.is_null() {
            String::new()
        } else {
            call.arguments
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| call.arguments.to_string())
        };
        s.push_str(&format!("\n  - {}({})", call.name, args));
    }
    s.push_str(&format!("\n过期时间: {}", short_time(&item.expires_at)));
    s
}

/// RFC3339 时间取前 19 位;空串(202 响应降级)显示未知。
fn short_time(value: &str) -> String {
    if value.is_empty() {
        return "(未知)".into();
    }
    value.chars().take(19).collect()
}

/// 提示符。
pub fn prompt(personalization_enabled: bool) -> String {
    format!(
        "digital-companion [个性化:{}] > ",
        if personalization_enabled {
            "开"
        } else {
            "关"
        }
    )
}

/// /help 输出。
pub fn help() -> String {
    "\
可用命令:
  /help                显示本帮助
  /quit /exit          退出
  /history [limit]     拉取历史消息(默认 20)
  /clear               清空当前会话转写(保留记忆和画像)
  /reopen              重新开启会话(获取新 conversation_id)
  /forget              遗忘全部数据(对话+记忆+画像,需确认)
  /memories [type] [limit]   查询记忆(type: preference/fact/emotional_pattern/goal)
  /persona             查询画像快照
  /profile             查询手填画像
  /rebuild             强制重建画像快照并开启个性化
  /reset               重置个性化(需确认)
  /approvals [limit]   查询当前待审批的工具任务(默认 20)
  /approve [checkpoint_id]  批准并恢复待审批任务(需确认;缺省用最近一次暂停)
  /reject [checkpoint_id]   拒绝并恢复待审批任务(需确认;缺省用最近一次暂停)
直接输入文本即可与助手对话。"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(name: &str, args: &str) -> ChatToolCallItem {
        ChatToolCallItem {
            name: name.into(),
            arguments: serde_json::Value::String(args.into()),
        }
    }

    #[test]
    fn assistant_reply_without_tool_calls() {
        let s = assistant_reply("你好", &[]);
        assert!(s.contains("你好"));
        assert!(!s.contains("工具调用"));
    }

    #[test]
    fn assistant_reply_with_tool_calls() {
        let s = assistant_reply("查到了", &[tc("get_weather", "合肥")]);
        assert!(s.contains("[工具调用: get_weather(合肥)]"));
    }

    #[test]
    fn history_extracts_text_from_json() {
        let m = ChatMessageItem {
            id: 1,
            sender_role: "user".into(),
            message_type: "text".into(),
            content: serde_json::json!({"text": "今天好吗"}),
            created_at: "2026-07-11T14:30:00.000Z".into(),
        };
        let s = history(&[m]);
        assert!(s.contains("[2026-07-11T14:30:00] 你: 今天好吗"));
    }

    #[test]
    fn memories_table_shows_counts() {
        let m = ChatMemoryItem {
            memory_id: 7,
            memory_type: "fact".into(),
            content: "用户姓名=Alice".into(),
            confidence: 0.95,
            reinforce_count: 3,
            created_at: "t".into(),
            reinforced_at: None,
        };
        let s = memories_table(&[m], 5);
        assert!(s.contains("fact"));
        assert!(s.contains("用户姓名=Alice"));
        assert!(s.contains("共 5 条活跃记忆"));
    }

    #[test]
    fn persona_inactive_prompts_rebuild() {
        let resp = ChatPersonaResponse {
            has_active_persona: false,
            generated_at: None,
            snapshot_summary: ChatPersonaSnapshotSummary::default(),
            personalization_enabled: false,
        };
        let s = persona(&resp);
        assert!(s.contains("/rebuild"));
    }

    #[test]
    fn persona_active_shows_counts() {
        let resp = ChatPersonaResponse {
            has_active_persona: true,
            generated_at: Some("2026-07-11T14:00:00Z".into()),
            snapshot_summary: ChatPersonaSnapshotSummary {
                communication_preferences_count: 3,
                stable_facts_count: 2,
                recurring_topics_count: 1,
                goals_count: 1,
                sensitive_context_count: 0,
            },
            personalization_enabled: true,
        };
        let s = persona(&resp);
        assert!(s.contains("已启用"));
        assert!(s.contains("稳定事实: 2 条"));
    }

    #[test]
    fn user_profile_shows_unset_for_empty() {
        let p = UserProfileResponse {
            id: 1,
            user_id: 1,
            interests: Some(vec!["编程".into()]),
            personality_traits: None,
            interaction_preferences: Some(vec![]),
            emotional_tendency: None,
            learning_records: None,
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let s = user_profile(&p);
        assert!(s.contains("兴趣爱好: 编程"));
        assert!(s.contains("性格特征: (未设置)"));
        // 空数组也算未设置
        assert!(s.contains("交互偏好: (未设置)"));
    }

    #[test]
    fn banner_and_prompt() {
        assert!(banner("http://x", "alice", true).contains("个性化: 开"));
        assert!(prompt(false).contains("个性化:关"));
    }

    #[test]
    fn help_lists_all_commands() {
        let h = help();
        for cmd in [
            "/help",
            "/quit",
            "/history",
            "/clear",
            "/reopen",
            "/forget",
            "/memories",
            "/persona",
            "/profile",
            "/rebuild",
            "/reset",
            "/approvals",
            "/approve",
            "/reject",
        ] {
            assert!(h.contains(cmd), "help 缺少命令 {cmd}");
        }
    }

    fn pending_item(checkpoint_id: &str, tool: &str) -> PendingApprovalItem {
        PendingApprovalItem {
            status: "pending".into(),
            checkpoint_id: checkpoint_id.into(),
            run_id: "90b4891f-cf68-4c1a-ad83-32d9d8494d18".into(),
            conversation_id: 9,
            reason: "approval".into(),
            created_at: "2026-07-22T01:00:00+00:00".into(),
            expires_at: "2026-07-23T01:00:00+00:00".into(),
            approval: ChatToolApprovalInfo {
                approval_id: "02f941ab-0fb8-4c44-999c-9ff896ef415a".into(),
                prompt: "模型请求执行受控工具，请确认是否允许。".into(),
                tool_calls: vec![ChatApprovalToolCallItem {
                    id: "call-1".into(),
                    name: tool.into(),
                    arguments: serde_json::json!({"url": "https://example.com"}),
                }],
            },
        }
    }

    #[test]
    fn suspended_notice_shows_checkpoint_tool_and_decision_hint() {
        let item = pending_item("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760", "fetch_web_content");
        let s = suspended_notice(&item);

        assert!(s.contains("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760"));
        assert!(s.contains("fetch_web_content"));
        assert!(s.contains("2026-07-23T01:00:00"));
        assert!(s.contains("/approve"));
        assert!(s.contains("/reject"));
        assert!(s.contains("等待你的审批"));
    }

    #[test]
    fn approvals_list_renders_items_and_empty_state() {
        assert!(approvals_list(&[]).contains("没有待审批"));

        let items = vec![
            pending_item("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760", "fetch_web_content"),
            pending_item("aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb", "web_search"),
        ];
        let s = approvals_list(&items);
        assert!(s.contains("待审批任务 (2)"));
        assert!(s.contains("fetch_web_content"));
        assert!(s.contains("web_search"));
        assert!(s.contains("/approve <checkpoint_id>"));
    }

    #[test]
    fn approval_summary_formats_tool_arguments() {
        let item = pending_item("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760", "fetch_web_content");
        let s = approval_summary(&item);

        assert!(s.contains("模型请求执行受控工具"));
        assert!(s.contains(r#"fetch_web_content({"url":"https://example.com"})"#));
        assert!(s.contains("过期时间: 2026-07-23T01:00:00"));
    }

    #[test]
    fn approval_summary_handles_unknown_expiry() {
        let mut item = pending_item("2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760", "fetch_web_content");
        item.expires_at = String::new();
        let s = approval_summary(&item);

        assert!(s.contains("过期时间: (未知)"));
    }
}
