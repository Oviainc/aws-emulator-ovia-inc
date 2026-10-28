// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ERROR SHAPE a handler-returned refusal of a paged answer
//! travels on, driven over the wire through the same front door as the rest of this
//! crate's suites (whose harness it reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * A refusal a handler of this service returns for a defect in the caller's input
//!   travels on the error shape THIS SERVICE models for that failure class, so the
//!   body carries that shape's own modelled members — `ValidationException$Message`,
//!   from `smithy_query(query="shape", shape="ValidationException")` — and never the
//!   framework's synthetic envelope, whose member restJson1 renders as lowercase
//!   `message`. An operation whose own `errors` list omits the shape widens it in
//!   rather than reaching for the synthetic one.
//!
//! The rows span EVERY paged answer this service serves, each probed with a token
//! stamped for each of the OTHER paged answers — a value the modelled `Token` pattern
//! `^(\d+)##(\S+)$` accepts, so it reaches the handler rather than being refused at
//! the boundary — and each required to answer the same status, the same code and the
//! same report under the same key. A token the answer DID issue a boundary for is
//! served, so a row that refused everything could not pass.
//!
//! What the boundary validator refuses before any handler runs (a `MaxResults`
//! outside the modelled range, a `NextToken` that fails the modelled pattern) is a
//! different channel with its own wording rule, stated in
//! [`crate::error_message_rules`].

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    ASSET_INSTANCE_PAGE, ASSET_PAGE, BLOCKING_INSTANCE_PAGE, CAPACITY_TASK_PAGE,
    CATALOG_ITEM_PAGE, CONFIGURED_INSTANCE_TYPE_PAGE, ORDERABLE_INSTANCE_TYPE_PAGE, ORDER_PAGE,
    OUTPOST_PAGE, QUOTE_PAGE, SITE_PAGE, SUPPORTED_INSTANCE_TYPE_PAGE,
};

use crate::capacity_task_lifecycle::{pool, start_ok};
use crate::site_and_outpost_lifecycle::{query_value, segment, text, Outposts};

/// The member the modelled `ValidationException` carries its report in.
///
/// Source: `smithy_query(query="shape", shape="ValidationException")` against this
/// service's model — `{"members": {"Message": {"target": "ErrorMessage"}}}`.
const MODELLED_MESSAGE: &str = "Message";

/// The member the framework's synthetic envelope would carry a report in on the JSON
/// protocols — the key a refusal on the modelled shape must NOT have.
const SYNTHETIC_MESSAGE: &str = "message";

/// Every page stamp this service issues, one per paged answer it serves.
///
/// Read from the service's own constants rather than copied, so a row probes a stamp
/// the emulator really issues for some other list.
const EVERY_PAGE_STAMP: &[&str] = &[
    OUTPOST_PAGE,
    SITE_PAGE,
    QUOTE_PAGE,
    ORDER_PAGE,
    ORDERABLE_INSTANCE_TYPE_PAGE,
    CATALOG_ITEM_PAGE,
    CAPACITY_TASK_PAGE,
    ASSET_PAGE,
    BLOCKING_INSTANCE_PAGE,
    ASSET_INSTANCE_PAGE,
    SUPPORTED_INSTANCE_TYPE_PAGE,
    CONFIGURED_INSTANCE_TYPE_PAGE,
];

/// The boundaries a probe stamps its foreign token with: the lowest the modelled
/// `Token` pattern's `(\d+)` accepts, a boundary a short list really has, and one no
/// store could hold — the refusal is about the STAMP, so none of them may change it.
const SEQUENCES: &[&str] = &["0", "1", "7", "18446744073709551615"];

/// One paged answer: the operation its refusal has to name, the route it is asked on
/// (every `@httpLabel` already bound), and the page stamp its OWN tokens carry.
struct Paged {
    operation: &'static str,
    path: String,
    stamp: &'static str,
}

/// The report this service states for a `NextToken` it did not issue for `operation`.
///
/// The grammar is the standard AWS validation report — the violation count, then one
/// `Value at '<member>' failed to satisfy constraint: <requirement>` clause — naming
/// the member the paged input binds the token in and the requirement the token failed.
fn expected_report(operation: &str) -> String {
    format!(
        "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
         Member must be a pagination token this service issued for {operation}"
    )
}

/// Every paged answer this service serves, with the resources the Outpost-scoped ones
/// are asked on already created.
async fn every_paged_answer(outposts: &Outposts) -> Vec<Paged> {
    let (outpost_id, _) = outposts.site_and_outpost("page-token-shape").await;
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

    vec![
        Paged {
            operation: "ListSites",
            path: "/sites".to_string(),
            stamp: SITE_PAGE,
        },
        Paged {
            operation: "ListOutposts",
            path: "/outposts".to_string(),
            stamp: OUTPOST_PAGE,
        },
        Paged {
            operation: "ListQuotes",
            path: "/quotes".to_string(),
            stamp: QUOTE_PAGE,
        },
        Paged {
            operation: "ListOrders",
            path: "/list-orders".to_string(),
            stamp: ORDER_PAGE,
        },
        Paged {
            operation: "ListCatalogItems",
            path: "/catalog/items".to_string(),
            stamp: CATALOG_ITEM_PAGE,
        },
        Paged {
            operation: "ListCapacityTasks",
            path: "/capacity/tasks".to_string(),
            stamp: CAPACITY_TASK_PAGE,
        },
        Paged {
            operation: "ListOrderableInstanceTypes",
            path: "/instanceTypes".to_string(),
            stamp: ORDERABLE_INSTANCE_TYPE_PAGE,
        },
        Paged {
            operation: "ListAssets",
            path: format!("/outposts/{outpost}/assets"),
            stamp: ASSET_PAGE,
        },
        Paged {
            operation: "ListAssetInstances",
            path: format!("/outposts/{outpost}/assetInstances"),
            stamp: ASSET_INSTANCE_PAGE,
        },
        Paged {
            operation: "GetOutpostInstanceTypes",
            path: format!("/outposts/{outpost}/instanceTypes"),
            stamp: CONFIGURED_INSTANCE_TYPE_PAGE,
        },
        Paged {
            operation: "GetOutpostSupportedInstanceTypes",
            path: format!("/outposts/{outpost}/supportedInstanceTypes"),
            stamp: SUPPORTED_INSTANCE_TYPE_PAGE,
        },
        Paged {
            operation: "ListBlockingInstancesForCapacityTask",
            path: format!("/outposts/{outpost}/capacity/{task}/blockingInstances"),
            stamp: BLOCKING_INSTANCE_PAGE,
        },
    ]
}

/// The rule: every paged answer refuses a token it did not issue on the SERVICE's own
/// `ValidationException` — 400, the code clients branch on, and the report under the
/// modelled `Message` member, with no synthetic `message` member anywhere.
#[tokio::test]
async fn a_foreign_page_token_is_refused_on_the_modelled_validation_exception() {
    let outposts = Outposts::new();
    let answers = every_paged_answer(&outposts).await;

    for answer in &answers {
        // A token THIS answer issues a boundary in is served, so a row that refused
        // every token could not pass this table.
        let own = format!(
            "{}?NextToken={}",
            answer.path,
            query_value(&format!("1##{}", answer.stamp))
        );
        let served = outposts.call("GET", &own, None).await;
        assert_eq!(
            served.status,
            200,
            "case {} (a token of its own list): expected HTTP 200, got {} {}",
            answer.operation,
            served.status,
            served.body
        );

        let expected = expected_report(answer.operation);
        let foreign = EVERY_PAGE_STAMP
            .iter()
            .filter(|stamp| **stamp != answer.stamp);
        for (index, stamp) in foreign.enumerate() {
            let token = format!("{}##{stamp}", SEQUENCES[index % SEQUENCES.len()]);
            let path = format!("{}?NextToken={}", answer.path, query_value(&token));
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
                "case {case}: the refusal carries its report under {MODELLED_MESSAGE:?}, \
                 got {}",
                reply.body
            );
            assert!(
                reply.body.get(SYNTHETIC_MESSAGE).is_none(),
                "case {case}: a refusal on the modelled shape carries no \
                 {SYNTHETIC_MESSAGE:?} member, got {}",
                reply.body
            );
        }
    }
}
