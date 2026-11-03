// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the key policy pair — `PutKeyPolicy` and the `GetKeyPolicy`
//! reader it round-trips through — driven over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: TrentService.<Operation>`) through the same multi-service router
//! the emulator boots.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one call:
//!
//! * **R1** — attaching a key policy REPLACES the document the key held, and the key
//!   serves that document back under the name `default`, rendered in the one layout KMS
//!   reports a key policy in (the exact text of that layout is pinned, document by
//!   document, in [`super::read_projection_fidelity`]); a key's policy is its own.
//! * **R2** — a key policy document is accepted exactly when it is an IAM JSON policy
//!   document whose every statement has an `Effect` of `Allow` or `Deny` and names one
//!   or more principals; anything else is a `MalformedPolicyDocumentException` and
//!   persists nothing. A statement missing `Action` or `Resource` is ACCEPTED, because
//!   "the `PutKeyPolicy` API request succeeds, even though the policy statement is
//!   ineffective"
//!   (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`).
//! * **R3** — `default` is the only key policy name a key has a policy under, so any
//!   other name is a `NotFoundException` on both the read and the write.
//! * **R4** — a stored key policy is bounded by KMS's key policy document quota
//!   (32 KB), past which the write is refused with `LimitExceededException`.
//! * **R5** — both operations resolve a `KeyId` in every form KMS accepts, and a
//!   policy write needs a key state that allows it: a key pending deletion refuses the
//!   write while still serving the read.

use serde_json::{json, Value};

use super::harness::{
    assert_empty, call, create_key, error_code, failure, failure_in, ok, ok_in, refused,
    reported_policy, router, text, REGION,
};

/// KMS's key policy document quota, "Key policy document size: 32 KB" (Quotas in the
/// Key Management Service Developer Guide) — mirrored from
/// `crate::state::MAX_KEY_POLICY_BYTES`, which is what the service enforces.
const MAX_KEY_POLICY_BYTES: usize = 32 * 1024;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attach `policy` to `key`, under `policy_name` when one is given, requiring the
/// empty 200 AWS documents for `PutKeyPolicy`.
///
/// The rows these tests attach are about the SHAPE of a key policy document, so many of
/// them leave the caller no path to a later `PutKeyPolicy` and the policy-lockout safety
/// check would refuse them. That check has its own tests
/// (`super::key_policy_lockout`), so the documents here are attached the way the model
/// provides for: `BypassPolicyLockoutSafetyCheck`.
async fn put_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key: &str,
    policy: &str,
    policy_name: Option<&str>,
) {
    let mut request = json!({
        "KeyId": key,
        "Policy": policy,
        "BypassPolicyLockoutSafetyCheck": true,
    });
    if let Some(policy_name) = policy_name {
        request["PolicyName"] = json!(policy_name);
    }
    let answer = ok(router, "PutKeyPolicy", &request).await;
    assert_empty(case, "PutKeyPolicy", &answer);
}

/// The document `GetKeyPolicy` serves for `key`, asserting it also reports the name it
/// was served under.
async fn get_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key: &str,
) -> String {
    get_policy_in(router, REGION, case, key).await
}

/// [`get_policy`], in `region`.
async fn get_policy_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    case: &str,
    key: &str,
) -> String {
    let answer = ok_in(router, region, "GetKeyPolicy", &json!({"KeyId": key})).await;
    let name = text(case, &answer, "PolicyName");
    assert_eq!(
        name, "default",
        "case {case:?}: a key serves its policy under the name default, got {name:?}"
    );
    text(case, &answer, "Policy")
}

/// One statement that allows `action` on `resource` to `principal` — the shape a key
/// policy statement takes.
fn statement(sid: &str, principal: Value, action: Value, resource: Value) -> Value {
    json!({
        "Sid": sid,
        "Effect": "Allow",
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    })
}

/// A key policy document carrying `statements`.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A valid key policy document of exactly `len` bytes, grown by padding a `Sid` — the
/// quota is about the SIZE of the document, so the padding is what varies.
fn policy_of_len(len: usize) -> String {
    let empty = policy(json!([statement(
        "",
        json!({"AWS": "*"}),
        json!("kms:*"),
        json!("*")
    )]));
    assert!(
        len >= empty.len(),
        "a key policy of {len} bytes is shorter than the smallest document this builds \
         ({} bytes)",
        empty.len()
    );
    let padded = policy(json!([statement(
        &"A".repeat(len - empty.len()),
        json!({"AWS": "*"}),
        json!("kms:*"),
        json!("*")
    )]));
    assert_eq!(
        padded.len(),
        len,
        "the padded key policy must be exactly {len} bytes"
    );
    padded
}

// ---------------------------------------------------------------------------
// R1: the round trip
// ---------------------------------------------------------------------------

/// R1: `PutKeyPolicy` replaces the key's policy, and `GetKeyPolicy` serves the stored
/// document back, rendered in KMS's own layout — for every shape a key policy document
/// takes.
///
/// The rows differ along what the rule generalises over: how many statements the
/// document carries, which of the two `Statement` forms it uses (one object, or a
/// list), the form of the `Principal` (a string, a map to one principal, a map to a
/// list, several kinds), the form of `Action` and `Resource` (a string or a list), the
/// `Effect`, which optional elements are present (`Sid`, `Condition`, no `Version`),
/// the whitespace (compact and pretty-printed, which carries newlines the modelled
/// pattern allows) and the characters (a Latin-1 value, the top of the modelled
/// `^[\u0009\u000A\u000D\u0020-\u00FF]+$` range).
///
/// Every row is attached to the SAME key in sequence, so each one also asserts that
/// the previous document is gone rather than merged with.
#[tokio::test]
async fn put_key_policy_replaces_the_document_get_key_policy_serves() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let metadata = create_key(&router, &json!({"Description": "policy round trip"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");

    // Rows: the case, the document, and the policy name the request carries (`None`
    // omits the member, which names `default` by default).
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "one statement, principal as a map",
            policy(json!([statement(
                "AllowRoot",
                json!({"AWS": root}),
                json!("kms:*"),
                json!("*")
            )])),
            None,
        ),
        (
            "the policy name spelled out",
            policy(json!([statement(
                "AllowRootNamed",
                json!({"AWS": root}),
                json!("kms:*"),
                json!("*")
            )])),
            Some("default"),
        ),
        (
            "one statement, principal as a bare string",
            policy(json!([statement(
                "AllowEveryone",
                json!("*"),
                json!("kms:Decrypt"),
                json!(key_arn)
            )])),
            None,
        ),
        (
            "Statement as a single object rather than a list",
            policy(statement(
                "SingleStatement",
                json!({"AWS": root}),
                json!("kms:PutKeyPolicy"),
                json!("*"),
            )),
            None,
        ),
        (
            "three statements, mixed effects",
            policy(json!([
                statement("One", json!({"AWS": root}), json!("kms:*"), json!("*")),
                {
                    "Sid": "Two",
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": ["kms:ScheduleKeyDeletion", "kms:DeleteAlias"],
                    "Resource": "*"
                },
                statement(
                    "Three",
                    json!({"Service": ["aiops.amazonaws.com", "logs.amazonaws.com"]}),
                    json!(["kms:Encrypt", "kms:Decrypt", "kms:GenerateDataKey*"]),
                    json!([key_arn.clone(), "*"]),
                ),
            ])),
            None,
        ),
        (
            "a statement carrying a Condition",
            policy(json!([{
                "Sid": "ViaService",
                "Effect": "Allow",
                "Principal": {"AWS": root},
                "Action": "kms:Decrypt",
                "Resource": "*",
                "Condition": {"StringEquals": {"kms:ViaService": "s3.us-east-1.amazonaws.com"}}
            }])),
            Some("default"),
        ),
        (
            "no Version, no Sid",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"CanonicalUser": "79a59df900b949e55d96a1e698fbaced"},
                "Action": "kms:DescribeKey",
                "Resource": "*"
            }]})
            .to_string(),
            None,
        ),
        (
            "a statement with no Action and no Resource, which AWS accepts as ineffective",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Allow", "Principal": {"AWS": root}}]})
            .to_string(),
            None,
        ),
        (
            "NotPrincipal instead of Principal",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Deny",
                                  "NotPrincipal": {"AWS": root},
                                  "Action": "kms:*",
                                  "Resource": "*"}]})
            .to_string(),
            None,
        ),
        (
            "pretty-printed, so the document carries newlines and spaces",
            serde_json::to_string_pretty(&json!({
                "Version": "2012-10-17",
                "Statement": [statement("Pretty", json!({"AWS": root}), json!("kms:*"), json!("*"))]
            }))
            .expect("the document serializes"),
            None,
        ),
        (
            "a Latin-1 value, the top of the modelled character range",
            policy(json!([{
                "Sid": "Latin1",
                "Effect": "Allow",
                "Principal": {"AWS": root},
                "Action": "kms:Encrypt",
                "Resource": "*",
                "Condition": {"StringEquals": {"aws:PrincipalTag/clé": "privée-ÿ"}}
            }])),
            None,
        ),
        (
            "a document at the quota, the largest one a key holds",
            policy_of_len(MAX_KEY_POLICY_BYTES),
            None,
        ),
    ];

    for (case, document, policy_name) in cases {
        put_policy(&router, case, &key_id, &document, policy_name).await;
        let served = get_policy(&router, case, &key_id).await;
        assert_eq!(
            served,
            reported_policy(&document),
            "case {case:?}: GetKeyPolicy must serve the attached document in KMS's own \
             rendering, got {served:?}"
        );
        // The key itself is untouched by a policy write: its metadata is what
        // `CreateKey` reported.
        let described = ok(&router, "DescribeKey", &json!({"KeyId": key_id}))
            .await
            .get("KeyMetadata")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            described, metadata,
            "case {case:?}: a policy write must not change the key's metadata, got {described}"
        );
    }
}

/// R1: a policy belongs to ONE key — attaching a document to a key leaves every other
/// key's policy as it was.
///
/// The rows differ in how many keys are in play and which of them is written to.
#[tokio::test]
async fn a_key_policy_is_the_property_of_one_key() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");

    // Three keys, each given a document naming itself.
    let mut keys: Vec<(String, String)> = Vec::new();
    for index in 0..3u8 {
        let metadata = create_key(&router, &json!({"Description": format!("key {index}")})).await;
        let key_id = text("setup", &metadata, "KeyId");
        let document = policy(json!([statement(
            &format!("Key{index}"),
            json!({"AWS": root}),
            json!("kms:*"),
            json!(text("setup", &metadata, "Arn"))
        )]));
        keys.push((key_id, document));
    }

    for (key_id, document) in &keys {
        put_policy(&router, "per-key policies", key_id, document, None).await;
    }
    for (key_id, document) in &keys {
        let served = get_policy(&router, "per-key policies", key_id).await;
        assert_eq!(
            served,
            reported_policy(document),
            "case {key_id:?}: each key must serve its OWN policy, got {served:?}"
        );
    }
}

/// R1: the document a key starts life with is the one `CreateKey` attached — an
/// explicit `Policy` when the create carried one, and otherwise the default key policy,
/// which "gives the Amazon Web Services account that owns the KMS key permission to use
/// it in all cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`).
///
/// The rows differ in what the create request said: nothing, an explicit document, and
/// a key whose spec and origin differ, since the policy is not a property of either.
#[tokio::test]
async fn a_fresh_key_serves_the_policy_create_key_attached() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let explicit = policy(json!([statement(
        "AllowRoot",
        json!({"AWS": root.clone()}),
        json!("kms:*"),
        json!("*")
    )]));

    // Rows: the case, the create request, and the document expected afterwards
    // (`None` means the default key policy, which the service builds).
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("a symmetric key with no Policy", json!({}), None),
        (
            "an HMAC key with no Policy",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            None,
        ),
        (
            "a key awaiting imported key material",
            json!({"Origin": "EXTERNAL"}),
            None,
        ),
        (
            "a key created with an explicit Policy",
            json!({"Policy": explicit}),
            Some(explicit.as_str()),
        ),
    ];

    for (case, request, expected) in cases {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let served = get_policy(&router, case, &key_id).await;
        match expected {
            Some(document) => assert_eq!(
                served,
                reported_policy(document),
                "case {case:?}: the explicit document must be served back, got {served:?}"
            ),
            None => {
                // The default policy is not a literal this test pins: what it must be
                // is a document granting the owning account every KMS action on the key.
                let document: Value = serde_json::from_str(&served)
                    .unwrap_or_else(|error| panic!("case {case:?}: {served:?} is not JSON: {error}"));
                let statements = document["Statement"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no Statement list in {served:?}"));
                let owner_allowed = statements.iter().any(|statement| {
                    statement["Effect"] == json!("Allow")
                        && statement["Principal"]["AWS"] == json!(root)
                        && statement["Action"] == json!("kms:*")
                });
                assert!(
                    owner_allowed,
                    "case {case:?}: the default key policy must allow the owning account \
                     every KMS action, got {served:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R2: which documents are accepted
// ---------------------------------------------------------------------------

/// R2: a document that is not an IAM JSON policy document whose every statement has an
/// `Effect` and names a principal is refused with `MalformedPolicyDocumentException`,
/// and the key keeps the policy it had.
///
/// The rows differ in HOW the document fails: it is not JSON at all, it is JSON of the
/// wrong kind, its `Statement` is missing, empty or of the wrong kind, a statement is
/// not an object, a statement's `Effect` is missing or not one of the two values, and a
/// statement names no principal in any of the forms a `Principal` element takes.
#[tokio::test]
async fn a_malformed_key_policy_is_refused_and_changes_nothing() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let metadata = create_key(&router, &json!({"Description": "malformed policies"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    let kept = policy(json!([statement(
        "Kept",
        json!({"AWS": root.clone()}),
        json!("kms:*"),
        json!("*")
    )]));
    put_policy(&router, "setup", &key_id, &kept, None).await;

    // Rows: the case, and the document the request carries.
    let cases: Vec<(&str, String)> = vec![
        ("not JSON at all", "not a policy".to_string()),
        ("a truncated document", "{\"Statement\": [".to_string()),
        ("a JSON list", "[]".to_string()),
        ("a JSON string", "\"default\"".to_string()),
        ("a JSON number", "7".to_string()),
        ("an object with no Statement", json!({"Version": "2012-10-17"}).to_string()),
        (
            "an empty Statement list",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        (
            "a Statement that is a string",
            json!({"Statement": "Allow everything"}).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({"Statement": ["Allow everything"]}).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({"Statement": [{"Principal": {"AWS": root}, "Action": "kms:*"}]}).to_string(),
        ),
        (
            "a statement whose Effect is lower case",
            json!({"Statement": [{"Effect": "allow",
                                  "Principal": {"AWS": root},
                                  "Action": "kms:*"}]})
            .to_string(),
        ),
        (
            "a statement whose Effect is not Allow or Deny",
            json!({"Statement": [{"Effect": "Permit",
                                  "Principal": {"AWS": root},
                                  "Action": "kms:*"}]})
            .to_string(),
        ),
        (
            "a statement with no principal at all",
            json!({"Statement": [{"Effect": "Allow", "Action": "kms:*", "Resource": "*"}]})
                .to_string(),
        ),
        (
            "a statement whose Principal map is empty",
            json!({"Statement": [{"Effect": "Allow", "Principal": {}, "Resource": "*"}]})
                .to_string(),
        ),
        (
            "a statement whose Principal list is empty",
            json!({"Statement": [{"Effect": "Allow", "Principal": {"AWS": []}}]}).to_string(),
        ),
        (
            "a statement whose Principal is the empty string",
            json!({"Statement": [{"Effect": "Allow", "Principal": ""}]}).to_string(),
        ),
        (
            "a statement whose Principal is null",
            json!({"Statement": [{"Effect": "Allow", "Principal": null}]}).to_string(),
        ),
        (
            "the second statement of two names no principal",
            json!({"Statement": [{"Effect": "Allow",
                                  "Principal": {"AWS": root},
                                  "Action": "kms:*"},
                                 {"Effect": "Deny", "Action": "kms:Decrypt"}]})
            .to_string(),
        ),
    ];

    for (case, document) in cases {
        let (status, code, message) = refused(
            &router,
            REGION,
            "PutKeyPolicy",
            &json!({"KeyId": key_id, "Policy": document}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a refused policy, got {status} ({code})"
        );
        assert_eq!(
            code, "MalformedPolicyDocumentException",
            "case {case:?}: expected MalformedPolicyDocumentException, got {code:?}"
        );
        // Which criterion the document failed is not disclosed: AWS words this refusal
        // with no message at all (recorded behavior), and a parser's account of the
        // request's bytes is exactly what it does not disclose.
        assert!(
            message.is_empty(),
            "case {case:?}: AWS words this refusal with no message, got {message:?}"
        );
        let served = get_policy(&router, case, &key_id).await;
        assert_eq!(
            served,
            reported_policy(&kept),
            "case {case:?}: a refused write must leave the stored policy alone, got {served:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// R3: the policy name
// ---------------------------------------------------------------------------

/// R3: `default` is the only name a key has a policy under, so every other name is a
/// `NotFoundException` — on the read and on the write alike, and a refused write leaves
/// the stored document alone.
///
/// The rows are names the modelled `PolicyNameType` (`^[\w]+$`, length 1..128) allows:
/// another spelling of the default, a one-character name, a name with digits and an
/// underscore, and a name at the modelled maximum length.
#[tokio::test]
async fn only_the_default_key_policy_name_exists() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let metadata = create_key(&router, &json!({"Description": "policy names"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let kept = policy(json!([statement(
        "Kept",
        json!({"AWS": root}),
        json!("kms:*"),
        json!("*")
    )]));
    put_policy(&router, "setup", &key_id, &kept, None).await;

    let longest = "n".repeat(128);
    let cases: Vec<&str> = vec![
        "Default",
        "DEFAULT",
        "notdefault",
        "default_2",
        "a",
        "key_policy_1",
        longest.as_str(),
    ];

    for name in cases {
        for (operation, request) in [
            (
                "GetKeyPolicy",
                json!({"KeyId": key_id, "PolicyName": name}),
            ),
            (
                "PutKeyPolicy",
                json!({"KeyId": key_id, "PolicyName": name, "Policy": kept}),
            ),
        ] {
            let case = format!("{operation} with PolicyName {name:?}");
            let (status, code) = failure(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a policy that does not exist, got \
                 {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?}"
            );
        }
        let served = get_policy(&router, name, &key_id).await;
        assert_eq!(
            served,
            reported_policy(&kept),
            "case {name:?}: a refused write must leave the stored policy alone, got {served:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// R4: the quota
// ---------------------------------------------------------------------------

/// R4: a key policy document is bounded by KMS's key policy quota: a document at the
/// bound is stored, and one past it is refused with `LimitExceededException` while the
/// stored document stays as it was.
///
/// The rows are the published bound, one byte under it, one byte past it, and a
/// document far past it.
#[tokio::test]
async fn a_key_policy_document_is_bounded_by_the_quota() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let metadata = create_key(&router, &json!({"Description": "policy quota"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let kept = policy(json!([statement(
        "Kept",
        json!({"AWS": root}),
        json!("kms:*"),
        json!("*")
    )]));
    put_policy(&router, "setup", &key_id, &kept, None).await;

    // Rows: the document's size in bytes, and whether a key accepts it.
    let cases: [(usize, bool); 4] = [
        (MAX_KEY_POLICY_BYTES - 1, true),
        (MAX_KEY_POLICY_BYTES, true),
        (MAX_KEY_POLICY_BYTES + 1, false),
        (MAX_KEY_POLICY_BYTES * 2, false),
    ];

    for (len, accepted) in cases {
        let case = format!("a {len}-byte key policy");
        let document = policy_of_len(len);
        if accepted {
            put_policy(&router, &case, &key_id, &document, None).await;
            let served = get_policy(&router, &case, &key_id).await;
            // The quota bounds the document the REQUEST carries; what the key then
            // serves is that whole document in KMS's rendering of it.
            assert_eq!(
                served,
                reported_policy(&document),
                "case {case:?}: a document at or under the quota must be stored whole, got \
                 {} bytes",
                served.len()
            );
            // Put the small document back, so the next refusal is compared against it.
            put_policy(&router, &case, &key_id, &kept, None).await;
        } else {
            let (status, code) = failure(
                &router,
                "PutKeyPolicy",
                &json!({"KeyId": key_id, "Policy": document}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 past the quota, got {status} ({code})"
            );
            assert_eq!(
                code, "LimitExceededException",
                "case {case:?}: expected LimitExceededException, got {code:?}"
            );
            let served = get_policy(&router, &case, &key_id).await;
            assert_eq!(
                served,
                reported_policy(&kept),
                "case {case:?}: a refused write must leave the stored policy alone, got \
                 {served:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R5: identifiers and key state
// ---------------------------------------------------------------------------

/// R5: both policy operations accept every `KeyId` form KMS does — a key id, a key ARN,
/// `alias/<name>` and an alias ARN — so a document attached through one form is served
/// through any other, while an identifier that names no key of this endpoint is refused
/// (`NotFoundException`) and one that is an ARN but not a KMS key or alias ARN is
/// refused as `InvalidArnException`.
#[tokio::test]
async fn the_policy_operations_resolve_every_key_identifier_form() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let metadata = create_key(&router, &json!({"Description": "identifier forms"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let alias = "alias/policy-target";
    super::harness::create_alias(&router, "setup", alias, &key_id).await;
    let alias_arn = super::harness::alias_arn(REGION, alias);

    // Rows: the identifier the write names, and the identifier the read names — every
    // pairing must reach the same key.
    let forms = [key_id.clone(), key_arn.clone(), alias.to_string(), alias_arn];
    for (index, written_through) in forms.iter().enumerate() {
        let document = policy(json!([statement(
            &format!("Form{index}"),
            json!({"AWS": root.clone()}),
            json!("kms:*"),
            json!(key_arn.clone())
        )]));
        put_policy(&router, written_through, written_through, &document, None).await;
        for read_through in &forms {
            let case = format!("written through {written_through}, read through {read_through}");
            let served = get_policy(&router, &case, read_through).await;
            assert_eq!(
                served,
                reported_policy(&document),
                "case {case:?}: every identifier form must reach the same key's policy, got \
                 {served:?}"
            );
        }
    }

    // Rows: an identifier no key of this endpoint answers to, and the error class it
    // is refused with. A key ARN pinned to another Region cannot be served here at
    // all, and an ARN that is not a KMS key or alias ARN is not an identifier at all.
    let other_region_arn = format!("arn:aws:kms:us-west-2:{account}:key/{key_id}");
    let identifier_cases: Vec<(&str, String, &str)> = vec![
        (
            "an unknown key id",
            "00000000-1111-2222-3333-444444444444".to_string(),
            "NotFoundException",
        ),
        (
            "an unknown alias",
            "alias/no-such-alias".to_string(),
            "NotFoundException",
        ),
        (
            "a key ARN of another Region",
            other_region_arn,
            "NotFoundException",
        ),
        (
            "an IAM ARN",
            format!("arn:aws:iam::{account}:user/someone"),
            "InvalidArnException",
        ),
        (
            "a KMS ARN naming neither a key nor an alias",
            format!("arn:aws:kms:{REGION}:{account}:grant/abcd"),
            "InvalidArnException",
        ),
    ];
    let document = policy(json!([statement(
        "Refused",
        json!({"AWS": root}),
        json!("kms:*"),
        json!("*")
    )]));
    for (case, identifier, expected_code) in identifier_cases {
        for (operation, request) in [
            ("GetKeyPolicy", json!({"KeyId": identifier})),
            (
                "PutKeyPolicy",
                json!({"KeyId": identifier, "Policy": document}),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let (status, code) = failure(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected {expected_code}, got {code:?}"
            );
        }
    }

    // A key is regional: the key id of a key created in one Region names no key at
    // another Region's endpoint, for the read and the write alike.
    for (operation, request) in [
        ("GetKeyPolicy", json!({"KeyId": key_id})),
        (
            "PutKeyPolicy",
            json!({"KeyId": key_id, "Policy": document}),
        ),
    ] {
        let case = format!("{operation} at another Region's endpoint");
        let (status, code) = failure_in(&router, "eu-west-1", operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?}"
        );
    }
}

/// R5: a policy write needs a key state that allows it. A key pending deletion refuses
/// the write with `KMSInvalidStateException` and keeps the document it had — while the
/// READ still serves it, as `DescribeKey` still reports the key's metadata — and
/// `CancelKeyDeletion` makes the key writable again.
///
/// The rows are the key states this slice can put a key into: `Enabled`, `Disabled`,
/// `PendingImport` (a key created with no key material) and `PendingDeletion`.
#[tokio::test]
async fn a_key_pending_deletion_refuses_a_policy_write_but_still_serves_the_read() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let document = policy(json!([statement(
        "Attached",
        json!({"AWS": root}),
        json!("kms:*"),
        json!("*")
    )]));

    // Rows: the case, the create request, what to do to the key before the write, and
    // whether a policy write is accepted in the state that leaves it in.
    let cases: Vec<(&str, Value, Option<&str>, bool)> = vec![
        ("Enabled", json!({}), None, true),
        ("Disabled", json!({}), Some("DisableKey"), true),
        (
            "PendingImport",
            json!({"Origin": "EXTERNAL"}),
            None,
            true,
        ),
        (
            "PendingDeletion",
            json!({}),
            Some("ScheduleKeyDeletion"),
            false,
        ),
    ];

    for (case, request, prepare, writable) in cases {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let before = get_policy(&router, case, &key_id).await;
        if let Some(operation) = prepare {
            ok(&router, operation, &json!({"KeyId": key_id})).await;
        }

        if writable {
            put_policy(&router, case, &key_id, &document, None).await;
            let served = get_policy(&router, case, &key_id).await;
            assert_eq!(
                served,
                reported_policy(&document),
                "case {case:?}: a key in this state must take a policy, got {served:?}"
            );
            continue;
        }

        let (status, payload) = call(
            &router,
            REGION,
            "PutKeyPolicy",
            &json!({"KeyId": key_id, "Policy": document}),
        )
        .await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a key in this state, got {status} with \
             {payload}"
        );
        assert_eq!(
            code, "KMSInvalidStateException",
            "case {case:?}: expected KMSInvalidStateException, got {code:?} in {payload}"
        );
        // The read is unaffected, and nothing was written.
        let served = get_policy(&router, case, &key_id).await;
        assert_eq!(
            served, before,
            "case {case:?}: a refused write must leave the stored policy alone, got {served:?}"
        );

        // Restoring the key makes it writable again, which is what makes the refusal
        // about the key STATE rather than about the key.
        ok(&router, "CancelKeyDeletion", &json!({"KeyId": key_id})).await;
        put_policy(&router, case, &key_id, &document, None).await;
        let served = get_policy_in(&router, REGION, case, &key_id).await;
        assert_eq!(
            served,
            reported_policy(&document),
            "case {case:?}: a restored key must take a policy, got {served:?}"
        );
    }
}
