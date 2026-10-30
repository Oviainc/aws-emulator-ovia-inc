// SPDX-License-Identifier: CC0-1.0

//! What `GetResourcePolicy` reports for a resource that holds NO policy, driven end-to-end
//! over the same `MultiServiceRouter` the emulator boots.
//!
//! One rule: **`GetResourcePolicy` always states `resourcePolicy` on its 200, and a
//! resource no policy is attached to states the EMPTY policy document** — a JSON object
//! with no statements. The member is the whole of what this operation reports
//! (`smithy_query(query="operation",operation="GetResourcePolicy")`: `GET
//! /resource-policy/{resourceArn}` -> 200 with `resourcePolicy`, a
//! `ResourcePolicyDocument` of `@length 1..20480`), so a caller that reads, diffs, backs up
//! or restores a policy always has a document to compare against, and "no policy" is told
//! apart from a resource that is not there by the modelled `ResourceNotFoundException`
//! (404) rather than by a missing member.
//!
//! The rows differ along what the rule generalises over: the resource TYPE (a guardrail, a
//! prompt router, a Provisioned Throughput — the types this service mints an ARN for), and
//! HOW the resource came to hold no policy (it never held one, a put was REFUSED, the
//! policy it held was deleted). Each row reads back through the implemented
//! `GetResourcePolicy`, so the empty document is what a later caller sees and not what one
//! call happened to answer.
//!
//! `tests/all/resource_policies.rs` is the suite for the attach / read / delete rules
//! themselves; the three calls, the resource builders and the admitted documents come from
//! it rather than from a second copy of them.

use crate::resource_policies::{
    admitted_documents, delete_policy, get_policy, guardrail_arn, prompt_router_arn,
    provisioned_model_arn, put_policy, NO_POLICY,
};
use crate::support::{emulator, Emulator, Reply};

/// Bedrock's refusal of a document that is not a policy, WORD FOR WORD
/// (`src/ops/put_resource_policy.rs`): fixed text carrying nothing of the request.
const NONCONFORMING_POLICY_DOCUMENT: &str =
    "The provided policy document does not conform to the supported policy formats. For \
     more information about valid policy document formats, check out the Amazon Bedrock \
     Guardrails documentation";

/// Documents the policy grammar refuses, each broken in a DIFFERENT way: not JSON at all,
/// and JSON that is an object but no policy.
fn rejected_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not JSON at all", "{this is not json at all::::"),
        ("a JSON object that is no policy", "{\"hello\": \"world\"}"),
        ("a policy with no statement", "{\"Version\":\"2012-10-17\"}"),
    ]
}

/// The document a read states for `arn`, asserting the read was a 200 first.
async fn reported_policy(case: &str, emu: &Emulator, arn: &str) -> String {
    let read: Reply = get_policy(emu, arn).await;
    assert_eq!(
        (read.status, read.error.as_deref()),
        (200, None),
        "case {case:?}: a resource this account holds is read with 200, got {} {:?}",
        read.status,
        read.body
    );
    assert!(
        read.get("resourcePolicy").is_string(),
        "case {case:?}: GetResourcePolicy always states resourcePolicy on its 200, got {:?}",
        read.body
    );
    read.text("resourcePolicy")
}

/// A resource that never held a policy reports the empty policy document — over every
/// resource type this service mints an ARN for.
#[tokio::test]
async fn a_resource_that_never_held_a_policy_reports_the_empty_document() {
    let emu = emulator();
    let rows = [
        ("guardrail", guardrail_arn(&emu, "emptydoc-guardrail").await),
        (
            "prompt router",
            prompt_router_arn(&emu, "emptydoc-router").await,
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "emptydocprovisioned").await,
        ),
    ];
    for (case, arn) in rows {
        let stated = reported_policy(case, &emu, &arn).await;
        assert_eq!(
            stated, NO_POLICY,
            "case {case:?}: expected the empty policy document {NO_POLICY:?}, got {stated:?}"
        );
    }
}

/// A REFUSED put leaves the resource exactly as it was: the baseline a caller compares
/// against is the empty policy document before the put and the same document after it, and
/// an attached policy is not replaced by a document the service would not take.
#[tokio::test]
async fn a_refused_put_leaves_the_reported_document_unchanged() {
    let emu = emulator();
    let arn = guardrail_arn(&emu, "refused-put-guardrail").await;
    let (_, attached, reported) = admitted_documents()
        .into_iter()
        .next()
        .expect("the document table holds at least one admitted document");

    // (case, the document the read must state before and after each refusal)
    for (stage, baseline) in [
        ("no policy attached", NO_POLICY.to_string()),
        ("a policy attached", reported.clone()),
    ] {
        if stage == "a policy attached" {
            let put = put_policy(&emu, &arn, &attached).await;
            assert_eq!(
                put.status, 201,
                "case {stage:?}: PutResourcePolicy must answer 201, got {} {:?}",
                put.status, put.body
            );
        }
        let before = reported_policy(stage, &emu, &arn).await;
        assert_eq!(
            before, baseline,
            "case {stage:?}: expected the baseline {baseline:?}, got {before:?}"
        );

        for (form, document) in rejected_documents() {
            let case = format!("{stage} / {form}");
            let refused = put_policy(&emu, &arn, document).await;
            assert_eq!(
                (refused.status, refused.error.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a document that is no policy is refused 400 \
                 ValidationException, got {} {:?}",
                refused.status,
                refused.body
            );
            let stated = refused
                .get("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                stated, NONCONFORMING_POLICY_DOCUMENT,
                "case {case:?}: expected {NONCONFORMING_POLICY_DOCUMENT:?}, got {stated:?}"
            );
            let after = reported_policy(&case, &emu, &arn).await;
            assert_eq!(
                after, baseline,
                "case {case:?}: a refused put changes nothing a later read sees, expected \
                 {baseline:?}, got {after:?}"
            );
        }
    }
}

/// A policy that was DELETED leaves the resource reporting the empty document again — over
/// every admitted document form, so what the read states is the resource's state and not
/// the shape of the document that was there.
#[tokio::test]
async fn a_deleted_policy_leaves_the_resource_reporting_the_empty_document() {
    let emu = emulator();
    let arn = guardrail_arn(&emu, "deleted-emptydoc-guardrail").await;
    for (form, submitted, reported) in admitted_documents() {
        let put = put_policy(&emu, &arn, &submitted).await;
        assert_eq!(
            put.status, 201,
            "case {form:?}: PutResourcePolicy must answer 201, got {} {:?}",
            put.status, put.body
        );
        let attached = reported_policy(form, &emu, &arn).await;
        assert_eq!(
            attached, reported,
            "case {form:?}: the attached document is reported, got {attached:?}"
        );

        let removed = delete_policy(&emu, &arn).await;
        assert_eq!(
            removed.status, 200,
            "case {form:?}: DeleteResourcePolicy must answer 200, got {} {:?}",
            removed.status, removed.body
        );
        let stated = reported_policy(form, &emu, &arn).await;
        assert_eq!(
            stated, NO_POLICY,
            "case {form:?}: expected the empty policy document {NO_POLICY:?} after the \
             delete, got {stated:?}"
        );
    }
}
