// SPDX-License-Identifier: CC0-1.0

//! How a refusal of a LIST member words the value that failed it, end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: in the `<n> validation error detected: Value '<value>' at '<member>'
//! failed to satisfy constraint: <constraint>` sentence, a LIST member's value half is how
//! many items the member carried — the rendering the validation layer uses for a list at the
//! boundary — and never the items themselves. A refusal a handler makes therefore reads
//! exactly as the identical one made at the boundary, and the refused response stays the
//! same size however long a list the request sent, which matters because `SubnetIds`
//! publishes no maximum at all.
//!
//! The tables span what the rule generalises over: which operation refuses (both VPC
//! endpoint writes), which of the five list members, which bound (the maximum and the
//! minimum `SubnetIds`/`SecurityGroupIds` publish, the emulator's own subnet bound, and the
//! bounds on the endpoint an update would LEAVE), and the count on the far side of it — one
//! past the bound up to three orders of magnitude past it.

use serde_json::{json, Value};

use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_SUBNET_IDS;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{await_active, create_endpoint};

/// `SecurityGroupIds`' own published maximum, from
/// `smithy_query(query="shape", shape="SecurityGroupIds")` -> `@length(min: 1, max: 5)`.
const MAX_SECURITY_GROUP_IDS: usize = 5;

/// The published minimum of BOTH list shapes, from
/// `smithy_query(query="shape", shape="SubnetIds")` and
/// `smithy_query(query="shape", shape="SecurityGroupIds")` -> `@length(min: 1)`.
const MIN_LIST_ENTRIES: usize = 1;

/// A VPC whose id satisfies `VpcId`'s `^vpc-[0-9a-z]*$`.
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// `count` distinct subnet ids of the short modelled `SubnetId` width, starting at `from`:
/// `smithy_query(query="shape", shape="SubnetId")` ->
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$`.
fn subnets(from: usize, count: usize) -> Vec<String> {
    (from..from + count)
        .map(|index| format!("subnet-{index:08x}"))
        .collect()
}

/// `count` distinct security group ids inside the modelled `SecurityGroupId` character
/// class, starting at `from`: `smithy_query(query="shape", shape="SecurityGroupId")` ->
/// `@length(min: 1, max: 128)`, `^[\w+\-]+$`.
fn groups(from: usize, count: usize) -> Vec<String> {
    (from..from + count)
        .map(|index| format!("sg-{index:08x}"))
        .collect()
}

/// The whole sentence the validation layer states for one violation.
fn one_violation(value: &str, member: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {constraint}"
    )
}

/// The constraint clause for a list over its maximum length, as the boundary validator
/// words it (pinned for a scalar in `client_error_status.rs` and for a list in
/// `reported_pattern.rs`).
fn over_maximum(limit: usize) -> String {
    format!("Member must have length less than or equal to {limit}")
}

/// The constraint clause for a list under its minimum length.
fn under_minimum(limit: usize) -> String {
    format!("Member must have length greater than or equal to {limit}")
}

/// Refuse `request` and answer the `ValidationException` message it was refused with.
async fn refusal_message(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ValidationException"),
        "case {case:?}: expected {CLIENT_ERROR} ValidationException from {operation}, got {status} {code} with {answer}"
    );
    text(&answer, "message")
}

/// Create an `ACTIVE` endpoint named `name` carrying `subnets` and `groups`, answering its
/// id.
async fn active_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    subnet_ids: &[String],
    group_ids: &[String],
) -> String {
    let mut request = json!({ "name": name, "vpcId": VPC_ID, "subnetIds": subnet_ids });
    if !group_ids.is_empty() {
        request["securityGroupIds"] = json!(group_ids);
    }
    let detail = create_endpoint(router, case, request).await;
    let id = text(&detail, "id");
    // An endpoint may only be CHANGED once it has settled, so every row starts from the
    // status the clock leaves it in rather than from the create's own.
    await_active(router, case, &id).await;
    id
}

/// How many subnets `BatchGetVpcEndpoint` reports for `id`.
async fn stored_subnet_count(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> usize {
    let detail = crate::vpc_endpoint::read_one(router, case, id).await;
    at(&detail, "subnetIds")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The longest a one-violation sentence about a list member can run: the fixed text, the
/// longest member name and constraint this service words, and a count. Anything longer
/// means entries of the caller's list reached the response.
const LONGEST_SENTENCE: usize = 200;

// ---------------------------------------------------------------------------
// The length bounds on the list a single request names: the maximum and the minimum each
// list shape publishes, and the emulator's own bound on a subnet list the model leaves
// unbounded.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_length_refusal_names_the_item_count_and_none_of_the_entries() {
    let router = emulator();

    // An endpoint that exists, so no row is refused for the id it addresses instead of for
    // the list it carries.
    let endpoint = active_endpoint(
        &router,
        "the endpoint",
        "list-clause-endpoint",
        &subnets(0, MIN_LIST_ENTRIES),
        &[],
    )
    .await;

    // Three orders of magnitude past the bound: the clause must not grow with it.
    let far_past = MAX_SUBNET_IDS * 1000;

    // (case, operation, the list member, its entries, the constraint it failed)
    let rows: Vec<(String, &str, &str, Vec<String>, String)> = vec![
        (
            format!("a create one subnet past the bound of {MAX_SUBNET_IDS}"),
            "CreateVpcEndpoint",
            "subnetIds",
            subnets(0, MAX_SUBNET_IDS + 1),
            over_maximum(MAX_SUBNET_IDS),
        ),
        (
            "a create ten times past the subnet bound".to_string(),
            "CreateVpcEndpoint",
            "subnetIds",
            subnets(0, MAX_SUBNET_IDS * 10),
            over_maximum(MAX_SUBNET_IDS),
        ),
        (
            format!("a create {far_past} subnets past the bound"),
            "CreateVpcEndpoint",
            "subnetIds",
            subnets(0, far_past),
            over_maximum(MAX_SUBNET_IDS),
        ),
        (
            format!("a create with an empty list, under the minimum of {MIN_LIST_ENTRIES}"),
            "CreateVpcEndpoint",
            "subnetIds",
            Vec::new(),
            under_minimum(MIN_LIST_ENTRIES),
        ),
        (
            format!("a create one group past the published maximum of {MAX_SECURITY_GROUP_IDS}"),
            "CreateVpcEndpoint",
            "securityGroupIds",
            groups(0, MAX_SECURITY_GROUP_IDS + 1),
            over_maximum(MAX_SECURITY_GROUP_IDS),
        ),
        (
            "a create far past the published group maximum".to_string(),
            "CreateVpcEndpoint",
            "securityGroupIds",
            groups(0, 64),
            over_maximum(MAX_SECURITY_GROUP_IDS),
        ),
        (
            format!("an add list one subnet past the bound of {MAX_SUBNET_IDS}"),
            "UpdateVpcEndpoint",
            "addSubnetIds",
            subnets(0, MAX_SUBNET_IDS + 1),
            over_maximum(MAX_SUBNET_IDS),
        ),
        (
            format!("a remove list {far_past} subnets past the bound"),
            "UpdateVpcEndpoint",
            "removeSubnetIds",
            subnets(0, far_past),
            over_maximum(MAX_SUBNET_IDS),
        ),
        (
            "an empty add list, under the published minimum".to_string(),
            "UpdateVpcEndpoint",
            "addSubnetIds",
            Vec::new(),
            under_minimum(MIN_LIST_ENTRIES),
        ),
        (
            format!("an add list one group past the maximum of {MAX_SECURITY_GROUP_IDS}"),
            "UpdateVpcEndpoint",
            "addSecurityGroupIds",
            groups(0, MAX_SECURITY_GROUP_IDS + 1),
            over_maximum(MAX_SECURITY_GROUP_IDS),
        ),
        (
            "a remove list far past the published group maximum".to_string(),
            "UpdateVpcEndpoint",
            "removeSecurityGroupIds",
            groups(0, 128),
            over_maximum(MAX_SECURITY_GROUP_IDS),
        ),
    ];

    for (case, operation, member, entries, constraint) in rows {
        let mut request = if operation == "CreateVpcEndpoint" {
            json!({ "name": "list-clause-create", "vpcId": VPC_ID })
        } else {
            json!({ "id": endpoint })
        };
        // A create is refused for the list under test, so every OTHER required member of it
        // has to be servable; the member under test overwrites the servable default below.
        if operation == "CreateVpcEndpoint" {
            request["subnetIds"] = json!(subnets(0, MIN_LIST_ENTRIES));
        }
        request[member] = json!(entries);

        let stated = refusal_message(&router, &case, operation, request).await;
        assert_eq!(
            stated,
            one_violation(&format!("{} items", entries.len()), member, &constraint),
            "case {case:?}: expected the clause to name the item count, got {stated:?}"
        );
        for entry in &entries {
            assert!(
                !stated.contains(entry.as_str()),
                "case {case:?}: the clause must not echo the entry {entry:?}, got {stated:?}"
            );
        }
        assert!(
            stated.len() <= LONGEST_SENTENCE,
            "case {case:?}: expected a refusal no longer than {LONGEST_SENTENCE} characters however long the list, got {} characters: {stated:?}",
            stated.len()
        );
    }
}

// ---------------------------------------------------------------------------
// The bounds on the endpoint an update would LEAVE — refusals this service words itself,
// which read the same way.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_result_bound_refusal_names_the_item_count_of_the_list_that_broke_it() {
    let router = emulator();

    // Each row starts from its own endpoint, sized so that a request whose own list is
    // WITHIN every bound still leaves an endpoint outside one.
    let held_subnets = subnets(0, MAX_SUBNET_IDS - 3);
    let over_result = active_endpoint(
        &router,
        "a result over the subnet bound",
        "result-over-subnets",
        &held_subnets,
        &[],
    )
    .await;
    let emptied = active_endpoint(
        &router,
        "a result under the subnet minimum",
        "result-under-subnets",
        &subnets(100, 2),
        &[],
    )
    .await;
    let over_groups = active_endpoint(
        &router,
        "a result over the group maximum",
        "result-over-groups",
        &subnets(200, MIN_LIST_ENTRIES),
        &groups(200, 3),
    )
    .await;

    // (case, id, member, entries, the constraint the RESULT fails)
    let rows: Vec<(String, &str, &str, Vec<String>, String)> = vec![
        (
            format!("four more subnets onto an endpoint holding {}", held_subnets.len()),
            over_result.as_str(),
            "addSubnetIds",
            subnets(10, 4),
            format!("The endpoint must not have more than {MAX_SUBNET_IDS} subnets"),
        ),
        (
            "both of the endpoint's subnets removed".to_string(),
            emptied.as_str(),
            "removeSubnetIds",
            subnets(100, 2),
            format!("The endpoint must keep at least {MIN_LIST_ENTRIES} subnet"),
        ),
        (
            "three more groups onto an endpoint holding three".to_string(),
            over_groups.as_str(),
            "addSecurityGroupIds",
            groups(300, 3),
            format!(
                "The endpoint must not have more than {MAX_SECURITY_GROUP_IDS} security groups"
            ),
        ),
    ];

    for (case, id, member, entries, constraint) in rows {
        let mut request = json!({ "id": id });
        request[member] = json!(entries);
        let stated = refusal_message(&router, &case, "UpdateVpcEndpoint", request).await;
        assert_eq!(
            stated,
            one_violation(&format!("{} items", entries.len()), member, &constraint),
            "case {case:?}: expected the clause to name the item count, got {stated:?}"
        );
        for entry in &entries {
            assert!(
                !stated.contains(entry.as_str()),
                "case {case:?}: the clause must not echo the entry {entry:?}, got {stated:?}"
            );
        }
    }

    // Every refusal above wrote nothing: each endpoint still carries the subnets it was
    // created with.
    assert_eq!(
        stored_subnet_count(&router, "after the refusals", &over_result).await,
        held_subnets.len(),
        "case after the refusals: expected the endpoint to still carry {} subnets",
        held_subnets.len()
    );
    assert_eq!(
        stored_subnet_count(&router, "after the refusals", &emptied).await,
        2,
        "case after the refusals: expected the endpoint to still carry 2 subnets"
    );
}
