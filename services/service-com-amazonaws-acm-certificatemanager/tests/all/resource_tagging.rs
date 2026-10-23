// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACM's GENERIC resource tagging over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: `TagResource`, `UntagResource` and `ListTagsForResource` address every ACM
//! resource type EXCEPT the certificate resource type
//! (`smithy_query(query="doc", shape="com.amazonaws.acm#TagResource")`: "Use this
//! action for all ACM resource types except the certificate resource type. For
//! certificate resources, use AddTagsToCertificate instead"). On the resource its ARN
//! names, `TagResource` merges the request's pairs into that resource's own stored tag
//! map by key, `UntagResource` drops the keys it names from that map, and
//! `ListTagsForResource` reports the map as it stands — so a tag written by one call is
//! read back by another, and only the named resource changes. A certificate ARN is
//! refused as input the operation does not admit (`ValidationException`), an ARN with no
//! record behind it is `ResourceNotFoundException`, a map that would grow past the
//! modelled `TagList` bound of 50 is `ServiceQuotaExceededException`, and a reserved
//! `aws:` pair is refused — each HTTP 400, and each before the stored map is touched.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted
//! store cannot pass. The wire harness is shared with [`crate::certificate_lifecycle`].
//!
//! The table below spans the classes the rule generalises over — pair counts from one
//! to the modelled maximum, values present, empty and at the modelled maximum length, a
//! key at its own maximum length, every character class `TagKey`'s `@pattern` admits
//! (Latin, Greek, Cyrillic, CJK, digits, separators and the punctuation set), and a key
//! repeated inside one request — rather than the single pair the stub issue quoted.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};

/// The most tags one request may name, and the most one resource may carry: the
/// modelled `TagList` length bound (`smithy_query(query="operation",
/// operation="TagResource")` reports `Tags` as `len 1..50`).
const TAG_LIST_MAX: usize = 50;

/// `TagKey`'s modelled maximum length
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKey")`: `len 1..128`).
const TAG_KEY_MAX: usize = 128;

/// `TagValue`'s modelled maximum length
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagValue")`: `len ..256`).
const TAG_VALUE_MAX: usize = 256;

/// One tagging case: what it exercises, the `Tags` the request names, and the pairs the
/// resource must carry afterwards.
pub(crate) type TagCase = (&'static str, Value, Vec<(String, String)>);

/// One refusal case: what it exercises and the ARN the request names.
type ArnCase = (&'static str, String);

/// One step of a tag/untag sequence: what it exercises, the operation to call, the
/// request members beside `ResourceArn`, and the pairs the resource must carry
/// afterwards.
type TagStep = (&'static str, &'static str, Value, Vec<(String, String)>);

/// A fresh, untagged ACME endpoint, created through the wire; its ARN.
///
/// The ACME endpoint is the non-certificate resource this service persists, so it is
/// the resource the generic tagging operations address here.
pub(crate) async fn create_endpoint(router: &MultiServiceRouter, case: &str) -> String {
    let (status, response) = call(
        router,
        "CreateAcmeEndpoint",
        &json!({
            "AuthorizationBehavior": "PRE_APPROVED",
            "Contact": "REQUIRED",
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAcmeEndpoint returned no AcmeEndpointArn"))
        .to_string()
}

/// A `TagList` on the wire as `(key, value)` pairs, in the order the response listed
/// them.
pub(crate) fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    tags.as_array()
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
        .unwrap_or_default()
}

/// The resource's stored tags, read back through `ListTagsForResource`, in the order it
/// reported them. An untagged resource reports no `Tags` member, which is the empty
/// vector here.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let (status, response) = call(router, "ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {response}"
    );
    tag_pairs(&response["Tags"])
}

/// `TagResource`, asserted to succeed.
pub(crate) async fn tag_resource(router: &MultiServiceRouter, case: &str, arn: &str, tags: &Value) {
    let (status, response) = call(
        router,
        "TagResource",
        &json!({ "ResourceArn": arn, "Tags": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must succeed, got {status} {response}"
    );
}

/// `(key, value)` pairs from literals, sorted by key — the order the stored map is
/// reported in.
pub(crate) fn expect(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    expected.sort();
    expected
}

/// The table every row of [`tag_resource_writes_the_tags_list_tags_for_resource_reports`]
/// walks.
fn tag_cases() -> Vec<TagCase> {
    // The widest row: the full modelled maximum of pairs in one request.
    let max_pairs: Vec<(String, String)> = (0..TAG_LIST_MAX)
        .map(|index| (format!("key{index:02}"), format!("value{index}")))
        .collect();
    let max_request = Value::Array(
        max_pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    );

    // The modelled length bounds, exercised at the bound itself.
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let longest_value = "v".repeat(TAG_VALUE_MAX);

    vec![
        (
            "one pair",
            json!([{ "Key": "env", "Value": "dev" }]),
            expect(&[("env", "dev")]),
        ),
        (
            "several pairs, reported in key order whatever order the request listed",
            json!([
                { "Key": "team", "Value": "platform" },
                { "Key": "env", "Value": "prod" },
                { "Key": "owner", "Value": "sre" },
            ]),
            expect(&[("env", "prod"), ("owner", "sre"), ("team", "platform")]),
        ),
        (
            "a pair with an empty value",
            json!([{ "Key": "marker", "Value": "" }]),
            expect(&[("marker", "")]),
        ),
        (
            "a pair with no Value member at all, stored as the empty value",
            json!([{ "Key": "solo" }]),
            expect(&[("solo", "")]),
        ),
        (
            "non-ASCII keys and values: Greek, Cyrillic, CJK and accented Latin",
            json!([
                { "Key": "περιβάλλον", "Value": "παραγωγή" },
                { "Key": "окружение", "Value": "продакшн" },
                { "Key": "環境", "Value": "本番" },
                { "Key": "propriétaire", "Value": "équipe-sécurité" },
            ]),
            // Sorted by key the way the stored map orders them, which for these keys is
            // their UTF-8 byte order: ASCII-leading first, then Greek, Cyrillic, CJK.
            expect(&[
                ("propriétaire", "équipe-sécurité"),
                ("περιβάλλον", "παραγωγή"),
                ("окружение", "продакшн"),
                ("環境", "本番"),
            ]),
        ),
        (
            "every other character class TagKey's pattern admits: digits, separators \
             and the punctuation set",
            json!([
                { "Key": "cost center 42", "Value": "9 lives" },
                { "Key": "a_b.c:d/e=f+g-h@i", "Value": "j_k.l:m/n=o+p-q@r" },
            ]),
            expect(&[
                ("a_b.c:d/e=f+g-h@i", "j_k.l:m/n=o+p-q@r"),
                ("cost center 42", "9 lives"),
            ]),
        ),
        (
            "a key and a value at their modelled maximum lengths",
            json!([{ "Key": longest_key, "Value": longest_value }]),
            vec![(longest_key.clone(), longest_value.clone())],
        ),
        (
            "one key named twice in a request, the later value winning",
            json!([
                { "Key": "env", "Value": "first" },
                { "Key": "env", "Value": "second" },
            ]),
            expect(&[("env", "second")]),
        ),
        (
            "keys differing only in case, which are different keys",
            json!([{ "Key": "Env", "Value": "upper" }, { "Key": "env", "Value": "lower" }]),
            expect(&[("Env", "upper"), ("env", "lower")]),
        ),
        (
            "the modelled maximum of pairs in one request",
            max_request,
            max_pairs,
        ),
    ]
}

/// RULE: `TagResource` writes the pairs it names onto the resource its ARN names, and
/// `ListTagsForResource` reports that stored map — in key order, for every row of the
/// table above — while another resource of the same type is left exactly as it was.
#[tokio::test]
async fn tag_resource_writes_the_tags_list_tags_for_resource_reports() {
    let router = router();

    let cases = tag_cases();

    for (case, request_tags, expected) in cases {
        let arn = create_endpoint(&router, case).await;

        // A freshly created resource carries no tags, so `Tags` is absent rather than
        // an empty list (`TagList`'s minimum length is 1).
        let before = stored_tags(&router, case, &arn).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a fresh resource must carry no tags, got {before:?}"
        );

        // A second resource, never named by the request: the tagging must not reach it.
        let bystander = create_endpoint(&router, case).await;

        tag_resource(&router, case, &arn, &request_tags).await;

        let actual = stored_tags(&router, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tags {expected:?}, got {actual:?}"
        );

        // Reading does not change what a later caller sees.
        let again = stored_tags(&router, case, &arn).await;
        assert_eq!(
            again, actual,
            "case {case:?}: a second read must report the same tags {actual:?}, got {again:?}"
        );

        // Idempotence: the same request again leaves the same map.
        tag_resource(&router, case, &arn, &request_tags).await;
        let repeated = stored_tags(&router, case, &arn).await;
        assert_eq!(
            repeated, expected,
            "case {case:?}: a repeated TagResource must leave {expected:?}, got {repeated:?}"
        );

        let untouched = stored_tags(&router, case, &bystander).await;
        assert!(
            untouched.is_empty(),
            "case {case:?}: the resource the request never named must stay untagged, \
             got {untouched:?}"
        );
    }
}

/// RULE: tagging MERGES by key and untagging drops the keys it names, so the stored map
/// is state rather than a replay of whichever request last touched it.
#[tokio::test]
async fn tagging_merges_by_key_and_untagging_drops_named_keys() {
    let router = router();
    let case = "merge and drop";
    let arn = create_endpoint(&router, case).await;

    // Each step: what it exercises, the operation, its request members, and the pairs
    // expected afterwards.
    let steps: Vec<TagStep> = vec![
        (
            "tag two pairs",
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "dev" }, { "Key": "team", "Value": "sre" }] }),
            expect(&[("env", "dev"), ("team", "sre")]),
        ),
        (
            "tag a third pair, merged beside the first two",
            "TagResource",
            json!({ "Tags": [{ "Key": "owner", "Value": "ada" }] }),
            expect(&[("env", "dev"), ("owner", "ada"), ("team", "sre")]),
        ),
        (
            "tag an existing key, which takes the new value",
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
            expect(&[("env", "prod"), ("owner", "ada"), ("team", "sre")]),
        ),
        (
            "untag one key, whatever value it holds",
            "UntagResource",
            json!({ "TagKeys": ["env"] }),
            expect(&[("owner", "ada"), ("team", "sre")]),
        ),
        (
            "untag a key the resource never carried, a no-op",
            "UntagResource",
            json!({ "TagKeys": ["absent"] }),
            expect(&[("owner", "ada"), ("team", "sre")]),
        ),
        (
            "untag a key differing only in case, which is another key",
            "UntagResource",
            json!({ "TagKeys": ["OWNER"] }),
            expect(&[("owner", "ada"), ("team", "sre")]),
        ),
        (
            "tag a removed key again, stored afresh",
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "staging" }] }),
            expect(&[("env", "staging"), ("owner", "ada"), ("team", "sre")]),
        ),
        (
            "untag several keys at once, one of them absent",
            "UntagResource",
            json!({ "TagKeys": ["env", "owner", "absent"] }),
            expect(&[("team", "sre")]),
        ),
        (
            "untag the last key, emptying the map",
            "UntagResource",
            json!({ "TagKeys": ["team"] }),
            expect(&[]),
        ),
        (
            "untag again, which is idempotent",
            "UntagResource",
            json!({ "TagKeys": ["team"] }),
            expect(&[]),
        ),
    ];

    for (step, operation, members, expected) in steps {
        let mut body = json!({ "ResourceArn": arn });
        for (member, value) in members.as_object().expect("the members are an object") {
            body[member] = value.clone();
        }
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {step:?}: {operation} must succeed, got {status} {response}"
        );

        let actual = stored_tags(&router, step, &arn).await;
        assert_eq!(
            actual, expected,
            "case {step:?}: expected the stored tags {expected:?}, got {actual:?}"
        );
    }

    // The map is empty now, so `ListTagsForResource` reports no `Tags` member at all:
    // `TagList` has a minimum length of one, so an empty list is not a legal value.
    let (status, response) = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(
        (status, response["Tags"].is_null()),
        (200, true),
        "expected no Tags member once every tag was removed, got {status} {response}"
    );
}

/// RULE: the generic tagging operations do not apply to the certificate resource type,
/// so a certificate ARN is refused with HTTP 400 `ValidationException` naming the
/// certificate-specific operation to use instead — and the certificate's own tags,
/// which `AddTagsToCertificate` and `ListTagsForCertificate` own, are left alone.
#[tokio::test]
async fn generic_tagging_refuses_a_certificate_arn() {
    let router = router();
    let certificate = request_certificate(
        &router,
        &json!({ "DomainName": "generic-tagging.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let (status, response) = call(
        &router,
        "AddTagsToCertificate",
        &json!({ "CertificateArn": certificate, "Tags": [{ "Key": "env", "Value": "dev" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "AddTagsToCertificate must succeed, got {status} {response}"
    );

    // Both a certificate that exists and one that never did: the refusal is about the
    // resource TYPE the ARN names, so it does not depend on there being a record.
    let missing =
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555");
    let arns: Vec<ArnCase> = vec![
        ("an existing certificate's ARN", certificate.clone()),
        ("a certificate ARN with no record behind it", missing),
    ];

    // Each generic operation, its request members, and the certificate-specific
    // operation its refusal must name.
    let operations: Vec<(&str, Value, &str)> = vec![
        (
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
            "AddTagsToCertificate",
        ),
        (
            "UntagResource",
            json!({ "TagKeys": ["env"] }),
            "RemoveTagsFromCertificate",
        ),
        ("ListTagsForResource", json!({}), "ListTagsForCertificate"),
    ];

    for (case, arn) in &arns {
        for (operation, members, certificate_operation) in &operations {
            let mut body = json!({ "ResourceArn": arn });
            for (member, value) in members.as_object().expect("the members are an object") {
                body[member] = value.clone();
            }
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {case:?} on {operation}: expected 400 ValidationException, \
                 got {status} {response}"
            );
            let message = error_message(&response);
            assert!(
                message.contains(certificate_operation),
                "case {case:?} on {operation}: expected the refusal to name \
                 {certificate_operation}, got {message:?}"
            );
        }
    }

    // The certificate's own tags are untouched by every refused call above.
    let (status, response) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": certificate }),
    )
    .await;
    assert_eq!(
        (status, tag_pairs(&response["Tags"])),
        (200, expect(&[("env", "dev")])),
        "expected the certificate to keep the tags AddTagsToCertificate wrote, \
         got {status} {response}"
    );
}

/// RULE: a well-formed ACM resource ARN with no resource behind it — a resource never
/// created, one in another Region or account, or a resource type this account holds
/// none of — draws HTTP 400 `ResourceNotFoundException` from each generic tagging
/// operation, and stores nothing.
#[tokio::test]
async fn generic_tagging_refuses_an_unknown_resource_arn() {
    let router = router();
    // One stored, tagged endpoint, so the store is not empty and a miss is a real miss.
    let stored = create_endpoint(&router, "a stored endpoint").await;
    tag_resource(
        &router,
        "a stored endpoint",
        &stored,
        &json!([{ "Key": "env", "Value": "dev" }]),
    )
    .await;

    let arns: Vec<ArnCase> = vec![
        (
            "an ACME endpoint id that was never created",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/00000000-0000-0000-0000-000000000000"),
        ),
        (
            "the stored endpoint's id in another Region",
            stored.replace(REGION, "eu-central-1"),
        ),
        (
            "the stored endpoint's id in another account",
            stored.replace(ACCOUNT, "210987654321"),
        ),
        (
            "a resource type this account holds none of",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-account/account-1"),
        ),
        (
            "a resource type this service does not model at all",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:widget/42"),
        ),
    ];

    let operations: Vec<(&str, Value)> = vec![
        (
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "prod" }] }),
        ),
        ("UntagResource", json!({ "TagKeys": ["env"] })),
        ("ListTagsForResource", json!({})),
    ];

    for (case, arn) in &arns {
        for (operation, members) in &operations {
            let mut body = json!({ "ResourceArn": arn });
            for (member, value) in members.as_object().expect("the members are an object") {
                body[member] = value.clone();
            }
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} on {operation}: expected 400 ResourceNotFoundException, \
                 got {status} {response}"
            );
            let message = error_message(&response);
            assert!(
                message.contains(arn.as_str()),
                "case {case:?} on {operation}: expected the refusal to name the ARN \
                 {arn}, got {message:?}"
            );
        }
    }

    // The one resource that does exist is unchanged by every refusal above.
    let tags = stored_tags(&router, "a stored endpoint", &stored).await;
    assert_eq!(
        tags,
        expect(&[("env", "dev")]),
        "expected the stored resource to keep its tags, got {tags:?}"
    );
}

/// RULE: one resource carries at most the modelled `TagList` maximum of tags — a
/// request that would leave it with more is refused with HTTP 400
/// `ServiceQuotaExceededException` and writes nothing, while a request that stays
/// within the bound (one that only re-values keys the resource already carries) is
/// served.
#[tokio::test]
async fn tagging_past_the_tag_quota_is_refused() {
    let router = router();
    let case = "the tag quota";
    let arn = create_endpoint(&router, case).await;

    let at_bound: Vec<(String, String)> = (0..TAG_LIST_MAX)
        .map(|index| (format!("key{index:02}"), format!("value{index}")))
        .collect();
    let list = |pairs: &[(String, String)]| {
        Value::Array(
            pairs
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        )
    };

    // At the bound: served.
    tag_resource(&router, case, &arn, &list(&at_bound)).await;
    let stored = stored_tags(&router, case, &arn).await;
    assert_eq!(
        stored, at_bound,
        "case {case:?}: expected the {TAG_LIST_MAX} tags at the bound to be stored, got {stored:?}"
    );

    // One past the bound: refused, and nothing written. Each row is another way for the
    // merged map to exceed the bound.
    let over: Vec<(&str, Value)> = vec![
        (
            "one new key past the bound",
            json!([{ "Key": "one-too-many", "Value": "x" }]),
        ),
        (
            "a new key beside a key the resource already carries",
            json!([
                { "Key": at_bound[0].0, "Value": "rewritten" },
                { "Key": "one-too-many", "Value": "x" },
            ]),
        ),
        (
            "a full request of new keys",
            list(&(0..TAG_LIST_MAX)
                .map(|index| (format!("fresh{index:02}"), format!("value{index}")))
                .collect::<Vec<_>>()),
        ),
    ];
    for (row, request_tags) in over {
        let (status, response) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "Tags": request_tags }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ServiceQuotaExceededException"),
            "case {row:?}: expected 400 ServiceQuotaExceededException, got {status} {response}"
        );
        let after = stored_tags(&router, row, &arn).await;
        assert_eq!(
            after, at_bound,
            "case {row:?}: a refused TagResource must leave the stored tags alone, got {after:?}"
        );
    }

    // At the bound, re-valuing keys the resource already carries does not grow the map,
    // so it is served.
    let revalued: Vec<(String, String)> = at_bound
        .iter()
        .map(|(key, _)| (key.clone(), "rewritten".to_string()))
        .collect();
    tag_resource(&router, "re-valuing at the bound", &arn, &list(&revalued)).await;
    let stored = stored_tags(&router, "re-valuing at the bound", &arn).await;
    assert_eq!(
        stored, revalued,
        "case re-valuing at the bound: expected the new values to be stored, got {stored:?}"
    );

    // Freeing a key makes room for another one.
    let (status, response) = call(
        &router,
        "UntagResource",
        &json!({ "ResourceArn": arn, "TagKeys": [at_bound[0].0] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UntagResource must succeed, got {status} {response}"
    );
    tag_resource(
        &router,
        "the freed slot",
        &arn,
        &json!([{ "Key": "one-more", "Value": "x" }]),
    )
    .await;
    let stored = stored_tags(&router, "the freed slot", &arn).await;
    assert_eq!(
        stored.len(),
        TAG_LIST_MAX,
        "case the freed slot: expected the map to be back at the bound, got {stored:?}"
    );
    assert!(
        stored.contains(&("one-more".to_string(), "x".to_string())),
        "case the freed slot: expected the new pair to be stored, got {stored:?}"
    );
}

/// RULE: the `aws:` prefix is reserved on both halves of a pair, so neither tagging nor
/// untagging a resource may name it — each refusal is HTTP 400 `ValidationException`
/// (the only input error these operations model) and leaves the stored map as it was.
#[tokio::test]
async fn reserved_tags_are_refused() {
    let router = router();
    let case = "reserved tags";
    let arn = create_endpoint(&router, case).await;
    tag_resource(
        &router,
        case,
        &arn,
        &json!([{ "Key": "env", "Value": "dev" }]),
    )
    .await;
    let before = stored_tags(&router, case, &arn).await;

    // Each row: what it exercises, the operation and the request members it names.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a reserved tag key on TagResource",
            "TagResource",
            json!({ "Tags": [{ "Key": "aws:internal", "Value": "x" }] }),
        ),
        (
            "a reserved tag value on TagResource",
            "TagResource",
            json!({ "Tags": [{ "Key": "stage", "Value": "aws:internal" }] }),
        ),
        (
            "a reserved pair beside an allowed one on TagResource",
            "TagResource",
            json!({
                "Tags": [
                    { "Key": "allowed", "Value": "y" },
                    { "Key": "aws:internal", "Value": "x" },
                ],
            }),
        ),
        (
            "a reserved tag key on UntagResource",
            "UntagResource",
            json!({ "TagKeys": ["aws:internal"] }),
        ),
        (
            "a reserved key beside a removable one on UntagResource",
            "UntagResource",
            json!({ "TagKeys": ["env", "aws:internal"] }),
        ),
    ];

    for (row, operation, members) in rows {
        let mut body = json!({ "ResourceArn": arn });
        for (member, value) in members.as_object().expect("the members are an object") {
            body[member] = value.clone();
        }
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {row:?}: expected 400 ValidationException, got {status} {response}"
        );
        let after = stored_tags(&router, row, &arn).await;
        assert_eq!(
            after, before,
            "case {row:?}: a refused request must leave the stored tags {before:?}, got {after:?}"
        );
    }
}
