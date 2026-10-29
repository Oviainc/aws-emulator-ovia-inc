// SPDX-License-Identifier: CC0-1.0

//! `ListVpcEndpoints` end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rules under test:
//!
//! 1. the listing reports the managed interface endpoints the account has stored, each as
//!    the identity and the CURRENT status of the stored record — the same status the detail
//!    readers answer — in one stable order, cutting at the page bound and handing back a
//!    `nextToken` only while records remain;
//! 2. `vpcEndpointFilters.status` narrows that listing to the endpoints in the status it
//!    names, and an absent filter narrows nothing.
//!
//! The tables span what those rules generalise over: the endpoint's shape (the subnet count
//! and both modelled `SubnetId` widths, the presence and count of `securityGroupIds` up to
//! its published maximum, the `VpcEndpointName` length at both published bounds), the
//! endpoint count, the page bound from 1 to past the count and up to `maxResults`' own
//! published maximum, and every `VpcEndpointStatus` value the slice can put a record in.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{
    await_active, create_endpoint, is_vpc_endpoint_id, read_one, ACTIVE, PENDING,
};

/// `VpcEndpointStatus$DELETING` — "Deleting VPCEndpoint resource"
/// (`smithy_query(query="shape", shape="VpcEndpointStatus")` -> `values`).
const DELETING: &str = "DELETING";

/// `VpcEndpointStatus$FAILED` — "VPCEndpoint resource create or delete failed"
/// (`smithy_query(query="shape", shape="VpcEndpointStatus")` -> `values`). Nothing in this
/// slice fails an endpoint, so no stored record is ever in it.
const FAILED: &str = "FAILED";

/// The members `VpcEndpointSummary` does NOT model: they belong to `VpcEndpointDetail`
/// (`smithy_query(query="shape", shape="VpcEndpointSummary,VpcEndpointDetail")`).
const DETAIL_ONLY_MEMBERS: &[&str] = &[
    "vpcId",
    "subnetIds",
    "securityGroupIds",
    "createdDate",
    "failureCode",
    "failureMessage",
];

/// One page of `ListVpcEndpoints`: its summaries and the token that resumes after them.
async fn list_page(
    router: &smithy_server_runtime::MultiServiceRouter,
    status: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = json!({});
    if let Some(status) = status {
        request["vpcEndpointFilters"] = json!({ "status": status });
    }
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    let case = request.to_string();
    let (http_status, code, answer) = call(router, "ListVpcEndpoints", request).await;
    assert_eq!(
        http_status, 200,
        "case ListVpcEndpoints({case}): expected 200, got {http_status} {code} with {answer}"
    );
    let summaries = at(&answer, "vpcEndpointSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = at(&answer, "nextToken").as_str().map(str::to_string);
    (summaries, token)
}

/// Every summary `ListVpcEndpoints` reports under `status`, across pages.
async fn list_all(
    router: &smithy_server_runtime::MultiServiceRouter,
    status: Option<&str>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(router, status, Some(100), token.as_deref()).await;
        summaries.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return summaries,
        }
    }
}

/// The `id` of every summary in `summaries`, in the order the listing reported them.
fn ids(summaries: &[Value]) -> Vec<String> {
    summaries.iter().map(|summary| text(summary, "id")).collect()
}

/// The create requests the tables below drive: the subnet count and both modelled
/// `SubnetId` widths, the security-group count from none to the published maximum of five,
/// and the `VpcEndpointName` length at both published bounds.
///
/// `SubnetId` is `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` and `SecurityGroupId` is `^[\w+\-]+$`
/// (`smithy_query(query="shape", shape="SubnetIds,SecurityGroupIds")`); `SubnetIds` is
/// `@length(min: 1)` and `SecurityGroupIds` `@length(min: 1, max: 5)`; `VpcEndpointName` is
/// `@length(min: 3, max: 32)` with `^[a-z][a-z0-9-]+$`.
fn endpoint_requests() -> Vec<(String, Value)> {
    let longest_name = format!("a{}", "b".repeat(31));
    vec![
        (
            "one short-width subnet, no security groups".to_string(),
            json!({
                "name": "listing-first",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "three long-width subnets, one security group".to_string(),
            json!({
                "name": "listing-second",
                "vpcId": "vpc-0123456789abcdef0",
                "subnetIds": [
                    "subnet-0123456789abcdef0",
                    "subnet-abcdef01234567890",
                    "subnet-00000000000000001",
                ],
                "securityGroupIds": ["sg-0123456789abcdef0"],
            }),
        ),
        (
            "two subnets, the maximum of five security groups".to_string(),
            json!({
                "name": "listing-third",
                "vpcId": "vpc-9f8e7d6c",
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg+5"],
            }),
        ),
        (
            "a name at the minimum length of three".to_string(),
            json!({
                "name": "abc",
                "vpcId": "vpc-",
                "subnetIds": ["subnet-deadbeef"],
                "securityGroupIds": ["sg-deadbeef"],
            }),
        ),
        (
            "a name at the maximum length of 32".to_string(),
            json!({
                "name": longest_name,
                "vpcId": "vpc-00000000000000000000000",
                "subnetIds": ["subnet-cafebabe", "subnet-cafebabecafebabe0"],
            }),
        ),
    ]
}

/// Create every row of [`endpoint_requests`], answering the minted id of each by name.
async fn create_every_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> BTreeMap<String, String> {
    let mut by_id: BTreeMap<String, String> = BTreeMap::new();
    for (case, request) in endpoint_requests() {
        let detail = create_endpoint(router, &case, request.clone()).await;
        let id = text(&detail, "id");
        assert!(
            is_vpc_endpoint_id(&id),
            "case {case}: expected an id matching ^vpce-[0-9a-z]*$, got {id:?}"
        );
        let name = text(&request, "name");
        assert!(
            by_id.insert(id.clone(), name).is_none(),
            "case {case}: expected a fresh id, got {id:?} a second time"
        );
    }
    by_id
}

// ---------------------------------------------------------------------------
// Rule 1: the listing is the stored endpoints, in one order, paged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_stored_endpoints_in_id_order() {
    let router = emulator();

    // Nothing stored yet: an empty listing, never a refusal and never a `nextToken`.
    let (summaries, token) = list_page(&router, None, None, None).await;
    assert!(
        summaries.is_empty() && token.is_none(),
        "case ListVpcEndpoints(empty store): expected no summaries and no nextToken, got {summaries:?} and {token:?}"
    );

    let created = create_every_endpoint(&router).await;
    // Every endpoint settles into `ACTIVE` on its own, so the listing below reports a
    // terminal status rather than racing the transition.
    for id in created.keys() {
        await_active(&router, "the created endpoints", id).await;
    }

    let summaries = list_all(&router, None).await;
    let expected: Vec<String> = created.keys().cloned().collect();
    assert_eq!(
        ids(&summaries),
        expected,
        "case ListVpcEndpoints(no filter): expected every stored endpoint in id order, got {summaries:?}"
    );

    for summary in &summaries {
        let id = text(summary, "id");
        let case = format!("the summary of {id}");
        // The summary is the STORED record's: the id, the name the create request named,
        // and the status the detail reader answers for the same endpoint.
        assert_eq!(
            text(summary, "name"),
            created[&id],
            "case {case}: expected the name the create request named, got {summary}"
        );
        let detail = read_one(&router, &case, &id).await;
        assert_eq!(
            text(summary, "status"),
            text(&detail, "status"),
            "case {case}: expected the status the detail reader reports, got {summary} beside {detail}"
        );
        assert_eq!(
            text(summary, "status"),
            ACTIVE,
            "case {case}: expected the settled status, got {summary}"
        );
        // `VpcEndpointSummary` models only `id`, `name` and `status`.
        for member in DETAIL_ONLY_MEMBERS {
            assert!(
                summary.get(member).is_none(),
                "case {case}: a summary carries no {member}, got {summary}"
            );
        }
    }

    // The page bound: 1, 2, one under the count, exactly the count, one past it, and
    // `maxResults`' own published maximum of 100. Every bound walks the same ordered
    // sequence, and a `nextToken` comes back only while a record remains after the page.
    let count = expected.len();
    for bound in [1_i64, 2, 4, 5, 6, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) = list_page(&router, None, Some(bound), token.as_deref()).await;
            let bound_size = usize::try_from(bound).expect("the bound is positive");
            let remaining = count - collected.len();
            let wanted = remaining.min(bound_size);
            assert_eq!(
                page.len(),
                wanted,
                "case ListVpcEndpoints(maxResults={bound}, after {collected:?}): expected {wanted} summaries, got {page:?}"
            );
            collected.extend(ids(&page));
            assert_eq!(
                next.is_some(),
                collected.len() < count,
                "case ListVpcEndpoints(maxResults={bound}, after {collected:?}): a nextToken comes back exactly while records remain, got {next:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case ListVpcEndpoints(maxResults={bound}): paging walks the whole listing in order, expected {expected:?}, got {collected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: `vpcEndpointFilters.status` narrows the listing to one status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_status_filter_narrows_the_listing_to_the_endpoints_in_that_status() {
    let router = emulator();

    let created = create_every_endpoint(&router).await;
    let expected: Vec<String> = created.keys().cloned().collect();

    // Straight after the creates every endpoint is in the transient `PENDING` status, so
    // the filter that names it selects all of them and the one that names the settled
    // status selects none.
    assert!(
        list_all(&router, Some(ACTIVE)).await.is_empty(),
        "case ListVpcEndpoints(status={ACTIVE}, straight after the creates): expected no endpoint to have settled yet"
    );
    assert_eq!(
        ids(&list_all(&router, Some(PENDING)).await),
        expected,
        "case ListVpcEndpoints(status={PENDING}): expected every just-created endpoint"
    );

    // The endpoints reach `ACTIVE` on their own, and the filter follows the stored status.
    for id in &expected {
        await_active(&router, "the created endpoints", id).await;
    }
    assert_eq!(
        ids(&list_all(&router, Some(ACTIVE)).await),
        expected,
        "case ListVpcEndpoints(status={ACTIVE}): expected every settled endpoint"
    );
    assert!(
        list_all(&router, Some(PENDING)).await.is_empty(),
        "case ListVpcEndpoints(status={PENDING}, after the transition): expected no endpoint to still be pending"
    );
    // Nothing in this slice fails an endpoint, so no stored record is in `FAILED`.
    assert!(
        list_all(&router, Some(FAILED)).await.is_empty(),
        "case ListVpcEndpoints(status={FAILED}): expected no endpoint in a status nothing puts a record in"
    );

    // The filter composes with the page bound: the same narrowed sequence, one summary at
    // a time.
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(&router, Some(ACTIVE), Some(1), token.as_deref()).await;
        collected.extend(ids(&page));
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected, expected,
        "case ListVpcEndpoints(status={ACTIVE}, maxResults=1): paging walks the narrowed listing in order, got {collected:?}"
    );

    // An accepted deletion moves ONE endpoint into `DELETING`, and that is what each filter
    // then reports: the deleting endpoint under `DELETING`, the rest under `ACTIVE`, and all
    // of them under no filter at all.
    let doomed = expected.first().cloned().expect("endpoints were created");
    let (status, code, answer) = call(&router, "DeleteVpcEndpoint", json!({ "id": doomed })).await;
    assert_eq!(
        status, 200,
        "case DeleteVpcEndpoint({doomed}): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        ids(&list_all(&router, Some(DELETING)).await),
        vec![doomed.clone()],
        "case ListVpcEndpoints(status={DELETING}): expected only the endpoint whose deletion was accepted"
    );
    let survivors: Vec<String> = expected
        .iter()
        .filter(|id| **id != doomed)
        .cloned()
        .collect();
    assert_eq!(
        ids(&list_all(&router, Some(ACTIVE)).await),
        survivors,
        "case ListVpcEndpoints(status={ACTIVE}, after the delete): expected the endpoints the delete left alone"
    );
    assert_eq!(
        ids(&list_all(&router, None).await),
        expected,
        "case ListVpcEndpoints(no filter, after the delete): expected every stored endpoint whatever its status"
    );

    // The deletion completes on its own, and the listing reports it by absence.
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for _ in 0..10 {
        listed = ids(&list_all(&router, None).await).into_iter().collect();
        if !listed.contains(&doomed) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
    }
    assert_eq!(
        listed.into_iter().collect::<Vec<String>>(),
        survivors,
        "case ListVpcEndpoints(no filter, after the deletion completed): expected the deleted endpoint to be gone"
    );
}

// ---------------------------------------------------------------------------
// The refusals the listing owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_refuses_what_it_must() {
    let router = emulator();

    // `maxResults` is `@range(min: 1, max: 100)` and `vpcEndpointFilters.status` is drawn
    // from `VpcEndpointStatus` (ACTIVE | DELETING | FAILED | PENDING); a `nextToken` this
    // service did not issue is not a pagination token.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "a status outside VpcEndpointStatus",
            json!({ "vpcEndpointFilters": { "status": "CREATING" } }),
        ),
        (
            "a status in the wrong case",
            json!({ "vpcEndpointFilters": { "status": "active" } }),
        ),
        (
            "an empty status",
            json!({ "vpcEndpointFilters": { "status": "" } }),
        ),
        ("maxResults one below the 1 minimum", json!({ "maxResults": 0 })),
        (
            "maxResults one past the 100 maximum",
            json!({ "maxResults": 101 }),
        ),
        (
            "a nextToken this service did not issue",
            json!({ "nextToken": "not-a-token" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListVpcEndpoints", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case ListVpcEndpoints({case}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Each accepted bound and each modelled status value is served, not refused — and an
    // empty filter object narrows nothing.
    for bound in [1_i64, 100] {
        let (summaries, token) = list_page(&router, None, Some(bound), None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListVpcEndpoints(maxResults={bound}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
    for status in [ACTIVE, DELETING, FAILED, PENDING] {
        let (summaries, token) = list_page(&router, Some(status), None, None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListVpcEndpoints(status={status}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
    let (http_status, code, answer) = call(
        &router,
        "ListVpcEndpoints",
        json!({ "vpcEndpointFilters": {} }),
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case ListVpcEndpoints(an empty filter object): expected 200, got {http_status} {code} with {answer}"
    );
}
