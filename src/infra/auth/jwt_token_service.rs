use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::auth::token_service::{
    AccessTokenClaims, RefreshTokenClaims, SignatureClaims, TokenService,
};
use crate::shared::error::AppError;

const DEFAULT_JWT_SECRET: &str = "dev-secret-change-in-production";
const DEFAULT_ACCESS_TTL_SECONDS: u64 = 15 * 60;
const DEFAULT_REFRESH_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const TOKEN_TYPE_ACCESS: &str = "access";
const TOKEN_TYPE_REFRESH: &str = "refresh";

#[derive(Debug, Clone)]
pub struct JwtTokenService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    access_ttl_seconds: u64,
    refresh_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JwtClaims {
    sub: String,
    username: String,
    role: String,
    token_type: String,
    jti: Option<String>,
    iat: u64,
    exp: u64,
}

/// 第三方签名 JWT 的声明结构（使用调用方提供的 appKey 作为 HMAC 密钥）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignatureJwtClaims {
    #[serde(rename = "appId")]
    app_id: String,
    iat: i64,
    exp: i64,
}

impl JwtTokenService {
    pub fn new(secret: &str, access_ttl_seconds: u64) -> Self {
        let refresh_ttl_seconds = access_ttl_seconds * 7; // refresh Token 有效期是 access 的 7 倍
        Self::new_with_ttls(secret, access_ttl_seconds, refresh_ttl_seconds)
    }

    pub fn new_with_ttls(secret: &str, access_ttl_seconds: u64, refresh_ttl_seconds: u64) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            access_ttl_seconds,
            refresh_ttl_seconds,
        }
    }

    pub fn from_env() -> Self {
        let secret = std::env::var("APP_JWT_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_JWT_SECRET.to_string());

        let access_ttl_seconds = std::env::var("APP_JWT_ACCESS_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_ACCESS_TTL_SECONDS);

        let refresh_ttl_seconds = std::env::var("APP_JWT_REFRESH_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_REFRESH_TTL_SECONDS);

        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            access_ttl_seconds,
            refresh_ttl_seconds,
        }
    }

    fn now_seconds() -> Result<u64, AppError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|err| AppError::internal(format!("system clock error: {err}")))
    }

    fn decode_claims(&self, token: &str) -> Result<JwtClaims, AppError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        let token_data = decode::<JwtClaims>(token, &self.decoding_key, &validation)
            .map_err(|_| AppError::Unauthorized)?;
        Ok(token_data.claims)
    }
}

impl TokenService for JwtTokenService {
    fn issue_access(&self, user_id: u64, username: &str, role: &str) -> Result<String, AppError> {
        let now = Self::now_seconds()?;
        let claims = JwtClaims {
            sub: user_id.to_string(),
            username: username.to_string(),
            role: role.to_string(),
            token_type: TOKEN_TYPE_ACCESS.to_string(),
            jti: None,
            iat: now,
            exp: now.saturating_add(self.access_ttl_seconds),
        };

        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)
            .map_err(|err| AppError::internal(format!("failed to issue access token: {err}")))
    }

    fn verify_access(&self, token: &str) -> Result<AccessTokenClaims, AppError> {
        let claims = self.decode_claims(token)?;

        if claims.token_type != TOKEN_TYPE_ACCESS {
            return Err(AppError::Unauthorized);
        }

        let user_id = claims
            .sub
            .parse::<u64>()
            .map_err(|_| AppError::Unauthorized)?;

        Ok(AccessTokenClaims {
            user_id,
            username: claims.username,
            role: claims.role,
        })
    }

    fn issue_refresh(&self, user_id: u64, username: &str) -> Result<String, AppError> {
        let now = Self::now_seconds()?;
        let claims = JwtClaims {
            sub: user_id.to_string(),
            username: username.to_string(),
            role: String::new(), // not needed for refresh tokens
            token_type: TOKEN_TYPE_REFRESH.to_string(),
            jti: Some(Uuid::new_v4().to_string()),
            iat: now,
            exp: now.saturating_add(self.refresh_ttl_seconds),
        };

        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding_key)
            .map_err(|err| AppError::internal(format!("failed to issue refresh token: {err}")))
    }

    fn verify_refresh(&self, refresh_token: &str) -> Result<RefreshTokenClaims, AppError> {
        let claims = self.decode_claims(refresh_token)?;

        if claims.token_type != TOKEN_TYPE_REFRESH {
            return Err(AppError::Unauthorized);
        }

        let token_id = claims.jti.ok_or(AppError::Unauthorized)?;
        let user_id = claims
            .sub
            .parse::<u64>()
            .map_err(|_| AppError::Unauthorized)?;

        Ok(RefreshTokenClaims {
            user_id,
            username: claims.username,
            token_id,
            expires_at: claims.exp,
        })
    }

    // ── 第三方签名 ──

    fn create_signature(
        &self,
        app_id: &str,
        app_key: &str,
        expires_in_seconds: i64,
    ) -> Result<String, AppError> {
        if expires_in_seconds <= 0 || expires_in_seconds > 86400 {
            return Err(AppError::Validation(
                "expires_in_seconds must be between 1 and 86400".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let exp = now.saturating_add(expires_in_seconds);

        let claims = SignatureJwtClaims {
            app_id: app_id.to_string(),
            iat: now,
            exp,
        };

        // 使用调用方提供的 appKey 作为 HMAC 密钥
        let encoding_key = EncodingKey::from_secret(app_key.as_bytes());
        encode(&Header::new(Algorithm::HS256), &claims, &encoding_key)
            .map_err(|err| AppError::internal(format!("failed to create signature: {err}")))
    }

    fn verify_signature(&self, token: &str, app_key: &str) -> Result<SignatureClaims, AppError> {
        let decoding_key = DecodingKey::from_secret(app_key.as_bytes());
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        match decode::<SignatureJwtClaims>(token, &decoding_key, &validation) {
            Ok(data) => {
                let now = chrono::Utc::now().timestamp();
                Ok(SignatureClaims {
                    valid: data.claims.exp > now,
                    app_id: Some(data.claims.app_id),
                    issued_at: Some(data.claims.iat),
                    expires_at: Some(data.claims.exp),
                })
            }
            Err(_) => Ok(SignatureClaims {
                valid: false,
                app_id: None,
                issued_at: None,
                expires_at: None,
            }),
        }
    }
}

#[test]
fn t() {
    println!(
        "Utc: {}, Local: {}",
        chrono::Utc::now().timestamp(),
        chrono::Local::now().timestamp()
    )
}
