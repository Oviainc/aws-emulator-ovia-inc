// SPDX-License-Identifier: CC0-1.0

//! `ListSchedulingPolicies`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a listing reports every scheduling policy this service holds, one entry per policy
//!   carrying the ARN the create minted and nothing else the detail does not model, in one
//!   stable order; an emulator holding none answers an empty list rather than a refusal;
//! * paging with `maxResults`/`nextToken` sees every policy exactly once in the unpaged
//!   order, `maxResults` is accepted on both published bounds and refused one past either,
//!   a request that names none is still bounded at the documented default, and a
//!   `nextToken` that resumes nothing is the modelled `ClientException` (HTTP 400);
//! * a listing is a READ: two listings at the same instant answer identically, and what it
//!   reports follows the writes — a create adds a row, an update leaves the row's ARN
//!   where it was, a delete removes it.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, error_code, error_message, Batch, REGION};

/// `ListSchedulingPolicies`' route (`smithy_query(query="operation",
/// operation="ListSchedulingPolicies")`: POST /v1/listschedulingpolicies).
const LIST: &str = "/v1/listschedulingpolicies";

/// `CreateSchedulingPolicy`' route.
const CREATE: &str = "/v1/createschedulingpolicy";

/// One scheduling policy this suite lists: the create body it is created with.
struct Policy {
    /// What the row varies: named in every assertion.
    label: &'static str,
    name: &'static str,
    /// The policy member the create carries (`fairsharePolicy` or `quotaSharePolicy`), or
    /// `None` for a create that carries neither.
    policy: Option<(&'static str, Value)>,
    /// `tags`, or `None` to leave the member out.
    tags: Option<Value>,
}

/// The table this suite lists: rows differ in the shape, the case and the length of the
/// name, in which policy kind the create carries (neither, fair-share, quota-share) and in
/// whether tags are carried — none of which the listing reports, so the listing must
/// answer the same single ARN member for every one of them.
fn policies() -> Vec<Policy> {
    vec![
        Policy {
            label: "no policy of either kind, no tags",
            name: "sp-bare",
            policy: None,
            tags: None,
        },
        Policy {
            label: "fair-share policy with a share distribution, mixed-case name",
            name: "SP-Fair-1",
            policy: Some((
                "fairsharePolicy",
                json!({
                    "shareDecaySeconds": 600,
                    "computeReservation": 25,
                    "shareDistribution": [{"shareIdentifier": "UserA", "weightFactor": 0.125}],
                }),
            )),
            tags: Some(json!({"team": "research"})),
        },
        Policy {
            label: "quota-share policy, underscored name",
            name: "sp_quota_share",
            // The only `QuotaShareIdleResourceAssignmentStrategy` the model declares
            // (`smithy_query(query="shape",
            // shape="QuotaShareIdleResourceAssignmentStrategy")`).
            policy: Some((
                "quotaSharePolicy",
                json!({"idleResourceAssignmentStrategy": "FIFO"}),
            )),
            tags: None,
        },
        Policy {
            label: "empty fair-share policy, single-character name",
            name: "z",
            policy: Some(("fairsharePolicy", json!({}))),
            tags: Some(json!({"owner": "platform"})),
        },
    ]
}

/// `ListSchedulingPolicies` over the wire: the status and the decoded body.
async fn list(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call(LIST, body).await
}

/// `ListSchedulingPolicies` over the wire, requiring the 200.
async fn listed(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = list(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListSchedulingPolicies to answer 200, got {status} {decoded}"
    );
    decoded
}

/// The `arn`s of a listing, in the order it answered them.
fn arns(body: &Value) -> Vec<String> {
    body["schedulingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| detail["arn"].as_str().unwrap_or("<no arn>").to_string())
        .collect()
}

/// The `nextToken` of a listing, if it carries one.
fn token_of(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create one scheduling policy over the wire; answers the ARN the create minted.
async fn create(batch: &Batch, policy: &Policy) -> String {
    let mut body = json!({"name": policy.name});
    if let Some((member, value)) = policy.policy.as_ref() {
        body[*member] = value.clone();
    }
    if let Some(tags) = policy.tags.as_ref() {
        body["tags"] = tags.clone();
    }
    let created = batch.ok(CREATE, body).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the create answered no arn", policy.label))
        .to_string()
}

/// Create every row of [`policies`].
async fn fixture(batch: &Batch) {
    for policy in policies() {
        create(batch, &policy).await;
    }
}

/// The ARNs of [`policies`], in the order a listing must answer them: the store is keyed
/// by the policy name, so the name is the listing's stable order.
fn stored_order() -> Vec<String> {
    let mut rows = policies();
    rows.sort_by(|left, right| left.name.cmp(right.name));
    rows.iter()
        .map(|policy| arn(&format!("scheduling-policy/{}", policy.name)))
        .collect()
}

/// RULE: a listing reports every scheduling policy this service holds, in one stable
/// order; an emulator holding none answers an empty list, and a listing changes nothing a
/// later listing sees.
#[tokio::test]
async fn a_listing_reports_every_stored_scheduling_policy() {
    let batch = Batch::new();

    // Nothing stored yet: the listing is EMPTY, never a refusal.
    let empty = listed(&batch, "an empty store", json!({})).await;
    assert_eq!(
        arns(&empty),
        Vec::<String>::new(),
        "case an empty store: expected no scheduling policy, got {empty}"
    );
    assert!(
        token_of(&empty).is_none(),
        "case an empty store: a page that ends the results carries no nextToken, got {empty}"
    );

    // Each create adds a row, and every earlier row stays listed.
    let mut created: Vec<String> = Vec::new();
    for policy in policies() {
        let minted = create(&batch, &policy).await;
        created.push(minted);
        created.sort();
        let case = format!("after creating {:?} ({})", policy.name, policy.label);
        let body = listed(&batch, &case, json!({})).await;
        assert_eq!(
            arns(&body),
            created,
            "case {case}: expected {created:?}, got {body}"
        );
        // A read: the same request at the same instant answers identically.
        let again = listed(&batch, &case, json!({})).await;
        assert_eq!(
            again, body,
            "case {case}: listing twice must answer identically, got {again} then {body}"
        );
    }
}

/// RULE: one entry per stored policy, carrying the ARN the create minted and no member
/// `SchedulingPolicyListingDetail` does not model.
#[tokio::test]
async fn a_listing_detail_carries_the_minted_arn_and_nothing_else() {
    let batch = Batch::new();

    for policy in policies() {
        let minted = create(&batch, &policy).await;
        // The ARN form `SchedulingPolicyDetail$arn` documents, for this endpoint's Region
        // and account.
        assert_eq!(
            minted,
            arn(&format!("scheduling-policy/{}", policy.name)),
            "case {:?}: expected the documented ARN form, got {minted}",
            policy.label
        );
    }

    let body = listed(&batch, "every detail", json!({})).await;
    let details = body["schedulingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        details.len(),
        policies().len(),
        "case every detail: expected every created policy, got {body}"
    );

    for detail in &details {
        let case = detail["arn"].as_str().unwrap_or("<no arn>").to_string();
        // The reader that owns the policy's details reports the SAME ARN, so the listing
        // reports the persisted record rather than a value of its own.
        let described = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [case.clone()]}),
            )
            .await;
        assert_eq!(
            described["schedulingPolicies"][0]["arn"].as_str(),
            Some(case.as_str()),
            "case {case}: the listing must report the ARN the reader reports, got {described}"
        );
        // `SchedulingPolicyListingDetail` models this one member
        // (`smithy_query(query="shape", shape="SchedulingPolicyListingDetail")`), so
        // nothing beyond it is invented.
        let object = detail
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a detail object, got {detail}"));
        for member in object.keys() {
            assert_eq!(
                member, "arn",
                "case {case}: {member} is not a member SchedulingPolicyListingDetail models, \
                 got {detail}"
            );
        }
    }
}

/// RULE: paging with `maxResults`/`nextToken` sees every policy exactly once in the
/// unpaged order, and `maxResults` is accepted on both published bounds.
#[tokio::test]
async fn paging_sees_every_scheduling_policy_exactly_once() {
    let batch = Batch::new();
    fixture(&batch).await;

    let expected = stored_order();
    let all = listed(&batch, "unpaged", json!({})).await;
    assert_eq!(
        arns(&all),
        expected,
        "case unpaged: expected every created policy, got {all}"
    );

    // Page sizes differ: one at a time, a size that does not divide the total, the exact
    // total, and both published bounds of `maxResults` (1 and 100).
    for page_size in [
        state::MIN_LIST_SCHEDULING_POLICIES_RESULTS,
        3,
        i32::try_from(expected.len()).expect("the fixture fits in an i32"),
        state::MAX_LIST_SCHEDULING_POLICIES_RESULTS,
    ] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(resume) = token.as_deref() {
                request["nextToken"] = json!(resume);
            }
            let body = listed(&batch, &case, request).await;
            let page = arns(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page holds at most maxResults policies, got {body}"
            );
            seen.extend(page);
            token = token_of(&body);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate after {pages} pages, last token {token:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must see every policy exactly once in the unpaged order, \
             got {seen:?}"
        );
    }
}

/// RULE: a request that names no `maxResults` is still bounded at the documented default
/// ("returns up to 100 results and a nextToken value if applicable"), and paging from that
/// first page sees the rest.
///
/// More policies are needed here than `CreateSchedulingPolicy` will store
/// (`state::limits::MAX_SCHEDULING_POLICIES` is below that default), so the records are
/// seeded into the very store the handler reads.
#[tokio::test]
async fn a_listing_that_names_no_page_size_is_bounded_at_the_documented_default() {
    let batch = Batch::new();
    let default = usize::try_from(state::MAX_LIST_SCHEDULING_POLICIES_RESULTS)
        .expect("the documented default is positive");
    // One past the default, so the first page cannot hold them all.
    let total = default + 1;
    let mut expected: Vec<String> = Vec::new();
    {
        let store = batch.store();
        let mut store = store.lock();
        for index in 0..total {
            // Zero-padded, so the name order is the index order.
            let name = format!("seeded-{index:04}");
            let policy_arn = state::scheduling_policy_arn(REGION, &name);
            store.scheduling_policies.insert(
                name.clone(),
                state::SchedulingPolicy {
                    name: name.clone(),
                    arn: policy_arn.clone(),
                    fairshare_policy: None,
                    quota_share_policy: None,
                    tags: None,
                },
            );
            store
                .scheduling_policies_by_arn
                .insert(policy_arn.clone(), name);
            expected.push(policy_arn);
        }
    }

    let first = listed(&batch, "no maxResults", json!({})).await;
    let page = arns(&first);
    assert_eq!(
        page.len(),
        default,
        "case no maxResults: expected the documented default page size {default}, got {}",
        page.len()
    );
    let token = token_of(&first);
    assert!(
        token.is_some(),
        "case no maxResults: a page that leaves results carries a nextToken, got {first}"
    );

    let rest = listed(
        &batch,
        "resuming the default page",
        json!({"nextToken": token.expect("the first page carries a token")}),
    )
    .await;
    let mut seen = page;
    seen.extend(arns(&rest));
    assert_eq!(
        seen, expected,
        "case resuming the default page: expected every seeded policy exactly once in the \
         stored order, got {} of {total}",
        seen.len()
    );
    assert!(
        token_of(&rest).is_none(),
        "case resuming the default page: the last page carries no nextToken, got {rest}"
    );
}

/// RULE: a request this operation cannot answer is the modelled `ClientException` (HTTP
/// 400): a `maxResults` outside the published 1..100, and a `nextToken` that resumes
/// nothing.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let batch = Batch::new();
    fixture(&batch).await;

    // Rows differ in WHICH part of the request is unusable; each must be the modelled
    // 400, never a 5xx or a 501.
    let cases: Vec<(&str, Value)> = vec![
        (
            "maxResults one below the published minimum",
            json!({"maxResults": state::MIN_LIST_SCHEDULING_POLICIES_RESULTS - 1}),
        ),
        (
            "maxResults one past the published maximum",
            json!({"maxResults": state::MAX_LIST_SCHEDULING_POLICIES_RESULTS + 1}),
        ),
        ("a negative maxResults", json!({"maxResults": -1})),
        (
            "a nextToken that resumes nothing",
            json!({"maxResults": 1, "nextToken": "not-a-token"}),
        ),
        (
            "a nextToken past the end of the results",
            json!({"nextToken": "9999"}),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list(&batch, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException with HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected ClientException, got {body} ({})",
            error_message(&body)
        );
    }

    // Both published bounds of `maxResults` are ACCEPTED, so the refusals above are the
    // bound and not the member.
    for accepted in [
        state::MIN_LIST_SCHEDULING_POLICIES_RESULTS,
        state::MAX_LIST_SCHEDULING_POLICIES_RESULTS,
    ] {
        let case = format!("maxResults {accepted}");
        listed(&batch, &case, json!({"maxResults": accepted})).await;
    }
}

/// RULE: a listing follows the writes — an update leaves the policy's ARN where it was and
/// a delete removes the row — and a listing itself changes nothing a later one sees.
#[tokio::test]
async fn a_listing_follows_the_writes_and_mutates_nothing() {
    let batch = Batch::new();
    fixture(&batch).await;

    // An update of a fair-share policy keeps its identity: the ARN is minted once, so the
    // listing is unchanged by it.
    let updated = arn("scheduling-policy/SP-Fair-1");
    batch
        .ok(
            "/v1/updateschedulingpolicy",
            json!({
                "arn": updated,
                "fairsharePolicy": {"shareDecaySeconds": 1200, "computeReservation": 50},
            }),
        )
        .await;
    let after_update = listed(&batch, "after the update", json!({})).await;
    assert_eq!(
        arns(&after_update),
        stored_order(),
        "case after the update: an update mints no new ARN, got {after_update}"
    );

    // A delete removes exactly that row, and a repeated listing still answers identically.
    for policy in policies() {
        let removed = arn(&format!("scheduling-policy/{}", policy.name));
        batch
            .ok(
                "/v1/deleteschedulingpolicy",
                json!({"arn": removed.clone()}),
            )
            .await;
        let case = format!("after deleting {:?}", policy.name);
        let body = listed(&batch, &case, json!({})).await;
        assert!(
            !arns(&body).contains(&removed),
            "case {case}: a deleted policy is no longer listed, got {body}"
        );
        let again = listed(&batch, &case, json!({})).await;
        assert_eq!(
            again, body,
            "case {case}: a listing must not change what a later listing sees, got {again} \
             then {body}"
        );
    }

    let drained = listed(&batch, "after deleting every policy", json!({})).await;
    assert_eq!(
        arns(&drained),
        Vec::<String>::new(),
        "case after deleting every policy: expected an empty listing, got {drained}"
    );
}
