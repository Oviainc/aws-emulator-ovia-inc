// SPDX-License-Identifier: CC0-1.0

//! The Region field of every ARN this service mints, end to end over the wire.
//!
//! Every case drives the real request path through the shared harness
//! (`tests/all/harness.rs`): the router picks the service from the SigV4 credential
//! scope, the framework deserializes and VALIDATES the request — an `@httpLabel` carrying
//! a `@pattern` is checked before any handler runs — the handler runs, and the response is
//! serialized as `restJson1`. So what these cases assert is what a client sees, and the
//! boundary itself is the oracle for the modelled `resourceArn` pattern: a request that
//! presents an ARN the pattern does not admit is refused with HTTP 400 before the handler
//! is reached, never answered.
//!
//! The rule under test: an ARN this service mints is a handle its own ARN-addressed
//! operations accept, so its Region field always holds a value the modelled `resourceArn`
//! pattern admits there (`smithy_query(query="operation",operation="TagResource")` gives
//! `resourceArn` the pattern
//! `^arn:aws[a-z\-]*:elemental-inference[a-z\-]*:[a-z0-9\-]+:[0-9]{12}:(feed|dictionary)/[a-zA-Z0-9]{1,19}$`,
//! whose Region field is `[a-z0-9\-]+`). The Region of a request is read out of its own
//! SigV4 credential scope, which is free text in the `Authorization` header, so a scope
//! field that is not shaped like a Region code names no Region: such a request is
//! attributed to the service's default Region, exactly as a request carrying no credential
//! scope is.
//!
//! What the rule generalises over is the TEXT the credential scope carries, so the table
//! below spans Region codes and non-Region text alike — letter case, characters outside
//! the Region class (including the `:` that would split an ARN's own fields), and length
//! at the bound this service accepts and one past it — crossed with both resources this
//! service mints an ARN for, and asserted through every operation that takes a
//! `resourceArn`.

use crate::harness::{
    create_tagged_resource_in, expected_tags, list_tags_in, pair, read_resource_in, router,
    tag_resource_in, tags_of, untag_resource_in, TAGGABLE_KINDS,
};
use service_com_amazonaws_elementalinference_elementalinference::state::DEFAULT_REGION;
// The longest Region label an ARN's Region field carries: the bound of the shared grammar
// this service reads a credential scope through (`service_common::arn`, the one place this
// tree states it — every AWS model's ARN pattern bounds that field at `[a-z0-9-]{1,20}`),
// so the rows below step along the real bound rather than a copy of the figure.
use service_common::arn::MAX_REGION_LENGTH as MAX_REGION_LABEL_LENGTH;

/// One row of the credential-scope table: the text the request's scope carries, and the
/// Region the service must attribute that request to.
struct ScopeCase {
    label: &'static str,
    /// The scope's Region field, as the caller wrote it. Never contains `/` or `,`,
    /// which terminate the scope itself rather than reaching its Region field.
    signed_for: String,
    /// The Region the resulting resource — and the Region field of its ARN — belongs to.
    attributed_to: String,
}

/// The rows: Region codes the service takes as given, and scope text that names no
/// Region and so falls back to the default.
fn scope_cases() -> Vec<ScopeCase> {
    let region_code = |label: &'static str, region: &str| ScopeCase {
        label,
        signed_for: region.to_string(),
        attributed_to: region.to_string(),
    };
    let not_a_region = |label: &'static str, text: &str| ScopeCase {
        label,
        signed_for: text.to_string(),
        attributed_to: DEFAULT_REGION.to_string(),
    };
    vec![
        region_code("a Region code", "us-west-2"),
        // A Region code from another geography, and one with a two-digit-free suffix
        // this crate's other suites do not sign for.
        region_code("a Region code of another geography", "ap-southeast-4"),
        region_code("a Region code with a three-part name", "us-gov-east-1"),
        region_code(
            "Region text at the longest label the grammar admits",
            &"a".repeat(MAX_REGION_LABEL_LENGTH),
        ),
        not_a_region(
            "Region text one character past the longest label the grammar admits",
            &"a".repeat(MAX_REGION_LABEL_LENGTH + 1),
        ),
        not_a_region("Region text far past every bound", &"a".repeat(400)),
        not_a_region("an upper-case Region code", "US-EAST-1"),
        not_a_region("a Region code spelled with underscores", "us_east_1"),
        not_a_region("a capitalised word that is no Region", "Local"),
        not_a_region(
            "text carrying the colon that separates an ARN's own fields",
            "us:west:2",
        ),
        not_a_region("text carrying a space", "us west 2"),
        not_a_region("text carrying a wildcard", "us-west-*"),
    ]
}

/// A created resource's ARN carries the Region its request was attributed to, whatever
/// the credential scope wrote there.
#[tokio::test]
async fn a_minted_arn_carries_the_region_the_request_was_attributed_to() {
    let router = router();

    for (index, case) in scope_cases().into_iter().enumerate() {
        for kind in TAGGABLE_KINDS {
            let label = format!("{} ({kind})", case.label);
            let (id, arn) = create_tagged_resource_in(
                &router,
                &case.signed_for,
                kind,
                &format!("arnregion{index}{kind}"),
                &[pair("env", "dev")],
            )
            .await;

            let expected = format!(
                "arn:aws:elemental-inference:{}:{}:{kind}/{id}",
                case.attributed_to,
                engine::EMULATOR_ACCOUNT_ID
            );
            assert_eq!(
                arn, expected,
                "case {label:?}: the minted ARN names the Region the request was attributed to, \
                 expected {expected}, got {arn:?}"
            );

            // The resource is the one that Region's endpoint holds: the id-based reader
            // of the attributed Region reports it.
            let (status, stored) = read_resource_in(&router, &case.attributed_to, kind, &id).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the attributed Region's endpoint reads the resource back by its \
                 id, got {status} with {stored}"
            );
        }
    }
}

/// Every ARN a create hands out is a `resourceArn` the tagging operations accept: the
/// boundary admits it, and all three of them resolve the resource it names.
#[tokio::test]
async fn a_minted_arn_is_a_handle_the_tagging_operations_take() {
    let router = router();

    for (index, case) in scope_cases().into_iter().enumerate() {
        for kind in TAGGABLE_KINDS {
            let label = format!("{} ({kind})", case.label);
            let created = vec![pair("env", "dev")];
            let (id, arn) = create_tagged_resource_in(
                &router,
                &case.signed_for,
                kind,
                &format!("tagarn{index}{kind}"),
                &created,
            )
            .await;
            // Signed exactly as the create was: a client holds one credential scope and
            // the ARN the service handed it back.
            let signed_for = case.signed_for.as_str();

            let (status, listed) = list_tags_in(&router, signed_for, &arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: ListTagsForResource takes the ARN this service minted \
                 ({arn}), got {status} with {listed}"
            );
            assert_eq!(
                tags_of(&listed),
                expected_tags(&created),
                "case {label:?}: the listing reports the tag set the resource was created \
                 with, got {listed}"
            );

            let added = pair("added", "yes");
            let (status, body) =
                tag_resource_in(&router, signed_for, &arn, std::slice::from_ref(&added)).await;
            assert_eq!(
                status, 204,
                "case {label:?}: TagResource takes the ARN this service minted ({arn}), got \
                 {status} with {body}"
            );
            let (_, listed) = list_tags_in(&router, signed_for, &arn).await;
            assert_eq!(
                tags_of(&listed),
                expected_tags(&[created[0].clone(), added.clone()]),
                "case {label:?}: the write landed beside the tag the resource was created with, \
                 got {listed}"
            );

            let (status, body) =
                untag_resource_in(&router, signed_for, &arn, std::slice::from_ref(&added.0)).await;
            assert_eq!(
                status, 204,
                "case {label:?}: UntagResource takes the ARN this service minted ({arn}), got \
                 {status} with {body}"
            );

            // The tag set the ARN-addressed path wrote is the one the resource's own
            // reader reports, so neither route has its own copy.
            let (status, stored) = read_resource_in(&router, &case.attributed_to, kind, &id).await;
            assert_eq!(
                (status, tags_of(&stored)),
                (200, expected_tags(&created)),
                "case {label:?}: the resource's own reader reports what the tagging path left, \
                 got {status} with {stored}"
            );
        }
    }
}
