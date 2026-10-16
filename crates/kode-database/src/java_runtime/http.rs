use super::{DOWNLOAD_LIMIT, Progress, check, invalid, io, progress};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use std::{fs::File, io::Write, path::Path, time::Duration};
use tokio::sync::watch;
pub(super) struct Http {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}
fn url(value: &str) -> Result<url::Url> {
    let url =
        url::Url::parse(value).map_err(|_| invalid("Invalid runtime release/download URL"))?;
    let local = matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || url.host_str() == Some("localhost");
    if value.len() > 8192
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && local)
    {
        return Err(invalid(
            "Runtime download requires HTTPS (HTTP is allowed only for local fixtures)",
        ));
    }
    Ok(url)
}
impl Http {
    pub fn new(runtime: tokio::runtime::Handle, release_url: &str) -> Result<Self> {
        let source = url(release_url)?;
        let local = source.scheme() == "http" || source.host_str() == Some("localhost");
        let builder = reqwest::Client::builder();
        let builder = if local { builder.no_proxy() } else { builder };
        let client = builder
            .retry(reqwest::retry::never())
            .user_agent("kode-rs Java-runtime-installer")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10
                    || (attempt.url().scheme() == "http"
                        && attempt.previous().iter().any(|url| url.scheme() == "https"))
                    || url(attempt.url().as_str()).is_err()
                {
                    attempt.error("Runtime redirect rejected")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| invalid("Cannot initialize runtime download transport"))?;
        Ok(Self { client, runtime })
    }
    fn fetch(
        &self,
        address: &str,
        limit: u64,
        expected: Option<u64>,
        output: &mut dyn Write,
        cancel: &CancellationToken,
        mut notify: impl FnMut(u64, Option<u64>),
    ) -> Result<()> {
        let address = url(address)?;
        self.runtime.block_on(async {
            tokio::select! {biased;
                _=cancel.cancelled()=>Err(DomainError::Cancelled),
                result=async {
                    let mut response=self.client.get(address).send().await.map_err(|_|invalid("Runtime download failed; check network access and retry"))?;
                    if !response.status().is_success() {return Err(invalid(&format!("Runtime release/download returned HTTP {}; check that a runtime release is published, or configure local Java",response.status().as_u16())));}
                    let total=response.content_length();
                    if total.is_some_and(|n|n>limit) {return Err(invalid("Runtime response exceeds its download limit"));}
                    let mut count=0u64;
                    while let Some(chunk)=response.chunk().await.map_err(|_|invalid("Runtime download was interrupted"))? {
                        check(cancel)?;count=count.saturating_add(chunk.len() as u64);
                        if count>limit {return Err(invalid("Runtime response exceeds its download limit"));}
                        output.write_all(&chunk).map_err(|error|io("write runtime download",Path::new("<temporary runtime download>"),error))?;
                        notify(count,total);
                    }
                    if total.is_some_and(|n|n!=count) || expected.is_some_and(|n|n!=count) {return Err(invalid("Runtime download length does not match its release metadata"));}
                    Ok(())
                }=>result,
            }
        })
    }
    pub fn small(&self, address: &str, limit: u64, cancel: &CancellationToken) -> Result<Vec<u8>> {
        let mut bytes = vec![];
        self.fetch(address, limit, None, &mut bytes, cancel, |_, _| {})?;
        Ok(bytes)
    }
    pub fn download(
        &self,
        address: &str,
        path: &Path,
        size: u64,
        cancel: &CancellationToken,
        sender: &watch::Sender<Progress>,
    ) -> Result<()> {
        let mut output = File::create(path).map_err(|e| io("create runtime download", path, e))?;
        self.fetch(
            address,
            DOWNLOAD_LIMIT,
            Some(size),
            &mut output,
            cancel,
            |completed, total| progress(sender, "Downloading", completed, total),
        )?;
        output
            .sync_all()
            .map_err(|e| io("sync runtime download", path, e))
    }
}
