//! Redeem a sharing link into a short-lived download URL, then stream the file to disk.
use crate::upload::guid;
use crate::{Auth, Body, Error, FilesClient, alias, api, origin, share_id, str_at, u64_at};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use teams_auth::Authenticator;
use teams_auth::net::{self, Signer};
use teams_i18n::t;
use tokio::io::AsyncWriteExt;
use url::Url;

const SELECT: &str = "select=@microsoft.graph.downloadUrl,@content.downloadUrl,file,id,webDavUrl,webUrl,sharepointIds,eTag,name,size,currentUserRole";
const REDEEM: [(&str, &str); 1] = [("prefer", "redeemSharingLink,getShortLivedDownloadUrl")];

/// `progress(received, total)`.
pub type Progress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadInfo {
    /// Tempauth URL, downloadable without any credentials.
    pub download_url: Option<String>,
    pub name: Option<String>,
    pub size: Option<u64>,
    pub web_url: Option<String>,
}
fn info(value: &serde_json::Value) -> DownloadInfo {
    DownloadInfo {
        download_url: str_at(value, &["/@content.downloadUrl", "/@microsoft.graph.downloadUrl"])
            .map(str::to_owned),
        name: str_at(value, &["/name"]).map(str::to_owned),
        size: u64_at(value, &["/size"]),
        web_url: str_at(value, &["/webUrl", "/webDavUrl"]).map(str::to_owned),
    }
}
fn part(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

impl<A: Authenticator> FilesClient<A> {
    /// Redeems through `shares/{shareId}`, replaying a multi-geo 404 once, then falls back to the site's item on 400/401/403/404.
    pub async fn redeem(
        &self,
        link: &str,
        fallback: Option<(&Url, &str)>,
    ) -> Result<DownloadInfo, Error> {
        let link_url = Url::parse(link).map_err(|_| Error::Invalid(t("files.download.invalid_file_link").into()))?;
        let mut host = origin(&link_url)?;
        let mut first = None;
        for _ in 0..2 {
            let url = api(
                &host,
                &format!("_api/v2.0/shares/{}/driveItem/?{SELECT}", share_id(link)),
            )?;
            match self
                .json(Auth::Resource, Method::GET, url, &REDEEM, Body::Empty)
                .await
            {
                Ok(value) => return Ok(info(&value)),
                Err(Error::Http {
                    status: 404,
                    redirect: Some(redirect),
                    ..
                }) if first.is_none() => {
                    let Some(next) = Url::parse(&redirect).ok().and_then(|u| origin(&u).ok()) else {
                        break;
                    };
                    first = Some(Error::Decode);
                    host = next;
                }
                Err(e) => {
                    first = Some(e);
                    break;
                }
            }
        }
        let error = first.unwrap_or(Error::Decode);
        match (fallback, error.status()) {
            (Some((site, file_id)), Some(400 | 401 | 403 | 404)) if !file_id.is_empty() => {
                let url = api(
                    site,
                    &format!(
                        "_api/v2.0/sites/root/items/{}/driveItem/?{SELECT}",
                        guid(file_id)?
                    ),
                )?;
                Ok(info(
                    &self
                        .json(Auth::Resource, Method::GET, url, &REDEEM, Body::Empty)
                        .await?,
                ))
            }
            _ => Err(error),
        }
    }

    /// Streams a pre-authenticated download URL to `dest` (through `dest.part`).
    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> Result<u64, Error> {
        let url = Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "https" | "http"))
            .ok_or_else(|| Error::Invalid(t("files.download.invalid_download_link").into()))?;
        self.stream(Auth::None, url, dest, progress, cancel).await
    }

    /// `getFileContent`: `{site}/_api/web/GetFileByServerRelativeUrl(@a1)/$value/?@a1='{path}'`.
    pub async fn download_content(
        &self,
        site: &Url,
        server_relative: &str,
        dest: &Path,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> Result<u64, Error> {
        let url = api(
            site,
            &format!(
                "_api/web/GetFileByServerRelativeUrl(@a1)/$value/?@a1='{}'",
                alias(server_relative)
            ),
        )?;
        self.stream(Auth::Resource, url, dest, progress, cancel).await
    }

    async fn stream(
        &self,
        auth: Auth,
        url: Url,
        dest: &Path,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> Result<u64, Error> {
        let url = std::sync::Mutex::new(url);
        let headers = || {
            let mut headers = HeaderMap::new();
            headers.insert("accept", HeaderValue::from_static("application/octet-stream"));
            if let Auth::Resource = auth {
                headers.insert("application", HeaderValue::from_static(crate::APPLICATION));
            }
            headers
        };
        let target = || Ok::<_, Error>(url.lock().unwrap().clone());
        // Only anonymous downloads may follow a redirect: no token leaves its host.
        let follow = |status: StatusCode, headers: &HeaderMap| -> Result<bool, Error> {
            if !status.is_redirection() || !matches!(auth, Auth::None) {
                return Ok(false);
            }
            let mut url = url.lock().unwrap();
            *url = headers
                .get("location")
                .and_then(|v| v.to_str().ok())
                .and_then(|l| url.join(l).ok())
                .filter(|u| u.scheme() == "https" || u.origin() == url.origin())
                .ok_or(Error::Decode)?;
            Ok(true)
        };
        let signer = match auth {
            Auth::Resource => Signer::Resource,
            _ => Signer::Anonymous,
        };
        let request = net::Exchange::<(), _>::new(signer, Method::GET, &target)
            .headers(&headers)
            .timeout(Some(crate::LONG_TIMEOUT))
            .redirects(5, &follow);
        let mut response = net::exchange(&self.http, self.auth.as_ref(), request).await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let headers = response.headers().clone();
            let body = response.bytes().await.unwrap_or_default();
            let url = url.lock().unwrap().clone();
            return Err(self.refused(&url, status, &headers, &body));
        }
        let total = response.content_length();
        let temporary = part(dest);
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|e| Error::Io(e.to_string()))?;
        let mut received = 0u64;
        progress(0, total);
        let result: Result<(), Error> = async {
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| Error::Transport(e.without_url().to_string()))?
            {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|e| Error::Io(e.to_string()))?;
                received += chunk.len() as u64;
                progress(received, total);
            }
            file.flush().await.map_err(|e| Error::Io(e.to_string()))
        }
        .await;
        drop(file);
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(e);
        }
        tokio::fs::rename(&temporary, dest)
            .await
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(received)
    }
}

/// Thumbnail of a shared file, served through its sharing link.
pub fn thumbnail_url(link: &str, size: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    Some(format!(
        "{}/_api/v2.1/shares/{}/driveItem/thumbnails/0/{size}/content?prefer=noredirect,closestavailablesize",
        url.origin().ascii_serialization(),
        share_id(link)
    ))
}
