//! Composio app connections of `bot serve`: plugs the `composio` crate into the
//! daemon's `/v1/composio/*` routes ([`daemon_api::ComposioControl`]) and the
//! tool registry.  The API key lives in the secret store under the reserved
//! `composio/` prefix; nothing here ever returns, logs or audits it.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use composio::{
    AppView, Audit, ComposioError, ComposioService, ConnState, KEY_SETUP_URL, LocalKeySource,
    Redact, ServiceConfig, Toolkit,
};
use daemon_api::composio::{
    ComposioAppDto, ComposioCatalogDto, ComposioCatalogQuery, ComposioConnState,
    ComposioConnectDto, ComposioDisconnectDto, ComposioStatusDto, ComposioToolkitDto,
};
use daemon_api::{ComposioControl, ComposioControlError};
use secret_store::SecretStore;
use tool_registry::ToolRegistry;

const DEFAULT_PAGE: u32 = 48;

/// The Composio service behind the daemon API.
pub struct ComposioHost {
    svc: ComposioService,
}

/// Audit rows of the tools and the connect flow, written through the daemon.
pub struct DaemonAudit<F>(pub F);

impl<F> Audit for DaemonAudit<F>
where
    F: Fn(&str, &serde_json::Value) -> Result<(), String> + Send + Sync,
{
    fn record(&self, action: &str, detail: serde_json::Value) -> Result<(), String> {
        (self.0)(action, &detail)
    }
}

impl ComposioHost {
    /// Load `composio.toml` and register the search/run tools.  Connected apps
    /// are refreshed in the background.  Must be called inside a tokio runtime.
    ///
    /// # Errors
    /// `composio.toml` exists but cannot be parsed.
    pub fn start(
        data_dir: &Path,
        registry: ToolRegistry,
        secrets: Arc<dyn SecretStore>,
        redact: Redact,
        audit: Arc<dyn Audit>,
    ) -> Result<Arc<Self>, String> {
        let svc = ComposioService::new(ServiceConfig {
            data_dir: data_dir.to_path_buf(),
            keys: Arc::new(LocalKeySource::new(secrets)),
            registry,
            redact,
            audit,
            loopback_base: None,
        })
        .map_err(|e| e.to_string())?;
        let bg = svc.clone();
        tokio::spawn(async move { bg.refresh_all().await });
        Ok(Arc::new(Self { svc }))
    }

    fn app_dto(v: AppView) -> ComposioAppDto {
        ComposioAppDto {
            slug: v.slug,
            name: v.name,
            state: conn(v.state),
            enabled: v.enabled,
            native_first: v.native_first,
            bots: v.bots,
            tools: u32::try_from(v.tools).unwrap_or(u32::MAX),
        }
    }

    fn toolkit_dto(&self, t: &Toolkit, apps: &[AppView]) -> ComposioToolkitDto {
        let mine = apps.iter().find(|a| a.slug == t.slug);
        ComposioToolkitDto {
            slug: t.slug.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            categories: t.categories.clone(),
            tools_count: t.tools_count,
            native_first: self.svc.curated().is_native_first(&t.slug),
            state: mine.map_or(ComposioConnState::NotConnected, |a| conn(a.state)),
            enabled: mine.is_some_and(|a| a.enabled),
        }
    }
}

fn conn(s: ConnState) -> ComposioConnState {
    match s {
        ConnState::NotConnected => ComposioConnState::NotConnected,
        ConnState::Pending => ComposioConnState::Pending,
        ConnState::Active => ComposioConnState::Active,
        ConnState::Expired => ComposioConnState::Expired,
    }
}

fn map_err(e: ComposioError) -> ComposioControlError {
    match e {
        ComposioError::NotConfigured => ComposioControlError::NotConfigured,
        ComposioError::Invalid(m) => ComposioControlError::Invalid(m),
        ComposioError::NotFound(m) => ComposioControlError::NotFound(m),
        e @ ComposioError::RateLimited { .. } => ComposioControlError::RateLimited(e.to_string()),
        e @ (ComposioError::Unauthorized
        | ComposioError::Upstream(_)
        | ComposioError::Transport(_)) => ComposioControlError::Upstream(e.to_string()),
        ComposioError::Internal(m) => ComposioControlError::Internal(m),
    }
}

#[async_trait]
impl ComposioControl for ComposioHost {
    fn status(&self) -> ComposioStatusDto {
        ComposioStatusDto {
            configured: self.svc.configured(),
            key_setup_url: KEY_SETUP_URL.to_owned(),
        }
    }

    async fn set_key(&self, key: &str) -> Result<(), ComposioControlError> {
        self.svc.set_key(key).await.map_err(map_err)
    }

    async fn remove_key(&self) -> Result<(), ComposioControlError> {
        self.svc.remove_key().await.map_err(map_err)
    }

    async fn catalog(
        &self,
        q: ComposioCatalogQuery,
    ) -> Result<ComposioCatalogDto, ComposioControlError> {
        let unfiltered = q.search.is_none() && q.category.is_none() && q.cursor.is_none();
        let page = self
            .svc
            .toolkits(
                q.search.as_deref(),
                q.category.as_deref(),
                q.cursor.as_deref(),
                q.limit.unwrap_or(DEFAULT_PAGE),
            )
            .await
            .map_err(map_err)?;
        let apps = self.svc.apps();
        let popular = if unfiltered {
            self.svc
                .curated()
                .popular
                .iter()
                .map(|p| {
                    let live = page.items.iter().find(|t| t.slug == p.slug);
                    let base = live.cloned().unwrap_or_else(|| Toolkit {
                        slug: p.slug.clone(),
                        name: p.name.clone(),
                        description: String::new(),
                        categories: Vec::new(),
                        tools_count: None,
                    });
                    self.toolkit_dto(&base, &apps)
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(ComposioCatalogDto {
            popular,
            items: page
                .items
                .iter()
                .map(|t| self.toolkit_dto(t, &apps))
                .collect(),
            next_cursor: page.next_cursor,
        })
    }

    fn apps(&self) -> Vec<ComposioAppDto> {
        self.svc.apps().into_iter().map(Self::app_dto).collect()
    }

    async fn app(&self, slug: &str) -> Result<ComposioAppDto, ComposioControlError> {
        self.svc
            .app_status(slug)
            .await
            .map(Self::app_dto)
            .map_err(map_err)
    }

    async fn connect(&self, slug: &str) -> Result<ComposioConnectDto, ComposioControlError> {
        let link = self.svc.connect(slug).await.map_err(map_err)?;
        Ok(ComposioConnectDto {
            redirect_url: link.redirect_url,
            expires_at: link.expires_at,
        })
    }

    async fn set_enabled(
        &self,
        slug: &str,
        enabled: bool,
    ) -> Result<ComposioAppDto, ComposioControlError> {
        self.svc
            .set_enabled(slug, enabled)
            .await
            .map(Self::app_dto)
            .map_err(map_err)
    }

    async fn disconnect(&self, slug: &str) -> Result<ComposioDisconnectDto, ComposioControlError> {
        let remote_removed = self.svc.disconnect(slug).await.map_err(map_err)?;
        Ok(ComposioDisconnectDto { remote_removed })
    }
}
