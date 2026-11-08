// SPDX-License-Identifier: CC0-1.0

//! `UpdateResponseHeadersPolicy`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's update states ("When you
//! update a response headers policy, the entire policy is replaced. You cannot update some
//! policy fields independent of others"):
//!
//! * the update REPLACES the stored configuration of the ONE record every read of that policy
//!   answers from, down to the members the request left out, and both readers then report the
//!   submitted configuration;
//! * the update MINTS a fresh version token and hands it back, so the token the caller
//!   matched on no longer satisfies an `If-Match` against the policy (`PreconditionFailed`)
//!   while the new one does — which is the token the delete of the same record is then gated
//!   on;
//! * the update is gated like every other mutation of this record: an absent or unusable
//!   `If-Match` is `InvalidIfMatchVersion`, an id this account holds no policy for is
//!   `NoSuchResponseHeadersPolicy`, a `Name` another policy of the account holds is
//!   `ResponseHeadersPolicyAlreadyExists`, and a policy AWS published is `IllegalUpdate` —
//!   each leaving the stored record exactly as it was.
//!
//! The rows span what those rules generalise over: every configuration
//! [`crate::response_headers_policy_lifecycle::policy_table`] builds, replaced by the NEXT
//! row's configuration, so each update both drops members the stored policy carried and adds
//! members it did not; every form an `If-Match` takes; and every entry of the published
//! catalogue for the `IllegalUpdate` rule.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s and the policy writer, readers and
//! configuration assertions are the response headers policy suites' — reused rather than
//! copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_RESPONSE_HEADERS_POLICIES, MAX_CSP_IN_RESPONSE_HEADERS_POLICY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::response_headers_policy_config_read::get_policy_config;
use crate::response_headers_policy_lifecycle::{
    assert_config, create_policy, get_policy, policy_table, Policy, Security,
};

/// The sentences CloudFront sends for these refusals, spelled out here rather than taken from
/// the crate, so a reworded refusal fails this suite.
const NO_SUCH_POLICY_MESSAGE: &str = "The specified response headers policy does not exist.";
const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions for this operation.";
/// The sentence a configuration whose `Content-Security-Policy` value is past the published
/// quota is refused with.
const TOO_LONG_CSP_MESSAGE: &str =
    "Your request contains longer Content Security Policy value in the Response Headers \
     Policy than is allowed.";

/// `PUT /2020-05-31/response-headers-policy/{Id}` with the `ResponseHeadersPolicyConfig`
/// document `body` under the version token `if_match` — the route, the payload member and the
/// header the model binds this operation to.
async fn update_policy(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/response-headers-policy/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// Create `row` and report its minted id and current version token.
async fn create(router: &MultiServiceRouter, row: &Policy) -> (String, String) {
    let answer = create_policy(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateResponseHeadersPolicy, got {} with {}",
        row.case, answer.status, answer.raw
    );
    (
        answer.root().text_at("Id").unwrap_or_default().to_string(),
        answer.header("etag").to_string(),
    )
}

/// Assert both readers report `expected` for the policy `id`, at the version token `etag`.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    expected: &Policy,
    etag: &str,
    context: &str,
) {
    let whole = get_policy(router, id).await;
    assert_eq!(
        whole.status, 200,
        "case {:?} ({context}): expected HTTP 200 from GetResponseHeadersPolicy, got {} with {}",
        expected.case, whole.status, whole.raw
    );
    let config = whole
        .root()
        .at("ResponseHeadersPolicyConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {:?} ({context}): no configuration in {}",
                expected.case, whole.raw
            )
        });
    assert_config(config, expected, context, &whole.raw);
    assert_eq!(
        whole.header("etag"),
        etag,
        "case {:?} ({context}): expected the version token {etag:?} from the read, got {:?}",
        expected.case,
        whole.header("etag")
    );

    let only_config = get_policy_config(router, id).await;
    assert_eq!(
        only_config.status, 200,
        "case {:?} ({context}): expected HTTP 200 from GetResponseHeadersPolicyConfig, got {} \
         with {}",
        expected.case, only_config.status, only_config.raw
    );
    assert_config(only_config.root(), expected, context, &only_config.raw);
    assert_eq!(
        only_config.header("etag"),
        etag,
        "case {:?} ({context}): expected one version token across both readers, got {:?} and \
         {etag:?}",
        expected.case,
        only_config.header("etag")
    );
}

/// Every configuration the create accepts is replaced WHOLE by the update under the policy's
/// current version token: both readers then report the submitted configuration, the token the
/// update minted is fresh and is the one the next mutation must match, and the token the
/// caller came in with no longer satisfies an `If-Match`.
#[tokio::test]
async fn an_update_replaces_the_whole_stored_configuration() {
    let rows = policy_table();
    let router = emulator();

    for (index, row) in rows.iter().enumerate() {
        let (id, created_etag) = create(&router, row).await;
        assert_stored(&router, &id, row, &created_etag, "after the create").await;

        // The replacement is the NEXT row's configuration under this policy's own name, so
        // the update both drops members the stored policy carried and adds members it did
        // not, while the name stays this policy's.
        let replacement = Policy {
            name: row.name.clone(),
            ..rows[(index + 1) % rows.len()].clone()
        };
        let answer = update_policy(&router, &id, &replacement.xml(), Some(&created_etag)).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200 from UpdateResponseHeadersPolicy, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let updated_etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&updated_etag),
            "case {:?}: expected a minted version token from the update, got {updated_etag:?}",
            row.case
        );
        assert_ne!(
            updated_etag, created_etag,
            "case {:?}: expected a fresh version token, got the create's {created_etag:?}",
            row.case
        );
        let config = answer
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no configuration in the update's own answer: {}",
                    row.case, answer.raw
                )
            });
        assert_config(config, &replacement, "the update's own answer", &answer.raw);

        assert_stored(&router, &id, &replacement, &updated_etag, "after the update").await;

        // The version the caller came in with is spent: the same update again is refused and
        // changes nothing.
        let stale = update_policy(&router, &id, &row.xml(), Some(&created_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {:?}: expected HTTP 412 for the spent version token, got {} with {}",
            row.case, stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {:?}: expected PreconditionFailed for the spent token, got {}",
            row.case,
            stale.raw
        );
        assert_eq!(
            stale.root().find_text("Message").unwrap_or_default(),
            PRECONDITION_FAILED_MESSAGE,
            "case {:?}: expected the modelled precondition sentence, got {}",
            row.case,
            stale.raw
        );
        assert_stored(
            &router,
            &id,
            &replacement,
            &updated_etag,
            "after the refused update",
        )
        .await;
    }
}

/// The refusals, each asserted on one stored policy that is read back afterwards: the record
/// is exactly as it was.
#[tokio::test]
async fn the_update_is_refused_without_changing_the_record() {
    let router = emulator();
    let subject = Policy {
        case: "the policy under the update's refusals",
        name: "response-headers-policy-update-subject".to_string(),
        comment: Some("the stored configuration".to_string()),
        ..Policy::default()
    };
    let other = Policy {
        case: "another policy of the account, whose name is taken",
        name: "response-headers-policy-update-other".to_string(),
        ..Policy::default()
    };
    let (id, etag) = create(&router, &subject).await;
    let (_, other_etag) = create(&router, &other).await;

    // An `If-Match` the operation cannot read, and one that is a token of another policy.
    let versions: &[(&str, Option<&str>, u16, &str, &str)] = &[
        (
            "no If-Match at all",
            None,
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match that is no version token",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an empty If-Match",
            Some(""),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "a version token that is another policy's",
            Some(other_etag.as_str()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
    ];
    let replacement = Policy {
        case: "the configuration the refused updates submitted",
        name: subject.name.clone(),
        comment: Some("never stored".to_string()),
        ..Policy::default()
    };
    for (case, if_match, status, code, message) in versions {
        let answer = update_policy(&router, &id, &replacement.xml(), *if_match).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            *message,
            "case {case:?}: expected the modelled sentence {message:?}, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &subject, &etag, case).await;
    }

    // A name another policy of the account holds cannot be moved onto this one.
    let renamed = Policy {
        case: "a name another policy already holds",
        name: other.name.clone(),
        ..Policy::default()
    };
    let answer = update_policy(&router, &id, &renamed.xml(), Some(&etag)).await;
    assert_eq!(
        answer.status, 409,
        "case a name another policy already holds: expected HTTP 409, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "ResponseHeadersPolicyAlreadyExists",
        "case a name another policy already holds: expected \
         ResponseHeadersPolicyAlreadyExists, got {}",
        answer.raw
    );
    assert_stored(
        &router,
        &id,
        &subject,
        &etag,
        "after the repeated-name refusal",
    )
    .await;

    // The policy may of course keep its OWN name, which is the same comparison from the other
    // side.
    let kept = Policy {
        case: "the policy keeps its own name",
        name: subject.name.clone(),
        comment: Some("kept its own name".to_string()),
        ..Policy::default()
    };
    let accepted = update_policy(&router, &id, &kept.xml(), Some(&etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case the policy keeps its own name: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert_stored(
        &router,
        &id,
        &kept,
        accepted.header("etag"),
        "after keeping its own name",
    )
    .await;

    // An id this account holds no policy for, at the same version token.
    for (case, candidate, message) in [
        (
            "an id never minted",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
            NO_SUCH_POLICY_MESSAGE,
        ),
        (
            "a token that is no policy id",
            "not-a-policy-id".to_string(),
            "Id format is invalid",
        ),
    ] {
        let answer = update_policy(&router, &candidate, &kept.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {case:?}: expected NoSuchResponseHeadersPolicy, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            message,
            "case {case:?}: expected the modelled sentence {message:?}, got {}",
            answer.raw
        );
    }
}

/// CloudFront's published quota on the length of the `Content-Security-Policy` header value a
/// policy sets: the bound itself is admissible and one character past it is refused with
/// `TooLongCSPInResponseHeadersPolicy` (HTTP 400), on the CREATE and on the UPDATE alike —
/// and a refused update leaves the stored policy at the configuration and the version token
/// it had, so the delete that follows is still gated on that token.
///
/// The rows span the bound and its neighbourhood (just under, exactly on, one past, and far
/// past it) and the character classes a value can carry, since the quota counts CHARACTERS:
/// one row is multi-byte text whose byte length is past the bound while its character count
/// is not.
#[tokio::test]
async fn a_content_security_policy_past_the_published_quota_is_refused() {
    let router = emulator();

    // `MAX_CSP_IN_RESPONSE_HEADERS_POLICY` is CloudFront's published quota for the maximum
    // number of characters in the Content-Security-Policy header value.
    let bound = MAX_CSP_IN_RESPONSE_HEADERS_POLICY;
    let cases: &[(&str, String, bool)] = &[
        ("one character under the bound", "a".repeat(bound - 1), true),
        ("exactly on the bound", "a".repeat(bound), true),
        ("one character past the bound", "a".repeat(bound + 1), false),
        ("far past the bound", "a".repeat(bound * 2), false),
        (
            "multi-byte characters inside the bound, whose bytes are past it",
            "\u{3042}".repeat(bound),
            true,
        ),
        (
            "multi-byte characters past the bound",
            "\u{3042}".repeat(bound + 1),
            false,
        ),
    ];

    // The policy every update row is aimed at, created with no security headers section at
    // all, so a refused update is visible as the section staying absent.
    let subject = Policy {
        case: "the policy under the content security policy quota",
        name: "response-headers-policy-csp-quota".to_string(),
        ..Policy::default()
    };
    let (id, etag) = create(&router, &subject).await;
    // Each admissible row's update mints a token and the restore mints another, so the token
    // the next row must match on is tracked here.
    let mut current = etag;

    for (index, (case, value, admissible)) in cases.iter().enumerate() {
        let attempt = Policy {
            case: "a configuration stating a content security policy",
            name: subject.name.clone(),
            security: Some(Security {
                csp: Some((true, value.clone())),
                ..Security::default()
            }),
            ..Policy::default()
        };

        // The CREATE path, under a name of its own so the attempt is refused for its value
        // rather than for a repeated name.
        let created = create_policy(
            &router,
            &Policy {
                name: format!("response-headers-policy-csp-{index}"),
                ..attempt.clone()
            }
            .xml(),
        )
        .await;
        if *admissible {
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected HTTP 201 from CreateResponseHeadersPolicy within the \
                 quota, got {} with {}",
                created.status, created.raw
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected HTTP 400 from CreateResponseHeadersPolicy past the \
                 quota, got {} with {}",
                created.status, created.raw
            );
            assert_eq!(
                created.error_code(),
                "TooLongCSPInResponseHeadersPolicy",
                "case {case:?}: expected TooLongCSPInResponseHeadersPolicy from the create, got \
                 {}",
                created.raw
            );
            assert_eq!(
                created.root().find_text("Message").unwrap_or_default(),
                TOO_LONG_CSP_MESSAGE,
                "case {case:?}: expected the modelled over-quota sentence from the create, got \
                 {}",
                created.raw
            );
        }

        // The UPDATE path, against the stored policy, under its own name.
        let updated = update_policy(&router, &id, &attempt.xml(), Some(&current)).await;
        if *admissible {
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateResponseHeadersPolicy within the \
                 quota, got {} with {}",
                updated.status, updated.raw
            );
            // Put the subject back at the version token the rest of the rows match on, so
            // each row is judged on its own value.
            let restored = update_policy(
                &router,
                &id,
                &subject.xml(),
                Some(updated.header("etag")),
            )
            .await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: expected HTTP 200 restoring the subject, got {} with {}",
                restored.status, restored.raw
            );
            assert_stored(
                &router,
                &id,
                &subject,
                restored.header("etag"),
                "after restoring the subject",
            )
            .await;
            current = restored.header("etag").to_string();
        } else {
            assert_eq!(
                updated.status, 400,
                "case {case:?}: expected HTTP 400 from UpdateResponseHeadersPolicy past the \
                 quota, got {} with {}",
                updated.status, updated.raw
            );
            assert_eq!(
                updated.error_code(),
                "TooLongCSPInResponseHeadersPolicy",
                "case {case:?}: expected TooLongCSPInResponseHeadersPolicy from the update, got \
                 {}",
                updated.raw
            );
            assert_eq!(
                updated.root().find_text("Message").unwrap_or_default(),
                TOO_LONG_CSP_MESSAGE,
                "case {case:?}: expected the modelled over-quota sentence from the update, got \
                 {}",
                updated.raw
            );
            // The refused update stored nothing and spent no version token.
            assert_stored(&router, &id, &subject, &current, case).await;
        }
    }
}

/// A policy AWS published is not this account's to change: the update is `IllegalUpdate`
/// whatever version the request states, and the policy keeps serving the configuration AWS
/// publishes. Asserted over EVERY entry of the published catalogue.
#[tokio::test]
async fn a_published_policy_is_refused_the_update() {
    let router = emulator();

    for managed in MANAGED_RESPONSE_HEADERS_POLICIES {
        let case = managed.name;
        let read = get_policy(&router, managed.id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the published policy to be readable, got {} with {}",
            read.status, read.raw
        );
        let etag = read.header("etag").to_string();
        let body = Policy {
            case: "the configuration aimed at a published policy",
            name: format!("rewritten-{}", managed.id),
            ..Policy::default()
        }
        .xml();

        for (described, if_match) in [
            ("no If-Match at all", None),
            ("the policy's own current version", Some(etag.as_str())),
        ] {
            let refused = update_policy(&router, managed.id, &body, if_match).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} with {described}: expected HTTP 400, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalUpdate",
                "case {case:?} with {described}: expected IllegalUpdate rather than a version \
                 or existence refusal, got {}",
                refused.raw
            );
        }

        // The refusals left the published policy exactly where it was, at the same version.
        let after = get_policy(&router, managed.id).await;
        assert_eq!(
            (after.status, after.header("etag")),
            (200, etag.as_str()),
            "case {case:?}: expected the refused updates to leave the policy unchanged, got {}",
            after.raw
        );
        assert_eq!(
            after.root().text_at("ResponseHeadersPolicyConfig/Name"),
            Some(managed.name),
            "case {case:?}: expected the published policy to keep its own name, got {}",
            after.raw
        );
    }
}
