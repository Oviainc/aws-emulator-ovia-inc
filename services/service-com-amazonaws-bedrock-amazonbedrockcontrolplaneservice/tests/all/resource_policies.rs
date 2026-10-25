// SPDX-License-Identifier: CC0-1.0

//! The resource policy end-to-end: `PutResourcePolicy` attaches one,
//! `GetResourcePolicy` reads it back, over the same `MultiServiceRouter` the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! recorded request:
//!
//! 1. **A policy attaches to the resource its ARN names and reads back VERBATIM** — the
//!    rows span the resource TYPES this service mints an ARN for (a guardrail, a prompt
//!    router, a Provisioned Throughput) and the FORMS a policy document is written in
//!    (one statement as an object or in a list, two statements, `Allow` and `Deny`, the
//!    `Not…` complements, no `Version`, whitespace and key order the caller chose).
//! 2. **A resource holds at most ONE policy**, so a further put REPLACES it and the read
//!    reports the last document stored; two resources hold their own policies
//!    independently, and both ARN spellings of one resource address the same policy.
//! 3. **A policy attaches only to a resource this account HOLDS** — an ARN of another
//!    vendor, another region, another account, a Bedrock resource type nothing created,
//!    or a resource that was deleted, is refused 400 `ValidationException` and attaches
//!    nothing.
//! 4. **A document that is not an IAM resource-based policy is refused** the same 400,
//!    over the rows the shared grammar separates (not JSON, not an object, no statement,
//!    an unknown policy version, a statement missing an element).
//! 5. **A resource with no policy is 404 `ResourceNotFoundException`** on the read, and a
//!    policy whose resource is deleted is reported the same way — a read never reports a
//!    grant on a resource that no longer exists.
//!
//! The status codes and error shapes come from the model
//! (`smithy_query(query="operation",operation="PutResourcePolicy,GetResourcePolicy")`:
//! `POST /resource-policy` -> 201 with `resourceArn`, `GET /resource-policy/{resourceArn}`
//! -> 200 with `resourcePolicy`, `ValidationException` 400 on the put and
//! `ResourceNotFoundException` 404 on the read). The requests go through
//! `crate::support` — the ONE harness every suite in this crate drives the emulator with.

use serde_json::json;

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

async fn put_policy(emu: &Emulator, resource_arn: &str, document: &str) -> Reply {
    emu.call(
        "POST",
        "/resource-policy",
        Some(json!({ "resourceArn": resource_arn, "resourcePolicy": document })),
    )
    .await
}

async fn get_policy(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/resource-policy/{}", label(resource_arn)),
        None,
    )
    .await
}

/// A guardrail created through `CreateGuardrail`, as its `guardrailArn`.
async fn guardrail_arn(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .create(json!({
            "name": name,
            "blockedInputMessaging": "blocked in",
            "blockedOutputsMessaging": "blocked out",
        }))
        .await;
    assert_eq!(
        reply.status, 202,
        "case {name:?}: CreateGuardrail must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("guardrailArn")
}

/// A prompt router created through `CreatePromptRouter`, as its `promptRouterArn`.
async fn prompt_router_arn(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .call(
            "POST",
            "/prompt-routers",
            Some(json!({
                "promptRouterName": name,
                "models": [{
                    "modelArn":
                        "arn:aws:bedrock:us-west-2::foundation-model/\
                         anthropic.claude-3-haiku-20240307-v1:0",
                }],
                "fallbackModel": {
                    "modelArn":
                        "arn:aws:bedrock:us-west-2::foundation-model/\
                         anthropic.claude-3-sonnet-20240229-v1:0",
                },
                "routingCriteria": { "responseQualityDifference": 5.0 },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: CreatePromptRouter must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("promptRouterArn")
}

/// A Provisioned Throughput created through `CreateProvisionedModelThroughput`, as its
/// `provisionedModelArn`.
async fn provisioned_model_arn(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .call(
            "POST",
            "/provisioned-model-throughput",
            Some(json!({
                "provisionedModelName": name,
                "modelId": "amazon.titan-text-lite-v1",
                "modelUnits": 1,
                "commitmentDuration": "OneMonth",
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "case {name:?}: CreateProvisionedModelThroughput must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("provisionedModelArn")
}

// ---------------------------------------------------------------------------------------
// Policy documents the rows carry.
// ---------------------------------------------------------------------------------------

/// The one statement every accepting document is built from: an `Effect`, an `Action` and
/// a `Principal`, as an IAM resource-based policy states them.
const STATEMENT: &str = "{\"Effect\":\"Allow\",\"Action\":[\"bedrock:ApplyGuardrail\",\
                         \"bedrock:GetGuardrail\"],\
                         \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}";

/// Documents the IAM resource-policy grammar admits, each written in a DIFFERENT form —
/// the forms `service_common::iam_policy` states (one statement as an object or in a
/// list, several statements, both effects, the `Not…` complements, an omitted `Version`)
/// plus whitespace and key order a caller chooses, which a verbatim store must keep.
fn admitted_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement in a list",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT}]}}"),
        ),
        (
            "one statement written as the object itself",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":{STATEMENT}}}"),
        ),
        (
            "two statements",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT},{STATEMENT}]}}"),
        ),
        (
            "no Version, which a policy may omit",
            format!("{{\"Statement\":[{STATEMENT}]}}"),
        ),
        (
            "the legacy policy version",
            format!("{{\"Version\":\"2008-10-17\",\"Statement\":[{STATEMENT}]}}"),
        ),
        (
            "a Deny over the Not… complements",
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\",\
             \"NotAction\":\"bedrock:DeleteGuardrail\",\
             \"NotPrincipal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\"]}}]}"
                .to_string(),
        ),
        (
            "indented, with the elements in the caller's own order",
            format!(
                "{{\n  \"Statement\": [\n    {STATEMENT}\n  ],\n  \
                 \"Version\": \"2012-10-17\"\n}}"
            ),
        ),
        (
            "a Sid carrying a non-ASCII character the member's pattern admits",
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"caf\u{e9}\",\
             \"Effect\":\"Allow\",\"Action\":\"bedrock:GetGuardrail\",\
             \"Principal\":\"*\",\"Resource\":\"*\"}]}"
                .to_string(),
        ),
    ]
}

/// Texts the grammar refuses, one per clause it states — the catalog's source is
/// `service_common::iam_policy`'s `PolicyDefect`, read here through the operation.
fn refused_documents() -> Vec<(&'static str, String)> {
    vec![
        ("not JSON at all", "let me in".to_string()),
        ("a bare number", "5".to_string()),
        ("an array of statements", format!("[{STATEMENT}]")),
        ("an empty object", "{}".to_string()),
        (
            "a Statement that is an empty list",
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "a policy version no language has",
            format!("{{\"Version\":\"2024-01-01\",\"Statement\":[{STATEMENT}]}}"),
        ),
        (
            "a statement with no Effect",
            "{\"Statement\":[{\"Action\":\"bedrock:GetGuardrail\",\"Principal\":\"*\"}]}"
                .to_string(),
        ),
        (
            "a statement with no Principal, which a RESOURCE policy must name",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"bedrock:GetGuardrail\"}]}"
                .to_string(),
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// 1 + 2. A policy attaches to the resource its ARN names, reads back verbatim, and is
// replaced by a further put.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_resource_type_round_trips_every_policy_form_verbatim() {
    let emu = emulator();
    // One resource of each type this service mints an ARN for and holds a record of, so
    // the rule is exercised across the resource dimension it generalises over rather than
    // on the guardrail alone.
    let resources = [
        ("guardrail", guardrail_arn(&emu, "policy-guardrail").await),
        (
            "prompt router",
            prompt_router_arn(&emu, "policy-router").await,
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "policy-provisioned").await,
        ),
    ];

    for (resource, arn) in &resources {
        for (form, document) in admitted_documents() {
            let case = format!("{resource} / {form}");
            let put = put_policy(&emu, arn, &document).await;
            assert_eq!(
                put.status, 201,
                "case {case:?}: PutResourcePolicy must answer 201, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                put.text("resourceArn"),
                *arn,
                "case {case:?}: the put must report the resource it attached to, got {:?}",
                put.body
            );

            let read = get_policy(&emu, arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetResourcePolicy must answer 200, got {} {:?}",
                read.status, read.body
            );
            let reported = read.text("resourcePolicy");
            assert_eq!(
                reported, document,
                "case {case:?}: the stored document must come back byte for byte, got \
                 {reported:?}"
            );
        }
    }

    // Each resource still reports the LAST document put on it — the three resources hold
    // their own policies rather than one shared record.
    let last = admitted_documents()
        .last()
        .expect("the document table has rows")
        .1
        .clone();
    for (resource, arn) in &resources {
        let read = get_policy(&emu, arn).await;
        assert_eq!(
            read.text("resourcePolicy"),
            last,
            "case {resource:?}: a resource reports the last policy put on it, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_further_put_replaces_the_policy_the_resource_holds() {
    let emu = emulator();
    let arn = guardrail_arn(&emu, "replaced-policy").await;
    let other = guardrail_arn(&emu, "untouched-policy").await;

    let documents = admitted_documents();
    let first = documents[0].1.clone();
    put_policy(&emu, &other, &first).await;

    // Every row is put in turn over the SAME resource: the read after each one reports
    // that row and nothing of the row before it.
    for (form, document) in &documents {
        let put = put_policy(&emu, &arn, document).await;
        assert_eq!(
            put.status, 201,
            "case {form:?}: a put over an attached policy must answer 201, got {} {:?}",
            put.status, put.body
        );
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.text("resourcePolicy"),
            *document,
            "case {form:?}: the last put decides the policy in force, got {:?}",
            read.body
        );
        let untouched = get_policy(&emu, &other).await;
        assert_eq!(
            untouched.text("resourcePolicy"),
            first,
            "case {form:?}: another resource's policy is untouched, got {:?}",
            untouched.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. A policy attaches only to a resource this account holds.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn both_arn_spellings_of_one_resource_address_the_same_policy() {
    // A policy is attached to the RESOURCE, not to one spelling of its ARN: a prompt
    // router is addressable as `default-prompt-router/<id>` and as `prompt-router/<id>`
    // (the two spellings this service's ARN resolution reads), so a put under either
    // spelling is the policy the other spelling reports.
    let emu = emulator();
    let router = prompt_router_arn(&emu, "two-spellings").await;
    let other_spelling = router.replace(":default-prompt-router/", ":prompt-router/");
    let documents = admitted_documents();

    for (case, written, read_through) in [
        ("put under the ARN, read under the alias", &router, &other_spelling),
        ("put under the alias, read under the ARN", &other_spelling, &router),
    ] {
        for (form, document) in &documents {
            let put = put_policy(&emu, written, document).await;
            assert_eq!(
                put.status, 201,
                "case {case:?} / {form:?}: the put must answer 201, got {} {:?}",
                put.status, put.body
            );
            let read = get_policy(&emu, read_through).await;
            assert_eq!(
                read.text("resourcePolicy"),
                *document,
                "case {case:?} / {form:?}: both spellings address one policy, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_is_refused_and_attaches_nothing() {
    let emu = emulator();
    let held = guardrail_arn(&emu, "held-guardrail").await;
    let document = admitted_documents()[0].1.clone();
    let guardrail_id = held
        .rsplit_once(":guardrail/")
        .expect("a guardrail ARN carries its id")
        .1
        .to_string();
    // A prompt router's store is keyed by its whole `PromptRouterArn`, so its ARN is the
    // one to vary the region and the account of: the guardrail family is addressed by its
    // `guardrailId` throughout this service (its store is keyed by that id, which is what
    // `guardrailIdentifier` admits beside the ARN), so an ARN's region field does not
    // select another guardrail there.
    let router = prompt_router_arn(&emu, "held-router").await;
    let router_id = router
        .rsplit_once(":default-prompt-router/")
        .expect("a prompt router ARN carries its id")
        .1
        .to_string();

    // The rows differ along what the rule generalises over: the ARN's vendor, region,
    // account, resource type and resource id, plus a value that is no ARN at all.
    let rows = [
        ("another vendor's resource", format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:endpoint/x")),
        (
            "the same prompt router id in another region",
            format!("arn:aws:bedrock:eu-west-1:{ACCOUNT}:default-prompt-router/{router_id}"),
        ),
        (
            "the same prompt router id in another account",
            format!("arn:aws:bedrock:{REGION}:000000000001:default-prompt-router/{router_id}"),
        ),
        (
            "a guardrail id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/zzzzzzzzzzzz"),
        ),
        (
            "a Bedrock resource type nothing created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/zzzzzzzzzzzz"),
        ),
        (
            "an evaluation job nothing created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:evaluation-job/zzzzzzzzzzzz"),
        ),
        ("a bare resource name", guardrail_id.clone()),
        ("text that is no ARN", "not-an-arn".to_string()),
    ];

    for (case, resource_arn) in rows {
        let put = put_policy(&emu, &resource_arn, &document).await;
        assert_eq!(
            put.status, 400,
            "case {case:?}: an ARN naming no resource of this account is refused 400, got \
             {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal is the modelled ValidationException, got {:?}",
            put.error
        );
        let read = get_policy(&emu, &resource_arn).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the refused put attached nothing, so the read is 404, got {} {:?}",
            read.status, read.body
        );
    }

    // The resource that IS held still takes a policy, so the refusals above are about the
    // ARN rather than about the request as a whole.
    let put = put_policy(&emu, &held, &document).await;
    assert_eq!(
        put.status, 201,
        "case \"held guardrail\": the held resource takes the same document, got {} {:?}",
        put.status, put.body
    );
}

// ---------------------------------------------------------------------------------------
// 4. A document that is not an IAM resource-based policy is refused.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_document_that_is_not_a_resource_policy_is_refused_and_attaches_nothing() {
    let emu = emulator();
    let arn = guardrail_arn(&emu, "policy-grammar").await;
    let admitted = admitted_documents()[0].1.clone();
    put_policy(&emu, &arn, &admitted).await;

    for (case, document) in refused_documents() {
        let put = put_policy(&emu, &arn, &document).await;
        assert_eq!(
            put.status, 400,
            "case {case:?}: a document that is not a resource policy is refused 400, got {} \
             {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal is the modelled ValidationException, got {:?}",
            put.error
        );
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.text("resourcePolicy"),
            admitted,
            "case {case:?}: a refused put leaves the attached policy as it was, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. A resource with no policy, and a policy whose resource is gone.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_with_no_policy_is_not_found_on_the_read() {
    let emu = emulator();
    // One resource of each type, each holding no policy: the read reports the modelled
    // 404 rather than an empty 200.
    let rows = [
        ("guardrail", guardrail_arn(&emu, "unattached-guardrail").await),
        (
            "prompt router",
            prompt_router_arn(&emu, "unattached-router").await,
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "unattached-provisioned").await,
        ),
    ];

    for (case, arn) in rows {
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a resource holding no policy is 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal is the modelled ResourceNotFoundException, got {:?}",
            read.error
        );
    }
}

#[tokio::test]
async fn a_policy_is_reported_no_longer_once_its_resource_is_deleted() {
    let emu = emulator();
    let deleted = guardrail_arn(&emu, "deleted-guardrail").await;
    let kept = guardrail_arn(&emu, "kept-guardrail").await;
    let document = admitted_documents()[0].1.clone();
    put_policy(&emu, &deleted, &document).await;
    put_policy(&emu, &kept, &document).await;

    let removed = emu.delete(&deleted, None).await;
    assert_eq!(
        removed.status, 202,
        "case \"delete the guardrail\": DeleteGuardrail must answer 202, got {} {:?}",
        removed.status, removed.body
    );

    let read = get_policy(&emu, &deleted).await;
    assert_eq!(
        read.status, 404,
        "case \"deleted guardrail\": its policy is reported no longer, got {} {:?}",
        read.status, read.body
    );
    let kept_read = get_policy(&emu, &kept).await;
    assert_eq!(
        kept_read.text("resourcePolicy"),
        document,
        "case \"kept guardrail\": the other resource keeps its policy, got {:?}",
        kept_read.body
    );

    // A put after the delete is still accepted for the resource that remains, and the
    // store no longer carries the deleted resource's document.
    let put = put_policy(&emu, &kept, &document).await;
    assert_eq!(
        put.status, 201,
        "case \"put after a delete\": the held resource still takes a policy, got {} {:?}",
        put.status, put.body
    );
    let read = get_policy(&emu, &deleted).await;
    assert_eq!(
        read.status, 404,
        "case \"deleted guardrail after a later put\": still 404, got {} {:?}",
        read.status, read.body
    );
}
