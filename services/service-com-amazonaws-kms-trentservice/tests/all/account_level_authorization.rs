// SPDX-License-Identifier: CC0-1.0

//! The authorization of the KMS operations that name NO key, over the awsJson1_1 wire.
//!
//! The rule under test: an operation that reaches no KMS key has no key policy to ask, so
//! its permission comes from the calling principal's identity-based policies alone —
//! "Required permissions: kms:<Operation> (IAM policy)" — and a principal whose identity
//! policies do not allow `kms:<Operation>` is refused `AccessDeniedException` (HTTP 400)
//! naming the principal, the action and the service's wildcard ARN in the Region the
//! request was signed for, having read or written nothing.
//!
//! Beside it, the composition a KEY-scoped request follows, which is the other half of
//! the same decision: KMS opts out of the same-account identity-OR-resource union, so the
//! KEY POLICY must reach the caller whatever its identity policy says, and an identity
//! policy becomes effective only once the key policy delegates to the account root.
//!
//! The decision is the real one: the principals below are made with IAM's own operations
//! over the same engine, through the harness `tests/all/grant_authorization.rs` holds, so
//! the documents this service reads are the ones IAM wrote.
//!
//! The tables differ along what the rule generalises over:
//!
//! * the operation — every implemented operation of this service that names no key, each
//!   named in the refusal as its own `kms:` action;
//! * which authority is asked — no policy at all, an identity policy allowing the action,
//!   one allowing another action, one allowing the action on a narrower resource, an
//!   explicit deny, and the account's own published credential;
//! * for the key-scoped half, whether the key policy names the principal, delegates to the
//!   account root, or does neither — and the same policy withdrawn again.

use serde_json::{json, Value};

use super::grant_authorization::{
    account_root_statement, allow_to, assert_denied, identity_allow, policy, Emulator,
    ACCOUNT_KEY,
};
use super::harness::{b64, error_code, reported_policy, text, REGION};

/// The payload the cryptographic legs below seal, which the issue's own reproduction
/// seals: 16 bytes.
const PAYLOAD: &[u8] = b"sixteen bytes!!!";

/// The operations of this service that name no KMS key, with a request body that is valid
/// for each. Source: each operation's own "Required permissions: kms:<Operation> (IAM
/// policy)" sentence in the model — `smithy_query(query="doc",shape="ListAliases")` for
/// the first, and the same sentence in each of the others' documentation — which is what
/// marks an operation as one no key policy governs.
fn account_level_operations(account: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListAliases", json!({"Limit": 1})),
        ("ListKeys", json!({})),
        ("GenerateRandom", json!({"NumberOfBytes": 16})),
        ("CreateKey", json!({"Description": "account level"})),
        ("DescribeCustomKeyStores", json!({})),
        (
            "ListRetirableGrants",
            json!({"RetiringPrincipal": format!("arn:aws:iam::{account}:role/Retirer")}),
        ),
    ]
}

/// The ARN an account-level action is judged against: every KMS resource of this account
/// in `region`, which is what an action carrying no resource type is written against in a
/// policy.
fn service_wildcard(region: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{account}:*",
        account = engine::emulator_account_id()
    )
}

/// The sentence AWS refuses a request no identity-based policy allows with.
fn no_identity_policy(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The sentence AWS refuses a request the RESOURCE's own policy withheld with.
fn no_resource_policy(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action"
    )
}

// ---------------------------------------------------------------------------
// The operations that name no key
// ---------------------------------------------------------------------------

/// Every operation that names no key is served exactly when the caller's identity-based
/// policies allow its action, and refused with AWS's denial sentence when they do not —
/// for every kind of caller in the table.
#[tokio::test]
async fn an_operation_that_names_no_key_is_gated_by_the_identity_policy_alone() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let wildcard = service_wildcard(REGION);
    let operations = account_level_operations(account);

    // A key the narrow identity policy below names, so a policy scoped to one key is a
    // real grant on a key and still no grant for an account-level action.
    let (_, key_arn) = emulator.key(None).await;

    let (every_action, every_action_arn) = emulator
        .iam_user("EveryKmsAction", Some(&identity_allow(json!("kms:*"), "*")))
        .await;
    let (bare, bare_arn) = emulator.iam_user("NoPolicyAtAll", None).await;
    let (key_scoped, key_scoped_arn) = emulator
        .iam_user(
            "KeyScoped",
            Some(&identity_allow(json!("kms:*"), &key_arn)),
        )
        .await;
    let (one_action, one_action_arn) = emulator
        .iam_user(
            "ListAliasesOnly",
            Some(&identity_allow(json!("kms:ListAliases"), "*")),
        )
        .await;
    let (denied, denied_arn) = emulator
        .iam_user(
            "DeniedEverything",
            Some(
                &json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {"Effect": "Allow", "Action": "kms:*", "Resource": "*"},
                        {"Effect": "Deny", "Action": "kms:*", "Resource": "*"},
                    ],
                })
                .to_string(),
            ),
        )
        .await;
    let ambient_arn = format!("arn:aws:iam::{account}:user/emulator");

    // (case, the caller's access key, the caller's ARN, what the authority answers)
    #[derive(Clone, Copy)]
    enum Verdict {
        Served,
        NoIdentityPolicy,
        ExplicitDeny,
        /// Served for exactly one action, refused for every other.
        OnlyListAliases,
    }
    let callers: Vec<(&str, &str, &str, Verdict)> = vec![
        (
            "the account's own published credential, which no identity policy governs",
            ACCOUNT_KEY,
            &ambient_arn,
            Verdict::Served,
        ),
        (
            "an IAM user whose inline policy allows every KMS action",
            &every_action,
            &every_action_arn,
            Verdict::Served,
        ),
        (
            "an IAM user with no policy at all",
            &bare,
            &bare_arn,
            Verdict::NoIdentityPolicy,
        ),
        (
            "an IAM user whose policy allows every KMS action on one key only",
            &key_scoped,
            &key_scoped_arn,
            Verdict::NoIdentityPolicy,
        ),
        (
            "an IAM user whose policy allows one account-level action",
            &one_action,
            &one_action_arn,
            Verdict::OnlyListAliases,
        ),
        (
            "an IAM user whose policy denies every KMS action beside an allow",
            &denied,
            &denied_arn,
            Verdict::ExplicitDeny,
        ),
    ];

    for (case, access_key, caller_arn, verdict) in callers {
        for (operation, body) in &operations {
            let action = format!("kms:{operation}");
            let case = format!("{case}, calling {operation}");
            let (status, payload) = emulator.call(access_key, operation, body).await;
            let served = match verdict {
                Verdict::Served => true,
                Verdict::OnlyListAliases => *operation == "ListAliases",
                Verdict::NoIdentityPolicy | Verdict::ExplicitDeny => false,
            };
            if served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status} with {payload}"
                );
                continue;
            }
            let expected = match verdict {
                Verdict::ExplicitDeny => format!(
                    "User: {caller_arn} is not authorized to perform: {action} on resource: \
                     {wildcard} with an explicit deny in an identity-based policy"
                ),
                _ => no_identity_policy(caller_arn, &action, &wildcard),
            };
            assert_denied(&case, status, &payload, &expected);
        }
    }

    // Nothing a refused call would have written is there: the only keys of this Region are
    // the one the setup made and the ones the SERVED `CreateKey` calls minted, so the
    // refusals minted none.
    let listed = emulator.ok(ACCOUNT_KEY, "ListKeys", &json!({})).await;
    let minted = listed["Keys"]
        .as_array()
        .unwrap_or_else(|| panic!("ListKeys reports a Keys list, got {listed}"))
        .len();
    assert_eq!(
        minted, 3,
        "a refused CreateKey must mint no key: expected the setup key beside the two \
         served CreateKey calls, got {listed}"
    );
}

/// A refused account-level call is refused in every Region the same way, and the refusal
/// names the Region's own wildcard ARN — the resource the request was judged against.
#[tokio::test]
async fn an_account_level_refusal_names_the_region_it_was_signed_for() {
    let emulator = Emulator::new();
    let (bare, bare_arn) = emulator.iam_user("RegionProbe", None).await;

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, payload) = emulator
            .call_in(region, &bare, "ListAliases", &json!({}))
            .await;
        assert_denied(
            region,
            status,
            &payload,
            &no_identity_policy(&bare_arn, "kms:ListAliases", &service_wildcard(region)),
        );
    }
}

/// The gate runs BEFORE the resource is looked up, so an account-level operation that
/// names a custom key store refuses an unauthorized caller without disclosing whether
/// that key store exists — while an authorized caller gets the modelled not-found.
///
/// The rows are the custom-key-store operations this service implements, each of which
/// names no KMS key and so is governed by the identity policy alone ("Required
/// permissions: kms:<Operation> (IAM policy)").
#[tokio::test]
async fn a_custom_key_store_operation_refuses_before_it_looks_the_store_up() {
    let emulator = Emulator::new();
    let wildcard = service_wildcard(REGION);
    let (bare, bare_arn) = emulator.iam_user("NoKeyStorePermission", None).await;
    // An id of the shape this service mints, which no key store of this account holds.
    let absent = json!({"CustomKeyStoreId": "cks-1234567890abcdef0"});

    for operation in [
        "DescribeCustomKeyStores",
        "ConnectCustomKeyStore",
        "DisconnectCustomKeyStore",
        "UpdateCustomKeyStore",
    ] {
        let (status, payload) = emulator.call(&bare, operation, &absent).await;
        assert_denied(
            operation,
            status,
            &payload,
            &no_identity_policy(&bare_arn, &format!("kms:{operation}"), &wildcard),
        );

        // The account's own credential reaches the lookup, which is what shows the
        // refusal above was the gate and not the absent key store. Every one of these
        // operations SELECTS by the id, so an id nothing holds is the modelled
        // not-found — the listing is not answered empty either.
        let (status, payload) = emulator.call(ACCOUNT_KEY, operation, &absent).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "CustomKeyStoreNotFoundException"),
            "case {operation}: an authorized caller must reach the key-store lookup, got \
             {status} with {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// The key-scoped half: the key policy is required
// ---------------------------------------------------------------------------

/// A key-scoped request is authorized by the KEY POLICY: a principal the policy names is
/// served, and a principal whose IDENTITY policy allows the action is refused while the
/// key policy neither names it nor delegates to the account root — then served once the
/// policy delegates, and refused again when the delegation is withdrawn.
///
/// The rows are the three forms a key policy takes for the second principal (names it,
/// delegates to the account, neither) walked in the order a client administering a key
/// walks them, which is what makes the refusal observable as the delegation goes away.
#[tokio::test]
async fn a_key_policy_must_reach_the_caller_whatever_the_identity_policy_says() {
    let emulator = Emulator::new();

    let (named_user, named_arn) = emulator.iam_user("NamedByTheKeyPolicy", None).await;
    // The key policy that names ONE principal and delegates to nobody: the account's own
    // root is deliberately absent, which is what makes an identity policy ineffective —
    // and leaves this principal the only one the key grants anything to.
    let withheld = policy(json!([allow_to(
        &named_arn,
        json!(["kms:Encrypt", "kms:Decrypt"])
    )]));
    let (key_id, key_arn) = emulator.key(Some(&withheld)).await;

    let (identity_user, identity_arn) = emulator
        .iam_user(
            "AllowedByItsIdentityPolicy",
            Some(&identity_allow(json!("kms:Encrypt"), &key_arn)),
        )
        .await;
    // The same policy, plus the account delegation: "Principal: the account root, Action
    // kms:*", which is what makes the identity policy effective.
    let delegating = policy(json!([
        allow_to(&named_arn, json!(["kms:Encrypt", "kms:Decrypt"])),
        account_root_statement(),
    ]));

    // The principal the key policy names is served, and the ciphertext opens to exactly
    // the plaintext under that key — read back through the `Decrypt` the same policy
    // allows it, since a policy that delegates to nobody reaches nobody else either.
    let sealed = emulator
        .ok(
            &named_user,
            "Encrypt",
            &json!({"KeyId": key_arn, "Plaintext": b64(PAYLOAD)}),
        )
        .await;
    assert_eq!(
        sealed["KeyId"],
        json!(key_arn),
        "the key policy's own principal must encrypt under the key it names, got {sealed}"
    );
    let opened = emulator
        .ok(
            &named_user,
            "Decrypt",
            &json!({"CiphertextBlob": text("round trip", &sealed, "CiphertextBlob")}),
        )
        .await;
    assert_eq!(
        opened["Plaintext"],
        json!(b64(PAYLOAD)),
        "the ciphertext must open to exactly the plaintext, got {opened}"
    );

    // (case, the key policy in force, whether the identity-only principal is served)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a key policy that neither names it nor delegates", &withheld, false),
        ("a key policy that delegates to the account root", &delegating, true),
        ("the non-delegating policy put back", &withheld, false),
    ];

    for (case, document, served) in rows {
        // `withheld` names only another principal, so it leaves the caller no path to a
        // later `PutKeyPolicy`: this test is about whose requests the document serves,
        // not about the policy-lockout safety check, so it is attached the way the model
        // provides for.
        emulator
            .ok(
                ACCOUNT_KEY,
                "PutKeyPolicy",
                &json!({
                    "KeyId": key_id,
                    "Policy": document,
                    "BypassPolicyLockoutSafetyCheck": true,
                }),
            )
            .await;

        let (status, payload) = emulator
            .call(
                &identity_user,
                "Encrypt",
                &json!({"KeyId": key_arn, "Plaintext": b64(PAYLOAD)}),
            )
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the identity policy to be effective, got {status} \
                 with {payload}"
            );
            assert!(
                payload["CiphertextBlob"].is_string(),
                "case {case:?}: a served Encrypt answers a ciphertext, got {payload}"
            );
        } else {
            assert_denied(
                case,
                status,
                &payload,
                &no_resource_policy(&identity_arn, "kms:Encrypt", &key_arn),
            );
            assert!(
                payload["CiphertextBlob"].is_null(),
                "case {case:?}: a refused Encrypt produces no ciphertext, got {payload}"
            );
        }

        // Whatever the key policy says about the second principal, the one it names
        // throughout is still served: the policy edits withdrew the DELEGATION and
        // nothing else.
        emulator
            .ok(
                &named_user,
                "Encrypt",
                &json!({"KeyId": key_arn, "Plaintext": b64(PAYLOAD)}),
            )
            .await;

        // An account-level action is decided by the identity policy alone, so the key
        // policy's delegation never reaches one: this principal's policy names
        // `kms:Encrypt` on one key, which is no grant for a listing.
        let (status, payload) = emulator
            .call(&identity_user, "ListAliases", &json!({"Limit": 1}))
            .await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "AccessDeniedException"),
            "case {case:?}: ListAliases is gated by the identity policy whatever the key \
             policy says, got {status} with {payload}"
        );
        // The sentence travels in `Message`: an authorization failure is answered by
        // AWS's authorization layer, not by a modelled KMS error, and that is the member
        // name it uses.
        assert_eq!(
            payload["Message"].as_str(),
            Some(
                no_identity_policy(&identity_arn, "kms:ListAliases", &service_wildcard(REGION))
                    .as_str()
            ),
            "case {case:?}: the listing refusal names the service wildcard ARN, got {payload}"
        );
    }

    // The account's own credential administered the key throughout, so the policy in force
    // is the one the last `PutKeyPolicy` wrote.
    let read_back = emulator
        .ok(
            ACCOUNT_KEY,
            "GetKeyPolicy",
            &json!({"KeyId": key_id, "PolicyName": "default"}),
        )
        .await;
    assert_eq!(
        read_back["Policy"],
        json!(reported_policy(&withheld)),
        "the policy in force must be the one the last PutKeyPolicy wrote, got {read_back}"
    );
}
