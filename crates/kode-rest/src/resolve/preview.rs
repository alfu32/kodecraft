use super::*;
pub struct RequestPreview {
    pub text: String,
    pub body_truncated: bool,
}
impl PreparedRequest {
    pub fn preview(&self, body_limit: usize) -> Result<RequestPreview> {
        self.request.preview(&self.body, body_limit)
    }
}
impl std::fmt::Debug for RequestPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestPreview")
            .field("body_truncated", &self.body_truncated)
            .finish_non_exhaustive()
    }
}
impl ResolvedRequest {
    /// A logical HTTP/1.1 view for the UI, not a packet capture. Supply the full
    /// materialized body; display truncation never changes the outgoing bytes.
    pub fn preview(&self, body: &[u8], body_limit: usize) -> Result<RequestPreview> {
        let url = url::Url::parse(&self.url).map_err(|_| invalid("Invalid preview URL"))?;
        let mut target = url.path().to_string();
        if target.is_empty() {
            target.push('/');
        }
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        let mut text = format!("{} {target} HTTP/1.1\n", self.method.to_ascii_uppercase());
        let host = url
            .host_str()
            .ok_or_else(|| invalid("Preview URL has no host"))?;
        text.push_str("Host: ");
        text.push_str(host);
        if let Some(port) = url.port() {
            text.push(':');
            text.push_str(&port.to_string());
        }
        text.push('\n');
        for header in &self.headers {
            if header.name.eq_ignore_ascii_case("Host")
                || header.name.eq_ignore_ascii_case("Content-Length")
                || (header.name.eq_ignore_ascii_case("Connection")
                    && header.value.eq_ignore_ascii_case("keep-alive"))
            {
                continue;
            }
            super::request::validate_header(header)?;
            text.push_str(&header.name);
            text.push_str(": ");
            if header.name.eq_ignore_ascii_case("Authorization")
                && self.auth.as_ref().is_some_and(|auth| auth.runtime_token)
            {
                text.push_str("[runtime OAuth credential]");
            } else {
                text.push_str(&header.value);
            }
            text.push('\n');
        }
        if !matches!(self.body, BodyPlan::Empty) {
            text.push_str(&format!("Content-Length: {}\n", body.len()));
        }
        text.push('\n');
        let shown = body.len().min(body_limit);
        text.push_str(&String::from_utf8_lossy(&body[..shown]));
        let body_truncated = shown < body.len();
        if body_truncated {
            text.push_str(&format!(
                "\n[body preview truncated: {shown} of {} bytes]",
                body.len()
            ));
        }
        if let Some(auth) = &self.auth
            && !self
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("Authorization"))
        {
            text.push_str(&format!(
                "\n[Authentication: {}; Authorization is generated during execution]",
                auth.kind
            ));
        }
        Ok(RequestPreview {
            text,
            body_truncated,
        })
    }
}
