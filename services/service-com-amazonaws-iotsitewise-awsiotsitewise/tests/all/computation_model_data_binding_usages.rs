// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListComputationModelDataBindingUsages`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: the listing answers from the data bindings the
//! stored computation models were created with — one summary per DISTINCT binding
//! value the filter's single data source matches, naming every model that holds
//! it — and it reads the store without changing it.
//!
//! The table spans what that rule generalises over: which of the four forms the
//! filter takes, whether the binding sits directly under a variable or inside a
//! nested `list`, how many models share one binding, and how many bindings of one
//! model match.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, hex_token, router, router_with_engine};
use crate::computation_model_lifecycle::{anomaly_detection, id_of};

/// `POST /computation-models/data-binding-usages`, as `OPERATIONS.md` reports the
/// route.
const USAGES_URI: &str = "/computation-models/data-binding-usages";

/// A `ComputationModelDataBindingValue` naming one asset's property.
fn asset_property(asset_id: &str, property_id: &str) -> Value {
    json!({ "assetProperty": { "assetId": asset_id, "propertyId": property_id } })
}

/// A `ComputationModelDataBindingValue` naming an asset model's property.
fn asset_model_property(asset_model_id: &str, property_id: &str) -> Value {
    json!({
        "assetModelProperty": { "assetModelId": asset_model_id, "propertyId": property_id }
    })
}

/// A `ComputationModelDataBindingValue` holding a LIST of binding values.
fn binding_list(values: Vec<Value>) -> Value {
    json!({ "list": values })
}

/// Create a computation model bound to `binding` and answer the id it was minted
/// with.
async fn create_model(router: &MultiServiceRouter, name: &str, binding: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/computation-models",
        Some(json!({
            "computationModelName": name,
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": binding,
        })),
    )
    .await;
    assert_eq!(status, 202, "create {name}: got {status} {created}");
    created["computationModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name}: no minted id in {created}"))
        .to_string()
}

/// `ListComputationModelDataBindingUsages` with `filter` and, when given, a page
/// size and a resume token. Both paging members live in the BODY for this
/// operation, not in the query string.
async fn list(
    router: &MultiServiceRouter,
    filter: Value,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "dataBindingValueFilter": filter });
    let object = body.as_object_mut().expect("an object body");
    if let Some(max_results) = max_results {
        object.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        object.insert("nextToken".to_string(), json!(next_token));
    }
    call(router, "POST", USAGES_URI, Some(body)).await
}

/// The summaries of one answer as `matched binding value -> the computation model
/// ids that hold it`, so a case states what it expects without depending on the
/// order the answer happens to carry (the paging suite below pins the order).
fn usages(body: &Value) -> BTreeMap<String, Vec<String>> {
    body["dataBindingUsageSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no dataBindingUsageSummaries list in {body}"))
        .iter()
        .map(|summary| {
            let value = summary["matchedDataBinding"]["value"].clone();
            let ids = summary["computationModelIds"]
                .as_array()
                .unwrap_or_else(|| panic!("no computationModelIds list in {summary}"))
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect();
            (value.to_string(), ids)
        })
        .collect()
}

/// The models every filter case below is answered from, and the ids they were
/// minted with.
struct Fixture {
    router: MultiServiceRouter,
    /// Binds `(asset_one, property_one)` and `(model_one, property_two)`.
    alpha: String,
    /// Binds `(asset_one, property_one)` — the same asset property `alpha` binds —
    /// and `(asset_one, property_three)`.
    beta: String,
    /// Binds `(model_one, property_two)` and `(asset_two, property_one)` inside a
    /// nested `list`, and `(model_two, property_four)` directly.
    gamma: String,
}

/// The ids the fixture's bindings reference. Each is a modelled `ID`: a lowercase
/// hyphenated UUID.
fn asset_one() -> String {
    id_of('1')
}
fn asset_two() -> String {
    id_of('2')
}
fn model_one() -> String {
    id_of('3')
}
fn model_two() -> String {
    id_of('4')
}
fn property_one() -> String {
    id_of('5')
}
fn property_two() -> String {
    id_of('6')
}
fn property_three() -> String {
    id_of('7')
}
fn property_four() -> String {
    id_of('8')
}

async fn fixture() -> Fixture {
    let router = router();
    let alpha = create_model(
        &router,
        "alpha",
        json!({
            "input": asset_property(&asset_one(), &property_one()),
            "result": asset_model_property(&model_one(), &property_two()),
        }),
    )
    .await;
    let beta = create_model(
        &router,
        "beta",
        json!({
            "input": asset_property(&asset_one(), &property_one()),
            "result": asset_property(&asset_one(), &property_three()),
        }),
    )
    .await;
    let gamma = create_model(
        &router,
        "gamma",
        json!({
            // A binding value's `list` holds binding values in turn, so the two
            // data sources below are reached only by walking into it.
            "input": binding_list(vec![
                asset_model_property(&model_one(), &property_two()),
                binding_list(vec![asset_property(&asset_two(), &property_one())]),
            ]),
            "result": asset_model_property(&model_two(), &property_four()),
        }),
    )
    .await;
    Fixture {
        router,
        alpha,
        beta,
        gamma,
    }
}

/// One row of the filter table: the `DataBindingValueFilter` a request carries and
/// the `matched binding -> model ids` the store owes it.
struct FilterCase {
    label: &'static str,
    filter: Value,
    expected: Vec<(Value, Vec<String>)>,
}

#[tokio::test]
async fn each_filter_form_lists_the_bindings_that_reference_its_data_source() {
    let fixture = fixture().await;
    let (alpha, beta, gamma) = (
        fixture.alpha.clone(),
        fixture.beta.clone(),
        fixture.gamma.clone(),
    );
    // The ids are minted, so the order a summary lists them in is their own sort
    // order rather than the order the models were created in.
    let sorted = |mut ids: Vec<String>| {
        ids.sort();
        ids
    };

    let rows = vec![
        FilterCase {
            label: "asset: every binding referencing one asset",
            filter: json!({ "asset": { "assetId": asset_one() } }),
            expected: vec![
                (
                    asset_property(&asset_one(), &property_one()),
                    sorted(vec![alpha.clone(), beta.clone()]),
                ),
                (
                    asset_property(&asset_one(), &property_three()),
                    vec![beta.clone()],
                ),
            ],
        },
        FilterCase {
            label: "asset: a binding reached only through a nested list",
            filter: json!({ "asset": { "assetId": asset_two() } }),
            expected: vec![(
                asset_property(&asset_two(), &property_one()),
                vec![gamma.clone()],
            )],
        },
        FilterCase {
            label: "asset: an asset nothing binds",
            filter: json!({ "asset": { "assetId": id_of('9') } }),
            expected: Vec::new(),
        },
        FilterCase {
            label: "assetModel: every binding referencing one asset model",
            filter: json!({ "assetModel": { "assetModelId": model_one() } }),
            expected: vec![(
                asset_model_property(&model_one(), &property_two()),
                sorted(vec![alpha.clone(), gamma.clone()]),
            )],
        },
        FilterCase {
            label: "assetModel: a model only one computation model binds",
            filter: json!({ "assetModel": { "assetModelId": model_two() } }),
            expected: vec![(
                asset_model_property(&model_two(), &property_four()),
                vec![gamma.clone()],
            )],
        },
        FilterCase {
            label: "assetProperty: one property of one asset",
            filter: json!({
                "assetProperty": { "assetId": asset_one(), "propertyId": property_one() }
            }),
            expected: vec![(
                asset_property(&asset_one(), &property_one()),
                sorted(vec![alpha.clone(), beta.clone()]),
            )],
        },
        FilterCase {
            label: "assetProperty: a property of that asset nothing binds",
            filter: json!({
                "assetProperty": { "assetId": asset_one(), "propertyId": property_four() }
            }),
            expected: Vec::new(),
        },
        FilterCase {
            label: "assetProperty: an asset-model property is not an asset property",
            filter: json!({
                "assetProperty": { "assetId": model_one(), "propertyId": property_two() }
            }),
            expected: Vec::new(),
        },
        FilterCase {
            label: "assetModelProperty: one property of one asset model",
            filter: json!({
                "assetModelProperty": {
                    "assetModelId": model_one(), "propertyId": property_two()
                }
            }),
            expected: vec![(
                asset_model_property(&model_one(), &property_two()),
                sorted(vec![alpha.clone(), gamma.clone()]),
            )],
        },
        FilterCase {
            label: "assetModelProperty: an asset property is not an asset-model property",
            filter: json!({
                "assetModelProperty": {
                    "assetModelId": asset_one(), "propertyId": property_one()
                }
            }),
            expected: Vec::new(),
        },
    ];

    for row in &rows {
        let (status, body) = list(&fixture.router, row.filter.clone(), None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the listing answered {status} {body}",
            row.label
        );
        let expected: BTreeMap<String, Vec<String>> = row
            .expected
            .iter()
            .map(|(value, ids)| (value.to_string(), ids.clone()))
            .collect();
        let actual = usages(&body);
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            row.label
        );
        assert!(
            body["nextToken"].is_null(),
            "case {:?}: a single page reported a resume token: {body}",
            row.label
        );
    }
}

#[tokio::test]
async fn a_filter_naming_no_single_data_source_is_refused() {
    let fixture = fixture().await;

    // `DataBindingValueFilter` carries one optional member per kind of data
    // source, so a request names exactly one: none and several are both bad
    // requests.
    let rows = [
        ("no member at all", json!({})),
        (
            "two members",
            json!({
                "asset": { "assetId": asset_one() },
                "assetModel": { "assetModelId": model_one() },
            }),
        ),
        (
            "every member",
            json!({
                "asset": { "assetId": asset_one() },
                "assetModel": { "assetModelId": model_one() },
                "assetProperty": { "assetId": asset_one(), "propertyId": property_one() },
                "assetModelProperty": {
                    "assetModelId": model_one(), "propertyId": property_two()
                },
            }),
        ),
    ];

    for (label, filter) in &rows {
        let (status, body) = list(&fixture.router, filter.clone(), None, None).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the listing answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_through_every_match_in_one_order() {
    let fixture = fixture().await;
    let filter = json!({ "asset": { "assetId": asset_one() } });

    // The whole answer, which the pages below have to add up to.
    let (status, whole) = list(&fixture.router, filter.clone(), None, None).await;
    assert_eq!(status, 200, "the unpaged listing answered {status} {whole}");
    let summaries = whole["dataBindingUsageSummaries"]
        .as_array()
        .expect("a summaries list")
        .clone();
    assert_eq!(
        summaries.len(),
        2,
        "case \"one asset, two bound properties\": expected two summaries, got {whole}"
    );

    // One row per page: a page of one summary carries a resume token, and the last
    // page carries none.
    let mut paged = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..summaries.len() {
        let (status, body) = list(&fixture.router, filter.clone(), Some(1), token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case \"page {page}\": the listing answered {status} {body}"
        );
        let got = body["dataBindingUsageSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case \"page {page}\": no summaries in {body}"))
            .clone();
        assert_eq!(
            got.len(),
            1,
            "case \"page {page}\": expected one summary, got {body}"
        );
        paged.push(got[0].clone());
        token = body["nextToken"].as_str().map(ToString::to_string);
        let last = page + 1 == summaries.len();
        assert_eq!(
            token.is_none(),
            last,
            "case \"page {page}\": expected a resume token {}, got {body}",
            if last { "absent" } else { "present" }
        );
    }
    assert_eq!(
        paged, summaries,
        "case \"paged in ones\": expected {summaries:?}, got {paged:?}"
    );

    // The token is the hex of the resume point's own key, computed here by an
    // independent oracle rather than by the handler's encoder.
    let (status, first) = list(&fixture.router, filter.clone(), Some(1), None).await;
    assert_eq!(status, 200, "the first page answered {status} {first}");
    let resume = format!("assetProperty|{}|{}", asset_one(), property_three());
    assert_eq!(
        first["nextToken"].as_str().unwrap_or(""),
        hex_token(&resume),
        "case \"the first of two pages\": unexpected resume token in {first}"
    );

    // A token this listing never issued is a bad request, not an empty page.
    for label_and_token in [
        ("not hex at all", "zz=="),
        ("hex of a key nothing matches", &hex_token("assetProperty|x|y")),
    ] {
        let (label, bad) = label_and_token;
        let (status, body) = list(&fixture.router, filter.clone(), None, Some(bad)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the listing answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn listing_the_usages_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let model = create_model(
        &router,
        "read-only",
        json!({
            "input": asset_property(&asset_one(), &property_one()),
            "result": asset_model_property(&model_one(), &property_two()),
        }),
    )
    .await;
    let before = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .clone();

    let filter = json!({ "asset": { "assetId": asset_one() } });
    let (status, first) = list(&router, filter.clone(), None, None).await;
    assert_eq!(status, 200, "the first listing answered {status} {first}");
    let (status, second) = list(&router, filter, None, None).await;
    assert_eq!(status, 200, "the second listing answered {status} {second}");
    assert_eq!(
        second, first,
        "case \"listed twice\": expected {first}, got {second}"
    );
    assert_eq!(
        usages(&first).get(&asset_property(&asset_one(), &property_one()).to_string()),
        Some(&vec![model.clone()]),
        "case \"listed twice\": the one bound model is missing from {first}"
    );

    let after = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .clone();
    assert_eq!(
        after.len(),
        before.len(),
        "case \"listed twice\": the listing changed how many models the store holds"
    );
    for (id, model) in &before {
        let held = after
            .get(id)
            .unwrap_or_else(|| panic!("case \"listed twice\": the listing dropped model {id}"));
        assert_eq!(
            format!("{held:?}"),
            format!("{model:?}"),
            "case \"listed twice\": the listing rewrote model {id}"
        );
    }
}
