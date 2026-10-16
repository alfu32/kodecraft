use super::{ClientAuthentication, Config, Grant, invalid};
use crate::resolve::{Auth, RequestResolver};
use kode_core::Result;
use serde_json::Value;
use std::time::Duration;
pub fn configuration_fields(kind: &str) -> &'static [(&'static str, &'static str)] {
    match kind {
        "oauth2" => &[
            ("grant_type", "authorization_code"),
            ("authUrl", ""),
            ("accessTokenUrl", ""),
            ("clientId", ""),
            ("clientSecret", ""),
            ("scope", ""),
            ("redirect_uri", "https://localhost:0/callback"),
            ("client_authentication", "header"),
            ("deviceAuthUrl", ""),
            ("authTimeout", "300"),
        ],
        "oauth1" => &[
            ("requestTokenUrl", ""),
            ("authUrl", ""),
            ("accessTokenUrl", ""),
            ("redirect_uri", "https://localhost:0/callback"),
            ("authTimeout", "300"),
        ],
        _ => &[],
    }
}
fn field(auth: &Auth, key: &str, resolver: &RequestResolver<'_>) -> Result<String> {
    let raw = auth
        .definition
        .get(&auth.kind)
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|r| r.get("key").and_then(Value::as_str) == Some(key))
        })
        .and_then(|r| crate::resolve::primitive(r.get("value")))
        .or_else(|| crate::resolve::primitive(auth.definition.get(key)))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            configuration_fields(&auth.kind)
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, default)| (*default).into())
        })
        .unwrap_or_default();
    if raw.len() > 64 * 1024 {
        return Err(invalid("OAuth configuration field exceeds 64 KiB"));
    }
    let value = resolver.resolve(&raw)?;
    if !value.unresolved.is_empty() || !value.cycles.is_empty() {
        return Err(invalid(&format!(
            "OAuth setting {key} has unresolved or cyclic variables"
        )));
    }
    Ok(value.value)
}
impl Config {
    pub fn from_resolver(resolver: &RequestResolver<'_>) -> Result<Self> {
        let auth = resolver
            .effective_auth()?
            .ok_or_else(|| invalid("Select OAuth 1.0 or OAuth 2.0 authentication first"))?;
        let get = |key| field(&auth, key, resolver);
        let grant = match auth.kind.as_str() {
            "oauth1" => Grant::OAuth1,
            "oauth2" => match get("grant_type")?.as_str() {
                "authorization_code" | "authorization_code_with_pkce" => Grant::AuthorizationCode,
                "client_credentials" => Grant::ClientCredentials,
                "device_code" => Grant::DeviceCode,
                _ => {
                    return Err(invalid(
                        "OAuth grant_type must be authorization_code, client_credentials or device_code",
                    ));
                }
            },
            _ => {
                return Err(invalid(
                    "Select OAuth 1.0 or OAuth 2.0 authentication first",
                ));
            }
        };
        let config = Self {
            grant,
            authorization_url: if matches!(grant, Grant::AuthorizationCode | Grant::OAuth1) {
                get("authUrl")?
            } else {
                String::new()
            },
            token_url: get("accessTokenUrl")?,
            request_token_url: if grant == Grant::OAuth1 {
                get("requestTokenUrl")?
            } else {
                String::new()
            },
            device_url: if grant == Grant::DeviceCode {
                get("deviceAuthUrl")?
            } else {
                String::new()
            },
            client_id: get(if grant == Grant::OAuth1 {
                "consumerKey"
            } else {
                "clientId"
            })?,
            client_secret: get(if grant == Grant::OAuth1 {
                "consumerSecret"
            } else {
                "clientSecret"
            })?,
            scopes: get("scope")?
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            redirect_url: local_callback(get("redirect_uri")?),
            client_authentication: match get("client_authentication")?.as_str() {
                "" | "header" | "basic" => ClientAuthentication::Basic,
                "body" => ClientAuthentication::Body,
                _ => {
                    return Err(invalid(
                        "OAuth client_authentication must be header or body",
                    ));
                }
            },
            timeout: Duration::from_secs(
                get("authTimeout")?
                    .parse()
                    .map_err(|_| invalid("OAuth authTimeout must be seconds from 1 to 1800"))?,
            ),
        };
        config.validate()?;
        Ok(config)
    }
}
#[derive(Clone)]
pub struct BoundTokens {
    pub config: Config,
    pub tokens: super::TokenSet,
}
impl std::fmt::Debug for BoundTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundOAuthTokens").finish_non_exhaustive()
    }
}
impl BoundTokens {
    pub fn for_resolver(&self, resolver: &RequestResolver<'_>) -> Result<super::TokenSet> {
        if Config::from_resolver(resolver)? != self.config {
            return Err(invalid(
                "OAuth settings changed; clear or acquire a token for the current settings",
            ));
        }
        if self.tokens.expired() {
            return Err(invalid(
                "OAuth token expired; refresh or acquire a token explicitly",
            ));
        }
        Ok(self.tokens.clone())
    }
}

// Imported Postman defaults are not callback listeners owned by Kode.
fn local_callback(value: String) -> String {
    if url::Url::parse(&value)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| {
            matches!(
                host.as_str(),
                "oauth.pstmn.io" | "oauth.getpostman.com" | "www.getpostman.com" | "getpostman.com"
            )
        })
    {
        Config::default().redirect_url
    } else {
        value
    }
}
