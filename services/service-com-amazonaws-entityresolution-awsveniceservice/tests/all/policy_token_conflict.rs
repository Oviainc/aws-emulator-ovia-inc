// SPDX-License-Identifier: CC0-1.0

//! The TEXT `PutPolicy` refuses a stale revision token with.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is
//! the body an AWS SDK client would parse. The policy helpers are the sibling
//! `resource_policy` and `policy_statements` suites', so this suite drives the same
//! calls they do rather than cloning them.
//!
//! The rule under test: when `PutPolicy` carries a `token` that is not the current
//! revision of the resource's policy, the write is refused with HTTP 400
//! `ConflictException` whose message is FIXED — it carries nothing of the request, and
//! in particular not the token, which is a value a write mints per revision — the
//! refused write does not apply, and a write carrying the token the standing revision
//! reports succeeds and replaces the document.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the resource type over the whole catalogue the modelled
//! `VeniceGlobalArn` admits, and the WAY the token is stale — superseded by another
//! `PutPolicy`, superseded by an `AddPolicyStatement` (the recorded sequence), issued
//! for the policy of ANOTHER resource, never issued by this service at all, and offered
//! to a resource that carries no policy, which has no current revision for any token to
//! be.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::policy_statements::{add_policy_statement, create_resource, RESOURCE_TYPES};
use crate::resource_policy::{get_policy, put_policy};
use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape, string_member, Emulator, Reply,
};

/// The `ConflictException$message` AWS answers a `PutPolicy` whose `token` is not the
/// current revision — RECORDED, and written out here so this suite pins the text
/// independently of the service crate's own constant.
const INCORRECT_TOKEN_MESSAGE: &str = "Policy put request failed. Incorrect token.";

/// A `PolicyToken` that is well-formed but was never issued: 36 characters in the
/// modelled `8-4-4-4-12` hexadecimal grouping (`PolicyToken`, `length 36..36`), so the
/// boundary validator passes it and the handler is what refuses it.
const NEVER_ISSUED_TOKEN: &str = "00000000-0000-4000-8000-000000000000";

/// A policy document naming this account's root user, parameterised by `sid` so two
/// documents of one row differ.
fn document(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account_id()) },
            "Action": "entityresolution:GetIdNamespace",
        }],
    })
    .to_string()
}

/// A statement body for `AddPolicyStatement`, which is one way a revision is bumped
/// behind a token a caller is holding.
fn statement() -> Value {
    json!({
        "effect": "Allow",
        "action": ["entityresolution:GetPolicy"],
        "principal": [account_id()],
    })
}

/// How a token comes to be stale.
#[derive(Clone, Copy)]
enum Staleness {
    /// Another `PutPolicy` produced a newer revision.
    SupersededByAWrite,
    /// An `AddPolicyStatement` produced a newer revision — the recorded sequence.
    SupersededByAStatement,
    /// The token of ANOTHER resource's standing revision.
    AnotherResourcesToken,
    /// Well-formed, never issued by this service.
    NeverIssued,
    /// Offered to a resource that carries no policy, so there is no current revision.
    NoPolicyAtAll,
}

/// The token to send, and the document that must still be standing afterwards (`None`
/// when the resource carries no policy at all).
async fn stale_token(
    emulator: &Emulator,
    arn: &str,
    other_arn: &str,
    staleness: Staleness,
) -> (String, Option<String>) {
    match staleness {
        Staleness::SupersededByAWrite => {
            let first = put_policy(emulator, arn, &document("first"), None).await;
            let token = string_member(&first.body, "token");
            let second = put_policy(emulator, arn, &document("second"), None).await;
            assert_eq!(
                second.status, 200,
                "the write that supersedes the token must succeed, got {second:?}"
            );
            (token, Some(document("second")))
        }
        Staleness::SupersededByAStatement => {
            let written = put_policy(emulator, arn, &document("first"), None).await;
            let token = string_member(&written.body, "token");
            let bumped = add_policy_statement(emulator, arn, "bump", &statement()).await;
            assert_eq!(
                bumped.status, 200,
                "the statement that bumps the revision must succeed, got {bumped:?}"
            );
            (token, Some(string_member(&bumped.body, "policy")))
        }
        Staleness::AnotherResourcesToken => {
            let standing = put_policy(emulator, arn, &document("standing"), None).await;
            assert_eq!(
                standing.status, 200,
                "the standing revision must be written, got {standing:?}"
            );
            let elsewhere = put_policy(emulator, other_arn, &document("elsewhere"), None).await;
            assert_eq!(
                elsewhere.status, 200,
                "the other resource's revision must be written, got {elsewhere:?}"
            );
            (
                string_member(&elsewhere.body, "token"),
                Some(document("standing")),
            )
        }
        Staleness::NeverIssued => {
            let standing = put_policy(emulator, arn, &document("standing"), None).await;
            assert_eq!(
                standing.status, 200,
                "the standing revision must be written, got {standing:?}"
            );
            (NEVER_ISSUED_TOKEN.to_string(), Some(document("standing")))
        }
        // Nothing is written first: the resource exists and carries no policy.
        Staleness::NoPolicyAtAll => (NEVER_ISSUED_TOKEN.to_string(), None),
    }
}

/// The document `GetPolicy` reports for `arn`, or `None` when the resource carries no
/// policy (which that reader answers as a 404).
async fn standing_document(emulator: &Emulator, arn: &str) -> Option<String> {
    let reply = get_policy(emulator, arn).await;
    if reply.status == 200 {
        Some(string_member(&reply.body, "policy"))
    } else {
        None
    }
}

/// Assert `reply` is the stale-token refusal: the modelled code and status, the fixed
/// message, and nothing of the request echoed into it.
fn assert_incorrect_token(case: &str, reply: &Reply, token: &str, arn: &str) {
    assert_error_shape(case, reply, 400, "ConflictException");
    let message = string_member(&reply.body, "message");
    assert_eq!(
        message, INCORRECT_TOKEN_MESSAGE,
        "case {case:?}: expected the refusal {INCORRECT_TOKEN_MESSAGE:?}, got {message:?}"
    );
    assert!(
        !message.contains(token) && !message.contains(arn),
        "case {case:?}: the refusal states nothing of the request, got {message:?}"
    );
}

fn staleness_cases() -> Vec<(&'static str, Staleness)> {
    vec![
        (
            "a token another PutPolicy superseded",
            Staleness::SupersededByAWrite,
        ),
        (
            "a token an AddPolicyStatement superseded",
            Staleness::SupersededByAStatement,
        ),
        (
            "the token of another resource's policy",
            Staleness::AnotherResourcesToken,
        ),
        ("a token this service never issued", Staleness::NeverIssued),
        (
            "any token on a resource that carries no policy",
            Staleness::NoPolicyAtAll,
        ),
    ]
}

#[tokio::test]
async fn a_stale_token_is_refused_with_one_fixed_message_and_applies_nothing() {
    let emulator = Emulator::new();
    let mut texts = BTreeSet::new();

    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        for (position, (label, staleness)) in staleness_cases().into_iter().enumerate() {
            let case = format!("{resource_type}: {label}");
            let name = format!("tok-{index}-{position}");
            let arn = create_resource(&emulator, resource_type, &name).await;
            let other = create_resource(&emulator, resource_type, &format!("{name}-other")).await;

            let (token, standing) = stale_token(&emulator, &arn, &other, staleness).await;

            let refused = put_policy(&emulator, &arn, &document("rejected"), Some(&token)).await;
            assert_incorrect_token(&case, &refused, &token, &arn);
            texts.insert(string_member(&refused.body, "message"));

            // The refused write did not apply: what is standing is what was there
            // before, including nothing at all.
            assert_eq!(
                standing_document(&emulator, &arn).await,
                standing,
                "case {case:?}: a refused write must leave the standing revision alone"
            );

            // ...and the token the standing revision reports is accepted, so the
            // refusal was about the token and not about the write.
            let standing_now = get_policy(&emulator, &arn).await;
            let accepted = if standing_now.status == 200 {
                let current = string_member(&standing_now.body, "token");
                put_policy(&emulator, &arn, &document("accepted"), Some(&current)).await
            } else {
                // A resource with no policy has no current revision, so the write that
                // establishes one carries no token.
                put_policy(&emulator, &arn, &document("accepted"), None).await
            };
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: a write carrying the current revision must succeed, got \
                 {accepted:?}"
            );
            assert_eq!(
                standing_document(&emulator, &arn).await,
                Some(document("accepted")),
                "case {case:?}: the accepted write must replace the document"
            );
        }
    }

    assert_eq!(
        texts.len(),
        1,
        "the stale-token message is fixed, so every row must answer the same one, saw \
         {texts:?}"
    );
}
