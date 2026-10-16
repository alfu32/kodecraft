use kode_rest::http::CookieJar;
use url::Url;
fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}
fn header(jar: &CookieJar, value: &str) -> String {
    jar.header(&url(value))
        .map(|h| h.to_str().unwrap().to_string())
        .unwrap_or_default()
}
#[test]
fn host_only_domain_secure_path_boundaries_default_path_and_expiration_follow_cookie_rules() {
    let jar = CookieJar::default();
    jar.accept(
        &url("https://api.example.test/accounts/login"),
        [
            "session=host; Secure; HttpOnly",
            "shared=yes; Domain=example.test; Path=/",
            "scoped=one; Path=/accounts",
            "scoped=two; Path=/accounts/sub",
            "expired=gone; Max-Age=0",
        ],
    );
    let sent = header(&jar, "https://api.example.test/accounts/sub/child");
    assert!(sent.starts_with("scoped=two"));
    assert!(sent.contains("session=host"));
    assert!(sent.contains("shared=yes"));
    assert!(!sent.contains("expired="));
    let sent = header(&jar, "http://api.example.test/accounts/login");
    assert!(!sent.contains("session="));
    assert!(sent.contains("shared=yes"));
    let sent = header(&jar, "https://other.api.example.test/accounts/login");
    assert!(!sent.contains("session="));
    assert!(sent.contains("shared=yes"));
    let sent = header(&jar, "https://api.example.test/accounts2");
    assert!(!sent.contains("scoped="));
    assert!(!sent.contains("session="));
    assert_eq!(header(&jar, "https://not-example.test/accounts/login"), "");
    jar.accept(
        &url("https://api.example.test/accounts/login"),
        [
            "session=updated; Secure; HttpOnly",
            "shared=delete; Domain=example.test; Path=/; Max-Age=0",
        ],
    );
    assert!(!header(&jar, "https://api.example.test/").contains("shared="));
    assert!(header(&jar, "https://api.example.test/accounts/").contains("session=updated"));
}
#[test]
fn public_suffix_wrong_domain_and_cookie_prefixes_are_checked_without_source_values_in_debug() {
    let jar = CookieJar::default();
    jar.accept(
        &url("https://api.example.co.uk/"),
        [
            "cross=bad; Domain=evil.test",
            "broad=bad; Domain=co.uk",
            "__Host-token=secret; Secure; Path=/; Domain=example.co.uk",
            "__Host-token=good-secret; Secure; Path=/",
            "__Secure-token=secret",
            "plain=ok; Path=/",
        ],
    );
    let cookies = jar.snapshot();
    assert_eq!(cookies.len(), 2);
    assert!(
        cookies
            .iter()
            .any(|c| c.name == "__Host-token" && c.host_only && c.secure)
    );
    assert!(!format!("{jar:?} {cookies:?}").contains("secret"));
    jar.accept(
        &url("http://api.example.co.uk/"),
        [
            "__Secure-x=secret; Secure",
            "s=secret; Secure",
            "__Host-x=secret; Secure; Path=/",
        ],
    );
    assert_eq!(jar.snapshot().len(), 2);
    assert!(jar.rejected() >= 5);
}
#[test]
fn cookie_store_caps_count_and_header_size_and_clear_drops_session_state() {
    let jar = CookieJar::default();
    let origin = url("https://example.test/");
    let headers: Vec<_> = (0..1025)
        .map(|index| format!("c{index}=v; Path=/"))
        .collect();
    jar.accept(&origin, headers);
    assert_eq!(jar.snapshot().len(), 1024);
    assert!(jar.rejected() > 0);
    jar.accept(&origin, [format!("too-big={}", "x".repeat(16 * 1024))]);
    assert_eq!(jar.snapshot().len(), 1024);
    assert!(!jar.snapshot().iter().any(|c| c.name == "too-big"));
    jar.clear();
    assert!(jar.snapshot().is_empty());
    assert!(jar.header(&origin).is_none());
}

#[test]
fn cookie_byte_budget_includes_default_paths_not_present_in_set_cookie_header() {
    let jar = CookieJar::default();
    let origin = url(&format!(
        "https://example.test/{}/login",
        "x".repeat(16 * 1024)
    ));
    jar.accept(&origin, (0..80).map(|index| format!("c{index}=v")));
    assert!(jar.snapshot().len() < 64);
    assert!(jar.rejected() > 0);
}
