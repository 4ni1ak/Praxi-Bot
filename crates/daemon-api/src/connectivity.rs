//! Host-side provider reachability, available without a desktop browser.

use axum::{Extension, Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// One configured model service. No URLs, credentials or provider error bodies.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceConnectivity {
    pub id: String,
    pub local: bool,
    pub reachable: bool,
}

/// Empty service lists mean unknown, never an inferred offline state.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ConnectivityReport {
    pub services: Vec<ServiceConnectivity>,
}

#[utoipa::path(get, path = "/v1/connectivity", tag = "providers",
    security(("bearer" = [])), responses((status = 200, body = ConnectivityReport),
    (status = 403, body = crate::error::ErrorBody)))]
pub(crate) async fn get_connectivity(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
    headers: HeaderMap,
) -> Result<Json<ConnectivityReport>, ApiError> {
    if let Some(Extension(peer)) = peer {
        if peer.role != crate::PeerRole::Desktop {
            return Err(ApiError::forbidden(
                "Model connectivity is available to the owner's desktop only",
            ));
        }
    } else {
        s.require_owner(&headers)?;
    }
    let monitor = s.agent().providers().connectivity;
    let services = monitor
        .check()
        .await
        .into_iter()
        .map(|r| ServiceConnectivity {
            id: r.id,
            local: r.local,
            reachable: r.reachable,
        })
        .collect();
    Ok(Json(ConnectivityReport { services }))
}
