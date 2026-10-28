// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResolverRuleAssociations`.
//!
//! The rule: the operation reports the associations the account made between
//! Resolver rules and VPCs, computed from the store, in the order they were
//! created, projected exactly as `GetResolverRuleAssociation` projects one,
//! narrowed to the records whose named member matches one of a filter's values,
//! and cut into pages a cursor resumes.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`], and the fixtures are the ones in
//! [`super::resolver_rule_deletion`].

use serde_json::{json, Value};
use super::harness::{emulator, expect_error, ok, string_member, without_status, Emulator};
use super::resolver_rule_deletion::{associate, forward_rule, outbound_endpoint};

/// The VPCs the table below associates rules with.
const FIRST_VPC: &str = "vpc-00000000000000061";
const SECOND_VPC: &str = "vpc-00000000000000062";

/// Associate two rules with two VPCs and return
/// `(rule ids, association ids in creation order)`.
///
/// The records span what a list query selects on: which rule the association is
/// of, which VPC it is with, and a `Name` present or absent.
async fn seed_associations(router: &Emulator) -> (Vec<String>, Vec<String>) {
    let endpoint = outbound_endpoint(router, "endpoint-association-listing").await;
    let first_rule = forward_rule(
        router,
        "rule-association-listing-one",
        "one.associations.example.com",
        &endpoint,
    )
    .await;
    let second_rule = forward_rule(
        router,
        "rule-association-listing-two",
        "two.associations.example.com",
        &endpoint,
    )
    .await;

    let mut associations = Vec::new();
    associations.push(associate(router, &first_rule, FIRST_VPC).await);
    associations.push(associate(router, &first_rule, SECOND_VPC).await);
    // One record created WITHOUT a `Name`, so the Name filter is exercised on a
    // member a record may not carry.
    let unnamed = ok(
        router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": second_rule, "VPCId": FIRST_VPC }),
    )
    .await;
    associations.push(string_member(
        unnamed
            .get("ResolverRuleAssociation")
            .unwrap_or(&Value::Null),
        "Id",
    ));

    (vec![first_rule, second_rule], associations)
}

/// The `Id`s one response carries.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("ResolverRuleAssociations")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// Every stored association is listed in creation order, projected as its own
/// reader projects it, and a filter restricts the page to the records whose
/// named member matches one of `Values`.
///
/// The filter names are exactly the catalog the model documents for this
/// operation (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRuleAssociations**): `Name`, `ResolverRuleId`, `Status`,
/// `VPCId` — each exercised here, in the documented spelling and in the early
/// upper-snake one the same doc says is "still supported".
#[tokio::test]
async fn associations_are_listed_from_state_and_filtered_on_every_documented_member() {
    let router = emulator();
    let (rules, associations) = seed_associations(&router).await;

    let listed = ok(&router, "ListResolverRuleAssociations", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        associations,
        "case \"no filters\": every stored association must be listed in creation order, got \
         {listed}"
    );
    // Each entry is the same projection the single-record reader answers. The
    // status is compared separately, because it is the member the clock is
    // allowed to move between the two calls.
    for (index, id) in associations.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": id.clone() }),
        )
        .await;
        let from_list = listed
            .get("ResolverRuleAssociations")
            .and_then(Value::as_array)
            .and_then(|items| items.get(index))
            .expect("the page carries this association");
        assert_eq!(
            without_status(from_list),
            without_status(
                fetched
                    .get("ResolverRuleAssociation")
                    .unwrap_or(&Value::Null)
            ),
            "case {id:?}: the listed association must be the stored one, got {from_list}"
        );
        let status = string_member(from_list, "Status");
        assert!(
            status == "CREATING" || status == "COMPLETE",
            "case {id:?}: a listed association reports a status of its own slice, got {status:?}"
        );
    }

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "ResolverRuleId",
            json!([{ "Name": "ResolverRuleId", "Values": [rules[0].clone()] }]),
            vec![associations[0].clone(), associations[1].clone()],
        ),
        (
            "RESOLVER_RULE_ID, the early spelling",
            json!([{ "Name": "RESOLVER_RULE_ID", "Values": [rules[1].clone()] }]),
            vec![associations[2].clone()],
        ),
        (
            "VPCId",
            json!([{ "Name": "VPCId", "Values": [SECOND_VPC] }]),
            vec![associations[1].clone()],
        ),
        (
            "VPC_ID, the early spelling, two VPCs",
            json!([{ "Name": "VPC_ID", "Values": [FIRST_VPC, SECOND_VPC] }]),
            associations.clone(),
        ),
        (
            "VPCId, a VPC no association names",
            json!([{ "Name": "VPCId", "Values": ["vpc-00000000000000069"] }]),
            vec![],
        ),
        (
            "Name",
            json!([{ "Name": "Name", "Values": [format!("{SECOND_VPC}-association")] }]),
            vec![associations[1].clone()],
        ),
        (
            "Name, which a record created without one does not match",
            json!([{
                "Name": "Name",
                "Values": [
                    format!("{FIRST_VPC}-association"),
                    format!("{SECOND_VPC}-association"),
                ],
            }]),
            vec![associations[0].clone(), associations[1].clone()],
        ),
        (
            "Status, one no association reports",
            json!([{ "Name": "Status", "Values": ["FAILED"] }]),
            vec![],
        ),
        (
            "two filters combine with AND, both matching",
            json!([
                { "Name": "ResolverRuleId", "Values": [rules[0].clone()] },
                { "Name": "VPCId", "Values": [FIRST_VPC] },
            ]),
            vec![associations[0].clone()],
        ),
        (
            "two filters combine with AND, disagreeing",
            json!([
                { "Name": "ResolverRuleId", "Values": [rules[1].clone()] },
                { "Name": "VPCId", "Values": [SECOND_VPC] },
            ]),
            vec![],
        ),
        (
            "an empty value list matches nothing",
            json!([{ "Name": "VPCId", "Values": [] }]),
            vec![],
        ),
        (
            "an empty filter list restricts nothing",
            json!([]),
            associations.clone(),
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = ok(
            &router,
            "ListResolverRuleAssociations",
            json!({ "Filters": filters }),
        )
        .await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case:?}: the filtered page is wrong, got {listed}"
        );
    }

    // A `Status` filter naming the status the records report selects all of
    // them, whichever side of the transition the clock owes they are on.
    let status = string_member(
        listed
            .get("ResolverRuleAssociations")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .unwrap_or(&Value::Null),
        "Status",
    );
    let by_status = ok(
        &router,
        "ListResolverRuleAssociations",
        json!({ "Filters": [{ "Name": "STATUS", "Values": [status.clone(), "COMPLETE"] }] }),
    )
    .await;
    assert_eq!(
        listed_ids(&by_status),
        associations,
        "case \"Status, the statuses the records report\": got {by_status}"
    );

    // A name outside THIS operation's catalog is refused, not ignored —
    // including the ones that are valid for the endpoint and rule lists.
    for case in [
        "Direction",
        "HostVPCId",
        "SecurityGroupIds",
        "DomainName",
        "Type",
        "CreatorRequestId",
        "Id",
    ] {
        expect_error(
            &router,
            "ListResolverRuleAssociations",
            json!({ "Filters": [{ "Name": case, "Values": ["anything"] }] }),
            case,
            "InvalidParameterException",
        )
        .await;
    }
}

/// The operation pages the collection the query selects: `MaxResults` bounds the
/// page and is echoed back, `NextToken` resumes where the previous page stopped,
/// the last page carries none, and a cursor only means anything for the query
/// that minted it.
#[tokio::test]
async fn paging_walks_every_association_exactly_once() {
    let router = emulator();
    let (rules, associations) = seed_associations(&router).await;

    for (case, page_size) in [
        ("one association per page", 1),
        ("two associations per page", 2),
        ("a page larger than the collection", 100),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let listed = ok(&router, "ListResolverRuleAssociations", request).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: a page must hold at most MaxResults associations, got {listed}"
            );
            assert_eq!(
                listed.get("MaxResults").and_then(Value::as_i64),
                Some(i64::from(page_size)),
                "case {case:?}: MaxResults must be echoed back, got {listed}"
            );
            walked.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, associations,
            "case {case:?}: paging must walk every association exactly once, in creation order"
        );
    }

    // A cursor is scoped to the query that minted it.
    let first = ok(
        &router,
        "ListResolverRuleAssociations",
        json!({
            "MaxResults": 1,
            "Filters": [{ "Name": "ResolverRuleId", "Values": [rules[0].clone()] }],
        }),
    )
    .await;
    let token = string_member(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "the filtered query must owe a cursor, got {first}"
    );
    for (case, request) in [
        (
            "a cursor minted under a filter, replayed with none",
            json!({ "MaxResults": 1, "NextToken": token.clone() }),
        ),
        (
            "a cursor minted under a filter, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "ResolverRuleId", "Values": [rules[1].clone()] }],
            }),
        ),
        (
            "a cursor this service never minted",
            json!({ "NextToken": "6e6f742d612d746f6b656e" }),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverRuleAssociations",
            request,
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    // The same cursor under the query that minted it resumes that query.
    let resumed = ok(
        &router,
        "ListResolverRuleAssociations",
        json!({
            "MaxResults": 1,
            "NextToken": token,
            "Filters": [{ "Name": "ResolverRuleId", "Values": [rules[0].clone()] }],
        }),
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        vec![associations[1].clone()],
        "the cursor must resume the query that minted it, got {resumed}"
    );
}

/// A list is a pure read: an association a disassociate took out is gone from
/// the next page, and listing never changes what a later caller sees.
#[tokio::test]
async fn listing_reports_the_store_and_changes_nothing() {
    let router = emulator();
    let (rules, associations) = seed_associations(&router).await;

    let before = listed_ids(&ok(&router, "ListResolverRuleAssociations", json!({})).await);
    let again = listed_ids(&ok(&router, "ListResolverRuleAssociations", json!({})).await);
    assert_eq!(
        before, again,
        "two lists of an unchanged store must agree, got {again:?}"
    );

    ok(
        &router,
        "DisassociateResolverRule",
        json!({ "ResolverRuleId": rules[0].clone(), "VPCId": FIRST_VPC }),
    )
    .await;
    let after = listed_ids(&ok(&router, "ListResolverRuleAssociations", json!({})).await);
    assert_eq!(
        after,
        vec![associations[1].clone(), associations[2].clone()],
        "a disassociated record must leave the page, got {after:?}"
    );
}
