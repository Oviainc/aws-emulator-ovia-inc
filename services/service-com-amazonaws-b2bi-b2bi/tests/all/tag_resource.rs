// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.TagResource`) through the same router the emulator boots, and read
//! back through `ListTagsForResource` so every assertion is about persisted state.
//!
//! RULE under test: `TagResource` attaches the key-value pairs it is given to the
//! resource whose ARN it names — a key the resource does not carry yet is ADDED, a key it
//! already carries has its value REPLACED, and every other tag on that resource, and
//! every other resource, is left as it was. An ARN this write cannot act on is refused
//! with the modelled `ValidationException` (400) and writes nothing — both a value that
//! does not satisfy the ARN grammar, reported with that value, and a well-formed ARN
//! naming no resource this service holds, reported with the fixed sentence this service
//! states for an ARN that addresses nothing. `ValidationException` for the second class is
//! the answer RECORDED against AWS for this operation (trace
//! `b2bi.tag-resource-nonexistent-arn-not-found`) and for the reporting
//! `ListTagsForResource` too (see `tagging.rs`).
//!
//! Each test asserts that rule over a TABLE of inputs rather than one recorded request:
//! the rows differ along the resource kind, the tags the resource already carries, the
//! number and spelling of the pairs applied, the signing region of the write, and the
//! published `TagList` bound (`len 0..200`) at and past its maximum.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{invalid_arn, INVALID_INPUT_ARN};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness is shared with the onboarding suite, and the tagging fixtures with the
// `ListTagsForResource` suite — same router, same signing, one copy of each helper.
use crate::onboarding_loop::{call, call_signed_for, error_code, text, ACCOUNT, SIGNING_REGION};
use crate::tagging::{given_tagged_resource, list_tags, message, tag_list, tag_pairs, Kind};

/// Attach `tags` to `arn`, signed for `region`.
async fn tag_resource_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    call_signed_for(
        router,
        region,
        "TagResource",
        // Input member names are `com.amazonaws.b2bi#TagResourceRequest`'s own.
        json!({ "ResourceARN": arn, "Tags": tag_list(tags) }),
    )
    .await
}

/// Attach `tags` to `arn`, signed for [`SIGNING_REGION`].
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    tag_resource_in(router, SIGNING_REGION, arn, tags).await
}

/// `(key, value)` pairs as the sorted form `tag_pairs` returns, for comparison.
pub(crate) fn expected_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    owned.sort();
    owned
}

/// The tags `ListTagsForResource` reports for `arn`, sorted. Asserts the read succeeded.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must be 200, got {status} {body}"
    );
    tag_pairs(&body["Tags"])
}

/// Both tagging writes model `output: {}` — "There is no response returned from this
/// call" — so a success carries HTTP 200 and no members.
pub(crate) fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: the call must answer 200, got {status} {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: the call returns no response members, got {body}"
    );
}

/// One row: the case name, the resource kind, the tags the resource is CREATED with, the
/// pairs `TagResource` applies, and the tag set the resource must then carry.
type UpsertCase<'a> = (
    &'a str,
    Kind,
    Option<&'a [(&'a str, &'a str)]>,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
);

/// RULE: `TagResource` adds the keys the resource does not carry, replaces the value of
/// the keys it does, and leaves its other tags alone — on every taggable resource kind
/// this service holds (AWS: "capabilities, partnerships, profiles and transformers").
///
/// The rows span the four resource kinds, a resource created untagged / with an empty tag
/// list / with several tags, applying zero / one / several pairs, a pure add, a pure
/// replace, a mixed add-and-replace, a value replaced by the empty string (the modelled
/// `TagValue` minimum, `len 0..256`), the same key named twice in one request, and a
/// 128-character key (the modelled `TagKey` maximum).
#[tokio::test]
async fn tag_resource_adds_new_keys_and_replaces_the_values_of_known_ones() {
    let max_key: String = "k".repeat(128);
    let max_key: &str = &max_key;
    let three = &[
        ("env", "prod"),
        ("team", "integration"),
        ("cost-center", "4711"),
    ];
    let cases: &[UpsertCase<'_>] = &[
        (
            "profile, untagged, one pair added",
            Kind::Profile,
            None,
            &[("env", "prod")],
            &[("env", "prod")],
        ),
        (
            "profile, empty tag list, three pairs added",
            Kind::Profile,
            Some(&[]),
            three,
            three,
        ),
        (
            "profile, nothing applied is a no-op",
            Kind::Profile,
            Some(&[("env", "prod")]),
            &[],
            &[("env", "prod")],
        ),
        (
            "capability, one key's value replaced",
            Kind::Capability,
            Some(&[("env", "staging")]),
            &[("env", "prod")],
            &[("env", "prod")],
        ),
        (
            "capability, add beside the tags it was created with",
            Kind::Capability,
            Some(three),
            &[("owner", "edi-team")],
            &[
                ("env", "prod"),
                ("team", "integration"),
                ("cost-center", "4711"),
                ("owner", "edi-team"),
            ],
        ),
        (
            "partnership, mixed add and replace",
            Kind::Partnership,
            Some(&[("env", "staging"), ("team", "integration")]),
            &[("env", "prod"), ("owner", "edi-team")],
            &[
                ("env", "prod"),
                ("team", "integration"),
                ("owner", "edi-team"),
            ],
        ),
        (
            "partnership, value replaced by the empty string",
            Kind::Partnership,
            Some(&[("retain-until", "2026-01-01")]),
            &[("retain-until", "")],
            &[("retain-until", "")],
        ),
        (
            "transformer, untagged, three pairs added",
            Kind::Transformer,
            None,
            three,
            three,
        ),
        (
            "transformer, the same key twice in one request keeps the last value",
            Kind::Transformer,
            None,
            &[("env", "staging"), ("env", "prod")],
            &[("env", "prod")],
        ),
        (
            "transformer, a 128-character key",
            Kind::Transformer,
            Some(&[("env", "prod")]),
            &[(max_key, "at-the-modelled-key-maximum")],
            &[("env", "prod"), (max_key, "at-the-modelled-key-maximum")],
        ),
    ];

    let (router, profile_id, capability_id) = b2bi_fixtures().await;

    for (case, kind, created, applied, expected) in cases {
        let arn = given_tagged_resource(
            &router,
            *kind,
            case,
            *created,
            &profile_id,
            &capability_id,
        )
        .await;

        let (status, body) = tag_resource(&router, &arn, applied).await;
        assert_empty_success(case, status, &body);

        let expected = expected_pairs(expected);
        assert_eq!(
            stored_tags(&router, case, &arn).await,
            expected,
            "case {case:?}: after TagResource({applied:?}) the stored tags must be {expected:?}"
        );

        // Idempotent: the same request again leaves the same tag set, so a retried call
        // neither duplicates a key nor drops one.
        let (status, body) = tag_resource(&router, &arn, applied).await;
        assert_empty_success(&format!("{case} (replayed)"), status, &body);
        assert_eq!(
            stored_tags(&router, case, &arn).await,
            expected,
            "case {case:?}: replaying TagResource must leave the tags at {expected:?}"
        );
    }
}

/// RULE: a write reaches only the resource its ARN names — the other resources this
/// service holds, of every kind, keep the tags they had.
///
/// The rows are one resource of each taggable kind, each tagged in turn while the others
/// are asserted unchanged.
#[tokio::test]
async fn tag_resource_writes_only_the_resource_its_arn_names() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let kinds = [
        Kind::Profile,
        Kind::Capability,
        Kind::Partnership,
        Kind::Transformer,
    ];

    // Every resource starts from the same tag set, so a write that leaked onto a
    // neighbour would show up as a changed value rather than as a missing key.
    let mut resources: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for kind in kinds {
        let arn = given_tagged_resource(
            &router,
            kind,
            &format!("isolation-{kind:?}"),
            Some(&[("env", "shared"), ("team", "integration")]),
            &profile_id,
            &capability_id,
        )
        .await;
        resources.push((arn, expected_pairs(&[("env", "shared"), ("team", "integration")])));
    }

    for index in 0..resources.len() {
        let case = format!("writing to {:?}", kinds[index]);
        let value = format!("owned-by-{index}");
        let (status, body) =
            tag_resource(&router, &resources[index].0, &[("env", &value)]).await;
        assert_empty_success(&case, status, &body);
        resources[index].1 = expected_pairs(&[("env", &value), ("team", "integration")]);

        for (arn, expected) in &resources {
            assert_eq!(
                &stored_tags(&router, &case, arn).await,
                expected,
                "case {case:?}: the tags of {arn} must be {expected:?}"
            );
        }
    }
}

/// RULE: a `ResourceARN` that does not satisfy the ARN grammar
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` is refused with the
/// modelled `ValidationException` (HTTP 400) — a client-side syntax error, answered
/// without any lookup at all — and the refusal writes nothing.
///
/// The rows span a value with no colons at all, a truncated ARN at each field count the
/// grammar needs, an empty partition / service / resource field, the `arn` prefix
/// misspelled and mis-cased, leading junk before it, and a bare resource portion.
#[tokio::test]
async fn an_arn_that_is_not_an_arn_is_refused_with_validation_exception() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let live = given_tagged_resource(
        &router,
        Kind::Profile,
        "syntax guard",
        Some(&[("env", "prod")]),
        &profile_id,
        &capability_id,
    )
    .await;

    let cases: &[(&str, String)] = &[
        ("no colons at all", "this-is-not-an-arn-at-all".to_string()),
        ("prefix only", "arn".to_string()),
        ("partition only", "arn:aws".to_string()),
        ("service only", "arn:aws:b2bi".to_string()),
        (
            "no account or resource",
            format!("arn:aws:b2bi:{SIGNING_REGION}"),
        ),
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
        ("prefix misspelled", live.replacen("arn:", "anr:", 1)),
        ("prefix upper-cased", live.replacen("arn:", "ARN:", 1)),
        ("leading junk before the prefix", format!("xarn:{live}")),
        (
            "the resource portion on its own",
            "profile/p-0123456789abcdef0".to_string(),
        ),
    ];

    for (case, candidate) in cases {
        let (status, body) = tag_resource(&router, candidate, &[("env", "written")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: TagResource({candidate}) must be 400, got {status} {body}"
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

    assert_eq!(
        stored_tags(&router, "live profile after every refusal", &live).await,
        expected_pairs(&[("env", "prod")]),
        "no refused call may have written a tag"
    );
}

/// RULE: a well-formed ARN that this WRITE cannot act on — one that names no resource this
/// service holds — is refused with the modelled `ValidationException` (HTTP 400) and
/// writes nothing, including a stored resource's own ARN once it is deleted and an ARN
/// differing from a stored one only in the partition, the region or the account it names.
///
/// `ValidationException` rather than `ResourceNotFoundException` is the RECORDED answer
/// for every tagging operation of this service: trace
/// `b2bi.tag-resource-nonexistent-arn-not-found` records it for this write, and the
/// recorded `b2bi-tagging-workflow` step records the same class, with the one fixed
/// sentence asserted here, for the REPORTING `ListTagsForResource` (see `tagging.rs`) — a
/// well-formed ARN is resolved, and one that resolves to nothing addressed no resource
/// wherever it arrives.
///
/// The rows span an unknown id of each resource kind, another service's ARN, a global ARN
/// with the region and account fields empty, a rewritten partition / region / account, and
/// a deleted profile's ARN.
#[tokio::test]
async fn a_well_formed_arn_naming_nothing_stored_is_refused_and_writes_nothing() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let live = given_tagged_resource(
        &router,
        Kind::Profile,
        "live profile",
        Some(&[("env", "prod")]),
        &profile_id,
        &capability_id,
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
    let (status, body) = call(&router, "DeleteProfile", json!({ "profileId": doomed_id })).await;
    assert_eq!(
        status, 200,
        "DeleteProfile({doomed_id}) failed: {status} {body}"
    );

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
            "unknown transformer id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:transformer/tr-0123456789abcdef0"),
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
        let (status, body) = tag_resource(&router, candidate, &[("env", "written")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: TagResource({candidate}) must be 400, got {status} {body}"
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

    assert_eq!(
        stored_tags(&router, "live profile after every refusal", &live).await,
        expected_pairs(&[("env", "prod")]),
        "no refused call may have written a tag"
    );
}

/// RULE: the tags belong to the resource the ARN names, not to the caller — so a write
/// reaches the stored resource whatever region it is signed for, and a resource created in
/// one region is tagged by a call signed for another.
///
/// The rows span three signing regions for the write, against a resource created in a
/// fourth.
#[tokio::test]
async fn tags_are_written_to_the_arn_whatever_region_the_write_is_signed_for() {
    let router = crate::onboarding_loop::b2bi_router();
    let (status, created) = call_signed_for(
        &router,
        "eu-central-1",
        "CreateProfile",
        json!({
            "name": "frankfurt profile",
            "phone": "555-555-0103",
            "businessName": "Frankfurt Corp",
            "logging": "DISABLED"
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateProfile failed: {created}");
    let arn = text(&created, "profileArn");
    assert!(
        arn.contains("eu-central-1"),
        "the ARN must name the creating region, got {arn:?}"
    );

    for region in ["us-west-2", "ap-southeast-1", "eu-central-1"] {
        let value = format!("written-from-{region}");
        let (status, body) = tag_resource_in(&router, region, &arn, &[("writer", &value)]).await;
        assert_empty_success(region, status, &body);
        assert_eq!(
            stored_tags(&router, region, &arn).await,
            expected_pairs(&[("writer", &value)]),
            "case {region:?}: a write signed elsewhere must still reach {arn}"
        );
    }
}

/// RULE (bound): a resource holds at most as many tags as the modelled `TagList` can
/// report (`len 0..200`), so a call that would carry it past that maximum is refused with
/// the modelled `ValidationException` and writes nothing, while a call that replaces the
/// value of a key it already holds is accepted at the maximum.
///
/// The rows walk the published bound: a request at the `TagList` maximum, a request one
/// entry past it (refused before this handler, by the modelled list constraint), the step
/// that would take a full resource to 201 tags, and a replacement at exactly 200.
#[tokio::test]
async fn a_resource_holds_at_most_the_modelled_tag_list_maximum() {
    // The maximum of `com.amazonaws.b2bi#TagList` (`length: min 0, max 200`), which is
    // also the shape `ListTagsForResourceResponse$Tags` reports a resource's tags through.
    const TAG_LIST_MAX: usize = 200;

    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let arn = given_tagged_resource(
        &router,
        Kind::Profile,
        "at the bound",
        Some(&[]),
        &profile_id,
        &capability_id,
    )
    .await;

    // One request at the bound: 200 distinct keys.
    let keys: Vec<String> = (0..TAG_LIST_MAX).map(|index| format!("key-{index:03}")).collect();
    let full: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v1")).collect();
    let (status, body) = tag_resource(&router, &arn, &full).await;
    assert_empty_success("a request at the TagList maximum", status, &body);
    let at_bound = stored_tags(&router, "at the TagList maximum", &arn).await;
    assert_eq!(
        at_bound.len(),
        TAG_LIST_MAX,
        "a request at the TagList maximum must store every pair, got {} tags",
        at_bound.len()
    );

    // One past the bound, in a single request: the modelled list constraint refuses it
    // before the handler runs, and it is still a 400 `ValidationException`.
    let one_past: Vec<(&str, &str)> = keys
        .iter()
        .map(|key| (key.as_str(), "v2"))
        .chain(std::iter::once(("one-too-many", "v2")))
        .collect();
    let (status, body) = tag_resource(&router, &arn, &one_past).await;
    assert_eq!(
        status, 400,
        "case \"one past the TagList maximum\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"one past the TagList maximum\": expected ValidationException, got {body}"
    );

    // One past the bound, reached across two calls: refused, and nothing is written.
    let (status, body) = tag_resource(&router, &arn, &[("one-too-many", "v2")]).await;
    assert_eq!(
        status, 400,
        "case \"a 201st tag on a full resource\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a 201st tag on a full resource\": expected ValidationException, got {body}"
    );
    assert_eq!(
        stored_tags(&router, "a 201st tag on a full resource", &arn).await,
        at_bound,
        "a refused call must leave the stored tags exactly as they were"
    );

    // At the bound, a key the resource already holds is REPLACED, not added, so it fits.
    let (status, body) = tag_resource(&router, &arn, &[(keys[0].as_str(), "replaced")]).await;
    assert_empty_success("a replacement at the TagList maximum", status, &body);
    let after = stored_tags(&router, "a replacement at the TagList maximum", &arn).await;
    assert_eq!(
        after.len(),
        TAG_LIST_MAX,
        "a replacement must not change the tag count, got {} tags",
        after.len()
    );
    assert!(
        after.contains(&(keys[0].clone(), "replaced".to_string())),
        "a replacement at the maximum must take effect, got {after:?}"
    );
}

/// A router plus the `profileId` and `capabilityId` a partnership row needs, created once
/// and untagged themselves.
pub(crate) async fn b2bi_fixtures() -> (MultiServiceRouter, String, String) {
    let router = crate::onboarding_loop::b2bi_router();
    let profile_id = crate::onboarding_loop::given_profile(&router, "tag-host").await;
    let capability_id = crate::onboarding_loop::given_capability(&router, "tag-capability").await;
    (router, profile_id, capability_id)
}
