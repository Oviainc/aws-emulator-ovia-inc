// SPDX-License-Identifier: CC0-1.0

//! A key policy's explicit `Deny` is final, and reaches only the key that carries it —
//! over the awsJson1_1 wire.
//!
//! The rule under test: a request for a key-scoped KMS action is refused
//! `AccessDeniedException` (HTTP 400) when a statement of THAT key's own key policy denies
//! the action to the calling principal, whatever the caller's identity-based policies
//! allow, and the refusal carries none of the operation's output. A key policy governs one
//! key, so the deny governs that key alone: the same caller asking the same action on a
//! key whose policy carries no such statement is served, and a statement whose `Action` or
//! `Resource` element does not cover the request does not refuse it.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine — `CreateRole` + `PutRolePolicy` + `AssumeRole`,
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey` — through the harness
//! `tests/all/grant_authorization.rs` holds, so the documents this service reads are the
//! ones those services wrote, and the identity-based half of the decision really does
//! allow the action.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the spelling the deny names the caller by — the IAM role behind the session, the
//!   session ARN STS reports, the account's root ARN, and the bare account id (denying an
//!   account denies each of its principals);
//! * the form of its `Action` element — one action, a list of two, a wildcard over a
//!   family of actions, and every KMS action;
//! * the `Resource` element it is scoped by — every resource, the key carrying the policy,
//!   a wildcard ARN over the account's keys, and a SIBLING key's ARN, which the request's
//!   key is not;
//! * the operation asked — `Decrypt`, `Encrypt`, `GenerateDataKey`,
//!   `GenerateDataKeyWithoutPlaintext` and `CreateGrant`, each named in the refusal as its
//!   own `kms:` action, and `Decrypt` reached through the ciphertext alone, so the policy
//!   consulted is the one of the key that SEALED it;
//! * the kind of caller and the breadth of its identity allow — an assumed-role session
//!   whose role allows every KMS action on every resource, and an IAM user signing with a
//!   long-term key whose policy allows the one action on every resource;
//! * when the deny was written — by a `PutKeyPolicy` after the ciphertext was sealed, and
//!   lifted by a later one, which is what shows the document is read at the instant of the
//!   call.

use serde_json::{json, Value};

use super::grant_authorization::{
    account_root_statement, assert_denied, identity_allow, policy, Emulator, ACCOUNT_KEY,
};
use super::harness::{b64, reported_policy, text};

/// The plaintext every case below seals and opens again.
const PAYLOAD: &[u8] = b"key policy deny scope payload";

/// The key-scoped operations this suite drives, each one the service authorizes against
/// the key policy of the key it reaches, and each named in a policy as `kms:<Operation>`.
const OPERATIONS: [&str; 5] = [
    "Decrypt",
    "Encrypt",
    "GenerateDataKey",
    "GenerateDataKeyWithoutPlaintext",
    "CreateGrant",
];

/// The members these operations answer their output in, none of which a refusal may carry.
const OUTPUT_MEMBERS: [&str; 5] = [
    "Plaintext",
    "CiphertextBlob",
    "GrantId",
    "GrantToken",
    "KeyId",
];

/// Which resources a deny statement's `Resource` element is scoped to.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// Every resource — `"Resource": "*"`.
    Every,
    /// The ARN of the key whose policy carries the statement.
    ThisKey,
    /// A wildcard ARN pattern over every key of this account, in any Region.
    EveryKeyOfTheAccount,
    /// The ARN of the OTHER key, which the request's key is not — so the statement does
    /// not cover the request at all.
    TheSiblingKey,
}

/// A key-policy statement denying `actions` to `principal` on `resource`.
fn deny_to(principal: &str, actions: Value, resource: &str) -> Value {
    json!({
        "Effect": "Deny",
        "Principal": {"AWS": principal},
        "Action": actions,
        "Resource": resource,
    })
}

/// The sentence AWS refuses a request a resource-based policy explicitly denied with.
fn explicit_deny(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in a resource-based policy"
    )
}

/// A request body that is valid for `operation` against the key `key_id`, with
/// `ciphertext` the envelope that key sealed — which is all a `Decrypt` needs, since the
/// envelope names the key that sealed it.
fn request(case: &str, operation: &str, key_id: &str, ciphertext: &str) -> Value {
    let account = engine::emulator_account_id();
    match operation {
        "Decrypt" => json!({"CiphertextBlob": ciphertext}),
        "Encrypt" => json!({"KeyId": key_id, "Plaintext": b64(PAYLOAD)}),
        "GenerateDataKey" | "GenerateDataKeyWithoutPlaintext" => {
            json!({"KeyId": key_id, "KeySpec": "AES_256"})
        }
        "CreateGrant" => json!({
            "KeyId": key_id,
            "GranteePrincipal": format!("arn:aws:iam::{account}:role/Grantee"),
            "Operations": ["Decrypt"],
        }),
        other => panic!("case {case:?}: {other} is not an operation this suite drives"),
    }
}

/// Seal [`PAYLOAD`] under `key_id` as the account's own credential — the administrator's
/// call, made before any deny is written — and answer the ciphertext it produced.
async fn seal(emulator: &Emulator, case: &str, key_id: &str) -> String {
    let sealed = emulator
        .ok(
            ACCOUNT_KEY,
            "Encrypt",
            &json!({"KeyId": key_id, "Plaintext": b64(PAYLOAD)}),
        )
        .await;
    text(case, &sealed, "CiphertextBlob")
}

// ---------------------------------------------------------------------------
// The deny beats the identity allow, on the key that carries it
// ---------------------------------------------------------------------------

/// A caller whose identity policy allows every KMS action on every resource is refused on
/// the key whose policy denies it the action, and served on a sibling key whose policy
/// carries no such statement — for every spelling, action form and resource scope in the
/// table.
#[tokio::test]
async fn a_key_policy_deny_beats_an_identity_allow_on_that_key_alone() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let role_name = "AllowedEverythingByItsIdentityPolicy";
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    // The identity-based half of the decision allows every KMS action on every resource,
    // which is the position the rule is about: only the key policy can refuse.
    let (session, session_arn) = emulator
        .assumed_role(role_name, Some(&identity_allow(json!("kms:*"), "*")))
        .await;

    // (case, the principal the deny names, its Action element, its Resource scope, the
    // operations it therefore refuses)
    let rows: Vec<(&str, String, Value, Scope, Vec<&str>)> = vec![
        (
            "the role behind the session, one action, every resource",
            role_arn.clone(),
            json!("kms:Decrypt"),
            Scope::Every,
            vec!["Decrypt"],
        ),
        (
            "the session ARN itself, one action",
            session_arn.clone(),
            json!("kms:Decrypt"),
            Scope::Every,
            vec!["Decrypt"],
        ),
        (
            "the role, a list of two actions",
            role_arn.clone(),
            json!(["kms:Decrypt", "kms:CreateGrant"]),
            Scope::Every,
            vec!["Decrypt", "CreateGrant"],
        ),
        (
            "the role, every KMS action, scoped to the key carrying the policy",
            role_arn.clone(),
            json!("kms:*"),
            Scope::ThisKey,
            OPERATIONS.to_vec(),
        ),
        (
            "the account root, one action, scoped by a wildcard over the account's keys",
            format!("arn:aws:iam::{account}:root"),
            json!("kms:Encrypt"),
            Scope::EveryKeyOfTheAccount,
            vec!["Encrypt"],
        ),
        (
            "the bare account id, one action naming no wildcard",
            account.to_string(),
            json!("kms:GenerateDataKey"),
            Scope::Every,
            vec!["GenerateDataKey"],
        ),
        (
            "the role, a wildcard over one family of actions",
            role_arn.clone(),
            json!("kms:GenerateDataKey*"),
            Scope::Every,
            vec!["GenerateDataKey", "GenerateDataKeyWithoutPlaintext"],
        ),
        (
            "the role, every KMS action, scoped to the SIBLING key instead",
            role_arn.clone(),
            json!("kms:*"),
            Scope::TheSiblingKey,
            Vec::new(),
        ),
    ];

    for (case, principal, actions, scope, denied_operations) in rows {
        // Two sibling keys, each created with the default key policy — which delegates to
        // the account root and so makes the caller's identity policy effective on both.
        let (denied_key, denied_arn) = emulator.key(None).await;
        let (sibling_key, sibling_arn) = emulator.key(None).await;
        let denied_ciphertext = seal(&emulator, case, &denied_key).await;
        let sibling_ciphertext = seal(&emulator, case, &sibling_key).await;

        let resource = match scope {
            Scope::Every => "*".to_string(),
            Scope::ThisKey => denied_arn.clone(),
            Scope::EveryKeyOfTheAccount => format!("arn:aws:kms:*:{account}:key/*"),
            Scope::TheSiblingKey => sibling_arn.clone(),
        };
        // The deny is written to ONE of the two keys, after both ciphertexts exist.
        emulator
            .ok(
                ACCOUNT_KEY,
                "PutKeyPolicy",
                &json!({
                    "KeyId": denied_key,
                    "Policy": policy(json!([
                        account_root_statement(),
                        deny_to(&principal, actions.clone(), &resource),
                    ])),
                }),
            )
            .await;

        for operation in OPERATIONS {
            let action = format!("kms:{operation}");

            let here = format!("{case}, calling {operation} on the key carrying the deny");
            let body = request(&here, operation, &denied_key, &denied_ciphertext);
            let (status, payload) = emulator.call(&session, operation, &body).await;
            if denied_operations.contains(&operation) {
                assert_denied(
                    &here,
                    status,
                    &payload,
                    &explicit_deny(&session_arn, &action, &denied_arn),
                );
                for member in OUTPUT_MEMBERS {
                    assert!(
                        payload[member].is_null(),
                        "case {here:?}: a refused call answers no {member}, got {payload:?}"
                    );
                }
            } else {
                assert_eq!(
                    status, 200,
                    "case {here:?}: the deny does not cover this request, so the identity \
                     policy serves it, got {status} with {payload:?}"
                );
            }

            // The same call on the sibling key, whose own policy carries no deny: a deny
            // lives in one key's policy and reaches no other key.
            let there = format!("{case}, calling {operation} on the sibling key");
            let body = request(&there, operation, &sibling_key, &sibling_ciphertext);
            let (status, payload) = emulator.call(&session, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {there:?}: a deny in another key's policy must not refuse this key, \
                 got {status} with {payload:?}"
            );
        }

        // A refused `CreateGrant` wrote nothing: the gate runs before the grant is minted.
        if denied_operations.contains(&"CreateGrant") {
            let listed = emulator
                .ok(ACCOUNT_KEY, "ListGrants", &json!({"KeyId": denied_key}))
                .await;
            assert_eq!(
                listed["Grants"],
                json!([]),
                "case {case:?}: a refused CreateGrant must mint no grant, got {listed:?}"
            );
        }

        // The sibling key still opens its ciphertext to exactly the payload, under its own
        // ARN: the deny on the other key changed no key material.
        let opened = emulator
            .ok(
                &session,
                "Decrypt",
                &json!({"CiphertextBlob": sibling_ciphertext}),
            )
            .await;
        assert_eq!(
            opened["Plaintext"],
            json!(b64(PAYLOAD)),
            "case {case:?}: the sibling ciphertext must open to exactly the plaintext, \
             got {opened:?}"
        );
        assert_eq!(
            opened["KeyId"],
            json!(sibling_arn),
            "case {case:?}: the sibling ciphertext opens under its own key, got {opened:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The document in force is the one read at the instant of the call
// ---------------------------------------------------------------------------

/// The deny is read from the key's stored policy on every call: the same `Decrypt` by the
/// same caller is served before the deny is written, refused while it is in force, and
/// served again once it is lifted — while the sibling key is served throughout.
///
/// The caller here is the other kind, an IAM user signing with a long-term access key, and
/// its identity policy allows exactly the one action on every resource: a narrower allow
/// than the table above, still powerless against the key policy's deny.
#[tokio::test]
async fn a_key_policy_deny_is_read_from_the_stored_policy_on_every_call() {
    let emulator = Emulator::new();
    let (user, user_arn) = emulator
        .iam_user(
            "AllowedDecryptEverywhere",
            Some(&identity_allow(json!("kms:Decrypt"), "*")),
        )
        .await;

    let (key_id, key_arn) = emulator.key(None).await;
    let (sibling_id, sibling_arn) = emulator.key(None).await;
    let ciphertext = seal(&emulator, "setup", &key_id).await;
    let sibling_ciphertext = seal(&emulator, "setup", &sibling_id).await;

    // The key policy with nothing but the account delegation, and the same document plus
    // the deny of this one principal.
    let delegating = policy(json!([account_root_statement()]));
    let denying = policy(json!([
        account_root_statement(),
        deny_to(&user_arn, json!("kms:Decrypt"), "*"),
    ]));

    // (case, the document written to the key, whether the user's Decrypt is served)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("the delegating policy, before any deny", &delegating, true),
        ("the deny a later PutKeyPolicy wrote", &denying, false),
        ("the deny lifted by the next PutKeyPolicy", &delegating, true),
    ];

    for (case, document, served) in rows {
        emulator
            .ok(
                ACCOUNT_KEY,
                "PutKeyPolicy",
                &json!({"KeyId": key_id, "Policy": document}),
            )
            .await;

        let (status, payload) = emulator
            .call(&user, "Decrypt", &json!({"CiphertextBlob": ciphertext}))
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the Decrypt to be served, got {status} with \
                 {payload:?}"
            );
            assert_eq!(
                payload["Plaintext"],
                json!(b64(PAYLOAD)),
                "case {case:?}: the ciphertext must open to exactly the plaintext, got \
                 {payload:?}"
            );
            assert_eq!(
                payload["KeyId"],
                json!(key_arn),
                "case {case:?}: the plaintext is answered under the key that sealed it, got \
                 {payload:?}"
            );
        } else {
            assert_denied(
                case,
                status,
                &payload,
                &explicit_deny(&user_arn, "kms:Decrypt", &key_arn),
            );
            assert!(
                payload["Plaintext"].is_null(),
                "case {case:?}: a refused Decrypt answers no Plaintext, got {payload:?}"
            );
        }

        // The sibling key carries no deny, so this caller's Decrypt on it is served at
        // every step of the sequence.
        let opened = emulator
            .ok(
                &user,
                "Decrypt",
                &json!({"CiphertextBlob": sibling_ciphertext}),
            )
            .await;
        assert_eq!(
            opened["KeyId"],
            json!(sibling_arn),
            "case {case:?}: the sibling key is served throughout, got {opened:?}"
        );
        assert_eq!(
            opened["Plaintext"],
            json!(b64(PAYLOAD)),
            "case {case:?}: the sibling ciphertext must open to exactly the plaintext, got \
             {opened:?}"
        );

        // The document the decision was made against is the one the last write stored,
        // read back through the implemented reader.
        let read_back = emulator
            .ok(
                ACCOUNT_KEY,
                "GetKeyPolicy",
                &json!({"KeyId": key_id, "PolicyName": "default"}),
            )
            .await;
        assert_eq!(
            read_back["Policy"],
            json!(reported_policy(document)),
            "case {case:?}: the policy in force must be the one the last PutKeyPolicy \
             wrote, got {read_back:?}"
        );
    }
}
