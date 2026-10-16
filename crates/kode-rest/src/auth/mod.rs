//! Runtime authentication parameters are resolved once and never serialized.
pub(crate) mod challenge;
mod signing;
use kode_core::{DomainError, Result};
pub use signing::sign;
use std::collections::BTreeMap;

pub struct Scheme {
    pub kind: &'static str,
    pub label: &'static str,
    pub fields: &'static [(&'static str, &'static str)],
}
pub static SCHEMES: &[Scheme] = &[
    Scheme {
        kind: "noauth",
        label: "No auth",
        fields: &[],
    },
    Scheme {
        kind: "apikey",
        label: "API key",
        fields: &[
            ("key", "X-API-Key"),
            ("value", "{{apiKey}}"),
            ("in", "header"),
        ],
    },
    Scheme {
        kind: "bearer",
        label: "Bearer",
        fields: &[("token", "{{bearerToken}}")],
    },
    Scheme {
        kind: "basic",
        label: "Basic",
        fields: &[("username", "{{username}}"), ("password", "{{password}}")],
    },
    Scheme {
        kind: "digest",
        label: "Digest",
        fields: &[("username", ""), ("password", "")],
    },
    Scheme {
        kind: "oauth1",
        label: "OAuth 1.0",
        fields: &[
            ("consumerKey", ""),
            ("consumerSecret", ""),
            ("token", ""),
            ("tokenSecret", ""),
            ("signatureMethod", "HMAC-SHA1"),
            ("realm", ""),
        ],
    },
    Scheme {
        kind: "oauth2",
        label: "OAuth 2.0",
        fields: &[("accessToken", ""), ("headerPrefix", "Bearer")],
    },
    Scheme {
        kind: "hawk",
        label: "Hawk",
        fields: &[
            ("authId", ""),
            ("authKey", ""),
            ("algorithm", "sha256"),
            ("extraData", ""),
            ("app", ""),
            ("dlg", ""),
        ],
    },
    Scheme {
        kind: "awsv4",
        label: "AWS Signature",
        fields: &[
            ("accessKey", ""),
            ("secretKey", ""),
            ("region", "us-east-1"),
            ("service", "execute-api"),
            ("sessionToken", ""),
        ],
    },
    Scheme {
        kind: "ntlm",
        label: "NTLM",
        fields: &[
            ("username", ""),
            ("password", ""),
            ("domain", ""),
            ("workstation", ""),
        ],
    },
    Scheme {
        kind: "edgegrid",
        label: "Akamai EdgeGrid",
        fields: &[
            ("clientToken", ""),
            ("clientSecret", ""),
            ("accessToken", ""),
            ("headersToSign", ""),
            ("maxBody", "131072"),
        ],
    },
];
pub fn scheme(kind: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|s| s.kind.eq_ignore_ascii_case(kind))
}
pub fn secret_field(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "value"
        || key.contains("password")
        || key.contains("secret")
        || key.contains("token")
        || matches!(key.as_str(), "authkey" | "accesskey")
}
#[derive(Clone, PartialEq, Eq)]
pub struct ExecutionAuth {
    pub kind: String,
    pub fields: BTreeMap<String, String>,
    /// Acquired credentials must not enter logical previews automatically.
    pub runtime_token: bool,
}
impl std::fmt::Debug for ExecutionAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionAuth")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl ExecutionAuth {
    pub fn value(&self, name: &str) -> &str {
        self.fields.get(name).map_or("", String::as_str)
    }
    pub fn required(&self, name: &str) -> Result<&str> {
        let value = self.value(name);
        if value.is_empty() {
            Err(invalid(&format!("{} requires {name}", self.kind)))
        } else {
            Ok(value)
        }
    }
    pub fn challenge(&self) -> bool {
        matches!(self.kind.as_str(), "digest" | "ntlm")
    }
    pub fn validate(&self) -> Result<()> {
        let required: &[&str] = match self.kind.as_str() {
            "digest" | "ntlm" => &["username"],
            "oauth1" => &["consumerKey", "consumerSecret"],
            "oauth2" => &["accessToken"],
            "hawk" => &["authId", "authKey"],
            "awsv4" => &["accessKey", "secretKey", "region", "service"],
            "edgegrid" => &["clientToken", "clientSecret", "accessToken"],
            _ => return Err(invalid("Unknown runtime authentication scheme")),
        };
        for field in required {
            self.required(field)?;
        }
        if self
            .fields
            .values()
            .any(|v| v.len() > 64 * 1024 || v.chars().any(|c| matches!(c, '\r' | '\n' | '\0')))
        {
            return Err(invalid(
                "Authentication field is too long or contains a control character",
            ));
        }
        if self.kind == "ntlm"
            && ["username", "domain", "workstation"]
                .iter()
                .any(|name| self.value(name).encode_utf16().count() > 16384)
        {
            return Err(invalid(
                "NTLM username, domain and workstation must fit protocol security buffers",
            ));
        }
        if self.kind == "oauth1" && !matches!(self.value("signatureMethod"), "" | "HMAC-SHA1") {
            return Err(invalid("OAuth 1.0 signatureMethod must be HMAC-SHA1"));
        }
        if self.kind == "hawk"
            && !matches!(self.value("algorithm"), "" | "sha256" | "sha384" | "sha512")
        {
            return Err(invalid("Hawk algorithm must be sha256, sha384 or sha512"));
        }
        if self.kind == "edgegrid"
            && !self.value("maxBody").is_empty()
            && self
                .value("maxBody")
                .parse::<usize>()
                .ok()
                .filter(|n| *n <= 64 * 1024 * 1024)
                .is_none()
        {
            return Err(invalid("EdgeGrid maxBody must be between 0 and 67108864"));
        }
        Ok(())
    }
}
pub(crate) fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST authentication",
        message: message.into(),
    }
}
