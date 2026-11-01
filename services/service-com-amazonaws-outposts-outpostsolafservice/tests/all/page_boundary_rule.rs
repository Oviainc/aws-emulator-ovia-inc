// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the resume point a paged answer of this service takes from the
//! caller's `NextToken`, which every paged answer now takes through the ONE shared rule
//! `state::page_boundary` instead of restating it in its own handler.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * A `NextToken` the caller omitted — or sent empty — names no resume point, so the
//!   answer starts at the beginning of its list; a non-empty one names the boundary it
//!   carries for THAT answer's own list, and everything after it is what the answer
//!   reports; and one this service never issued for that list is refused as a bad
//!   request (400 `ValidationException`) whose report names the operation it was asked
//!   of, never honoured as a boundary and never served as a silently truncated page.
//!   An answer NESTED under a parent resource is that parent's own list, so the stamp
//!   its tokens carry names the parents the route binds as well as the kind of list
//!   (`state::nested_page_kind`) — which parent's list a token belongs to is stated
//!   over a set of parents in [`crate::nested_page_tokens`].
//!
//! The rows span EVERY paged answer this service serves — the twelve that each used to
//! carry the rule themselves — and, per answer, the token forms the rule distinguishes:
//! absent, empty, a boundary of its own list, a boundary past everything that list
//! holds, a boundary no `u64` can hold, and a boundary stamped for another list.
//!
//! What ERROR SHAPE such a refusal travels on is a different rule, stated in
//! [`crate::page_token_refusal_shape`]; the paging arithmetic itself is stated in each
//! answer's own suite.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    encode_page_token, nested_page_kind, page_boundary, ASSET_INSTANCE_PAGE, ASSET_PAGE,
    BLOCKING_INSTANCE_PAGE, CAPACITY_TASK_PAGE, CATALOG_ITEM_PAGE, CONFIGURED_INSTANCE_TYPE_PAGE,
    ORDERABLE_INSTANCE_TYPE_PAGE, ORDER_PAGE, OUTPOST_PAGE, QUOTE_PAGE, SITE_PAGE,
    SUPPORTED_INSTANCE_TYPE_PAGE,
};

use crate::capacity_task_lifecycle::{pool, start_ok};
use crate::site_and_outpost_lifecycle::{query_value, segment, text, Outposts};

/// The member the modelled `ValidationException` carries its report in.
///
/// Source: `smithy_query(query="shape", shape="ValidationException")` against this
/// service's model — `{"members": {"Message": {"target": "ErrorMessage"}}}`.
const MODELLED_MESSAGE: &str = "Message";

/// Every paged answer this service serves: the stamp its OWN page tokens carry, read
/// off the service's own constant rather than re-typed, and the operation name its
/// refusal of a token it did not issue has to state.
const EVERY_PAGED_ANSWER: &[(&str, &str)] = &[
    (SITE_PAGE, "ListSites"),
    (OUTPOST_PAGE, "ListOutposts"),
    (QUOTE_PAGE, "ListQuotes"),
    (ORDER_PAGE, "ListOrders"),
    (CATALOG_ITEM_PAGE, "ListCatalogItems"),
    (CAPACITY_TASK_PAGE, "ListCapacityTasks"),
    (ORDERABLE_INSTANCE_TYPE_PAGE, "ListOrderableInstanceTypes"),
    (ASSET_PAGE, "ListAssets"),
    (ASSET_INSTANCE_PAGE, "ListAssetInstances"),
    (CONFIGURED_INSTANCE_TYPE_PAGE, "GetOutpostInstanceTypes"),
    (
        SUPPORTED_INSTANCE_TYPE_PAGE,
        "GetOutpostSupportedInstanceTypes",
    ),
    (
        BLOCKING_INSTANCE_PAGE,
        "ListBlockingInstancesForCapacityTask",
    ),
];

/// A boundary past everything any list of this store can hold, so the answer that
/// takes it reports nothing and issues no further token. It is still a boundary the
/// rule can READ — the largest one a `u64` holds — which is what separates it from
/// [`PAST_U64`].
const PAST_THE_END: u64 = u64::MAX;

/// A boundary one past what a `u64` holds: all digits, so the modelled `Token` pattern
/// `^(\d+)##(\S+)$` accepts it and it reaches the handler, yet no boundary can be read
/// from it.
const PAST_U64: &str = "18446744073709551616";

/// The report this service states for a `NextToken` it did not issue for `operation`.
///
/// The standard AWS validation report grammar — the violation count, then one
/// `Value at '<member>' failed to satisfy constraint: <requirement>` clause — naming
/// the member the paged input binds the token in and the requirement the token failed.
fn expected_report(operation: &str) -> String {
    format!(
        "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
         Member must be a pagination token this service issued for {operation}"
    )
}

/// Rule, read directly off the shared helper every paged answer of this service now
/// reaches it through: which token forms name a boundary, and which are refused.
///
/// The table spans every paged answer (its own stamp, its own operation name) crossed
/// with every token form the rule distinguishes — absent, empty, its own boundaries
/// across the whole `u64` range, and the five ways a value can fail to name a boundary
/// of that list.
#[test]
fn the_shared_resume_rule_reads_every_token_form() {
    for (index, (stamp, operation)) in EVERY_PAGED_ANSWER.iter().enumerate() {
        // A list a caller named no token for, and one that named an empty token, both
        // start at the beginning.
        for absent in [None, Some("")] {
            assert_eq!(
                page_boundary(stamp, operation, absent),
                Ok(None),
                "case {operation} asked with {absent:?}: expected no resume point, got {:?}",
                page_boundary(stamp, operation, absent)
            );
        }

        // A token this answer issued names the boundary it carries, at both ends of
        // the range the modelled `Token` pattern's `(\d+)` accepts and in between.
        for boundary in [0, 1, 7, 4_096, PAST_THE_END] {
            let token = encode_page_token(stamp, boundary);
            let read = page_boundary(stamp, operation, Some(&token));
            assert_eq!(
                read,
                Ok(Some(boundary)),
                "case {operation} asked with {token:?}: expected boundary {boundary}, got {read:?}"
            );
        }

        // Nothing a boundary of THIS list can be read from: the refusal names this
        // operation, whatever the shape of the unusable value.
        let foreign = EVERY_PAGED_ANSWER[(index + 1) % EVERY_PAGED_ANSWER.len()].0;
        let unusable = [
            format!("1##{foreign}"),
            format!("abc##{stamp}"),
            format!("##{stamp}"),
            format!("{PAST_U64}##{stamp}"),
            "1".to_string(),
        ];
        for token in &unusable {
            let read = page_boundary(stamp, operation, Some(token));
            assert_eq!(
                read,
                Err(expected_report(operation)),
                "case {operation} asked with {token:?}: expected the refusal report, got {read:?}"
            );
        }
    }
}

/// One paged answer over the wire: the operation its refusal has to name, the route it
/// is asked on (every `@httpLabel` already bound), and the stamp its own tokens carry —
/// for an answer nested under a parent resource, the kind together with the parents the
/// route names ([`nested_page_kind`]).
struct Paged {
    operation: &'static str,
    path: String,
    stamp: String,
}

/// Every paged answer this service serves, with the resources the Outpost-scoped ones
/// are asked on already created, and each route's URI as the model binds it.
async fn every_route(outposts: &Outposts) -> Vec<Paged> {
    let (outpost_id, _) = outposts.site_and_outpost_in_service("page-boundary").await;
    let task_id = text(
        &start_ok(
            outposts,
            &outpost_id,
            &json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
        )
        .await,
        "CapacityTaskId",
    );
    let outpost = segment(&outpost_id);
    let task = segment(&task_id);
    let route = |operation: &'static str, path: String, parents: &[&str]| {
        let kind = EVERY_PAGED_ANSWER
            .iter()
            .find(|(_, named)| *named == operation)
            .unwrap_or_else(|| panic!("{operation} is a paged answer of this service"))
            .0;
        Paged {
            operation,
            path,
            stamp: nested_page_kind(kind, parents),
        }
    };

    vec![
        route("ListSites", "/sites".to_string(), &[]),
        route("ListOutposts", "/outposts".to_string(), &[]),
        route("ListQuotes", "/quotes".to_string(), &[]),
        route("ListOrders", "/list-orders".to_string(), &[]),
        route("ListCatalogItems", "/catalog/items".to_string(), &[]),
        route("ListCapacityTasks", "/capacity/tasks".to_string(), &[]),
        route(
            "ListOrderableInstanceTypes",
            "/instanceTypes".to_string(),
            &[],
        ),
        route(
            "ListAssets",
            format!("/outposts/{outpost}/assets"),
            &[&outpost_id],
        ),
        route(
            "ListAssetInstances",
            format!("/outposts/{outpost}/assetInstances"),
            &[&outpost_id],
        ),
        route(
            "GetOutpostInstanceTypes",
            format!("/outposts/{outpost}/instanceTypes"),
            &[&outpost_id],
        ),
        route(
            "GetOutpostSupportedInstanceTypes",
            format!("/outposts/{outpost}/supportedInstanceTypes"),
            &[&outpost_id],
        ),
        route(
            "ListBlockingInstancesForCapacityTask",
            format!("/outposts/{outpost}/capacity/{task}/blockingInstances"),
            &[&outpost_id, &task_id],
        ),
    ]
}

/// The same rule, driven end-to-end through EVERY paged answer: each takes its resume
/// point from the shared rule, so each serves a token of its own list and refuses one
/// it never issued with the same 400 report, naming itself.
#[tokio::test]
async fn every_paged_answer_takes_its_resume_point_from_the_shared_rule() {
    let outposts = Outposts::new();
    let answers = every_route(&outposts).await;

    for (index, answer) in answers.iter().enumerate() {
        // No token at all: the first page of the list, served.
        let first = outposts.call("GET", &answer.path, None).await;
        assert_eq!(
            first.status,
            200,
            "case {} asked with no token: expected HTTP 200, got {} {}",
            answer.operation,
            first.status,
            first.body
        );

        // A boundary of its OWN list past everything that list holds: an empty page,
        // and no further token — honoured as a boundary rather than refused.
        let past = format!(
            "{}?NextToken={}",
            answer.path,
            query_value(&encode_page_token(&answer.stamp, PAST_THE_END))
        );
        let reply = outposts.call("GET", &past, None).await;
        assert_eq!(
            (reply.status, reply.body.get("NextToken")),
            (200, None),
            "case {} asked past the end of its own list: expected HTTP 200 with no \
             NextToken, got {} {}",
            answer.operation,
            reply.status,
            reply.body
        );

        // Two values no boundary of this list can be read from — one stamped for
        // another paged answer, one whose digits no `u64` holds — are the same 400,
        // reported by this operation's name.
        let expected = expected_report(answer.operation);
        let foreign = &answers[(index + 1) % answers.len()].stamp;
        let unusable = [
            encode_page_token(foreign, 1),
            format!("{PAST_U64}##{}", answer.stamp),
        ];
        for token in &unusable {
            let path = format!("{}?NextToken={}", answer.path, query_value(token));
            let reply = outposts.call("GET", &path, None).await;
            let case = format!("{} asked with {token:?}", answer.operation);
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected 400 ValidationException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                reply.body.get(MODELLED_MESSAGE).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the refusal to name {}, got {}",
                answer.operation,
                reply.body
            );
        }
    }
}
