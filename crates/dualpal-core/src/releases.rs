//! The newest published Dual Pal release, read from the GitHub Releases API.
//!
//! The app compares its version with this one and suggests an upgrade. It
//! works without the Tauri updater's signed `latest.json`, so a release that
//! only has installers is still announced.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// GitHub's "latest release" for the project: the newest release that is
/// neither a draft nor a pre-release.
pub const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/hoangtuanbs/DualPal/releases/latest";

/// What the UI needs to announce a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    /// The version without a leading `v`, e.g. `0.2.0` for tag `v0.2.0`.
    pub version: String,
    /// The tag as published, e.g. `v0.2.0`.
    pub tag: String,
    /// The release's title, or the tag when it has none.
    pub name: String,
    /// The release page on github.com, where the installers are.
    pub url: String,
    /// When it was published (RFC 3339), if GitHub says.
    pub published_at: Option<String>,
    /// The files attached to the release: installers and updater files.
    pub assets: Vec<ReleaseAsset>,
}

/// One file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    /// As GitHub stores it, e.g. `Dual.Pal_0.2.0_x64-setup.exe`.
    pub name: String,
    /// The download link.
    pub url: String,
    pub size: u64,
    /// GitHub's SHA-256 of the file, lowercase hex, when it reports one.
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

/// Read a release from the JSON of GitHub's `releases/latest` endpoint.
pub fn parse_latest_release(json: &[u8]) -> Result<ReleaseInfo> {
    let r: GithubRelease = serde_json::from_slice(json)
        .map_err(|e| Error::Invalid(format!("release information: {e}")))?;
    if r.draft || r.prerelease {
        return Err(Error::Invalid(format!(
            "{} is not a final release",
            r.tag_name
        )));
    }
    if !r.html_url.starts_with("https://") {
        return Err(Error::Invalid(format!("release page {}", r.html_url)));
    }
    let version = r.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    if version.is_empty() {
        return Err(Error::Invalid("release without a version tag".into()));
    }
    let name = r
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| r.tag_name.clone());
    Ok(ReleaseInfo {
        version,
        tag: r.tag_name,
        name,
        url: r.html_url,
        published_at: r.published_at,
        assets: r
            .assets
            .into_iter()
            .filter(|a| a.browser_download_url.starts_with("https://"))
            .map(|a| ReleaseAsset {
                sha256: a
                    .digest
                    .as_deref()
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(|h| h.to_ascii_lowercase()),
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    })
}

/// Fetch the newest release from `url` (normally [`LATEST_RELEASE_URL`]).
pub async fn latest_release(url: &str, user_agent: &str) -> Result<ReleaseInfo> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| Error::Connection(e.to_string()))?;
    let resp = client
        .get(url)
        // GitHub's API refuses requests without a User-Agent.
        .header(reqwest::header::USER_AGENT, user_agent)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Connection(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotFound("no published release yet".into()));
    }
    if !status.is_success() {
        return Err(Error::Connection(format!("GitHub answered {status}")));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| Error::Connection(e.to_string()))?;
    parse_latest_release(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","name":"Dual Pal {tag}","html_url":"https://github.com/hoangtuanbs/DualPal/releases/tag/{tag}",
               "published_at":"2026-11-13T10:00:00Z","assets":[]{extra}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn reads_version_page_and_date() {
        let r = parse_latest_release(&release("v0.3.0", "")).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.tag, "v0.3.0");
        assert_eq!(r.name, "Dual Pal v0.3.0");
        assert_eq!(
            r.url,
            "https://github.com/hoangtuanbs/DualPal/releases/tag/v0.3.0"
        );
        assert_eq!(r.published_at.as_deref(), Some("2026-11-13T10:00:00Z"));
    }

    #[test]
    fn reads_assets_with_their_digest() {
        let json = br#"{"tag_name":"v0.3.0","html_url":"https://github.com/x/y/releases/tag/v0.3.0","assets":[
            {"name":"Dual.Pal_0.3.0_x64-setup.exe","browser_download_url":"https://github.com/x/y/releases/download/v0.3.0/Dual.Pal_0.3.0_x64-setup.exe","size":42,
             "digest":"sha256:F83F1CBFA7350638EBBF13485B55D86E134112E252018AA09EF2B39E74676899"},
            {"name":"odd.bin","browser_download_url":"https://github.com/x/y/odd.bin","digest":"md5:abc"},
            {"name":"plain.bin","browser_download_url":"http://example.com/plain.bin"}]}"#;
        let r = parse_latest_release(json).unwrap();
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.assets[0].size, 42);
        assert_eq!(
            r.assets[0].sha256.as_deref(),
            Some("f83f1cbfa7350638ebbf13485b55d86e134112e252018aa09ef2b39e74676899")
        );
        assert_eq!(r.assets[1].sha256, None);
    }

    #[test]
    fn accepts_a_tag_without_v_and_a_missing_name() {
        let json = br#"{"tag_name":"1.0.0","name":null,"html_url":"https://github.com/x/y/releases/tag/1.0.0"}"#;
        let r = parse_latest_release(json).unwrap();
        assert_eq!(r.version, "1.0.0");
        assert_eq!(r.name, "1.0.0");
        assert_eq!(r.published_at, None);
    }

    #[test]
    fn refuses_drafts_prereleases_and_odd_pages() {
        assert!(parse_latest_release(&release("v0.3.0", r#","draft":true"#)).is_err());
        assert!(parse_latest_release(&release("v0.3.0", r#","prerelease":true"#)).is_err());
        let http = br#"{"tag_name":"v1","html_url":"http://example.com/"}"#;
        assert!(parse_latest_release(http).is_err());
        let no_version = br#"{"tag_name":"v","html_url":"https://example.com/"}"#;
        assert!(parse_latest_release(no_version).is_err());
        assert!(parse_latest_release(b"not json").is_err());
    }
}
