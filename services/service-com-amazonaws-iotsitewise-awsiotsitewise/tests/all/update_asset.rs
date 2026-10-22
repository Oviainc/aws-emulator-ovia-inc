// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAsset`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `UpdateAsset` REPLACES the caller-owned attributes
//! (`assetName`, `assetDescription`, and `assetExternalId` when the asset has none)
//! of the stored asset the request's `CustomID` label names, reports the asset in
//! `UPDATING` until the transition that call owes falls due, and writes nothing at
//! all when it refuses — so every later read answers with exactly what the last
//! accepted update wrote.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

// The wire harness and the model fixture this crate's other suites already have:
// one router, one `call`, one `create_model`, so every suite talks to the service
// exactly the same way.
use super::asset_lifecycle::{call, create_model, error_code, router};

/// Create an asset and return its id.
///
/// The shared `asset_lifecycle::create_asset` fixture posts only a name and a
/// model; this suite needs the two OPTIONAL members an update replaces
/// (`assetDescription`) or assigns (`assetExternalId`) to be set at create time,
/// so it builds the body here.
async fn create_asset(
    router: &MultiServiceRouter,
    asset_model_id: &str,
    name: &str,
    description: Option<&str>,
    external_id: Option<&str>,
) -> String {
    let mut body = json!({ "assetName": name, "assetModelId": asset_model_id });
    if let Some(description) = description {
        body["assetDescription"] = json!(description);
    }
    if let Some(external_id) = external_id {
        body["assetExternalId"] = json!(external_id);
    }
    let (status, created) = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(status, 202, "create asset {name}: got {status} {created}");
    created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {name}: no assetId in {created}"))
        .to_string()
}

/// `DescribeAsset`'s body for `label`, which must answer 200.
async fn describe(router: &MultiServiceRouter, label: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{label}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {label}: got {status} {body}");
    body
}

/// A modelled `Timestamp` member as epoch seconds.
fn epoch_seconds(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not an epoch-seconds number in {body}"))
}

/// One row of the update table: how the asset is addressed and what the update
/// carries.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `true` when the update addresses the asset by its `externalId:` label
    /// rather than by its raw UUID.
    by_external_id: bool,
    new_name: String,
    /// The description the update carries; `None` means the member is omitted,
    /// which clears the stored one.
    new_description: Option<String>,
}

/// The rows span what the rule generalises over: which `CustomID` label form names
/// the asset, the form and length of the replacement name (the modelled `Name`
/// bounds come from `smithy_query(query="operation", operation="UpdateAsset")`:
/// length 1..256, no control characters), and whether the request carries a
/// description at all.
fn update_rows() -> Vec<Row> {
    vec![
        Row {
            case: "rename by raw id, description replaced",
            by_external_id: false,
            new_name: "Boiler 2".to_string(),
            new_description: Some("the renamed boiler".to_string()),
        },
        Row {
            case: "rename by externalId alias",
            by_external_id: true,
            new_name: "Boiler 3".to_string(),
            new_description: Some("addressed by its external id".to_string()),
        },
        Row {
            case: "description omitted, so cleared",
            by_external_id: false,
            new_name: "Boiler 4".to_string(),
            new_description: None,
        },
        Row {
            case: "name unchanged, description replaced",
            by_external_id: false,
            new_name: "initial".to_string(),
            new_description: Some("only the description moved".to_string()),
        },
        Row {
            case: "non-ascii name",
            by_external_id: false,
            new_name: "Chaudière ♨ 熱".to_string(),
            new_description: Some("Unité métrique".to_string()),
        },
        Row {
            case: "name at the modelled minimum length (1)",
            by_external_id: false,
            new_name: "x".to_string(),
            new_description: Some("a".to_string()),
        },
        Row {
            case: "name at the modelled maximum length (256)",
            by_external_id: true,
            new_name: "n".repeat(256),
            new_description: Some("d".repeat(2048)),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_stored_attributes_of_the_asset_it_names() {
    let router = router();
    // `UpdateAsset` touches none of the asset's properties, so the model needs none.
    let model_id = create_model(&router, "Boiler", json!([])).await;

    for (at, row) in update_rows().into_iter().enumerate() {
        let external_id = format!("boiler_{at}");
        let asset_id = create_asset(
            &router,
            &model_id,
            "initial",
            Some("the initial description"),
            Some(&external_id),
        )
        .await;
        let before = describe(&router, &asset_id).await;
        let label = if row.by_external_id {
            format!("externalId:{external_id}")
        } else {
            asset_id.clone()
        };

        let mut body = json!({ "assetName": row.new_name });
        if let Some(description) = row.new_description.as_deref() {
            body["assetDescription"] = json!(description);
        }
        let (status, updated) =
            call(&router, "PUT", &format!("/assets/{label}"), Some(body)).await;

        let case = row.case;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {updated}"
        );
        assert_eq!(
            updated["assetId"].as_str().unwrap_or(""),
            asset_id,
            "case {case:?}: expected assetId {asset_id}, got {updated}"
        );
        // The model documents the state as UPDATING after a successful call.
        assert_eq!(
            updated["assetStatus"]["state"].as_str().unwrap_or(""),
            "UPDATING",
            "case {case:?}: expected state UPDATING, got {updated}"
        );

        // --- the write is what every later read answers with -------------------
        let after = describe(&router, &asset_id).await;
        assert_eq!(
            after["assetName"].as_str().unwrap_or(""),
            row.new_name,
            "case {case:?}: expected assetName {:?}, got {after}",
            row.new_name
        );
        assert_eq!(
            after
                .get("assetDescription")
                .and_then(Value::as_str)
                .map(str::to_string),
            row.new_description,
            "case {case:?}: expected assetDescription {:?}, got {after}",
            row.new_description
        );
        // The read agrees with the update: the asset is settling (UPDATING) or has
        // already settled (ACTIVE), never back in the create's CREATING. Which of the
        // two it is depends on the clock, so the exact label is pinned by
        // `the_updating_state_settles_to_active_and_touches_no_other_asset` instead.
        let state = after["assetStatus"]["state"].as_str().unwrap_or("");
        assert!(
            state == "UPDATING" || state == "ACTIVE",
            "case {case:?}: expected UPDATING or ACTIVE after an update, got {state:?} in {after}"
        );

        // --- what the caller does not own is untouched -------------------------
        for member in ["assetId", "assetArn", "assetModelId", "assetExternalId"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: {member} must not change across an update, got {after}"
            );
        }
        assert_eq!(
            epoch_seconds(&after, "assetCreationDate"),
            epoch_seconds(&before, "assetCreationDate"),
            "case {case:?}: assetCreationDate must not change across an update, got {after}"
        );
        assert!(
            epoch_seconds(&after, "assetLastUpdateDate")
                >= epoch_seconds(&before, "assetLastUpdateDate"),
            "case {case:?}: assetLastUpdateDate must not move backwards, got {after}"
        );

        // --- the summary view reads the same record ----------------------------
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/assets?assetModelId={model_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAssets got {status} {listed}"
        );
        let summary = listed["assetSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetSummaries in {listed}"))
            .iter()
            .find(|summary| summary["id"].as_str() == Some(asset_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {asset_id} is not listed in {listed}"));
        assert_eq!(
            summary["name"].as_str().unwrap_or(""),
            row.new_name,
            "case {case:?}: ListAssets reports a stale name in {summary}"
        );
    }
}

/// What a request is expected to be refused with: an HTTP status and the wire error
/// code, or `None` when it is expected to be accepted.
type Refusal = Option<(u16, &'static str)>;

#[tokio::test]
async fn an_external_id_is_assignable_once_and_is_unique_within_the_account() {
    let router = router();
    // `UpdateAsset` touches none of the asset's properties, so the model needs none.
    let model_id = create_model(&router, "Boiler", json!([])).await;
    let asset_id = create_asset(&router, &model_id, "Boiler1", None, None).await;
    let other_id = create_asset(&router, &model_id, "Boiler2", None, None).await;

    // An asset with no external id accepts one, and the alias form then resolves to it.
    let (status, body) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}"),
        Some(json!({ "assetName": "Boiler1", "assetExternalId": "boiler_one" })),
    )
    .await;
    assert_eq!(status, 202, "assigning an external id: got {status} {body}");
    let described = describe(&router, "externalId:boiler_one").await;
    assert_eq!(
        described["assetId"].as_str().unwrap_or(""),
        asset_id,
        "the externalId alias must resolve to the asset it was assigned to, got {described}"
    );
    assert_eq!(
        described["assetExternalId"].as_str().unwrap_or(""),
        "boiler_one",
        "the assigned external id must be reported, got {described}"
    );

    // Each row is one later request; `accepted` says whether the store changes.
    // Source for the rule: `smithy_query(query="operation", operation="UpdateAsset")`
    // — "The asset must not already have an external ID. The external ID must be
    // unique within your Amazon Web Services account."
    let rows: [(&str, &str, &str, Refusal); 3] = [
        (
            "the id it already carries is a no-op",
            &asset_id,
            "boiler_one",
            None,
        ),
        (
            "a second external id for the same asset is refused",
            &asset_id,
            "boiler_renamed",
            Some((400, "InvalidRequestException")),
        ),
        (
            "an external id another asset holds is refused",
            &other_id,
            "boiler_one",
            Some((409, "ResourceAlreadyExistsException")),
        ),
    ];
    for (case, target, requested, refusal) in rows {
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/assets/{target}"),
            Some(json!({ "assetName": "Renamed", "assetExternalId": requested })),
        )
        .await;
        match refusal {
            None => assert_eq!(status, 202, "case {case:?}: expected 202, got {status} {body}"),
            Some((expected_status, expected_code)) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected {expected_status}, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {case:?}: expected error code {expected_code}, got {body}"
                );
            }
        }
    }

    // A refused update writes NOTHING: the asset whose request collided still has no
    // external id and still carries the name it was created with.
    let other = describe(&router, &other_id).await;
    assert_eq!(
        other.get("assetExternalId").and_then(Value::as_str),
        None,
        "a refused update must not assign the external id, got {other}"
    );
    assert_eq!(
        other["assetName"].as_str().unwrap_or(""),
        "Boiler2",
        "a refused update must not rename the asset, got {other}"
    );
    // ... and the asset that already had one keeps exactly that one.
    let mine = describe(&router, &asset_id).await;
    assert_eq!(
        mine["assetExternalId"].as_str().unwrap_or(""),
        "boiler_one",
        "a refused re-assignment must leave the stored external id alone, got {mine}"
    );
}

#[tokio::test]
async fn updating_an_asset_that_does_not_exist_is_a_modelled_refusal() {
    let router = router();
    // Each row is a label form that resolves to no stored asset.
    let labels = [
        ("raw id", "11111111-1111-1111-1111-111111111111"),
        ("externalId alias", "externalId:no_such_asset"),
    ];
    for (case, label) in labels {
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/assets/{label}"),
            Some(json!({ "assetName": "Whatever" })),
        )
        .await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn the_modelled_input_constraints_bound_what_an_update_accepts() {
    let router = router();
    // `UpdateAsset` touches none of the asset's properties, so the model needs none.
    let model_id = create_model(&router, "Boiler", json!([])).await;
    let asset_id = create_asset(&router, &model_id, "Boiler1", Some("initial"), None).await;
    // The `(assetName, assetDescription)` pair the last ACCEPTED request wrote — what
    // every read must answer with, including the reads after a refused request.
    let mut last_accepted: (String, Option<String>) =
        ("Boiler1".to_string(), Some("initial".to_string()));

    // Rows: each published bound of the modelled members, and one value past it.
    // Source: `smithy_query(query="operation", operation="UpdateAsset")` — assetName
    // length 1..256, assetDescription length 1..2048, both with the pattern
    // `^[^\u0000-\u001F\u007F]+$` (no control characters).
    let rows: Vec<(String, String, Option<String>, bool)> = vec![
        ("name at min (1)".to_string(), "a".to_string(), None, true),
        (
            "name at max (256)".to_string(),
            "a".repeat(256),
            None,
            true,
        ),
        (
            "name past max (257)".to_string(),
            "a".repeat(257),
            None,
            false,
        ),
        ("name past min (0)".to_string(), String::new(), None, false),
        (
            "name with a control character".to_string(),
            "bad\u{1}name".to_string(),
            None,
            false,
        ),
        (
            "description at max (2048)".to_string(),
            "Boiler1".to_string(),
            Some("d".repeat(2048)),
            true,
        ),
        (
            "description past max (2049)".to_string(),
            "Boiler1".to_string(),
            Some("d".repeat(2049)),
            false,
        ),
    ];

    for (case, name, description, accepted) in rows {
        let mut body = json!({ "assetName": name });
        if let Some(description) = description.as_deref() {
            body["assetDescription"] = json!(description);
        }
        let (status, answered) = call(
            &router,
            "PUT",
            &format!("/assets/{asset_id}"),
            Some(body),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 202,
                "case {case:?}: expected 202, got {status} {answered}"
            );
            last_accepted = (name.clone(), description.clone());
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {answered}"
            );
        }
        // Whether accepted or refused, the stored asset carries exactly what the
        // last ACCEPTED update wrote: a refused request writes nothing.
        let described = describe(&router, &asset_id).await;
        let stored = (
            described["assetName"].as_str().unwrap_or("").to_string(),
            described
                .get("assetDescription")
                .and_then(Value::as_str)
                .map(str::to_string),
        );
        assert_eq!(
            stored, last_accepted,
            "case {case:?}: expected the last accepted write {last_accepted:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_updating_state_settles_to_active_and_touches_no_other_asset() {
    let router = router();
    // `UpdateAsset` touches none of the asset's properties, so the model needs none.
    let model_id = create_model(&router, "Boiler", json!([])).await;
    let asset_id = create_asset(&router, &model_id, "Boiler1", None, None).await;
    let sibling_id = create_asset(&router, &model_id, "Boiler2", Some("untouched"), None).await;

    // The create's own owed transition settles first, so the asset is ACTIVE before
    // the update and the UPDATING below cannot be the leftover CREATING.
    let settle = ACTIVATION_DELAY + Duration::from_millis(200);
    tokio::time::sleep(settle).await;
    let described = describe(&router, &asset_id).await;
    assert_eq!(
        described["assetStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "a created asset settles to ACTIVE, got {described}"
    );

    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}"),
        Some(json!({ "assetName": "Boiler1 renamed" })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAsset: got {status} {updated}");

    // The transition the update owes is not instant: a read taken straight after it
    // still observes UPDATING.
    let during = describe(&router, &asset_id).await;
    assert_eq!(
        during["assetStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "an asset reports UPDATING until the owed transition falls due, got {during}"
    );

    tokio::time::sleep(settle).await;
    let settled = describe(&router, &asset_id).await;
    assert_eq!(
        settled["assetStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "the owed UPDATING -> ACTIVE transition must fall due, got {settled}"
    );
    assert_eq!(
        settled["assetName"].as_str().unwrap_or(""),
        "Boiler1 renamed",
        "settling must not undo the update, got {settled}"
    );

    // An update names exactly one asset: every other one reads back as it was.
    let sibling = describe(&router, &sibling_id).await;
    assert_eq!(
        sibling["assetName"].as_str().unwrap_or(""),
        "Boiler2",
        "updating one asset must not rename another, got {sibling}"
    );
    assert_eq!(
        sibling["assetDescription"].as_str().unwrap_or(""),
        "untouched",
        "updating one asset must not touch another's description, got {sibling}"
    );
}
