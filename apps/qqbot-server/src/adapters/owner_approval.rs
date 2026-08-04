//! Owner 审批命令的确定性、无副作用解析。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalCommand {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedApprovalCommand {
    pub command: ApprovalCommand,
    pub proposal_short_id: Option<String>,
}

pub fn parse_owner_approval_command(input: &str) -> Option<ParsedApprovalCommand> {
    let mut parts = input.split_whitespace();
    let command = match parts.next()? {
        "确认" | "批准" => ApprovalCommand::Approve,
        "拒绝" | "不执行" => ApprovalCommand::Reject,
        _ => return None,
    };
    let proposal_short_id = parts.next().map(str::to_owned);
    if parts.next().is_some()
        || proposal_short_id
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 36)
    {
        return None;
    }
    Some(ParsedApprovalCommand {
        command,
        proposal_short_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_approve_and_reject_with_optional_short_id() {
        assert_eq!(
            parse_owner_approval_command("确认 abcd"),
            Some(ParsedApprovalCommand {
                command: ApprovalCommand::Approve,
                proposal_short_id: Some("abcd".into()),
            })
        );
        assert_eq!(
            parse_owner_approval_command("不执行"),
            Some(ParsedApprovalCommand {
                command: ApprovalCommand::Reject,
                proposal_short_id: None,
            })
        );
    }
}
