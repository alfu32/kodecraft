use super::{ExecutionAuth, invalid};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use kode_core::Result;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

/// Sign the actual transport headers and materialized body. Explicit time/nonce
/// support deterministic external vectors; execution supplies fresh values.
pub fn sign(
    auth: &ExecutionAuth,
    method: &str,
    url: &Url,
    headers: &mut HeaderMap,
    body: &[u8],
    now: SystemTime,
    nonce: &str,
) -> Result<()> {
    auth.validate()?;
    if auth.challenge() {
        return Ok(());
    }
    headers.remove(AUTHORIZATION);
    let value = match auth.kind.as_str() {
        "oauth2" => format!(
            "{} {}",
            auth.value("headerPrefix"),
            auth.required("accessToken")?
        ),
        "oauth1" => oauth1(auth, method, url, headers, body, now, nonce)?,
        "hawk" => hawk(auth, method, url, headers, body, now, nonce)?,
        "edgegrid" => edgegrid(auth, method, url, headers, body, now, nonce)?,
        "awsv4" => {
            aws(auth, method, url, headers, body, now)?;
            return Ok(());
        }
        _ => return Err(invalid("Unknown request signer")),
    };
    let mut value = HeaderValue::from_str(&value)
        .map_err(|_| invalid("Invalid generated authorization header"))?;
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    Ok(())
}
fn content_type(headers: &HeaderMap) -> &str {
    headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
}
const OAUTH_ESCAPE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
fn oauth_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, OAUTH_ESCAPE).to_string()
}
struct OAuthParameters(Vec<(String, String)>);
impl oauth1_request::Request for OAuthParameters {
    fn serialize<S: oauth1_request::serializer::Serializer>(&self, mut serializer: S) -> S::Output {
        use oauth1_request::serializer::SerializerExt;
        let mut oauth = false;
        for (key, value) in &self.0 {
            if !oauth && key.as_str() > "oauth_" {
                serializer.serialize_oauth_parameters();
                oauth = true;
            }
            // Unlike values, the crate's Authorizer writes keys directly into
            // the signature base string. Supply the second escaping level.
            serializer.serialize_parameter_encoded(&oauth_encode(key), value);
        }
        if !oauth {
            serializer.serialize_oauth_parameters();
        }
        serializer.end()
    }
}
fn oauth1(
    auth: &ExecutionAuth,
    method: &str,
    url: &Url,
    headers: &HeaderMap,
    body: &[u8],
    now: SystemTime,
    nonce: &str,
) -> Result<String> {
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if content_type(headers).eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        params.extend(
            url::form_urlencoded::parse(body).map(|(k, v)| (k.into_owned(), v.into_owned())),
        );
    }
    if params.iter().any(|(k, _)| k.starts_with("oauth_")) {
        return Err(invalid(
            "OAuth protocol parameters belong in the authentication fields",
        ));
    }
    // The crate requires already-escaped keys and its ParameterList sorts raw
    // display values. RFC 5849 instead sorts encoded key/value pairs, including
    // duplicates and punctuation. Feed its public serializer the normalized list.
    let mut params: Vec<_> = params
        .into_iter()
        .map(|(k, v)| (oauth_encode(&k), oauth_encode(&v)))
        .collect();
    params.sort();
    let params = OAuthParameters(params);
    let client =
        oauth1_request::Credentials::new(auth.value("consumerKey"), auth.value("consumerSecret"));
    let mut builder = oauth1_request::Builder::new(client, oauth1_request::HMAC_SHA1);
    if !auth.value("token").is_empty() {
        builder.token(oauth1_request::Credentials::new(
            auth.value("token"),
            auth.value("tokenSecret"),
        ));
    }
    builder.nonce(nonce).timestamp(std::num::NonZeroU64::new(
        now.duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("Invalid OAuth signing time"))?
            .as_secs(),
    ));
    let mut base = url.clone();
    base.set_query(None);
    base.set_fragment(None);
    let value = builder.authorize(method, base, &params);
    if auth.value("realm").is_empty() {
        Ok(value)
    } else {
        let realm = percent_encoding::utf8_percent_encode(
            auth.value("realm"),
            percent_encoding::NON_ALPHANUMERIC,
        );
        Ok(value.replacen("OAuth ", &format!("OAuth realm=\"{realm}\","), 1))
    }
}
fn hawk(
    auth: &ExecutionAuth,
    method: &str,
    url: &Url,
    headers: &HeaderMap,
    body: &[u8],
    now: SystemTime,
    nonce: &str,
) -> Result<String> {
    let algorithm = match auth.value("algorithm") {
        "sha384" => hawk::SHA384,
        "sha512" => hawk::SHA512,
        _ => hawk::SHA256,
    };
    let key = hawk::Key::new(auth.value("authKey"), algorithm)
        .map_err(|_| invalid("Cannot create Hawk key"))?;
    let credentials = hawk::Credentials {
        id: auth.value("authId").into(),
        key,
    };
    let hash =
        hawk::PayloadHasher::hash(content_type(headers).to_ascii_lowercase(), algorithm, body)
            .map_err(|_| invalid("Cannot hash Hawk payload"))?;
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let mut builder = hawk::RequestBuilder::new(
        method,
        url.host_str()
            .ok_or_else(|| invalid("Hawk URL has no host"))?,
        url.port_or_known_default()
            .ok_or_else(|| invalid("Hawk URL has no port"))?,
        target,
    )
    .hash(&hash[..]);
    if !auth.value("extraData").is_empty() {
        builder = builder.ext(auth.value("extraData"));
    }
    if !auth.value("app").is_empty() {
        builder = builder.app(auth.value("app"));
    }
    if !auth.value("dlg").is_empty() {
        builder = builder.dlg(auth.value("dlg"));
    }
    let header = builder
        .request()
        .make_header_full(&credentials, now, nonce)
        .map_err(|_| invalid("Cannot sign Hawk request"))?;
    Ok(format!("Hawk {header}"))
}
fn aws(
    auth: &ExecutionAuth,
    method: &str,
    url: &Url,
    headers: &mut HeaderMap,
    body: &[u8],
    now: SystemTime,
) -> Result<()> {
    use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
    let token = auth.value("sessionToken");
    let identity = aws_credential_types::Credentials::new(
        auth.value("accessKey"),
        auth.value("secretKey"),
        (!token.is_empty()).then(|| token.to_string()),
        None,
        "kode-request",
    )
    .into();
    let mut settings = SigningSettings::default();
    if auth.value("service") == "s3" {
        settings.percent_encoding_mode = aws_sigv4::http_request::PercentEncodingMode::Single;
        settings.uri_path_normalization_mode =
            aws_sigv4::http_request::UriPathNormalizationMode::Disabled;
        settings.payload_checksum_kind = aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;
    }
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(auth.value("region"))
        .name(auth.value("service"))
        .time(now)
        .settings(settings)
        .build()
        .map_err(|_| invalid("Invalid AWS signing settings"))?
        .into();
    let pairs = headers
        .iter()
        .map(|(k, v)| {
            Ok((
                k.as_str(),
                v.to_str()
                    .map_err(|_| invalid("AWS signed headers must be text"))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let request = SignableRequest::new(
        method,
        url.as_str(),
        pairs.into_iter(),
        SignableBody::Bytes(body),
    )
    .map_err(|_| invalid("Cannot construct AWS signing request"))?;
    let (instructions, _) = sign(request, &params)
        .map_err(|_| invalid("Cannot sign AWS request"))?
        .into_parts();
    for (key, value) in instructions.headers() {
        let key = http::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| invalid("Invalid AWS header name"))?;
        let mut value =
            HeaderValue::from_str(value).map_err(|_| invalid("Invalid AWS header value"))?;
        value.set_sensitive(true);
        headers.insert(key, value);
    }
    Ok(())
}
fn hmac(key: &[u8], data: &[u8]) -> Result<String> {
    let mut hmac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| invalid("Invalid EdgeGrid key"))?;
    hmac.update(data);
    Ok(STANDARD.encode(hmac.finalize().into_bytes()))
}
fn edgegrid(
    auth: &ExecutionAuth,
    method: &str,
    url: &Url,
    headers: &HeaderMap,
    body: &[u8],
    now: SystemTime,
    nonce: &str,
) -> Result<String> {
    let timestamp = time::OffsetDateTime::from(now)
        .format(time::macros::format_description!(
            "[year][month][day]T[hour]:[minute]:[second]+0000"
        ))
        .map_err(|_| invalid("Invalid EdgeGrid time"))?;
    if [auth.value("clientToken"), auth.value("accessToken"), nonce]
        .iter()
        .any(|v| v.contains([';', '\t', ' ']))
    {
        return Err(invalid("Invalid EdgeGrid token or nonce"));
    }
    let unsigned = format!(
        "EG1-HMAC-SHA256 client_token={};access_token={};timestamp={timestamp};nonce={nonce};",
        auth.value("clientToken"),
        auth.value("accessToken")
    );
    let mut signed = vec![];
    for key in auth
        .value("headersToSign")
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let name = key.to_ascii_lowercase();
        if let Some(value) = headers.get(&name) {
            let value = value
                .to_str()
                .map_err(|_| invalid("Invalid EdgeGrid header"))?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            signed.push(format!("{name}:{value}"));
        }
    }
    let max = if auth.value("maxBody").is_empty() {
        131072
    } else {
        auth.value("maxBody")
            .parse()
            .map_err(|_| invalid("Invalid EdgeGrid maxBody"))?
    };
    let hash = if method == "POST" && !body.is_empty() && max > 0 {
        STANDARD.encode(Sha256::digest(&body[..body.len().min(max)]))
    } else {
        String::new()
    };
    let data = [
        method,
        url.scheme(),
        &url[url::Position::BeforeHost..url::Position::AfterPort],
        &url[url::Position::BeforePath..url::Position::AfterQuery],
        &signed.join("\t"),
        &hash,
        &unsigned,
    ]
    .join("\t");
    let key = hmac(auth.value("clientSecret").as_bytes(), timestamp.as_bytes())?;
    Ok(format!(
        "{unsigned}signature={}",
        hmac(key.as_bytes(), data.as_bytes())?
    ))
}
