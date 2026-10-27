// SPDX-License-Identifier: CC0-1.0

//! `DeleteOriginRequestPolicy`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states:
//!
//! * the delete REMOVES the stored record a read answered from, so the id afterwards names
//!   no policy (`NoSuchOriginRequestPolicy`, HTTP 404) and the name it held is free for a
//!   later create to claim under a new id;
//! * the delete is gated on the policy's own version token: an absent `If-Match` and one
//!   that is no version token at all are `InvalidIfMatchVersion` (HTTP 400), a well-formed
//!   token that is not this policy's current one is `PreconditionFailed` (HTTP 412), and in
//!   either case the policy stays exactly where it was;
//! * "You cannot delete an origin request policy if it's attached to any cache behaviors",
//!   so a policy any stored cache behaviour — the default one or any ordered one — still
//!   attaches is `OriginRequestPolicyInUse` (HTTP 409) and stays readable, and once every
//!   behaviour has let go the same delete lands;
//! * a policy Amazon Web Services published is read-only to the account that reads it, so a
//!   delete aimed at one is `IllegalDelete` (HTTP 400) whatever version the request carries,
//!   and the policy is still readable afterwards.
//!
//! The rows span what those rules generalise over: every configuration of
//! [`crate::origin_request_policy_lifecycle::policy_table`] (which of the three name lists
//! the policy carries at all, each list's length, every value of the three behaviour
//! catalogs, text that is empty, ASCII, non-ASCII or carries XML metacharacters), every form
//! an `If-Match` takes (absent, bare, quoted, weak, a token of another resource, a value
//! that is no token), which behaviour attaches the policy, and every entry of the published
//! catalogue — including entries no issue cited.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the policy create
//! and read are [`crate::origin_request_policy_lifecycle`]'s, and the distribution that
//! attaches a policy is built by
//! [`crate::distributions_by_origin_request_policy_id::config_xml`].

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE, MANAGED_ORIGIN_REQUEST_POLICIES,
    NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE, ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, is_version_token,
    update_distribution, Answer, API_VERSION,
};
use crate::distributions_by_origin_request_policy_id::{
    config_xml, listed_ids, list_by_policy, Row,
};
use crate::origin_request_policy_lifecycle::{create_policy, get_policy, policy_table, Policy};
use crate::response_headers_policy_lifecycle::is_policy_id;

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's
/// own `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

/// `DELETE /2020-05-31/origin-request-policy/{Id}` under the version token `if_match` — the
/// route and the header the model binds this operation to (`IfMatch` travels in `If-Match`).
async fn delete_policy(
    router: &MultiServiceRouter,
    policy_id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-request-policy/{policy_id}"),
        None,
        if_match,
    )
    .await
}

/// Create `policy`, returning its minted id and its current version token.
async fn create(router: &MultiServiceRouter, policy: &Policy) -> (String, String) {
    let answer = create_policy(router, &policy.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 from CreateOriginRequestPolicy, got {} with {}",
        policy.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_policy_id(&id),
        "case {}: expected a minted policy id, got {id:?}",
        policy.case
    );
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token, got {etag:?}",
        policy.case
    );
    (id, etag)
}

/// Assert that `id` names no origin request policy any more: the single-policy read answers
/// `NoSuchOriginRequestPolicy` with the modelled sentence, and so does a second delete.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    let fetched = get_policy(router, id).await;
    assert_eq!(
        fetched.status, 404,
        "case {case}: expected HTTP 404 from GetOriginRequestPolicy after the delete, got {} \
         with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.error_code(),
        "NoSuchOriginRequestPolicy",
        "case {case}: expected NoSuchOriginRequestPolicy after the delete, got {}",
        fetched.raw
    );
    let message = fetched.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
        "case {case}: expected the modelled missing-policy sentence, got {message:?}"
    );
}

/// Every configuration the create accepts is deleted under its version token: the delete
/// answers HTTP 204 with no body, the id afterwards names no policy, deleting it again is
/// `NoSuchOriginRequestPolicy`, and the name it held is free for a NEW policy under a new id.
#[tokio::test]
async fn every_stored_policy_is_removed_by_its_delete() {
    for policy in policy_table() {
        let router = emulator();
        let (id, etag) = create(&router, &policy).await;

        // Round trip: the policy is readable before the delete, so the 404 after it is the
        // delete's doing and not a policy that was never stored.
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetOriginRequestPolicy before the delete, got {} \
             with {}",
            policy.case, fetched.status, fetched.raw
        );

        let deleted = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {}: expected HTTP 204 from DeleteOriginRequestPolicy, got {} with {}",
            policy.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {}: expected no body from a 204, got {}",
            policy.case,
            deleted.raw
        );

        assert_gone(&router, &id, policy.case).await;
        let again = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {}: expected HTTP 404 from a second delete, got {} with {}",
            policy.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {}: expected NoSuchOriginRequestPolicy from a second delete, got {}",
            policy.case,
            again.raw
        );

        // The name the deleted policy held identifies nothing any more, so a create under it
        // mints a SECOND policy rather than conflicting with the one that is gone.
        let (reused_id, _) = create(&router, &policy).await;
        assert_ne!(
            reused_id, id,
            "case {}: expected a freshly minted id for the policy recreated under the freed \
             name, got the deleted one {id:?}",
            policy.case
        );
    }
}

/// The delete is gated on the policy's OWN version token.
///
/// The rows span every form an `If-Match` takes: absent, a value that is no version token at
/// all, a well-formed token belonging to another resource, and the current token bare,
/// quoted and weak — the three ways HTTP and CloudFront's own `ETag` header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();
    let subject = Policy {
        case: "the policy under the version gate",
        name: "version-gate".to_string(),
        comment: Some("gated on its own ETag".to_string()),
        ..Policy::default()
    };
    let other = Policy {
        case: "another policy, whose token is not the subject's",
        name: "version-gate-other".to_string(),
        ..Policy::default()
    };

    let (_, other_etag) = create(&router, &other).await;

    // Each refusal is asserted on its own freshly created policy, and the policy is read back
    // after it: a refused delete removes nothing.
    let refusals: &[(&str, Option<&str>, u16, &str)] = &[
        ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty If-Match",
            Some(""),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match carrying a lowercase token",
            Some("e1stale1version"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a version token that is another policy's",
            Some(other_etag.as_str()),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, code) in refusals {
        let (id, _) = create(&router, &subject).await;
        let answer = delete_policy(&router, &id, *if_match).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the refused delete to leave the policy readable, got {} with \
             {}",
            fetched.status, fetched.raw
        );

        // Clear the way for the next row, which creates a policy under the same name.
        let cleanup = delete_policy(&router, &id, Some(fetched.header("etag"))).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case}: expected the current token to delete the policy, got {} with {}",
            cleanup.status, cleanup.raw
        );
    }

    // The current token, spelled each of the three admissible ways, deletes the policy.
    let spellings: &[(&str, Spelling)] = &[
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (case, spell) in spellings {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_policy(&router, &id, Some(&spell(&etag))).await;
        assert_eq!(
            answer.status, 204,
            "case {case}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// A policy a stored cache behaviour attaches cannot be deleted, and once every behaviour has
/// let go the same delete lands — the attachment the by-policy listing reports is the gate.
///
/// The rows cover which behaviour holds the policy: the default cache behaviour, one ordered
/// behaviour among several, and both at once.
#[tokio::test]
async fn an_attached_policy_cannot_be_deleted_and_a_detached_one_can() {
    let attachments: &[(&str, Row, Row)] = &[
        (
            "the default cache behaviour attaches the policy",
            Row {
                case: "attached at the default behaviour",
                default_policy: Some(0),
                ordered: &[],
            },
            Row {
                case: "detached at the default behaviour",
                default_policy: None,
                ordered: &[],
            },
        ),
        (
            "a later ordered cache behaviour attaches the policy",
            Row {
                case: "attached at an ordered behaviour",
                default_policy: None,
                ordered: &[None, Some(0)],
            },
            Row {
                case: "detached at every ordered behaviour",
                default_policy: None,
                ordered: &[None, None],
            },
        ),
        (
            "the default behaviour and an ordered one both attach the policy",
            Row {
                case: "attached twice",
                default_policy: Some(0),
                ordered: &[Some(0)],
            },
            Row {
                case: "detached everywhere",
                default_policy: None,
                ordered: &[None],
            },
        ),
    ];

    for (case, attached, detached) in attachments {
        let router = emulator();
        let (policy_id, policy_etag) = create(
            &router,
            &Policy {
                case: "the policy under the attachment gate",
                name: "delete-gate".to_string(),
                header_behavior: "whitelist".to_string(),
                headers: Some(vec!["authorization".to_string()]),
                ..Policy::default()
            },
        )
        .await;
        let policies = vec![policy_id.clone()];

        let created =
            create_distribution(&router, &config_xml("ref-delete-gate", attached, &policies)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
            created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&distribution_id),
            "case {case}: expected a minted distribution id, got {distribution_id:?}"
        );
        let distribution_etag = created.header("etag").to_string();

        // The by-policy listing and the delete gate read the SAME attachment.
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert_eq!(
            listed_ids(listed.root()),
            vec![distribution_id.clone()],
            "case {case}: expected the policy to list the distribution that attaches it, got {}",
            listed.raw
        );

        let refused = delete_policy(&router, &policy_id, Some(&policy_etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 while attached, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "OriginRequestPolicyInUse",
            "case {case}: expected OriginRequestPolicyInUse while attached, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE,
            "case {case}: expected the modelled in-use sentence, got {}",
            refused.raw
        );
        assert_eq!(
            get_policy(&router, &policy_id).await.status,
            200,
            "case {case}: expected the refused delete to leave the policy readable"
        );

        // A version that is not the policy's is refused on its own terms, before anything
        // about the attachment.
        for (description, if_match, status, code) in [
            ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
            (
                "a version token that is the distribution's",
                Some(distribution_etag.as_str()),
                412,
                "PreconditionFailed",
            ),
        ] {
            let answer = delete_policy(&router, &policy_id, if_match).await;
            assert_eq!(
                answer.status, status,
                "case {case} with {description}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case} with {description}: expected {code}, got {}",
                answer.raw
            );
        }

        // Remove the policy from every cache behaviour, then the delete lands.
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml("ref-delete-gate", detached, &policies),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            updated.status, updated.raw
        );
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert!(
            listed_ids(listed.root()).is_empty(),
            "case {case}: expected the detached policy to list nothing, got {}",
            listed.raw
        );

        let deleted = delete_policy(&router, &policy_id, Some(&policy_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 once detached, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &policy_id, case).await;

        // The by-policy listing resolves the policy too, so it reports the delete as well.
        let listed = list_by_policy(&router, &policy_id, "").await;
        assert_eq!(
            listed.status, 404,
            "case {case}: expected HTTP 404 from the by-policy listing after the delete, got {} \
             with {}",
            listed.status, listed.raw
        );

        // The distribution itself outlives the policy's delete.
        assert_eq!(
            get_distribution(&router, &distribution_id).await.status,
            200,
            "case {case}: expected the distribution to outlive the policy"
        );
    }
}

/// A policy Amazon Web Services published cannot be deleted by the account that reads it:
/// every entry of the published catalogue is refused with `IllegalDelete` whatever version the
/// request carries, and is still readable afterwards.
#[tokio::test]
async fn a_published_policy_cannot_be_deleted() {
    let router = emulator();

    for managed in MANAGED_ORIGIN_REQUEST_POLICIES {
        let fetched = get_policy(&router, managed.id).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetOriginRequestPolicy for the published policy, \
             got {} with {}",
            managed.name, fetched.status, fetched.raw
        );
        let etag = fetched.header("etag").to_string();

        // The version the request carries cannot make the delete legal, so every form of
        // `If-Match` is refused the same way.
        for (description, if_match) in [
            ("the policy's own version token", Some(etag.as_str())),
            ("no If-Match at all", None),
            ("an If-Match that is no version token", Some("not-a-version")),
        ] {
            let refused = delete_policy(&router, managed.id, if_match).await;
            assert_eq!(
                refused.status, 400,
                "case {} with {description}: expected HTTP 400, got {} with {}",
                managed.name, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalDelete",
                "case {} with {description}: expected IllegalDelete, got {}",
                managed.name,
                refused.raw
            );
            assert_eq!(
                refused.root().find_text("Message").unwrap_or_default(),
                ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE,
                "case {} with {description}: expected the modelled illegal-delete sentence, got {}",
                managed.name,
                refused.raw
            );
        }

        let after = get_policy(&router, managed.id).await;
        assert_eq!(
            after.status, 200,
            "case {}: expected the published policy to stay readable, got {} with {}",
            managed.name, after.status, after.raw
        );
        assert_eq!(
            after.header("etag"),
            etag,
            "case {}: expected the refused delete to leave the version token alone, got {:?}",
            managed.name,
            after.header("etag")
        );
    }
}

/// An id the store holds no origin request policy for is `NoSuchOriginRequestPolicy`,
/// whatever shape the id takes — never a 204, which would claim something was removed.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused() {
    let router = emulator();
    let (known, etag) = create(
        &router,
        &Policy {
            case: "the only policy the account holds",
            name: "the-only-one".to_string(),
            ..Policy::default()
        },
    )
    .await;

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a known id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("a known id in upper case", known.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
        ("a policy name rather than an id", "the-only-one".to_string()),
    ];

    for (case, candidate) in absent {
        let answer = delete_policy(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case {case}: expected NoSuchOriginRequestPolicy for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            "case {case}: expected the modelled message, got {message:?}"
        );
    }

    // None of those refusals touched the policy the account really holds.
    let fetched = get_policy(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the only policy the account holds: expected it to survive every refusal, got {} \
         with {}",
        fetched.status, fetched.raw
    );
}
