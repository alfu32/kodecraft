use super::{
    Channels, ClientAuthentication, Config, Grant, Progress, TokenSet, callback::Callback,
    client::Client, invalid,
};
use kode_core::Result;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, ErrorResponse, PkceCodeChallenge, RedirectUrl, RefreshToken,
    RequestTokenError, Scope, StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};
use std::time::Instant;
type OAuthClient = oauth2::basic::BasicClient<
    oauth2::EndpointMaybeSet,
    oauth2::EndpointMaybeSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;
fn client(config: &Config) -> Result<OAuthClient> {
    let mut client = oauth2::basic::BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_token_uri(
            TokenUrl::new(config.token_url.clone())
                .map_err(|_| invalid("Invalid OAuth token URL"))?,
        )
        .set_auth_uri_option(if config.authorization_url.is_empty() {
            None
        } else {
            Some(
                AuthUrl::new(config.authorization_url.clone())
                    .map_err(|_| invalid("Invalid OAuth authorization URL"))?,
            )
        })
        .set_device_authorization_url_option(if config.device_url.is_empty() {
            None
        } else {
            Some(
                DeviceAuthorizationUrl::new(config.device_url.clone())
                    .map_err(|_| invalid("Invalid device authorization URL"))?,
            )
        })
        .set_auth_type(
            if config.client_authentication == ClientAuthentication::Basic {
                AuthType::BasicAuth
            } else {
                AuthType::RequestBody
            },
        );
    if !config.client_secret.is_empty() {
        client = client.set_client_secret(ClientSecret::new(config.client_secret.clone()));
    }
    Ok(client)
}
fn error<E: ErrorResponse>(
    error: RequestTokenError<kode_core::DomainError, E>,
) -> kode_core::DomainError {
    // Never format the library error: parse/request errors can retain token
    // response bytes or credential-bearing URLs. Provider descriptions are not
    // trusted diagnostic text either.
    match error {
        RequestTokenError::ServerResponse(_) => {
            invalid("OAuth endpoint rejected the grant or client credentials")
        }
        RequestTokenError::Request(_) => {
            invalid("OAuth endpoint request failed or exceeded its response limit")
        }
        RequestTokenError::Parse(_, _) => {
            invalid("OAuth endpoint returned an invalid token response")
        }
        _ => invalid("OAuth endpoint returned an unexpected response"),
    }
}
fn tokens(response: oauth2::basic::BasicTokenResponse) -> Result<TokenSet> {
    let access = response.access_token().secret();
    if access.is_empty() || access.len() > 32 * 1024 || access.chars().any(char::is_control) {
        return Err(invalid(
            "OAuth endpoint returned an empty or invalid access token",
        ));
    }
    if *response.token_type() != oauth2::basic::BasicTokenType::Bearer {
        return Err(invalid(
            "OAuth token response must use the supported Bearer token type",
        ));
    }
    let refresh = response.refresh_token().map(|token| token.secret().clone());
    if refresh
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 32 * 1024 || s.chars().any(char::is_control))
    {
        return Err(invalid("OAuth endpoint returned an invalid refresh token"));
    }
    let expires_at = response
        .expires_in()
        .map(|duration| {
            Instant::now()
                .checked_add(duration)
                .ok_or_else(|| invalid("OAuth token expiry exceeds the supported range"))
        })
        .transpose()?;
    Ok(TokenSet {
        kind: "oauth2".into(),
        access_token: access.clone(),
        refresh_token: refresh,
        token_secret: None,
        prefix: "Bearer".into(),
        expires_at,
        scopes: response
            .scopes()
            .map(|scopes| scopes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
    })
}
pub(super) async fn acquire(
    config: &Config,
    http: &Client,
    channels: &mut Channels,
) -> Result<TokenSet> {
    let client = client(config)?;
    channels
        .progress
        .send_modify(|p| p.message = "Contacting OAuth endpoint".into());
    match config.grant {
        Grant::ClientCredentials => {
            let mut request = client.exchange_client_credentials();
            for scope in &config.scopes {
                request = request.add_scope(Scope::new(scope.clone()));
            }
            tokens(request.request_async(http).await.map_err(error)?)
        }
        Grant::AuthorizationCode => {
            let callback = Callback::bind(&config.redirect_url).await?;
            let client = client.set_redirect_uri(
                RedirectUrl::new(callback.url.to_string())
                    .map_err(|_| invalid("Invalid OAuth callback URL"))?,
            );
            let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
            let mut request = client
                .authorize_url(CsrfToken::new_random)
                .map_err(|_| invalid("OAuth authorization URL is missing"))?
                .set_pkce_challenge(challenge);
            for scope in &config.scopes {
                request = request.add_scope(Scope::new(scope.clone()));
            }
            let (url, state) = request.url();
            channels.progress.send_replace(Progress{message:"Open or copy the authorization URL; complete login in your GUI browser, or paste its full redirect URL".into(),authorization_url:Some(url.into()),user_code:None,callback_url:Some(callback.url.to_string())});
            let code = callback
                .wait(channels, |fields| {
                    let returned = fields
                        .get("state")
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| invalid("OAuth callback is missing state"))?;
                    if CsrfToken::new(returned.clone()) != state {
                        return Err(invalid(
                            "OAuth callback state does not match the pending flow",
                        ));
                    }
                    if fields.contains_key("error") {
                        return Ok(Err(invalid(
                            "OAuth authorization was denied by the authorization server",
                        )));
                    }
                    let code = fields
                        .get("code")
                        .filter(|s| {
                            !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control)
                        })
                        .ok_or_else(|| {
                            invalid("OAuth callback is missing a valid authorization code")
                        })?;
                    Ok(Ok(AuthorizationCode::new(code.clone())))
                })
                .await??;
            channels
                .progress
                .send_modify(|p| p.message = "Exchanging authorization code".into());
            tokens(
                client
                    .exchange_code(code)
                    .set_pkce_verifier(verifier)
                    .request_async(http)
                    .await
                    .map_err(error)?,
            )
        }
        Grant::DeviceCode => {
            let mut request = client
                .exchange_device_code()
                .map_err(|_| invalid("Device authorization URL is missing"))?;
            for scope in &config.scopes {
                request = request.add_scope(Scope::new(scope.clone()));
            }
            let details: StandardDeviceAuthorizationResponse =
                request.request_async(http).await.map_err(error)?;
            if details.interval() < std::time::Duration::from_secs(1)
                || details.user_code().secret().len() > 1024
                || details.user_code().secret().chars().any(char::is_control)
            {
                return Err(invalid(
                    "Device authorization response has an invalid polling interval or user code",
                ));
            }
            super::endpoint(details.verification_uri().as_str())?;
            if let Some(url) = details.verification_uri_complete() {
                super::endpoint(url.secret())?;
            }
            let url = details
                .verification_uri_complete()
                .map(|url| url.secret().as_str())
                .unwrap_or(details.verification_uri().as_str());
            channels.progress.send_replace(Progress{message:"Open or copy the verification URL in your GUI browser and enter the displayed code; waiting for device authorization".into(),authorization_url:Some(url.into()),user_code:Some(details.user_code().secret().clone()),callback_url:None});
            tokens(
                client
                    .exchange_device_access_token(&details)
                    .request_async(http, tokio::time::sleep, Some(config.timeout))
                    .await
                    .map_err(error)?,
            )
        }
        Grant::OAuth1 => Err(invalid("OAuth 1.0 uses its separate acquisition flow")),
    }
}
pub(super) async fn refresh(
    config: &Config,
    http: &Client,
    previous: TokenSet,
) -> Result<TokenSet> {
    if previous.kind != "oauth2" {
        return Err(invalid(
            "OAuth 1.0 credentials do not use OAuth 2.0 refresh grants",
        ));
    }
    let refresh = previous
        .refresh_token
        .as_ref()
        .ok_or_else(|| invalid("No refresh token is available; acquire a token explicitly"))?;
    let response = client(config)?
        .exchange_refresh_token(&RefreshToken::new(refresh.clone()))
        .request_async(http)
        .await
        .map_err(error)?;
    let mut result = tokens(response)?;
    if result.refresh_token.is_none() {
        result.refresh_token = previous.refresh_token;
    }
    if result.scopes.is_empty() {
        result.scopes = previous.scopes;
    }
    Ok(result)
}
