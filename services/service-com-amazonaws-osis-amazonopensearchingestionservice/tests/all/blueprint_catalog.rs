// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPipelineBlueprints`: the catalog of Data Prepper blueprints
//! the service publishes.
//!
//! Every case drives the service the way a client does — one HTTP request per call,
//! through the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, asserted over the WHOLE published catalog rather than one
//! recorded blueprint:
//!
//! 1. A listing reports one `PipelineBlueprintSummary` per published blueprint, with every
//!    modelled summary member populated and nothing the summary does not model.
//! 2. The listing is deterministic: blueprint-name order, unique names, and a second call
//!    answers exactly what the first one did — a report changes nothing a later caller sees.
//! 3. The catalog is the same for every caller: the request carries no member that selects
//!    anything, so two callers signing for different Regions see one catalog.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A Region that is NOT the emulator default, so nothing here can be passing because it
/// was pinned to the default one.
const REGION: &str = "eu-west-2";

/// A second Region, for rule 3.
const OTHER_REGION: &str = "ap-southeast-2";

/// Every member `PipelineBlueprintSummary` models (smithy_query: shape
/// PipelineBlueprintSummary).
const SUMMARY_MEMBERS: [&str; 5] = [
    "BlueprintName",
    "DisplayName",
    "DisplayDescription",
    "Service",
    "UseCase",
];

/// The listing must report the WHOLE published catalog, so a handler that filtered or
/// truncated it fails here rather than passing on a single entry. The catalog is
/// service-shipped data (`BLUEPRINT_CATALOG` in `src/state.rs`, following the
/// `AWS-<source>Pipeline` blueprints of the "Using blueprints to create a pipeline" page
/// that `ListPipelineBlueprints`' own model doc points at), and AWS grows it over time, so
/// the floor is a minimum rather than an exact set.
const MIN_PUBLISHED_BLUEPRINTS: usize = 7;

/// One published name, checked explicitly so a catalog that silently loses entries is
/// caught. Deliberately NOT one of the blueprints the recorded units named.
const A_PUBLISHED_BLUEPRINT: &str = "AWS-VPCFlowLogPipeline";

/// A published blueprint name has the service's `AWS-<source>Pipeline` form.
fn is_aws_blueprint_name(name: &str) -> bool {
    name.starts_with("AWS-") && name.ends_with("Pipeline") && name.len() > "AWS-Pipeline".len()
}

/// What one call answered: the status and the parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

/// `ListPipelineBlueprints`, signed for `region`.
async fn list_blueprints_in(router: &MultiServiceRouter, region: &str) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://osis.{region}.amazonaws.com/2022-01-01/osis/listPipelineBlueprints"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(b"{}".to_vec()))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

async fn list_blueprints(router: &MultiServiceRouter) -> Answer {
    list_blueprints_in(router, REGION).await
}

/// The `Blueprints` member of a successful listing.
fn summaries(answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "expected 200 from ListPipelineBlueprints, got {} / {:?}",
        answer.status, answer.body
    );
    answer.body["Blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a `Blueprints` array, got {:?}", answer.body))
        .clone()
}

fn text(value: &Value, member: &str, case: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `{member}` in {value:?}"))
        .to_string()
}

fn names(blueprints: &[Value]) -> Vec<String> {
    blueprints
        .iter()
        .map(|summary| text(summary, "BlueprintName", "<listing>"))
        .collect()
}

/// Rule 1: one summary per published blueprint, every modelled member populated, and no
/// member the summary shape does not model.
#[tokio::test]
async fn listing_reports_every_published_blueprint_with_all_summary_members() {
    let router = new_router();
    let listed = list_blueprints(&router).await;
    let blueprints = summaries(&listed);

    assert!(
        blueprints.len() >= MIN_PUBLISHED_BLUEPRINTS,
        "expected at least {MIN_PUBLISHED_BLUEPRINTS} published blueprints, got {} / {:?}",
        blueprints.len(),
        listed.body
    );

    for summary in &blueprints {
        let case = text(summary, "BlueprintName", "<listing>");
        for member in SUMMARY_MEMBERS {
            let value = text(summary, member, &case);
            assert!(
                !value.is_empty(),
                "case {case:?}: summary member `{member}` must be populated, got {value:?}"
            );
        }

        let members = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary:?}"));
        for member in members.keys() {
            assert!(
                SUMMARY_MEMBERS.contains(&member.as_str()),
                "case {case:?}: `{member}` is not a member of PipelineBlueprintSummary, \
                 got {summary:?}"
            );
        }
        // In particular, the configuration body belongs to `PipelineBlueprint`, not to a
        // summary: a listing must not carry it.
        assert!(
            summary.get("PipelineConfigurationBody").is_none(),
            "case {case:?}: a summary carries no configuration body, got {summary:?}"
        );
    }

    // Each name has the service's published form, and a named entry is really there — so a
    // listing that quietly dropped or renamed entries cannot pass.
    let listed_names = names(&blueprints);
    for name in &listed_names {
        assert!(
            is_aws_blueprint_name(name),
            "case {name:?}: a published blueprint is named AWS-<source>Pipeline, got {name:?}"
        );
    }
    assert!(
        listed_names.iter().any(|name| name == A_PUBLISHED_BLUEPRINT),
        "the listing must report the published blueprint {A_PUBLISHED_BLUEPRINT:?}, \
         got {listed_names:?}"
    );
}

/// Rule 2: the listing is ordered by blueprint name, holds each name once, and repeats
/// byte for byte — a read changes nothing a later caller sees.
#[tokio::test]
async fn listing_is_ordered_unique_and_repeatable() {
    let router = new_router();
    let first = list_blueprints(&router).await;
    let blueprints = summaries(&first);
    let listed_names = names(&blueprints);

    let mut sorted = listed_names.clone();
    sorted.sort();
    assert_eq!(
        listed_names, sorted,
        "the listing must be in blueprint-name order, got {listed_names:?}"
    );

    let mut deduped = sorted.clone();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        listed_names.len(),
        "blueprint names must be unique, got {listed_names:?}"
    );

    // Three calls, so a handler that mutated the catalog on the first read (consuming it,
    // reordering it, or appending to it) fails here.
    for call in 1..=3 {
        let again = list_blueprints(&router).await;
        assert_eq!(
            summaries(&again),
            blueprints,
            "call {call}: a listing must not change what a later caller sees, got {:?}",
            again.body
        );
    }
}

/// Rule 3: the request selects nothing, so the catalog does not depend on the caller — a
/// listing signed for another Region reports the same records.
#[tokio::test]
async fn the_catalog_is_the_same_for_every_caller() {
    let router = new_router();
    let here = list_blueprints_in(&router, REGION).await;
    let there = list_blueprints_in(&router, OTHER_REGION).await;
    assert_eq!(
        summaries(&there),
        summaries(&here),
        "case {OTHER_REGION:?}: the published catalog is caller-independent, got {:?} vs {:?}",
        there.body,
        here.body
    );

    // A second, independent service instance publishes the same catalog: it is seeded from
    // the service's own data, not accumulated by use.
    let fresh = new_router();
    assert_eq!(
        summaries(&list_blueprints(&fresh).await),
        summaries(&here),
        "a fresh store must publish the same catalog, got {:?}",
        here.body
    );
}
