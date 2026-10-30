// SPDX-License-Identifier: CC0-1.0

//! The resource policy end-to-end: `PutResourcePolicy` attaches one,
//! `GetResourcePolicy` reads it back and `DeleteResourcePolicy` takes it away, over the
//! same `MultiServiceRouter` the emulator boots.
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
//!    an unknown policy version, a statement missing an element), each with Bedrock's own
//!    FIXED non-conforming-document sentence — one text for every such document, carrying
//!    nothing of the request.
//! 5. **A resource this account holds which carries no policy is 200 with `resourcePolicy`
//!    set to the EMPTY policy document** [`NO_POLICY`], while an ARN naming no resource of
//!    this account — including a resource that was deleted — is 404
//!    `ResourceNotFoundException`: a read never reports a grant on a resource that no
//!    longer exists, and never reports a found resource as missing.
//! 6. **A delete leaves the resource it names governed by no policy, and only that one** —
//!    over the same resource types and document forms the attach spans, after which the
//!    read reports the empty policy document and a further put attaches again; the delete is
//!    `@idempotent`, so repeating it, or making it on a resource that holds no policy, is
//!    the same 200 as making it once, and the other resources keep their documents.
//! 7. **A delete reaches exactly the resources the put and the read reach** — either ARN
//!    spelling of one resource removes the one policy, while an ARN naming no resource of
//!    this account (including a resource that was deleted) is 404
//!    `ResourceNotFoundException` with the sentence the model writes for it.
//!
//! The status codes and error shapes come from the model
//! (`smithy_query(query="operation",operation="PutResourcePolicy,GetResourcePolicy,DeleteResourcePolicy")`:
//! `POST /resource-policy` -> 201 with `resourceArn`, `GET /resource-policy/{resourceArn}`
//! -> 200 with `resourcePolicy`, `DELETE /resource-policy/{resourceArn}` -> 200 with no
//! members and `@idempotent`, `ValidationException` 400 on the put and
//! `ResourceNotFoundException` 404 on the read and the delete). The requests go through
//! `crate::support` — the ONE harness every suite in this crate drives the emulator with.

use serde_json::json;

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// What a read reports for a resource that holds no policy: the EMPTY policy document, the
/// form "no policy is attached" takes in the member `GetResourcePolicy` reports
/// (`src/ops/get_resource_policy.rs`).
///
/// `pub(crate)` for the reason [`get_policy`] is.
pub(crate) const NO_POLICY: &str = "{}";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// `pub(crate)` because `tests/all/resource_policy_empty_document.rs` — the suite whose
/// rule is the document a read states for a resource holding NO policy — drives the same
/// three calls rather than a second copy of them.
pub(crate) async fn put_policy(emu: &Emulator, resource_arn: &str, document: &str) -> Reply {
    emu.call(
        "POST",
        "/resource-policy",
        Some(json!({ "resourceArn": resource_arn, "resourcePolicy": document })),
    )
    .await
}

pub(crate) async fn get_policy(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/resource-policy/{}", label(resource_arn)),
        None,
    )
    .await
}

pub(crate) async fn delete_policy(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/resource-policy/{}", label(resource_arn)),
        None,
    )
    .await
}

/// A guardrail created through `CreateGuardrail`, as its `guardrailArn`.
pub(crate) async fn guardrail_arn(emu: &Emulator, name: &str) -> String {
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
pub(crate) async fn prompt_router_arn(emu: &Emulator, name: &str) -> String {
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
pub(crate) async fn provisioned_model_arn(emu: &Emulator, name: &str) -> String {
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
/// plus whitespace and key order a caller chooses — as `(case, submitted, reported)`.
///
/// `reported` is what a read hands back: the document's CANONICAL form, which is the
/// submitted document with the whitespace between its JSON tokens dropped and nothing else
/// about it changed. Amazon Bedrock holds a resource policy parsed and serializes it again
/// on read, so the key order the caller chose, its string literals and its number
/// spellings survive while its layout does not. Each row's reported text is written out
/// here rather than computed, so a row pins the bytes a client reads.
pub(crate) fn admitted_documents() -> Vec<(&'static str, String, String)> {
    let already_canonical = |case: &'static str, document: String| (case, document.clone(), document);
    vec![
        already_canonical(
            "one statement in a list",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT}]}}"),
        ),
        already_canonical(
            "one statement written as the object itself",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":{STATEMENT}}}"),
        ),
        already_canonical(
            "two statements",
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT},{STATEMENT}]}}"),
        ),
        already_canonical(
            "no Version, which a policy may omit",
            format!("{{\"Statement\":[{STATEMENT}]}}"),
        ),
        already_canonical(
            "the legacy policy version",
            format!("{{\"Version\":\"2008-10-17\",\"Statement\":[{STATEMENT}]}}"),
        ),
        already_canonical(
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
            // The layout is gone; the caller's own element order is not.
            format!("{{\"Statement\":[{STATEMENT}],\"Version\":\"2012-10-17\"}}"),
        ),
        (
            "a space after every colon and comma, as a default JSON dump writes it",
            format!(
                "{{\"Version\": \"2012-10-17\", \"Statement\": [{STATEMENT}]}}"
            ),
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT}]}}"),
        ),
        (
            "whitespace around the whole document and inside a Sid",
            " \t\n{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"two  words\",\
             \"Effect\":\"Allow\",\"Action\":\"bedrock:GetGuardrail\",\
             \"Principal\":\"*\",\"Resource\":\"*\"}]}\n "
                .to_string(),
            // The whitespace INSIDE the `Sid` string is part of its value, so it stays.
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"two  words\",\
             \"Effect\":\"Allow\",\"Action\":\"bedrock:GetGuardrail\",\
             \"Principal\":\"*\",\"Resource\":\"*\"}]}"
                .to_string(),
        ),
        already_canonical(
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
// 1 + 2. A policy attaches to the resource its ARN names, reads back in its canonical
// form, and is replaced by a further put.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_resource_type_round_trips_every_policy_form_canonically() {
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
        for (form, document, reported) in admitted_documents() {
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
            let answered = read.text("resourcePolicy");
            assert_eq!(
                answered, reported,
                "case {case:?}: the read must report the canonical form of the stored \
                 document, got {answered:?}"
            );
        }
    }

    // Each resource still reports the LAST document put on it — the three resources hold
    // their own policies rather than one shared record.
    let last = admitted_documents()
        .last()
        .expect("the document table has rows")
        .2
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
    for (form, document, reported) in &documents {
        let put = put_policy(&emu, &arn, document).await;
        assert_eq!(
            put.status, 201,
            "case {form:?}: a put over an attached policy must answer 201, got {} {:?}",
            put.status, put.body
        );
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.text("resourcePolicy"),
            *reported,
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
        for (form, document, reported) in &documents {
            let put = put_policy(&emu, written, document).await;
            assert_eq!(
                put.status, 201,
                "case {case:?} / {form:?}: the put must answer 201, got {} {:?}",
                put.status, put.body
            );
            let read = get_policy(&emu, read_through).await;
            assert_eq!(
                read.text("resourcePolicy"),
                *reported,
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

#[tokio::test]
async fn a_document_that_is_not_a_resource_policy_is_refused_with_bedrocks_own_sentence() {
    let emu = emulator();
    let arn = guardrail_arn(&emu, "policy-refusal-wording").await;

    // Bedrock's refusal of a non-conforming policy document, word for word: entirely fixed
    // text, so every row below — whatever the document was and whichever part of the
    // grammar it broke — is answered with the SAME sentence, naming neither the member,
    // nor the document, nor the defect.
    let expected = "The provided policy document does not conform to the supported policy \
                    formats. For more information about valid policy document formats, \
                    check out the Amazon Bedrock Guardrails documentation";

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
        let stated = put.text("message");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        // Nothing of the request reaches this sentence: neither the document, nor the
        // member it arrived in, nor the resource it was put on.
        for leaked in [document.as_str(), "resourcePolicy", arn.as_str()] {
            assert!(
                !stated.contains(leaked),
                "case {case:?}: the refusal states nothing of the request, got {stated:?} \
                 carrying {leaked:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 5. A resource with no policy, and a policy whose resource is gone.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_holding_no_policy_reports_the_empty_policy_on_the_read() {
    let emu = emulator();
    // One resource of each type, each holding no policy: the read reports the resource it
    // FOUND, with `resourcePolicy` set to the empty policy document rather than a 404.
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
            read.status, 200,
            "case {case:?}: a resource this account holds is found, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("resourcePolicy"),
            NO_POLICY,
            "case {case:?}: a resource carrying no policy reports the empty policy document, \
             got {:?}",
            read.body
        );
        // The same resource, once a policy IS attached, reports it — so the empty document
        // above is the resource's state and not a read that reports nothing whatever is
        // stored.
        let document = admitted_documents()[0].1.clone();
        put_policy(&emu, &arn, &document).await;
        let attached = get_policy(&emu, &arn).await;
        assert_eq!(
            attached.text("resourcePolicy"),
            document,
            "case {case:?}: the attached policy is reported, got {:?}",
            attached.body
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_is_not_found_on_the_read() {
    let emu = emulator();
    // The rows differ along what the rule generalises over: the ARN's vendor, resource
    // type and resource id, and a value that is no ARN at all.
    let rows = [
        (
            "another vendor's resource",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:endpoint/x"),
        ),
        (
            "a guardrail id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/zzzzzzzzzzzz"),
        ),
        (
            "a Bedrock resource type nothing created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/zzzzzzzzzzzz"),
        ),
        ("text that is no ARN", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: an ARN naming no resource of this account is 404, got {} {:?}",
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

// ---------------------------------------------------------------------------------------
// 6 + 7. `DeleteResourcePolicy` takes the policy away.
// ---------------------------------------------------------------------------------------

/// The sentence the model writes for `ResourceNotFoundException`
/// (`smithy_query(query="doc",shape="ResourceNotFoundException")`), which is what the
/// delete and the read both state for an ARN naming no resource of this account.
const NO_SUCH_RESOURCE: &str = "The specified resource Amazon Resource Name (ARN) was not \
                                found. Check the Amazon Resource Name (ARN) and try your \
                                request again.";

/// Assert one delete was ACCEPTED: `DELETE /resource-policy/{resourceArn}` is modelled
/// `-> 200` with an output carrying no members, so the answer is that status, no error
/// code, and nothing of a policy in the body.
fn assert_accepted_delete(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DeleteResourcePolicy must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error, None,
        "case {case:?}: an accepted delete carries no error code, got {:?}",
        reply.error
    );
    assert_eq!(
        reply.get("resourcePolicy"),
        json!(null),
        "case {case:?}: the delete's output models no members, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn every_resource_type_and_policy_form_is_taken_away_by_the_delete() {
    let emu = emulator();
    // The same dimensions the attach is asserted over: one resource of each type this
    // service mints an ARN for, each in turn carrying every form a policy document is
    // written in. What the delete removes is the resource's policy, whatever document it
    // is and whichever resource holds it.
    let resources = [
        ("guardrail", guardrail_arn(&emu, "deleted-policy-guardrail").await),
        (
            "prompt router",
            prompt_router_arn(&emu, "deleted-policy-router").await,
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "deleted-policy-provisioned").await,
        ),
    ];

    for (resource, arn) in &resources {
        for (form, document, reported) in admitted_documents() {
            let case = format!("{resource} / {form}");
            let put = put_policy(&emu, arn, &document).await;
            assert_eq!(
                put.status, 201,
                "case {case:?}: PutResourcePolicy must answer 201, got {} {:?}",
                put.status, put.body
            );

            let removed = delete_policy(&emu, arn).await;
            assert_accepted_delete(&case, &removed);

            // The resource is still HELD — so it is found — and now governed by no
            // policy, which the read reports as the empty policy document.
            let read = get_policy(&emu, arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the resource is still held after the delete, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("resourcePolicy"),
                NO_POLICY,
                "case {case:?}: the deleted policy is reported no longer, got {:?}",
                read.body
            );

            // And a further put attaches again, so the delete removed the document
            // rather than barring the resource from holding one.
            put_policy(&emu, arn, &document).await;
            let reattached = get_policy(&emu, arn).await;
            assert_eq!(
                reattached.text("resourcePolicy"),
                reported,
                "case {case:?}: a put after the delete attaches again, got {:?}",
                reattached.body
            );
            delete_policy(&emu, arn).await;
        }
    }
}

#[tokio::test]
async fn repeating_the_delete_has_the_effect_of_making_it_once() {
    // The operation is `@idempotent`: a repeat, and a delete on a resource that holds no
    // policy at all, are the same accepted answer as the first delete — the call
    // establishes "this resource is governed by no policy" rather than consuming a
    // document that must be there.
    let emu = emulator();
    let rows = [
        (
            "guardrail",
            guardrail_arn(&emu, "idempotent-delete-guardrail").await,
        ),
        (
            "prompt router",
            prompt_router_arn(&emu, "idempotent-delete-router").await,
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "idempotent-delete-provisioned").await,
        ),
    ];
    let document = admitted_documents()[0].1.clone();

    for (resource, arn) in &rows {
        // A resource that never held a policy: the delete is accepted all the same.
        for attempt in 1..=3 {
            let case = format!("{resource} / never attached / delete #{attempt}");
            let removed = delete_policy(&emu, arn).await;
            assert_accepted_delete(&case, &removed);
            let read = get_policy(&emu, arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the resource is untouched by the delete, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("resourcePolicy"),
                NO_POLICY,
                "case {case:?}: it still holds no policy, got {:?}",
                read.body
            );
        }

        // And a resource that DID hold one: the first delete removes it and every
        // repeat answers exactly as that first one did.
        put_policy(&emu, arn, &document).await;
        for attempt in 1..=3 {
            let case = format!("{resource} / attached / delete #{attempt}");
            let removed = delete_policy(&emu, arn).await;
            assert_accepted_delete(&case, &removed);
            let read = get_policy(&emu, arn).await;
            assert_eq!(
                read.text("resourcePolicy"),
                NO_POLICY,
                "case {case:?}: the policy stays removed, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn the_delete_takes_away_only_the_named_resources_policy() {
    // Each resource holds its OWN policy, so a delete is addressed to one of them: the
    // rows are three resources of different types, each carrying a different document,
    // deleted one at a time.
    let emu = emulator();
    let documents = admitted_documents();
    assert!(
        documents.len() >= 3,
        "case \"the document table\": this test gives each of the three resources a \
         DIFFERENT document, so the table needs at least three rows, got {}",
        documents.len()
    );
    let rows = [
        (
            "guardrail",
            guardrail_arn(&emu, "one-of-many-guardrail").await,
            documents[0].1.clone(),
        ),
        (
            "prompt router",
            prompt_router_arn(&emu, "one-of-many-router").await,
            documents[1].1.clone(),
        ),
        (
            "provisioned throughput",
            provisioned_model_arn(&emu, "one-of-many-provisioned").await,
            documents[2].1.clone(),
        ),
    ];
    for (_, arn, document) in &rows {
        put_policy(&emu, arn, document).await;
    }

    let mut removed_so_far: Vec<&str> = Vec::new();
    for (case, arn, _) in &rows {
        let removed = delete_policy(&emu, arn).await;
        assert_accepted_delete(case, &removed);
        removed_so_far.push(arn);

        for (other, other_arn, other_document) in &rows {
            let read = get_policy(&emu, other_arn).await;
            let expected = if removed_so_far.contains(&other_arn.as_str()) {
                json!(NO_POLICY)
            } else {
                json!(other_document)
            };
            assert_eq!(
                read.get("resourcePolicy"),
                expected,
                "case {case:?} deleted / {other:?} read: expected {expected:?}, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_delete_under_either_arn_spelling_removes_the_one_policy() {
    // A policy is attached to the RESOURCE, not to one spelling of its ARN, so a delete
    // under either spelling of a prompt router's ARN removes the policy the other
    // spelling reports — the same two spellings this service's ARN resolution reads.
    let emu = emulator();
    let router = prompt_router_arn(&emu, "delete-two-spellings").await;
    let other_spelling = router.replace(":default-prompt-router/", ":prompt-router/");
    let document = admitted_documents()[0].1.clone();

    for (case, written, deleted_through, read_through) in [
        (
            "put under the ARN, deleted under the alias",
            &router,
            &other_spelling,
            &router,
        ),
        (
            "put under the alias, deleted under the ARN",
            &other_spelling,
            &router,
            &other_spelling,
        ),
    ] {
        put_policy(&emu, written, &document).await;
        let read = get_policy(&emu, read_through).await;
        assert_eq!(
            read.text("resourcePolicy"),
            document,
            "case {case:?}: the policy is attached before the delete, got {:?}",
            read.body
        );

        let removed = delete_policy(&emu, deleted_through).await;
        assert_accepted_delete(case, &removed);

        let read = get_policy(&emu, read_through).await;
        assert_eq!(
            read.text("resourcePolicy"),
            NO_POLICY,
            "case {case:?}: both spellings address the one policy, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_is_not_found_on_the_delete() {
    let emu = emulator();
    let held = guardrail_arn(&emu, "held-for-delete").await;
    let document = admitted_documents()[0].1.clone();
    put_policy(&emu, &held, &document).await;
    let router = prompt_router_arn(&emu, "held-router-for-delete").await;
    let router_id = router
        .rsplit_once(":default-prompt-router/")
        .expect("a prompt router ARN carries its id")
        .1
        .to_string();

    // The rows differ along what the rule generalises over: the ARN's vendor, region,
    // account, resource type and resource id, plus a value that is no ARN at all.
    let rows = [
        (
            "another vendor's resource",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:endpoint/x"),
        ),
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
        ("text that is no ARN", "not-an-arn".to_string()),
    ];

    for (case, resource_arn) in rows {
        let removed = delete_policy(&emu, &resource_arn).await;
        assert_eq!(
            removed.status, 404,
            "case {case:?}: an ARN naming no resource of this account is 404, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal is the modelled ResourceNotFoundException, got {:?}",
            removed.error
        );
        let stated = removed.text("message");
        assert_eq!(
            stated, NO_SUCH_RESOURCE,
            "case {case:?}: expected {NO_SUCH_RESOURCE:?}, got {stated:?}"
        );
    }

    // None of those refusals touched the policy the HELD resource carries.
    let read = get_policy(&emu, &held).await;
    assert_eq!(
        read.text("resourcePolicy"),
        document,
        "case \"held guardrail\": a refused delete removes nothing, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_resource_that_was_deleted_is_not_found_on_the_delete() {
    // A resource that is gone holds no policy to remove, and it is no resource of this
    // account either: the delete is the same 404 the read answers for it.
    let emu = emulator();
    let deleted = guardrail_arn(&emu, "gone-before-the-delete").await;
    let kept = guardrail_arn(&emu, "kept-across-the-delete").await;
    let document = admitted_documents()[0].1.clone();
    put_policy(&emu, &deleted, &document).await;
    put_policy(&emu, &kept, &document).await;

    let removed = emu.delete(&deleted, None).await;
    assert_eq!(
        removed.status, 202,
        "case \"delete the guardrail\": DeleteGuardrail must answer 202, got {} {:?}",
        removed.status, removed.body
    );

    let policy_delete = delete_policy(&emu, &deleted).await;
    assert_eq!(
        policy_delete.status, 404,
        "case \"deleted guardrail\": its policy delete is 404, got {} {:?}",
        policy_delete.status, policy_delete.body
    );
    assert_eq!(
        policy_delete.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"deleted guardrail\": the refusal is the modelled ResourceNotFoundException, \
         got {:?}",
        policy_delete.error
    );

    // The resource that remains keeps its own document, and its delete is accepted.
    let read = get_policy(&emu, &kept).await;
    assert_eq!(
        read.text("resourcePolicy"),
        document,
        "case \"kept guardrail\": the other resource keeps its policy, got {:?}",
        read.body
    );
    let accepted = delete_policy(&emu, &kept).await;
    assert_accepted_delete("kept guardrail", &accepted);
}
