// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTagsForResource`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.ListTagsForResource`) through the same router the emulator boots.
//!
//! RULE under test: `ListTagsForResource` reports the tag list persisted on the
//! resource whose ARN it is given — for every taggable resource kind this service
//! holds, and whatever the size of that list, an untagged resource included. An ARN this
//! service cannot act on is refused with the modelled `ValidationException` (400), worded
//! per the way it failed: a value that does not satisfy the ARN grammar is reported WITH
//! that value, and an ARN that parses and addresses no resource of this account is the one
//! fixed sentence that echoes nothing.
//!
//! Each test asserts that rule over a TABLE of inputs rather than one recorded
//! request: the rows differ along the resource kind, the number of tags, the tag
//! key/value spellings, the ARN's shape and the signing region.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{invalid_arn, INVALID_INPUT_ARN};
use smithy_server_runtime::MultiServiceRouter;

// The request harness is shared with the onboarding suite — same router, same signing.
use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, capability_configuration, error_code,
    given_capability_buckets, given_transformer, text, ACCOUNT, SIGNING_REGION,
};

/// A `tags` list for a create request, from `(key, value)` pairs. Member names are
/// `com.amazonaws.b2bi#Tag`'s own: `Key` and `Value`.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    json!(pairs
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect::<Vec<_>>())
}

/// The `(Key, Value)` pairs of a `Tags` list, sorted, so a comparison is about the
/// CONTENT of the list and not about the order the service happens to report it in.
pub(crate) fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|tag| (text(tag, "Key"), text(tag, "Value")))
        .collect();
    pairs.sort();
    pairs
}

/// The `message` of an error body. This service's model spells that member `Message`
/// on `ValidationException` and `message` on `ResourceNotFoundException`, so both
/// spellings are read here.
pub(crate) fn message(body: &Value) -> String {
    let capitalized = text(body, "Message");
    if capitalized.is_empty() {
        text(body, "message")
    } else {
        capitalized
    }
}

/// Ask `ListTagsForResource` for `arn`, signed for `region`.
pub(crate) async fn list_tags_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> (u16, Value) {
    call_signed_for(
        router,
        region,
        "ListTagsForResource",
        json!({ "ResourceARN": arn }),
    )
    .await
}

/// Ask `ListTagsForResource` for `arn`, signed for [`SIGNING_REGION`].
pub(crate) async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    list_tags_in(router, SIGNING_REGION, arn).await
}

/// One row: the case name, the resource kind, and the tags the resource is created with
/// (`None` omits the `tags` member entirely).
type TagCase<'a> = (&'a str, Kind, Option<&'a [(&'a str, &'a str)]>);

/// One row of the region table: the case name, the region the profile is created in,
/// and the tags it carries.
type RegionCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Which of this service's taggable resources a row creates. AWS: tags apply to
/// "capabilities, partnerships, profiles and transformers" (`com.amazonaws.b2bi#Tag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Profile,
    Capability,
    Partnership,
    Transformer,
}

/// Create a resource of `kind` carrying `tags`, and return its ARN.
///
/// `profile_id` / `capability_id` are the references a partnership needs; they are
/// created once by the caller so every row shares them.
pub(crate) async fn given_tagged_resource(
    router: &MultiServiceRouter,
    kind: Kind,
    name: &str,
    tags: Option<&[(&str, &str)]>,
    profile_id: &str,
    capability_id: &str,
) -> String {
    // `TransformerName` is `^[a-zA-Z0-9_-]{1,512}$`, so a case label is reduced to the
    // characters that constraint accepts before it is used as a transformer's name.
    let modelled_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // A capability's locations have to be buckets this account owns, so the two the
    // configuration below names are created before any row asks for one.
    given_capability_buckets(router).await;
    let (operation, mut request, arn_member) = match kind {
        Kind::Profile => (
            "CreateProfile",
            json!({
                "name": name,
                "phone": "555-555-0100",
                "businessName": format!("{name} Corp"),
                "logging": "DISABLED"
            }),
            "profileArn",
        ),
        Kind::Capability => (
            "CreateCapability",
            json!({
                "name": name,
                "type": "edi",
                // A capability resolves its `transformerId`, so each one gets a real
                // transformer to name.
                "configuration": capability_configuration(
                    &given_transformer(router, &format!("{name}-transformer")).await,
                    "X12_850",
                    "VERSION_4010",
                )
            }),
            "capabilityArn",
        ),
        Kind::Partnership => (
            "CreatePartnership",
            json!({
                "profileId": profile_id,
                "name": name,
                "email": "partner@example.com",
                "capabilities": [capability_id]
            }),
            "partnershipArn",
        ),
        Kind::Transformer => (
            "CreateTransformer",
            json!({
                "name": modelled_name,
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                }
            }),
            "transformerArn",
        ),
    };
    if let Some(tags) = tags {
        request["tags"] = tag_list(tags);
    }

    let (status, created) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: {operation} failed: {status} {created}"
    );
    let arn = text(&created, arn_member);
    assert!(
        !arn.is_empty(),
        "case {name:?}: {operation} returned no {arn_member}: {created}"
    );
    arn
}

/// RULE: `ListTagsForResource` reports the tags persisted on the resource its ARN
/// names, for every taggable resource kind, and an EMPTY list for a resource that
/// carries none.
///
/// The rows span the three resource kinds this service persists, the `tags` member
/// omitted / empty / one / three entries, a key reused across resources with a
/// different value, and the modelled `TagValue` minimum length (`len 0..256`, so an
/// empty value is legal).
#[tokio::test]
async fn list_tags_reports_the_tags_each_resource_was_created_with() {
    let three = &[
        ("env", "prod"),
        ("team", "integration"),
        ("cost-center", "4711"),
    ];
    let cases: &[TagCase<'_>] = &[
        ("profile, tags omitted", Kind::Profile, None),
        ("profile, empty tag list", Kind::Profile, Some(&[])),
        ("profile, one tag", Kind::Profile, Some(&[("env", "prod")])),
        ("profile, three tags", Kind::Profile, Some(three)),
        ("capability, tags omitted", Kind::Capability, None),
        (
            "capability, one tag, same key different value",
            Kind::Capability,
            Some(&[("env", "staging")]),
        ),
        ("capability, three tags", Kind::Capability, Some(three)),
        ("partnership, empty tag list", Kind::Partnership, Some(&[])),
        (
            "partnership, empty tag value",
            Kind::Partnership,
            // `TagValue` is `len 0..256`, so "" is a legal value, not an absent one.
            Some(&[("retain-until", "")]),
        ),
        ("partnership, three tags", Kind::Partnership, Some(three)),
    ];

    let router = b2bi_router();
    given_capability_buckets(&router).await;
    // The references a partnership row needs, created once and untagged themselves.
    let profile_id = text(
        &call(
            &router,
            "CreateProfile",
            json!({
                "name": "partnership host",
                "phone": "555-555-0101",
                "businessName": "Partnership Host Corp",
                "logging": "DISABLED"
            }),
        )
        .await
        .1,
        "profileId",
    );
    let capability_id = text(
        &call(
            &router,
            "CreateCapability",
            json!({
                "name": "partnership capability",
                "type": "edi",
                "configuration": capability_configuration(
                    &given_transformer(&router, "partnership-transformer").await,
                    "X12_850",
                    "VERSION_4010",
                )
            }),
        )
        .await
        .1,
        "capabilityId",
    );

    for (case, kind, tags) in cases {
        let arn = given_tagged_resource(
            &router,
            *kind,
            case,
            *tags,
            &profile_id,
            &capability_id,
        )
        .await;

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource({arn}) must be 200, got {status} {body}"
        );
        assert!(
            body["Tags"].is_array(),
            "case {case:?}: Tags must be a list even when empty, got {:?}",
            body["Tags"]
        );

        let mut expected: Vec<(String, String)> = tags
            .unwrap_or(&[])
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            tag_pairs(&body["Tags"]),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            body["Tags"]
        );

        // Reading tags changes nothing: a second call reports the same list.
        let (status, again) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second ListTagsForResource must be 200, got {status} {again}"
        );
        assert_eq!(
            tag_pairs(&again["Tags"]),
            expected,
            "case {case:?}: a read must not change the stored tags, got {:?}",
            again["Tags"]
        );
    }
}

/// RULE: a `ResourceARN` that does not satisfy the ARN grammar
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` is refused with the
/// modelled `ValidationException` (HTTP 400) reporting the value that was rejected — the
/// recorded wording for a value that is not an ARN at all, which is a different sentence
/// from the one an ARN that parses and resolves to nothing gets (the other test below).
///
/// The rows span a value with no colons at all, a truncated ARN at each field count
/// the grammar needs, an empty partition / service / resource field, the `arn` prefix
/// misspelled and mis-cased, and leading junk before it.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_a_validation_exception() {
    let router = b2bi_router();
    // Created so the suite cannot pass by having nothing in the store at all.
    let arn = given_tagged_resource(
        &router,
        Kind::Profile,
        "syntax guard",
        Some(&[("env", "prod")]),
        "",
        "",
    )
    .await;

    let cases: &[(&str, String)] = &[
        ("no colons at all", "this-is-not-an-arn-at-all".to_string()),
        ("prefix only", "arn".to_string()),
        ("partition only", "arn:aws".to_string()),
        ("service only", "arn:aws:b2bi".to_string()),
        ("no account or resource", format!("arn:aws:b2bi:{SIGNING_REGION}")),
        (
            "no resource field",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}"),
        ),
        (
            "empty resource field",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:"),
        ),
        (
            "empty partition field",
            format!("arn::b2bi:{SIGNING_REGION}:{ACCOUNT}:profile/p-0123456789abcdef0"),
        ),
        (
            "empty service field",
            format!("arn:aws::{SIGNING_REGION}:{ACCOUNT}:profile/p-0123456789abcdef0"),
        ),
        (
            "prefix misspelled",
            arn.replacen("arn:", "anr:", 1),
        ),
        (
            "prefix upper-cased",
            arn.replacen("arn:", "ARN:", 1),
        ),
        ("leading junk before the prefix", format!("xarn:{arn}")),
        ("the resource portion on its own", "profile/p-0123456789abcdef0".to_string()),
    ];

    for (case, candidate) in cases {
        let (status, body) = list_tags(&router, candidate).await;
        assert_eq!(
            status, 400,
            "case {case:?}: ListTagsForResource({candidate}) must be 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            invalid_arn(candidate),
            "case {case:?}: the refusal must report the rejected value, got {body}"
        );
    }
}

/// RULE: an ARN that PARSES and names no resource this service holds is refused with the
/// modelled `ValidationException` (HTTP 400) too, carrying the one fixed sentence this
/// service states for an ARN that addresses nothing — because a well-formed ARN is
/// RESOLVED rather than merely parsed: it addressed no resource, including a stored
/// resource's own ARN once that resource is deleted, and one that differs from a stored one
/// only in the region or the account it names.
///
/// The rows span an unknown id of each resource kind, another service's ARN, a global
/// ARN with the region and account fields empty, a different partition, a rewritten
/// region, a rewritten account, and a deleted profile's ARN.
#[tokio::test]
async fn a_well_formed_arn_naming_nothing_stored_is_refused_as_an_invalid_input_arn() {
    let router = b2bi_router();
    let live = given_tagged_resource(
        &router,
        Kind::Profile,
        "live profile",
        Some(&[("env", "prod")]),
        "",
        "",
    )
    .await;
    // A second profile, deleted below, so its ARN is well-formed but unresolvable.
    let (status, created) = call(
        &router,
        "CreateProfile",
        json!({
            "name": "doomed profile",
            "phone": "555-555-0102",
            "businessName": "Doomed Corp",
            "logging": "DISABLED",
            "tags": tag_list(&[("env", "scratch")])
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateProfile(doomed) failed: {created}");
    let doomed_arn = text(&created, "profileArn");
    let doomed_id = text(&created, "profileId");

    // Its tags ARE visible while it exists — the refusal below is about the delete, not
    // about this ARN being unreadable.
    let (status, body) = list_tags(&router, &doomed_arn).await;
    assert_eq!(status, 200, "the doomed profile's tags must be readable: {body}");
    let (status, body) = call(&router, "DeleteProfile", json!({ "profileId": doomed_id })).await;
    assert_eq!(status, 200, "DeleteProfile({doomed_id}) failed: {status} {body}");

    let cases: &[(&str, String)] = &[
        (
            "unknown profile id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:profile/p-0123456789abcdef0"),
        ),
        (
            "unknown capability id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:capability/ca-0123456789abcdef0"),
        ),
        (
            "unknown partnership id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:partnership/ps-0123456789abcdef0"),
        ),
        (
            "unknown resource type",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:transformer/t-0123456789abcdef0"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{SIGNING_REGION}:{ACCOUNT}:bucket/not-a-b2bi-resource"),
        ),
        (
            "a global ARN with empty region and account",
            "arn:aws:s3:::some-bucket".to_string(),
        ),
        (
            "a stored ARN in another partition",
            live.replacen("arn:aws:", "arn:aws-us-gov:", 1),
        ),
        (
            "a stored ARN with the region rewritten",
            live.replacen(SIGNING_REGION, "eu-central-1", 1),
        ),
        (
            "a stored ARN with the account rewritten",
            live.replacen(ACCOUNT, "123456789012", 1),
        ),
        ("a deleted profile's ARN", doomed_arn.clone()),
    ];

    for (case, candidate) in cases {
        let (status, body) = list_tags(&router, candidate).await;
        assert_eq!(
            status, 400,
            "case {case:?}: ListTagsForResource({candidate}) must be 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            INVALID_INPUT_ARN,
            "case {case:?}: the refusal is this service's one fixed sentence, got {body}"
        );
        assert!(
            !message(&body).contains(candidate.as_str()),
            "case {case:?}: the refusal must not echo the ARN that resolved to nothing, got \
             {body}"
        );
    }

    // The live resource is untouched by every refusal above.
    let (status, body) = list_tags(&router, &live).await;
    assert_eq!(status, 200, "the live profile's tags must still be readable: {body}");
    assert_eq!(
        tag_pairs(&body["Tags"]),
        vec![("env".to_string(), "prod".to_string())],
        "the live profile's tags must be unchanged, got {:?}",
        body["Tags"]
    );
}

/// RULE: the tags belong to the resource the ARN names, not to the caller — so the
/// stored ARN reports the same tags whatever region the read is signed for.
///
/// The rows span three signing regions for the read, against resources created in two.
#[tokio::test]
async fn tags_are_read_off_the_arn_whatever_region_the_read_is_signed_for() {
    let router = b2bi_router();

    // (case, the region the profile is created in, the tags it carries)
    let created_in: &[RegionCase<'_>] = &[
        ("west profile", "us-west-2", &[("env", "prod")]),
        (
            "frankfurt profile",
            "eu-central-1",
            &[("env", "dr"), ("team", "integration")],
        ),
    ];
    // The regions the READ is signed for; none of them changes the answer.
    let read_from = ["us-west-2", "eu-central-1", "ap-southeast-1"];

    for (case, region, tags) in created_in {
        let (status, created) = call_signed_for(
            &router,
            region,
            "CreateProfile",
            json!({
                "name": case.to_string(),
                "phone": "555-555-0103",
                "businessName": format!("{case} Corp"),
                "logging": "ENABLED",
                "tags": tag_list(tags)
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateProfile failed: {created}");
        let arn = text(&created, "profileArn");
        assert!(
            arn.contains(region),
            "case {case:?}: the ARN must name the creating region, got {arn:?}"
        );

        let mut expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        expected.sort();

        for reader in read_from {
            let (status, body) = list_tags_in(&router, reader, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} read from {reader}: expected 200, got {status} {body}"
            );
            assert_eq!(
                tag_pairs(&body["Tags"]),
                expected,
                "case {case:?} read from {reader}: expected {expected:?}, got {:?}",
                body["Tags"]
            );
        }
    }
}
