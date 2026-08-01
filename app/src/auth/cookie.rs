use cookie::Key;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AppError;

pub const COOKIE_NAME: &str = "vanyline_token";

pub fn extract_token(cookie_header: Option<&str>, key: &Key) -> Result<(String, String), AppError> {
    let cookie_header = cookie_header.ok_or(AppError::NotAuthenticated)?;

    let cookie_value = cookie_header
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .find_map(|c| {
            c.split_once('=').and_then(|(name, value)| {
                if name == COOKIE_NAME {
                    Some(value.to_string())
                } else {
                    None
                }
            })
        })
        .ok_or(AppError::NotAuthenticated)?;

    let jar = cookie::CookieJar::new();
    let private_jar = jar.private(key);
    let raw_cookie = cookie::Cookie::new(COOKIE_NAME, cookie_value);
    let decrypted = private_jar
        .decrypt(raw_cookie)
        .ok_or(AppError::InvalidToken)?;

    let value = decrypted.value();
    let parts: Vec<&str> = value.splitn(2, '|').collect();
    if parts.len() != 2 {
        return Err(AppError::InvalidToken);
    }

    let id_token = parts[0].to_string();
    let email = parts[1].to_string();

    let exp = extract_exp_claim(&id_token).ok_or(AppError::InvalidToken)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::InvalidToken)?
        .as_secs();

    if exp <= now {
        return Err(AppError::InvalidToken);
    }

    Ok((id_token, email))
}

fn extract_exp_claim(jwt: &str) -> Option<u64> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    use base64::Engine;
    let payload_json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let payload_str = String::from_utf8(payload_json).ok()?;
    let exp_key = "\"exp\":";
    let start = payload_str.find(exp_key)?.saturating_add(exp_key.len());
    let rest = &payload_str[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

pub fn build_set_cookie(id_token: &str, email: &str, key: &Key) -> String {
    let value = format!("{}|{}", id_token, email);
    let exp = extract_exp_claim(id_token).unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let max_age = exp.saturating_sub(now);

    let mut jar = cookie::CookieJar::new();
    let mut private_jar = jar.private_mut(key);
    private_jar.add(cookie::Cookie::new(COOKIE_NAME, value));

    let encrypted = jar.get(COOKIE_NAME);
    let encrypted_value = encrypted.map(|c| c.value()).unwrap_or("");

    format!(
        "{}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        COOKIE_NAME, encrypted_value, max_age
    )
}

pub fn clear_cookie() -> String {
    format!(
        "{}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0",
        COOKIE_NAME
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use base64::Engine;

    fn test_key() -> Key {
        Key::from(&[0u8; 64])
    }

    fn make_jwt(exp: u64) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"exp":{exp},"email":"test@example.com"}}"#));
        format!("header.{payload}.sig")
    }

    #[test]
    fn build_and_extract_valid_token() {
        let key = test_key();
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let jwt = make_jwt(exp);
        let set_cookie = build_set_cookie(&jwt, "test@example.com", &key);
        let cookie_value = set_cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{COOKIE_NAME}="));

        let result = extract_token(Some(&format!("{COOKIE_NAME}={cookie_value}")), &key);
        assert!(result.is_ok());
        let (token, email) = result.unwrap();
        assert_eq!(email, "test@example.com");
        assert_eq!(token, jwt);
    }

    #[test]
    fn expired_token_returns_error() {
        let key = test_key();
        let jwt = make_jwt(1_000_000);
        let set_cookie = build_set_cookie(&jwt, "test@example.com", &key);
        let cookie_value = set_cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{COOKIE_NAME}="));

        let result = extract_token(Some(&format!("{COOKIE_NAME}={cookie_value}")), &key);
        assert!(matches!(result, Err(AppError::InvalidToken)));
    }

    #[test]
    fn missing_cookie_returns_unauthenticated() {
        let key = test_key();
        assert!(matches!(
            extract_token(None, &key),
            Err(AppError::NotAuthenticated)
        ));
    }

    #[test]
    fn clear_cookie_has_max_age_zero() {
        let cleared = clear_cookie();
        assert!(cleared.contains("Max-Age=0"));
        assert!(cleared.contains(COOKIE_NAME));
    }
}
