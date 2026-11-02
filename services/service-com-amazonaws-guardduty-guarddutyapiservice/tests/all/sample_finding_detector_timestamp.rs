// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when sample findings
//! are generated on it.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of requests rather than the one a recorded
//! call made: a call that changes WHICH sub-resources a detector holds changes the
//! detector record itself, so `GetDetector`'s `UpdatedAt` — "the last-updated timestamp
//! for the detector" (`smithy_query(operation="GetDetector")`) — moves to the instant of
//! that call, while a call that leaves that set alone moves it nowhere, whether it
//! persisted nothing (a refusal), changed only one sub-resource's own contents (another
//! observation aggregated onto a finding already stored) or changed nothing at all (a
//! read). `detector_sub_resource_creates`, `filter_detector_timestamp` and
//! `investigation_detector_timestamp` assert the same rule over the detector's other
//! sub-resource families; this suite covers the finding family, whose only create is
//! `CreateSampleFindings`.
//!
//! The rows differ along what the rule generalises over — HOW MANY finding types a
//! request names (one, two, the 50 the model's `FindingTypes` range tops out at), WHICH
//! affected resource types they span, and the FORM a request that names none takes (an
//! empty list, or no member at all, both of which generate "sample findings of all
//! supported finding types", `smithy_query(query="doc",shape="CreateSampleFindings")`).
//! The catalogue the rows draw their types from is read over the wire rather than
//! written out here, so no row can drift from the types the service actually supports.
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these assertions
//! order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{all_finding_ids, call, enabled_detector, new_router, Router};
// The detector's own `UpdatedAt`, and a read of generated findings, come from the
// helpers the other suites already share, so every suite sees them the same way.
use crate::refused_create_clock::detector_updated_at;
use crate::sample_finding_catalog::get_findings;

/// The top of `CreateSampleFindings.findingTypes`' modelled range (`FindingTypes` is
/// `len 0..50`, `smithy_query(operation="CreateSampleFindings")`).
const MAX_FINDING_TYPES: usize = 50;

/// A string that is not a GuardDuty finding type: a real threat purpose and affected
/// resource type under an invented threat family, so only the catalogue — not the shape
/// of the identifier — can refuse it.
const UNCATALOGUED_FINDING_TYPE: &str = "Recon:EC2/NoSuchThreatFamilyHere";

/// Ask for sample findings with `body` as the request.
async fn create_sample_findings(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(body),
    )
    .await
}

/// Every finding type the service supports, read OVER THE WIRE: a request that names no
/// finding type generates a sample of all supported types, so the types of the findings
/// it leaves behind are the catalogue itself.
async fn published_finding_types() -> Vec<String> {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, got) = create_sample_findings(&router, &detector_id, json!({})).await;
    assert_eq!(
        status, 200,
        "reading the catalogue: expected create-sample-findings to answer 200, got {status} \
         {got:?}"
    );
    let ids = all_finding_ids(&router, &detector_id, MAX_FINDING_TYPES as i64).await;
    let mut types: Vec<String> = get_findings(&router, &detector_id, &ids)
        .await
        .iter()
        .filter_map(|finding| finding["type"].as_str().map(str::to_string))
        .collect();
    types.sort();
    assert!(
        types.len() > MAX_FINDING_TYPES,
        "reading the catalogue: expected more supported finding types than one request may \
         name, got {types:?}"
    );
    types
}

/// One `CreateSampleFindings` request and the finding types it generates.
struct Row {
    case: &'static str,
    /// The body of a create this service accepts.
    create: Value,
    /// The finding types that create leaves in the detector's store, sorted.
    expected: Vec<String>,
}

/// The rows, spanning the counts and forms a request can carry. `catalog` is the
/// service's own catalogue, as [`published_finding_types`] read it.
fn rows(catalog: &[String]) -> Vec<Row> {
    let whole_catalog = catalog.to_vec();
    // A type from each end and the middle of the catalogue, so the rows do not all sit
    // in one corner of it.
    let first = catalog[0].clone();
    let middle = catalog[catalog.len() / 2].clone();
    let last = catalog[catalog.len() - 1].clone();
    let mut two = vec![first.clone(), last.clone()];
    two.sort();
    let mut fifty: Vec<String> = catalog.iter().take(MAX_FINDING_TYPES).cloned().collect();
    fifty.sort();

    vec![
        Row {
            case: "one finding type",
            create: json!({"findingTypes": [middle.clone()]}),
            expected: vec![middle],
        },
        Row {
            case: "two finding types from opposite ends of the catalogue",
            create: json!({"findingTypes": [last, first]}),
            expected: two,
        },
        Row {
            case: "the 50 finding types the modelled range tops out at",
            create: json!({"findingTypes": fifty.clone()}),
            expected: fifty,
        },
        Row {
            case: "an empty findingTypes list, which names no type",
            create: json!({"findingTypes": []}),
            expected: whole_catalog.clone(),
        },
        Row {
            case: "no findingTypes member at all",
            create: json!({}),
            expected: whole_catalog,
        },
    ]
}

/// The types of the findings the detector holds, sorted, read back through
/// `ListFindings` and `GetFindings`.
async fn stored_finding_types(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let ids = all_finding_ids(router, detector_id, MAX_FINDING_TYPES as i64).await;
    let mut types: Vec<String> = get_findings(router, detector_id, &ids)
        .await
        .iter()
        .map(|finding| {
            finding["type"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a finding type, got {finding:?}"))
                .to_string()
        })
        .collect();
    types.sort();
    types
}

/// Each stored finding's own `updatedAt`, keyed by finding id.
async fn finding_updated_ats(
    router: &Router,
    detector_id: &str,
    case: &str,
) -> Vec<(String, String)> {
    let ids = all_finding_ids(router, detector_id, MAX_FINDING_TYPES as i64).await;
    let mut clocks: Vec<(String, String)> = get_findings(router, detector_id, &ids)
        .await
        .iter()
        .map(|finding| {
            let id = finding["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a finding id, got {finding:?}"));
            let updated = finding["updatedAt"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a finding updatedAt, got {finding:?}")
            });
            (id.to_string(), updated.to_string())
        })
        .collect();
    clocks.sort();
    clocks
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

// ---------------------------------------------------------------------------
// The rule: a generation that adds findings moves the detector's UpdatedAt; a refused
// generation, a generation that only aggregates, and a read move it nowhere.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generating_sample_findings_moves_the_detectors_updated_at() {
    let catalog = published_finding_types().await;
    assert!(
        !catalog.contains(&UNCATALOGUED_FINDING_TYPE.to_string()),
        "the refused row must name a type outside the catalogue, got \
         {UNCATALOGUED_FINDING_TYPE:?} in {catalog:?}"
    );
    // One past the top of the modelled range, so the refusal is the model's.
    let too_many: Vec<String> = catalog
        .iter()
        .take(MAX_FINDING_TYPES + 1)
        .cloned()
        .collect();

    for row in rows(&catalog) {
        let case = row.case;
        for finding_type in &row.expected {
            assert!(
                catalog.contains(finding_type),
                "case {case:?}: the row's types must come from the catalogue, got \
                 {finding_type:?} outside {catalog:?}"
            );
        }
        // A router and a detector per row, so a row cannot pass on another row's write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A request naming a type the service cannot generate persists nothing, so the
        // detector is unchanged by it.
        let (status, refusal) = create_sample_findings(
            &router,
            &detector_id,
            json!({"findingTypes": [UNCATALOGUED_FINDING_TYPE]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a finding type outside the catalogue must be refused, got {status} \
             {refusal:?}"
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused generation must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        // So does a request one entry past the member's published maximum.
        let (status, over) =
            create_sample_findings(&router, &detector_id, json!({"findingTypes": too_many})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {} finding types is past the modelled maximum of \
             {MAX_FINDING_TYPES} and must be refused, got {status} {over:?}",
            MAX_FINDING_TYPES + 1
        );
        let after_over = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_over, updated_before,
            "case {case:?}: a request past the modelled maximum must not move the detector's \
             updatedAt, expected {updated_before:?}, got {after_over:?}"
        );

        // Reading the findings is not a change either.
        assert_eq!(
            stored_finding_types(&router, &detector_id, case).await,
            Vec::<String>::new(),
            "case {case:?}: the refused generations must have stored no finding"
        );
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, updated_before,
            "case {case:?}: reading the detector's findings must not move its updatedAt, expected \
             {updated_before:?}, got {after_read:?}"
        );

        // The accepted generation: the detector now holds findings it did not hold, so
        // its UpdatedAt moves.
        let (status, created) =
            create_sample_findings(&router, &detector_id, row.create.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the generation to answer 200, got {status} {created:?}"
        );

        // Round trip: the findings the generation stored read back under their types, so
        // the moved UpdatedAt records a change that really landed in the store.
        let stored = stored_finding_types(&router, &detector_id, case).await;
        assert_eq!(
            stored, row.expected,
            "case {case:?}: expected the generated findings to read back as {:?}, got {stored:?}",
            row.expected
        );

        let after_create = read_detector(&router, &detector_id, case).await;
        let updated_after_create = after_create["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {after_create:?}"))
            .to_string();
        assert!(
            updated_after_create > updated_before,
            "case {case:?}: the generation changed which findings the detector holds, so its \
             updatedAt must move past {updated_before:?}, got {updated_after_create:?}"
        );
        assert_eq!(
            after_create["createdAt"], created_at,
            "case {case:?}: generating findings must not move the detector's createdAt, expected \
             {created_at:?}, got {:?}",
            after_create["createdAt"]
        );

        // The same request again aggregates onto the findings already stored: the
        // detector holds the SAME findings, so its own UpdatedAt stays where the first
        // generation put it, while each finding's own clock moves.
        let clocks_before_replay = finding_updated_ats(&router, &detector_id, case).await;
        let (status, again) = create_sample_findings(&router, &detector_id, row.create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated generation to answer 200, got {status} \
             {again:?}"
        );
        let stored_again = stored_finding_types(&router, &detector_id, case).await;
        assert_eq!(
            stored_again, row.expected,
            "case {case:?}: a repeated generation must aggregate rather than add, expected {:?}, \
             got {stored_again:?}",
            row.expected
        );
        let clocks_after_replay = finding_updated_ats(&router, &detector_id, case).await;
        for ((id, before_clock), (same_id, after_clock)) in
            clocks_before_replay.iter().zip(clocks_after_replay.iter())
        {
            assert_eq!(
                id, same_id,
                "case {case:?}: a repeated generation must keep each finding's id, expected \
                 {id:?}, got {same_id:?}"
            );
            assert!(
                after_clock > before_clock,
                "case {case:?}: another observation of finding {id:?} must move the finding's own \
                 updatedAt past {before_clock:?}, got {after_clock:?}"
            );
        }
        let after_replay = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_replay, updated_after_create,
            "case {case:?}: an aggregated generation leaves the detector holding the same \
             findings, so its updatedAt must stay at {updated_after_create:?}, got \
             {after_replay:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every change to the set moves the instant again, so the detector's UpdatedAt names
// the LAST such change and not the first.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_generation_that_adds_a_finding_moves_the_instant_again() {
    let catalog = published_finding_types().await;
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut previous = detector_updated_at(&router, &detector_id, "the fresh detector").await;
    // A handful of types from across the catalogue, generated one request at a time.
    let picked: Vec<String> = catalog
        .iter()
        .step_by(catalog.len() / 4)
        .take(4)
        .cloned()
        .collect();
    for finding_type in &picked {
        let case = finding_type.as_str();
        let (status, created) = create_sample_findings(
            &router,
            &detector_id,
            json!({"findingTypes": [finding_type]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the generation to answer 200, got {status} {created:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each generation that adds a finding must move the detector's \
             updatedAt past {previous:?}, got {now:?}"
        );
        previous = now;
    }

    let stored = stored_finding_types(&router, &detector_id, "one detector").await;
    let mut expected = picked.clone();
    expected.sort();
    assert_eq!(
        stored, expected,
        "one detector: expected the generations to have stored {expected:?}, got {stored:?}"
    );

    // Generating every type the picks already cover adds nothing, so nothing moves.
    let (status, again) = create_sample_findings(
        &router,
        &detector_id,
        json!({"findingTypes": picked.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "one detector: expected the repeated generation to answer 200, got {status} {again:?}"
    );
    let after_aggregated = detector_updated_at(&router, &detector_id, "after aggregation").await;
    assert_eq!(
        after_aggregated, previous,
        "one detector: a generation that only aggregates leaves the detector holding the same \
         findings, so its updatedAt must stay at {previous:?}, got {after_aggregated:?}"
    );

    // A generation naming a type the detector does not hold yet adds a record, so the
    // instant moves again.
    let fresh = catalog
        .iter()
        .find(|finding_type| !picked.contains(finding_type))
        .expect("the catalogue holds a type the picks did not cover");
    let (status, created) =
        create_sample_findings(&router, &detector_id, json!({"findingTypes": [fresh]})).await;
    assert_eq!(
        status, 200,
        "one detector: expected the generation of {fresh:?} to answer 200, got {status} \
         {created:?}"
    );
    let after_fresh = detector_updated_at(&router, &detector_id, "after a fresh type").await;
    assert!(
        after_fresh > previous,
        "one detector: generating {fresh:?} added a finding, so the detector's updatedAt must \
         move past {previous:?}, got {after_fresh:?}"
    );
}
