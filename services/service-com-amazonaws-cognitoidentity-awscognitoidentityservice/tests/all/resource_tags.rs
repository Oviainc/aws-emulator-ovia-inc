// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListTagsForResource`: the tags a client reads back for an
//! identity pool.
//!
//! The RULE every case asserts, over a table of inputs rather than one recorded call:
//! `ListTagsForResource` reports exactly the tag set stored on the resource its
//! `ResourceArn` names — an EMPTY map when the resource carries none — leaves that set
//! unchanged, refuses an ARN that is not an identity-pool ARN of this service with the
//! modelled 400 `InvalidParameterException`, and answers the modelled 404
//! `ResourceNotFoundException` for one naming a pool this service does not hold.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_arn;

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The region the harness signs for, and so the region every pool it creates is minted
/// in and named by.
const REGION: &str = "us-east-1";

/// An identity pool's ARN, in the form AWS documents for a Cognito Identity pool:
/// `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`.
/// Built here from its parts rather than through the service's own helper, so a case can
/// vary each field independently of how the emulator renders an ARN.
fn pool_arn(partition: &str, region: &str, account: &str, identity_pool_id: &str) -> String {
    format!("arn:{partition}:cognito-identity:{region}:{account}:identitypool/{identity_pool_id}")
}

/// The ARN a client holds for a pool in this emulator: the canonical partition, the
/// pool's own region and the emulator's account id (`engine::emulator_account_id`).
fn canonical_arn(identity_pool_id: &str) -> String {
    pool_arn("aws", REGION, engine::emulator_account_id(), identity_pool_id)
}

/// A JSON tag map from `(key, value)` pairs, so a case can carry keys and values that are
/// computed rather than written out.
fn tag_map<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

/// `(key, value)` as owned strings, for [`tag_map`].
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Every shape of tag set a pool can carry, as `(case, IdentityPoolTags)` — `None` sends
/// the member not at all. The rows differ in the COUNT of tags, in key and value length
/// (`IdentityPoolTagsType` maps `TagKeysType`, len 1..128, to `TagValueType`, len 0..256,
/// from `smithy_query(query="shape",shape="IdentityPoolTagsType")`, so each bound is a
/// row) and in the characters a value carries. The 50-tag row is the ceiling the model's
/// own `TagResource` doc publishes: "An identity pool can have as many as 50 tags"
/// (`smithy_query(query="doc",shape="TagResource")`).
fn tag_sets() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no IdentityPoolTags member at all", None),
        ("an empty tag map", Some(tag_map([]))),
        ("one tag", Some(tag_map([tag("Environment", "Test")]))),
        (
            "several tags",
            Some(tag_map([
                tag("Environment", "Production"),
                tag("Owner", "platform"),
                tag("CostCenter", "4711"),
            ])),
        ),
        (
            "a key at the modelled maximum length of 128",
            Some(tag_map([tag(&"k".repeat(128), "at-the-key-bound")])),
        ),
        (
            "a key at the modelled minimum length of 1",
            Some(tag_map([tag("k", "at-the-key-bound")])),
        ),
        (
            "a value at the modelled maximum length of 256",
            Some(tag_map([tag("at-the-value-bound", &"v".repeat(256))])),
        ),
        (
            "an empty value, the modelled minimum length of 0",
            Some(tag_map([tag("Unset", "")])),
        ),
        (
            "a value carrying spaces and non-ASCII characters",
            Some(tag_map([tag("Description", "pool d'identité — étape 1")])),
        ),
        (
            "the 50 tags an identity pool can have",
            Some(tag_map(
                (0..50).map(|position| tag(&format!("key-{position}"), &format!("value-{position}"))),
            )),
        ),
    ]
}

/// Create a pool carrying `tags` (the member is sent only when the case has one) and
/// return its minted `IdentityPoolId`.
async fn tagged_pool(emulator: &Emulator, name: &str, tags: Option<&Value>) -> String {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": false,
    });
    if let Some(tags) = tags {
        request["IdentityPoolTags"] = tags.clone();
    }
    let created = emulator.ok("CreateIdentityPool", &request).await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// List the tags of the pool `identity_pool_id` through its canonical ARN.
async fn listed_tags(emulator: &Emulator, identity_pool_id: &str) -> Value {
    let listed = emulator
        .ok(
            "ListTagsForResource",
            &json!({"ResourceArn": canonical_arn(identity_pool_id)}),
        )
        .await;
    listed["Tags"].clone()
}

/// RULE: `ListTagsForResource` reports exactly the tag set stored on the pool its
/// `ResourceArn` names, as an empty map when the pool carries none, and reading it leaves
/// that set exactly as it was — as the pool's other reader, `DescribeIdentityPool`,
/// confirms.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_stored_on_the_pool() {
    let emulator = Emulator::new();
    for (case, tags) in tag_sets() {
        let pool_id = tagged_pool(&emulator, "tagged-pool", tags.as_ref()).await;
        // A pool created without tags carries NONE, which is an empty set.
        let expected = tags.unwrap_or_else(|| tag_map([]));

        let listed = listed_tags(&emulator, &pool_id).await;
        assert!(
            listed.is_object(),
            "case {case:?}: expected Tags to be a map even when empty, got {listed:?}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected Tags {expected}, got {listed:?}"
        );

        // A read changes nothing a later caller sees: the same call, and the pool's other
        // reader, report the same set.
        let again = listed_tags(&emulator, &pool_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second ListTagsForResource expected Tags {expected}, got {again:?}"
        );
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": pool_id.clone()}),
            )
            .await;
        let described_tags = described
            .get("IdentityPoolTags")
            .cloned()
            .unwrap_or_else(|| tag_map([]));
        assert_eq!(
            described_tags, expected,
            "case {case:?}: DescribeIdentityPool expected IdentityPoolTags {expected}, got {described_tags:?}"
        );
    }
}

/// RULE: the pool a `ResourceArn` names is the `IdentityPoolId` inside it, so every
/// well-formed identity-pool ARN for one pool reads that pool's tags — whatever partition,
/// region or account the ARN names them in.
#[tokio::test]
async fn every_well_formed_arn_for_a_pool_reads_that_pools_tags() {
    let emulator = Emulator::new();
    let tags = tag_map([tag("Environment", "Test"), tag("Owner", "platform")]);
    let pool_id = tagged_pool(&emulator, "arn-forms", Some(&tags)).await;

    let arns: Vec<(&str, String)> = vec![
        (
            "the ARN this service itself mints for the pool",
            identity_pool_arn(&pool_id),
        ),
        (
            "the canonical partition, region and emulator account",
            canonical_arn(&pool_id),
        ),
        (
            "the China partition",
            pool_arn(
                "aws-cn",
                "cn-north-1",
                engine::emulator_account_id(),
                &pool_id,
            ),
        ),
        (
            "the GovCloud partition",
            pool_arn(
                "aws-us-gov",
                "us-gov-west-1",
                engine::emulator_account_id(),
                &pool_id,
            ),
        ),
        (
            "another twelve-digit account id",
            pool_arn("aws", REGION, "123456789012", &pool_id),
        ),
    ];

    for (case, arn) in arns {
        let listed = emulator
            .ok("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
        assert_eq!(
            listed["Tags"], tags,
            "case {case:?} ({arn}): expected Tags {tags}, got {listed}"
        );
    }
}

/// RULE: a `ResourceArn` naming a pool this service does not hold — one never created, or
/// one already deleted — is the modelled 404 `ResourceNotFoundException`.
#[tokio::test]
async fn an_arn_naming_a_pool_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    // A stored pool, so the store is not empty and a miss is a miss on this pool alone.
    let kept = tagged_pool(
        &emulator,
        "kept-pool",
        Some(&tag_map([tag("Environment", "Test")])),
    )
    .await;
    let deleted = tagged_pool(
        &emulator,
        "deleted-pool",
        Some(&tag_map([tag("Environment", "Test")])),
    )
    .await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": deleted.clone()}),
        )
        .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "an IdentityPoolId no pool was ever created with",
            canonical_arn(ABSENT_POOL_ID),
        ),
        (
            "a pool named in another region, never created there",
            pool_arn(
                "aws",
                "eu-west-1",
                engine::emulator_account_id(),
                "eu-west-1:22222222-2222-2222-2222-222222222222",
            ),
        ),
        ("a pool that was deleted", canonical_arn(&deleted)),
    ];

    for (case, arn) in cases {
        let (status, body) = emulator
            .call("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?} ({arn}): expected HTTP 404, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?} ({arn}): expected ResourceNotFoundException, got {body}"
        );
    }

    // The pool that was kept is untouched by every miss above.
    let listed = listed_tags(&emulator, &kept).await;
    assert_eq!(
        listed,
        tag_map([tag("Environment", "Test")]),
        "the kept pool expected its own tags after the misses, got {listed:?}"
    );
}

/// RULE: a `ResourceArn` that is not an identity-pool ARN of this service names no
/// resource at all, so it is a bad input parameter (400 `InvalidParameterException`) and
/// not a missing resource.
#[tokio::test]
async fn an_arn_that_names_no_identity_pool_is_an_invalid_parameter() {
    let emulator = Emulator::new();
    let pool_id = tagged_pool(&emulator, "malformed-arns", None).await;

    let cases: Vec<(&str, String)> = vec![
        (
            "not an ARN at all",
            "this-is-not-an-arn-at-all".to_string(),
        ),
        (
            "another service's ARN namespace",
            format!("arn:aws:cognito-idp:{REGION}:000000000000:userpool/{REGION}_abcdefghi"),
        ),
        (
            "another resource type in this namespace",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identity/{pool_id}"),
        ),
        (
            "no pool id after the resource type",
            format!("arn:aws:cognito-identity:{REGION}:000000000000:identitypool/"),
        ),
        (
            "an account id that is not twelve digits",
            pool_arn("aws", REGION, "12345", &pool_id),
        ),
        (
            "an empty region",
            pool_arn("aws", "", engine::emulator_account_id(), &pool_id),
        ),
        (
            "an empty partition",
            pool_arn("", REGION, engine::emulator_account_id(), &pool_id),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:cognito-identity:identitypool/{pool_id}"),
        ),
        (
            "shorter than ARNString's modelled minimum of 20 characters",
            "arn:aws:cognito".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let (status, body) = emulator
            .call("ListTagsForResource", &json!({"ResourceArn": arn.clone()}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected HTTP 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterException"),
            "case {case:?} ({arn}): expected InvalidParameterException, got {body}"
        );
    }

    // A required member that is absent is the same class of refusal.
    let (status, body) = emulator.call("ListTagsForResource", &json!({})).await;
    assert_eq!(
        status, 400,
        "an absent ResourceArn expected HTTP 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterException"),
        "an absent ResourceArn expected InvalidParameterException, got {body}"
    );
}
