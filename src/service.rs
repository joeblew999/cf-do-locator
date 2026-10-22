//! `locator.v1.LocatorService` implementation.
//!
//! Every RPC reads the snapshot via `load_snapshot`, which keeps the parsed
//! blob per isolate for `SNAPSHOT_TTL_MS` and only then re-reads KV. For
//! high-volume lookup workloads, consumers should still call `ListColos`
//! once per isolate and cache locally — see `examples/ts-client/`.

use std::cell::RefCell;
use std::sync::Arc;

use buffa::MessageField;
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use worker::send::IntoSendFuture;

use crate::proto::locator::v1::{
    GetColoInfoRequest, GetColoInfoResponse, GetLocationHintRequest, GetLocationHintResponse,
    GetSnapshotRequest, GetSnapshotResponse, ListColosRequest, ListColosResponse, LocatorService,
};
use crate::snapshot::{Snapshot, entry_to_pb, hint_str_to_pb, snapshot_to_pb};
use crate::state::SharedState;

pub struct LocatorServer {
    state: SharedState,
}

impl LocatorServer {
    pub fn new(state: SharedState) -> Self {
        Self { state }
    }

    /// Isolate-cached snapshot: KV is read and parsed at most once per
    /// `SNAPSHOT_TTL_MS` per isolate. Re-reading + parsing the ~344-colo blob
    /// on every request was most of the request's CPU (P50 9.0 → 2.9 ms).
    async fn load_snapshot(&self) -> Result<Arc<Snapshot>, ConnectError> {
        let now = worker::Date::now().as_millis() as f64;
        if let Some(snap) = cached_snapshot(now) {
            return Ok(snap);
        }
        let raw = self
            .state
            .kv
            .get(&self.state.snapshot_key)
            .text()
            .into_send()
            .await
            .map_err(|e| ConnectError::internal(format!("kv read: {e}")))?
            .ok_or_else(|| {
                ConnectError::unavailable("snapshot not yet populated; cron has not run")
            })?;
        let snap: Arc<Snapshot> = serde_json::from_str::<Snapshot>(&raw)
            .map_err(|e| ConnectError::internal(format!("snapshot decode: {e}")))?
            .into();
        SNAPSHOT_CACHE.with_borrow_mut(|c| *c = Some((now, Arc::clone(&snap))));
        Ok(snap)
    }
}

/// How long an isolate reuses a parsed snapshot before re-reading KV.
/// KV already serves edge-cached reads for up to 60 s and the snapshot
/// changes weekly, so this adds at most another minute of staleness.
const SNAPSHOT_TTL_MS: f64 = 60_000.0;

thread_local! {
    /// `(fetched_at_ms, snapshot)`. One per isolate (wasm32 is single-threaded).
    static SNAPSHOT_CACHE: RefCell<Option<(f64, Arc<Snapshot>)>> = const { RefCell::new(None) };
}

fn cached_snapshot(now_ms: f64) -> Option<Arc<Snapshot>> {
    SNAPSHOT_CACHE.with_borrow(|c| {
        c.as_ref()
            .filter(|(at, _)| now_ms - at < SNAPSHOT_TTL_MS)
            .map(|(_, snap)| Arc::clone(snap))
    })
}

/// Drop this isolate's cached snapshot (after a refresh wrote a new one).
pub(crate) fn invalidate_snapshot_cache() {
    SNAPSHOT_CACHE.with_borrow_mut(|c| *c = None);
}

impl LocatorService for LocatorServer {
    async fn get_location_hint(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetLocationHintRequest>,
    ) -> ServiceResult<GetLocationHintResponse> {
        let snap = self.load_snapshot().await?;
        let entry = snap.find(request.colo);
        let resp = GetLocationHintResponse {
            hint: hint_str_to_pb(entry.and_then(|e| e.nearest_region.as_deref())),
            known: entry.is_some(),
            ..Default::default()
        };
        Ok(Response::new(resp))
    }

    async fn get_colo_info(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetColoInfoRequest>,
    ) -> ServiceResult<GetColoInfoResponse> {
        let snap = self.load_snapshot().await?;
        let entry = snap.find(request.colo);
        let resp = GetColoInfoResponse {
            colo: entry
                .map(entry_to_pb)
                .map(MessageField::some)
                .unwrap_or_default(),
            known: entry.is_some(),
            ..Default::default()
        };
        Ok(Response::new(resp))
    }

    async fn list_colos(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ListColosRequest>,
    ) -> ServiceResult<ListColosResponse> {
        let snap = self.load_snapshot().await?;
        let resp = ListColosResponse {
            snapshot: MessageField::some(snapshot_to_pb(&snap)),
            colos: snap.colos.iter().map(entry_to_pb).collect(),
            ..Default::default()
        };
        Ok(Response::new(resp))
    }

    async fn get_snapshot(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, GetSnapshotRequest>,
    ) -> ServiceResult<GetSnapshotResponse> {
        let snap = self.load_snapshot().await?;
        let resp = GetSnapshotResponse {
            snapshot: MessageField::some(snapshot_to_pb(&snap)),
            ..Default::default()
        };
        Ok(Response::new(resp))
    }
}
