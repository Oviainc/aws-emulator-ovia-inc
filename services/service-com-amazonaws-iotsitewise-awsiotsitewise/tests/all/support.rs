// SPDX-License-Identifier: CC0-1.0

//! The clock helpers every property-value suite in this crate shares.
//!
//! `BatchPutAssetPropertyValue` ingests only TQVs inside
//! `[now - 7 days, now + 10 minutes]`, so no suite can seed a series at a fixed
//! calendar second. A suite either anchors its timestamps on [`now_seconds`]
//! directly, or — when its tables read better with the fixed timestamps they were
//! written against — keeps them and converts at the wire boundary with
//! [`anchored`] / [`relative`], which translate a table second into the accepted
//! window and back. The anchor is computed ONCE per test binary, so two suites
//! cannot drift about where "now" is.
//!
//! The wire harness itself (`router`, `call`, `create_model`, …) lives in
//! `super::asset_lifecycle`, which is this crate's harness module.
//!
//! [`ensure_workspace`] lives here for the same reason: every workspace-scoped
//! create resolves its `workspaceName` through the workspace registry, so a suite
//! working inside a workspace opens it first, and one definition of that step is
//! what keeps the suites from drifting on it.

use serde_json::json;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The property definitions `properties`, each carrying the id the asset model
/// `model_id` already holds for a property of that NAME.
///
/// `UpdateAssetModel` replaces the whole model, and an id-less definition whose
/// name an existing property already holds would RE-CREATE that property under a
/// new id — losing the id every time series, variable reference and ingest path
/// addressing it names — so the service refuses one. A suite that resubmits a
/// stored property in order to KEEP it therefore names it by its stored id, which
/// is what a client of the real service does; a definition the model holds no
/// property of that name for is new and stays as written.
pub(crate) async fn named_by_stored_id(
    router: &MultiServiceRouter,
    model_id: &str,
    properties: Value,
) -> Value {
    let (status, described) = crate::asset_lifecycle::call(
        router,
        "GET",
        &format!("/asset-models/{model_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel({model_id}): got {status} {described}"
    );
    let stored = described["assetModelProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut named = properties.as_array().cloned().unwrap_or_default();
    for property in &mut named {
        if property.get("id").is_some() {
            continue;
        }
        if let Some(held) = stored.iter().find(|held| held["name"] == property["name"]) {
            property["id"] = held["id"].clone();
        }
    }
    Value::Array(named)
}

/// Open workspace `name` unless this router already holds it.
///
/// `CreateWorkspace` is what puts a workspace in the registry every
/// workspace-scoped create resolves against, so a suite that creates a pipeline,
/// task, dataset or export job inside a workspace calls this first. A second call
/// for a name already open is the modelled conflict (HTTP 409), which leaves the
/// workspace exactly as this caller needs it — so a per-create helper can call it
/// unconditionally without tracking what it has already opened.
pub(crate) async fn ensure_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = crate::asset_lifecycle::call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert!(
        status == 200 || status == 409,
        "workspace {name:?}: expected it to be open, got {status} {body}"
    );
}

/// The current second in the Unix epoch.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the Unix epoch")
        .as_secs() as i64
}

/// How far back of `now_seconds()` an anchored table lands.
///
/// Far enough inside the accepted window that a run taking several seconds cannot
/// drift out of it, and close enough that a table spanning a few thousand seconds
/// forward of the anchor stays in the past.
const ANCHOR_AGE: i64 = 10_000;

/// Where an anchored table's own epoch lands, fixed once for the test binary.
pub fn anchor() -> i64 {
    static ANCHOR: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ANCHOR.get_or_init(|| now_seconds() - ANCHOR_AGE)
}

/// A table second, measured against `row_epoch`, as the absolute `timeInSeconds`
/// the wire carries.
pub fn anchored(row_epoch: i64, seconds: i64) -> i64 {
    anchor() + (seconds - row_epoch)
}

/// The inverse of [`anchored`]: an absolute `timeInSeconds` as the table second it
/// was written from.
pub fn relative(row_epoch: i64, seconds: i64) -> i64 {
    row_epoch + (seconds - anchor())
}
