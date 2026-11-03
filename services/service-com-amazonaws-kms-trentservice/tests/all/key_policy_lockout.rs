// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the POLICY LOCKOUT SAFETY CHECK every operation that attaches a
//! key policy applies, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: a key policy a request carries "must allow the calling principal
//! to make a subsequent PutKeyPolicy request on the KMS key. This reduces the risk that
//! the KMS key becomes unmanageable. … (To omit this condition, set
//! BypassPolicyLockoutSafetyCheck to true.)"
//! (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`, which
//! `CreateKeyRequest` and `ReplicateKeyRequest` repeat). A document that leaves no such
//! path is refused `MalformedPolicyDocumentException` (HTTP 400) with the safety check's
//! own sentence, and the key keeps the policy it had; the same document is accepted when
//! the request sets `BypassPolicyLockoutSafetyCheck`, the default the member declares
//! being `false`.
//!
//! The suites assert it over TABLES whose rows differ along what the rule generalises
//! over: HOW a document does or does not reach the caller (naming the principal itself,
//! naming its account, a wildcard principal, an action wildcard of each width, a
//! `Resource` naming this key or another, a condition the request cannot satisfy, an
//! explicit deny beside an allow), how many statements it carries, which OPERATION
//! attaches it (`PutKeyPolicy`, `CreateKey`, `ReplicateKey`) and whether the request asks
//! for the check to be bypassed.

use serde_json::{json, Value};

use super::harness::{
    assert_empty, call, create_key, error_code, error_message, ok, ok_in, refused, reported_policy,
    router, text, REGION,
};

/// The sentence KMS refuses a locking-out key policy with: the safety check's own, which
/// is entirely FIXED text naming neither the principal nor the key.
const LOCKOUT_MESSAGE: &str = "The new key policy will not allow you to update the key policy \
     in the future.";

/// The Region a replica is created in, which is not [`REGION`]: "the replica must be in a
/// different Amazon Web Services Region than its primary key"
/// (`smithy_query(query="doc",shape="ReplicateKeyRequest",member="ReplicaRegion")`).
const REPLICA_REGION: &str = "eu-west-1";

/// A key policy document carrying `statements`.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One `Allow` statement, with `condition` only when one is given.
fn allow(principal: Value, action: Value, resource: Value, condition: Value) -> Value {
    let mut statement = json!({
        "Sid": "UnderTest",
        "Effect": "Allow",
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    statement
}

/// The ARN of the account's root user, the principal the default key policy names.
fn account_root() -> String {
    format!("arn:aws:iam::{}:root", engine::emulator_account_id())
}

/// The principal every request in this suite authenticates as.
fn caller() -> String {
    service_com_amazonaws_kms_trentservice::state::caller_identity_arn()
}

/// One row: the case, the document, and whether it leaves the caller a path to a later
/// `PutKeyPolicy`.
type Row = (&'static str, String, bool);

/// The documents under test, each paired with whether it keeps the policy path open.
///
/// `key_arn` is the key the document will be attached to, so a row can scope its
/// `Resource` to that key or to another one.
fn rows(key_arn: &str, other_key_arn: &str) -> Vec<Row> {
    let root = account_root();
    let caller = caller();
    vec![
        // Reaches the caller: the document names the principal itself, or the account
        // it belongs to — the delegation the default key policy is built on.
        (
            "the caller itself, every KMS action",
            policy(json!([allow(
                json!({"AWS": caller.clone()}),
                json!("kms:*"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
        (
            "the caller itself, the policy action alone",
            policy(json!([allow(
                json!({"AWS": caller.clone()}),
                json!("kms:PutKeyPolicy"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
        (
            "the account root, every KMS action",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:*"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
        (
            "a wildcard principal, the policy action alone",
            policy(json!([allow(
                json!("*"),
                json!("kms:PutKeyPolicy"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
        (
            "an action wildcard that covers the policy action",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:Put*"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
        (
            "a Resource naming this key rather than a wildcard",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:*"),
                json!(key_arn),
                Value::Null
            )])),
            true,
        ),
        (
            "three statements, one of which keeps the path",
            policy(json!([
                allow(
                    json!({"Service": "logs.amazonaws.com"}),
                    json!("kms:Decrypt"),
                    json!("*"),
                    Value::Null
                ),
                allow(
                    json!({"AWS": root.clone()}),
                    json!(["kms:GetKeyPolicy", "kms:PutKeyPolicy"]),
                    json!("*"),
                    Value::Null
                ),
                allow(
                    json!({"AWS": caller.clone()}),
                    json!("kms:Encrypt"),
                    json!("*"),
                    Value::Null
                ),
            ])),
            true,
        ),
        // Leaves no path: the action, the resource, the principal or the condition
        // excludes a later policy write by this caller.
        (
            "every KMS action, to another principal entirely",
            policy(json!([allow(
                json!({"AWS": format!("arn:aws:iam::{}:user/Someone", engine::emulator_account_id())}),
                json!("kms:*"),
                json!("*"),
                Value::Null
            )])),
            false,
        ),
        (
            "every cryptographic action, to the account root",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!(["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey*"]),
                json!("*"),
                Value::Null
            )])),
            false,
        ),
        (
            "an action wildcard that stops short of the policy action",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:Get*"),
                json!("*"),
                Value::Null
            )])),
            false,
        ),
        (
            "every KMS action, scoped to ANOTHER key",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:*"),
                json!(other_key_arn),
                Value::Null
            )])),
            false,
        ),
        (
            "the policy action, gated on a condition the request cannot satisfy",
            policy(json!([allow(
                json!({"AWS": root.clone()}),
                json!("kms:*"),
                json!("*"),
                json!({"StringEquals": {"kms:ViaService": format!("s3.{REGION}.amazonaws.com")}})
            )])),
            false,
        ),
        (
            "the policy action allowed and then explicitly denied",
            policy(json!([
                allow(
                    json!({"AWS": root.clone()}),
                    json!("kms:*"),
                    json!("*"),
                    Value::Null
                ),
                {
                    "Sid": "DenyPolicyWrites",
                    "Effect": "Deny",
                    "Principal": {"AWS": root},
                    "Action": "kms:PutKeyPolicy",
                    "Resource": "*"
                },
            ])),
            false,
        ),
        (
            "a document whose only statement grants nothing to anybody",
            policy(json!([{
                "Sid": "DenyEverything",
                "Effect": "Deny",
                "Principal": "*",
                "Action": "kms:*",
                "Resource": "*"
            }])),
            false,
        ),
    ]
}

/// The document `GetKeyPolicy` serves for `key_id` in [`REGION`].
async fn stored_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
) -> String {
    let answer = ok(router, "GetKeyPolicy", &json!({"KeyId": key_id})).await;
    text(case, &answer, "Policy")
}

/// `PutKeyPolicy` attaches a document that keeps the policy path open and refuses one
/// that closes it — leaving, on a refusal, exactly the document the key already had.
#[tokio::test]
async fn put_key_policy_refuses_a_document_that_locks_the_caller_out() {
    let router = router();
    let under_test = create_key(&router, &json!({"Description": "lockout"})).await;
    let key_id = text("setup", &under_test, "KeyId");
    let key_arn = text("setup", &under_test, "Arn");
    let other = create_key(&router, &json!({"Description": "another key"})).await;
    let other_arn = text("setup", &other, "Arn");

    // The document in force at the start of every row, which does keep the path open.
    let kept = policy(json!([allow(
        json!({"AWS": account_root()}),
        json!("kms:*"),
        json!("*"),
        Value::Null
    )]));
    let answer = ok(
        &router,
        "PutKeyPolicy",
        &json!({"KeyId": &key_id, "Policy": &kept}),
    )
    .await;
    assert_empty("setup", "PutKeyPolicy", &answer);

    for (case, document, reaches_caller) in rows(&key_arn, &other_arn) {
        let request = json!({"KeyId": &key_id, "Policy": &document});
        if reaches_caller {
            let answer = ok(&router, "PutKeyPolicy", &request).await;
            assert_empty(case, "PutKeyPolicy", &answer);
            assert_eq!(
                stored_policy(&router, case, &key_id).await,
                reported_policy(&document),
                "case {case:?}: the attached document must be the one served back"
            );
            // Put the opening document back, so the next row starts from the same place.
            ok(
                &router,
                "PutKeyPolicy",
                &json!({"KeyId": &key_id, "Policy": &kept}),
            )
            .await;
            continue;
        }

        let (status, code, message) = refused(&router, REGION, "PutKeyPolicy", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code:?}"
        );
        assert_eq!(
            message, LOCKOUT_MESSAGE,
            "case {case:?}: expected the safety check's own sentence, got {message:?}"
        );
        assert_eq!(
            stored_policy(&router, case, &key_id).await,
            reported_policy(&kept),
            "case {case:?}: a refused write must leave the stored policy alone"
        );

        // The SAME document is attached when the request asks for the check to be
        // bypassed, which is the only thing that differs between the two calls.
        let mut bypassing = request.clone();
        bypassing["BypassPolicyLockoutSafetyCheck"] = json!(true);
        let answer = ok(&router, "PutKeyPolicy", &bypassing).await;
        assert_empty(case, "PutKeyPolicy", &answer);
        assert_eq!(
            stored_policy(&router, case, &key_id).await,
            reported_policy(&document),
            "case {case:?}: a bypassing write must attach the document"
        );
        ok(
            &router,
            "PutKeyPolicy",
            &json!({"KeyId": &key_id, "Policy": &kept}),
        )
        .await;
    }
}

/// `CreateKey` applies the same check to the `Policy` it is given — and mints no key when
/// it refuses.
#[tokio::test]
async fn create_key_refuses_a_policy_that_locks_the_caller_out() {
    let router = router();
    // A key that exists only so a row can scope its `Resource` to another key.
    let other = create_key(&router, &json!({"Description": "another key"})).await;
    let other_arn = text("setup", &other, "Arn");
    let keys_before = ok(&router, "ListKeys", &json!({})).await["Keys"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    let mut minted = 0usize;

    // A `CreateKey` does not know the key's ARN in advance, so the row scoped to THIS
    // key is dropped from the table here; every other row holds unchanged.
    for (case, document, reaches_caller) in rows("", &other_arn) {
        if case == "a Resource naming this key rather than a wildcard" {
            continue;
        }
        let request = json!({"Description": case, "Policy": &document});
        if reaches_caller {
            let metadata = create_key(&router, &request).await;
            minted += 1;
            let key_id = text(case, &metadata, "KeyId");
            assert_eq!(
                stored_policy(&router, case, &key_id).await,
                reported_policy(&document),
                "case {case:?}: the created key must serve the document it was given"
            );
            continue;
        }

        let (status, code, message) = refused(&router, REGION, "CreateKey", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code:?}"
        );
        assert_eq!(
            message, LOCKOUT_MESSAGE,
            "case {case:?}: expected the safety check's own sentence, got {message:?}"
        );

        // The same document creates a key when the request bypasses the check.
        let mut bypassing = request.clone();
        bypassing["BypassPolicyLockoutSafetyCheck"] = json!(true);
        let metadata = create_key(&router, &bypassing).await;
        minted += 1;
        let key_id = text(case, &metadata, "KeyId");
        assert_eq!(
            stored_policy(&router, case, &key_id).await,
            reported_policy(&document),
            "case {case:?}: a bypassing create must attach the document"
        );
    }

    // Every refused create minted nothing: the store holds exactly the keys the served
    // calls made.
    let keys_after = ok(&router, "ListKeys", &json!({})).await["Keys"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        keys_after,
        keys_before + minted,
        "a refused CreateKey must mint no key: expected {} keys, got {keys_after}",
        keys_before + minted
    );
}

/// `ReplicateKey` applies the same check to the replica's own `Policy` — and creates no
/// replica when it refuses.
#[tokio::test]
async fn replicate_key_refuses_a_policy_that_locks_the_caller_out() {
    let router = router();

    // Rows: one document that keeps the policy path open and one that does not, each
    // attached to its own primary key so the refused replica can be retried.
    let open = policy(json!([allow(
        json!({"AWS": account_root()}),
        json!("kms:*"),
        json!("*"),
        Value::Null
    )]));
    let closed = policy(json!([allow(
        json!({"AWS": account_root()}),
        json!("kms:Decrypt"),
        json!("*"),
        Value::Null
    )]));

    for (case, document, reaches_caller) in [
        ("a replica policy that reaches the caller", &open, true),
        ("a replica policy that locks the caller out", &closed, false),
    ] {
        let primary = create_key(
            &router,
            &json!({"Description": case, "MultiRegion": true}),
        )
        .await;
        let key_id = text(case, &primary, "KeyId");
        let request = json!({
            "KeyId": &key_id,
            "ReplicaRegion": REPLICA_REGION,
            "Policy": document,
        });
        if reaches_caller {
            let answer = ok(&router, "ReplicateKey", &request).await;
            assert_eq!(
                text(case, &answer, "ReplicaPolicy"),
                reported_policy(document),
                "case {case:?}: the replica must carry the document the request gave"
            );
            continue;
        }

        let (status, code, message) = refused(&router, REGION, "ReplicateKey", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code:?}"
        );
        assert_eq!(
            message, LOCKOUT_MESSAGE,
            "case {case:?}: expected the safety check's own sentence, got {message:?}"
        );
        // Nothing was created: the replica Region holds no key of this set yet, which is
        // why the retry below succeeds rather than answering `AlreadyExistsException`.
        let (status, payload) = call(
            &router,
            REPLICA_REGION,
            "DescribeKey",
            &json!({"KeyId": &key_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused replicate must create no replica, got {payload} ({})",
            error_code(&payload)
        );

        let mut bypassing = request.clone();
        bypassing["BypassPolicyLockoutSafetyCheck"] = json!(true);
        let answer = ok(&router, "ReplicateKey", &bypassing).await;
        assert_eq!(
            text(case, &answer, "ReplicaPolicy"),
            reported_policy(document),
            "case {case:?}: a bypassing replicate must attach the document"
        );
        let replica = ok_in(
            &router,
            REPLICA_REGION,
            "GetKeyPolicy",
            &json!({"KeyId": &key_id}),
        )
        .await;
        assert_eq!(
            text(case, &replica, "Policy"),
            reported_policy(document),
            "case {case:?}: the replica's endpoint must serve the replica's own document"
        );
    }
}

/// The check is about the DOCUMENT, not about the request's other members: a document
/// that keeps the path open is attached whether or not the request bypasses the check,
/// and a document that is not a key policy document at all is refused by the criterion
/// that judges its form — which carries no message, where the lockout refusal carries
/// the safety check's sentence.
#[tokio::test]
async fn the_two_refusals_a_key_policy_document_meets_are_worded_apart() {
    let router = router();
    let under_test = create_key(&router, &json!({"Description": "two refusals"})).await;
    let key_id = text("setup", &under_test, "KeyId");

    // Rows: the document, and whether it is a key policy document at all.
    let cases: Vec<(&str, String, bool)> = vec![
        ("not a document at all", "{".to_string(), false),
        (
            "a statement naming no principal",
            policy(json!([{"Effect": "Allow", "Action": "kms:*", "Resource": "*"}])),
            false,
        ),
        (
            "a well-formed document that locks the caller out",
            policy(json!([allow(
                json!({"AWS": account_root()}),
                json!("kms:Decrypt"),
                json!("*"),
                Value::Null
            )])),
            true,
        ),
    ];

    for (case, document, well_formed) in cases {
        let (status, code, message) = refused(
            &router,
            REGION,
            "PutKeyPolicy",
            &json!({"KeyId": &key_id, "Policy": &document}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: both refusals carry one code, got {code:?}"
        );
        if well_formed {
            assert_eq!(
                message, LOCKOUT_MESSAGE,
                "case {case:?}: expected the safety check's own sentence, got {message:?}"
            );
        } else {
            assert!(
                message.is_empty(),
                "case {case:?}: a document KMS cannot read is refused with no message, got \
                 {message:?}"
            );
        }

        // Bypassing the safety check does not make a malformed document acceptable: the
        // form is judged whatever the request asks for.
        let (status, payload) = call(
            &router,
            REGION,
            "PutKeyPolicy",
            &json!({
                "KeyId": &key_id,
                "Policy": &document,
                "BypassPolicyLockoutSafetyCheck": true,
            }),
        )
        .await;
        if well_formed {
            assert_eq!(
                status, 200,
                "case {case:?}: a bypassing write must attach the document, got {payload}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a malformed document is refused whatever the bypass says, \
                 got {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "MalformedPolicyDocumentException",
                "case {case:?}: expected MalformedPolicyDocumentException, got {payload}"
            );
            assert!(
                error_message(&payload).is_empty(),
                "case {case:?}: a document KMS cannot read is refused with no message, got \
                 {payload}"
            );
        }
    }
}
