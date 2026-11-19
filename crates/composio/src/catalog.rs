//! The app catalog the Apps modal shows: bundled popular apps at once, the full
//! Composio catalog from a disk cache, refreshed in the background.  Nothing
//! here is on the request path of the UI: a view is built from memory only, so
//! it never blocks and works offline.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::client::Toolkit;
use crate::curated::{Curated, group_of};
use crate::error::{ComposioError, Result};

/// File name inside the data directory.  Public catalog data only.
pub const CATALOG_FILE: &str = "composio-catalog.json";

/// A cache older than this is refreshed in the background.
pub const STALE_AFTER_SECS: u64 = 6 * 60 * 60;

/// Largest cache read or written (a catalog of ~2000 toolkits is ~1 MB).
const MAX_FILE_BYTES: usize = 6 * 1024 * 1024;

/// What the cache file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogFile {
    /// Unix seconds of the last complete refresh.
    pub updated_at: u64,
    /// Toolkits in Composio's popularity order.
    pub items: Vec<Toolkit>,
}

/// Where the shown list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// Only the apps shipped with Praxi (no refresh has finished yet).
    Bundled,
    /// The disk cache of Composio's catalog.
    Cache,
}

/// One filtered page of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogView {
    /// Toolkits and their chip group, popular apps first.
    pub items: Vec<(Toolkit, &'static str)>,
    /// Offset cursor of the next page.
    pub next_cursor: Option<String>,
    /// Where the list comes from.
    pub source: CatalogSource,
    /// Unix seconds of the cache, when there is one.
    pub updated_at: Option<u64>,
}

#[must_use]
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl CatalogFile {
    /// Read the cache; a missing, oversized or broken file is simply no cache.
    #[must_use]
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() > MAX_FILE_BYTES {
            return None;
        }
        let file: Self = serde_json::from_slice(&bytes).ok()?;
        let clean = file
            .items
            .iter()
            .all(|t| crate::surface::toolkit_allowed(&t.slug));
        (clean && !file.items.is_empty()).then_some(file)
    }

    /// Write atomically (temp file + rename).
    ///
    /// # Errors
    /// I/O errors.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_vec(self)
            .map_err(|e| ComposioError::Internal(format!("{CATALOG_FILE}: {e}")))?;
        if text.len() > MAX_FILE_BYTES {
            return Err(ComposioError::Internal(format!(
                "{CATALOG_FILE}: too large"
            )));
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| ComposioError::Internal(format!("{CATALOG_FILE}: {e}")))
    }

    /// Whether a background refresh is due.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        now_secs().saturating_sub(self.updated_at) > STALE_AFTER_SECS
    }
}

fn bundled(curated: &Curated) -> Vec<Toolkit> {
    curated
        .popular
        .iter()
        .map(|p| Toolkit {
            slug: p.slug.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            categories: Vec::new(),
            tools_count: None,
            logo: None,
        })
        .collect()
}

/// Popular apps first (live data where the cache has it, bundled otherwise),
/// then the rest of the cache in Composio's order.
fn ordered(curated: &Curated, cache: Option<&CatalogFile>) -> Vec<Toolkit> {
    let Some(cache) = cache else {
        return bundled(curated);
    };
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(cache.items.len() + curated.popular.len());
    for b in bundled(curated) {
        let live = cache.items.iter().find(|t| t.slug == b.slug);
        let mut t = live.cloned().unwrap_or(b);
        if t.description.is_empty()
            && let Some(p) = curated.popular.iter().find(|p| p.slug == t.slug)
        {
            t.description.clone_from(&p.description);
        }
        seen.insert(t.slug.clone());
        out.push(t);
    }
    out.extend(
        cache
            .items
            .iter()
            .filter(|t| seen.insert(t.slug.clone()))
            .cloned(),
    );
    out
}

/// A filtered, paged view.  `group` is a chip id (`all` or empty = no filter).
#[must_use]
pub fn view(
    curated: &Curated,
    cache: Option<&CatalogFile>,
    search: Option<&str>,
    group: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
) -> CatalogView {
    let needle = search
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let group = group.filter(|g| !g.is_empty() && *g != "all");
    let mut hits: Vec<(Toolkit, &'static str, u8)> = Vec::new();
    for t in ordered(curated, cache) {
        let g = group_of(curated, &t.slug, &t.categories);
        if group.is_some_and(|want| want != g) {
            continue;
        }
        let rank = match &needle {
            None => 0,
            Some(n) => {
                let name = t.name.to_lowercase();
                if name.starts_with(n.as_str()) || t.slug.starts_with(n.as_str()) {
                    0
                } else if name.contains(n.as_str()) || t.slug.contains(n.as_str()) {
                    1
                } else if t.description.to_lowercase().contains(n.as_str())
                    || t.categories
                        .iter()
                        .any(|c| c.to_lowercase().contains(n.as_str()))
                {
                    2
                } else {
                    continue;
                }
            }
        };
        hits.push((t, g, rank));
    }
    // Stable: ties keep the popular-first / usage order.
    hits.sort_by_key(|h| h.2);
    let start = cursor
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(hits.len());
    let limit = limit.clamp(1, 100);
    let end = (start + limit).min(hits.len());
    let next_cursor = (end < hits.len()).then(|| end.to_string());
    CatalogView {
        items: hits.drain(start..end).map(|(t, g, _)| (t, g)).collect(),
        next_cursor,
        source: if cache.is_some() {
            CatalogSource::Cache
        } else {
            CatalogSource::Bundled
        },
        updated_at: cache.map(|c| c.updated_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(slug: &str, name: &str, cats: &[&str]) -> Toolkit {
        Toolkit {
            slug: slug.into(),
            name: name.into(),
            description: format!("{name} things"),
            categories: cats.iter().map(|c| (*c).to_owned()).collect(),
            tools_count: Some(3),
            logo: None,
        }
    }

    #[test]
    fn bundled_list_needs_no_cache_and_leads_with_popular_apps() {
        let c = Curated::builtin();
        let v = view(&c, None, None, None, None, 10);
        assert_eq!(v.source, CatalogSource::Bundled);
        assert_eq!(v.items[0].0.slug, "gmail");
        assert!(v.items.iter().all(|(t, _)| !t.description.is_empty()));
        assert!(v.next_cursor.is_some());
    }

    #[test]
    fn cache_search_group_and_paging_work_locally() {
        let c = Curated::builtin();
        let cache = CatalogFile {
            updated_at: now_secs(),
            items: vec![
                tk("zzcrm", "Zed CRM", &["CRM"]),
                tk("gmail", "Gmail", &["email"]),
                tk("ledger", "Ledger Pro", &["accounting"]),
            ],
        };
        let v = view(&c, Some(&cache), Some("zed"), None, None, 10);
        assert_eq!(v.source, CatalogSource::Cache);
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].1, "business");
        let fin = view(&c, Some(&cache), None, Some("finance"), None, 100);
        assert!(fin.items.iter().any(|(t, _)| t.slug == "ledger"));
        let p1 = view(&c, Some(&cache), None, None, None, 2);
        let p2 = view(&c, Some(&cache), None, None, p1.next_cursor.as_deref(), 2);
        assert_ne!(p1.items[0].0.slug, p2.items[0].0.slug);
    }

    #[test]
    fn file_round_trips_and_a_broken_file_is_no_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CATALOG_FILE);
        assert!(CatalogFile::load(&path).is_none());
        let f = CatalogFile {
            updated_at: 5,
            items: vec![tk("gmail", "Gmail", &[])],
        };
        f.save(&path).unwrap();
        assert_eq!(CatalogFile::load(&path), Some(f));
        std::fs::write(&path, "{ nope").unwrap();
        assert!(CatalogFile::load(&path).is_none());
    }
}
