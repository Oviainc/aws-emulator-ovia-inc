// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT and the IDENTIFIERS two KMS read paths project onto the
//! wire, driven over the wire through the shared [`super::harness`].
//!
//! The rules under test, each asserted over a table of inputs:
//!
//! 1. A key policy document is not echoed back as the caller's bytes: KMS parses the
//!    document it was handed and re-serializes it, so every path that reports one —
//!    `GetKeyPolicy` and `ReplicateKey`'s `ReplicaPolicy` — answers the same canonical
//!    text whatever layout the document arrived in. The layout is written out by hand
//!    below, which is what makes this suite the place that pins it: each member of an
//!    object on its own line indented two spaces per enclosing OBJECT, `" : "` between a
//!    member name and its value, arrays inline (`[ `, `, `, ` ]`), and a value list
//!    holding one string written as that string.
//! 2. `ListKeyRotations` names the key of every entry it reports by the key's ARN — the
//!    fully qualified identifier, which the key record holds — rather than by the bare
//!    key id, whichever identifier form the request used and whichever key of a
//!    multi-Region set is listed.
//!
//! The third read projection the same issue covers — that `GetKeyLastUsage` reports the
//! usage record EMPTY until KMS's usage tracking has produced it, so a read taken
//! straight after a cryptographic operation carries none of its members — is
//! [`super::key_last_usage`]'s table.

use serde_json::{json, Value};

use super::harness::{
    alias_arn, create_alias, create_key, describe_in, ok, ok_in, reported_policy,
    rotate_on_demand, router, settle_key_state, text, REGION,
};

/// The Region a replica is created in, so the listing of a key whose id is shared by two
/// keys can be read at each key's own endpoint.
const REPLICA_REGION: &str = "eu-west-1";

/// The account every document below names as a principal: the one this emulator answers
/// for, so the text is built from the service's own account rather than from a literal.
fn account_root() -> String {
    format!("arn:aws:iam::{}:root", engine::emulator_account_id())
}

/// Attach `document` to `key_id`, bypassing the policy-lockout safety check: the rows
/// below are about the TEXT a document is reported as, not about which documents the
/// check accepts (`super::key_policy_lockout` covers that), and the model provides for
/// attaching one either way.
async fn put_policy(router: &smithy_server_runtime::MultiServiceRouter, key_id: &str, document: &str) {
    ok(
        router,
        "PutKeyPolicy",
        &json!({
            "KeyId": key_id,
            "Policy": document,
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    )
    .await;
}

/// The document `GetKeyPolicy` serves for `key_id`.
async fn served_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_id: &str,
) -> String {
    let answer = ok(router, "GetKeyPolicy", &json!({"KeyId": key_id})).await;
    text(case, &answer, "Policy")
}

/// Rule 1: the document a key serves is the canonical rendering of the document that was
/// attached — the text written out in each row — whatever layout the caller submitted.
///
/// The rows differ along what the rendering generalises over: how deeply the document
/// nests, how many statements the `Statement` list holds, how many values a value list
/// holds (one, which is written as the value alone, and several, which stay a list),
/// which element carries that list, how the submitted text spaces its tokens (none,
/// newlines and tabs, the AWS layout itself), and what sits inside a string literal (a
/// non-ASCII character, an escape, text that looks like JSON).
#[tokio::test]
async fn a_key_policy_is_served_in_the_canonical_layout_whatever_was_submitted() {
    let router = router();
    let root = account_root();
    let metadata = create_key(&router, &json!({"Description": "policy layout"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: the case, the document as submitted, and the text the key must serve.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "one statement, submitted compact",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Compact\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{root}\"}},\
                 \"Action\":\"kms:*\",\"Resource\":\"*\"}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"Compact\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
            ),
        ),
        (
            "the same document, submitted over several lines with tabs",
            format!(
                "{{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{{\n\t\t\t\
                 \"Sid\": \"Compact\",\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\
                 \"Principal\": {{\"AWS\": \"{root}\"}},\n\t\t\t\"Action\": \"kms:*\",\n\t\t\t\
                 \"Resource\": \"*\"\n\t\t}}\n\t]\n}}\n"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"Compact\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
            ),
        ),
        (
            "a one-value Action list, which is written as the value alone",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"OneValue\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":[\"{root}\"]}},\
                 \"Action\":[\"kms:Decrypt\"],\"Resource\":[\"*\"]}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"OneValue\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:Decrypt\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
            ),
        ),
        (
            "value lists of several values, which stay lists, laid out inline",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"ManyValues\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{root}\",\
                 \"Service\":[\"logs.amazonaws.com\",\"sqs.amazonaws.com\"]}},\
                 \"Action\":[\"kms:Encrypt\",\"kms:Decrypt\"],\"Resource\":\"*\"}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"ManyValues\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\",\n      \
                 \"Service\" : [ \"logs.amazonaws.com\", \"sqs.amazonaws.com\" ]\n    }},\n    \
                 \"Action\" : [ \"kms:Encrypt\", \"kms:Decrypt\" ],\n    \
                 \"Resource\" : \"*\"\n  }} ]\n}}"
            ),
        ),
        (
            "two statements, so the statement list holds two objects",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"First\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{root}\"}},\
                 \"Action\":\"kms:*\",\"Resource\":\"*\"}},{{\"Sid\":\"Second\",\
                 \"Effect\":\"Deny\",\"Principal\":{{\"AWS\":\"*\"}},\
                 \"Action\":\"kms:ScheduleKeyDeletion\",\"Resource\":\"*\"}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"First\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }}, {{\n    \
                 \"Sid\" : \"Second\",\n    \"Effect\" : \"Deny\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"*\"\n    }},\n    \
                 \"Action\" : \"kms:ScheduleKeyDeletion\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
            ),
        ),
        (
            "a condition, which nests two objects deeper, with a number and a boolean",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Conditioned\",\
                 \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{root}\"}},\
                 \"Action\":\"kms:Decrypt\",\"Resource\":\"*\",\"Condition\":{{\
                 \"StringEquals\":{{\"kms:ViaService\":\"s3.us-east-1.amazonaws.com\"}},\
                 \"NumericLessThan\":{{\"kms:GrantIsForAWSResource\":1.50}},\
                 \"Bool\":{{\"aws:SecureTransport\":true}}}}}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"Conditioned\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:Decrypt\",\n    \"Resource\" : \"*\",\n    \
                 \"Condition\" : {{\n      \"StringEquals\" : {{\n        \
                 \"kms:ViaService\" : \"s3.us-east-1.amazonaws.com\"\n      }},\n      \
                 \"NumericLessThan\" : {{\n        \
                 \"kms:GrantIsForAWSResource\" : 1.50\n      }},\n      \
                 \"Bool\" : {{\n        \"aws:SecureTransport\" : true\n      }}\n    }}\n  \
                 }} ]\n}}"
            ),
        ),
        (
            "string literals carrying non-ASCII text, an escape and JSON-looking text",
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\
                 \"Sid\":\"privée-ÿ\",\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"{root}\"}},\"Action\":\"kms:Encrypt\",\
                 \"Resource\":\"*\",\"Condition\":{{\"StringEquals\":{{\
                 \"aws:PrincipalTag/clé\":\"a \\\"quoted\\\" {{\\\"Action\\\":[\\\"x\\\"]}}\"\
                 }}}}}}]}}"
            ),
            format!(
                "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
                 \"Sid\" : \"privée-ÿ\",\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
                 \"Action\" : \"kms:Encrypt\",\n    \"Resource\" : \"*\",\n    \
                 \"Condition\" : {{\n      \"StringEquals\" : {{\n        \
                 \"aws:PrincipalTag/clé\" : \"a \\\"quoted\\\" {{\\\"Action\\\":[\\\"x\\\"]}}\"\
                 \n      }}\n    }}\n  }} ]\n}}"
            ),
        ),
    ];

    for (case, submitted, expected) in rows {
        put_policy(&router, &key_id, &submitted).await;
        let served = served_policy(&router, case, &key_id).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected the canonical rendering {expected:?}, got {served:?}"
        );

        // The rendering is a FIXED POINT: attaching the text the key served stores the
        // same text again, so a caller that reads, edits nothing and puts it back sees no
        // drift.
        put_policy(&router, &key_id, &served).await;
        let again = served_policy(&router, case, &key_id).await;
        assert_eq!(
            again, served,
            "case {case:?}: re-attaching the served document must serve the same text, got \
             {again:?}"
        );

        // The shared rendering every reader of a stored policy goes through agrees with
        // the text written out above.
        let shared = reported_policy(&submitted);
        assert_eq!(
            shared, expected,
            "case {case:?}: the shared rendering must be the canonical layout, got {shared:?}"
        );
    }
}

/// Rule 1, continued: a key created with no `Policy` of its own holds the DEFAULT key
/// policy, and that document is served in the same canonical layout — the one text AWS
/// answers for a freshly created key.
///
/// The rows differ in the kind of key, since the default policy is a property of none of
/// them: the symmetric encryption key `CreateKey` mints by default, an asymmetric signing
/// key, an HMAC key, and a multi-Region primary key.
#[tokio::test]
async fn the_default_key_policy_is_served_in_the_canonical_layout() {
    let router = router();
    // The default key policy "gives the Amazon Web Services account that owns the KMS key
    // permission to use it in all cryptographic operations"
    // (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`), as the one
    // statement of the document AWS attaches, under the policy id AWS gives it.
    let expected = format!(
        "{{\n  \"Version\" : \"2012-10-17\",\n  \"Id\" : \"key-default-1\",\n  \
         \"Statement\" : [ {{\n    \"Sid\" : \"Enable IAM User Permissions\",\n    \
         \"Effect\" : \"Allow\",\n    \"Principal\" : {{\n      \"AWS\" : \"{}\"\n    }},\n    \
         \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }} ]\n}}",
        account_root()
    );

    let kinds: [(&str, Value); 4] = [
        ("the default symmetric encryption key", json!({})),
        (
            "an RSA_2048 signing key",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "an HMAC_256 key",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
    ];

    for (case, request) in kinds {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let served = served_policy(&router, case, &key_id).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected the default key policy as {expected:?}, got {served:?}"
        );
    }
}

/// Rule 1, continued: `ReplicateKey` reports the replica's policy in the same layout, and
/// the replica's own endpoint serves exactly that text — so the two readers of one stored
/// document cannot disagree.
///
/// The rows are the two ways a replica gets a policy: a document the request carries, and
/// the default document KMS attaches when it carries none.
#[tokio::test]
async fn a_replica_reports_its_policy_in_the_canonical_layout() {
    let router = router();
    let root = account_root();
    let submitted = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Replica\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":[\"{root}\"]}},\
         \"Action\":[\"kms:*\"],\"Resource\":\"*\"}}]}}"
    );
    let submitted_expected = format!(
        "{{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {{\n    \
         \"Sid\" : \"Replica\",\n    \"Effect\" : \"Allow\",\n    \
         \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
         \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
    );
    let default_expected = format!(
        "{{\n  \"Version\" : \"2012-10-17\",\n  \"Id\" : \"key-default-1\",\n  \
         \"Statement\" : [ {{\n    \"Sid\" : \"Enable IAM User Permissions\",\n    \
         \"Effect\" : \"Allow\",\n    \"Principal\" : {{\n      \"AWS\" : \"{root}\"\n    }},\n    \
         \"Action\" : \"kms:*\",\n    \"Resource\" : \"*\"\n  }} ]\n}}"
    );

    // Rows: the case, the `Policy` the replicate request carries, and the text both
    // readers must then answer.
    let rows: Vec<(&str, Option<&str>, &String)> = vec![
        (
            "a policy the request carried",
            Some(submitted.as_str()),
            &submitted_expected,
        ),
        ("no policy, so the default one", None, &default_expected),
    ];

    for (case, policy, expected) in rows {
        let metadata = create_key(
            &router,
            &json!({"Description": case, "MultiRegion": true}),
        )
        .await;
        let key_id = text(case, &metadata, "KeyId");
        let mut request = json!({
            "KeyId": key_id,
            "ReplicaRegion": REPLICA_REGION,
            "BypassPolicyLockoutSafetyCheck": true,
        });
        if let Some(policy) = policy {
            request["Policy"] = json!(policy);
        }
        let answer = ok(&router, "ReplicateKey", &request).await;
        assert_eq!(
            &text(case, &answer, "ReplicaPolicy"),
            expected,
            "case {case:?}: expected the replica policy as {expected:?}, got {answer}"
        );

        settle_key_state(&router, REPLICA_REGION, case, &key_id, "Creating").await;
        let at_replica = ok_in(
            &router,
            REPLICA_REGION,
            "GetKeyPolicy",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            &text(case, &at_replica, "Policy"),
            expected,
            "case {case:?}: the replica's endpoint must serve the same text, got {at_replica}"
        );
    }
}

/// Rule 2: every entry `ListKeyRotations` reports names its key by the key ARN, not by
/// the bare key id — for every entry the listing can hold and through every identifier
/// form a request can name the key by.
///
/// The rows differ in how many rotations the key has had (so in how many entries the
/// listing holds), in whether the entry is a rotation or the material the key was created
/// with (`ALL_KEY_MATERIAL`), and in the identifier the request names the key by (a key
/// id, a key ARN, an alias name, an alias ARN).
#[tokio::test]
async fn every_rotation_entry_names_its_key_by_the_key_arn() {
    let router = router();

    for rotations in [0usize, 1, 2] {
        let case = format!("{rotations} rotation(s)");
        let metadata = create_key(&router, &json!({"Description": case.clone()})).await;
        let key_id = text(&case, &metadata, "KeyId");
        let key_arn = text(&case, &metadata, "Arn");
        let alias = format!("alias/rotation-arn-{rotations}");
        create_alias(&router, &case, &alias, &key_id).await;
        for round in 1..=rotations {
            rotate_on_demand(&router, &format!("{case} rotation {round}"), &key_id).await;
        }

        let forms = [
            ("key id", key_id.clone()),
            ("key ARN", key_arn.clone()),
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
        ];
        for (form, identifier) in &forms {
            // `ALL_KEY_MATERIAL` adds the material the key was created with, so the
            // listing holds one entry more than the key has rotations.
            for include in ["ROTATIONS_ONLY", "ALL_KEY_MATERIAL"] {
                let case = format!("{case}, read by {form}, {include}");
                let listed = ok(
                    &router,
                    "ListKeyRotations",
                    &json!({"KeyId": identifier, "IncludeKeyMaterial": include}),
                )
                .await;
                let entries = listed["Rotations"].as_array().cloned().unwrap_or_default();
                let expected_entries = if include == "ALL_KEY_MATERIAL" {
                    rotations + 1
                } else {
                    rotations
                };
                assert_eq!(
                    entries.len(),
                    expected_entries,
                    "case {case:?}: expected {expected_entries} entries, got {listed}"
                );
                for (position, entry) in entries.iter().enumerate() {
                    assert_eq!(
                        entry["KeyId"],
                        json!(key_arn),
                        "case {case:?} entry {position}: expected the key ARN {key_arn}, got \
                         {entry}"
                    );
                    assert_ne!(
                        entry["KeyId"],
                        json!(key_id),
                        "case {case:?} entry {position}: the bare key id is not the fully \
                         qualified identifier, got {entry}"
                    );
                }
            }
        }
    }
}

/// Rule 2, continued: the ARN an entry names is the one of the key BEING LISTED, so the
/// two keys of a multi-Region set — which share a key id — are named by their own ARNs,
/// each carrying the Region its key lives in.
#[tokio::test]
async fn a_replica_rotation_entry_names_the_replica_arn() {
    let router = router();
    let metadata = create_key(
        &router,
        &json!({"Description": "rotation entry ARNs", "MultiRegion": true}),
    )
    .await;
    let key_id = text("setup", &metadata, "KeyId");
    let primary_arn = text("setup", &metadata, "Arn");
    ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": key_id, "ReplicaRegion": REPLICA_REGION}),
    )
    .await;
    settle_key_state(&router, REPLICA_REGION, "replica", &key_id, "Creating").await;
    let replica_arn = text(
        "replica",
        &describe_in(&router, REPLICA_REGION, &key_id).await,
        "Arn",
    );
    assert_ne!(
        replica_arn, primary_arn,
        "case setup: the replica's ARN names its own Region, got {replica_arn}"
    );

    // Rotate the primary key: the rotation is shared by the set, so both endpoints list
    // an entry for it — each naming the key of ITS Region.
    rotate_on_demand(&router, "primary rotation", &key_id).await;

    for (case, region, expected_arn) in [
        ("primary", REGION, &primary_arn),
        ("replica", REPLICA_REGION, &replica_arn),
    ] {
        let listed = ok_in(
            &router,
            region,
            "ListKeyRotations",
            &json!({"KeyId": key_id, "IncludeKeyMaterial": "ALL_KEY_MATERIAL"}),
        )
        .await;
        let entries = listed["Rotations"].as_array().cloned().unwrap_or_default();
        assert!(
            !entries.is_empty(),
            "case {case}: expected the set's key materials to be listed, got {listed}"
        );
        for (position, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry["KeyId"],
                json!(expected_arn),
                "case {case} entry {position}: expected the ARN of the key being listed \
                 ({expected_arn}), got {entry}"
            );
        }
    }
}
