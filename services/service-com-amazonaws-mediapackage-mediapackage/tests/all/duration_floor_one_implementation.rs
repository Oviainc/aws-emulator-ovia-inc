// SPDX-License-Identifier: CC0-1.0

//! The non-negative floor on an OriginEndpoint's duration-in-seconds members is ONE rule
//! with ONE implementation, whatever the shape of the member's path.
//!
//! RULE under test: a member whose value is a number of seconds of media is refused with
//! `UnprocessableEntityException` (HTTP 422) when it carries a value below
//! `crate::state::MIN_DURATION_SECONDS`, keyed by the path the wire names that member by
//! and stating the one requirement sentence — and that holds identically for a member at a
//! FIXED path (`hlsPackage.playlistWindowSeconds`) and for one nested in a LIST, whose path
//! carries the entry's index (`cmafPackage.hlsManifests[1].playlistWindowSeconds`), on both
//! write paths (`CreateOriginEndpoint`, `UpdateOriginEndpoint`).
//!
//! `tests/all/origin_endpoint_durations.rs` is the suite for the member CATALOG; this one
//! pins the rule's SHARED path: the rows pair a fixed-path member with an indexed one in
//! the same request, and every refusal's requirement sentence is asserted to be the SAME
//! sentence, so a second comparison or a second refusal constructor for the indexed group
//! would show up as a difference here.
//!
//! The rows differ along what the rule generalises over: whether the refused member's path
//! is fixed or indexed, WHICH index it carries (0 and a later one), how many entries the
//! list holds, which packaging format the request names, and which operation carries it.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses. The client, the region and the helpers are the ones
//! `tests/all/channel_lifecycle.rs` defines.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;

use crate::channel_lifecycle::{call, emulator, error_code};

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "floor-channel";

/// A value below the shared floor, and one at it: the floor is zero, so the step below it
/// is refused and the floor itself is stored.
const BELOW_FLOOR: i32 = state::MIN_DURATION_SECONDS - 1;

/// The one requirement sentence a refused duration states, built from the service's own
/// floor rather than restated here.
fn requirement() -> Value {
    json!(format!(
        "must be greater than or equal to {}",
        state::MIN_DURATION_SECONDS
    ))
}

/// One case: the member path the refusal must be keyed by, and the body that carries
/// `seconds` in exactly that member while every other duration in the request stays at the
/// floor.
struct Case {
    path: &'static str,
    body: fn(i32) -> Value,
}

/// A CMAF package carrying `count` child manifests, with `seconds` in the
/// `playlistWindowSeconds` of the entry at `at` (`None` for a list whose every entry stays
/// at the floor) and the floor in every other duration — so the member a refusal names is
/// never ambiguous, and the list is long enough for the index under test.
fn cmaf_with_window_at(count: usize, at: Option<usize>, seconds: i32) -> Value {
    let manifests: Vec<Value> = (0..count)
        .map(|index| {
            let window = if at == Some(index) {
                seconds
            } else {
                state::MIN_DURATION_SECONDS
            };
            json!({
                "id": format!("child-{index}"),
                "manifestName": format!("child-{index}"),
                "playlistWindowSeconds": window
            })
        })
        .collect();
    json!({ "cmafPackage": { "hlsManifests": manifests } })
}

/// The JSON pointer that reads a member back out of an endpoint projection, derived from
/// the member path (a list index becomes its own segment) so the two cannot drift apart.
fn pointer(path: &str) -> String {
    let flattened = path.replace('[', ".").replace(']', "");
    format!("/{}", flattened.replace('.', "/"))
}

/// The cases. Each pairs the two groups the floor used to be implemented separately for:
/// a fixed-path member and an index-carrying one.
const CASES: &[Case] = &[
    // Fixed path, top level of the request.
    Case {
        path: "timeDelaySeconds",
        body: |seconds| {
            json!({
                "timeDelaySeconds": seconds,
                "hlsPackage": { "playlistWindowSeconds": state::MIN_DURATION_SECONDS }
            })
        },
    },
    // Fixed path, inside a packaging configuration.
    Case {
        path: "hlsPackage.playlistWindowSeconds",
        body: |seconds| {
            json!({
                "timeDelaySeconds": state::MIN_DURATION_SECONDS,
                "hlsPackage": { "playlistWindowSeconds": seconds }
            })
        },
    },
    // Fixed path, inside the CMAF configuration that also carries the indexed group.
    Case {
        path: "cmafPackage.segmentDurationSeconds",
        body: |seconds| {
            let mut body = cmaf_with_window_at(2, None, state::MIN_DURATION_SECONDS);
            body["cmafPackage"]["segmentDurationSeconds"] = json!(seconds);
            body
        },
    },
    // Indexed path, the first entry of a one-entry list.
    Case {
        path: "cmafPackage.hlsManifests[0].playlistWindowSeconds",
        body: |seconds| cmaf_with_window_at(1, Some(0), seconds),
    },
    // Indexed path, the first entry of a longer list.
    Case {
        path: "cmafPackage.hlsManifests[0].playlistWindowSeconds",
        body: |seconds| cmaf_with_window_at(3, Some(0), seconds),
    },
    // Indexed path, an entry the issue never cited: the index is rendered from the
    // entry's position, not from a fixed literal.
    Case {
        path: "cmafPackage.hlsManifests[1].playlistWindowSeconds",
        body: |seconds| cmaf_with_window_at(2, Some(1), seconds),
    },
    Case {
        path: "cmafPackage.hlsManifests[2].playlistWindowSeconds",
        body: |seconds| cmaf_with_window_at(3, Some(2), seconds),
    },
];

/// A request body for `case` carrying `seconds`, plus the ids a create needs.
fn create_body(case: &Case, id: &str, seconds: i32) -> Value {
    let mut body = (case.body)(seconds);
    let object = body
        .as_object_mut()
        .expect("every row's body is a JSON object");
    object.insert("id".to_string(), json!(id));
    object.insert("channelId".to_string(), json!(CHANNEL));
    body
}

/// An id derived from the case, so each row writes its own endpoint.
fn endpoint_id(prefix: &str, index: usize, case: &Case) -> String {
    format!(
        "{prefix}-{index}-{}",
        case.path.replace(['.', '[', ']'], "-")
    )
}

#[tokio::test]
async fn a_duration_below_the_floor_is_refused_the_same_way_at_every_path_shape() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel {CHANNEL:?} expected 200, got {status} {body:?}"
    );

    // Every refusal's requirement sentence is collected and compared, so the fixed-path
    // and the indexed group cannot answer with two different sentences.
    let expected = requirement();

    for (index, case) in CASES.iter().enumerate() {
        // The write path the create takes.
        let created_id = endpoint_id("refused-create", index, case);
        let case_name = format!("create {} = {BELOW_FLOOR}", case.path);
        let (status, refusal) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(case, &created_id, BELOW_FLOOR)),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case_name:?}: expected 422, got {status} {refusal:?}"
        );
        assert_eq!(
            error_code(&refusal),
            "UnprocessableEntityException",
            "case {case_name:?}: expected UnprocessableEntityException, got {refusal:?}"
        );
        assert_eq!(
            refusal.pointer(&format!("/errors/{}", case.path)),
            Some(&expected),
            "case {case_name:?}: the refusal must be keyed by {:?} and state the one \
             requirement sentence, got {refusal:?}",
            case.path
        );
        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{created_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case_name:?}: the refused create must store no endpoint, \
             got {status} {described:?}"
        );

        // The write path the update takes, on an endpoint whose every duration is at the
        // floor: the same rule, reached from the other handler.
        let stored_id = endpoint_id("refused-update", index, case);
        let (status, stored) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(case, &stored_id, state::MIN_DURATION_SECONDS)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {stored_id:?}: a request at the floor must be stored, got {status} {stored:?}"
        );

        let case_name = format!("update {} = {BELOW_FLOOR}", case.path);
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{stored_id}"),
            Some(&(case.body)(BELOW_FLOOR)),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case_name:?}: expected 422, got {status} {refusal:?}"
        );
        assert_eq!(
            error_code(&refusal),
            "UnprocessableEntityException",
            "case {case_name:?}: expected UnprocessableEntityException, got {refusal:?}"
        );
        assert_eq!(
            refusal.pointer(&format!("/errors/{}", case.path)),
            Some(&expected),
            "case {case_name:?}: the refusal must be keyed by {:?} and state the one \
             requirement sentence, got {refusal:?}",
            case.path
        );
        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{stored_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, stored,
            "case {case_name:?}: a refused update must leave the stored record alone, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_duration_at_the_floor_is_stored_at_every_path_shape() {
    let router = emulator();
    let (status, body) = call(&router, "POST", "/channels", Some(&json!({ "id": CHANNEL }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel {CHANNEL:?} expected 200, got {status} {body:?}"
    );

    for (index, case) in CASES.iter().enumerate() {
        let case_name = format!("create {} = {}", case.path, state::MIN_DURATION_SECONDS);
        let id = endpoint_id("stored", index, case);
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(case, &id, state::MIN_DURATION_SECONDS)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected 200, got {status} {created:?}"
        );
        let read = pointer(case.path);
        assert_eq!(
            created.pointer(&read),
            Some(&json!(state::MIN_DURATION_SECONDS)),
            "case {case_name:?}: the create response must carry the floor at {read:?}, \
             got {created:?}"
        );

        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case_name:?}: the read must render the stored record, got {described:?}"
        );
    }
}
