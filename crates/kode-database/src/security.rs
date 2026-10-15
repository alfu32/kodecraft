//! Persistence validation. Credential values belong in the separate private store,
//! never URL recents, datasource definitions or project state.
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{collections::BTreeMap, sync::LazyLock};
pub fn sensitive_key(name: &str) -> bool {
    let canonical: String = name
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | '.' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        canonical.as_str(),
        "password"
            | "passwd"
            | "pwd"
            | "pass"
            | "secret"
            | "clientsecret"
            | "oauthclientsecret"
            | "token"
            | "accesstoken"
            | "oauthaccesstoken"
            | "oauthrefreshtoken"
            | "apikey"
            | "privatekey"
            | "secretaccesskey"
            | "accesskeyid"
            | "sessiontoken"
            | "credentials"
    )
}
fn percent_decode(text: &str) -> String {
    let mut output = vec![];
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(a), Some(b)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            output.push((a * 16 + b) as u8);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}
pub fn contains_secret(url: &str) -> bool {
    static PARAM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:^|[?;&:#(])\s*([^=?;&:#()]+)\s*=").unwrap());
    static USERINFO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"://[^/@\s]+@").unwrap());
    static ORACLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^jdbc:oracle:[^:]+:[^@/]+/[^@]*@").unwrap());
    let mut value = url.to_owned();
    for _ in 0..4 {
        if USERINFO.is_match(&value)
            || ORACLE.is_match(&value)
            || PARAM
                .captures_iter(&value)
                .any(|capture| sensitive_key(&capture[1]))
        {
            return true;
        }
        let next = percent_decode(&value);
        if next == value {
            break;
        }
        value = next;
    }
    false
}
pub fn validate_persistent_json(value: &Value) -> Result<(), &'static str> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "credentialProperties" {
                    if !value
                        .as_object()
                        .is_some_and(|refs| refs.values().all(Value::is_string))
                    {
                        return Err("credential property references must be strings");
                    }
                    continue;
                }
                if sensitive_key(key) {
                    return Err("credential values must be stored separately behind references");
                }
                if matches!(
                    key.to_ascii_lowercase().as_str(),
                    "jdbcurl" | "repository" | "downloadurl" | "vendordownloadpage"
                ) && value.as_str().is_some_and(contains_secret)
                {
                    return Err("credential-bearing URLs cannot be persisted");
                }
                validate_persistent_json(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_persistent_json(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn serialize_extra<S: Serializer>(
    value: &BTreeMap<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_persistent_json(&Value::Object(value.clone().into_iter().collect()))
        .map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}
pub fn serialize_properties<S: Serializer>(
    value: &BTreeMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value.keys().any(|key| sensitive_key(key)) {
        return Err(serde::ser::Error::custom(
            "secret driver properties require credential references",
        ));
    }
    value.serialize(serializer)
}
pub fn deserialize_properties<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    let value = BTreeMap::<String, String>::deserialize(deserializer)?;
    if value.keys().any(|key| sensitive_key(key)) {
        return Err(serde::de::Error::custom(
            "secret driver properties require credential references",
        ));
    }
    Ok(value)
}
pub fn serialize_url<S: Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    if !value.trim().to_ascii_lowercase().starts_with("jdbc:") || contains_secret(value) {
        return Err(serde::ser::Error::custom(
            "expected a JDBC URL without embedded credentials",
        ));
    }
    value.serialize(serializer)
}
pub fn deserialize_url<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !value.trim().to_ascii_lowercase().starts_with("jdbc:") || contains_secret(&value) {
        return Err(serde::de::Error::custom(
            "expected a JDBC URL without embedded credentials",
        ));
    }
    Ok(value)
}
pub fn serialize_optional_locator<S: Serializer>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value.as_deref().is_some_and(contains_secret) {
        return Err(serde::ser::Error::custom(
            "credential-bearing artifact locations cannot be persisted",
        ));
    }
    value.serialize(serializer)
}
pub fn deserialize_optional_locator<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_deref().is_some_and(contains_secret) {
        return Err(serde::de::Error::custom(
            "credential-bearing artifact locations cannot be persisted",
        ));
    }
    Ok(value)
}
