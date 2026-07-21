use bcrypt::verify;

use crate::domain::user::user::User;
use crate::repositories::RepositorySet;

pub const TEST_USERNAME: &str = "test";
pub const TEST_PASSWORD: &str = "123123123";

pub async fn load_test_user(repos: &RepositorySet) -> User {
    let user = repos
        .user_repo
        .find_by_username(TEST_USERNAME)
        .await
        .unwrap_or_else(|error| panic!("读取测试用户失败: {error}"))
        .unwrap_or_else(|| panic!("测试用户不存在: username={TEST_USERNAME}"));
    assert!(user.is_active(), "测试用户必须是 active 状态");

    let password_hash = user
        .password_hash
        .as_deref()
        .unwrap_or_else(|| panic!("测试用户 {TEST_USERNAME} 没有密码哈希"));
    assert!(
        verify(TEST_PASSWORD, password_hash).unwrap_or(false),
        "测试用户密码必须匹配固定密码 {TEST_PASSWORD}"
    );
    user
}
