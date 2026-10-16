use cookie_store::{Cookie, CookieDomain, CookieStore as Store};
use reqwest::{cookie::CookieStore, header::HeaderValue};
use std::{collections::VecDeque, sync::Mutex};
use url::Url;
const MAX_COOKIES: usize = 1024;
const MAX_COOKIE_BYTES: usize = 1024 * 1024;
const MAX_COOKIE_HEADER: usize = 16 * 1024;
type Key = (String, String, String);
#[derive(Default)]
struct State {
    store: Store,
    order: VecDeque<Key>,
    rejected: u64,
}
/// Session-scoped cookie data has no persistence/serialization API. Debug omits
/// all names, values, domains and paths. `snapshot` is explicit inspection.
#[derive(Default)]
pub struct CookieJar(Mutex<State>);
#[derive(Clone, PartialEq, Eq)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub host_only: bool,
    pub secure: bool,
    pub http_only: bool,
    pub persistent: bool,
}
impl std::fmt::Debug for CookieInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieInfo(..)")
    }
}
impl std::fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieJar(..)")
    }
}
fn key(cookie: &Cookie<'_>) -> Key {
    (
        String::from(&cookie.domain),
        String::from(&cookie.path),
        cookie.name().to_string(),
    )
}
fn cookie_bytes(cookie: &Cookie<'_>) -> usize {
    cookie.to_string().len()
        + cookie.domain.as_cow().map_or(0, |domain| domain.len())
        + cookie.path.len()
}
impl CookieJar {
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = State::default();
    }
    pub fn rejected(&self) -> u64 {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).rejected
    }
    pub fn snapshot(&self) -> Vec<CookieInfo> {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut cookies: Vec<_> = state
            .store
            .iter_unexpired()
            .map(|cookie| CookieInfo {
                name: cookie.name().into(),
                value: cookie.value().into(),
                domain: String::from(&cookie.domain),
                path: String::from(&cookie.path),
                host_only: matches!(cookie.domain, CookieDomain::HostOnly(_)),
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
                persistent: cookie.is_persistent(),
            })
            .collect();
        cookies.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        cookies
    }
    pub fn accept(&self, url: &Url, headers: impl IntoIterator<Item = impl AsRef<str>>) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for header in headers {
            let header = header.as_ref();
            if header.len() > MAX_COOKIE_HEADER {
                state.rejected = state.rejected.saturating_add(1);
                continue;
            }
            let Ok(mut cookie) = Cookie::parse(header.to_string(), url).map(Cookie::into_owned)
            else {
                continue;
            };
            if let CookieDomain::Suffix(domain) = &cookie.domain
                && psl::suffix_str(domain) == Some(domain.as_str())
            {
                if url.host_str() == Some(domain.as_str()) {
                    cookie.domain = CookieDomain::HostOnly(domain.clone());
                } else {
                    state.rejected = state.rejected.saturating_add(1);
                    continue;
                }
            }
            let secure_origin = url.scheme() == "https";
            if cookie.secure().unwrap_or(false) && !secure_origin {
                state.rejected = state.rejected.saturating_add(1);
                continue;
            }
            if (cookie.name().starts_with("__Secure-")
                && (!secure_origin || !cookie.secure().unwrap_or(false)))
                || (cookie.name().starts_with("__Host-")
                    && (!secure_origin
                        || !cookie.secure().unwrap_or(false)
                        || !matches!(cookie.domain, CookieDomain::HostOnly(_))
                        || &*cookie.path != "/"))
            {
                state.rejected = state.rejected.saturating_add(1);
                continue;
            }
            let identity = key(&cookie);
            if state.store.insert(cookie, url).is_err() {
                continue;
            }
            state.order.retain(|entry| entry != &identity);
            state.order.push_back(identity);
            let expired: Vec<_> = state
                .store
                .iter_any()
                .filter(|c| c.is_expired())
                .map(key)
                .collect();
            for (domain, path, name) in expired {
                state.store.remove(&domain, &path, &name);
            }
            let valid: std::collections::HashSet<_> =
                state.store.iter_unexpired().map(key).collect();
            state.order.retain(|entry| valid.contains(entry));
            let mut bytes: usize = state.store.iter_unexpired().map(cookie_bytes).sum();
            while state.order.len() > MAX_COOKIES || bytes > MAX_COOKIE_BYTES {
                let Some((domain, path, name)) = state.order.pop_front() else {
                    break;
                };
                if let Some(cookie) = state.store.remove(&domain, &path, &name) {
                    bytes = bytes.saturating_sub(cookie_bytes(&cookie));
                }
                state.rejected = state.rejected.saturating_add(1);
            }
        }
    }
    pub fn header(&self, url: &Url) -> Option<HeaderValue> {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut cookies = state.store.matches(url);
        cookies.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then_with(|| a.name().cmp(b.name()))
        });
        if cookies.is_empty() {
            return None;
        }
        let value = cookies
            .iter()
            .map(|c| format!("{}={}", c.name(), c.value()))
            .collect::<Vec<_>>()
            .join("; ");
        HeaderValue::from_str(&value).ok()
    }
}
impl CookieStore for CookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        self.accept(url, headers.filter_map(|header| header.to_str().ok()));
    }
    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        self.header(url)
    }
}
