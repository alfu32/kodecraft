use super::invalid;
use kode_core::Result;
use std::{future::Future, pin::Pin, time::Duration};
pub(crate) const MAX_BODY: usize = 64 * 1024;
pub(crate) struct Client(reqwest::Client);
impl Client {
    pub fn new() -> Result<Self> {
        Ok(Self(
            reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .redirect(reqwest::redirect::Policy::none())
                .referer(false)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| invalid("Cannot configure OAuth HTTP client"))?,
        ))
    }
    pub async fn send(&self, request: oauth2::HttpRequest) -> Result<oauth2::HttpResponse> {
        let mut request: reqwest::Request = request
            .try_into()
            .map_err(|_| invalid("Invalid OAuth HTTP request"))?;
        if let Some(value) = request.headers_mut().get_mut(http::header::AUTHORIZATION) {
            value.set_sensitive(true);
        }
        let mut response = self
            .0
            .execute(request)
            .await
            .map_err(|_| invalid("OAuth endpoint request failed"))?;
        if response.headers().len() > 256
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(invalid("OAuth endpoint response exceeds its size limit"));
        }
        let mut output = http::Response::builder().status(response.status());
        *output
            .headers_mut()
            .ok_or_else(|| invalid("Invalid OAuth response headers"))? = response.headers().clone();
        let mut body = vec![];
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| invalid("OAuth endpoint response body failed"))?
        {
            if chunk.len() > MAX_BODY.saturating_sub(body.len()) {
                return Err(invalid("OAuth endpoint response exceeds its size limit"));
            }
            body.extend_from_slice(&chunk);
        }
        output
            .body(body)
            .map_err(|_| invalid("Invalid OAuth response"))
    }
}
impl<'a> oauth2::AsyncHttpClient<'a> for Client {
    type Error = kode_core::DomainError;
    type Future = Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse>> + Send + 'a>>;
    fn call(&'a self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(self.send(request))
    }
}
