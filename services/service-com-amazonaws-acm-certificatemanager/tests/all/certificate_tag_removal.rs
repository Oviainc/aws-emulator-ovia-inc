// SPDX-License-Identifier: CC0-1.0

//! `RemoveTagsFromCertificate` over the wire, read back through
//! `ListTagsForCertificate`.
//!
//! RULE: an untagging request names pairs, and each named pair drops its key from the
//! certificate's stored tag map when the pair carries no value, or drops it only when
//! the stored value equals the value the pair carries; every other stored pair, and
//! every other certificate, is left exactly as it was
//! (`smithy_query(query="doc", shape="com.amazonaws.acm#RemoveTagsFromCertificate")`:
//! "If you do not specify the value portion of the tag when calling this function, the
//! tag will be removed regardless of value. If you specify a value, the tag is removed
//! only if it is associated with the specified value").
//!
//! The table below spans the classes the rule generalises over: a pair with and
//! without a value, a value that matches and one that does not, keys and values that
//! differ only in case, a key the certificate never carried, a stored pair with an
//! empty value, repeated keys in one request, one pair and the full modelled maximum
//! of them, and a request that empties the map. Every call goes through
//! `MultiServiceRouter::dispatch` with the harness shared with
//! [`crate::certificate_lifecycle`], so a response that is not computed from the
//! persisted store cannot pass.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};

/// The most tags one request may name, and the most a certificate may carry: the
/// modelled `TagList` length bound
/// (`smithy_query(query="operation", operation="RemoveTagsFromCertificate")` reports
/// `Tags` as `len 1..50`).
const TAG_LIST_MAX: usize = 50;

/// One untagging case: what it exercises, the tags the certificate is seeded with
/// (a `None` value seeds a pair whose value member is absent), the `Tags` the
/// `RemoveTagsFromCertificate` request names, and the pairs the certificate must
/// carry afterwards.
type RemovalCase = (
    &'static str,
    Vec<(String, Option<String>)>,
    Value,
    Vec<(String, String)>,
);

/// One step of a tag/untag sequence: what it exercises, the operation to call, the
/// `Tags` it names, and the pairs the certificate must carry afterwards.
type TagStep = (&'static str, &'static str, Value, Vec<(String, String)>);

/// One refusal case: what it exercises, the `CertificateArn` and `Tags` the request
/// names, and the wire error code and message the rule requires.
type RefusalCase = (&'static str, String, Value, &'static str, String);

/// `AddTagsToCertificate`'s `Tags` for a seed list: a `None` value is sent as a pair
/// with no `Value` member at all.
fn seed_body(tags: &[(String, Option<String>)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({ "Key": key, "Value": value }),
                None => json!({ "Key": key }),
            })
            .collect(),
    )
}

/// A certificate carrying exactly `tags`, created and tagged through the wire.
async fn tagged_certificate(
    router: &smithy_server_runtime::MultiServiceRouter,
    domain: &str,
    tags: &[(String, Option<String>)],
) -> String {
    let arn = request_certificate(
        router,
        &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
    )
    .await;
    if !tags.is_empty() {
        let (status, response) = call(
            router,
            "AddTagsToCertificate",
            &json!({ "CertificateArn": arn, "Tags": seed_body(tags) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {domain}: seeding tags must succeed, got {status} {response}"
        );
    }
    arn
}

/// The certificate's stored tags as sorted `(key, value)` pairs, read back through
/// `ListTagsForCertificate`. An untagged certificate reports no `Tags` member, which
/// is the empty vector here.
async fn stored_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> Vec<(String, String)> {
    let (status, response) = call(
        router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {arn}: ListTagsForCertificate must succeed, got {status} {response}"
    );
    let mut pairs: Vec<(String, String)> = response["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// `(key, value)` pairs from string literals, for the expectations below.
fn expect(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    expected.sort();
    expected
}

/// A seed list from `(key, value)` literals.
fn seed(pairs: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), Some(value.to_string())))
        .collect()
}

/// RULE: each pair a `RemoveTagsFromCertificate` request names drops its key when the
/// pair carries no value, and drops it only on an equal stored value when it carries
/// one; every other stored pair survives, and repeating the request changes nothing.
#[tokio::test]
async fn removing_tags_drops_exactly_the_named_pairs() {
    let router = router();

    // The widest row: the full modelled maximum of tags, every one of them named for
    // removal in a single request.
    let full_seed: Vec<(String, Option<String>)> = (0..TAG_LIST_MAX)
        .map(|index| (format!("key{index:02}"), Some(format!("value{index}"))))
        .collect();
    let full_request = seed_body(&full_seed);

    // A row that names half of that maximum, so the surviving half proves the removal
    // is per-pair rather than per-request.
    let half_request = Value::Array(
        full_seed
            .iter()
            .take(TAG_LIST_MAX / 2)
            .map(|(key, _)| json!({ "Key": key }))
            .collect(),
    );
    let half_expected: Vec<(String, String)> = full_seed
        .iter()
        .skip(TAG_LIST_MAX / 2)
        .map(|(key, value)| (key.clone(), value.clone().unwrap_or_default()))
        .collect();

    let cases: Vec<RemovalCase> = vec![
        (
            "one key named with no value, dropped regardless of value",
            seed(&[("env", "dev"), ("team", "platform")]),
            json!([{ "Key": "env" }]),
            expect(&[("team", "platform")]),
        ),
        (
            "one key named with the stored value, dropped",
            seed(&[("env", "dev"), ("team", "platform")]),
            json!([{ "Key": "env", "Value": "dev" }]),
            expect(&[("team", "platform")]),
        ),
        (
            "one key named with a different value, kept",
            seed(&[("env", "dev")]),
            json!([{ "Key": "env", "Value": "prod" }]),
            expect(&[("env", "dev")]),
        ),
        (
            "a value differing only in case, kept",
            seed(&[("env", "Prod")]),
            json!([{ "Key": "env", "Value": "prod" }]),
            expect(&[("env", "Prod")]),
        ),
        (
            "a key differing only in case, kept",
            seed(&[("Env", "dev")]),
            json!([{ "Key": "env" }]),
            expect(&[("Env", "dev")]),
        ),
        (
            "a key the certificate never carried, a no-op",
            seed(&[("env", "dev")]),
            json!([{ "Key": "absent" }]),
            expect(&[("env", "dev")]),
        ),
        (
            "several keys at once, valued and valueless mixed",
            seed(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            json!([
                { "Key": "a" },
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "mismatch" }
            ]),
            expect(&[("c", "3"), ("d", "4")]),
        ),
        (
            "a pair stored with no value, named by its key alone",
            vec![("solo".to_string(), None), ("env".to_string(), Some("dev".to_string()))],
            json!([{ "Key": "solo" }]),
            expect(&[("env", "dev")]),
        ),
        (
            "a pair stored with no value, named with the empty value",
            vec![("solo".to_string(), None), ("env".to_string(), Some("dev".to_string()))],
            json!([{ "Key": "solo", "Value": "" }]),
            expect(&[("env", "dev")]),
        ),
        (
            "a pair stored with no value, named with a non-empty value, kept",
            vec![("solo".to_string(), None)],
            json!([{ "Key": "solo", "Value": "something" }]),
            expect(&[("solo", "")]),
        ),
        (
            "one key named twice, the mismatching entry then the valueless one",
            seed(&[("env", "dev")]),
            json!([{ "Key": "env", "Value": "mismatch" }, { "Key": "env" }]),
            expect(&[]),
        ),
        (
            "every stored key named, emptying the map",
            seed(&[("env", "dev"), ("team", "platform")]),
            json!([{ "Key": "env" }, { "Key": "team", "Value": "platform" }]),
            expect(&[]),
        ),
        (
            "half of the modelled maximum named",
            full_seed.clone(),
            half_request,
            half_expected,
        ),
        (
            "the modelled maximum of pairs named at once",
            full_seed,
            full_request,
            expect(&[]),
        ),
    ];

    for (index, (case, initial, request_tags, expected)) in cases.into_iter().enumerate() {
        let arn = tagged_certificate(&router, &format!("row{index}.example.com"), &initial).await;

        // A second certificate with the same tags, never named by the request: the
        // removal must not reach it.
        let bystander = tagged_certificate(
            &router,
            &format!("bystander{index}.example.com"),
            &initial,
        )
        .await;
        let bystander_before = stored_tags(&router, &bystander).await;

        let (status, response) = call(
            &router,
            "RemoveTagsFromCertificate",
            &json!({ "CertificateArn": arn, "Tags": request_tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: RemoveTagsFromCertificate must succeed, got {status} {response}"
        );

        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the stored tags {expected:?}, got {actual:?}"
        );

        // Idempotence: the same request again leaves the same map.
        let (status, response) = call(
            &router,
            "RemoveTagsFromCertificate",
            &json!({ "CertificateArn": arn, "Tags": request_tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a repeated RemoveTagsFromCertificate must succeed, got {status} {response}"
        );
        let repeated = stored_tags(&router, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {case}: expected a repeated removal to leave {expected:?}, got {repeated:?}"
        );

        let bystander_after = stored_tags(&router, &bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {case}: expected the untouched certificate to keep {bystander_before:?}, \
             got {bystander_after:?}"
        );
    }
}

/// RULE: a removed tag is gone from the record, so adding it again stores it afresh —
/// the tag map is state, not a replay of the request that last touched it.
#[tokio::test]
async fn a_removed_tag_can_be_added_again() {
    let router = router();
    let arn = tagged_certificate(
        &router,
        "readd.example.com",
        &seed(&[("env", "dev"), ("team", "platform")]),
    )
    .await;

    // Each step: the operation, its `Tags`, and the map expected afterwards.
    let steps: Vec<TagStep> = vec![
        (
            "remove env",
            "RemoveTagsFromCertificate",
            json!([{ "Key": "env" }]),
            expect(&[("team", "platform")]),
        ),
        (
            "add env back with another value",
            "AddTagsToCertificate",
            json!([{ "Key": "env", "Value": "prod" }]),
            expect(&[("env", "prod"), ("team", "platform")]),
        ),
        (
            "remove env naming the stale value, which no longer matches",
            "RemoveTagsFromCertificate",
            json!([{ "Key": "env", "Value": "dev" }]),
            expect(&[("env", "prod"), ("team", "platform")]),
        ),
        (
            "remove env naming the current value",
            "RemoveTagsFromCertificate",
            json!([{ "Key": "env", "Value": "prod" }]),
            expect(&[("team", "platform")]),
        ),
        (
            "remove the last tag",
            "RemoveTagsFromCertificate",
            json!([{ "Key": "team" }]),
            expect(&[]),
        ),
    ];

    for (case, operation, tags, expected) in steps {
        let (status, response) = call(
            &router,
            operation,
            &json!({ "CertificateArn": arn, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} must succeed, got {status} {response}"
        );
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case}: expected the stored tags {expected:?}, got {actual:?}"
        );
    }

    // The map is empty now, so `ListTagsForCertificate` reports no `Tags` member at
    // all: `TagList` has a minimum length of one, so an empty list is not a legal
    // value.
    let (status, response) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        (status, response["Tags"].is_null()),
        (200, true),
        "expected no Tags member once every tag was removed, got {status} {response}"
    );
}

/// RULE: a request `RemoveTagsFromCertificate` refuses is refused before the stored
/// map is touched — an ARN that is not an ACM certificate ARN draws
/// `InvalidArnException`, a well-formed ARN with no record draws
/// `ResourceNotFoundException`, and a reserved `aws:` pair draws
/// `InvalidTagException`, each HTTP 400.
#[tokio::test]
async fn a_refused_removal_leaves_the_stored_tags_alone() {
    let router = router();
    let initial = seed(&[("env", "dev"), ("team", "platform")]);
    let arn = tagged_certificate(&router, "refused.example.com", &initial).await;
    let before = stored_tags(&router, &arn).await;

    let deleted = tagged_certificate(&router, "deleted.example.com", &initial).await;
    let (status, response) = call(
        &router,
        "DeleteCertificate",
        &json!({ "CertificateArn": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteCertificate must succeed, got {status} {response}"
    );

    // Each row: the ARN, the `Tags` and the modelled failure the rule requires, with
    // the message template it is built from.
    let cases: Vec<RefusalCase> = vec![
        (
            "an ARN naming a resource that is not a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij"),
            json!([{ "Key": "env" }]),
            "InvalidArnException",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij is not a valid ARN"),
        ),
        (
            "a certificate ARN with no record behind it",
            format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"
            ),
            json!([{ "Key": "env" }]),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate arn:aws:acm:{REGION}:{ACCOUNT}:certificate/\
                 11111111-2222-3333-4444-555555555555."
            ),
        ),
        (
            "a deleted certificate's ARN",
            deleted.clone(),
            json!([{ "Key": "env" }]),
            "ResourceNotFoundException",
            format!("Could not find certificate {deleted}."),
        ),
        (
            "a reserved tag key",
            arn.clone(),
            json!([{ "Key": "aws:reserved" }]),
            "InvalidTagException",
            "tag aws:reserved is reserved: neither a tag key nor its value may begin with aws:"
                .to_string(),
        ),
        (
            "a reserved tag value beside a removable key",
            arn.clone(),
            json!([{ "Key": "env" }, { "Key": "team", "Value": "aws:platform" }]),
            "InvalidTagException",
            "tag team is reserved: neither a tag key nor its value may begin with aws:".to_string(),
        ),
    ];

    for (case, certificate_arn, tags, expected_code, expected_message) in cases {
        let (status, response) = call(
            &router,
            "RemoveTagsFromCertificate",
            &json!({ "CertificateArn": certificate_arn, "Tags": tags }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, expected_code),
            "case {case}: expected 400 {expected_code}, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, expected_message,
            "case {case}: expected message {expected_message:?}, got {message:?}"
        );

        let after = stored_tags(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused removal must leave the stored tags {before:?}, got {after:?}"
        );
    }
}
