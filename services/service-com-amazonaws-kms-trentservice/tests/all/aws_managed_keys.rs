// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the predefined Amazon Web Services aliases and the Amazon
//! Web Services managed keys behind them, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: an alias name in the reserved `alias/aws/<service-name>`
//! namespace is a PREDEFINED alias that every account and Region holds with no target
//! key, and `DescribeKey` on one associates it with an Amazon Web Services managed
//! key — creating that key on the first such call — and answers its `KeyId` and
//! `Arn`, after which the alias resolves to that one key for every operation taking a
//! `KeyId` and `ListAliases` reports it. An identifier OUTSIDE that namespace is
//! still not found, and creates nothing.
//!
//! Every assertion is about that rule over a TABLE whose rows differ in the
//! service-name segment of the alias (its length and character classes), in the
//! identifier form the request uses (alias name, alias ARN, key id, key ARN), and in
//! the Region and account the identifier is scoped to. The segment forms follow the
//! documented shape of these names — "you can recognize Amazon Web Services aliases
//! because their names have the format `aws/<service-name>`, such as `aws/dynamodb`"
//! (`smithy_query(query="doc",shape="ListAliases")`) — and the lengths span
//! `AliasNameType`'s modelled `len 1..256`
//! (`smithy_query(query="operation",operation="CreateAlias")`).

use serde_json::{json, Value};

use super::harness::{
    alias_arn, call, create_key, describe, error_code, failure, is_key_id, ok, ok_in, router, text,
    REGION,
};

/// The reserved prefix a predefined Amazon Web Services alias lives under:
/// "the alias name cannot begin with alias/aws/. The alias/aws/ prefix is reserved
/// for Amazon Web Services managed keys"
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="AliasName")`), so no
/// caller can have created any name in this table.
const RESERVED_PREFIX: &str = "alias/aws/";

/// One row of the "a predefined alias resolves to an Amazon Web Services managed
/// key" table: the case name and the alias name.
///
/// The service-name segments span the documented `aws/<service-name>` form: the
/// model's own example, the shortest segments, digits, a hyphen, an underscore (both
/// of which `AliasNameType`'s `^[a-zA-Z0-9:/_-]+$` pattern permits) and the longest
/// name the modelled length allows.
fn predefined_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "the model's own example service name",
            format!("{RESERVED_PREFIX}dynamodb"),
        ),
        (
            "two-character service name",
            format!("{RESERVED_PREFIX}es"),
        ),
        (
            "a digit in the service name",
            format!("{RESERVED_PREFIX}s3"),
        ),
        (
            "one-character service name",
            format!("{RESERVED_PREFIX}x"),
        ),
        (
            "a hyphen in the service name",
            format!("{RESERVED_PREFIX}kinesis-video"),
        ),
        (
            "an underscore in the service name",
            format!("{RESERVED_PREFIX}service_one"),
        ),
        (
            "longest name the modelled length allows",
            format!("{RESERVED_PREFIX}{}", "x".repeat(246)),
        ),
    ]
}

/// The key ARN of `key_id` as the endpoint of `region` names it.
fn key_arn(region: &str, key_id: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{account}:key/{key_id}",
        account = engine::emulator_account_id()
    )
}

/// Assert `metadata` describes the Amazon Web Services managed key of a predefined
/// alias in `region`: a symmetric encryption key KMS generated the material of,
/// enabled and usable, whose manager is Amazon Web Services rather than the customer.
fn assert_aws_managed(case: &str, metadata: &Value, region: &str) -> String {
    let key_id = text(case, metadata, "KeyId");
    assert!(
        is_key_id(&key_id, false),
        "case {case:?}: expected a single-Region key id, got {key_id}"
    );
    for (member, expected) in [
        ("Arn", key_arn(region, &key_id)),
        ("KeyManager", "AWS".to_string()),
        ("KeyState", "Enabled".to_string()),
        ("KeyUsage", "ENCRYPT_DECRYPT".to_string()),
        ("KeySpec", "SYMMETRIC_DEFAULT".to_string()),
        ("Origin", "AWS_KMS".to_string()),
        ("AWSAccountId", engine::emulator_account_id().to_string()),
    ] {
        assert_eq!(
            metadata[member],
            json!(expected),
            "case {case:?}: expected {member} {expected}, got {metadata}"
        );
    }
    assert_eq!(
        metadata["Enabled"],
        json!(true),
        "case {case:?}: an Amazon Web Services managed key is enabled, got {metadata}"
    );
    assert_eq!(
        metadata["MultiRegion"],
        json!(false),
        "case {case:?}: the key of a predefined alias is single-Region, got {metadata}"
    );
    key_id
}

/// Every alias `ListAliases` reports in `region`, as (AliasName, AliasArn,
/// TargetKeyId) triples.
async fn listed_aliases(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
) -> Vec<(String, String, String)> {
    let answer = ok_in(router, region, "ListAliases", &json!({})).await;
    answer["Aliases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["AliasName"].as_str().unwrap_or_default().to_string(),
                entry["AliasArn"].as_str().unwrap_or_default().to_string(),
                entry["TargetKeyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// `DescribeKey` on a predefined Amazon Web Services alias answers the Amazon Web
/// Services managed key it names, creating it on the first call and reporting the
/// same key on every later one, through every identifier form — and `ListAliases`
/// then reports the alias pointing at that key.
#[tokio::test]
async fn describe_key_associates_a_predefined_alias_with_an_aws_managed_key() {
    let router = router();

    for (case, alias) in predefined_rows() {
        let metadata = describe(&router, &alias).await;
        let key_id = assert_aws_managed(case, &metadata, REGION);

        // Asking again — by any form KMS accepts — reports the SAME key, byte for
        // byte: the association happens once, and a later read neither mints a
        // second key nor moves the first.
        for (form, identifier) in [
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
            ("key id", key_id.clone()),
            ("key ARN", key_arn(REGION, &key_id)),
        ] {
            let again = describe(&router, &identifier).await;
            assert_eq!(
                again, metadata,
                "case {case:?} read by {form}: expected the same key metadata {metadata}, \
                 got {again}"
            );
        }

        let listed = listed_aliases(&router, REGION).await;
        let entry = (alias.clone(), alias_arn(REGION, &alias), key_id.clone());
        assert!(
            listed.contains(&entry),
            "case {case:?}: ListAliases must report {entry:?}, got {listed:?}"
        );
    }

    // One alias and one key per row, and nothing else: the reads above created
    // exactly what they resolved.
    let listed = listed_aliases(&router, REGION).await;
    assert_eq!(
        listed.len(),
        predefined_rows().len(),
        "case all rows: expected one alias per predefined name, got {listed:?}"
    );
}

/// The key a predefined alias resolves to is a real symmetric encryption key, not a
/// record: it seals a plaintext named by its alias and opens it again, reporting its
/// own ARN both times.
#[tokio::test]
async fn an_aws_managed_key_encrypts_and_decrypts_through_its_predefined_alias() {
    let router = router();

    for (case, alias) in predefined_rows() {
        let key_id = assert_aws_managed(case, &describe(&router, &alias).await, REGION);
        let arn = key_arn(REGION, &key_id);

        for (form, identifier) in [
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
        ] {
            let plaintext = aws_smithy_types::base64::encode(case.as_bytes());
            let sealed = ok(
                &router,
                "Encrypt",
                &json!({"KeyId": identifier, "Plaintext": plaintext}),
            )
            .await;
            assert_eq!(
                sealed["KeyId"],
                json!(arn),
                "case {case:?} sealed by {form}: expected the key ARN {arn}, got {sealed}"
            );

            let opened = ok(
                &router,
                "Decrypt",
                &json!({"CiphertextBlob": sealed["CiphertextBlob"].clone()}),
            )
            .await;
            assert_eq!(
                opened["Plaintext"],
                json!(plaintext),
                "case {case:?} sealed by {form}: Decrypt must return the plaintext that \
                 was sealed, got {opened}"
            );
            assert_eq!(
                opened["KeyId"],
                json!(arn),
                "case {case:?} sealed by {form}: expected the key ARN {arn}, got {opened}"
            );
        }
    }
}

/// Only the predefined namespace resolves: an identifier outside it is still not
/// found, and the refused read creates neither a key nor an alias.
#[tokio::test]
async fn an_identifier_outside_the_predefined_namespace_is_not_found() {
    let router = router();

    // Rows: an identifier that is NOT a predefined `aws/<service-name>` alias of this
    // endpoint — the reserved prefix with no segment after it, more than one segment
    // under it, a segment outside the character classes these names use, a name that
    // merely starts with the same letters, an alias a caller never created, and a key
    // id naming no key.
    let rows: Vec<(&str, String)> = vec![
        ("nothing after the reserved prefix", RESERVED_PREFIX.to_string()),
        (
            "two segments under the reserved prefix",
            format!("{RESERVED_PREFIX}team/project"),
        ),
        (
            "upper case in the service name",
            format!("{RESERVED_PREFIX}DynamoDB"),
        ),
        (
            "a colon in the service name",
            format!("{RESERVED_PREFIX}dynamo:db"),
        ),
        (
            "starts with the prefix's letters but is not under it",
            "alias/awsome-key".to_string(),
        ),
        ("an alias nobody created", "alias/absent".to_string()),
        (
            "a key id naming no key",
            "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
        ),
    ];

    for (case, identifier) in rows {
        let (status, code) = failure(&router, "DescribeKey", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code}"
        );
    }

    let listed = listed_aliases(&router, REGION).await;
    assert!(
        listed.is_empty(),
        "case refused reads: a refused DescribeKey must create nothing, got {listed:?}"
    );
    let keys = ok(&router, "ListKeys", &json!({})).await;
    assert_eq!(
        keys["Keys"],
        json!([]),
        "case refused reads: a refused DescribeKey must create no key, got {keys}"
    );
}

/// A predefined alias is REGIONAL and belongs to this account: each Region's endpoint
/// associates the same name with its own key, and an alias ARN scoped to another
/// Region or another account is not this endpoint's alias at all.
#[tokio::test]
async fn a_predefined_alias_is_regional_and_scoped_to_this_account() {
    let router = router();
    let alias = format!("{RESERVED_PREFIX}s3");

    // Rows: the Region whose endpoint is asked for the same predefined name.
    let regions = ["us-east-1", "eu-west-1", "ap-south-1"];
    let mut per_region: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let metadata = ok_in(&router, region, "DescribeKey", &json!({"KeyId": alias})).await
            ["KeyMetadata"]
            .clone();
        let key_id = assert_aws_managed(region, &metadata, region);
        per_region.push((region, key_id));
    }
    for (region, key_id) in &per_region {
        assert_eq!(
            per_region
                .iter()
                .filter(|(_, other)| other == key_id)
                .count(),
            1,
            "case {region}: each Region's endpoint holds its own key for the name, got \
             {per_region:?}"
        );
    }

    // Rows: an alias ARN that is not this endpoint's — another Region's, and another
    // account's. Neither resolves here, and neither creates anything.
    let other_account = format!("arn:aws:kms:{REGION}:999999999999:{alias}");
    for (case, identifier) in [
        ("another Region's alias ARN", alias_arn("eu-west-1", &alias)),
        ("another account's alias ARN", other_account),
    ] {
        let (status, payload) = call(&router, REGION, "DescribeKey", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {payload}"
        );
    }

    // us-east-1 holds exactly the one alias its own endpoint associated.
    let listed = listed_aliases(&router, REGION).await;
    assert_eq!(
        listed.len(),
        1,
        "case out-of-scope ARNs: us-east-1 holds one alias, got {listed:?}"
    );
}

/// An Amazon Web Services managed key is managed by Amazon Web Services: KMS rotates
/// its key material every year from the moment it exists, a caller cannot enable or
/// disable that rotation, and a key the caller creates is untouched by all of it.
#[tokio::test]
async fn an_aws_managed_key_rotates_yearly_and_its_rotation_is_not_configurable() {
    let router = router();
    let alias = format!("{RESERVED_PREFIX}secretsmanager");
    let key_id = assert_aws_managed("managed", &describe(&router, &alias).await, REGION);
    let arn = key_arn(REGION, &key_id);

    let report = ok(
        &router,
        "GetKeyRotationStatus",
        &json!({"KeyId": alias.clone()}),
    )
    .await;
    assert_eq!(
        report["KeyRotationEnabled"],
        json!(true),
        "case managed: an Amazon Web Services managed key rotates, got {report}"
    );
    assert_eq!(
        report["RotationPeriodInDays"],
        json!(365),
        "case managed: expected the yearly rotation period, got {report}"
    );
    assert_eq!(
        report["KeyId"],
        json!(arn),
        "case managed: expected the key ARN {arn}, got {report}"
    );

    // Rows: the two operations that configure automatic rotation, neither of which a
    // caller may use on a key Amazon Web Services manages.
    for operation in ["EnableKeyRotation", "DisableKeyRotation"] {
        let (status, code) = failure(&router, operation, &json!({"KeyId": alias.clone()})).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {operation}: expected UnsupportedOperationException, got {code}"
        );
    }

    // The refusals left the managed key exactly as it was, and a key the caller
    // creates is still customer managed and still not rotating.
    let after = ok(
        &router,
        "GetKeyRotationStatus",
        &json!({"KeyId": alias.clone()}),
    )
    .await;
    assert_eq!(
        after, report,
        "case managed: a refused rotation call changes nothing, got {after}"
    );
    let customer = create_key(&router, &json!({"Description": "customer managed"})).await;
    assert_eq!(
        customer["KeyManager"],
        json!("CUSTOMER"),
        "case customer: a key the caller creates is customer managed, got {customer}"
    );
    let customer_status = ok(
        &router,
        "GetKeyRotationStatus",
        &json!({"KeyId": text("customer", &customer, "KeyId")}),
    )
    .await;
    assert_eq!(
        customer_status["KeyRotationEnabled"],
        json!(false),
        "case customer: a new customer managed key is not rotating, got {customer_status}"
    );
}
