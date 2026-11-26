//! A per-call guard the loop installs around a tool that fetches web addresses on its own,
//! so the owner's site rules also hold for the address that was asked for and for every
//! redirect hop the fetcher follows (the loop cannot see those from outside).
//!
//! The guard lives in a task-local: it is only in force while the loop awaits the tool
//! inside [`with_url_guard`]. A fetcher calls [`url_blocked`] for the requested address and
//! for every redirect target BEFORE it connects. Nothing is installed when the bot has no
//! rule, so the check costs nothing then.

use std::future::Future;
use std::sync::Arc;

/// Decides whether the web tools may contact an address.
pub trait UrlGuard: Send + Sync {
    /// `true` when `url` must not be contacted. `redirect_hop` is `true` for the target of a
    /// redirect (a hop cannot be put to the owner for approval, so it is judged stricter).
    fn blocked(&self, url: &str, redirect_hop: bool) -> bool;
}

tokio::task_local! {
    static URL_GUARD: Arc<dyn UrlGuard>;
}

/// Run `fut` with `guard` in force for every [`url_blocked`] call made inside it.
pub async fn with_url_guard<F: Future>(guard: Arc<dyn UrlGuard>, fut: F) -> F::Output {
    URL_GUARD.scope(guard, fut).await
}

/// Whether the installed guard refuses `url`. `false` when no guard is installed.
#[must_use]
pub fn url_blocked(url: &str, redirect_hop: bool) -> bool {
    URL_GUARD
        .try_with(|g| g.blocked(url, redirect_hop))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Deny(&'static str);
    impl UrlGuard for Deny {
        fn blocked(&self, url: &str, _hop: bool) -> bool {
            url.contains(self.0)
        }
    }

    #[tokio::test]
    async fn the_guard_is_only_in_force_inside_the_scope() {
        assert!(!url_blocked("https://bank.example/", false));
        let seen = with_url_guard(Arc::new(Deny("bank.example")), async {
            (
                url_blocked("https://bank.example/", false),
                url_blocked("https://ok.example/", true),
            )
        })
        .await;
        assert_eq!(seen, (true, false));
        assert!(!url_blocked("https://bank.example/", false));
    }
}
