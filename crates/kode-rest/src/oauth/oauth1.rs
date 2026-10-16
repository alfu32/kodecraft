use super::{Channels, Config, Progress, TokenSet, callback::Callback, client::Client, invalid};
use crate::auth::ExecutionAuth;
use kode_core::Result;
use std::{collections::BTreeMap, time::SystemTime};
async fn exchange(
    http: &Client,
    url: &str,
    auth: &ExecutionAuth,
) -> Result<BTreeMap<String, String>> {
    let url = super::endpoint(url)?;
    let mut request = http::Request::builder()
        .method("POST")
        .uri(url.as_str())
        .header("Accept", "application/x-www-form-urlencoded")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(vec![])
        .map_err(|_| invalid("Invalid OAuth 1.0 endpoint request"))?;
    crate::auth::sign(
        auth,
        "POST",
        &url,
        request.headers_mut(),
        &[],
        SystemTime::now(),
        &uuid::Uuid::new_v4().to_string(),
    )?;
    let response = http.send(request).await?;
    if !response.status().is_success() {
        return Err(invalid("OAuth 1.0 endpoint rejected the request"));
    }
    let mut fields = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(response.body()) {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(invalid("Duplicate OAuth 1.0 token response parameter"));
        }
    }
    for name in ["oauth_token", "oauth_token_secret"] {
        if fields
            .get(name)
            .is_none_or(|s| s.is_empty() || s.len() > 32 * 1024 || s.chars().any(char::is_control))
        {
            return Err(invalid(
                "OAuth 1.0 endpoint returned invalid token credentials",
            ));
        }
    }
    Ok(fields)
}
pub(super) async fn acquire(
    config: &Config,
    http: &Client,
    channels: &mut Channels,
) -> Result<TokenSet> {
    if config.client_secret.is_empty() {
        return Err(invalid("OAuth 1.0 requires a consumer secret"));
    }
    let callback = Callback::bind(&config.redirect_url).await?;
    // OAuth 1.0 does not define OAuth 2.0 state. Its temporary token is the
    // callback correlation secret, validated before accepting the verifier.
    let mut auth = ExecutionAuth {
        runtime_token: false,
        kind: "oauth1".into(),
        fields: BTreeMap::from([
            ("consumerKey".into(), config.client_id.clone()),
            ("consumerSecret".into(), config.client_secret.clone()),
            ("callback".into(), callback.url.to_string()),
        ]),
    };
    let temporary = exchange(http, &config.request_token_url, &auth).await?;
    if temporary
        .get("oauth_callback_confirmed")
        .map(String::as_str)
        != Some("true")
    {
        return Err(invalid("OAuth 1.0 server did not confirm the callback"));
    }
    let token = &temporary["oauth_token"];
    let mut url = super::endpoint(&config.authorization_url)?;
    if url.query_pairs().any(|(k, _)| k == "oauth_token") {
        return Err(invalid(
            "OAuth 1.0 authorization URL already has an oauth_token parameter",
        ));
    }
    url.query_pairs_mut().append_pair("oauth_token", token);
    channels.progress.send_replace(Progress{message:"Open or copy the OAuth 1.0 authorization URL in your GUI browser, then complete login or paste the full redirect URL".into(),authorization_url:Some(url.into()),user_code:None,callback_url:Some(callback.url.to_string())});
    let verifier = callback
        .wait(channels, |fields| {
            if fields.get("oauth_token") != Some(token) {
                return Err(invalid(
                    "OAuth 1.0 callback token does not match the pending flow",
                ));
            }
            if fields.contains_key("denied") || fields.contains_key("oauth_problem") {
                return Ok(Err(invalid("OAuth 1.0 authorization was denied")));
            }
            let verifier = fields
                .get("oauth_verifier")
                .filter(|v| !v.is_empty() && v.len() <= 8192 && !v.chars().any(char::is_control))
                .ok_or_else(|| invalid("OAuth 1.0 callback is missing a valid verifier"))?;
            Ok(Ok(verifier.clone()))
        })
        .await??;
    auth.fields.remove("callback");
    auth.fields.insert("token".into(), token.clone());
    auth.fields.insert(
        "tokenSecret".into(),
        temporary["oauth_token_secret"].clone(),
    );
    auth.fields.insert("verifier".into(), verifier);
    let credentials = exchange(http, &config.token_url, &auth).await?;
    Ok(TokenSet {
        kind: "oauth1".into(),
        access_token: credentials["oauth_token"].clone(),
        token_secret: Some(credentials["oauth_token_secret"].clone()),
        refresh_token: None,
        prefix: String::new(),
        expires_at: None,
        scopes: vec![],
    })
}
