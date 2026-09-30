use crate::error::{ForbiddenReason, XhlError};
use serde::de::DeserializeOwned;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct GqlEnvelope<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Vec<GqlError>,
}

#[derive(Debug, Deserialize)]
pub struct GqlError {
    pub message: String,
    #[serde(default)]
    pub code: Option<i64>,
}

pub fn unwrap_envelope<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, XhlError> {
    if status == 401 || status == 403 {
        return Err(XhlError::Forbidden {
            reason: ForbiddenReason::classify(status, body),
            detail: body.to_string(),
        });
    }

    if status >= 400 {
        return Err(XhlError::Internal(format!("HTTP status {status}: {body}")));
    }

    let env: GqlEnvelope<T> = serde_json::from_str(body).map_err(|e| {
        XhlError::Internal(format!(
            "gagal deserialize envelope GraphQL: {e}; body: {body}"
        ))
    })?;

    if let Some(err) = env.errors.first() {
        return Err(XhlError::Internal(format!(
            "GraphQL error (code {:?}): {}",
            err.code, err.message
        )));
    }

    env.data
        .ok_or_else(|| XhlError::Internal("GraphQL data kosong".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_envelope_sukses() {
        let json = r#"{"data":{"val":123},"errors":[]}"#;
        #[derive(Deserialize, PartialEq, Eq, Debug)]
        struct S {
            val: i32,
        }
        let s: S = unwrap_envelope(200, json).unwrap();
        assert_eq!(s.val, 123);
    }

    #[test]
    fn unwrap_envelope_error_graphql() {
        let json = r#"{"data":null,"errors":[{"message":"Rate limit exceeded","code":88}]}"#;
        let err = unwrap_envelope::<serde_json::Value>(200, json).unwrap_err();
        assert!(matches!(err, XhlError::Internal(msg) if msg.contains("Rate limit exceeded")));
    }

    #[test]
    fn unwrap_envelope_forbidden_csrf() {
        let err = unwrap_envelope::<serde_json::Value>(403, "Bad CSRF Token").unwrap_err();
        assert!(matches!(
            err,
            XhlError::Forbidden {
                reason: ForbiddenReason::Csrf,
                ..
            }
        ));
    }
}
