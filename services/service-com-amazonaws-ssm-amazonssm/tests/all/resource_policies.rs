// SPDX-License-Identifier: CC0-1.0

//! Systems Manager resource policies end to end: `PutResourcePolicy` attaches and
//! rewrites, `GetResourcePolicies` reports and `DeleteResourcePolicy` detaches.
//!
//! The rules under test:
//!
//! * **a policy is attached to the resource its `ResourceArn` names, under a `PolicyId`
//!   the service mints, and a read of that resource reports the document that was
//!   persisted with the `PolicyHash` the write answered;**
//! * **the hash is the document's own digest, so the same document always hashes to the
//!   same value and a document changed in any way hashes to a different one;**
//! * **a rewrite proceeds only against the policy version the caller last read: it takes
//!   both the `PolicyId` and the stored `PolicyHash`, an obsolete hash is refused
//!   `ResourcePolicyConflictException`, an unknown id `ResourcePolicyNotFoundException`
//!   and one of the pair without the other `ResourcePolicyInvalidParameterException` —
//!   and a refused call changes nothing;**
//! * **a delete addresses a policy by its id AND its hash together, so a pair naming no
//!   stored policy — an unknown id, or a hash the policy no longer carries — removes
//!   nothing and is refused `ResourcePolicyNotFoundException`;**
//! * **a `Policy` that is not a resource-policy document is refused
//!   `MalformedResourcePolicyDocumentException`, a `ResourceArn` naming no resource that
//!   can hold a policy `ResourcePolicyInvalidParameterException`, one naming a parameter
//!   or document this account does not hold — or one of another account or Region —
//!   `ResourceNotFoundException`, and a request past either documented limit
//!   `ResourcePolicyLimitExceededException`.**
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body — through this crate's
//! one harness (`all/parameter_store.rs`). Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass: every policy read
//! follows the call that attached it, and every resource a policy is attached to is
//! created through its own operation.
//!
//! The tables' rows differ along what the rules generalise over: which kind of resource
//! holds the policy, the shape and spelling of the document, which of the id/hash pair a
//! call carries, how many policies a resource holds and how a document fails to be a
//! policy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, router, ACCOUNT, REGION};

/// The largest policy document `PutResourcePolicy` accepts: "a policy can't be greater
/// than 1024 bytes in size" (`ResourcePolicyLimitExceededException`).
const MAX_POLICY_SIZE: usize = 1024;

/// How many policies one OpsItemGroup holds: "only one policy can be attached to
/// `OpsItemGroup`" (the same error).
const MAX_OPS_ITEM_GROUP_POLICIES: usize = 1;

// The store also refuses a create past `state::resource_policies::MAX_RESOURCE_POLICIES`,
// a ceiling on how many policy records one emulator process holds. No AWS condition
// corresponds to it — it is this emulator's own bound on caller-driven growth — so no row
// below asserts its wording.

/// A `Command` document, for the cases whose policy is attached to a document.
const COMMAND_DOCUMENT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Echo a message on a managed node",
  "mainSteps": [
    {
      "action": "aws:runShellScript",
      "name": "runShell",
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#;

/// The ARN of an OpsItemGroup in the Region every request signs for.
fn ops_item_group_arn(id: &str) -> String {
    format!("arn:aws:ssm:{REGION}:{ACCOUNT}:opsitemgroup/{id}")
}

/// The ARN of a parameter in the Region every request signs for.
fn parameter_arn(name: &str) -> String {
    format!(
        "arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// The ARN of a document in the Region every request signs for.
fn document_arn(name: &str) -> String {
    format!("arn:aws:ssm:{REGION}:{ACCOUNT}:document/{name}")
}

/// A resource policy whose `Sid` is `sid`, so a case can choose the document's SIZE and
/// two cases can differ in their document alone.
fn policy_with_sid(sid: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\
         \"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::111122223333:root\"}},\
         \"Action\":\"ssm:GetOpsItem\",\"Resource\":\"*\"}}]}}"
    )
}

/// A policy document of exactly `bytes` bytes, padded in its `Sid`.
fn policy_of_size(bytes: usize) -> String {
    let empty = policy_with_sid("");
    assert!(
        empty.len() <= bytes,
        "a policy of {bytes} bytes cannot be built from a {} byte frame",
        empty.len()
    );
    let document = policy_with_sid(&"A".repeat(bytes - empty.len()));
    assert_eq!(document.len(), bytes, "the padded policy is {bytes} bytes");
    document
}

/// `PutResourcePolicy`, returning `(status, body)`.
async fn put(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PutResourcePolicy", body).await
}

/// `PutResourcePolicy` of a NEW policy, asserting it succeeded and returning
/// `(PolicyId, PolicyHash)`.
async fn attach(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
    policy: &str,
) -> (String, String) {
    let (status, response) = put(
        router,
        &json!({ "ResourceArn": resource_arn, "Policy": policy }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutResourcePolicy must succeed, got {status} {response}"
    );
    let policy_id = response["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a put reports a PolicyId, got {response}"))
        .to_string();
    let policy_hash = response["PolicyHash"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a put reports a PolicyHash, got {response}"))
        .to_string();
    assert!(
        !policy_id.is_empty(),
        "case {case:?}: expected a minted PolicyId, got {policy_id:?}"
    );
    assert!(
        is_policy_hash(&policy_hash),
        "case {case:?}: expected a hexadecimal digest as the PolicyHash, got {policy_hash:?}"
    );
    (policy_id, policy_hash)
}

/// `GetResourcePolicies` of one resource, asserting the read succeeded and returning its
/// `Policies`.
async fn policies_of(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> Vec<Value> {
    let (status, response) = call(
        router,
        "GetResourcePolicies",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetResourcePolicies must succeed, got {status} {response}"
    );
    response["Policies"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a read reports Policies, got {response}"))
}

/// Whether `value` is the digest form a `PolicyHash` carries: lowercase hexadecimal of a
/// 256-bit digest.
fn is_policy_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether two policy documents say the same thing, whatever layout each is written in —
/// which is what a client that re-reads what it wrote compares.
fn same_document(left: &str, right: &str) -> bool {
    let parse = |text: &str| serde_json::from_str::<Value>(text).ok();
    parse(left).is_some() && parse(left) == parse(right)
}

/// The `Policy` text of one reported entry.
fn policy_text(entry: &Value) -> String {
    entry["Policy"].as_str().unwrap_or_default().to_string()
}

/// Create the parameter `name` through Parameter Store's own operation.
async fn create_parameter(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, response) = call(
        router,
        "PutParameter",
        &json!({ "Name": name, "Value": "attached", "Type": "String" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutParameter {name} must succeed, got {status} {response}"
    );
}

/// Create the document `name` through the document store's own operation.
async fn create_document(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, response) = call(
        router,
        "CreateDocument",
        &json!({ "Name": name, "Content": COMMAND_DOCUMENT, "DocumentType": "Command" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateDocument {name} must succeed, got {status} {response}"
    );
}

/// Which kind of resource a row attaches its policy to, and what has to exist first.
enum Holder {
    /// The account's OpsCenter group of this id — nothing creates one.
    OpsItemGroup(&'static str),
    /// A parameter of this name, created before the policy is attached.
    Parameter(&'static str),
    /// A document of this name, created before the policy is attached.
    Document(&'static str),
}

/// One row of the attach-and-read suite.
struct AttachRow {
    case: &'static str,
    holder: Holder,
    /// The document attached, in the layout the caller wrote it.
    policy: &'static str,
}

/// A policy written compactly, one with whitespace and a statement written as the object
/// itself rather than as a list — the spellings a client submits.
const COMPACT_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Share","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ssm:GetOpsItem","Resource":"*"}]}"#;

const INDENTED_POLICY: &str = r#"{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "ShareParameter",
            "Effect": "Allow",
            "Principal": { "AWS": ["arn:aws:iam::111122223333:root", "arn:aws:iam::444455556666:root"] },
            "Action": ["ssm:GetParameter", "ssm:GetParameters"],
            "Resource": "*"
        }
    ]
}"#;

const SINGLE_STATEMENT_POLICY: &str = r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":"ssm:GetDocument"}}"#;

const DENY_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","NotPrincipal":{"AWS":"arn:aws:iam::111122223333:root"},"NotAction":"ssm:GetOpsItem","Resource":"*"}]}"#;

const ATTACH_ROWS: &[AttachRow] = &[
    AttachRow {
        case: "an OpsItemGroup, a compact one-statement policy",
        holder: Holder::OpsItemGroup("default"),
        policy: COMPACT_POLICY,
    },
    AttachRow {
        case: "another OpsItemGroup, a policy written with a deny and the complement elements",
        holder: Holder::OpsItemGroup("team-oncall"),
        policy: DENY_POLICY,
    },
    AttachRow {
        case: "a plain parameter, an indented policy naming two principals and two actions",
        holder: Holder::Parameter("shared-parameter"),
        policy: INDENTED_POLICY,
    },
    AttachRow {
        case: "a hierarchy parameter",
        holder: Holder::Parameter("/demo/db/password"),
        policy: COMPACT_POLICY,
    },
    AttachRow {
        case: "a document, one statement written as the object itself",
        holder: Holder::Document("SharedCommandDocument"),
        policy: SINGLE_STATEMENT_POLICY,
    },
];

/// A put attaches the policy to the resource its ARN names and a read of that resource
/// reports what was persisted — the same id, the same hash and the same document.
#[tokio::test]
async fn a_put_attaches_a_policy_that_a_read_reports_back() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for row in ATTACH_ROWS {
        let resource_arn = match row.holder {
            Holder::OpsItemGroup(id) => ops_item_group_arn(id),
            Holder::Parameter(name) => {
                create_parameter(&router, row.case, name).await;
                parameter_arn(name)
            }
            Holder::Document(name) => {
                create_document(&router, row.case, name).await;
                document_arn(name)
            }
        };

        let (policy_id, policy_hash) = attach(&router, row.case, &resource_arn, row.policy).await;

        let reported = policies_of(&router, row.case, &resource_arn).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the one attached policy, got {reported:?}",
            row.case
        );
        let entry = &reported[0];
        assert_eq!(
            entry["PolicyId"].as_str(),
            Some(policy_id.as_str()),
            "case {:?}: expected the read to report the minted PolicyId {policy_id}, got {entry}",
            row.case
        );
        assert_eq!(
            entry["PolicyHash"].as_str(),
            Some(policy_hash.as_str()),
            "case {:?}: expected the read to report the PolicyHash the put answered \
             {policy_hash}, got {entry}",
            row.case
        );
        let read_back = policy_text(entry);
        assert!(
            same_document(row.policy, &read_back),
            "case {:?}: expected the read to report the submitted document, got {read_back:?}",
            row.case
        );

        assert!(
            !minted.contains(&policy_id),
            "case {:?}: expected a fresh PolicyId, got {policy_id} again after {minted:?}",
            row.case
        );
        minted.push(policy_id);
    }
}

/// One row of the digest suite: a document, and whether it must hash like the reference.
struct HashRow {
    case: &'static str,
    policy: &'static str,
    /// Whether this document is the same document as [`COMPACT_POLICY`].
    same_as_reference: bool,
}

const HASH_ROWS: &[HashRow] = &[
    HashRow {
        case: "the reference document, re-submitted byte for byte",
        policy: COMPACT_POLICY,
        same_as_reference: true,
    },
    HashRow {
        case: "the same document written with whitespace between its tokens",
        policy: r#"{ "Version" : "2012-10-17" , "Statement" : [ { "Sid" : "Share" ,
            "Effect" : "Allow" , "Principal" : { "AWS" : "arn:aws:iam::111122223333:root" } ,
            "Action" : "ssm:GetOpsItem" , "Resource" : "*" } ] }"#,
        same_as_reference: true,
    },
    HashRow {
        case: "a document whose action differs",
        policy: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Share","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ssm:DescribeOpsItems","Resource":"*"}]}"#,
        same_as_reference: false,
    },
    HashRow {
        case: "a document whose principal differs",
        policy: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Share","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::444455556666:root"},"Action":"ssm:GetOpsItem","Resource":"*"}]}"#,
        same_as_reference: false,
    },
    HashRow {
        case: "a document carrying one more statement",
        policy: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Share","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ssm:GetOpsItem","Resource":"*"},{"Sid":"Second","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ssm:DescribeOpsItems","Resource":"*"}]}"#,
        same_as_reference: false,
    },
];

/// The `PolicyHash` is the document's own digest: the same document always hashes to the
/// same value, whatever layout it arrived in, and a document changed in any way hashes to
/// a different one.
#[tokio::test]
async fn the_policy_hash_is_the_documents_own_digest() {
    let router = router();
    let reference =
        attach(&router, "the reference", &ops_item_group_arn("reference"), COMPACT_POLICY)
            .await
            .1;

    for (index, row) in HASH_ROWS.iter().enumerate() {
        // Each row is attached to a resource of its own, so what is compared is the
        // document and nothing about which resource holds it.
        let resource_arn = ops_item_group_arn(&format!("hash-row-{index}"));
        let (_, policy_hash) = attach(&router, row.case, &resource_arn, row.policy).await;
        assert_eq!(
            policy_hash == reference,
            row.same_as_reference,
            "case {:?}: expected a hash {} the reference {reference}, got {policy_hash}",
            row.case,
            if row.same_as_reference {
                "equal to"
            } else {
                "different from"
            }
        );
    }
}

/// Which of the id/hash pair an update carries, and how it is spelled.
enum Addressed {
    /// Both, as the last write answered them: the one form that updates.
    StoredPair,
    /// The stored id with a hash that was the policy's before the last update.
    ObsoleteHash,
    /// The stored id with a hash nothing ever issued.
    UnknownHash,
    /// A policy id this resource holds nothing under, with the stored hash.
    UnknownId,
    /// The stored id alone.
    IdOnly,
    /// The stored hash alone.
    HashOnly,
}

/// One row of the update suite.
struct UpdateRow {
    case: &'static str,
    addressed: Addressed,
    /// The HTTP status the call must answer.
    status: u16,
    /// The modelled error code, or `None` for the row that succeeds.
    code: Option<&'static str>,
    /// The member `ParameterNames` must name, when the refusal names one.
    parameter_name: Option<&'static str>,
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "the stored id and hash",
        addressed: Addressed::StoredPair,
        status: 200,
        code: None,
        parameter_name: None,
    },
    UpdateRow {
        case: "the stored id with the hash the policy carried before it was updated",
        addressed: Addressed::ObsoleteHash,
        status: 400,
        code: Some("ResourcePolicyConflictException"),
        parameter_name: None,
    },
    UpdateRow {
        case: "the stored id with a hash nothing issued",
        addressed: Addressed::UnknownHash,
        status: 400,
        code: Some("ResourcePolicyConflictException"),
        parameter_name: None,
    },
    UpdateRow {
        case: "a policy id the resource holds nothing under",
        addressed: Addressed::UnknownId,
        status: 400,
        code: Some("ResourcePolicyNotFoundException"),
        parameter_name: None,
    },
    UpdateRow {
        case: "the id without the hash",
        addressed: Addressed::IdOnly,
        status: 400,
        code: Some("ResourcePolicyInvalidParameterException"),
        parameter_name: Some("PolicyHash"),
    },
    UpdateRow {
        case: "the hash without the id",
        addressed: Addressed::HashOnly,
        status: 400,
        code: Some("ResourcePolicyInvalidParameterException"),
        parameter_name: Some("PolicyId"),
    },
];

/// A rewrite proceeds only against the policy version the caller last read: the stored
/// pair rewrites the document and moves the hash, every other spelling of the pair is
/// refused with its own modelled class and leaves the stored policy exactly as it was.
#[tokio::test]
async fn an_update_proceeds_only_against_the_stored_policy_version() {
    for row in UPDATE_ROWS {
        // A router per row, so each row starts from the same one attached policy.
        let router = router();
        let resource_arn = ops_item_group_arn("default");
        let (policy_id, first_hash) =
            attach(&router, row.case, &resource_arn, COMPACT_POLICY).await;

        // One accepted update first, so the row that sends an obsolete hash has one to
        // send: the hash moves while the id stays.
        let updated = policy_with_sid("FirstUpdate");
        let (status, response) = put(
            &router,
            &json!({
                "ResourceArn": resource_arn,
                "Policy": updated,
                "PolicyId": policy_id,
                "PolicyHash": first_hash,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first update must succeed, got {status} {response}",
            row.case
        );
        assert_eq!(
            response["PolicyId"].as_str(),
            Some(policy_id.as_str()),
            "case {:?}: expected an update to keep the policy's id, got {response}",
            row.case
        );
        let stored_hash = response["PolicyHash"]
            .as_str()
            .expect("an update reports a PolicyHash")
            .to_string();
        assert_ne!(
            stored_hash, first_hash,
            "case {:?}: expected the hash to move with the document, got {stored_hash}",
            row.case
        );

        let rewritten = policy_with_sid("SecondUpdate");
        let mut body = json!({ "ResourceArn": resource_arn, "Policy": rewritten });
        let fields = body.as_object_mut().expect("the request body is an object");
        match row.addressed {
            Addressed::StoredPair => {
                fields.insert("PolicyId".into(), json!(policy_id));
                fields.insert("PolicyHash".into(), json!(stored_hash));
            }
            Addressed::ObsoleteHash => {
                fields.insert("PolicyId".into(), json!(policy_id));
                fields.insert("PolicyHash".into(), json!(first_hash));
            }
            Addressed::UnknownHash => {
                fields.insert("PolicyId".into(), json!(policy_id));
                fields.insert("PolicyHash".into(), json!("f".repeat(64)));
            }
            Addressed::UnknownId => {
                fields.insert("PolicyId".into(), json!("00000000-0000-4000-8000-000000000000"));
                fields.insert("PolicyHash".into(), json!(stored_hash));
            }
            Addressed::IdOnly => {
                fields.insert("PolicyId".into(), json!(policy_id));
            }
            Addressed::HashOnly => {
                fields.insert("PolicyHash".into(), json!(stored_hash));
            }
        }

        let (status, response) = put(&router, &body).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {response}",
            row.case, row.status
        );
        if let Some(expected) = row.code {
            let code = error_code(&response);
            assert_eq!(
                code, expected,
                "case {:?}: expected {expected}, got {code} {response}",
                row.case
            );
        }
        if let Some(member) = row.parameter_name {
            let named = response["ParameterNames"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                named,
                vec![Value::String(member.to_string())],
                "case {:?}: expected the refusal to name {member}, got {response}",
                row.case
            );
        }

        // What the resource holds afterwards: the rewritten document only where the
        // update was accepted, and the document the accepted update stored everywhere
        // else.
        let reported = policies_of(&router, row.case, &resource_arn).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the resource to still hold one policy, got {reported:?}",
            row.case
        );
        let expected_document = if row.code.is_none() { &rewritten } else { &updated };
        let read_back = policy_text(&reported[0]);
        assert!(
            same_document(expected_document, &read_back),
            "case {:?}: expected the stored document to be {expected_document}, got {read_back:?}",
            row.case
        );
        assert_eq!(
            reported[0]["PolicyId"].as_str(),
            Some(policy_id.as_str()),
            "case {:?}: expected the policy to keep its id, got {:?}",
            row.case,
            reported[0]
        );
    }
}

/// One row of the delete suite: which pair the delete carries.
struct DeleteRow {
    case: &'static str,
    /// Whether the call carries the stored id.
    stored_id: bool,
    /// Whether the call carries the stored hash.
    stored_hash: bool,
    /// `DeletionMode`, which "applies only to the Document resource type", or `None`.
    deletion_mode: Option<&'static str>,
    /// The modelled error code, or `None` for the rows that delete.
    code: Option<&'static str>,
}

const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "the stored id and hash",
        stored_id: true,
        stored_hash: true,
        deletion_mode: None,
        code: None,
    },
    DeleteRow {
        case: "the stored pair with RemoveSharing",
        stored_id: true,
        stored_hash: true,
        deletion_mode: Some("RemoveSharing"),
        code: None,
    },
    DeleteRow {
        case: "the stored pair with RollbackMigration",
        stored_id: true,
        stored_hash: true,
        deletion_mode: Some("RollbackMigration"),
        code: None,
    },
    DeleteRow {
        case: "the stored id with a hash that is not the policy's",
        stored_id: true,
        stored_hash: false,
        deletion_mode: None,
        code: Some("ResourcePolicyNotFoundException"),
    },
    DeleteRow {
        case: "a policy id the resource holds nothing under",
        stored_id: false,
        stored_hash: true,
        deletion_mode: None,
        code: Some("ResourcePolicyNotFoundException"),
    },
    DeleteRow {
        case: "neither the stored id nor the stored hash",
        stored_id: false,
        stored_hash: false,
        deletion_mode: None,
        code: Some("ResourcePolicyNotFoundException"),
    },
];

/// A delete addresses a policy by its id and its hash together: a pair naming no stored
/// policy removes nothing, and the policy is still there afterwards.
#[tokio::test]
async fn a_delete_removes_only_the_policy_its_id_and_hash_name() {
    for row in DELETE_ROWS {
        let router = router();
        let resource_arn = ops_item_group_arn("default");
        let (policy_id, policy_hash) =
            attach(&router, row.case, &resource_arn, COMPACT_POLICY).await;

        let mut body = json!({
            "ResourceArn": resource_arn,
            "PolicyId": if row.stored_id { policy_id.clone() } else { "00000000-0000-4000-8000-000000000000".to_string() },
            "PolicyHash": if row.stored_hash { policy_hash.clone() } else { "f".repeat(64) },
        });
        if let Some(mode) = row.deletion_mode {
            body.as_object_mut()
                .expect("the request body is an object")
                .insert("DeletionMode".into(), json!(mode));
        }

        let (status, response) = call(&router, "DeleteResourcePolicy", &body).await;
        match row.code {
            None => assert_eq!(
                status, 200,
                "case {:?}: DeleteResourcePolicy must succeed, got {status} {response}",
                row.case
            ),
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected HTTP 400, got {status} {response}",
                    row.case
                );
                let code = error_code(&response);
                assert_eq!(
                    code, expected,
                    "case {:?}: expected {expected}, got {code} {response}",
                    row.case
                );
            }
        }

        let reported = policies_of(&router, row.case, &resource_arn).await;
        let expected_remaining = usize::from(row.code.is_some());
        assert_eq!(
            reported.len(),
            expected_remaining,
            "case {:?}: expected {expected_remaining} policy left, got {reported:?}",
            row.case
        );

        // The policy is gone for good: a second delete of the pair that worked finds
        // nothing to remove.
        if row.code.is_none() {
            let (status, response) = call(
                &router,
                "DeleteResourcePolicy",
                &json!({
                    "ResourceArn": resource_arn,
                    "PolicyId": policy_id,
                    "PolicyHash": policy_hash,
                }),
            )
            .await;
            let code = error_code(&response);
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourcePolicyNotFoundException"),
                "case {:?}: expected the second delete to find no policy, got {status} {response}",
                row.case
            );
        }
    }

    // The hash a policy carried BEFORE it was rewritten names no policy either: the pair
    // addresses the version the caller last read, and that version is gone.
    let router = router();
    let resource_arn = ops_item_group_arn("default");
    let (policy_id, first_hash) =
        attach(&router, "an obsolete hash", &resource_arn, COMPACT_POLICY).await;
    let (status, response) = put(
        &router,
        &json!({
            "ResourceArn": resource_arn,
            "Policy": policy_with_sid("Rewritten"),
            "PolicyId": policy_id,
            "PolicyHash": first_hash,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {response}"
    );
    let (status, response) = call(
        &router,
        "DeleteResourcePolicy",
        &json!({
            "ResourceArn": resource_arn,
            "PolicyId": policy_id,
            "PolicyHash": first_hash,
        }),
    )
    .await;
    let code = error_code(&response);
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourcePolicyNotFoundException"),
        "expected the hash from before the update to name no policy, got {status} {response}"
    );
    let reported = policies_of(&router, "an obsolete hash", &resource_arn).await;
    assert_eq!(
        reported.len(),
        1,
        "expected the rewritten policy to survive a delete holding the old hash, got \
         {reported:?}"
    );
}

/// How a text fails to be a resource-policy document. The rows walk the clauses of the
/// IAM policy grammar, one broken clause each.
const MALFORMED_ROWS: &[(&str, &str)] = &[
    ("text that is not JSON at all", "not a policy"),
    ("a JSON scalar", "\"just a string\""),
    ("a JSON array", "[{\"Effect\":\"Allow\"}]"),
    ("an object declaring no statement", "{\"Version\":\"2012-10-17\"}"),
    ("an empty statement list", "{\"Statement\":[]}"),
    (
        "a policy version that does not exist",
        r#"{"Version":"2024-01-01","Statement":[{"Effect":"Allow","Principal":"*","Action":"ssm:GetOpsItem"}]}"#,
    ),
    (
        "a statement with no Effect",
        r#"{"Statement":[{"Principal":"*","Action":"ssm:GetOpsItem"}]}"#,
    ),
    (
        "a statement naming no action",
        r#"{"Statement":[{"Effect":"Allow","Principal":"*"}]}"#,
    ),
    (
        "a statement naming no principal",
        r#"{"Statement":[{"Effect":"Allow","Action":"ssm:GetOpsItem"}]}"#,
    ),
];

/// A `Policy` that is not a resource-policy document is refused as malformed, and nothing
/// is attached.
#[tokio::test]
async fn a_policy_that_is_not_a_resource_policy_document_is_refused() {
    let router = router();
    let resource_arn = ops_item_group_arn("default");

    for (case, policy) in MALFORMED_ROWS {
        let (status, response) = put(
            &router,
            &json!({ "ResourceArn": resource_arn, "Policy": policy }),
        )
        .await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, "MalformedResourcePolicyDocumentException"),
            "case {case:?}: expected a malformed-document refusal, got {status} {response}"
        );
        let reported = policies_of(&router, case, &resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected a refused put to attach nothing, got {reported:?}"
        );
    }
}

/// A `ResourceArn` that names no Systems Manager resource able to hold a resource policy.
/// Each row is well inside the modelled `ResourceArnString` length, so the refusal is this
/// operation's own.
const INVALID_RESOURCE_ARN_ROWS: &[(&str, &str)] = &[
    ("a value that is no ARN at all", "not-an-arn-but-long-enough-to-pass"),
    ("an ARN of another vendor", "arn:aws:s3:::a-bucket-of-some-length"),
    (
        "an ssm ARN of a resource type that holds no policy",
        "arn:aws:ssm:us-west-2:000000000000:automation-execution/abc123",
    ),
    (
        "an ssm ARN whose resource names no id",
        "arn:aws:ssm:us-west-2:000000000000:parameter/",
    ),
    (
        "an ARN missing its resource field",
        "arn:aws:ssm:us-west-2:000000000000:",
    ),
];

/// A put, a read and a delete all refuse a `ResourceArn` that names no resource a policy
/// can be attached to, naming the member that isn't valid.
#[tokio::test]
async fn a_resource_arn_naming_no_policy_holding_resource_is_refused() {
    let router = router();

    for (case, resource_arn) in INVALID_RESOURCE_ARN_ROWS {
        let calls: [(&str, Value); 3] = [
            (
                "PutResourcePolicy",
                json!({ "ResourceArn": resource_arn, "Policy": COMPACT_POLICY }),
            ),
            (
                "GetResourcePolicies",
                json!({ "ResourceArn": resource_arn }),
            ),
            (
                "DeleteResourcePolicy",
                json!({
                    "ResourceArn": resource_arn,
                    "PolicyId": "00000000-0000-4000-8000-000000000000",
                    "PolicyHash": "f".repeat(64),
                }),
            ),
        ];
        for (operation, body) in &calls {
            let (status, response) = call(&router, operation, body).await;
            let code = error_code(&response);
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourcePolicyInvalidParameterException"),
                "case {case:?}: expected {operation} to refuse the ARN, got {status} {response}"
            );
            assert_eq!(
                response["ParameterNames"].as_array().cloned().unwrap_or_default(),
                vec![Value::String("ResourceArn".to_string())],
                "case {case:?}: expected {operation} to name ResourceArn, got {response}"
            );
        }
    }
}

/// The sentence `ResourceNotFoundException` carries, which is the one the model documents
/// for that error (`smithy_query(query="shape",shape="ResourceNotFoundException")`: "the
/// specified parameter to be shared could not be found").
const RESOURCE_NOT_FOUND_MESSAGE: &str = "The specified parameter to be shared could not be found.";

/// An ARN naming a parameter or a document this account does not hold names nothing to
/// share, and the same ARN works once the resource is created.
#[tokio::test]
async fn an_arn_naming_a_resource_this_account_does_not_hold_is_not_found() {
    let router = router();

    let rows: [(&str, String); 3] = [
        (
            "a parameter nobody created",
            parameter_arn("never-created"),
        ),
        (
            "a hierarchy parameter nobody created",
            parameter_arn("/missing/path/value"),
        ),
        (
            "a document nobody created",
            document_arn("NeverCreatedDocument"),
        ),
    ];

    for (case, resource_arn) in &rows {
        let (status, response) = put(
            &router,
            &json!({ "ResourceArn": resource_arn, "Policy": COMPACT_POLICY }),
        )
        .await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected a not-found refusal, got {status} {response}"
        );
        assert_eq!(
            response["Message"].as_str(),
            Some(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the modelled not-found sentence, got {response}"
        );
    }

    // The same ARN, once the resource exists: the refusal was about the resource, not
    // about the ARN.
    create_parameter(&router, "the parameter is created", "never-created").await;
    let (_, policy_hash) = attach(
        &router,
        "the parameter is created",
        &parameter_arn("never-created"),
        COMPACT_POLICY,
    )
    .await;
    assert!(
        is_policy_hash(&policy_hash),
        "expected the put to succeed once the parameter exists, got {policy_hash:?}"
    );
}

/// The two documented limits: a policy document at most [`MAX_POLICY_SIZE`] bytes, and at
/// most [`MAX_OPS_ITEM_GROUP_POLICIES`] policy on an OpsItemGroup. The size rows are the
/// published bound and one byte past it.
#[tokio::test]
async fn the_documented_resource_policy_limits_are_enforced() {
    let router = router();

    for (case, size, accepted) in [
        ("one byte under the size limit", MAX_POLICY_SIZE - 1, true),
        ("exactly at the size limit", MAX_POLICY_SIZE, true),
        ("one byte past the size limit", MAX_POLICY_SIZE + 1, false),
    ] {
        let resource_arn = ops_item_group_arn(&format!("size-{size}"));
        let (status, response) = put(
            &router,
            &json!({ "ResourceArn": resource_arn, "Policy": policy_of_size(size) }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a {size} byte policy must be accepted, got {status} {response}"
            );
            continue;
        }
        let code = error_code(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourcePolicyLimitExceededException"),
            "case {case:?}: expected a limit refusal, got {status} {response}"
        );
        assert_eq!(
            response["Limit"].as_u64(),
            Some(MAX_POLICY_SIZE as u64),
            "case {case:?}: expected the refusal to report the size limit, got {response}"
        );
    }

    // "Only one policy can be attached to OpsItemGroup": the second create is refused and
    // the first policy is untouched.
    let resource_arn = ops_item_group_arn("default");
    let (policy_id, _) = attach(&router, "the first policy", &resource_arn, COMPACT_POLICY).await;
    let (status, response) = put(
        &router,
        &json!({ "ResourceArn": resource_arn, "Policy": policy_with_sid("Second") }),
    )
    .await;
    let code = error_code(&response);
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourcePolicyLimitExceededException"),
        "expected a second OpsItemGroup policy to be refused, got {status} {response}"
    );
    assert_eq!(
        response["Limit"].as_u64(),
        Some(MAX_OPS_ITEM_GROUP_POLICIES as u64),
        "expected the refusal to report how many policies an OpsItemGroup holds, got {response}"
    );
    let reported = policies_of(&router, "the first policy survives", &resource_arn).await;
    assert_eq!(
        reported.len(),
        1,
        "expected the one attached policy to be untouched, got {reported:?}"
    );
    assert_eq!(
        reported[0]["PolicyId"].as_str(),
        Some(policy_id.as_str()),
        "expected the first policy to be the one still attached, got {:?}",
        reported[0]
    );
}

/// A read walks a resource's policies in attachment order and pages at the number the
/// request asks for, resuming at the policy the previous page stopped at; a token this
/// operation did not issue is refused.
#[tokio::test]
async fn a_read_pages_a_resources_policies_in_attachment_order() {
    let router = router();
    create_parameter(&router, "the holder", "multi-policy").await;
    let resource_arn = parameter_arn("multi-policy");

    let mut attached: Vec<String> = Vec::new();
    for sid in ["First", "Second", "Third"] {
        let (policy_id, _) = attach(&router, sid, &resource_arn, &policy_with_sid(sid)).await;
        attached.push(policy_id);
    }

    // Rows: a page smaller than, equal to and larger than what the resource holds, plus
    // the call that asks for no page size at all.
    for (case, page_size) in [
        ("one at a time", Some(1usize)),
        ("two at a time", Some(2)),
        ("more than it holds", Some(50)),
        ("no page size asked for", None),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({ "ResourceArn": resource_arn });
            let fields = body.as_object_mut().expect("the request body is an object");
            if let Some(size) = page_size {
                fields.insert("MaxResults".into(), json!(size));
            }
            if let Some(token) = &next_token {
                fields.insert("NextToken".into(), json!(token));
            }
            let (status, response) = call(&router, "GetResourcePolicies", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetResourcePolicies must succeed, got {status} {response}"
            );
            let page = response["Policies"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size.unwrap_or(50),
                "case {case:?}: expected at most {page_size:?} policies in a page, got {page:?}"
            );
            walked.extend(page.iter().map(|entry| {
                entry["PolicyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            next_token = response["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, attached,
            "case {case:?}: expected the policies in attachment order, got {walked:?}"
        );
    }

    let (status, response) = call(
        &router,
        "GetResourcePolicies",
        &json!({ "ResourceArn": resource_arn, "NextToken": "not-a-token-this-read-issued" }),
    )
    .await;
    let code = error_code(&response);
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourcePolicyInvalidParameterException"),
        "expected a token this read did not issue to be refused, got {status} {response}"
    );
    assert_eq!(
        response["ParameterNames"].as_array().cloned().unwrap_or_default(),
        vec![Value::String("NextToken".to_string())],
        "expected the refusal to name NextToken, got {response}"
    );
}

/// A policy is held per resource: two resources' policies do not mix, and deleting one
/// leaves the other.
#[tokio::test]
async fn policies_are_held_per_resource() {
    let router = router();
    create_parameter(&router, "two resources", "first-parameter").await;
    create_parameter(&router, "two resources", "second-parameter").await;
    let first = parameter_arn("first-parameter");
    let second = parameter_arn("second-parameter");

    let (first_id, first_hash) = attach(&router, "the first resource", &first, COMPACT_POLICY).await;
    let (second_id, _) = attach(&router, "the second resource", &second, INDENTED_POLICY).await;
    assert_ne!(
        first_id, second_id,
        "expected each attachment to carry its own id, got {first_id} twice"
    );

    let (status, response) = call(
        &router,
        "DeleteResourcePolicy",
        &json!({ "ResourceArn": first, "PolicyId": first_id, "PolicyHash": first_hash }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete of the first resource's policy must succeed, got {status} {response}"
    );

    assert!(
        policies_of(&router, "the first resource", &first)
            .await
            .is_empty(),
        "expected the first resource to hold no policy after the delete"
    );
    let remaining = policies_of(&router, "the second resource", &second).await;
    assert_eq!(
        remaining.len(),
        1,
        "expected the second resource's policy to be untouched, got {remaining:?}"
    );
    assert_eq!(
        remaining[0]["PolicyId"].as_str(),
        Some(second_id.as_str()),
        "expected the second resource's own policy, got {:?}",
        remaining[0]
    );
}

/// A policy is held against the RESOURCE the ARN names, reached through the endpoint the
/// request was signed for — so two spellings of the one resource address the one set of
/// policies, while an ARN naming another account or another Region names nothing
/// reachable here.
#[tokio::test]
async fn an_arn_addresses_the_resource_the_endpoint_holds() {
    let router = router();
    create_parameter(&router, "one parameter", "shared-by-two-spellings").await;

    // Spellings of the one resource: the `type:id` separator an ARN may also be written
    // with, and the Region-less and account-less forms a well-formed ARN may carry.
    let same: [(&str, String, String); 3] = [
        (
            "an OpsItemGroup ARN written with a colon before its id",
            ops_item_group_arn("default"),
            format!("arn:aws:ssm:{REGION}:{ACCOUNT}:opsitemgroup:default"),
        ),
        (
            "a parameter ARN with an empty Region field",
            parameter_arn("shared-by-two-spellings"),
            format!("arn:aws:ssm::{ACCOUNT}:parameter/shared-by-two-spellings"),
        ),
        (
            "a parameter ARN with an empty account field",
            parameter_arn("shared-by-two-spellings"),
            format!("arn:aws:ssm:{REGION}::parameter/shared-by-two-spellings"),
        ),
    ];

    for (case, written, other) in &same {
        let (policy_id, policy_hash) = attach(&router, case, written, COMPACT_POLICY).await;

        // The other spelling reports the one attached policy …
        let reported = policies_of(&router, case, other).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected {other} to report the policy attached through \
             {written}, got {reported:?}"
        );
        assert_eq!(
            reported[0]["PolicyId"].as_str(),
            Some(policy_id.as_str()),
            "case {case:?}: expected the same policy through both spellings, got {:?}",
            reported[0]
        );

        // … and deleting through it removes that policy, which the first spelling then
        // reports as gone.
        let (status, response) = call(
            &router,
            "DeleteResourcePolicy",
            &json!({ "ResourceArn": other, "PolicyId": policy_id, "PolicyHash": policy_hash }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete through {other} must succeed, got {status} {response}"
        );
        assert!(
            policies_of(&router, case, written).await.is_empty(),
            "case {case:?}: expected {written} to hold no policy after the delete"
        );
    }

    // An ARN of another account or another Region than the request was signed for names
    // nothing this endpoint holds, on every one of the three operations.
    let unreachable: [(&str, String); 3] = [
        (
            "a parameter ARN naming another Region",
            format!("arn:aws:ssm:us-east-1:{ACCOUNT}:parameter/shared-by-two-spellings"),
        ),
        (
            "a parameter ARN naming another account",
            format!("arn:aws:ssm:{REGION}:111122223333:parameter/shared-by-two-spellings"),
        ),
        (
            "an OpsItemGroup ARN naming another account",
            format!("arn:aws:ssm:{REGION}:111122223333:opsitemgroup/default"),
        ),
    ];

    for (case, resource_arn) in &unreachable {
        let calls: [(&str, Value); 3] = [
            (
                "PutResourcePolicy",
                json!({ "ResourceArn": resource_arn, "Policy": COMPACT_POLICY }),
            ),
            ("GetResourcePolicies", json!({ "ResourceArn": resource_arn })),
            (
                "DeleteResourcePolicy",
                json!({
                    "ResourceArn": resource_arn,
                    "PolicyId": "00000000-0000-4000-8000-000000000000",
                    "PolicyHash": "f".repeat(64),
                }),
            ),
        ];
        for (operation, body) in &calls {
            let (status, response) = call(&router, operation, body).await;
            let code = error_code(&response);
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected {operation} to find nothing for {resource_arn}, \
                 got {status} {response}"
            );
        }
    }
}
