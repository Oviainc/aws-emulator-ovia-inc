// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.UntagResource`) through the same router the emulator boots, and
//! read back through `ListTagsForResource` so every assertion is about persisted state.
//!
//! RULE under test: `UntagResource` detaches the keys it names from the resource whose ARN
//! it names — a key the resource carries is REMOVED with its value, a key it does not
//! carry is ignored, and every other tag on that resource, and every other resource, is
//! left as it was. An ARN this service cannot act on is refused with the modelled
//! `ValidationException` (400) — a value that fails the ARN grammar reported with that
//! value, a well-formed ARN naming nothing reported with the fixed sentence for an ARN that
//! addresses nothing — and no refusal removes anything.
//!
//! Each test asserts that rule over a TABLE of inputs rather than one recorded request:
//! the rows differ along the resource kind, the tags the resource carries, the number and
//! spelling of the keys removed, and whether those keys are present at all.

use serde_json::json;
use service_com_amazonaws_b2bi_b2bi::state::{invalid_arn, INVALID_INPUT_ARN};
use smithy_server_runtime::MultiServiceRouter;

// One copy of the wire harness and of the tagging fixtures, shared with the other suites.
use crate::onboarding_loop::{call, call_signed_for, error_code, text, ACCOUNT, SIGNING_REGION};
use crate::tag_resource::{
    assert_empty_success, b2bi_fixtures, expected_pairs, stored_tags, tag_resource,
};
use crate::tagging::{given_tagged_resource, message, tag_list, Kind};

/// Detach `keys` from `arn`, signed for [`SIGNING_REGION`].
pub(crate) async fn untag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    keys: &[&str],
) -> (u16, serde_json::Value) {
    call_signed_for(
        router,
        SIGNING_REGION,
        "UntagResource",
        // Input member names are `com.amazonaws.b2bi#UntagResourceRequest`'s own.
        json!({ "ResourceARN": arn, "TagKeys": keys }),
    )
    .await
}

/// One row: the case name, the resource kind, the tags the resource is CREATED with, the
/// keys `UntagResource` detaches, and the tag set the resource must then carry.
type UntagCase<'a> = (
    &'a str,
    Kind,
    Option<&'a [(&'a str, &'a str)]>,
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
);

/// RULE: `UntagResource` removes exactly the keys it names and keeps the rest, on every
/// taggable resource kind this service holds, and is `@idempotent` — a key that is not
/// there is not an error, and a replay leaves the same tag set.
///
/// The rows span the four resource kinds, a resource created untagged / with one / with
/// three tags, removing zero / one / two / every key, a key the resource never carried, a
/// mix of present and absent keys, the same key named twice in one request, and a key
/// whose value is the empty string (the modelled `TagValue` minimum, `len 0..256`).
#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_it_names() {
    let three = &[
        ("env", "prod"),
        ("team", "integration"),
        ("cost-center", "4711"),
    ];
    let cases: &[UntagCase<'_>] = &[
        (
            "profile, one of three removed",
            Kind::Profile,
            Some(three),
            &["team"],
            &[("env", "prod"), ("cost-center", "4711")],
        ),
        (
            "profile, every key removed",
            Kind::Profile,
            Some(three),
            &["env", "team", "cost-center"],
            &[],
        ),
        (
            "profile, no keys named is a no-op",
            Kind::Profile,
            Some(three),
            &[],
            three,
        ),
        (
            "capability, a key the resource never carried",
            Kind::Capability,
            Some(&[("env", "prod")]),
            &["owner"],
            &[("env", "prod")],
        ),
        (
            "capability, present and absent keys together",
            Kind::Capability,
            Some(three),
            &["env", "owner"],
            &[("team", "integration"), ("cost-center", "4711")],
        ),
        (
            "partnership, the same key twice in one request",
            Kind::Partnership,
            Some(three),
            &["env", "env"],
            &[("team", "integration"), ("cost-center", "4711")],
        ),
        (
            "partnership, a key whose value is the empty string",
            Kind::Partnership,
            Some(&[("retain-until", ""), ("env", "prod")]),
            &["retain-until"],
            &[("env", "prod")],
        ),
        (
            "transformer, removing from an untagged resource",
            Kind::Transformer,
            None,
            &["env"],
            &[],
        ),
        (
            "transformer, two of three removed",
            Kind::Transformer,
            Some(three),
            &["env", "cost-center"],
            &[("team", "integration")],
        ),
    ];

    let (router, profile_id, capability_id) = b2bi_fixtures().await;

    for (case, kind, created, removed, expected) in cases {
        let arn = given_tagged_resource(
            &router,
            *kind,
            case,
            *created,
            &profile_id,
            &capability_id,
        )
        .await;

        let (status, body) = untag_resource(&router, &arn, removed).await;
        assert_empty_success(case, status, &body);

        let expected = expected_pairs(expected);
        assert_eq!(
            stored_tags(&router, case, &arn).await,
            expected,
            "case {case:?}: after UntagResource({removed:?}) the stored tags must be {expected:?}"
        );

        // `@idempotent`: the same request again leaves the same tag set.
        let (status, body) = untag_resource(&router, &arn, removed).await;
        assert_empty_success(&format!("{case} (replayed)"), status, &body);
        assert_eq!(
            stored_tags(&router, case, &arn).await,
            expected,
            "case {case:?}: replaying UntagResource must leave the tags at {expected:?}"
        );
    }
}

/// RULE (round trip): a removed key is gone from the stored tag set, and `TagResource` can
/// put it back with a new value — so remove and attach act on the one stored list.
///
/// The rows walk the tag set of one resource of each kind through tag -> untag -> tag.
#[tokio::test]
async fn a_removed_key_can_be_attached_again() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;

    for kind in [
        Kind::Profile,
        Kind::Capability,
        Kind::Partnership,
        Kind::Transformer,
    ] {
        let case = format!("round trip on {kind:?}");
        let arn = given_tagged_resource(
            &router,
            kind,
            &format!("round-trip-{kind:?}"),
            Some(&[("team", "integration")]),
            &profile_id,
            &capability_id,
        )
        .await;

        let (status, body) = tag_resource(&router, &arn, &[("env", "prod")]).await;
        assert_empty_success(&case, status, &body);
        assert_eq!(
            stored_tags(&router, &case, &arn).await,
            expected_pairs(&[("env", "prod"), ("team", "integration")]),
            "case {case:?}: the attached key must be stored"
        );

        let (status, body) = untag_resource(&router, &arn, &["env"]).await;
        assert_empty_success(&case, status, &body);
        assert_eq!(
            stored_tags(&router, &case, &arn).await,
            expected_pairs(&[("team", "integration")]),
            "case {case:?}: the removed key must be gone and the other kept"
        );

        let (status, body) = tag_resource(&router, &arn, &[("env", "staging")]).await;
        assert_empty_success(&case, status, &body);
        assert_eq!(
            stored_tags(&router, &case, &arn).await,
            expected_pairs(&[("env", "staging"), ("team", "integration")]),
            "case {case:?}: the key must be attachable again, with its new value"
        );
    }
}

/// RULE: a removal reaches only the resource its ARN names — the other resources this
/// service holds keep the tags they had.
#[tokio::test]
async fn untag_resource_removes_only_from_the_resource_its_arn_names() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let kinds = [
        Kind::Profile,
        Kind::Capability,
        Kind::Partnership,
        Kind::Transformer,
    ];

    let mut resources: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for kind in kinds {
        let arn = given_tagged_resource(
            &router,
            kind,
            &format!("untag-isolation-{kind:?}"),
            Some(&[("env", "shared"), ("team", "integration")]),
            &profile_id,
            &capability_id,
        )
        .await;
        resources.push((
            arn,
            expected_pairs(&[("env", "shared"), ("team", "integration")]),
        ));
    }

    for index in 0..resources.len() {
        let case = format!("removing from {:?}", kinds[index]);
        let (status, body) = untag_resource(&router, &resources[index].0, &["env"]).await;
        assert_empty_success(&case, status, &body);
        resources[index].1 = expected_pairs(&[("team", "integration")]);

        for (arn, expected) in &resources {
            assert_eq!(
                &stored_tags(&router, &case, arn).await,
                expected,
                "case {case:?}: the tags of {arn} must be {expected:?}"
            );
        }
    }
}

/// RULE: a `ResourceARN` this service cannot act on is refused with the modelled
/// `ValidationException` (400) — the same answer, and the same two wordings, the other
/// tagging operations give: the rejected value for something that is not an ARN, and the
/// one fixed sentence for an ARN that parses and addresses nothing — and no refusal removes
/// a tag.
///
/// The rows span malformed candidates (no colons, truncated at each field count, an empty
/// partition / service / resource field, the prefix mis-cased) and well-formed but
/// unresolvable ones (an unknown id of each kind, another service's ARN, a rewritten
/// region, a rewritten account, a deleted profile's ARN).
#[tokio::test]
async fn untag_resource_refuses_an_arn_it_cannot_resolve() {
    let (router, profile_id, capability_id) = b2bi_fixtures().await;
    let live = given_tagged_resource(
        &router,
        Kind::Profile,
        "untag refusal guard",
        Some(&[("env", "prod"), ("team", "integration")]),
        &profile_id,
        &capability_id,
    )
    .await;

    // A profile created and then deleted, so its ARN is well-formed but unresolvable.
    let (status, created) = call(
        &router,
        "CreateProfile",
        json!({
            "name": "doomed untag profile",
            "phone": "555-555-0104",
            "businessName": "Doomed Untag Corp",
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

    // (case, the candidate ARN, whether it satisfies the ARN grammar at all)
    let cases: &[(&str, String, bool)] = &[
        (
            "no colons at all",
            "this-is-not-an-arn-at-all".to_string(),
            false,
        ),
        ("prefix only", "arn".to_string(), false),
        ("partition only", "arn:aws".to_string(), false),
        (
            "no resource field",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}"),
            false,
        ),
        (
            "empty resource field",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:"),
            false,
        ),
        (
            "empty service field",
            format!("arn:aws::{SIGNING_REGION}:{ACCOUNT}:profile/p-0123456789abcdef0"),
            false,
        ),
        (
            "prefix upper-cased",
            live.replacen("arn:", "ARN:", 1),
            false,
        ),
        (
            "unknown profile id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:profile/p-0123456789abcdef0"),
            true,
        ),
        (
            "unknown capability id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:capability/ca-0123456789abcdef0"),
            true,
        ),
        (
            "unknown transformer id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:transformer/tr-0123456789abcdef0"),
            true,
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{SIGNING_REGION}:{ACCOUNT}:bucket/not-a-b2bi-resource"),
            true,
        ),
        (
            "a stored ARN with the region rewritten",
            live.replacen(SIGNING_REGION, "eu-central-1", 1),
            true,
        ),
        (
            "a stored ARN with the account rewritten",
            live.replacen(ACCOUNT, "123456789012", 1),
            true,
        ),
        ("a deleted profile's ARN", doomed_arn.clone(), true),
    ];

    for (case, candidate, parses) in cases {
        let (status, body) = untag_resource(&router, candidate, &["env"]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UntagResource({candidate}) must be 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = if *parses {
            INVALID_INPUT_ARN.to_string()
        } else {
            invalid_arn(candidate)
        };
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    assert_eq!(
        stored_tags(&router, "live profile after every refusal", &live).await,
        expected_pairs(&[("env", "prod"), ("team", "integration")]),
        "no refused call may have removed a tag"
    );
}
