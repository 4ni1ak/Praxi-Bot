//! Signed, online refresh of the model catalog (`provider-catalog.json`).
//!
//! The only network traffic is a `GET` of two fixed `https://` URLs (the
//! catalog and its minisign signature). No user data is sent. The download is
//! used only if its signature verifies against [`PINNED_PUBLIC_KEY`] (compiled
//! in), it is not older than what is installed (no rollback), it stays inside
//! size/count caps and it passes the same validation as the bundled table.
//! Anything else is refused and the bundled/cached catalog keeps working.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    catalog_freshness::IsoDate,
    model_metadata::{
        CuratedModels, ModelKind, ModelMetadata, ProviderMetadata, embedded_metadata,
    },
};

/// Fixed URL of the signed catalog (a release asset).
pub const CATALOG_URL: &str =
    "https://github.com/emreocell/Praxi-Bot/releases/latest/download/provider-catalog.json";
/// Fixed URL of its minisign signature.
pub const SIGNATURE_URL: &str =
    "https://github.com/emreocell/Praxi-Bot/releases/latest/download/provider-catalog.json.minisig";
/// The maintainer's minisign-compatible public key, pinned at compile time.
/// Rotate it only together with an app release.
pub const PINNED_PUBLIC_KEY: &str = "RWSsBkZmVxltCDpKwjhfa+mwNTgUu4AotVgzS6Wp/jokp6vYxeMYuqZ0";

/// Largest catalog (or signature) we accept, in bytes.
pub const MAX_CATALOG_BYTES: usize = 512 * 1024;
const MAX_SIGNATURE_BYTES: usize = 4 * 1024;
/// Most providers a downloaded catalog may list.
pub const MAX_PROVIDERS: usize = 64;
/// Most models one provider may list.
pub const MAX_MODELS_PER_PROVIDER: usize = 500;
/// Days after which the catalog is called out of date.
pub const STALE_AFTER_DAYS: i64 = 30;
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
const LOOP_TICK: Duration = Duration::from_secs(3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Why an update was refused. The text is meant for the owner.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogUpdateError {
    #[error("the downloaded catalog is larger than {MAX_CATALOG_BYTES} bytes")]
    TooLarge,
    #[error("the catalog signature is not valid: {0}")]
    BadSignature(String),
    #[error("the downloaded catalog is malformed: {0}")]
    Malformed(String),
    #[error("the downloaded catalog ({remote}) is older than the installed one ({current})")]
    Rollback { remote: String, current: String },
    #[error("the downloaded catalog failed validation: {0}")]
    Invalid(String),
    #[error("could not download the catalog: {0}")]
    Network(String),
    #[error("could not store the catalog: {0}")]
    Io(String),
}

// ------------------------------------------------------------ lenient parse

// Mirrors of the table types WITHOUT `deny_unknown_fields`, so a newer
// catalog can add fields without breaking older apps.
#[derive(Deserialize)]
struct WireTable {
    #[serde(alias = "verified_at")]
    last_reviewed: String,
    providers: Vec<WireProvider>,
}

#[derive(Deserialize)]
struct WireProvider {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default, alias = "verified_at")]
    last_reviewed: Option<String>,
    sources: Vec<String>,
    models: Vec<WireModel>,
}

#[derive(Deserialize)]
struct WireModel {
    id: String,
    display_name: String,
    kind: String,
    #[serde(default)]
    recommended: bool,
    #[serde(default)]
    recommended_rank: Option<u8>,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    note: Option<String>,
}

impl WireModel {
    fn into_model(self) -> ModelMetadata {
        // A kind this app does not know yet is shown as "other", not rejected.
        let kind = serde_json::from_value(Value::String(self.kind)).unwrap_or(ModelKind::Other);
        ModelMetadata {
            id: self.id,
            display_name: self.display_name,
            kind,
            recommended: self.recommended,
            recommended_rank: self.recommended_rank,
            deprecated: self.deprecated,
            note: self.note,
        }
    }
}

/// Verify the signature, parse leniently, enforce the caps and validate.
///
/// # Errors
/// [`CatalogUpdateError`] for a too-large, unsigned/tampered, malformed or
/// invalid catalog.
pub fn verify_and_parse(
    data: &[u8],
    signature: &str,
    public_key: &str,
) -> Result<CuratedModels, CatalogUpdateError> {
    if data.len() > MAX_CATALOG_BYTES || signature.len() > MAX_SIGNATURE_BYTES {
        return Err(CatalogUpdateError::TooLarge);
    }
    let key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| CatalogUpdateError::BadSignature(format!("pinned key unusable: {e}")))?;
    let sig = minisign_verify::Signature::decode(signature)
        .map_err(|e| CatalogUpdateError::BadSignature(e.to_string()))?;
    key.verify(data, &sig, false)
        .map_err(|e| CatalogUpdateError::BadSignature(e.to_string()))?;

    let wire: WireTable =
        serde_json::from_slice(data).map_err(|e| CatalogUpdateError::Malformed(e.to_string()))?;
    if wire.providers.len() > MAX_PROVIDERS
        || wire
            .providers
            .iter()
            .any(|p| p.models.len() > MAX_MODELS_PER_PROVIDER)
    {
        return Err(CatalogUpdateError::Malformed(
            "too many providers or models".into(),
        ));
    }
    let table = CuratedModels {
        last_reviewed: wire.last_reviewed,
        providers: wire
            .providers
            .into_iter()
            .map(|p| ProviderMetadata {
                id: p.id,
                aliases: p.aliases,
                last_reviewed: p.last_reviewed,
                sources: p.sources,
                models: p.models.into_iter().map(WireModel::into_model).collect(),
            })
            .collect(),
    };
    table.validate().map_err(CatalogUpdateError::Invalid)?;
    Ok(table)
}

// -------------------------------------------------------------------- merge

/// Lay `remote` over `base`: a provider from `remote` replaces the bundled one
/// when its review date is the same or newer; providers only in one side are
/// kept. The result must validate, otherwise the update is refused.
///
/// # Errors
/// [`CatalogUpdateError::Invalid`] when the merged table is not consistent
/// (for example an alias clash).
pub fn merge(
    base: &CuratedModels,
    remote: &CuratedModels,
) -> Result<CuratedModels, CatalogUpdateError> {
    let date_of =
        |t: &CuratedModels, p: &ProviderMetadata| IsoDate::parse(t.effective_last_reviewed(p)).ok();
    let mut providers = base.providers.clone();
    for rp in &remote.providers {
        let rdate = date_of(remote, rp);
        // Stamp the effective date so it survives the table-level date.
        let mut incoming = rp.clone();
        incoming.last_reviewed = Some(remote.effective_last_reviewed(rp).to_string());
        match providers.iter().position(|b| b.id == rp.id) {
            Some(i) => {
                if rdate >= date_of(base, &providers[i]) {
                    providers[i] = incoming;
                }
            }
            None => providers.push(incoming),
        }
    }
    let newest = std::cmp::max(
        IsoDate::parse(&base.last_reviewed).ok(),
        IsoDate::parse(&remote.last_reviewed).ok(),
    )
    .map_or_else(|| base.last_reviewed.clone(), |d| d.to_string());
    let merged = CuratedModels {
        last_reviewed: newest,
        providers,
    };
    merged.validate().map_err(CatalogUpdateError::Invalid)?;
    Ok(merged)
}

/// Refuse `remote` when it is older than what is installed.
///
/// # Errors
/// [`CatalogUpdateError::Rollback`].
pub fn check_not_rollback(
    installed: &CuratedModels,
    remote: &CuratedModels,
) -> Result<(), CatalogUpdateError> {
    match (
        IsoDate::parse(&remote.last_reviewed),
        IsoDate::parse(&installed.last_reviewed),
    ) {
        (Ok(r), Ok(c)) if r < c => Err(CatalogUpdateError::Rollback {
            remote: r.to_string(),
            current: c.to_string(),
        }),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------ active table

struct Active {
    table: Arc<CuratedModels>,
    /// Review date of the downloaded catalog in use, if any.
    online_date: Option<String>,
}

fn active_cell() -> &'static RwLock<Option<Active>> {
    static CELL: OnceLock<RwLock<Option<Active>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

/// The catalog in effect: the bundled table, or the bundled table with a
/// verified online update laid over it.
#[must_use]
pub fn active_metadata() -> Arc<CuratedModels> {
    active_cell()
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|a| Arc::clone(&a.table)))
        .unwrap_or_else(|| Arc::new(embedded_metadata().clone()))
}

/// Review date of the online catalog in use (`None` = bundled only).
#[must_use]
pub fn online_catalog_date() -> Option<String> {
    active_cell()
        .read()
        .ok()
        .and_then(|g| g.as_ref().and_then(|a| a.online_date.clone()))
}

fn install_active(table: CuratedModels, online_date: Option<String>) {
    if let Ok(mut g) = active_cell().write() {
        *g = Some(Active {
            table: Arc::new(table),
            online_date,
        });
    }
}

/// Human line for the doctor / UI: when the catalog was updated and whether
/// it is getting old. `now` is injectable for tests.
#[must_use]
pub fn status_line(table: &CuratedModels, online: Option<&str>, now: SystemTime) -> (String, bool) {
    let date = online.unwrap_or(&table.last_reviewed);
    let stale = match (IsoDate::from_system_time(now), IsoDate::parse(date)) {
        (Some(today), Ok(d)) => today.days_since(d) > STALE_AFTER_DAYS,
        _ => false,
    };
    let source = if online.is_some() {
        "updated"
    } else {
        "bundled, reviewed"
    };
    (format!("Catalog {source} {date}"), stale)
}

// ------------------------------------------------------------------- store

/// Settings and cache under `<data_dir>/provider-catalog/`.
#[derive(Debug, Clone)]
pub struct CatalogStore {
    dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreSettings {
    #[serde(default = "yes")]
    auto_update: bool,
}

impl Default for StoreSettings {
    fn default() -> Self {
        Self { auto_update: true }
    }
}

const fn yes() -> bool {
    true
}

/// What the last check did, for `bot catalog update` and the doctor.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogState {
    /// Unix seconds of the last attempt.
    pub last_checked: u64,
    /// Review date of the installed online catalog.
    pub last_updated: Option<String>,
    /// Why the last attempt failed, if it did.
    pub last_error: Option<String>,
}

impl CatalogStore {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("provider-catalog"),
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Whether the owner allows the daily check. Default ON;
    /// `BOT_CATALOG_UPDATES=off` always wins.
    #[must_use]
    pub fn auto_update_enabled(&self) -> bool {
        if let Ok(v) = std::env::var("BOT_CATALOG_UPDATES")
            && matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        {
            return false;
        }
        std::fs::read_to_string(self.file("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<StoreSettings>(&t).ok())
            .unwrap_or_default()
            .auto_update
    }

    /// Switch the daily check on or off.
    ///
    /// # Errors
    /// [`CatalogUpdateError::Io`] when the file cannot be written.
    pub fn set_auto_update(&self, on: bool) -> Result<(), CatalogUpdateError> {
        let text = serde_json::to_string_pretty(&StoreSettings { auto_update: on })
            .map_err(|e| CatalogUpdateError::Io(e.to_string()))?;
        self.write("settings.json", text.as_bytes())
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), CatalogUpdateError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| CatalogUpdateError::Io(e.to_string()))?;
        let tmp = self.file(&format!("{name}.tmp"));
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, self.file(name)))
            .map_err(|e| CatalogUpdateError::Io(e.to_string()))
    }

    /// The last recorded attempt.
    #[must_use]
    pub fn state(&self) -> CatalogState {
        std::fs::read_to_string(self.file("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, state: &CatalogState) {
        if let Ok(text) = serde_json::to_string_pretty(state) {
            let _ = self.write("state.json", text.as_bytes());
        }
    }

    /// Whether 24 hours passed since the last attempt.
    #[must_use]
    pub fn check_due(&self, now: SystemTime) -> bool {
        let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        now.saturating_sub(self.state().last_checked) >= CHECK_EVERY.as_secs()
    }

    /// The cached online catalog, re-verified on every read (a file edited on
    /// disk is ignored).
    #[must_use]
    pub fn load_cached(&self, public_key: &str) -> Option<CuratedModels> {
        let data = std::fs::read(self.file("provider-catalog.json")).ok()?;
        let sig = std::fs::read_to_string(self.file("provider-catalog.json.minisig")).ok()?;
        verify_and_parse(&data, &sig, public_key).ok()
    }

    /// Install the cached catalog (if any) over the bundled one. Call once at
    /// start-up; cheap and offline.
    pub fn activate_cached(&self) {
        self.activate_cached_with(PINNED_PUBLIC_KEY);
    }

    fn activate_cached_with(&self, key: &str) {
        let bundled = embedded_metadata();
        if let Some(remote) = self.load_cached(key)
            && let Ok(merged) = merge(bundled, &remote)
        {
            install_active(merged, Some(remote.last_reviewed));
        }
    }
}

// ------------------------------------------------------------------ update

/// Result of one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum UpdateOutcome {
    /// Nothing newer than what is installed.
    UpToDate { installed: String },
    /// A newer verified catalog exists (`--check`), or was installed.
    Newer {
        from: String,
        to: String,
        installed: bool,
    },
}

/// Download, verify and (when `install`) store the catalog.
///
/// # Errors
/// [`CatalogUpdateError`]; the installed catalog is never touched on error.
pub async fn check_for_update(
    store: &CatalogStore,
    now: SystemTime,
    install: bool,
) -> Result<UpdateOutcome, CatalogUpdateError> {
    let result = run_check(
        store,
        install,
        CATALOG_URL,
        SIGNATURE_URL,
        PINNED_PUBLIC_KEY,
        false,
    )
    .await;
    let mut state = store.state();
    state.last_checked = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match &result {
        Ok(UpdateOutcome::Newer {
            to,
            installed: true,
            ..
        }) => {
            state.last_updated = Some(to.clone());
            state.last_error = None;
        }
        Ok(_) => state.last_error = None,
        Err(e) => state.last_error = Some(e.to_string()),
    }
    store.save_state(&state);
    result
}

async fn run_check(
    store: &CatalogStore,
    install: bool,
    url: &str,
    sig_url: &str,
    key: &str,
    allow_http: bool,
) -> Result<UpdateOutcome, CatalogUpdateError> {
    let data = fetch(url, MAX_CATALOG_BYTES, allow_http).await?;
    let sig = String::from_utf8(fetch(sig_url, MAX_SIGNATURE_BYTES, allow_http).await?)
        .map_err(|_| CatalogUpdateError::BadSignature("signature is not text".into()))?;
    let remote = verify_and_parse(&data, &sig, key)?;
    let bundled = embedded_metadata();
    let installed = match store.load_cached(key) {
        Some(c) => merge(bundled, &c)?,
        None => bundled.clone(),
    };
    check_not_rollback(&installed, &remote)?;
    let merged = merge(bundled, &remote)?;
    let (from, to) = (
        installed.last_reviewed.clone(),
        merged.last_reviewed.clone(),
    );
    if merged == installed {
        return Ok(UpdateOutcome::UpToDate { installed: from });
    }
    if install {
        store.write("provider-catalog.json", &data)?;
        store.write("provider-catalog.json.minisig", sig.as_bytes())?;
        install_active(merged, Some(remote.last_reviewed));
    }
    Ok(UpdateOutcome::Newer {
        from,
        to,
        installed: install,
    })
}

async fn fetch(url: &str, cap: usize, allow_http: bool) -> Result<Vec<u8>, CatalogUpdateError> {
    use futures::StreamExt;
    if !(url.starts_with("https://") || (allow_http && url.starts_with("http://"))) {
        return Err(CatalogUpdateError::Network("only https is allowed".into()));
    }
    let net = |e: reqwest::Error| CatalogUpdateError::Network(e.without_url().to_string());
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(move |a| {
            if a.previous().len() > 3 || !(allow_http || a.url().scheme() == "https") {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .user_agent("praxi-bot-catalog")
        .build()
        .map_err(net)?;
    let resp = client.get(url).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(CatalogUpdateError::Network(format!(
            "HTTP {}",
            resp.status()
        )));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(net)?);
        if body.len() > cap {
            return Err(CatalogUpdateError::TooLarge);
        }
    }
    Ok(body)
}

/// Background loop for the daemon: at most one check per 24 hours, only while
/// the owner allows it. Failures are recorded, never fatal.
pub async fn daily_loop(store: CatalogStore) {
    loop {
        let now = SystemTime::now();
        if store.auto_update_enabled() && store.check_due(now) {
            let _ = check_for_update(&store, now, true).await;
        }
        tokio::time::sleep(LOOP_TICK).await;
    }
}

#[cfg(test)]
mod tests;
