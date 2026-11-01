// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE selection rule the lists of this service apply to a
//! filter modelled as an IDENTIFIER rather than as a list of values, driven over the
//! wire through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! The rule, stated once: such a filter names ONE resource and is resolved against
//! the endpoint the request reached, so a filter the caller did not send narrows
//! nothing, a record matches it only when its value for the filtered member is the
//! resource the filter names, and a value that resolves to a resource of ANOTHER
//! endpoint — which is what an ARN of another partition, Region or account states —
//! names nothing of this inventory and so matches no record, reporting an empty list
//! rather than failing. `ListOrders` (on the order's own `OutpostId`) and
//! `ListCapacityTasks` (on the task's own `OutpostId`) both select by that one rule
//! (`state::identifier_filter_accepts`), so this suite drives the same table of
//! filter forms through BOTH of them and requires the same selection of each:
//! whatever one list answers for a filter, the other answers for its own.
//!
//! The two filter forms no query string can carry — a filter whose member the record
//! holds no value for at all, and the absent filter's own arm against such a record —
//! are asserted against the shared rule itself in
//! [`the_shared_rule_covers_the_forms_no_query_string_can_carry`].

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    identifier_filter_accepts, ResolvedIdentifier,
};

use crate::capacity_task_lifecycle::{capacity_route, pool};
use crate::site_and_outpost_lifecycle::{
    query_value, text, Outposts, ABSENT_OUTPOST_ID, HOME_REGION, OUTPOST_ARN_PREFIX,
};

/// The two lists under test: the operation, the route the model binds it to, the
/// output member its page arrives in and the member identifying one record of that
/// page — from `smithy_query(query="operation",
/// operation="ListOrders,ListCapacityTasks")`.
const LISTS: [Listing; 2] = [
    Listing {
        operation: "ListOrders",
        route: "/list-orders",
        page_member: "Orders",
        id_member: "OrderId",
    },
    Listing {
        operation: "ListCapacityTasks",
        route: "/capacity/tasks",
        page_member: "CapacityTasks",
        id_member: "CapacityTaskId",
    },
];

/// One list the identifier filter narrows.
struct Listing {
    operation: &'static str,
    route: &'static str,
    page_member: &'static str,
    id_member: &'static str,
}

/// What one Outpost of the fixture holds: the order placed against it and the
/// capacity task started on it, both named by the list that reports them.
struct Held {
    outpost_id: String,
    outpost_arn: String,
    order_id: String,
    capacity_task_id: String,
}

impl Held {
    /// The record this Outpost contributes to `listing`'s page.
    fn record_of(&self, listing: &Listing) -> String {
        match listing.page_member {
            "Orders" => self.order_id.clone(),
            _ => self.capacity_task_id.clone(),
        }
    }
}

/// One Outposts resource ARN: `arn:<partition>:outposts:<region>:<account>:outpost/<id>`,
/// the form the modelled `OutpostIdentifier` pattern's ARN branch accepts.
fn outpost_arn(partition: &str, region: &str, account: &str, outpost_id: &str) -> String {
    format!("arn:{partition}:outposts:{region}:{account}:outpost/{outpost_id}")
}

/// The identifiers one page reports, in the order it reported them.
fn listed(output: &Value, listing: &Listing, case: &str) -> Vec<String> {
    output
        .get(listing.page_member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} answers a {} list, got {output}",
                listing.operation, listing.page_member
            )
        })
        .iter()
        .map(|item| text(item, listing.id_member))
        .collect()
}

/// An Outpost of [`HOME_REGION`] holding one order and one capacity task, created in
/// the order this function is called in — which is the creation order both lists
/// report their records in.
async fn held(outposts: &Outposts, name: &str) -> Held {
    let (outpost_id, outpost_arn) = outposts.site_and_outpost_in_service(name).await;
    let order = outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }],
            })),
        )
        .await;
    let task = outposts
        .ok(
            "POST",
            &capacity_route(&outpost_id),
            Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        )
        .await;
    Held {
        order_id: text(&order["Order"], "OrderId"),
        capacity_task_id: text(&task, "CapacityTaskId"),
        outpost_id,
        outpost_arn,
    }
}

/// The rule, driven through both lists over a table of filter forms.
///
/// The rows differ along everything the rule generalises over: whether the filter is
/// sent at all, which identifier form names the Outpost (bare id, this endpoint's own
/// ARN), how many records the named Outpost holds (one, none), an Outpost id this
/// inventory does not hold at all, and each segment of an ARN's identity that can
/// name another endpoint (account, Region, partition, and all three at once).
#[tokio::test]
async fn an_identifier_filter_narrows_both_lists_by_the_same_rule() {
    let outposts = Outposts::new();
    let first = held(&outposts, "identifier-filter-first").await;
    let second = held(&outposts, "identifier-filter-second").await;
    // A third Outpost of the same region, holding neither an order nor a task: the
    // "narrows to nothing without failing" row for an Outpost this endpoint DOES
    // hold.
    let (idle_id, idle_arn) = outposts.site_and_outpost("identifier-filter-idle").await;

    // The partition, Region and account this endpoint's own ARNs carry, each replaced
    // in turn: `OUTPOST_ARN_PREFIX` is what this emulator mints an Outpost ARN of
    // `HOME_REGION` from.
    let own = format!("{OUTPOST_ARN_PREFIX}{}", first.outpost_id);
    assert_eq!(
        first.outpost_arn, own,
        "the fixture's Outpost ARN is this endpoint's own, got {:?}",
        first.outpost_arn
    );
    let foreign = [
        (
            "an ARN of another account",
            outpost_arn("aws", HOME_REGION, "999999999999", &first.outpost_id),
        ),
        (
            "an ARN of another Region of this partition",
            outpost_arn("aws", "eu-west-1", "000000000000", &first.outpost_id),
        ),
        (
            "an ARN of another partition",
            outpost_arn("aws-cn", HOME_REGION, "000000000000", &first.outpost_id),
        ),
        (
            "an ARN of another partition, Region and account",
            outpost_arn("aws-cn", "cn-north-1", "999999999999", &first.outpost_id),
        ),
    ];

    // Each row: the case, the filter the request carries (absent for the first), and
    // the Outposts whose records the lists must report, in creation order.
    let mut rows: Vec<(String, Option<String>, Vec<&Held>)> = vec![
        ("no filter at all".to_string(), None, vec![&first, &second]),
        (
            "the bare id of an Outpost holding records".to_string(),
            Some(first.outpost_id.clone()),
            vec![&first],
        ),
        (
            "that same Outpost, by this endpoint's own ARN".to_string(),
            Some(first.outpost_arn.clone()),
            vec![&first],
        ),
        (
            "another Outpost holding records, by bare id".to_string(),
            Some(second.outpost_id.clone()),
            vec![&second],
        ),
        (
            "that same other Outpost, by its own ARN".to_string(),
            Some(second.outpost_arn.clone()),
            vec![&second],
        ),
        (
            "an Outpost of this endpoint holding no record, by bare id".to_string(),
            Some(idle_id.clone()),
            Vec::new(),
        ),
        (
            "that idle Outpost, by its own ARN".to_string(),
            Some(idle_arn.clone()),
            Vec::new(),
        ),
        (
            "an Outpost id this inventory does not hold".to_string(),
            Some(ABSENT_OUTPOST_ID.to_string()),
            Vec::new(),
        ),
    ];
    for (case, identifier) in foreign {
        rows.push((case.to_string(), Some(identifier), Vec::new()));
    }

    for (case, identifier, expected_held) in &rows {
        for listing in &LISTS {
            let query = match identifier {
                Some(identifier) => {
                    format!("?OutpostIdentifierFilter={}", query_value(identifier))
                }
                None => String::new(),
            };
            let reply = outposts
                .call("GET", &format!("{}{query}", listing.route), None)
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} ({} GET {}{query}): a filter is a filter and not an identifier \
                 the request is addressed to, so it is answered rather than refused, got {} {:?}",
                listing.operation, listing.route, reply.status, reply.body
            );
            let reported = listed(&reply.body, listing, case);
            let expected: Vec<String> = expected_held
                .iter()
                .map(|held| held.record_of(listing))
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?} ({} GET {}{query}): expected {expected:?}, got {reported:?}",
                listing.operation, listing.route
            );
        }
    }
}

/// The filter narrows a PAGE of either list too: the boundary a token names is a
/// position in the SAME narrowed list, so paging a filtered list reports that
/// Outpost's records once each and no other's.
#[tokio::test]
async fn an_identifier_filter_narrows_the_pages_of_both_lists() {
    let outposts = Outposts::new();
    let first = held(&outposts, "identifier-filter-page-first").await;
    let second = held(&outposts, "identifier-filter-page-second").await;

    for listing in &LISTS {
        let filtered = format!(
            "{}?MaxResults=1&OutpostIdentifierFilter={}",
            listing.route,
            query_value(&second.outpost_id)
        );
        let page = outposts.ok("GET", &filtered, None).await;
        let reported = listed(&page, listing, listing.operation);
        assert_eq!(
            reported,
            vec![second.record_of(listing)],
            "case {:?}: a page of the filtered list reports that Outpost's record, got {page}",
            listing.operation
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {:?}: a page that exhausts the NARROWED list issues no token, got {page}",
            listing.operation
        );
        assert!(
            !reported.contains(&first.record_of(listing)),
            "case {:?}: the other Outpost's record is narrowed out, got {reported:?}",
            listing.operation
        );
    }
}

/// The two arms of the rule no query string can state: a record that holds no value
/// at all for the filtered member matches a filter that names a resource (so the
/// filter narrows it out) and matches the absent filter (which narrows nothing) —
/// asserted against the shared rule itself, since every record this API creates
/// carries an Outpost id.
#[test]
fn the_shared_rule_covers_the_forms_no_query_string_can_carry() {
    let here = ResolvedIdentifier::Here("op-0123456789abcdef0".to_string());
    let cases: [(&str, Option<&ResolvedIdentifier>, Option<&str>, bool); 6] = [
        ("no filter, a record with a value", None, Some("op-0123456789abcdef0"), true),
        ("no filter, a record with no value", None, None, true),
        (
            "a filter naming this endpoint's resource, the same value",
            Some(&here),
            Some("op-0123456789abcdef0"),
            true,
        ),
        (
            "a filter naming this endpoint's resource, another value",
            Some(&here),
            Some("op-fedcba98765432100"),
            false,
        ),
        (
            "a filter naming this endpoint's resource, a record with no value",
            Some(&here),
            None,
            false,
        ),
        (
            "a filter resolved to another endpoint's resource",
            Some(&ResolvedIdentifier::Elsewhere),
            Some("op-0123456789abcdef0"),
            false,
        ),
    ];

    for (case, filter, value, expected) in cases {
        let accepted = identifier_filter_accepts(filter, value);
        assert_eq!(
            accepted, expected,
            "case {case:?}: expected {expected:?}, got {accepted:?}"
        );
    }
}
