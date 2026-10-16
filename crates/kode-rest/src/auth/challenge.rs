//! Bounded challenge exchanges on one HTTP/1.1 client (NTLM is connection-bound).
use super::ExecutionAuth;
use crate::http::{Failure, Response, classify};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Client, Request,
    header::{AUTHORIZATION, HeaderValue, WWW_AUTHENTICATE},
};

fn header(value: String) -> std::result::Result<HeaderValue, Failure> {
    let mut value = HeaderValue::from_str(&value).map_err(|_| Failure::Authentication)?;
    value.set_sensitive(true);
    Ok(value)
}
fn find_challenge<'a>(value: &'a str, wanted: &str) -> Option<&'a str> {
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (i, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quoted {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        }
        if i == start && !quoted {
            let tail = value[i..].trim_start();
            if tail
                .get(..wanted.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(wanted))
            {
                return Some(tail);
            }
        }
        if ch == ',' && !quoted {
            start = i + 1;
        }
    }
    None
}
fn negotiate(auth: &ExecutionAuth) -> std::result::Result<String, Failure> {
    use ntlmclient::{Flags, Message, NegotiateMessage};
    let msg = Message::Negotiate(NegotiateMessage {
        flags: Flags::NEGOTIATE_UNICODE
            | Flags::REQUEST_TARGET
            | Flags::NEGOTIATE_NTLM
            | Flags::NEGOTIATE_NTLM2_KEY
            | Flags::NEGOTIATE_TARGET_INFO,
        supplied_domain: auth.value("domain").into(),
        supplied_workstation: auth.value("workstation").into(),
        os_version: Default::default(),
    });
    Ok(format!(
        "NTLM {}",
        STANDARD.encode(msg.to_bytes().map_err(|_| Failure::Authentication)?)
    ))
}
fn ntlm(auth: &ExecutionAuth, challenge: &str) -> std::result::Result<String, Failure> {
    let data = STANDARD
        .decode(challenge.trim())
        .map_err(|_| Failure::Authentication)?;
    // Validate the library's parsing preconditions for untrusted server bytes.
    if data.len() < 56
        || data.get(..8) != Some(b"NTLMSSP\0")
        || data.get(8..12) != Some(&2u32.to_le_bytes())
    {
        return Err(Failure::Authentication);
    }
    let flags = u32::from_le_bytes(
        data[20..24]
            .try_into()
            .map_err(|_| Failure::Authentication)?,
    );
    if ntlmclient::Flags::from_bits(flags).is_none() {
        return Err(Failure::Authentication);
    }
    for at in [12, 40] {
        let length = u16::from_le_bytes(
            data[at..at + 2]
                .try_into()
                .map_err(|_| Failure::Authentication)?,
        ) as usize;
        let offset = u32::from_le_bytes(
            data[at + 4..at + 8]
                .try_into()
                .map_err(|_| Failure::Authentication)?,
        ) as usize;
        if length > 0 && (offset < 56 || offset > data.len() || length > data.len() - offset) {
            return Err(Failure::Authentication);
        }
    }
    let message =
        ntlmclient::Message::try_from(data.as_slice()).map_err(|_| Failure::Authentication)?;
    let ntlmclient::Message::Challenge(challenge) = message else {
        return Err(Failure::Authentication);
    };
    if !challenge
        .flags
        .contains(ntlmclient::Flags::NEGOTIATE_UNICODE | ntlmclient::Flags::NEGOTIATE_NTLM)
    {
        return Err(Failure::Authentication);
    }
    let info: Vec<u8> = challenge
        .target_information
        .iter()
        .flat_map(|e| e.to_bytes())
        .collect();
    let credentials = ntlmclient::Credentials {
        username: auth.value("username").into(),
        password: auth.value("password").into(),
        domain: auth.value("domain").into(),
    };
    let response = ntlmclient::respond_challenge_ntlm_v2(
        challenge.challenge,
        &info,
        ntlmclient::get_ntlm_time(),
        &credentials,
    );
    let flags = ntlmclient::Flags::NEGOTIATE_UNICODE | ntlmclient::Flags::NEGOTIATE_NTLM;
    let msg = response.to_message(&credentials, auth.value("workstation"), flags);
    Ok(format!(
        "NTLM {}",
        STANDARD.encode(msg.to_bytes().map_err(|_| Failure::Authentication)?)
    ))
}
fn digest(
    auth: &ExecutionAuth,
    challenge: &str,
    request: &Request,
    body: &[u8],
) -> std::result::Result<String, Failure> {
    let target = &request.url()[url::Position::BeforePath..url::Position::AfterQuery];
    let context = digest_auth::AuthContext::new_with_method(
        auth.value("username"),
        auth.value("password"),
        target,
        Some(body),
        digest_auth::HttpMethod::from(request.method().as_str()),
    );
    digest_auth::parse(challenge)
        .and_then(|mut p| p.respond(&context))
        .map(|v| v.to_string())
        .map_err(|_| Failure::Authentication)
}
/// The caller owns cancellation/timeout around this future. No automatic retries
/// occur after non-401 responses; authentication never crosses a redirect.
pub(crate) async fn execute(
    client: Client,
    mut request: Request,
    output: &mut Response,
) -> std::result::Result<reqwest::Response, Failure> {
    let captured = output.request.clone();
    let auth = captured.request.auth.as_ref().filter(|a| a.challenge());
    if let Some(auth) = auth.filter(|a| a.kind == "ntlm") {
        request
            .headers_mut()
            .insert(AUTHORIZATION, header(negotiate(auth)?)?);
    }
    let retry = request.try_clone().ok_or(Failure::Body)?;
    let mut response = client.execute(request).await.map_err(|e| classify(&e))?;
    if response.status() != 401 {
        return Ok(response);
    }
    let Some(auth) = auth else {
        return Ok(response);
    };
    // Preserve actionable status even if a server sends malformed challenge data.
    crate::http::apply_metadata(&response, output)?;
    let wanted = if auth.kind == "digest" {
        "Digest "
    } else {
        "NTLM "
    };
    let challenge = response
        .headers()
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| find_challenge(v, wanted))
        .filter(|v| v.len() <= 64 * 1024)
        .ok_or(Failure::Authentication)?
        .to_string();
    let answer = if auth.kind == "digest" {
        digest(auth, &challenge, &retry, &output.request.body)?
    } else {
        ntlm(auth, &challenge[wanted.len()..])?
    };
    if auth.kind == "ntlm"
        && response
            .headers()
            .get("connection")
            .is_some_and(|h| h.as_bytes().eq_ignore_ascii_case(b"close"))
    {
        return Err(Failure::Authentication);
    }
    let mut received = 0usize;
    while let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? {
        received = received.saturating_add(chunk.len());
        if received > 64 * 1024 {
            return Err(Failure::Authentication);
        }
    }
    let mut request = retry;
    request.headers_mut().insert(AUTHORIZATION, header(answer)?);
    let mut normalized = (*output.normalized_request).clone();
    normalized.request.headers = crate::http::headers_from_request(&request);
    output.normalized_request = std::sync::Arc::new(normalized);
    client.execute(request).await.map_err(|e| classify(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combined_challenges_respect_quoted_commas_and_hostile_ntlm_frames_fail() {
        let value = "Basic realm=\"fake, Digest nope\", Digest realm=\"real\", nonce=\"server\"";
        assert_eq!(
            find_challenge(value, "Digest "),
            Some("Digest realm=\"real\", nonce=\"server\"")
        );
        assert_eq!(
            find_challenge("Basic realm=\"Digest nope\"", "Digest "),
            None
        );
        let auth = ExecutionAuth {
            runtime_token: false,
            kind: "ntlm".into(),
            fields: Default::default(),
        };
        for length in 0..256 {
            let mut bytes = vec![0u8; length];
            if length >= 12 {
                bytes[..8].copy_from_slice(b"NTLMSSP\0");
                bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
            }
            if length >= 48 {
                bytes[12..14].copy_from_slice(&16u16.to_le_bytes());
                bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            assert_eq!(
                ntlm(&auth, &STANDARD.encode(bytes)),
                Err(Failure::Authentication)
            );
        }
    }
}
