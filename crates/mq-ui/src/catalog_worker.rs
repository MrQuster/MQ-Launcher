#![deny(clippy::await_holding_lock)]
//! Background (non-UI-thread) catalog work: multi-source search with stale-request
//! cancellation and disk-cached icon loading.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use mq_core::AppConfig;
use mq_mods::{
    CatalogHit, CatalogSource, CatalogSourceFilter, ContentProvider, CurseForgeClient,
    ModrinthClient, SearchQuery, DEFAULT_CURSEFORGE_API_URL, DEFAULT_MODRINTH_API_URL,
    MODRINTH_USER_AGENT, MQ_USER_AGENT,
};

use crate::catalog_state::{
    icon_cache_name, merge_hits, CatalogErrorKind, CatalogPage, CatalogSourceError,
};

/// Max accepted icon payload (2 MiB) and decoded edge length.
const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;
pub const ICON_EDGE: u32 = 96;
/// Decoded edge length for gallery images.
pub const GALLERY_EDGE: u32 = 512;
/// Hard per-provider timeout so one slow/unreachable API never stalls catalog search for 10s+.
const PROVIDER_SEARCH_TIMEOUT: Duration = Duration::from_secs(6);
/// Hosts icons may be fetched from.
const ICON_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "media.forgecdn.net",
    "media.curseforge.com",
    "mediafilez.forgecdn.net",
];

fn modrinth_search_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(MODRINTH_USER_AGENT)
            .connect_timeout(Duration::from_secs(3))
            .timeout(PROVIDER_SEARCH_TIMEOUT)
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_default()
    })
}

fn curseforge_search_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(MQ_USER_AGENT)
            .connect_timeout(Duration::from_secs(3))
            .timeout(PROVIDER_SEARCH_TIMEOUT)
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_default()
    })
}

fn icon_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(MQ_USER_AGENT)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(8)
            .build()
            .unwrap_or_default()
    })
}

fn resolve_modrinth_base_url(config: &AppConfig) -> String {
    let env_override = std::env::var(mq_core::config::ENV_MODRINTH_BASE_URL)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    match env_override.as_deref().or_else(|| {
        config
            .mock_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }) {
        Some(mock) => {
            let trimmed = mock.trim_end_matches('/');
            if trimmed.ends_with("/modrinth/v2") || trimmed.ends_with("/v2") {
                trimmed.to_owned()
            } else {
                format!("{trimmed}/modrinth/v2")
            }
        }
        None => DEFAULT_MODRINTH_API_URL.to_owned(),
    }
}

fn resolve_curseforge_base_url(config: &AppConfig) -> String {
    let env_override = std::env::var(mq_core::config::ENV_CURSEFORGE_BASE_URL)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    match env_override.as_deref().or_else(|| {
        config
            .mock_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }) {
        Some(mock) => {
            let trimmed = mock.trim_end_matches('/');
            if trimmed.ends_with("/curseforge/v1") || trimmed.ends_with("/v1") {
                trimmed.to_owned()
            } else {
                format!("{trimmed}/curseforge/v1")
            }
        }
        None => DEFAULT_CURSEFORGE_API_URL.to_owned(),
    }
}

/// Searches one provider and classifies a failure into a [`CatalogSourceError`].
pub async fn search_source<P: ContentProvider>(
    provider: &P,
    query: &SearchQuery,
    config: &AppConfig,
) -> Result<Vec<CatalogHit>, CatalogSourceError> {
    provider
        .search(query)
        .await
        .map_err(|e| CatalogSourceError {
            source: provider.source(),
            kind: CatalogErrorKind::classify(&e),
            message: config.sanitize_cf_api_key(&e.to_string()),
        })
}

/// Searches one provider bounded by `timeout` so a stalled upstream cannot block the whole query.
pub async fn search_source_with_timeout<P: ContentProvider>(
    provider: &P,
    query: &SearchQuery,
    config: &AppConfig,
    timeout: Duration,
) -> Result<Vec<CatalogHit>, CatalogSourceError> {
    match tokio::time::timeout(timeout, search_source(provider, query, config)).await {
        Ok(res) => res,
        Err(_) => Err(CatalogSourceError {
            source: provider.source(),
            kind: CatalogErrorKind::Network,
            message: "Превышено время ожидания ответа сервера".to_string(),
        }),
    }
}

/// Combines per-source outcomes into one page. `has_next` is set when a source filled the page.
pub fn assemble_page(
    request_id: u64,
    filter: CatalogSourceFilter,
    modrinth: Option<Result<Vec<CatalogHit>, CatalogSourceError>>,
    curseforge: Option<Result<Vec<CatalogHit>, CatalogSourceError>>,
    query: &SearchQuery,
) -> CatalogPage {
    let mut errors = Vec::new();
    let mut take = |r: Option<Result<Vec<CatalogHit>, CatalogSourceError>>| match r {
        Some(Ok(hits)) => hits,
        Some(Err(e)) => {
            errors.push(e);
            Vec::new()
        }
        None => Vec::new(),
    };
    let m = take(modrinth);
    let c = take(curseforge);
    let full = |n: usize| n >= query.limit as usize;
    let has_next = full(m.len()) || full(c.len());
    let hits = match filter {
        CatalogSourceFilter::All => merge_hits(m, c, query.sort),
        _ => m.into_iter().chain(c).collect(),
    };
    CatalogPage {
        request_id,
        hits,
        errors,
        has_next,
    }
}

/// Runs the search for the selected source filter. Returns `None` if a newer request
/// superseded this one (before or after the network round trip).
pub async fn run_search(
    config: &AppConfig,
    request_id: u64,
    latest: &AtomicU64,
    filter: CatalogSourceFilter,
    query: &SearchQuery,
) -> Option<CatalogPage> {
    let is_stale = || latest.load(Ordering::SeqCst) != request_id;
    if is_stale() {
        return None;
    }

    let modrinth_fut = async {
        if !filter.includes(CatalogSource::Modrinth) {
            return None;
        }
        let client = ModrinthClient::with_http_client(
            modrinth_search_client().clone(),
            resolve_modrinth_base_url(config),
        )
        .with_retry_policy(1, Duration::from_millis(200));
        Some(search_source_with_timeout(&client, query, config, PROVIDER_SEARCH_TIMEOUT).await)
    };
    let curseforge_fut = async {
        if !filter.includes(CatalogSource::CurseForge) {
            return None;
        }
        // Without a key CurseForge is reported as `MissingKey`; Modrinth keeps working.
        let Some(api_key) = config.effective_cf_api_key() else {
            return Some(Err(CatalogSourceError {
                source: CatalogSource::CurseForge,
                kind: CatalogErrorKind::MissingKey,
                message: String::new(),
            }));
        };
        let client = CurseForgeClient::with_http_client(
            curseforge_search_client().clone(),
            resolve_curseforge_base_url(config),
            Some(api_key),
        );
        Some(search_source_with_timeout(&client, query, config, PROVIDER_SEARCH_TIMEOUT).await)
    };

    let (m, c) = tokio::join!(modrinth_fut, curseforge_fut);
    if is_stale() {
        return None;
    }
    Some(assemble_page(request_id, filter, m, c, query))
}

fn icon_url_allowed(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str().is_some_and(|h| ICON_HOSTS.contains(&h))
    })
}

/// Decodes raw image bytes into a thumbnail `ColorImage`.
pub fn decode_icon(bytes: &[u8], edge: u32) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    // `thumbnail` also upscales, so only shrink oversized images.
    let thumb = if img.width() > edge || img.height() > edge {
        img.thumbnail(edge, edge).to_rgba8()
    } else {
        img.to_rgba8()
    };
    let size = [thumb.width() as usize, thumb.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        thumb.as_raw(),
    ))
}

/// Loads an icon from the disk cache or downloads it (HTTPS, allow-listed hosts, size-capped)
/// and stores the original bytes in `cache_dir`.
pub async fn load_icon(cache_dir: &Path, url: &str, edge: u32) -> Option<egui::ColorImage> {
    if !icon_url_allowed(url) {
        return None;
    }
    let path = cache_dir.join(icon_cache_name(url));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if let Some(img) = decode_icon(&bytes, edge) {
            return Some(img);
        }
    }
    let resp = icon_http_client()
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if resp
        .content_length()
        .is_some_and(|n| n as usize > MAX_ICON_BYTES)
    {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_ICON_BYTES {
        return None;
    }
    let img = decode_icon(&bytes, edge)?;
    if tokio::fs::create_dir_all(cache_dir).await.is_ok() {
        static ICON_TMP_SEQ: AtomicU64 = AtomicU64::new(1);
        let seq = ICON_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("icon");
        let tmp = cache_dir.join(format!("{file_name}.part.{}.{seq}", std::process::id()));
        if tokio::fs::write(&tmp, &bytes).await.is_ok() {
            // On Windows, `rename` fails if the destination already exists; fall back to
            // remove + rename and clean up any leftover `.part` file on error.
            if tokio::fs::rename(&tmp, &path).await.is_err() {
                let _ = tokio::fs::remove_file(&path).await;
                if tokio::fs::rename(&tmp, &path).await.is_err() {
                    let _ = tokio::fs::remove_file(&tmp).await;
                }
            }
        }
    }
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mq_mods::ProjectKind;

    fn hit(source: CatalogSource, id: &str) -> CatalogHit {
        CatalogHit {
            source,
            project_id: id.into(),
            slug: id.into(),
            title: id.into(),
            description: String::new(),
            author: String::new(),
            downloads: 1,
            icon_url: None,
            project_kind: ProjectKind::Mod,
            loaders: Vec::new(),
            game_versions: Vec::new(),
            latest_version: None,
        }
    }

    fn cf_missing_key() -> CatalogSourceError {
        CatalogSourceError {
            source: CatalogSource::CurseForge,
            kind: CatalogErrorKind::MissingKey,
            message: String::new(),
        }
    }

    #[test]
    fn modrinth_works_without_curseforge_key() {
        let q = SearchQuery::new("x");
        let page = assemble_page(
            3,
            CatalogSourceFilter::All,
            Some(Ok(vec![hit(CatalogSource::Modrinth, "a")])),
            Some(Err(cf_missing_key())),
            &q,
        );
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.errors.len(), 1);
        assert!(!page.has_next);
    }

    #[test]
    fn full_page_sets_has_next() {
        let q = SearchQuery::new("x").with_limit(2);
        let hits = vec![
            hit(CatalogSource::Modrinth, "a"),
            hit(CatalogSource::Modrinth, "b"),
        ];
        let page = assemble_page(1, CatalogSourceFilter::Modrinth, Some(Ok(hits)), None, &q);
        assert!(page.has_next);
        assert!(page.errors.is_empty());
    }

    #[test]
    fn icon_hosts_are_allow_listed() {
        assert!(icon_url_allowed("https://cdn.modrinth.com/data/x/icon.png"));
        assert!(icon_url_allowed("https://media.forgecdn.net/avatars/1.png"));
        assert!(!icon_url_allowed("http://cdn.modrinth.com/x.png"));
        assert!(!icon_url_allowed("https://evil.example/x.png"));
        assert!(!icon_url_allowed("file:///etc/passwd"));
    }

    #[test]
    fn decode_icon_rejects_garbage_and_accepts_png() {
        assert!(decode_icon(b"not an image", ICON_EDGE).is_none());
        let mut buf = Vec::new();
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 90, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let out = decode_icon(&buf, ICON_EDGE).unwrap();
        assert_eq!(out.size, [8, 8]);
    }

    #[tokio::test]
    async fn stale_request_is_cancelled_before_network() {
        let latest = AtomicU64::new(5);
        let cfg = AppConfig::default();
        let q = SearchQuery::new("x");
        let out = run_search(&cfg, 4, &latest, CatalogSourceFilter::All, &q).await;
        assert!(out.is_none());
    }

    struct SlowMockProvider(Duration);

    impl ContentProvider for SlowMockProvider {
        fn source(&self) -> CatalogSource {
            CatalogSource::CurseForge
        }

        async fn search(&self, _query: &SearchQuery) -> mq_core::MqResult<Vec<CatalogHit>> {
            tokio::time::sleep(self.0).await;
            Ok(Vec::new())
        }

        async fn get_project(&self, _id: &str) -> mq_core::MqResult<CatalogHit> {
            Err(mq_core::MqError::Network("mock".into()))
        }

        async fn get_versions(
            &self,
            _id: &str,
            _mc: Option<&str>,
            _loader: Option<mq_core::ModLoaderKind>,
        ) -> mq_core::MqResult<Vec<mq_mods::ProjectVersion>> {
            Ok(Vec::new())
        }

        async fn get_version(
            &self,
            _project_id: &str,
            _version_id: &str,
        ) -> mq_core::MqResult<mq_mods::ProjectVersion> {
            Err(mq_core::MqError::Network("mock".into()))
        }

        async fn identify_by_hashes(
            &self,
            _hashes: &[String],
            _fingerprints: &[u32],
        ) -> mq_core::MqResult<std::collections::HashMap<String, mq_mods::ProjectVersion>> {
            Ok(std::collections::HashMap::new())
        }
    }

    #[tokio::test]
    async fn slow_provider_times_out_cleanly_as_network_error() {
        let slow = SlowMockProvider(Duration::from_millis(500));
        let cfg = AppConfig::default();
        let q = SearchQuery::new("sodium");
        let started = std::time::Instant::now();
        let res = search_source_with_timeout(&slow, &q, &cfg, Duration::from_millis(40)).await;
        let err = res.expect_err("slow provider must time out");
        assert_eq!(err.source, CatalogSource::CurseForge);
        assert_eq!(err.kind, CatalogErrorKind::Network);
        assert!(
            started.elapsed() < Duration::from_millis(350),
            "timeout should abort quickly instead of waiting for full upstream hang"
        );
    }
}
