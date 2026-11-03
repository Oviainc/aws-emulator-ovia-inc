// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READER of a KMS key's key policy, `GetKeyPolicy`, driven
//! over the wire through the shared [`super::harness`].
//!
//! The sibling [`super::key_policy`] suite drives this reader from the WRITER of the
//! document (`PutKeyPolicy`, and the policy `CreateKey` attaches); this one is about
//! the reader's own rules:
//!
//! 1. "Gets a key policy attached to the specified KMS key"
//!    (`smithy_query(query="doc",shape="GetKeyPolicy")`) — so it is a pure REPORT of
//!    the document the key holds: repeated reads answer the same document under the
//!    same name, and reading changes nothing a later caller sees.
//! 2. The document is a property of the key alone, so it does not move when the key is
//!    written for any other reason, nor when the key moves through its key states.
//! 3. "If no policy name is specified, the default value is default"
//!    (`smithy_query(query="doc",shape="GetKeyPolicyRequest",member="PolicyName")`), so
//!    omitting the member and naming `default` are the same request.

use serde_json::{json, Value};

use super::harness::{b64, create_key, describe, ok, reported_policy, router, text};

/// The only key policy name a KMS key has a policy under: "The only valid value is
/// default" (`smithy_query(query="doc",shape="GetKeyPolicyResponse",member="PolicyName")`).
const DEFAULT_POLICY_NAME: &str = "default";

/// The whole `GetKeyPolicy` answer for `identifier` — both the document and the name it
/// is served under — with `PolicyName` carried only when `policy_name` names it.
async fn read_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
    policy_name: Option<&str>,
) -> Value {
    let mut request = json!({"KeyId": identifier});
    if let Some(policy_name) = policy_name {
        request["PolicyName"] = json!(policy_name);
    }
    ok(router, "GetKeyPolicy", &request).await
}

/// Assert an answer carries a non-empty document served under the one policy name, and
/// report that document.
fn served_document(case: &str, answer: &Value) -> String {
    let name = text(case, answer, "PolicyName");
    assert_eq!(
        name, DEFAULT_POLICY_NAME,
        "case {case:?}: a key serves its policy under the name {DEFAULT_POLICY_NAME}, got \
         {name:?}"
    );
    let document = text(case, answer, "Policy");
    assert!(
        !document.is_empty(),
        "case {case:?}: expected the key's policy document, got {document:?}"
    );
    document
}

/// The policy names `ListKeyPolicies` reports for a key, which must not move either.
async fn policy_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> Value {
    ok(router, "ListKeyPolicies", &json!({"KeyId": identifier})).await["PolicyNames"].clone()
}

/// Rule 1 and rule 3: the read is a report. Every read of a key answers the same
/// document under the same name, whether the request names the policy or leaves the
/// member out, and the reads leave the key exactly as it was — the metadata
/// `DescribeKey` reports and the names `ListKeyPolicies` reports are unchanged
/// afterwards.
///
/// The rows differ in the kind of key holding the policy — the `KeySpec`/`KeyUsage`
/// combinations this service mints, a multi-Region primary, and a key created with no
/// key material of its own — because a key policy is not a property of any of them.
#[tokio::test]
async fn reading_a_key_policy_changes_nothing_a_later_caller_sees() {
    let router = router();

    // Rows: a key kind and the rest of its `CreateKey` request.
    let kinds: [(&str, Value); 6] = [
        ("the default symmetric encryption key", json!({})),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
        (
            "an HMAC_384 key",
            json!({"KeySpec": "HMAC_384", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "an RSA_3072 encryption key",
            json!({"KeySpec": "RSA_3072", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        (
            "an ECC_NIST_P256 signing key",
            json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"}),
        ),
        ("a key created with no key material", json!({"Origin": "EXTERNAL"})),
    ];

    for (described, request) in kinds {
        let metadata = create_key(&router, &request).await;
        let key_id = text(described, &metadata, "KeyId");
        let before_metadata = describe(&router, &key_id).await;
        let before_names = policy_names(&router, &key_id).await;

        let first = read_policy(&router, &key_id, None).await;
        let document = served_document(described, &first);

        // Rows: how the request spells the policy name, over several rounds — an
        // omitted member and the one valid name are the same request, and neither
        // answer moves between rounds.
        for round in 1..=3 {
            for spelling in [None, Some(DEFAULT_POLICY_NAME)] {
                let case = format!("{described}, read {round} with PolicyName {spelling:?}");
                let answer = read_policy(&router, &key_id, spelling).await;
                assert_eq!(
                    answer, first,
                    "case {case:?}: every read must answer the same document under the same \
                     name, got {answer} (was {first})"
                );
            }
        }

        let after_metadata = describe(&router, &key_id).await;
        assert_eq!(
            after_metadata, before_metadata,
            "case {described:?}: reading the key policy must persist nothing, got \
             {after_metadata} (was {before_metadata})"
        );
        let after_names = policy_names(&router, &key_id).await;
        assert_eq!(
            after_names, before_names,
            "case {described:?}: reading the key policy must leave the policy listing as it \
             was, got {after_names} (was {before_names})"
        );
        assert_eq!(
            after_names,
            json!([DEFAULT_POLICY_NAME]),
            "case {described:?}: a key holds one policy, named {DEFAULT_POLICY_NAME}, got \
             {after_names}"
        );
        assert!(
            document.contains("Statement"),
            "case {described:?}: the served document must be an IAM policy document, got \
             {document:?}"
        );
    }
}

/// Rule 2: the document is the key's own, so writing the key for any OTHER reason —
/// and moving it through its key states — leaves the answer byte for byte as it was.
///
/// The rows are the writes this slice can make to a key without touching its policy:
/// its description, its tags, an alias, uses of its key material, its rotation
/// schedule, and the key-state moves `DisableKey`/`EnableKey` and
/// `ScheduleKeyDeletion`/`CancelKeyDeletion` — a key pending deletion still serves the
/// read, as `DescribeKey` still reports its metadata.
#[tokio::test]
async fn the_served_document_does_not_move_when_the_key_is_written_for_another_reason() {
    let router = router();
    let account = engine::emulator_account_id();
    let metadata = create_key(&router, &json!({"Description": "policy reader"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowRoot",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "kms:*",
            "Resource": "*"
        }]
    })
    .to_string();
    ok(
        &router,
        "PutKeyPolicy",
        &json!({"KeyId": key_id, "Policy": document}),
    )
    .await;

    let served = read_policy(&router, &key_id, None).await;
    assert_eq!(
        served_document("setup", &served),
        reported_policy(&document),
        "case setup: the reader must serve the attached document in KMS's own rendering of \
         it, got {served}"
    );

    // Rows: a call that writes the key, or uses it, without touching its policy. The
    // order matters only in that a use of the key material comes before the key is
    // disabled and the deletion moves come last.
    let writes: [(&str, &str, Value); 12] = [
        (
            "a new description",
            "UpdateKeyDescription",
            json!({"KeyId": key_id, "Description": "renamed"}),
        ),
        (
            "a new tag",
            "TagResource",
            json!({"KeyId": key_id, "Tags": [{"TagKey": "env", "TagValue": "prod"}]}),
        ),
        (
            "a tag removed",
            "UntagResource",
            json!({"KeyId": key_id, "TagKeys": ["env"]}),
        ),
        (
            "an alias added",
            "CreateAlias",
            json!({"AliasName": "alias/policy-reader", "TargetKeyId": key_id}),
        ),
        (
            "the key material used to encrypt",
            "Encrypt",
            json!({"KeyId": key_id, "Plaintext": b64(b"key policy")}),
        ),
        (
            "a data key generated under the key",
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
        (
            "automatic rotation configured",
            "EnableKeyRotation",
            json!({"KeyId": key_id, "RotationPeriodInDays": 120}),
        ),
        (
            "automatic rotation turned off",
            "DisableKeyRotation",
            json!({"KeyId": key_id}),
        ),
        ("the key disabled", "DisableKey", json!({"KeyId": key_id})),
        ("the key enabled again", "EnableKey", json!({"KeyId": key_id})),
        (
            "the key pending deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": key_id, "PendingWindowInDays": 7}),
        ),
        (
            "the deletion cancelled",
            "CancelKeyDeletion",
            json!({"KeyId": key_id}),
        ),
    ];

    for (case, operation, request) in writes {
        ok(&router, operation, &request).await;
        let answer = read_policy(&router, &key_id, None).await;
        assert_eq!(
            answer, served,
            "case {case:?}: {operation} must leave the key policy answer exactly as it was, \
             got {answer} (was {served})"
        );
    }
}
