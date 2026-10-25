// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetModels`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListAssetModels` enumerates the asset models the
//! service's state holds, each exactly once, in ONE stable order, narrowed to the
//! `assetModelTypes` the request asks for (every type when it asks for none) and
//! to the models that have the version the alias names, cut into pages of at most
//! `maxResults` entries, where `nextToken` is present only while another entry
//! follows and resumes the enumeration exactly where the previous page stopped; a
//! listed model carries the same values `DescribeAssetModel` reports for it, and a
//! `nextToken` the operation never issued is refused rather than answered with an
//! empty page.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `hex_token`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second
//! copy.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};

/// The collection this operation lists.
const ASSET_MODELS: &str = "/asset-models";

/// Comfortably past `state::ACTIVATION_DELAY`, so the owed CREATING -> ACTIVE
/// transition of every model in the fixture has fallen due.
const PAST_THE_OWED_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);

/// One row of the fixture: the model's name, its `assetModelType`, the external id
/// it was created with (or none) and the description it was created with (or none).
struct ModelCase {
    name: &'static str,
    model_type: Option<&'static str>,
    external_id: Option<&'static str>,
    description: Option<&'static str>,
}

/// The rows: nine asset models spanning every `AssetModelType` the model declares
/// — source `smithy_query(query="shape", shape="AssetModelType")`: ASSET_MODEL |
/// COMPONENT_MODEL | INTERFACE — including the type left absent (which defaults to
/// ASSET_MODEL), with and without an external id and with and without a
/// description, so a page boundary never lands only between two identically shaped
/// models.
fn model_cases() -> Vec<ModelCase> {
    vec![
        ModelCase {
            name: "Pump",
            model_type: None,
            external_id: None,
            description: None,
        },
        ModelCase {
            name: "Turbine",
            model_type: Some("ASSET_MODEL"),
            external_id: Some("list-models-turbine"),
            description: Some("A turbine"),
        },
        ModelCase {
            name: "Boiler",
            model_type: Some("ASSET_MODEL"),
            external_id: None,
            description: Some("A boiler"),
        },
        ModelCase {
            name: "Motor",
            model_type: Some("COMPONENT_MODEL"),
            external_id: Some("list-models-motor"),
            description: None,
        },
        ModelCase {
            name: "Bearing",
            model_type: Some("COMPONENT_MODEL"),
            external_id: None,
            description: Some("A reusable bearing"),
        },
        ModelCase {
            name: "Gearbox",
            model_type: Some("COMPONENT_MODEL"),
            external_id: Some("list-models-gearbox"),
            description: Some("A reusable gearbox"),
        },
        ModelCase {
            name: "Measured",
            model_type: Some("INTERFACE"),
            external_id: None,
            description: None,
        },
        ModelCase {
            name: "Monitored",
            model_type: Some("INTERFACE"),
            external_id: Some("list-models-monitored"),
            description: Some("An interface"),
        },
        ModelCase {
            name: "Audited",
            model_type: Some("INTERFACE"),
            external_id: None,
            description: Some("Another interface"),
        },
    ]
}

/// Create one asset model from a row and return its id.
async fn create_model(router: &MultiServiceRouter, case: &ModelCase) -> String {
    let mut body = json!({
        "assetModelName": case.name,
        "assetModelProperties": [
            {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}}
        ],
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(model_type) = case.model_type {
        object.insert("assetModelType".to_string(), json!(model_type));
    }
    if let Some(external_id) = case.external_id {
        object.insert("assetModelExternalId".to_string(), json!(external_id));
    }
    if let Some(description) = case.description {
        object.insert("assetModelDescription".to_string(), json!(description));
    }

    let (status, created) = call(router, "POST", ASSET_MODELS, Some(body)).await;
    assert_eq!(
        status, 202,
        "case {:?}: CreateAssetModel expected 202, got {status} {created}",
        case.name
    );
    let id = created["assetModelId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: assetModelId must be a modelled UUID, got {created}",
        case.name
    );
    id
}

/// Create the whole fixture and return `(id, the row it came from)`, in creation
/// order.
async fn create_fixture(router: &MultiServiceRouter) -> Vec<(String, ModelCase)> {
    let mut created = Vec::new();
    for case in model_cases() {
        let id = create_model(router, &case).await;
        created.push((id, case));
    }
    created
}

/// The type a row asks for, with the documented default applied.
fn effective_type(case: &ModelCase) -> &'static str {
    // "assetModelType ... (default) An asset model that you can use to create
    // assets" — an absent type is ASSET_MODEL.
    case.model_type.unwrap_or("ASSET_MODEL")
}

/// Build one `ListAssetModels` URI from its query parameters.
fn uri(query: &[String]) -> String {
    if query.is_empty() {
        ASSET_MODELS.to_string()
    } else {
        format!("{ASSET_MODELS}?{}", query.join("&"))
    }
}

/// Walk the whole enumeration at `page_size` (or with no `maxResults` at all when
/// `page_size` is `None`), with `extra` query parameters on every page, returning
/// the ids in the order the pages reported them.
async fn walk(
    router: &MultiServiceRouter,
    page_size: Option<usize>,
    extra: &[String],
) -> Vec<String> {
    let case = format!("maxResults={page_size:?}, {extra:?}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut query: Vec<String> = extra.to_vec();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }

        let (status, body) = call(router, "GET", &uri(&query), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["assetModelSummaries"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: assetModelSummaries must be a list, got {body}")
        });
        // The default when the caller sends no `maxResults` is the documented 50.
        let limit = page_size.unwrap_or(50);
        assert!(
            summaries.len() <= limit,
            "case {case:?}: a page holds at most {limit}, got {} in {body}",
            summaries.len()
        );
        for summary in summaries {
            seen.push(
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary without an id: {summary}"))
                    .to_string(),
            );
        }
        pages += 1;
        assert!(
            pages <= seen.len() + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // "The token for the next set of results, or null if there are no
            // additional results": a token is issued only while the page it resumes
            // would hold something, so the page before it is full.
            Some(_) => assert_eq!(
                summaries.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
    }

    let expected_pages = seen.len().div_ceil(page_size.unwrap_or(50)).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {} models, walked {pages}",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn list_asset_models_pages_through_every_stored_model_once() {
    let router = router();
    let created = create_fixture(&router).await;

    // The reference order: the enumeration one unpaged call reports.
    let reference = walk(&router, Some(250), &[]).await;
    let listed: BTreeSet<String> = reference.iter().cloned().collect();
    let expected: BTreeSet<String> = created.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        listed, expected,
        "the enumeration must cover exactly the created models, saw {reference:?}"
    );
    assert_eq!(
        reference.len(),
        created.len(),
        "every model must be listed exactly once, saw {reference:?}"
    );

    // Rows differ in page size: the published `maxResults` bounds (1 and 250), sizes
    // that divide the fixture and sizes that do not, a size past the fixture, and no
    // `maxResults` at all (the documented default of 50).
    for page_size in [Some(1_usize), Some(2), Some(3), Some(4), Some(9), Some(10), Some(250), None]
    {
        let seen = walk(&router, page_size, &[]).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_asset_model_carries_what_describe_asset_model_reports() {
    let router = router();
    let created = create_fixture(&router).await;
    // Both reads are taken after the owed transition has fallen due, so the derived
    // status cannot move between them.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let (status, body) = call(&router, "GET", &uri(&["maxResults=250".to_string()]), None).await;
    assert_eq!(status, 200, "ListAssetModels expected 200, got {status} {body}");
    let summaries = body["assetModelSummaries"]
        .as_array()
        .expect("assetModelSummaries")
        .clone();
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created model must be listed, got {body}"
    );

    // The summary member and the `DescribeAssetModel` member that reports the same
    // stored value.
    let same: [(&str, &str); 10] = [
        ("id", "assetModelId"),
        ("externalId", "assetModelExternalId"),
        ("arn", "assetModelArn"),
        ("name", "assetModelName"),
        ("assetModelType", "assetModelType"),
        ("description", "assetModelDescription"),
        ("creationDate", "assetModelCreationDate"),
        ("lastUpdateDate", "assetModelLastUpdateDate"),
        ("status", "assetModelStatus"),
        ("version", "assetModelVersion"),
    ];
    for summary in &summaries {
        let id = summary["id"].as_str().unwrap_or("");
        let (status, described) =
            call(&router, "GET", &format!("{ASSET_MODELS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribeAssetModel expected 200, got {status} {described}"
        );
        for (listed, described_as) in same {
            assert_eq!(
                summary[listed], described[described_as],
                "case {id:?}: the summary's {listed} must be what DescribeAssetModel reports as \
                 {described_as}: {summary} vs {described}"
            );
        }
    }

    // A model created with a name and a type is listed under them, so the row the
    // summary came from is identifiable without consulting the store.
    for (id, case) in &created {
        let summary = summaries
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: not listed, got {body}", case.name));
        assert_eq!(
            summary["name"], json!(case.name),
            "case {:?}: unexpected name in {summary}",
            case.name
        );
        assert_eq!(
            summary["assetModelType"],
            json!(effective_type(case)),
            "case {:?}: unexpected assetModelType in {summary}",
            case.name
        );
        assert_eq!(
            summary["externalId"],
            case.external_id.map(Value::from).unwrap_or(Value::Null),
            "case {:?}: unexpected externalId in {summary}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_asset_model_types_filter_selects_exactly_those_types() {
    let router = router();
    let created = create_fixture(&router).await;

    // Rows differ in WHICH types are asked for: none at all, each single type, each
    // pair, and all three. The expected set is computed from the fixture rows, so
    // nothing here is keyed to one recorded request.
    let selections: [&[&str]; 8] = [
        &[],
        &["ASSET_MODEL"],
        &["COMPONENT_MODEL"],
        &["INTERFACE"],
        &["ASSET_MODEL", "COMPONENT_MODEL"],
        &["ASSET_MODEL", "INTERFACE"],
        &["COMPONENT_MODEL", "INTERFACE"],
        &["ASSET_MODEL", "COMPONENT_MODEL", "INTERFACE"],
    ];
    for selection in selections {
        let query: Vec<String> = selection
            .iter()
            .map(|model_type| format!("assetModelTypes={model_type}"))
            .collect();
        let seen: BTreeSet<String> = walk(&router, Some(250), &query).await.into_iter().collect();
        let expected: BTreeSet<String> = created
            .iter()
            .filter(|(_, case)| selection.is_empty() || selection.contains(&effective_type(case)))
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(
            seen, expected,
            "case {selection:?}: the listing must hold exactly the models of those types, \
             got {seen:?}"
        );
    }

    // A type outside the modelled enum is refused rather than answered with an
    // empty page.
    let (status, body) = call(
        &router,
        "GET",
        &uri(&["assetModelTypes=NOT_A_MODEL_TYPE".to_string()]),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "an unmodelled assetModelTypes value must be refused, got {status} {body}"
    );
}

#[tokio::test]
async fn the_version_alias_lists_a_model_only_once_it_has_that_version() {
    let router = router();
    let created = create_fixture(&router).await;
    let all: BTreeSet<String> = created.iter().map(|(id, _)| id.clone()).collect();

    // LATEST is the documented default, and a freshly created model's latest
    // version is the one being created: both forms list every model straight away.
    for alias in ["", "assetModelVersion=LATEST"] {
        let query: Vec<String> = if alias.is_empty() {
            Vec::new()
        } else {
            vec![alias.to_string()]
        };
        let seen: BTreeSet<String> = walk(&router, Some(250), &query).await.into_iter().collect();
        assert_eq!(
            seen, all,
            "case {alias:?}: the latest version of every model must be listed, got {seen:?}"
        );
    }

    // Once the transition the clock owes has fallen due, every model has an ACTIVE
    // version. (The settling half of the rule is pinned by
    // `a_model_still_settling_has_no_active_version_yet`, which takes its reading
    // off a single create so it cannot race the clock.)
    let query = vec!["assetModelVersion=ACTIVE".to_string()];
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let active: BTreeSet<String> = walk(&router, Some(250), &query).await.into_iter().collect();
    assert_eq!(
        active, all,
        "every settled model has an ACTIVE version, got {active:?}"
    );

    // An alias outside the modelled pattern `^(LATEST|ACTIVE)$` is refused.
    for alias in ["latest", "OLDEST", "LATEST,ACTIVE"] {
        let (status, body) = call(
            &router,
            "GET",
            &uri(&[format!("assetModelVersion={alias}")]),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {alias:?}: an unmodelled assetModelVersion must be refused, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_model_still_settling_has_no_active_version_yet() {
    let router = router();
    // ONE create, and the reading is taken on the next call: the only way to be
    // inside `state::ACTIVATION_DELAY` whatever the machine is doing.
    let id = create_model(
        &router,
        &ModelCase {
            name: "Settling",
            model_type: None,
            external_id: None,
            description: None,
        },
    )
    .await;

    let (status, body) = call(
        &router,
        "GET",
        &uri(&["assetModelVersion=ACTIVE".to_string()]),
        None,
    )
    .await;
    assert_eq!(status, 200, "the ACTIVE alias expected 200, got {status} {body}");
    assert_eq!(
        body["assetModelSummaries"],
        json!([]),
        "a model whose CREATING -> ACTIVE transition is still owed has no ACTIVE version, \
         got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "an empty page issues no nextToken, got {body}"
    );

    // Its LATEST version — the default — is the one being created, so that form
    // lists it straight away.
    let (status, latest) = call(
        &router,
        "GET",
        &uri(&["assetModelVersion=LATEST".to_string()]),
        None,
    )
    .await;
    assert_eq!(status, 200, "the LATEST alias expected 200, got {status} {latest}");
    assert_eq!(
        latest["assetModelSummaries"][0]["id"],
        json!(id),
        "the latest version of a settling model is listed, got {latest}"
    );

    // Once the transition the clock owes has fallen due, the ACTIVE alias lists it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, active) = call(
        &router,
        "GET",
        &uri(&["assetModelVersion=ACTIVE".to_string()]),
        None,
    )
    .await;
    assert_eq!(status, 200, "the ACTIVE alias expected 200, got {status} {active}");
    assert_eq!(
        active["assetModelSummaries"][0]["id"],
        json!(id),
        "a settled model has an ACTIVE version, got {active}"
    );
}

#[tokio::test]
async fn listing_asset_models_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A store with no asset model reports an empty list and no token, rather than a
    // fabricated entry or a token that resumes nothing.
    let (status, empty) = call(&router, "GET", ASSET_MODELS, None).await;
    assert_eq!(status, 200, "an empty store expected 200, got {status} {empty}");
    assert_eq!(
        empty["assetModelSummaries"],
        json!([]),
        "an empty store lists no asset model, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let _ = create_fixture(&router).await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // Rows differ in page size and in the filters applied; each read twice. A read
    // never moves the enumeration.
    let cases: [(&str, &str); 4] = [
        ("a single-entry page", "maxResults=1"),
        ("the whole enumeration", "maxResults=250"),
        ("one type", "maxResults=250&assetModelTypes=INTERFACE"),
        ("the active version", "maxResults=250&assetModelVersion=ACTIVE"),
    ];
    for (case, query) in cases {
        let target = uri(&[query.to_string()]);
        let (status, first) = call(&router, "GET", &target, None).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {first}");
        let (status, again) = call(&router, "GET", &target, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second read expected 200, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case {case:?}: a read must not change what the next caller sees, got {again} \
             after {first}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let router = router();
    let created = create_fixture(&router).await;

    // Rows differ in HOW the token is wrong: not hex at all, hex of a well-formed id
    // no model has, hex of a string that is no id, and an odd-length hex run.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        (
            "hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("hex of something that is no id", hex_token("Pump")),
        ("odd-length hex", "abc".to_string()),
    ];
    for (case, token) in forged {
        let (status, body) = call(&router, "GET", &uri(&[format!("nextToken={token}")]), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A token this operation DID issue, resumed under a filter the model it points
    // at does not satisfy, is no longer part of that enumeration and is refused the
    // same way.
    let (status, body) = call(
        &router,
        "GET",
        &uri(&["maxResults=1&assetModelTypes=INTERFACE".to_string()]),
        None,
    )
    .await;
    assert_eq!(status, 200, "a filtered first page expected 200, got {status} {body}");
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a filtered first page must carry a nextToken, got {body}"))
        .to_string();
    let (status, body) = call(
        &router,
        "GET",
        &uri(&[format!("maxResults=1&assetModelTypes=COMPONENT_MODEL&nextToken={token}")]),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "a token resumed under a filter that excludes it must be refused, got {status} {body}"
    );

    // The refusals left the enumeration intact.
    let seen = walk(&router, Some(250), &[]).await;
    assert_eq!(
        seen.len(),
        created.len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let _ = create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListAssetModels"): each published bound is served and one past it on
    // either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (case, max_results, served) in cases {
        let (status, body) =
            call(&router, "GET", &uri(&[format!("maxResults={max_results}")]), None).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
        }
    }
}
