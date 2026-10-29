// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchDisassociateProjectAssets` — the removal side of an
//! IoT SiteWise Monitor project's asset membership — driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a disassociation takes exactly the assets the request names out of the
//!   project's stored membership and leaves the rest of it alone, which
//!   `ListProjectAssets` then reads back;
//! * the membership is a SET, so removing an asset the project does not hold takes
//!   nothing out and fails nothing, and an id named twice in one request removes it
//!   once — an accepted request therefore always reports an EMPTY per-asset error
//!   list, the only thing the modelled error list could carry being an
//!   `INTERNAL_FAILURE` this store cannot produce;
//! * a request naming a resource this service does not hold — the project in the
//!   path, or any one of the assets — is refused in full, in the one wording every
//!   not-found refusal of this service carries, and removes nothing;
//! * a removal from one project leaves every other project's membership alone.
//!
//! The rows differ along what those rules generalise over: how many assets the
//! membership holds and how many of them one request names (one, several, all of
//! them, none of them, the modelled maximum of 100 ids), whether an id is one the
//! project holds, one it never held, or one repeated inside the same request, and
//! whether a `clientToken` is carried.
//!
//! The membership a row starts from is built through `BatchAssociateProjectAssets`
//! over the wire and read back through `ListProjectAssets`, so this suite asserts a
//! real round trip; the helpers for all three (`associate`, `list_project_assets`,
//! `create_project`, `create_assets`, `stored_membership`) are the sibling
//! project-assets suite's, and `call` / `error_code` / `error_message` are the asset
//! and refusal suites'. This suite reuses them rather than growing a second copy.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, router_with_engine};
use super::hierarchies_etags_and_refusals::error_message;
use super::project_assets::{
    associate, create_assets, create_project, list_project_assets, stored_membership,
};

/// The greatest number of ids one request may carry
/// (smithy_query query="operation" operation="BatchDisassociateProjectAssets":
/// `assetIds` (IDs, len 1..100)).
const MAX_IDS_PER_REQUEST: usize = 100;

/// A UUID of the modelled `ID` form that no create in this suite ever mints, used
/// where a request has to name a resource the service does not hold.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// `POST /projects/{projectId}/assets/disassociate`, as the model binds the
/// operation (same source as above: `http {method: POST, uri:
/// /projects/{projectId}/assets/disassociate}`).
pub(crate) fn disassociate_uri(project_id: &str) -> String {
    format!("/projects/{project_id}/assets/disassociate")
}

/// The body one disassociation request carries.
pub(crate) fn disassociate_body(asset_ids: &[String], client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert(
        "assetIds".to_string(),
        Value::Array(
            asset_ids
                .iter()
                .map(|id| Value::String(id.clone()))
                .collect(),
        ),
    );
    if let Some(token) = client_token {
        body.insert("clientToken".to_string(), Value::String(token.to_string()));
    }
    Value::Object(body)
}

/// Send one disassociation request and answer `(status, body)`.
async fn disassociate(
    router: &MultiServiceRouter,
    project_id: &str,
    asset_ids: &[String],
    client_token: Option<&str>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &disassociate_uri(project_id),
        Some(disassociate_body(asset_ids, client_token)),
    )
    .await
}

/// One row of the removal table.
#[derive(Debug)]
struct RemovalCase {
    label: &'static str,
    /// How many assets the row associates with the project.
    associated: usize,
    /// Which of those assets, by index in the created list, the request names.
    remove: Vec<usize>,
    /// Indices the request names a SECOND time inside the same list.
    repeats: Vec<usize>,
    /// How many assets the row creates but never associates, and then names in the
    /// request: ids this service holds that the project does not.
    unassociated: usize,
    /// The `clientToken` the request carries, if any. A token targets
    /// `ClientToken` (len 36..64, pattern `^\S{36,64}$`), per the same
    /// smithy_query call as the list bound above.
    client_token: Option<&'static str>,
}

/// The rows.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            label: "the-only-asset-of-a-project-holding-one",
            associated: 1,
            remove: vec![0],
            repeats: Vec::new(),
            unassociated: 0,
            client_token: None,
        },
        RemovalCase {
            label: "one-of-three",
            associated: 3,
            remove: vec![1],
            repeats: Vec::new(),
            unassociated: 0,
            client_token: None,
        },
        RemovalCase {
            label: "two-of-four-named-out-of-order",
            associated: 4,
            remove: vec![3, 0],
            repeats: Vec::new(),
            unassociated: 0,
            client_token: None,
        },
        RemovalCase {
            label: "every-asset-the-project-holds",
            associated: 3,
            remove: vec![0, 1, 2],
            repeats: Vec::new(),
            unassociated: 0,
            client_token: None,
        },
        RemovalCase {
            label: "only-an-asset-the-project-never-held",
            associated: 2,
            remove: Vec::new(),
            repeats: Vec::new(),
            unassociated: 1,
            client_token: None,
        },
        RemovalCase {
            label: "one-it-holds-beside-two-it-never-held",
            associated: 3,
            remove: vec![0],
            repeats: Vec::new(),
            unassociated: 2,
            client_token: None,
        },
        RemovalCase {
            label: "one-id-repeated-inside-one-request",
            associated: 3,
            remove: vec![2],
            repeats: vec![2],
            unassociated: 0,
            client_token: None,
        },
        RemovalCase {
            label: "a-request-carrying-a-client-token",
            associated: 2,
            remove: vec![0],
            repeats: Vec::new(),
            unassociated: 0,
            client_token: Some("3f1c6a9e-8b2d-4e5f-9a1b-7c6d5e4f3a2b-token"),
        },
        RemovalCase {
            label: "a-request-at-the-modelled-maximum-of-one-hundred-ids",
            associated: MAX_IDS_PER_REQUEST,
            remove: (0..MAX_IDS_PER_REQUEST).collect(),
            repeats: Vec::new(),
            unassociated: 0,
            client_token: None,
        },
    ]
}

#[tokio::test]
async fn a_disassociation_removes_exactly_the_assets_the_request_names() {
    for case in removal_cases() {
        let label = case.label;
        let (router, engine) = router_with_engine();
        let project_id = create_project(&router, label, &format!("{label}-project")).await;
        let held = create_assets(&router, label, case.associated).await;
        associate(&router, label, &project_id, &held, None).await;
        let spare = create_assets(&router, &format!("{label}-spare"), case.unassociated).await;

        // The request's id list: the named members, then the repeats, then the assets
        // this service holds that the project does not.
        let mut named: Vec<String> = case
            .remove
            .iter()
            .chain(case.repeats.iter())
            .map(|at| held[*at].clone())
            .collect();
        named.extend(spare.iter().cloned());
        assert!(
            (1..=MAX_IDS_PER_REQUEST).contains(&named.len()),
            "case {label:?}: the row must itself stay inside the modelled list bounds, \
             it names {} ids",
            named.len()
        );

        let (status, answered) =
            disassociate(&router, &project_id, &named, case.client_token).await;
        assert_eq!(
            status, 200,
            "case {label:?}: BatchDisassociateProjectAssets expected 200, got {status} {answered}"
        );
        let errors = answered["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected an errors list, got {answered}"));
        assert!(
            errors.is_empty(),
            "case {label:?}: expected no per-asset errors, got {answered}"
        );

        // What the project must hold afterwards: its membership, a set ordered by id,
        // without the assets the request named.
        let mut expected: Vec<String> = held
            .iter()
            .enumerate()
            .filter(|(at, _)| !case.remove.contains(at))
            .map(|(_, asset_id)| asset_id.clone())
            .collect();
        expected.sort();

        let listed = list_project_assets(&router, label, &project_id, None).await;
        assert_eq!(
            listed, expected,
            "case {label:?}: the listing reports the membership the removal left, \
             got {listed:?}"
        );
        let stored = stored_membership(&engine, &project_id);
        assert_eq!(
            stored, expected,
            "case {label:?}: the membership on the project's record lost exactly the \
             assets the request named, got {stored:?}"
        );

        // The assets themselves are untouched: a disassociation removes a membership,
        // never the asset.
        for asset_id in held.iter().chain(spare.iter()) {
            let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: asset {asset_id} must still exist, got {status} {described}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_a_disassociation_changes_nothing_and_answers_the_same() {
    let case = "idempotence";
    let (router, engine) = router_with_engine();
    let project_id = create_project(&router, case, "idempotence-project").await;
    let held = create_assets(&router, case, 3).await;
    associate(&router, case, &project_id, &held, None).await;
    let spare = create_assets(&router, "idempotence-spare", 1).await;

    // The rows differ along how much of the membership the request names: part of it,
    // none of it (an asset the project never held), and the rest of it.
    let mut rest: Vec<String> = vec![held[0].clone(), held[2].clone()];
    rest.sort();
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one-of-three", vec![held[1].clone()], rest.clone()),
        ("an-asset-never-associated", spare.clone(), rest.clone()),
        ("the-rest-of-the-membership", rest.clone(), Vec::new()),
    ];

    for (label, named, expected) in rows {
        let mut answers = Vec::new();
        for attempt in ["first", "second"] {
            let (status, answered) = disassociate(&router, &project_id, &named, None).await;
            assert_eq!(
                status, 200,
                "case {label:?} ({attempt} call): expected 200, got {status} {answered}"
            );
            let listed = list_project_assets(&router, label, &project_id, None).await;
            assert_eq!(
                listed, expected,
                "case {label:?} ({attempt} call): the project must hold {expected:?}, \
                 got {listed:?}"
            );
            assert_eq!(
                stored_membership(&engine, &project_id),
                expected,
                "case {label:?} ({attempt} call): the stored membership must be {expected:?}"
            );
            answers.push(answered);
        }
        assert_eq!(
            answers[0], answers[1],
            "case {label:?}: a repeated request answers exactly as the first did, \
             got {:?} then {:?}",
            answers[0], answers[1]
        );
    }
}

#[tokio::test]
async fn a_request_naming_a_resource_this_service_does_not_hold_is_refused_in_full() {
    let case = "refusals";
    let (router, engine) = router_with_engine();
    let project_id = create_project(&router, case, "refusal-project").await;
    let held = create_assets(&router, case, 3).await;
    associate(&router, case, &project_id, &held, None).await;
    let mut settled: Vec<String> = held.clone();
    settled.sort();

    // An asset this service held and no longer holds: deleting it already took it out
    // of the membership, so naming it names a resource that is not there.
    let doomed = create_assets(&router, "refusal-doomed", 1).await[0].clone();
    associate(&router, case, &project_id, std::slice::from_ref(&doomed), None).await;
    let (status, deleted) = call(&router, "DELETE", &format!("/assets/{doomed}"), None).await;
    assert_eq!(
        status, 202,
        "DeleteAsset on the doomed asset: got {status} {deleted}"
    );

    // Each row: the id the request names, the resource kind the refusal states, and
    // the rest of the request's id list.
    let rows: Vec<(&str, String, &str, Vec<String>)> = vec![
        (
            "a-project-this-service-does-not-hold",
            ABSENT_ID.to_string(),
            "Project",
            vec![held[0].clone()],
        ),
        (
            "an-asset-id-nothing-ever-minted",
            project_id.clone(),
            "Asset",
            vec![ABSENT_ID.to_string()],
        ),
        (
            "an-absent-asset-among-assets-that-exist",
            project_id.clone(),
            "Asset",
            vec![held[1].clone(), ABSENT_ID.to_string(), held[2].clone()],
        ),
        (
            "an-asset-this-service-no-longer-holds",
            project_id.clone(),
            "Asset",
            vec![held[0].clone(), doomed.clone()],
        ),
    ];

    for (label, addressed, kind, named) in rows {
        let (status, answered) = disassociate(&router, &addressed, &named, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {label:?}: the modelled 404 code, got {answered}"
        );
        let missing = if kind == "Project" {
            addressed.clone()
        } else if named.contains(&doomed) {
            doomed.clone()
        } else {
            ABSENT_ID.to_string()
        };
        assert_eq!(
            error_message(&answered),
            format!("{kind} with ID {missing} not found"),
            "case {label:?}: the one not-found wording, naming the id the request sent"
        );
        let listed = list_project_assets(&router, label, &project_id, None).await;
        assert_eq!(
            listed, settled,
            "case {label:?}: a refused request removes nothing, got {listed:?}"
        );
        assert_eq!(
            stored_membership(&engine, &project_id),
            settled,
            "case {label:?}: a refused request writes nothing to the store"
        );
    }
}

#[tokio::test]
async fn an_id_list_outside_the_modelled_bounds_is_refused_before_anything_is_removed() {
    let case = "bounds";
    let (router, engine) = router_with_engine();
    let project_id = create_project(&router, case, "bounds-project").await;
    let held = create_assets(&router, case, 2).await;
    associate(&router, case, &project_id, &held, None).await;
    let mut settled: Vec<String> = held.clone();
    settled.sort();

    // The rows walk the published bound of `assetIds` — one under @length min 1 and
    // one past @length max 100 — and the forms of the member itself a request can get
    // wrong. Only the STATUS is asserted: every row is refused by the boundary
    // validator before the handler runs, and the error code that walk renders is the
    // subject of its own issue, the same reason the sibling project suites record.
    let past_the_maximum: Vec<String> = (0..=MAX_IDS_PER_REQUEST)
        .map(|at| format!("{:08x}-0000-4000-8000-000000000000", at + 1))
        .collect();
    let rows: Vec<(&str, Value)> = vec![
        ("no-assetIds-at-all", json!({})),
        (
            "an-empty-assetIds-list-one-under-length-min-1",
            disassociate_body(&[], None),
        ),
        (
            "one-hundred-and-one-ids-one-past-length-max-100",
            disassociate_body(&past_the_maximum, None),
        ),
        (
            "an-assetId-that-is-not-a-modelled-ID",
            json!({ "assetIds": ["not-a-uuid"] }),
        ),
    ];

    for (label, body) in rows {
        let (status, answered) =
            call(&router, "POST", &disassociate_uri(&project_id), Some(body)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a modelled-rule violation is a 400, got {status} {answered}"
        );
    }
    assert_eq!(
        stored_membership(&engine, &project_id),
        settled,
        "every refusal above left the stored membership exactly as it was"
    );
}

#[tokio::test]
async fn disassociating_from_one_project_leaves_every_other_project_alone() {
    let case = "isolation";
    let (router, engine) = router_with_engine();
    let held = create_assets(&router, case, 3).await;
    let mut settled: Vec<String> = held.clone();
    settled.sort();

    // Three projects, each holding the SAME three assets, so only the call can tell
    // them apart afterwards.
    let mut project_ids = Vec::new();
    for at in ["first", "second", "third"] {
        let project_id = create_project(&router, case, &format!("{at}-isolation-project")).await;
        associate(&router, case, &project_id, &held, None).await;
        project_ids.push(project_id);
    }

    let named = vec![held[0].clone(), held[2].clone()];
    let (status, answered) = disassociate(&router, &project_ids[1], &named, None).await;
    assert_eq!(
        status, 200,
        "the project the request named: got {status} {answered}"
    );

    for (at, project_id) in project_ids.iter().enumerate() {
        let expected: Vec<String> = if at == 1 {
            vec![held[1].clone()]
        } else {
            settled.clone()
        };
        let listed = list_project_assets(&router, case, project_id, None).await;
        assert_eq!(
            listed, expected,
            "project {at}: expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            stored_membership(&engine, project_id),
            expected,
            "project {at}: the stored membership must be {expected:?}"
        );
    }
}
