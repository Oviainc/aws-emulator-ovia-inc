// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the BODY of CloudWatch RUM's optimistic-locking refusal,
//! `InvalidPolicyRevisionIdException` (400), on `PutResourcePolicy` and
//! `DeleteResourcePolicy`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` the monitor is named by, the
//! `@httpQuery` the delete binds its revision id to, the handler and the error envelope
//! are all exercised, not just a handler body.
//!
//! The assertions pin one RULE over a SET of inputs rather than one request: each
//! operation answers its own FIXED sentence for this error class, carrying no part of the
//! request — not the revision id the request named, not the revision in force, not the
//! monitor's name — and the two operations' sentences differ, so a caller reconciling puts
//! and deletes can tell which of the two checks refused it. The table's rows differ along
//! what the request names as its `PolicyRevisionId` (a stale id, a foreign one, ids at both
//! modelled length bounds, a near-miss of the id in force, and — for the put, whose rule
//! also refuses a request that provides nothing once an id has been assigned — no id at
//! all) and along how many revisions the policy has taken before the refused call, so the
//! wording cannot be tied to one spelling, one length or one history.

use serde_json::json;

use crate::support::{call, create, emulator, message};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The sentence `PutResourcePolicy` answers `InvalidPolicyRevisionIdException` with.
/// Source: recorded RUM behaviour — wholly fixed text, with nothing from the request in
/// it. Spelled out here rather than read out of the crate, so these cases pin the wording
/// instead of agreeing with it by construction.
const PUT_MISMATCH_MESSAGE: &str =
    "If policyRevisionID is provided, then it must match the existing revision.";

/// The sentence `DeleteResourcePolicy` answers the same error class with: recorded RUM
/// behaviour again, deliberately worded differently from the put's and with no trailing
/// period.
const DELETE_MISMATCH_MESSAGE: &str = "PolicyRevisionID must match the existing revision";

/// The modelled error code, and the status the model binds to it.
/// Source: `smithy_query(query="operation", operation="PutResourcePolicy,DeleteResourcePolicy")`
/// — `InvalidPolicyRevisionIdException`, client error, `httpError` 400.
const MISMATCH_CODE: &str = "InvalidPolicyRevisionIdException";
const MISMATCH_STATUS: u16 = 400;

/// `PolicyRevisionId`'s modelled length bounds.
/// Source: `smithy_query(query="shape", shape="PolicyRevisionId")` — `@length(min: 1,
/// max: 255)`.
const POLICY_REVISION_ID_BOUNDS: (usize, usize) = (1, 255);

/// A well-formed IAM resource policy, parameterised so a row can write a document that
/// differs from the one in force and so a wrongly accepted write shows up in the reader.
fn policy_document(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
            "Action": action,
            "Resource": "*"
        }]
    })
    .to_string()
}

/// The actions the rows' documents name, so successive writes to one monitor really
/// change the stored bytes.
const POLICY_ACTIONS: [&str; 3] = ["rum:PutRumEvents", "rum:GetAppMonitorData", "rum:*"];

/// `PutResourcePolicy`: `PUT /appmonitor/{Name}/policy`, with `PolicyRevisionId` in the
/// body when the row names one.
async fn put_policy(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    revision_id: Option<&str>,
) -> crate::support::Reply {
    let mut body = json!({"PolicyDocument": document});
    if let Some(revision_id) = revision_id {
        body.as_object_mut()
            .expect("a JSON object")
            .insert("PolicyRevisionId".to_string(), json!(revision_id));
    }
    call(
        router,
        "PUT",
        &format!("/appmonitor/{name}/policy"),
        Some(body),
    )
    .await
}

/// `DeleteResourcePolicy`: `DELETE /appmonitor/{Name}/policy`, with `PolicyRevisionId`
/// bound to the `policyRevisionId` query string (`@httpQuery`).
async fn delete_policy(
    router: &MultiServiceRouter,
    name: &str,
    revision_id: Option<&str>,
) -> crate::support::Reply {
    let path = match revision_id {
        Some(revision_id) => format!(
            "/appmonitor/{name}/policy?policyRevisionId={}",
            crate::support::percent_encoded(revision_id)
        ),
        None => format!("/appmonitor/{name}/policy"),
    };
    call(router, "DELETE", &path, None).await
}

/// `GetResourcePolicy`: `GET /appmonitor/{Name}/policy`.
async fn get_policy(router: &MultiServiceRouter, name: &str) -> crate::support::Reply {
    call(router, "GET", &format!("/appmonitor/{name}/policy"), None).await
}

/// Create one app monitor for a row.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
}

/// What a row's refused request names as its `PolicyRevisionId`, as a CLASS of value: the
/// literal is derived from the revisions the row's policy actually took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The member is absent from the request.
    Nothing,
    /// The revision the policy carried before the one in force.
    Stale,
    /// A well-formed id no revision of any policy here carries.
    Foreign,
    /// The shortest id the model admits (1 character), which is no revision's.
    ShortestPossible,
    /// The longest id the model admits (255 characters).
    LongestPossible,
    /// The id in force with a digit appended — a near miss, so a prefix comparison would
    /// wrongly accept it.
    NearMiss,
}

impl Named {
    fn value(self, in_force: &str, stale: &str) -> Option<String> {
        let (min, max) = POLICY_REVISION_ID_BOUNDS;
        match self {
            Named::Nothing => None,
            Named::Stale => Some(stale.to_string()),
            Named::Foreign => Some("00000000-0000-4000-8000-000000000000".to_string()),
            // A digit no row's revision counter reaches, repeated to each bound, so the
            // literal is inside the modelled length and is no revision of this policy.
            Named::ShortestPossible => Some("7".repeat(min)),
            Named::LongestPossible => Some("7".repeat(max)),
            Named::NearMiss => Some(format!("{in_force}0")),
        }
    }
}

/// Assert `reply` is the modelled refusal, worded EXACTLY as `expected` and carrying
/// nothing from the request: not the id the request named, not the id in force, not the
/// monitor's name. Also asserts the body reports the ONE member the shape models.
fn assert_mismatch_body(
    reply: &crate::support::Reply,
    case: &str,
    expected: &str,
    named: Option<&str>,
    in_force: &str,
    name: &str,
) {
    assert_eq!(
        reply.status,
        MISMATCH_STATUS,
        "case {case:?}: expected HTTP {MISMATCH_STATUS}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let code = reply.error_code(case);
    assert_eq!(
        code, MISMATCH_CODE,
        "case {case:?}: expected error code {MISMATCH_CODE}, got {code:?}"
    );
    let document = reply.json(case);
    let answered = message(&document).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a message member, got {:?}",
            String::from_utf8_lossy(&reply.body)
        )
    });
    assert_eq!(
        answered, expected,
        "case {case:?}: expected the message {expected:?}, got {answered:?}"
    );
    if let Some(named) = named {
        assert!(
            !answered.contains(named),
            "case {case:?}: the message must carry no part of the request, but \
             {answered:?} contains the submitted revision id {named:?}"
        );
    }
    assert!(
        !answered.contains(in_force),
        "case {case:?}: the message must not disclose the revision in force, but \
         {answered:?} contains {in_force:?}"
    );
    assert!(
        !answered.contains(name),
        "case {case:?}: the message must carry no part of the request, but \
         {answered:?} contains the app monitor name {name:?}"
    );
    let members: Vec<&String> = document
        .as_object()
        .map(|body| body.keys().filter(|key| *key != "__type").collect())
        .unwrap_or_default();
    assert_eq!(
        members,
        vec![&"message".to_string()],
        "case {case:?}: InvalidPolicyRevisionIdException models `message` and nothing \
         else, got {members:?}"
    );
}

/// The `PolicyDocument` the monitor currently holds, read back through
/// `GetResourcePolicy`.
async fn held_document(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let read = get_policy(router, name).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected GetResourcePolicy to answer 200, got {} {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    read.json(case)["PolicyDocument"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the read must report a PolicyDocument"))
        .to_string()
}

/// The revision `GetResourcePolicy` reports for `name`.
async fn held_revision(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    get_policy(router, name).await.json(case)["PolicyRevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the read must report a PolicyRevisionId"))
        .to_string()
}

/// Write `revisions` revisions of a policy to a fresh monitor, each one after the first
/// providing the revision in force (which is what ASSIGNS a revision id to the policy, so
/// a put that provides none is refused too). Reports the revision in force, the one before
/// it, and the document in force.
async fn seed_policy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    revisions: usize,
) -> (String, String, String) {
    create_monitor(router, case, name).await;
    let mut document = policy_document(POLICY_ACTIONS[0]);
    let first = put_policy(router, name, &document, None).await;
    assert_eq!(
        first.status,
        200,
        "case {case:?}: a monitor's first policy must be storable, got {} {:?}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let mut in_force = first.json(case)["PolicyRevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the put must report a PolicyRevisionId"))
        .to_string();
    let mut stale = in_force.clone();
    for step in 1..revisions {
        document = policy_document(POLICY_ACTIONS[step % POLICY_ACTIONS.len()]);
        let written = put_policy(router, name, &document, Some(&in_force)).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: write {step} naming the revision in force must succeed, \
             got {} {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        stale = in_force;
        in_force = written.json(case)["PolicyRevisionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the put must report a PolicyRevisionId"))
            .to_string();
    }
    let held = held_document(router, case, name).await;
    (in_force, stale, held)
}

/// The rows the refusal table walks: what the refused request names, and how many
/// revisions the policy took first.
fn rows() -> Vec<(&'static str, Named, usize)> {
    vec![
        ("a stale revision id", Named::Stale, 2),
        ("a stale revision id, four revisions in", Named::Stale, 4),
        ("a foreign revision id", Named::Foreign, 2),
        ("a foreign revision id on the first revision", Named::Foreign, 1),
        (
            "the shortest revision id the model admits",
            Named::ShortestPossible,
            2,
        ),
        (
            "the longest revision id the model admits",
            Named::LongestPossible,
            3,
        ),
        ("a near miss of the revision in force", Named::NearMiss, 2),
        (
            "a near miss of the revision in force, five revisions in",
            Named::NearMiss,
            5,
        ),
    ]
}

#[tokio::test]
async fn a_refused_put_answers_put_resource_policys_own_fixed_wording() {
    let (router, _engine) = emulator();

    // The put's rule also refuses a request that provides NOTHING once a revision id has
    // been assigned, which is the same refusal of the same operation and so the same
    // sentence: that row only exists for the put.
    let mut put_rows = rows();
    put_rows.push(("no revision id at all", Named::Nothing, 2));

    for (row, (case, named, revisions)) in put_rows.into_iter().enumerate() {
        let name = format!("policy-mismatch-wording-put-{row}");
        let (in_force, stale, held) = seed_policy(&router, case, &name, revisions).await;
        let value = named.value(&in_force, &stale);

        // A document that differs from the one in force, so a put that is wrongly
        // accepted shows up in what the reader reports.
        let replacement = policy_document(POLICY_ACTIONS[(row + 1) % POLICY_ACTIONS.len()]);
        let refused = put_policy(&router, &name, &replacement, value.as_deref()).await;
        assert_mismatch_body(
            &refused,
            case,
            PUT_MISMATCH_MESSAGE,
            value.as_deref(),
            &in_force,
            &name,
        );

        // Nothing was written: the policy and the revision in force are untouched.
        let after = held_document(&router, case, &name).await;
        assert_eq!(
            after, held,
            "case {case:?}: a refused put must leave the stored document alone, \
             expected {held:?}"
        );
        let revision_after = held_revision(&router, case, &name).await;
        assert_eq!(
            revision_after, in_force,
            "case {case:?}: a refused put must leave the revision in force \
             ({in_force:?}), got {revision_after:?}"
        );

        // And the compare-and-swap loop still goes through on the current revision.
        let accepted = put_policy(&router, &name, &replacement, Some(&in_force)).await;
        assert_eq!(
            accepted.status,
            200,
            "case {case:?}: a put naming the revision in force must succeed, got {} {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }
}

#[tokio::test]
async fn a_refused_delete_answers_delete_resource_policys_own_fixed_wording() {
    let (router, _engine) = emulator();

    for (row, (case, named, revisions)) in rows().into_iter().enumerate() {
        let name = format!("policy-mismatch-wording-delete-{row}");
        let (in_force, stale, held) = seed_policy(&router, case, &name, revisions).await;
        let value = named.value(&in_force, &stale);

        let refused = delete_policy(&router, &name, value.as_deref()).await;
        assert_mismatch_body(
            &refused,
            case,
            DELETE_MISMATCH_MESSAGE,
            value.as_deref(),
            &in_force,
            &name,
        );

        // The policy and its revision survived the refused delete.
        let after = held_document(&router, case, &name).await;
        assert_eq!(
            after, held,
            "case {case:?}: a refused delete must leave the stored document alone, \
             expected {held:?}"
        );
        let revision_after = held_revision(&router, case, &name).await;
        assert_eq!(
            revision_after, in_force,
            "case {case:?}: a refused delete must leave the revision in force \
             ({in_force:?}), got {revision_after:?}"
        );

        // Deleting with the revision in force then succeeds.
        let deleted = delete_policy(&router, &name, Some(&in_force)).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: a delete naming the revision in force must succeed, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

#[tokio::test]
async fn the_two_operations_word_the_same_refusal_differently() {
    let (router, _engine) = emulator();
    let case = "put and delete refused with the same stale id";
    let name = "policy-mismatch-wording-shared";
    let (in_force, stale, _held) = seed_policy(&router, case, name, 2).await;

    // ONE request value, refused by both operations: the bodies must differ, so the
    // message tells a caller which operation's optimistic-lock check refused it.
    let refused_put = put_policy(
        &router,
        name,
        &policy_document(POLICY_ACTIONS[1]),
        Some(&stale),
    )
    .await;
    let refused_delete = delete_policy(&router, name, Some(&stale)).await;
    assert_mismatch_body(
        &refused_put,
        case,
        PUT_MISMATCH_MESSAGE,
        Some(&stale),
        &in_force,
        name,
    );
    assert_mismatch_body(
        &refused_delete,
        case,
        DELETE_MISMATCH_MESSAGE,
        Some(&stale),
        &in_force,
        name,
    );
    assert_ne!(
        PUT_MISMATCH_MESSAGE, DELETE_MISMATCH_MESSAGE,
        "case {case:?}: the two operations must not share one sentence"
    );
}
