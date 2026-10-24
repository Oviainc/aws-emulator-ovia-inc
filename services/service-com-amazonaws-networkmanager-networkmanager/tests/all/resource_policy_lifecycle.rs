// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the resource-policy slice — `PutResourcePolicy`,
//! `GetResourcePolicy` and `DeleteResourcePolicy` — driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The first rule under test: a resource policy is attached only to a resource that may
//! CARRY one, which is a resource of ANOTHER service — the Direct Connect gateway
//! `CreateDirectConnectGatewayAttachment` joins to a core network, whose ARN the model
//! types as `DirectConnectGatewayArn` (`smithy_query shape=DirectConnectGatewayArn`). A
//! `PutResourcePolicy` naming any other resource — every resource Network Manager itself
//! owns included, a core network among them — is refused with `ValidationException`
//! (HTTP 400) naming `ResourceArn` and stores nothing, while `GetResourcePolicy` keeps
//! reporting no policy for it and `DeleteResourcePolicy` stays a no-op.
//!
//! The second rule: for the resources that may carry one, the policy is a per-ARN store.
//! `PutResourcePolicy` stores, under the ARN its `ResourceArn` names, the document it
//! accepted — verbatim, replacing whatever that ARN held; `GetResourcePolicy` answers
//! exactly that stored document and no `PolicyDocument` member at all while the ARN holds
//! none; `DeleteResourcePolicy` removes it, and both reads and deletes of one ARN leave
//! every other ARN alone. A document outside the IAM JSON policy grammar (it must parse as
//! JSON, be an object, carry at least one statement, and every statement must name an
//! `Effect` of `Allow` or `Deny`) and a string that names no resource these operations
//! address at all are refused with `ValidationException` (HTTP 400) and store nothing. The
//! store's own bounds are pinned too: a document is accepted at the documented size bound
//! and refused one character past it, and a policy for one ARN more than the account quota
//! allows is refused with `ServiceQuotaExceededException` (HTTP 402) while a replacement
//! for an ARN already held is not.
//!
//! The tables below span those classes: documents differing in how `Statement` is written
//! (one object, a list, several statements), in which elements the statements carry, in
//! whitespace, in alphabet and in size; attachable ARNs differing in partition, in owning
//! account and in gateway id; refused ARNs differing in which resource kind of this
//! service they name, whether that resource is stored, and which part of the attachable
//! ARN form they break.
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

/// Assert `arn` was refused by `PutResourcePolicy` with a `ValidationException` naming
/// `ResourceArn`, and answer the body so a caller can look at the rest of it.
async fn assert_put_refused(emulator: &Emulator, case: &str, arn: &str, document: &str) -> Value {
    let (status, body) = put_policy(emulator, arn, document).await;
    assert_eq!(
        status, 400,
        "case {case}: PutResourcePolicy expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some("ResourceArn"),
        "case {case}: expected the Fields entry to name ResourceArn, got {body}"
    );
    body
}

/// A Direct Connect gateway ARN: the form the model types as `DirectConnectGatewayArn`,
/// `arn:<partition>:directconnect::<account>:dx-gateway/<id>` with an EMPTY region segment
/// (`smithy_query shape=DirectConnectGatewayArn`).
fn gateway_arn(partition: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:directconnect::{account}:dx-gateway/{id}")
}

/// The `index`th distinct gateway id in the 8-4-4-4-12 lowercase-hex form that same
/// pattern spells out.
fn gateway_id(index: usize) -> String {
    format!("{index:08x}-1a2b-3c4d-5e6f-{index:012x}")
}

/// Create a core network (in its own global network) and answer its ARN — a resource this
/// service owns, so one that cannot carry a resource policy.
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

/// The ARNs the policy table runs over: resources that may carry a policy, differing in
/// partition, in which account owns the gateway (the point of a resource policy is that
/// the resource is another account's) and in gateway id.
fn policy_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a gateway of this account",
            gateway_arn("aws", engine::emulator_account_id(), &gateway_id(1)),
        ),
        (
            "a gateway of another account",
            gateway_arn("aws", "222222222222", &gateway_id(2)),
        ),
        (
            "a gateway in another partition",
            gateway_arn("aws-cn", "333333333333", &gateway_id(3)),
        ),
        // The modelled pattern writes the account segment `[^:]{0,63}`, so it may be empty.
        (
            "a gateway ARN carrying no account",
            gateway_arn("aws", "", &gateway_id(4)),
        ),
    ]
}

/// One policy-attachable ARN, for the tests that need a single subject.
fn one_policy_arn() -> String {
    policy_arns()
        .into_iter()
        .next()
        .expect("the ARN table is not empty")
        .1
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
                "Action": "networkmanager:CreateDirectConnectGatewayAttachment",
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
            "a single statement granting attachment creation",
            compact(&json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowAttachments",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::222222222222:root" },
                    "Action": "networkmanager:CreateDirectConnectGatewayAttachment",
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
                        "Action": ["networkmanager:CreateDirectConnectGatewayAttachment"],
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

/// The first document of the table, for the tests that need just one.
fn one_document() -> String {
    accepted_documents()
        .into_iter()
        .next()
        .expect("the document table is not empty")
        .1
}

#[tokio::test]
async fn a_put_policy_is_reported_back_verbatim_for_every_arn_and_document_form() {
    // The cross product of the two tables: every document form against every ARN form,
    // each document in its own store so what is reported can only come from its own put.
    for (document_case, document) in accepted_documents() {
        let emulator = Emulator::new();
        for (arn_case, arn) in policy_arns() {
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
    let arns = policy_arns();
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
    let arn = one_policy_arn();

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
    let arns = policy_arns();
    let document = one_document();

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

#[tokio::test]
async fn a_resource_that_cannot_carry_a_policy_is_refused_and_keeps_reporting_none() {
    let emulator = Emulator::new();
    let document = one_document();

    // Resources of THIS service, which carry no resource policy: the kinds it mints, both
    // as stored resources and as well-formed ARNs naming nothing (none of the three
    // operations models a not-found failure, so storage is not what decides this).
    let core_network = create_core_network_arn(&emulator).await;
    let global_network = emulator.create_global_network("carries no policy").await;
    let site = emulator.create_site(&global_network, "carries no policy").await;
    let refused: Vec<(String, String)> = vec![
        ("a stored core network".to_string(), core_network),
        (
            "a stored global network".to_string(),
            expected_arn("global-network", &global_network),
        ),
        ("a stored site".to_string(), expected_arn("site", &site)),
        (
            "a device ARN naming nothing stored".to_string(),
            expected_arn("device", "device-0000000000000beef"),
        ),
        (
            "a link ARN naming nothing stored".to_string(),
            expected_arn("link", "link-0000000000000beef"),
        ),
        (
            "an attachment ARN naming nothing stored".to_string(),
            expected_arn("attachment", "attachment-0000000000000beef"),
        ),
        (
            "a core network ARN naming nothing stored".to_string(),
            expected_arn("core-network", "core-network-0000000000000beef"),
        ),
    ];

    // The refusal wording is the service's own constant, so it must not vary by resource.
    let mut refusal_message: Option<String> = None;
    for (case, arn) in &refused {
        // The put is refused, and the reporting sibling still answers no policy: the
        // refusal stored nothing.
        let body = assert_put_refused(&emulator, case, arn, &document).await;
        assert_eq!(
            body["Reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case}: expected Reason FieldValidationFailed, got {body}"
        );
        // The wording carries no per-request detail: the offending member travels in
        // `Fields`, so the message is the same whichever resource was named.
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Message in {body}"))
            .to_string();
        assert!(
            !message.contains(arn.as_str()),
            "case {case}: expected the message to carry no per-request detail, got {message:?}"
        );
        match &refusal_message {
            Some(first) => assert_eq!(
                &message, first,
                "case {case}: expected every refusal to carry the same message {first:?}, got {message:?}"
            ),
            None => refusal_message = Some(message),
        }
        assert_no_policy(&emulator, case, arn).await;

        // Deleting the policy of a resource that holds none is a no-op, not a failure.
        delete_policy_ok(&emulator, case, arn).await;
        assert_no_policy(&emulator, &format!("{case} after delete"), arn).await;

        // Refusing one resource does not stop a policy-attachable one from being stored.
        let attachable = one_policy_arn();
        put_policy_ok(&emulator, case, &attachable, &document).await;
        assert_policy_is(&emulator, case, &attachable, &document).await;
        delete_policy_ok(&emulator, case, &attachable).await;
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
    let arn = one_policy_arn();

    // The ARN already carries a policy, so a refusal must leave that one in place.
    let held = one_document();
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
async fn a_string_that_names_no_resource_is_refused_by_all_three_operations() {
    let emulator = Emulator::new();
    let document = one_document();

    // Rows differ along which part of an ARN form is missing or wrong — of this service's
    // ARNs, and of the attachable gateway ARN the model types.
    let bad_arns: Vec<(&str, String)> = vec![
        ("an empty-looking plain name", "not-an-arn".to_string()),
        (
            "an ARN of a service neither this one nor the gateway's",
            format!(
                "arn:aws:ec2:us-west-2:{}:vpc/vpc-1234",
                engine::emulator_account_id()
            ),
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
        (
            "a gateway ARN whose id is not a gateway id",
            gateway_arn("aws", "222222222222", "not-a-gateway-id"),
        ),
        (
            "a gateway ARN whose id carries upper-case hex",
            gateway_arn("aws", "222222222222", &gateway_id(5).to_uppercase()),
        ),
        (
            "a gateway ARN whose id is one group short",
            gateway_arn("aws", "222222222222", "00000006-1a2b-3c4d-5e6f"),
        ),
        (
            "a gateway ARN carrying a region",
            "arn:aws:directconnect:us-west-2:222222222222:dx-gateway/00000007-1a2b-3c4d-5e6f-000000000007".to_string(),
        ),
        (
            "a gateway ARN with no partition",
            gateway_arn("", "222222222222", &gateway_id(8)),
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
    let arn = one_policy_arn();

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
    let document = one_document();

    // Policy-attachable ARNs, each naming a different gateway.
    let arn_of = |index: usize| gateway_arn("aws", "222222222222", &gateway_id(index));

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
