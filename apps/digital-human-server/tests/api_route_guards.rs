//! 路由安全守卫：待审批收件箱端点必须与既有 Chat 端点一样，
//! 注册在 `require_bearer_auth` 保护的路由组内，且挂在 Checkpoint 子路由下，
//! 不与 `{checkpoint_id}/resume` 发生冲突。

use std::fs;
use std::path::Path;

fn router_source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/router.rs"))
        .expect("router.rs must be readable")
}

/// 返回 `(protected 路由组源码段, 其余源码)`。
fn split_protected(source: &str) -> (&str, String) {
    let start = source
        .find("let protected = Router::new()")
        .expect("protected router must exist");
    let end = source[start..]
        .find("route_layer(middleware::from_fn_with_state")
        .map(|offset| start + offset)
        .expect("bearer auth layer must exist");
    let rest = format!("{}{}", &source[..start], &source[end..]);
    (&source[start..end], rest)
}

#[test]
fn pending_approval_routes_are_registered_under_bearer_protection() {
    let source = router_source();
    let (protected, _) = split_protected(&source);

    for needle in [
        "\"/api/v1/chat/checkpoints/pending\"",
        "get(chat_list_pending_approvals)",
        "\"/api/v1/chat/checkpoints/{checkpoint_id}\"",
        "get(chat_get_checkpoint)",
    ] {
        assert!(protected.contains(needle), "受保护路由组必须注册 {needle}");
    }
}

#[test]
fn checkpoint_routes_do_not_conflict_with_resume_subroute() {
    let source = router_source();
    let (protected, rest) = split_protected(&source);

    // resume 子路由必须仍然存在且只接受 POST
    assert!(
        protected.contains("\"/api/v1/chat/checkpoints/{checkpoint_id}/resume\""),
        "resume 子路由必须保持兼容"
    );
    assert!(
        protected.contains("post(chat_resume_checkpoint)"),
        "resume 处理器必须保持兼容"
    );

    // 待审批端点的注册形式不得出现在 protected 路由组之外
    assert!(
        !rest.contains("get(chat_list_pending_approvals)"),
        "待审批列表不得注册到 protected 之外"
    );
    assert!(
        !rest.contains("get(chat_get_checkpoint)"),
        "待审批详情不得注册到 protected 之外"
    );
}
