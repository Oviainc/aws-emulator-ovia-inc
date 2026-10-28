// SPDX-License-Identifier: CC0-1.0

//! A CMAF package's child manifest ids are a SET, on every write of an OriginEndpoint.
//!
//! `HlsManifestCreateOrUpdateParameters$Id` is documented "the ID of the manifest. The ID
//! must be unique within the OriginEndpoint and it cannot be changed after it is created"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//! so a request whose `cmafPackage.hlsManifests` names one id twice describes no storable
//! configuration: it is refused with `UnprocessableEntityException` (HTTP 422) keyed by the
//! list member, and nothing is stored — the create persists no endpoint and the update
//! leaves the stored one exactly as it was. The rule holds for BOTH writes, because both
//! judge the member with the one shared helper.
//!
//! The list carries no modelled `@uniqueItems` and its entries target the bare `__string`
//! `Id` with no `@pattern`, so the rule is the service's own and is asserted here over a
//! SET of inputs rather than on the one list an issue reported. The rows differ along what
//! the rule generalises over: WHERE the repeat sits (adjacent, separated, at the tail), how
//! MANY entries share the id, how long the id is, whether the repeated entries differ in
//! their other members, how many entries the list carries (up to the count bound), whether
//! two ids differ only in case, and which operation carries the list (create or update).
//! A generated suite then drives the same invariant — accepted exactly when the ids are
//! distinct — over lists drawn from a small id pool, so duplicates arise at every position.
//!
//! The requirement each refusal states is read from the service's own
//! [`state::unique_child_id_requirement`] rather than restated here, so the suite and the
//! handler cannot word the rule differently. Every case drives real HTTP through the
//! `MultiServiceRouter`, with the harness `tests/all/channel_lifecycle.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code, SeededGenerator};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "unique-ids-channel";

/// The child manifest list `ids` describes, one entry per id and nothing else set.
fn manifests(ids: &[&str]) -> Vec<Value> {
    ids.iter().map(|id| json!({ "id": id })).collect()
}

/// A `cmafPackage` publishing `children`.
fn cmaf(children: Vec<Value>) -> Value {
    json!({ "cmafPackage": { "hlsManifests": children } })
}

/// A `CreateOriginEndpoint` body: the configuration `body` describes, plus the two ids a
/// create needs.
fn create_body(body: &Value, id: &str) -> Value {
    let mut created = body.clone();
    let object = created
        .as_object_mut()
        .expect("every row's body is a JSON object");
    object.insert("id".to_string(), json!(id));
    object.insert("channelId".to_string(), json!(CHANNEL_ID));
    created
}

/// Assert `refusal` is this service's 422, keyed by the CMAF manifest list member and
/// stating the service's own repeated-id requirement.
fn assert_refused(case: &str, status: u16, refusal: &Value, requirement: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{}", state::CMAF_HLS_MANIFESTS_MEMBER)),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {:?} and state {requirement:?}, \
         got {refusal:?}",
        state::CMAF_HLS_MANIFESTS_MEMBER
    );
}

/// Assert the service holds no origin endpoint under `id`.
async fn assert_not_stored(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused create must store no endpoint, got {status} {described:?}"
    );
}

/// Create the endpoint `id` with a well-formed configuration, and answer the record the
/// service stored for it — the baseline a refused update must leave untouched.
async fn stored_endpoint(router: &MultiServiceRouter, id: &str) -> Value {
    let body = create_body(&cmaf(manifests(&["baseline"])), id);
    let (status, created) = call(router, "POST", "/origin_endpoints", Some(&body)).await;
    assert_eq!(
        status, 200,
        "creating the baseline endpoint {id:?} expected 200, got {status} {created:?}"
    );
    created
}

/// The child manifests the stored record `endpoint` reports.
fn stored_children(endpoint: &Value) -> Vec<Value> {
    endpoint
        .pointer("/cmafPackage/hlsManifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The values the children of `endpoint` carry for `member`, in stored order.
fn child_values(endpoint: &Value, member: &str) -> Vec<String> {
    stored_children(endpoint)
        .iter()
        .map(|child| {
            child
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The first id `ids` repeats and how many entries carry it — the rule the service enforces,
/// restated here so each generated case knows which answer to expect.
fn first_repeat(ids: &[String]) -> Option<(String, usize)> {
    let mut seen: Vec<&String> = Vec::new();
    for id in ids {
        if seen.contains(&id) {
            return Some((id.clone(), ids.iter().filter(|other| *other == id).count()));
        }
        seen.push(id);
    }
    None
}

/// One refused row: what it exercises, the children it publishes, and the id the refusal
/// has to name together with how many entries carry it.
struct RefusedRow {
    case: &'static str,
    children: Vec<Value>,
    repeated: String,
    count: usize,
}

/// Every way a request can name one child manifest id more than once.
fn refused_rows() -> Vec<RefusedRow> {
    // An id far longer than any an issue cited, well inside `MAX_STRING_CHARS`, so the rule
    // is shown not to depend on how short the id is.
    let long_id = "z".repeat(300);
    // A list at the service's own count bound whose LAST entry repeats its first, so the
    // rule is shown to hold for the longest list the size bound admits.
    let at_the_count_bound: Vec<Value> = (0..state::MAX_CMAF_HLS_MANIFESTS)
        .map(|index| {
            let id = if index + 1 == state::MAX_CMAF_HLS_MANIFESTS {
                "m-0".to_string()
            } else {
                format!("m-{index}")
            };
            json!({ "id": id })
        })
        .collect();

    vec![
        RefusedRow {
            case: "the same id twice, adjacent",
            children: manifests(&["a", "a"]),
            repeated: "a".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "the same id twice, separated by a distinct id",
            children: manifests(&["a", "b", "a"]),
            repeated: "a".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "three entries under one id",
            children: manifests(&["dup", "dup", "dup"]),
            repeated: "dup".to_string(),
            count: 3,
        },
        RefusedRow {
            case: "the repeat at the tail of a longer list",
            children: manifests(&["m-0", "m-1", "m-2", "m-1"]),
            repeated: "m-1".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "two repeated ids: the first one is reported",
            children: manifests(&["x", "y", "x", "y"]),
            repeated: "x".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "an id repeated across a case-differing neighbour",
            children: manifests(&["a", "A", "a"]),
            repeated: "a".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "the repeated entries carry different manifestNames",
            children: vec![
                json!({ "id": "a", "manifestName": "one" }),
                json!({ "id": "a", "manifestName": "two" }),
            ],
            repeated: "a".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "the repeated entries carry different per-manifest settings",
            children: vec![
                json!({ "id": "a", "playlistType": "EVENT" }),
                json!({ "id": "a", "includeIframeOnlyStream": true }),
            ],
            repeated: "a".to_string(),
            count: 2,
        },
        RefusedRow {
            case: "a long id repeated",
            children: manifests(&[long_id.as_str(), long_id.as_str()]),
            repeated: long_id.clone(),
            count: 2,
        },
        RefusedRow {
            case: "a list at the count bound whose last entry repeats its first",
            children: at_the_count_bound,
            repeated: "m-0".to_string(),
            count: 2,
        },
    ]
}

#[tokio::test]
async fn a_cmaf_package_repeating_a_child_manifest_id_is_refused_by_both_writes() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;
    let baseline_id = "unique-ids-baseline";
    let stored = stored_endpoint(&router, baseline_id).await;

    for (index, row) in refused_rows().into_iter().enumerate() {
        let requirement = state::unique_child_id_requirement(&row.repeated, row.count);
        let configuration = cmaf(row.children);

        let id = format!("dup-refused-{index}");
        let create_case = format!("create {}", row.case);
        let (status, refusal) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&configuration, &id)),
        )
        .await;
        assert_refused(&create_case, status, &refusal, &requirement);
        assert_not_stored(&router, &create_case, &id).await;

        let update_case = format!("update {}", row.case);
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{baseline_id}"),
            Some(&configuration),
        )
        .await;
        assert_refused(&update_case, status, &refusal, &requirement);
        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{baseline_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {update_case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, stored,
            "case {update_case:?}: a refused update must leave the stored record alone, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn distinct_child_manifest_ids_are_stored_and_addressable_apart() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // An id list differing only in case is DISTINCT — the ids are compared as the caller
    // sent them — and a list at the count bound is the largest the size rule admits.
    let long_id = "y".repeat(300);
    let at_the_count_bound: Vec<String> = (0..state::MAX_CMAF_HLS_MANIFESTS)
        .map(|index| format!("m-{index}"))
        .collect();
    let accepted: Vec<(&str, Vec<&str>)> = vec![
        ("one child", vec!["only"]),
        ("two distinct ids", vec!["a", "b"]),
        ("ids differing only in case", vec!["a", "A", "aA", "Aa"]),
        ("a long id beside a short one", vec![long_id.as_str(), "a"]),
        (
            "a list at the count bound",
            at_the_count_bound.iter().map(String::as_str).collect(),
        ),
    ];

    for (index, (case, ids)) in accepted.into_iter().enumerate() {
        let id = format!("dup-accepted-{index}");
        let configuration = cmaf(manifests(&ids));
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&configuration, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        assert_eq!(
            child_values(&created, "id"),
            ids,
            "case {case:?}: every child manifest must be stored under the id it was sent \
             with, got {created:?}"
        );

        // What the uniqueness rule protects: distinct ids leave every child with a URL of
        // its own, so a client keying children by their id can address them apart.
        let urls = child_values(&created, "url");
        let mut distinct = urls.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            urls.len(),
            "case {case:?}: distinct child ids must derive distinct playback URLs, got {urls:?}"
        );

        // Round trip: the later read renders the record the create stored.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );
    }
}

/// The ids generated lists are drawn from: a pool small enough that a repeat lands at every
/// position, and carrying two ids that differ only in case so a case-differing pair is
/// never mistaken for a repeat.
const ID_POOL: &[&str] = &["a", "A", "b", "manifest-1"];

#[tokio::test]
async fn acceptance_follows_whether_the_generated_child_ids_are_distinct() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // A bounded number of generated lists per seed, each judged against the rule — stored
    // exactly when its ids are distinct — rather than against the service's own answer. A
    // failing case reproduces from the seed named in its assertion message, and nothing is
    // written outside the in-memory engine.
    for seed in [11u64, 97, 2_026] {
        let mut rng = SeededGenerator::new(seed);

        for case_index in 0..24u32 {
            let length = rng.next_in(5);
            let ids: Vec<String> = (0..length)
                .map(|_| ID_POOL[rng.next_in(ID_POOL.len() as u64) - 1].to_string())
                .collect();
            let repeat = first_repeat(&ids);
            let case = format!(
                "seed {seed}, case {case_index}: ids {ids:?}, repeated={:?}",
                repeat.as_ref().map(|(id, count)| (id.as_str(), *count))
            );
            let id = format!("gen-{seed}-{case_index}");
            let configuration = cmaf(manifests(
                &ids.iter().map(String::as_str).collect::<Vec<&str>>(),
            ));
            let (status, answer) = call(
                &router,
                "POST",
                "/origin_endpoints",
                Some(&create_body(&configuration, &id)),
            )
            .await;

            match repeat {
                Some((repeated, count)) => {
                    assert_refused(
                        &case,
                        status,
                        &answer,
                        &state::unique_child_id_requirement(&repeated, count),
                    );
                    assert_not_stored(&router, &case, &id).await;
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a list of distinct ids must be stored, got {status} \
                         {answer:?}"
                    );
                    assert_eq!(
                        child_values(&answer, "id"),
                        ids,
                        "case {case:?}: every child must be stored under its own id, \
                         got {answer:?}"
                    );
                }
            }
        }
    }
}
