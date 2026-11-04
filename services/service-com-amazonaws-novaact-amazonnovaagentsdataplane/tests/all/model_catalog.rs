// SPDX-License-Identifier: CC0-1.0

//! `ListModels`: the published model catalog, seen through the caller's own client
//! compatibility version.
//!
//! The rule these cases assert is not "this one request answers these bytes". It is:
//! every call reports the same published catalog — each model with its `ModelStatus` and
//! its own `minimumCompatibilityVersion` — while everything that depends on the REQUEST
//! is computed from it: the echoed `clientCompatibilityVersion`, the `supportedModelIds`
//! that version reaches (a model at or above its own minimum whose lifecycle is not
//! `DEPRECATED`), each alias's `resolvedModelId` (filled exactly when that caller reaches
//! the model the alias points at) and the compatibility `message` (only where the caller
//! reaches no model at all).
//!
//! So the table below varies what that rule generalises over — the compatibility version,
//! across every published minimum, each bound of `Integer` and one version below the
//! lowest minimum — and the per-row expectations are computed from the catalog the
//! response itself reports rather than written out, so a later revision of the catalog
//! changes no expectation here.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::ops::types::MODEL_STATUS_VALUES;

use crate::workflow_lifecycle::{text, Emulator};

/// `ModelStatus`' retired value: the one status that admits no client version at all.
/// Spelled by the generated `MODEL_STATUS_VALUES` catalog this suite also asserts over.
const DEPRECATED: &str = "DEPRECATED";

/// `ModelId` is `@length(min: 1, max: 100)`
/// (`smithy_query(query="shape",shape="ModelId")`), which bounds every model id and every
/// alias name this operation reports.
const MODEL_ID_MAX_LENGTH: usize = 100;

/// One `ListModels` call. `clientCompatibilityVersion` is an `@httpQuery` member of a
/// `POST /models` with no body members
/// (`smithy_query(query="operation",operation="ListModels")`).
async fn list_models(emulator: &Emulator, client_compatibility_version: i64) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/models?clientCompatibilityVersion={client_compatibility_version}"),
            None,
        )
        .await
}

/// A list member of a response body, or a panic naming what came back instead.
fn list_of<'a>(body: &'a Value, member: &str) -> &'a Vec<Value> {
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("member {member} must be a list in {body}"))
}

/// An integer member of a response body.
fn number(body: &Value, member: &str) -> i64 {
    body[member]
        .as_i64()
        .unwrap_or_else(|| panic!("member {member} must be an integer in {body}"))
}

/// The catalog a response reports, as `(modelId, status, minimumCompatibilityVersion)` in
/// the order it carries them.
fn reported_catalog(body: &Value) -> Vec<(String, String, i64)> {
    list_of(body, "modelSummaries")
        .iter()
        .map(|summary| {
            (
                text(summary, "modelId"),
                text(&summary["modelLifecycle"], "status"),
                number(summary, "minimumCompatibilityVersion"),
            )
        })
        .collect()
}

/// The model ids a response offers for the version it was asked about.
fn supported_ids(body: &Value) -> Vec<String> {
    list_of(&body["compatibilityInformation"], "supportedModelIds")
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("every supportedModelIds entry must be a string in {body}"))
                .to_string()
        })
        .collect()
}

/// RULE: one published catalog, and one compatibility view of it per caller.
///
/// Each row is a client compatibility version; every expectation in it is DERIVED from
/// the catalog the response reports, so the row asserts the rule rather than a value.
#[tokio::test]
async fn the_compatibility_view_of_the_catalog_is_computed_from_the_requested_version() {
    let emulator = Emulator::new("us-west-2");

    // The catalog as the service publishes it, read once so every row can be checked
    // against the same published fact.
    let (status, first) = list_models(&emulator, 1).await;
    assert_eq!(
        status, 200,
        "case \"catalog\": expected HTTP 200, got {status} {first}"
    );
    let catalog = reported_catalog(&first);
    assert!(
        !catalog.is_empty(),
        "case \"catalog\": expected ListModels to publish at least one model, got {first}"
    );
    // The rule only bites where the catalog has something on each side of it, so the two
    // sides are asserted before the rows lean on them.
    assert!(
        catalog.iter().any(|(_, status, _)| status == DEPRECATED),
        "case \"catalog\": expected a retired model, so the lifecycle rule is exercised, got {catalog:?}"
    );
    assert!(
        catalog.iter().any(|(_, status, _)| status != DEPRECATED),
        "case \"catalog\": expected a usable model, got {catalog:?}"
    );
    for (model_id, status, _) in &catalog {
        assert!(
            !model_id.is_empty() && model_id.len() <= MODEL_ID_MAX_LENGTH,
            "case \"catalog\": modelId must satisfy ModelId's length 1..100, got {model_id:?}"
        );
        assert!(
            MODEL_STATUS_VALUES.contains(&status.as_str()),
            "case \"catalog\": status must be one of {MODEL_STATUS_VALUES:?}, got {status:?}"
        );
    }

    // The published minimums, one version below the lowest of them, one above the highest,
    // and each bound of the modelled `Integer` the member targets: the rows differ along
    // the one value the rule generalises over.
    let lowest = catalog
        .iter()
        .map(|(_, _, minimum)| *minimum)
        .min()
        .expect("a non-empty catalog");
    let highest = catalog
        .iter()
        .map(|(_, _, minimum)| *minimum)
        .max()
        .expect("a non-empty catalog");
    let mut versions: BTreeSet<i64> = catalog
        .iter()
        .map(|(_, _, minimum)| *minimum)
        .collect();
    versions.insert(lowest - 1);
    versions.insert(highest + 1);
    versions.insert(i64::from(i32::MIN));
    versions.insert(i64::from(i32::MAX));
    versions.insert(0);

    for version in versions {
        let (status, body) = list_models(&emulator, version).await;
        assert_eq!(
            status, 200,
            "case {version}: expected HTTP 200, got {status} {body}"
        );

        // The catalog belongs to the service, so it does not move with the caller.
        assert_eq!(
            reported_catalog(&body),
            catalog,
            "case {version}: expected the same published catalog, got {body}"
        );

        let compatibility = &body["compatibilityInformation"];
        assert_eq!(
            number(compatibility, "clientCompatibilityVersion"),
            version,
            "case {version}: expected the caller's own compatibility version echoed, got {body}"
        );

        // A model is offered when the caller is at or above its minimum and its lifecycle
        // still admits use; everything else is not offered.
        let expected_supported: Vec<String> = catalog
            .iter()
            .filter(|(_, status, minimum)| status != DEPRECATED && *minimum <= version)
            .map(|(model_id, _, _)| model_id.clone())
            .collect();
        let supported = supported_ids(&body);
        assert_eq!(
            supported, expected_supported,
            "case {version}: expected supportedModelIds {expected_supported:?}, got {supported:?}"
        );

        // The note is there exactly when the caller reaches no model at all, and names a
        // version that does reach one.
        match compatibility["message"].as_str() {
            Some(message) => {
                assert!(
                    supported.is_empty(),
                    "case {version}: a caller that reaches {supported:?} needs no compatibility note, got {message:?}"
                );
                assert!(
                    message.contains(&lowest.to_string()),
                    "case {version}: expected the note to name compatibility version {lowest}, got {message:?}"
                );
            }
            None => assert!(
                !supported.is_empty(),
                "case {version}: a caller that reaches no model must be told which version does, got {body}"
            ),
        }

        // Every alias points at a model the catalog holds and has not retired, and
        // resolves exactly when THIS caller reaches it.
        let aliases = list_of(&body, "modelAliases");
        assert!(
            !aliases.is_empty(),
            "case {version}: expected ListModels to publish at least one alias, got {body}"
        );
        for alias in aliases {
            let alias_name = text(alias, "aliasName");
            let latest = text(alias, "latestModelId");
            assert!(
                !alias_name.is_empty() && alias_name.len() <= MODEL_ID_MAX_LENGTH,
                "case {version}: aliasName must satisfy ModelId's length 1..100, got {alias_name:?}"
            );
            let pointed = catalog
                .iter()
                .find(|(model_id, _, _)| *model_id == latest)
                .unwrap_or_else(|| {
                    panic!("case {version}: alias {alias_name:?} must point at a published model, got {latest:?}")
                });
            assert_ne!(
                pointed.1, DEPRECATED,
                "case {version}: alias {alias_name:?} must not point at a retired model, got {pointed:?}"
            );
            let expected_resolved = supported.contains(&latest).then(|| latest.clone());
            let resolved = alias["resolvedModelId"].as_str().map(str::to_string);
            assert_eq!(
                resolved, expected_resolved,
                "case {version}: alias {alias_name:?} expected resolvedModelId {expected_resolved:?}, got {resolved:?}"
            );
        }

        // A reporting operation answers the same thing twice: nothing about the call
        // changes what the next caller sees.
        let (again_status, again) = list_models(&emulator, version).await;
        assert_eq!(
            again_status, 200,
            "case {version}: expected the repeated call to answer HTTP 200, got {again_status} {again}"
        );
        assert_eq!(
            again, body,
            "case {version}: expected the repeated call to answer the same listing, got {again}"
        );
    }
}

/// RULE: the offered set only ever grows as the caller's compatibility version grows — a
/// model a lower version reaches is reached by every higher one, since the only
/// version-dependent condition is a lower bound.
#[tokio::test]
async fn a_newer_client_reaches_every_model_an_older_one_reaches() {
    let emulator = Emulator::new("eu-central-1");

    let mut previous: BTreeSet<String> = BTreeSet::new();
    // Walk upwards across the whole published range and a little beyond it on both sides.
    for version in -2..=6 {
        let (status, body) = list_models(&emulator, version).await;
        assert_eq!(
            status, 200,
            "case {version}: expected HTTP 200, got {status} {body}"
        );
        let offered: BTreeSet<String> = supported_ids(&body).into_iter().collect();
        assert!(
            previous.is_subset(&offered),
            "case {version}: expected every model version {} reached to still be reached, got {offered:?}",
            version - 1
        );
        previous = offered;
    }
    assert!(
        !previous.is_empty(),
        "case \"top of the range\": expected the highest version walked to reach a model, got {previous:?}"
    );
}

/// RULE: the ids the catalog publishes are this service's own model ids — a model a
/// caller's version resolves through an alias drives a workflow run, and the run reports
/// it back.
#[tokio::test]
async fn a_resolved_model_id_drives_a_workflow_run() {
    let emulator = Emulator::new("us-west-2");

    let (status, body) = list_models(&emulator, i64::from(i32::MAX)).await;
    assert_eq!(
        status, 200,
        "case \"resolve\": expected HTTP 200, got {status} {body}"
    );
    // Every alias resolves for a caller at the highest compatibility version there is, so
    // each resolved id is exercised as a run's `modelId`.
    let resolved: Vec<String> = list_of(&body, "modelAliases")
        .iter()
        .map(|alias| {
            alias["resolvedModelId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case \"resolve\": every alias must resolve at the highest version, got {alias}")
                })
                .to_string()
        })
        .collect();
    assert!(
        !resolved.is_empty(),
        "case \"resolve\": expected at least one resolved alias, got {body}"
    );

    let (status, created) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "model-catalog" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case \"definition\": expected HTTP 201, got {status} {created}"
    );

    for model_id in &resolved {
        let (status, run) = emulator
            .call(
                "PUT",
                "/workflow-definitions/model-catalog/workflow-runs",
                Some(json!({
                    "modelId": model_id,
                    "clientInfo": { "compatibilityVersion": 1 },
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {model_id:?}: expected HTTP 201, got {status} {run}"
        );
        let run_id = text(&run, "workflowRunId");

        let (status, read_back) = emulator
            .call(
                "GET",
                &format!("/workflow-definitions/model-catalog/workflow-runs/{run_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {model_id:?}: expected the read-back to answer HTTP 200, got {status} {read_back}"
        );
        assert_eq!(
            text(&read_back, "modelId"),
            *model_id,
            "case {model_id:?}: expected the run to report the model it was created with, got {read_back}"
        );
    }
}
