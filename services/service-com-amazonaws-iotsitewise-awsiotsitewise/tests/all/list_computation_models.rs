// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListComputationModels`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListComputationModels` enumerates the computation
//! models the service's state holds, each exactly once, in ONE stable order,
//! narrowed to the `computationModelType` the request asks for (every type when it
//! asks for none), cut into pages of at most `maxResults` entries, where
//! `nextToken` is present only while another entry follows and resumes the
//! enumeration exactly where the previous page stopped; a listed model carries the
//! same values `DescribeComputationModel` reports for it, and a `nextToken` this
//! operation never issued is refused rather than answered with an empty page.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`,
//! `is_modelled_id`) and the computation-model fixtures (`anomaly_detection`,
//! `asset_model_property`, `expected_arn`, `id_of`) are the ones the asset and
//! computation-model suites already have; this suite reuses them rather than
//! growing a second copy.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};
use crate::computation_model_lifecycle::{
    anomaly_detection, asset_model_property, expected_arn, id_of,
};

/// The collection this operation lists, as `OPERATIONS.md` reports the route
/// (`GET /computation-models`).
const COMPUTATION_MODELS: &str = "/computation-models";

/// The one value the modelled `ComputationModelType` enum declares — source
/// `smithy_query(query="operation", operation="ListComputationModels")`:
/// `computationModelType` (one of: ANOMALY_DETECTION).
const ANOMALY_DETECTION: &str = "ANOMALY_DETECTION";

/// One row of the fixture: the model's name, the description it was created with
/// (or none), and the variable names its configuration references, which is also
/// the shape of the data binding it carries.
struct ModelCase {
    name: &'static str,
    description: Option<&'static str>,
    /// The data-binding variables, each bound to a distinct asset-model property.
    /// A configuration's reference to one carries the modelled pattern
    /// `^\$\{[a-z][a-z0-9_]*\}$`, so each name below is in that form.
    variables: (&'static str, &'static str),
    /// Whether the create carries a `clientToken`. The member carries the modelled
    /// pattern `^\S{36,64}$`, so each token below is at least 36 non-space
    /// characters.
    client_token: Option<&'static str>,
}

/// The rows: seven computation models differing in whether they were created with
/// a description, in the variable names their configuration references, and in
/// whether their create carried a `clientToken`, so a page boundary never lands
/// only between two identically shaped models.
fn model_cases() -> Vec<ModelCase> {
    vec![
        ModelCase {
            name: "Pump anomalies",
            description: None,
            variables: ("input", "result"),
            client_token: None,
        },
        ModelCase {
            name: "Turbine anomalies",
            description: Some("Detects turbine anomalies"),
            variables: ("input", "result"),
            client_token: Some("list-computation-models-turbine-token"),
        },
        ModelCase {
            name: "Boiler anomalies",
            description: Some("Detects boiler anomalies"),
            variables: ("measured", "detected"),
            client_token: None,
        },
        ModelCase {
            name: "Motor anomalies",
            description: None,
            variables: ("motor_input", "motor_result"),
            client_token: Some("list-computation-models-motor-token-01"),
        },
        ModelCase {
            name: "Bearing anomalies",
            description: Some("A"),
            variables: ("a", "b"),
            client_token: None,
        },
        ModelCase {
            name: "Gearbox anomalies #1",
            description: Some("Detects gearbox anomalies"),
            variables: ("gearbox_input", "gearbox_result"),
            client_token: None,
        },
        ModelCase {
            name: "Compressor anomalies",
            description: None,
            variables: ("input", "result"),
            client_token: None,
        },
    ]
}

/// Create one computation model from a row and return the id it was minted with.
async fn create_model(router: &MultiServiceRouter, case: &ModelCase) -> String {
    let (input, result) = case.variables;
    let mut body = json!({
        "computationModelName": case.name,
        "computationModelConfiguration": anomaly_detection(input, result),
        "computationModelDataBinding": {
            input: asset_model_property(&id_of('1'), &id_of('2')),
            result: asset_model_property(&id_of('1'), &id_of('3')),
        },
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert(
            "computationModelDescription".to_string(),
            json!(description),
        );
    }
    if let Some(client_token) = case.client_token {
        object.insert("clientToken".to_string(), json!(client_token));
    }

    let (status, created) = call(router, "POST", COMPUTATION_MODELS, Some(body)).await;
    assert_eq!(
        status, 202,
        "case {:?}: CreateComputationModel expected 202, got {status} {created}",
        case.name
    );
    let id = created["computationModelId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: computationModelId must be a modelled UUID, got {created}",
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

/// Build one `ListComputationModels` URI from its query parameters.
fn uri(query: &[String]) -> String {
    if query.is_empty() {
        COMPUTATION_MODELS.to_string()
    } else {
        format!("{COMPUTATION_MODELS}?{}", query.join("&"))
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
        let summaries = body["computationModelSummaries"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: computationModelSummaries must be a list, got {body}")
            });
        // The page size this listing answers with when the caller sends no
        // `maxResults`.
        let limit = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
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

    let expected_pages = seen.len().div_ceil(page_size.unwrap_or(DEFAULT_PAGE_SIZE)).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {} models, walked {pages}",
        seen.len()
    );
    seen
}

/// The page size a call that names no `maxResults` is answered with.
const DEFAULT_PAGE_SIZE: usize = 50;

#[tokio::test]
async fn list_computation_models_pages_through_every_stored_model_once() {
    let router = router();
    let created = create_fixture(&router).await;

    // The reference order: the enumeration one whole-answer call reports.
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

    // Rows differ in page size: the published `maxResults` bounds (1 and 250),
    // sizes that divide the fixture and sizes that do not, a size past the fixture,
    // and no `maxResults` at all.
    for page_size in [
        Some(1_usize),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(8),
        Some(250),
        None,
    ] {
        let seen = walk(&router, page_size, &[]).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }

    // The page size a call naming no `maxResults` is answered with is at least the
    // whole fixture, so that call is one page and issues no token: the walk above
    // caps a default page at `DEFAULT_PAGE_SIZE`, and this pins the other side of
    // it.
    let (status, body) = call(&router, "GET", COMPUTATION_MODELS, None).await;
    assert_eq!(
        status, 200,
        "case \"no maxResults at all\": expected 200, got {status} {body}"
    );
    let ids: Vec<String> = body["computationModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"no maxResults at all\": no summaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids, reference,
        "case \"no maxResults at all\": one page must carry the whole enumeration, got {ids:?}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case \"no maxResults at all\": a page that leaves nothing to resume issues no token, \
         got {body}"
    );
}

#[tokio::test]
async fn a_listed_computation_model_carries_what_describe_reports() {
    let router = router();
    let created = create_fixture(&router).await;

    let (status, body) = call(&router, "GET", &uri(&["maxResults=250".to_string()]), None).await;
    assert_eq!(
        status, 200,
        "ListComputationModels expected 200, got {status} {body}"
    );
    let summaries = body["computationModelSummaries"]
        .as_array()
        .expect("computationModelSummaries")
        .clone();
    assert_eq!(
        summaries.len(),
        created.len(),
        "every created model must be listed, got {body}"
    );

    // The summary member and the `DescribeComputationModel` member that reports the
    // same stored value. The status is left out of this table: it is derived from
    // the clock, so the two reads can straddle the transition the create owes; the
    // state it carries is checked below against the two values that transition runs
    // between.
    let same: [(&str, &str); 7] = [
        ("id", "computationModelId"),
        ("arn", "computationModelArn"),
        ("name", "computationModelName"),
        ("description", "computationModelDescription"),
        ("creationDate", "computationModelCreationDate"),
        ("lastUpdateDate", "computationModelLastUpdateDate"),
        ("version", "computationModelVersion"),
    ];
    for summary in &summaries {
        let id = summary["id"].as_str().unwrap_or("");
        let (status, described) =
            call(&router, "GET", &format!("{COMPUTATION_MODELS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribeComputationModel expected 200, got {status} {described}"
        );
        for (listed, described_as) in same {
            assert_eq!(
                summary[listed], described[described_as],
                "case {id:?}: the summary's {listed} must be what DescribeComputationModel \
                 reports as {described_as}: {summary} vs {described}"
            );
        }
        // The type a summary reports follows from the configuration the model was
        // created with, which is an `anomalyDetection` one for every row.
        assert_eq!(
            summary["type"],
            json!(ANOMALY_DETECTION),
            "case {id:?}: unexpected type in {summary}"
        );
        assert!(
            described["computationModelConfiguration"]["anomalyDetection"].is_object(),
            "case {id:?}: the described configuration must name the listed type, got {described}"
        );
        // The state is whichever side of the owed CREATING -> ACTIVE transition this
        // read fell on, and nothing else.
        let state = summary["status"]["state"].as_str().unwrap_or("");
        assert!(
            state == "CREATING" || state == "ACTIVE",
            "case {id:?}: expected CREATING or ACTIVE, got {summary}"
        );
    }

    // A model created with a name, a description and the documented ARN form is
    // listed under them, so the row the summary came from is identifiable without
    // consulting the store.
    for (id, case) in &created {
        let summary = summaries
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: not listed, got {body}", case.name));
        assert_eq!(
            summary["name"],
            json!(case.name),
            "case {:?}: unexpected name in {summary}",
            case.name
        );
        assert_eq!(
            summary["description"],
            case.description.map(Value::from).unwrap_or(Value::Null),
            "case {:?}: unexpected description in {summary}",
            case.name
        );
        assert_eq!(
            summary["arn"],
            json!(expected_arn(id)),
            "case {:?}: unexpected arn in {summary}",
            case.name
        );
        // "The version of the computation model": the create is the first accepted
        // write, so a model nothing has updated is at version 1.
        assert_eq!(
            summary["version"],
            json!("1"),
            "case {:?}: unexpected version in {summary}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_computation_model_type_filter_selects_exactly_those_types() {
    let router = router();
    let created = create_fixture(&router).await;
    let all: BTreeSet<String> = created.iter().map(|(id, _)| id.clone()).collect();

    // Rows differ in WHICH type is asked for: none at all ("If a
    // computationModelType is not provided, all types of computation models are
    // returned") and each value the modelled enum declares.
    let selections: [Option<&str>; 2] = [None, Some(ANOMALY_DETECTION)];
    for selection in selections {
        let query: Vec<String> = selection
            .map(|model_type| vec![format!("computationModelType={model_type}")])
            .unwrap_or_default();
        let seen: BTreeSet<String> = walk(&router, Some(250), &query).await.into_iter().collect();
        // Every fixture row is created from an `anomalyDetection` configuration, so
        // both rows select the whole fixture; a type the enum does not declare is
        // refused below rather than answered with an empty page.
        assert_eq!(
            seen, all,
            "case {selection:?}: the listing must hold exactly the models of that type, \
             got {seen:?}"
        );
    }

    // A type outside the modelled enum is refused rather than answered with an
    // empty page. Rows differ in HOW the value is wrong: a neighbouring spelling, a
    // value of another enum of this service, and two values at once.
    for model_type in [
        "anomaly_detection",
        "ANOMALY-DETECTION",
        "ASSET_MODEL",
        "ANOMALY_DETECTION,ANOMALY_DETECTION",
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &uri(&[format!("computationModelType={model_type}")]),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {model_type:?}: an unmodelled computationModelType must be refused, \
             got {status} {body}"
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
        (
            "hex of something that is no id",
            hex_token("Pump anomalies"),
        ),
        ("odd-length hex", "abc".to_string()),
    ];
    for (case, token) in forged {
        let (status, body) =
            call(&router, "GET", &uri(&[format!("nextToken={token}")]), None).await;
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

    // A token this operation DID issue resumes the enumeration exactly where the
    // page that issued it stopped, and is the hex of that resume point's own id,
    // computed here by an independent oracle rather than by the handler's encoder.
    let reference = walk(&router, Some(250), &[]).await;
    let (status, first) = call(&router, "GET", &uri(&["maxResults=1".to_string()]), None).await;
    assert_eq!(
        status, 200,
        "the first page expected 200, got {status} {first}"
    );
    assert_eq!(
        first["nextToken"].as_str().unwrap_or(""),
        hex_token(&reference[1]),
        "case \"the first of several pages\": unexpected resume token in {first}"
    );

    // The refusals left the enumeration intact.
    let seen = walk(&router, Some(250), &[]).await;
    assert_eq!(
        seen, reference,
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
    assert_eq!(
        seen.len(),
        created.len(),
        "every created model must still be listed, saw {seen:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let _ = create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListComputationModels"): each published bound is served and one
    // past it on either side is refused.
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

#[tokio::test]
async fn listing_computation_models_does_not_change_what_a_later_caller_sees() {
    let router = router();

    // A store with no computation model reports an empty list and no token, rather
    // than a fabricated entry or a token that resumes nothing.
    let (status, empty) = call(&router, "GET", COMPUTATION_MODELS, None).await;
    assert_eq!(
        status, 200,
        "an empty store expected 200, got {status} {empty}"
    );
    assert_eq!(
        empty["computationModelSummaries"],
        json!([]),
        "an empty store lists no computation model, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let created = create_fixture(&router).await;
    // Past the transition the create owes, so the derived status cannot move
    // between the two reads of a row below.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // Rows differ in page size and in the filter applied; each read twice. A read
    // never moves the enumeration.
    let cases: [(&str, &str); 4] = [
        ("a single-entry page", "maxResults=1"),
        ("the whole enumeration", "maxResults=250"),
        (
            "one type",
            "maxResults=250&computationModelType=ANOMALY_DETECTION",
        ),
        ("no query parameters at all", ""),
    ];
    for (case, query) in cases {
        let target = if query.is_empty() {
            COMPUTATION_MODELS.to_string()
        } else {
            uri(&[query.to_string()])
        };
        let (status, first) = call(&router, "GET", &target, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {first}"
        );
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

    // And the models themselves still read back one by one after all that listing.
    for (id, case) in &created {
        let (status, described) =
            call(&router, "GET", &format!("{COMPUTATION_MODELS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeComputationModel expected 200 after the listings, got \
             {status} {described}",
            case.name
        );
    }
}

/// Comfortably past the span the create's owed CREATING -> ACTIVE transition
/// falls due in.
const PAST_THE_OWED_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);
