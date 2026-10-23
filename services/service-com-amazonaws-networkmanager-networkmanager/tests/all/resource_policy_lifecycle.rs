// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the resource-policy slice — `PutResourcePolicy`,
//! `GetResourcePolicy` and `DeleteResourcePolicy` — driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the resource policy of a resource is a per-ARN store.
//! `PutResourcePolicy` stores, under the ARN its `ResourceArn` names, the document it
//! accepted — verbatim, replacing whatever that ARN held; `GetResourcePolicy` answers
//! exactly that stored document and no `PolicyDocument` member at all while the ARN holds
//! none; `DeleteResourcePolicy` removes it, and both reads and deletes of one ARN leave
//! every other ARN alone. A document outside the IAM JSON policy grammar (it must parse as
//! JSON, be an object, carry at least one statement, and every statement must name an
//! `Effect` of `Allow` or `Deny`) and a string that is not an ARN of this service are
//! refused with `ValidationException` (HTTP 400) and store nothing. The store's own
//! bounds are pinned too: a document is accepted at the documented size bound and refused
//! one character past it, and a policy for one ARN more than the account quota allows is
//! refused with `ServiceQuotaExceededException` (HTTP 402) while a replacement for an ARN
//! already held is not.
//!
//! The tables below span that class: documents differing in how `Statement` is written
//! (one object, a list, several statements), in which elements the statements carry, in
//! whitespace, in alphabet and in size; and ARNs differing in which resource kind they
//! name, including a well-formed ARN naming a resource this store does not hold — none of
//! the three operations models a `ResourceNotFoundException`, so the policy store is keyed
//! by ARN alone.
//!
//! Nothing here reaches into `crate::state`: a policy is only ever read back through
//! `GetResourcePolicy`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_arn, Emulator};
use crate::resource_tagging::escape;

/// The `/resource-policy/{ResourceArn}` URI of `arn`.
///
/// The ARN travels as a NON-greedy `@httpLabel` (`smithy_query query="operation"
/// operation="GetResourcePolicy"`), so a client sends its `:` and `/` percent-encoded.
fn policy_uri(arn: &str) -> String {
    format!("/resource-policy/{}", escape(arn))
}

/// `PutResourcePolicy`: attach `document` to `arn`.
async fn put_policy(emulator: &Emulator, arn: &str, document: &str) -> (u16, Value) {
    emulator
        .post(&policy_uri(arn), json!({ "PolicyDocument": document }))
        .await
}

/// `PutResourcePolicy`, asserting it was accepted.
async fn put_policy_ok(emulator: &Emulator, case: &str, arn: &str, document: &str) {
    let (status, body) = put_policy(emulator, arn, document).await;
    assert_eq!(
        status, 200,
        "case {case}: PutResourcePolicy expected 200, got {status} with {body}"
    );
}

/// `GetResourcePolicy`: the status and the `PolicyDocument` member, which is absent
/// (`Value::Null`) while the ARN holds no policy.
async fn get_policy(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let (status, body) = emulator.get(&policy_uri(arn)).await;
    (status, body["PolicyDocument"].clone())
}

/// Assert `arn` reports exactly `document`.
async fn assert_policy_is(emulator: &Emulator, case: &str, arn: &str, document: &str) {
    let (status, reported) = get_policy(emulator, arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetResourcePolicy expected 200, got {status} with {reported}"
    );
    assert_eq!(
        reported,
        json!(document),
        "case {case}: expected the stored document {document:?}, got {reported}"
    );
}

/// Assert `arn` reports no policy at all.
async fn assert_no_policy(emulator: &Emulator, case: &str, arn: &str) {
    let (status, reported) = get_policy(emulator, arn).await;
    assert_eq!(
        status, 200,
        "case {case}: GetResourcePolicy expected 200, got {status} with {reported}"
    );
    assert!(
        reported.is_null(),
        "case {case}: expected no PolicyDocument member, got {reported}"
    );
}

/// `DeleteResourcePolicy`, asserting it was accepted.
async fn delete_policy_ok(emulator: &Emulator, case: &str, arn: &str) {
    let (status, body) = emulator.delete(&policy_uri(arn)).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteResourcePolicy expected 200, got {status} with {body}"
    );
}

/// Create a core network (in its own global network) and answer its ARN — the resource a
/// Cloud WAN resource policy is normally attached to.
async fn create_core_network_arn(emulator: &Emulator) -> String {
    let network = emulator.create_global_network("owns a core network").await;
    let (status, created) = emulator
        .post("/core-networks", json!({ "GlobalNetworkId": network }))
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    created["CoreNetwork"]["CoreNetworkArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkArn in {created}"))
        .to_string()
}

/// The ARNs the policy table runs over: one per resource kind this store holds, plus a
/// well-formed ARN of this service naming nothing stored.
async fn policy_arns(emulator: &Emulator) -> Vec<(&'static str, String)> {
    let core_network = create_core_network_arn(emulator).await;
    let network = emulator.create_global_network("carries a policy").await;
    let site = emulator.create_site(&network, "carries a policy").await;
    vec![
        ("a core network's ARN", core_network),
        ("a global network's ARN", expected_arn("global-network", &network)),
        ("a site's ARN", expected_arn("site", &site)),
        // No resource-policy operation models a not-found failure, so an ARN of this
        // service that names no stored resource still carries a policy of its own.
        (
            "an ARN naming no stored resource",
            expected_arn("core-network", "core-network-0000000000000beef"),
        ),
    ]
}

/// A JSON policy document rendered as the string the member carries, so a test can assert
/// the exact text comes back.
fn compact(document: &Value) -> String {
    serde_json::to_string(document).expect("the policy document must serialize")
}

/// A document of `statements` statements, for the size row of the table.
fn many_statements(statements: usize) -> String {
    let listed: Vec<Value> = (0..statements)
        .map(|index| {
            json!({
                "Sid": format!("statement{index}"),
                "Effect": if index % 2 == 0 { "Allow" } else { "Deny" },
                "Principal": { "AWS": format!("arn:aws:iam::{:012}:root", index) },
                "Action": "networkmanager:CreateVpcAttachment",
                "Resource": "*",
            })
        })
        .collect();
    compact(&json!({ "Version": "2012-10-17", "Statement": listed }))
}

/// The accepted-document table: one row per form a resource policy document takes. The
/// element names (`Version`, `Id`, `Statement`, `Sid`, `Effect`, `Principal`, `Action`,
/// `Resource`, `Condition`) are the IAM JSON policy grammar's (AWS IAM User Guide, "IAM
/// JSON policy element reference"), which is also where the rules the handler enforces
/// come from.
fn accepted_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "a single statement granting attachment acceptance",
            compact(&json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowAttachments",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::222222222222:root" },
                    "Action": "networkmanager:CreateVpcAttachment",
                    "Resource": "*",
                }],
            })),
        ),
        (
            "Statement written as one object rather than a list",
            compact(&json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "networkmanager:*",
                    "Resource": "*",
                },
            })),
        ),
        (
            "two statements, one of them a Deny carrying a Condition",
            compact(&json!({
                "Version": "2012-10-17",
                "Id": "cloud-wan-policy",
                "Statement": [
                    {
                        "Sid": "AllowOne",
                        "Effect": "Allow",
                        "Principal": { "AWS": ["arn:aws:iam::222222222222:root"] },
                        "Action": ["networkmanager:CreateVpcAttachment"],
                        "Resource": "*",
                    },
                    {
                        "Sid": "DenyOther",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "networkmanager:DeleteAttachment",
                        "Resource": "*",
                        "Condition": { "StringNotEquals": { "aws:PrincipalOrgID": "o-abcdefghij" } },
                    },
                ],
            })),
        ),
        (
            "a statement carrying nothing but its Effect",
            compact(&json!({ "Statement": [{ "Effect": "Deny" }] })),
        ),
        (
            "a pretty-printed document, whose whitespace must survive the round trip",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \"Effect\": \"Allow\",\n      \"Principal\": \"*\",\n      \"Action\": \"networkmanager:*\",\n      \"Resource\": \"*\"\n    }\n  ]\n}\n"
                .to_string(),
        ),
        (
            "a document whose Sid and Condition value carry non-ASCII text",
            compact(&json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "разрешение",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "networkmanager:*",
                    "Resource": "*",
                    "Condition": { "StringEquals": { "aws:ResourceTag/команда": "сеть" } },
                }],
            })),
        ),
        ("a document of two statements", many_statements(2)),
        ("a document of fifty statements", many_statements(50)),
    ]
}

#[tokio::test]
async fn a_put_policy_is_reported_back_verbatim_for_every_arn_and_document_form() {
    // The cross product of the two tables: every document form against every ARN form,
    // each document in its own store so what is reported can only come from its own put.
    for (document_case, document) in accepted_documents() {
        let emulator = Emulator::new();
        for (arn_case, arn) in policy_arns(&emulator).await {
            let case = format!("{document_case} on {arn_case}");

            assert_no_policy(&emulator, &case, &arn).await;
            put_policy_ok(&emulator, &case, &arn, &document).await;
            assert_policy_is(&emulator, &case, &arn, &document).await;
        }
    }
}

#[tokio::test]
async fn every_arn_reports_only_its_own_policy_and_a_delete_removes_only_that_one() {
    let emulator = Emulator::new();
    let arns = policy_arns(&emulator).await;
    let documents = accepted_documents();

    // Every ARN carries a different document at the same time.
    let rows: Vec<(String, String, String)> = arns
        .iter()
        .zip(documents.iter())
        .map(|((arn_case, arn), (document_case, document))| {
            (
                format!("{arn_case} carrying {document_case}"),
                arn.clone(),
                document.clone(),
            )
        })
        .collect();
    for (case, arn, document) in &rows {
        put_policy_ok(&emulator, case, arn, document).await;
    }
    for (case, arn, document) in &rows {
        assert_policy_is(&emulator, case, arn, document).await;
    }

    // Deleting one ARN's policy leaves every other ARN's exactly as it was.
    let mut deleted: Vec<&String> = Vec::new();
    for (deleted_case, deleted_arn, _) in &rows {
        delete_policy_ok(&emulator, deleted_case, deleted_arn).await;
        deleted.push(deleted_arn);

        for (case, arn, document) in &rows {
            if deleted.contains(&arn) {
                assert_no_policy(&emulator, case, arn).await;
            } else {
                assert_policy_is(&emulator, case, arn, document).await;
            }
        }
    }
}

#[tokio::test]
async fn putting_again_replaces_the_document_the_arn_reports() {
    let emulator = Emulator::new();
    let arn = create_core_network_arn(&emulator).await;

    // Every document form in turn, so a replacement is exercised from and to each of them.
    let mut previous: Option<String> = None;
    for (case, document) in accepted_documents() {
        put_policy_ok(&emulator, case, &arn, &document).await;
        assert_policy_is(&emulator, case, &arn, &document).await;
        if let Some(previous) = previous {
            assert_ne!(
                previous, document,
                "case {case}: the table must change the document between rows"
            );
        }
        previous = Some(document);
    }

    // Putting the same document twice is stable, not additive.
    let (_, document) = accepted_documents()
        .pop()
        .expect("the document table is not empty");
    put_policy_ok(&emulator, "repeat", &arn, &document).await;
    put_policy_ok(&emulator, "repeat", &arn, &document).await;
    assert_policy_is(&emulator, "repeat", &arn, &document).await;
}

#[tokio::test]
async fn reading_a_policy_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let arns = policy_arns(&emulator).await;
    let (_, document) = accepted_documents()
        .into_iter()
        .next()
        .expect("the document table is not empty");

    for (case, arn) in &arns {
        // Reading an ARN that holds no policy may not create one, and may not stop a
        // later put from being stored.
        for attempt in 1..=3u32 {
            assert_no_policy(&emulator, &format!("{case} empty read {attempt}"), arn).await;
        }
        put_policy_ok(&emulator, case, arn, &document).await;
        for attempt in 1..=3u32 {
            assert_policy_is(&emulator, &format!("{case} read {attempt}"), arn, &document).await;
        }
    }

    // Deleting is idempotent: a delete of an ARN that holds no policy succeeds and leaves
    // the store as it was.
    for (case, arn) in &arns {
        delete_policy_ok(&emulator, case, arn).await;
        for attempt in 1..=3u32 {
            delete_policy_ok(&emulator, &format!("{case} repeat delete {attempt}"), arn).await;
            assert_no_policy(&emulator, &format!("{case} after delete {attempt}"), arn).await;
        }
        // A put after a delete is stored again.
        put_policy_ok(&emulator, &format!("{case} re-put"), arn, &document).await;
        assert_policy_is(&emulator, &format!("{case} re-put"), arn, &document).await;
    }
}

/// One row of the refused-document table: the document, and the `Reason` the refusal
/// carries — `CannotParse` when the text is not JSON at all, `FieldValidationFailed` when
/// it parses but is not a policy document.
struct BadDocument {
    case: &'static str,
    document: String,
    reason: &'static str,
}

/// The refused-document table. The rules are the IAM JSON policy grammar's: a policy is a
/// JSON object carrying at least one statement, and every statement names an `Effect` of
/// `Allow` or `Deny`.
fn refused_documents() -> Vec<BadDocument> {
    let rows: Vec<(&'static str, String, &'static str)> = vec![
        ("not JSON at all", "this is not a policy".to_string(), "CannotParse"),
        ("a truncated JSON object", "{\"Statement\":".to_string(), "CannotParse"),
        (
            "a JSON array at the root",
            compact(&json!([{ "Effect": "Allow" }])),
            "FieldValidationFailed",
        ),
        (
            "a JSON string at the root",
            compact(&json!("Allow")),
            "FieldValidationFailed",
        ),
        (
            "a JSON number at the root",
            compact(&json!(17)),
            "FieldValidationFailed",
        ),
        (
            "an object carrying no Statement",
            compact(&json!({ "Version": "2012-10-17" })),
            "FieldValidationFailed",
        ),
        (
            "a Statement that is a string",
            compact(&json!({ "Statement": "Allow" })),
            "FieldValidationFailed",
        ),
        (
            "an empty Statement list",
            compact(&json!({ "Statement": [] })),
            "FieldValidationFailed",
        ),
        (
            "a Statement list holding a string",
            compact(&json!({ "Statement": ["Allow"] })),
            "FieldValidationFailed",
        ),
        (
            "a statement naming no Effect",
            compact(&json!({ "Statement": [{ "Action": "networkmanager:*" }] })),
            "FieldValidationFailed",
        ),
        (
            "a statement whose Effect is lower case",
            compact(&json!({ "Statement": [{ "Effect": "allow" }] })),
            "FieldValidationFailed",
        ),
        (
            "a statement whose Effect is neither Allow nor Deny",
            compact(&json!({ "Statement": [{ "Effect": "Permit" }] })),
            "FieldValidationFailed",
        ),
        (
            "a statement whose Effect is not a string",
            compact(&json!({ "Statement": [{ "Effect": true }] })),
            "FieldValidationFailed",
        ),
        (
            "a second statement that is not an object",
            compact(&json!({ "Statement": [{ "Effect": "Allow" }, 17] })),
            "FieldValidationFailed",
        ),
        (
            "a second statement whose Effect is missing",
            compact(&json!({ "Statement": [{ "Effect": "Allow" }, { "Sid": "no-effect" }] })),
            "FieldValidationFailed",
        ),
    ];
    rows.into_iter()
        .map(|(case, document, reason)| BadDocument {
            case,
            document,
            reason,
        })
        .collect()
}

#[tokio::test]
async fn a_document_outside_the_policy_grammar_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let arn = create_core_network_arn(&emulator).await;

    // The ARN already carries a policy, so a refusal must leave that one in place.
    let (_, held) = accepted_documents()
        .into_iter()
        .next()
        .expect("the document table is not empty");
    put_policy_ok(&emulator, "the held policy", &arn, &held).await;

    for row in refused_documents() {
        let BadDocument {
            case,
            document,
            reason,
        } = row;
        let (status, body) = put_policy(&emulator, &arn, &document).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some(reason),
            "case {case}: expected Reason {reason:?}, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("PolicyDocument"),
            "case {case}: expected the Fields entry to name PolicyDocument, got {body}"
        );
        assert_policy_is(&emulator, case, &arn, &held).await;
    }
}

#[tokio::test]
async fn a_string_that_is_not_an_arn_of_this_service_is_refused_by_all_three_operations() {
    let emulator = Emulator::new();
    let (_, document) = accepted_documents()
        .into_iter()
        .next()
        .expect("the document table is not empty");

    // Rows differ along which part of the ARN form is missing or wrong.
    let bad_arns: Vec<(&str, String)> = vec![
        ("an empty-looking plain name", "not-an-arn".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:us-west-2:{}:vpc/vpc-1234", engine::emulator_account_id()),
        ),
        (
            "an ARN with no resource segment",
            format!("arn:aws:networkmanager::{}:", engine::emulator_account_id()),
        ),
        (
            "an ARN whose resource carries no id",
            format!(
                "arn:aws:networkmanager::{}:core-network/",
                engine::emulator_account_id()
            ),
        ),
        (
            "an ARN with no partition",
            format!(
                "arn::networkmanager::{}:core-network/core-network-0000000000000beef",
                engine::emulator_account_id()
            ),
        ),
        (
            "an ARN with no account",
            "arn:aws:networkmanager:::core-network/core-network-0000000000000beef".to_string(),
        ),
    ];

    for (case, arn) in bad_arns {
        for (operation, answer) in [
            ("PutResourcePolicy", put_policy(&emulator, &arn, &document).await),
            ("GetResourcePolicy", emulator.get(&policy_uri(&arn)).await),
            ("DeleteResourcePolicy", emulator.delete(&policy_uri(&arn)).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 400,
                "case {case} on {operation}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} on {operation}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"].as_str(),
                Some("ResourceArn"),
                "case {case} on {operation}: expected the Fields entry to name ResourceArn, got {body}"
            );
        }
    }
}

/// The bounds the store enforces, mirrored from `crate::state`: at most
/// `MAX_RESOURCE_POLICIES` ARNs hold a policy and a document is at most
/// `MAX_RESOURCE_POLICY_CHARS` characters (both documented there as CHOSEN, the model
/// constraining the document only to 0..10000000).
const MAX_RESOURCE_POLICIES: usize = 100;
/// See [`MAX_RESOURCE_POLICIES`].
const MAX_RESOURCE_POLICY_CHARS: usize = 20_480;

/// A valid policy document of exactly `total` characters, padded in its `Sid`.
fn document_of_chars(total: usize) -> String {
    let skeleton = compact(&json!({ "Statement": [{ "Effect": "Allow", "Sid": "" }] }));
    let padding = total.saturating_sub(skeleton.chars().count());
    let document = compact(&json!({
        "Statement": [{ "Effect": "Allow", "Sid": "a".repeat(padding) }],
    }));
    assert_eq!(
        document.chars().count(),
        total,
        "case document_of_chars({total}): the padded document must have the asked-for length"
    );
    document
}

#[tokio::test]
async fn a_document_is_accepted_at_the_size_bound_and_refused_past_it() {
    let emulator = Emulator::new();
    let arn = create_core_network_arn(&emulator).await;

    // Each published bound, and one past it.
    let rows: Vec<(String, usize, bool)> = vec![
        ("far inside the bound".to_string(), 64, true),
        ("one character inside the bound".to_string(), MAX_RESOURCE_POLICY_CHARS - 1, true),
        ("exactly at the bound".to_string(), MAX_RESOURCE_POLICY_CHARS, true),
        ("one character past the bound".to_string(), MAX_RESOURCE_POLICY_CHARS + 1, false),
        ("far past the bound".to_string(), MAX_RESOURCE_POLICY_CHARS * 4, false),
    ];

    let mut held: Option<String> = None;
    for (case, total, accepted) in rows {
        let document = document_of_chars(total);
        let (status, body) = put_policy(&emulator, &arn, &document).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case} ({total} characters): expected 200, got {status} with {body}"
            );
            assert_policy_is(&emulator, &case, &arn, &document).await;
            held = Some(document);
        } else {
            assert_eq!(
                status, 400,
                "case {case} ({total} characters): expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"].as_str(),
                Some("PolicyDocument"),
                "case {case}: expected the Fields entry to name PolicyDocument, got {body}"
            );
            // The refusal stored nothing: the last accepted document still stands.
            match &held {
                Some(document) => assert_policy_is(&emulator, &case, &arn, document).await,
                None => assert_no_policy(&emulator, &case, &arn).await,
            }
        }
    }
}

#[tokio::test]
async fn the_policy_quota_refuses_one_more_arn_and_stores_nothing() {
    let emulator = Emulator::new();
    let (_, document) = accepted_documents()
        .into_iter()
        .next()
        .expect("the document table is not empty");

    // Well-formed ARNs of this service, each naming a different resource.
    let arn_of = |index: usize| expected_arn("core-network", &format!("core-network-{index:017x}"));

    for index in 0..MAX_RESOURCE_POLICIES {
        put_policy_ok(&emulator, &format!("policy {index}"), &arn_of(index), &document).await;
    }

    // Replacing the policy of an ARN already held never trips the quota, however full the
    // store is.
    for index in [0, MAX_RESOURCE_POLICIES / 2, MAX_RESOURCE_POLICIES - 1] {
        let replacement = document_of_chars(128 + index);
        put_policy_ok(&emulator, &format!("replace {index}"), &arn_of(index), &replacement).await;
        assert_policy_is(&emulator, &format!("replace {index}"), &arn_of(index), &replacement).await;
    }

    // One ARN past the quota is refused, and stores nothing.
    let past = arn_of(MAX_RESOURCE_POLICIES);
    let (status, body) = put_policy(&emulator, &past, &document).await;
    assert_eq!(
        status, 402,
        "one ARN past the quota: expected 402, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "one ARN past the quota: expected ServiceQuotaExceededException, got {body}"
    );
    assert_eq!(
        body["LimitCode"],
        json!("resource-policies-per-account"),
        "the refusal must name the quota it enforces, got {body}"
    );
    assert_eq!(
        body["ServiceCode"],
        json!("networkmanager"),
        "the refusal must name this service, got {body}"
    );
    assert_no_policy(&emulator, "one ARN past the quota", &past).await;

    // Freeing a slot lets the refused ARN in.
    delete_policy_ok(&emulator, "freeing a slot", &arn_of(0)).await;
    put_policy_ok(&emulator, "after freeing a slot", &past, &document).await;
    assert_policy_is(&emulator, "after freeing a slot", &past, &document).await;
}
