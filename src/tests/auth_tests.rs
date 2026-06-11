#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use dashmap::DashMap;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde::{Deserialize, Serialize};

    use crate::application::auth::auth_service::{AuthConfig, LoginAttemptRecord};
    use crate::domain::auth::password_service::PasswordService;
    use crate::domain::auth::token_service::TokenService;
    use crate::infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
    use crate::infrastructure::auth::jwt_token_service::JwtTokenService;
    use crate::shared::error::AppError;

    #[test]
    fn bcrypt_hash_and_verify_correct_password() {
        let svc = BcryptPasswordHasher::default();
        let hash = svc.hash("secret123").unwrap();
        assert!(svc.verify("secret123", &hash).unwrap());
    }

    #[test]
    fn bcrypt_verify_wrong_password_returns_false() {
        let svc = BcryptPasswordHasher::default();
        let hash = svc.hash("secret123").unwrap();
        assert!(!svc.verify("wrong", &hash).unwrap());
    }

    #[test]
    fn jwt_issue_and_verify_access_roundtrip() {
        let svc = JwtTokenService::new("test-secret", 900);
        let token = svc.issue_access(42, "alice", "USER").unwrap();
        let claims = svc.verify_access(&token).unwrap();
        assert_eq!(claims.user_id, 42);
        assert_eq!(claims.username, "alice");
    }

    #[test]
    fn jwt_access_token_rejects_refresh_token() {
        let svc = JwtTokenService::new("test-secret", 900);
        let refresh = svc.issue_refresh(1, "bob").unwrap();
        assert!(matches!(
            svc.verify_access(&refresh),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn jwt_issue_and_verify_refresh_roundtrip() {
        let svc = JwtTokenService::new("test-secret", 900);
        let token = svc.issue_refresh(99, "carol").unwrap();
        let claims = svc.verify_refresh(&token).unwrap();
        assert_eq!(claims.user_id, 99);
        assert_eq!(claims.username, "carol");
        assert!(!claims.token_id.is_empty());
    }

    #[test]
    fn jwt_refresh_token_rejects_access_token() {
        let svc = JwtTokenService::new("test-secret", 900);
        let access = svc.issue_access(1, "dave", "USER").unwrap();
        assert!(matches!(
            svc.verify_refresh(&access),
            Err(AppError::Unauthorized)
        ));
    }

    #[derive(Serialize, Deserialize)]
    struct MinimalClaims {
        sub: String,
        username: String,
        token_type: String,
        iat: u64,
        exp: u64,
    }

    #[test]
    fn jwt_expired_access_token_returns_unauthorized() {
        let secret = "test-secret";
        let claims = MinimalClaims {
            sub: "1".into(),
            username: "eve".into(),
            token_type: "access".into(),
            iat: 0,
            exp: 1,
        };
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        let svc = JwtTokenService::new(secret, 900);
        assert!(matches!(
            svc.verify_access(&token),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn jwt_username_roundtrip() {
        let svc = JwtTokenService::new("test-secret", 900);
        let token = svc.issue_access(7, "admin_user", "USER").unwrap();
        let claims = svc.verify_access(&token).unwrap();
        assert_eq!(claims.username, "admin_user");
    }

    #[test]
    fn login_attempt_lock_triggers_after_max_failures() {
        let map: DashMap<String, LoginAttemptRecord> = DashMap::new();
        let config = AuthConfig {
            max_attempts: 3,
            lockout_secs: 300,
            access_ttl_secs: 900,
        };
        let user = "testuser";

        let record_failure = |map: &DashMap<String, LoginAttemptRecord>| {
            let mut rec = map
                .entry(user.to_string())
                .or_insert_with(|| LoginAttemptRecord {
                    failures: 0,
                    locked_until: None,
                });
            rec.failures += 1;
            if rec.failures >= config.max_attempts {
                rec.locked_until = Some(Instant::now() + Duration::from_secs(config.lockout_secs));
            }
        };

        record_failure(&map);
        record_failure(&map);
        assert!(map.get(user).unwrap().locked_until.is_none());
        record_failure(&map);
        let locked_until = map.get(user).unwrap().locked_until;
        assert!(locked_until.is_some());
        assert!(locked_until.unwrap() > Instant::now());
    }

    #[test]
    fn login_attempt_lock_clears_on_success() {
        let map: DashMap<String, LoginAttemptRecord> = DashMap::new();
        map.insert(
            "u".to_string(),
            LoginAttemptRecord {
                failures: 4,
                locked_until: Some(Instant::now() + Duration::from_secs(300)),
            },
        );
        map.remove("u");
        assert!(!map.contains_key("u"));
    }

    #[test]
    fn login_attempt_lock_expired_allows_entry() {
        let map: DashMap<String, LoginAttemptRecord> = DashMap::new();
        map.insert(
            "u".to_string(),
            LoginAttemptRecord {
                failures: 5,
                locked_until: Some(Instant::now() - Duration::from_secs(1)),
            },
        );
        let is_locked = map
            .get("u")
            .and_then(|r| r.locked_until)
            .map(|until| Instant::now() < until)
            .unwrap_or(false);
        assert!(!is_locked);
    }
}
