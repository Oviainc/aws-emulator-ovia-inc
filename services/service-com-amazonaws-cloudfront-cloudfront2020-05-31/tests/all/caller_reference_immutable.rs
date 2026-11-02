// SPDX-License-Identifier: CC0-1.0

//! The refusal an update that would change a `CallerReference` carries, end to end over
//! HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a `CallerReference` "uniquely identifies a request to create a
//! resource" and names that resource for its whole life, so no update moves one onto
//! another value — whichever resource is being updated. The refusal is `IllegalUpdate`
//! (HTTP 400) carrying the FIXED sentence the surface being updated states this
//! modification with — one sentence per surface, each stating the rule and embedding NOTHING
//! from the request or from the stored record; a client or a fixture matching on that
//! sentence sees the same string whatever the two references are. A refused update writes
//! nothing, so the stored reference is still the one the create minted the resource under.
//!
//! The rows span both operations of this service that refuse the modification
//! (`UpdateCloudFrontOriginAccessIdentity` and `UpdateDistribution`) and the forms a
//! submitted reference can differ in: another value entirely, the stored value in upper
//! case, the stored value with text appended, the stored value with text prepended, and a
//! value far longer than the stored one. Every row also asserts that neither reference
//! appears anywhere in the message — which is what tells a fixed sentence from an
//! interpolated one.
//!
//! The wire harness, the distribution configuration builder and the identity request
//! helpers are the lifecycle suites', reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE, ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE,
};

use crate::distribution_lifecycle::{
    config, create_distribution, emulator, get_distribution, update_distribution, Answer, Config,
};
use crate::origin_access_identity_lifecycle::{
    create_identity, get_identity_config, Config as IdentityConfig,
};
use crate::origin_access_identity_update::update_identity;

/// The stored `CallerReference` every row below starts from.
const STORED: &str = "caller-reference-under-test";

/// How a submitted reference differs from the stored one: `(case, the submitted value)`.
///
/// The rows span the forms a client's mutation takes — a different value, a case change, a
/// suffix, a prefix and a much longer value — so the rule is asserted over the class rather
/// than over one string.
fn mutations() -> Vec<(&'static str, String)> {
    vec![
        ("another value entirely", "a-different-reference".to_string()),
        ("the stored value in upper case", STORED.to_uppercase()),
        ("the stored value with a suffix", format!("{STORED}-2")),
        ("the stored value with a prefix", format!("new-{STORED}")),
        ("a much longer value", "r".repeat(128)),
    ]
}

/// Assert `answer` is the one refusal this rule states, in the sentence `expected` — the
/// wording the surface under test refuses the modification with — and that neither reference
/// leaked into its message.
fn assert_refusal(answer: &Answer, case: &str, submitted: &str, expected: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400 from the update, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "IllegalUpdate",
        "case {case}: expected IllegalUpdate, got {:?}",
        answer.error_code()
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case}: expected the surface's fixed caller-reference sentence {expected:?}, got \
         {message:?}"
    );
    assert!(
        !message.contains(STORED) && !message.contains(submitted),
        "case {case}: expected a sentence carrying no reference value, got {message:?}"
    );
}

/// `UpdateCloudFrontOriginAccessIdentity` refuses every form of the modification with the
/// one sentence, and the stored configuration keeps the reference the create minted.
#[tokio::test]
async fn an_identity_update_refuses_a_changed_caller_reference_with_one_sentence() {
    let router = emulator();

    let stored = IdentityConfig {
        case: "the identity under update",
        caller_reference: STORED.to_string(),
        comment: "the stored configuration".to_string(),
    };
    let created = create_identity(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the create: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    for (case, submitted) in mutations() {
        let moved = IdentityConfig {
            case: "a moved caller reference",
            caller_reference: submitted.clone(),
            comment: "within the documented bound".to_string(),
        };
        let answer = update_identity(&router, &id, &moved.xml(), Some(&etag)).await;
        assert_refusal(
            &answer,
            case,
            &submitted,
            ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE,
        );

        // Nothing was written: the stored configuration still carries the reference the
        // create minted the identity under, at the version the refused update matched on.
        let stored_read = get_identity_config(&router, &id).await;
        assert_eq!(
            stored_read.status, 200,
            "case {case}: expected HTTP 200 reading the configuration back, got {} with {}",
            stored_read.status, stored_read.raw
        );
        assert_eq!(
            stored_read.root().text_at("CallerReference"),
            Some(STORED),
            "case {case}: expected the stored reference to be untouched, got {}",
            stored_read.raw
        );
        assert_eq!(
            stored_read.header("etag"),
            etag,
            "case {case}: expected the version token to be untouched, got {:?}",
            stored_read.header("etag")
        );
    }
}

/// `UpdateDistribution` refuses the SAME modification under the SAME code: one rule across
/// the resources whose creates mint a caller reference, in the sentence the DISTRIBUTION
/// surface words it with.
#[tokio::test]
async fn a_distribution_update_refuses_a_changed_caller_reference_with_one_sentence() {
    let router = emulator();

    let stored = config("the distribution under update", STORED);
    let created = create_distribution(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the create: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    for (case, submitted) in mutations() {
        let moved = Config {
            caller_reference: submitted.clone(),
            ..stored.clone()
        };
        let answer = update_distribution(&router, &id, &moved.xml(), Some(&etag)).await;
        assert_refusal(
            &answer,
            case,
            &submitted,
            ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE,
        );

        // Nothing was written: the stored configuration still carries the reference the
        // create minted the distribution under.
        let read = get_distribution(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 reading the distribution back, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("DistributionConfig/CallerReference"),
            Some(STORED),
            "case {case}: expected the stored reference to be untouched, got {}",
            read.raw
        );
    }
}
