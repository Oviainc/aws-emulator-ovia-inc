// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListCompositionRelationships`, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rule these suites pin: the composition relationships of an asset model are
//! every composite model stored in the account that reuses it as its component
//! model — whatever asset model holds that composite model, at whatever nesting
//! depth and of whatever type — each reported from the stored composite model the
//! describes read back, in one stable order cut into pages of at most `maxResults`
//! (default 50) joined by a resumable `nextToken`; an asset model nothing reuses
//! has none, and a label or a token that names nothing stored is a modelled
//! refusal.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, hex_token, router};

/// The composite model ids the fixture's creates fix, so the expected
/// enumeration and every page token are known to the test without asking the
/// emulator. Each is a lowercase hyphenated UUID with RFC 4122's version-4
/// nibble, which is what the modelled `ID` pattern admits.
const IDS: [&str; 6] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
    "55555555-5555-4555-8555-555555555555",
    "66666666-6666-4666-8666-666666666666",
];

/// A UUID-shaped asset model id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "99999999-9999-4999-8999-999999999999";

/// The asset models and composite models the suites share.
struct Fixture {
    /// A `COMPONENT_MODEL` three composite models, on three different asset
    /// models, reuse.
    reused_component: String,
    /// A `COMPONENT_MODEL` exactly one composite model reuses.
    single_use_component: String,
    /// A `COMPONENT_MODEL` nothing reuses.
    unused_component: String,
    /// The asset models holding the composite models, in the order they were
    /// created — which is the order the listing enumerates their relationships
    /// in.
    holders: [String; 3],
}

/// One expected relationship: the composite model, the asset model holding it
/// and the type it carries.
struct Expected {
    /// What the row varies, named by every assertion that reads it.
    case: &'static str,
    /// The `assetModelCompositeModelId` the relationship names.
    composite_id: &'static str,
    /// Index into [`Fixture::holders`] of the asset model holding it.
    holder: usize,
    /// The `assetModelCompositeModelType` the create asked for.
    composite_type: &'static str,
}

/// Create an asset model from `body` and return its id.
async fn create_model(router: &MultiServiceRouter, name: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(status, 202, "create model {name}: got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create model {name}: no assetModelId in {created}"))
        .to_string()
}

/// `POST /asset-models/{model_id}/composite-models` with `body`, which must be
/// accepted.
async fn create_composite(router: &MultiServiceRouter, model_id: &str, body: Value) {
    let (status, created) = call(
        router,
        "POST",
        &format!("/asset-models/{model_id}/composite-models"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel({body}): got {status} {created}"
    );
}

/// Build the shared fixture: three `COMPONENT_MODEL`s and three asset models
/// reusing them, so one component model is reused by composite models on
/// several asset models and one is reused by none.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let reused_component = create_model(
        router,
        "Pump",
        json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let single_use_component = create_model(
        router,
        "Valve",
        json!({"assetModelName": "Valve", "assetModelType": "COMPONENT_MODEL"}),
    )
    .await;
    let unused_component = create_model(
        router,
        "Fan",
        json!({"assetModelName": "Fan", "assetModelType": "COMPONENT_MODEL"}),
    )
    .await;

    // The holders, created in this order, which is the order their relationships
    // are enumerated in.
    let mut holders: Vec<String> = Vec::new();
    for name in ["Boiler", "Turbine", "Chiller"] {
        holders.push(create_model(router, name, json!({"assetModelName": name})).await);
    }

    // Boiler: one composite model reusing the shared component model, one inline
    // composite model that reuses nothing (so it is in no relationship) and one
    // reusing the single-use component model.
    create_composite(
        router,
        &holders[0],
        json!({
            "assetModelCompositeModelId": IDS[0],
            "assetModelCompositeModelName": "ThePump",
            "assetModelCompositeModelExternalId": "the_pump",
            "assetModelCompositeModelDescription": "The pump this boiler feeds from",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &reused_component,
        }),
    )
    .await;
    create_composite(
        router,
        &holders[0],
        json!({
            "assetModelCompositeModelId": IDS[1],
            "assetModelCompositeModelName": "Inline",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Pressure", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    create_composite(
        router,
        &holders[0],
        json!({
            "assetModelCompositeModelId": IDS[2],
            "assetModelCompositeModelName": "TheValve",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
            "composedAssetModelId": &single_use_component,
        }),
    )
    .await;

    // Turbine: a composite model reusing the shared component model NESTED inside
    // an inline one, so depth is one of the things the rule generalises over.
    create_composite(
        router,
        &holders[1],
        json!({
            "assetModelCompositeModelId": IDS[3],
            "assetModelCompositeModelName": "Housing",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    create_composite(
        router,
        &holders[1],
        json!({
            "assetModelCompositeModelId": IDS[4],
            "assetModelCompositeModelName": "NestedPump",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": IDS[3],
            "composedAssetModelId": &reused_component,
        }),
    )
    .await;

    // Chiller: the shared component model reused once more, under another type,
    // and addressed by its `externalId:` label rather than its raw id.
    create_composite(
        router,
        &holders[2],
        json!({
            "assetModelCompositeModelId": IDS[5],
            "assetModelCompositeModelName": "AnotherPump",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
            "composedAssetModelId": &reused_component,
        }),
    )
    .await;

    Fixture {
        reused_component,
        single_use_component,
        unused_component,
        holders: [
            holders[0].clone(),
            holders[1].clone(),
            holders[2].clone(),
        ],
    }
}

/// The relationships of [`Fixture::reused_component`], in the order the listing
/// enumerates them: the holding asset models oldest first, and within one holder
/// the order its composite models were added.
///
/// One row per thing the rule generalises over: the asset model holding the
/// composite model, its nesting depth (top-level and one deep), its type (from
/// the catalog the `CompositionRelationshipSummary$assetModelCompositeModelType`
/// documentation states — `AWS/ALARM`, `CUSTOM` or `AWS/L4E_ANOMALY`) and
/// whether it carries an external id or a description.
fn expected_for_reused() -> Vec<Expected> {
    vec![
        Expected {
            case: "top-level, CUSTOM, described, external id",
            composite_id: IDS[0],
            holder: 0,
            composite_type: "CUSTOM",
        },
        Expected {
            case: "nested one deep, CUSTOM",
            composite_id: IDS[4],
            holder: 1,
            composite_type: "CUSTOM",
        },
        Expected {
            case: "top-level, AWS/L4E_ANOMALY, on a third asset model",
            composite_id: IDS[5],
            holder: 2,
            composite_type: "AWS/L4E_ANOMALY",
        },
    ]
}

/// `GET /asset-models/{label}/composition-relationships{query}`.
async fn list(router: &MultiServiceRouter, label: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/asset-models/{label}/composition-relationships{query}"),
        None,
    )
    .await
}

/// The summaries a `ListCompositionRelationships` body reports.
fn summaries(body: &Value) -> Vec<Value> {
    body["compositionRelationshipSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no compositionRelationshipSummaries in {body}"))
        .clone()
}

/// The composite model id of each summary, in the order the page reports them.
fn listed_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["assetModelCompositeModelId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("a summary without an assetModelCompositeModelId in {summary}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_every_composite_model_that_reuses_the_component_model() {
    let router = router();
    let fixture = fixture(&router).await;

    let (status, body) = list(&router, &fixture.reused_component, "?maxResults=250").await;
    assert_eq!(status, 200, "ListCompositionRelationships: got {status} {body}");
    let expected = expected_for_reused();
    assert_eq!(
        listed_ids(&body),
        expected
            .iter()
            .map(|row| row.composite_id.to_string())
            .collect::<Vec<String>>(),
        "every composite model reusing the component model must be listed, nested ones and \
         ones on other asset models included, got {body}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "a page that holds everything must carry no nextToken, got {body}"
    );

    // Every member must come from the SAME stored composite model the implemented
    // reader reports — the round trip through `DescribeAssetModelCompositeModel`,
    // whose `compositionDetails` names the component model this listing was
    // scoped to.
    let listed = summaries(&body);
    for row in &expected {
        let case = row.case;
        let summary = listed
            .iter()
            .find(|summary| summary["assetModelCompositeModelId"] == json!(row.composite_id))
            .unwrap_or_else(|| panic!("case {case:?}: not listed in {body}"));
        assert_eq!(
            summary["assetModelId"].as_str(),
            Some(fixture.holders[row.holder].as_str()),
            "case {case:?}: the summary must name the asset model holding the composite \
             model, got {summary}"
        );
        assert_eq!(
            summary["assetModelCompositeModelType"].as_str(),
            Some(row.composite_type),
            "case {case:?}: the summary must report the stored composite model type, \
             got {summary}"
        );

        let (status, described) = call(
            &router,
            "GET",
            &format!(
                "/asset-models/{}/composite-models/{}",
                fixture.holders[row.holder], row.composite_id
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelCompositeModel got {status} {described}"
        );
        assert_eq!(
            summary["assetModelCompositeModelType"],
            described["assetModelCompositeModelType"],
            "case {case:?}: the summary disagrees with the stored composite model \
             ({summary} vs {described})"
        );
        assert_eq!(
            described["compositionDetails"]["compositionRelationship"][0]["id"].as_str(),
            Some(fixture.reused_component.as_str()),
            "case {case:?}: the listed composite model must be the one that reuses this \
             component model, got {described}"
        );
    }
}

#[tokio::test]
async fn the_listing_is_scoped_to_the_asset_model_it_names() {
    let router = router();
    let fixture = fixture(&router).await;

    // Rows: each asset model of the fixture and the composite models reusing it —
    // a component model reused several times, one reused once, one reused by
    // none, and the asset models that HOLD composite models, which nothing
    // reuses.
    let single_relationship = vec![IDS[2].to_string()];
    let none: Vec<String> = Vec::new();
    let cases: Vec<(&str, &str, &Vec<String>)> = vec![
        (
            "a component model reused once",
            fixture.single_use_component.as_str(),
            &single_relationship,
        ),
        (
            "a component model nothing reuses",
            fixture.unused_component.as_str(),
            &none,
        ),
        (
            "an asset model that holds composite models but is reused by none",
            fixture.holders[0].as_str(),
            &none,
        ),
        (
            "another holder, likewise reused by none",
            fixture.holders[1].as_str(),
            &none,
        ),
    ];
    for (case, label, want) in cases {
        let (status, body) = list(&router, label, "?maxResults=250").await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            &listed_ids(&body),
            want,
            "case {case:?}: expected {want:?}, got {body}"
        );
        if want.is_empty() {
            assert!(
                body.get("nextToken").is_none_or(Value::is_null),
                "case {case:?}: an empty listing must carry no nextToken, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn paging_visits_every_relationship_exactly_once_whatever_the_page_size() {
    let router = router();
    let fixture = fixture(&router).await;
    let expected: Vec<String> = expected_for_reused()
        .iter()
        .map(|row| row.composite_id.to_string())
        .collect();

    // Rows: page sizes from one-per-page to larger than the enumeration,
    // including its exact length, one either side of it, and the published upper
    // bound of `maxResults` (range 1..250, per `smithy_query(query="operation",
    // operation="ListCompositionRelationships")`).
    for max_results in [1_usize, 2, 3, 4, 250] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|token| format!("&nextToken={token}"))
                .unwrap_or_default();
            let (status, body) = list(
                &router,
                &fixture.reused_component,
                &format!("?maxResults={max_results}{resume}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: page {pages} got {status} {body}"
            );
            let page = listed_ids(&body);
            assert!(
                page.len() <= max_results,
                "case maxResults={max_results}: page {pages} holds {} summaries, more than \
                 asked: {body}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={max_results}: paging did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={max_results}: the pages joined must be the whole enumeration once"
        );
        let want_pages = expected.len().div_ceil(max_results);
        assert_eq!(
            pages, want_pages,
            "case maxResults={max_results}: expected {want_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn the_next_token_names_the_first_relationship_of_the_next_page() {
    let router = router();
    let fixture = fixture(&router).await;

    // The token the handler issues after a page is the hex of the composite model
    // id the next page resumes from, computed here by an independent oracle — so
    // the assertion is about the token's content and not just that paging
    // round-trips.
    for (page_size, resumes_at) in [(1_usize, IDS[4]), (2, IDS[5])] {
        let (status, body) = list(
            &router,
            &fixture.reused_component,
            &format!("?maxResults={page_size}"),
        )
        .await;
        assert_eq!(status, 200, "case maxResults={page_size}: got {status} {body}");
        assert_eq!(
            body["nextToken"].as_str().unwrap_or(""),
            hex_token(resumes_at),
            "case maxResults={page_size}: the nextToken must name the first relationship of \
             the next page, got {body}"
        );
    }

    // And resuming from the last relationship answers exactly it.
    let (status, tail) = list(
        &router,
        &fixture.reused_component,
        &format!("?nextToken={}", hex_token(IDS[5])),
    )
    .await;
    assert_eq!(status, 200, "resumed page: got {status} {tail}");
    assert_eq!(
        listed_ids(&tail),
        vec![IDS[5].to_string()],
        "resuming from the last relationship must answer exactly it, got {tail}"
    );
}

#[tokio::test]
async fn max_results_is_refused_outside_its_published_range() {
    let router = router();
    let fixture = fixture(&router).await;

    // `maxResults` carries the modelled range 1..250: each published bound is
    // accepted and the value just past each bound is refused.
    for (max_results, accepted) in [(0_i64, false), (1, true), (250, true), (251, false)] {
        let (status, body) = list(
            &router,
            &fixture.reused_component,
            &format!("?maxResults={max_results}"),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults={max_results}: an out-of-range page size must be refused, \
                 got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_label_or_token_that_names_nothing_in_the_enumeration_is_a_modelled_refusal() {
    let router = router();
    let fixture = fixture(&router).await;

    // Rows: the ways a request can name something this enumeration does not hold.
    let cases: [(&str, String, String, u16, &str); 4] = [
        (
            "an asset model id no create minted",
            ABSENT_ID.to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a token naming a composite model in ANOTHER component model's \
             relationships",
            fixture.reused_component.clone(),
            format!("?nextToken={}", hex_token(IDS[2])),
            400,
            "InvalidRequestException",
        ),
        (
            "a token that is not a token this operation issued",
            fixture.reused_component.clone(),
            "?nextToken=zzz".to_string(),
            400,
            "InvalidRequestException",
        ),
        (
            "a token that decodes to text naming no composite model",
            fixture.reused_component.clone(),
            format!("?nextToken={}", hex_token("not-an-id")),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, label, query, want_status, want_code) in cases {
        let (status, body) = list(&router, &label, &query).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected HTTP {want_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(want_code),
            "case {case:?}: expected {want_code}, got {:?} in {body}",
            error_code(&body)
        );
    }

    // The absent asset model is reported as absent by its id, in this service's
    // one wording for that absence.
    let (status, body) = list(&router, ABSENT_ID, "").await;
    assert_eq!(status, 404, "the absent model: got {status} {body}");
    let want = format!("Asset Model with ID {ABSENT_ID} not found");
    assert_eq!(
        body["message"].as_str(),
        Some(want.as_str()),
        "the refusal must name the asset model the request asked for, got {body}"
    );
}

#[tokio::test]
async fn the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    let fixture = fixture(&router).await;

    let (status, first) = list(&router, &fixture.reused_component, "?maxResults=250").await;
    assert_eq!(status, 200, "the first read: got {status} {first}");

    // Rows: the same request again, and the same enumeration walked one page at a
    // time — a reporting operation answers identically however often it is
    // called.
    for (case, query) in [
        ("the same request again", "?maxResults=250"),
        ("the whole enumeration with no page size", ""),
    ] {
        let (status, body) = list(&router, &fixture.reused_component, query).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["compositionRelationshipSummaries"], first["compositionRelationshipSummaries"],
            "case {case:?}: must report the same relationships as the first read, got {body}"
        );
    }

    // And the composite models themselves are untouched by the reads: the asset
    // model still reports the one it holds.
    let (status, described) = call(
        &router,
        "GET",
        &format!(
            "/asset-models/{}/composite-models/{}",
            fixture.holders[0], IDS[0]
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModelCompositeModel: got {status} {described}");
    assert_eq!(
        described["compositionDetails"]["compositionRelationship"][0]["id"].as_str(),
        Some(fixture.reused_component.as_str()),
        "the stored composition relationship must be unchanged by the listings, got {described}"
    );
}
