// SPDX-License-Identifier: CC0-1.0

//! `DeleteResponseHeadersPolicy`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states ("Deletes a
//! response headers policy"):
//!
//! * the delete REMOVES the one stored record every read of that policy answered from, so
//!   afterwards the id names no policy of this account through either reader
//!   (`NoSuchResponseHeadersPolicy`, HTTP 404) and the name it held is free for a later
//!   create to claim under a new id;
//! * the delete is gated on the policy's own version token: an absent `If-Match` and one that
//!   is no version token at all are `InvalidIfMatchVersion` (HTTP 400), a well-formed token
//!   that is not this policy's current one is `PreconditionFailed` (HTTP 412), and in either
//!   case the policy stays exactly where it was;
//! * "You cannot delete a response headers policy if it's attached to a cache behavior": a
//!   policy a stored cache behaviour attaches is `ResponseHeadersPolicyInUse` (HTTP 409) and
//!   stays readable, and once no behaviour attaches it the same delete lands;
//! * a policy AWS PUBLISHED belongs to AWS, so a delete aimed at one is `IllegalDelete`
//!   (HTTP 400) whatever the store says about it, and the policy stays readable.
//!
//! The rows span what those rules generalise over: every policy
//! [`crate::response_headers_policy_lifecycle::policy_table`] builds (the name alone, each
//! optional configuration alone and in combination, lists of several lengths, both values of
//! each flag, both published bounds of `SamplingRate`, and text that is empty, non-ASCII or
//! carries XML metacharacters); every form an `If-Match` takes (absent, empty, bare, quoted,
//! weak, lower case, another policy's token, a value that is no token); WHICH cache behaviour
//! attaches the policy (the default one, one ordered one, both at once, none); every entry of
//! the published catalogue; and ids of several shapes the store holds nothing for.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the policy writer and readers are
//! the response headers policy suites', and the attaching distribution is
//! [`crate::distributions_by_response_headers_policy_id`]'s document — reused rather than
//! copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MANAGED_RESPONSE_HEADERS_POLICIES;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, is_minted_uuid, is_version_token, update_distribution,
    Answer, API_VERSION,
};
use crate::distributions_by_response_headers_policy_id::{config_xml, Row};
use crate::response_headers_policy_config_read::get_policy_config;
use crate::response_headers_policy_lifecycle::{create_policy, get_policy, policy_table, Policy};

/// The sentences CloudFront sends for these refusals, spelled out here rather than taken from
/// the crate, so a reworded refusal fails this suite.
const NO_SUCH_POLICY_MESSAGE: &str = "The specified response headers policy does not exist.";
const POLICY_IN_USE_MESSAGE: &str =
    "The specified response headers policy is currently associated with a cache behavior. \
     Please disassociate the policy before deleting.";
const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";
/// The policy family's precondition sentence.
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions for this operation.";
/// The sentence a delete aimed at a policy AWS published is refused with.
const ILLEGAL_DELETE_MESSAGE: &str =
    "The request failed because delete is not allowed for this policy.";
/// The sentence an identifier that is in no policy id's shape at all is refused with.
const ID_FORMAT_MESSAGE: &str = "Id format is invalid";

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's
/// own `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/response-headers-policy/{Id}` under the version token `if_match` — the
/// route and the header the model binds this operation to (`IfMatch` travels in `If-Match`).
async fn delete_policy(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/response-headers-policy/{id}"),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/response-headers-policy` — the listing, read here only to check what it
/// reports once a policy is gone.
async fn list_policies(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/response-headers-policy"),
        None,
        None,
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
    let id = answer
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_minted_uuid(&id),
        "case {:?}: expected a minted policy id, got {id:?}",
        row.case
    );
    assert!(
        is_version_token(&etag),
        "case {:?}: expected a minted version token, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Assert that `id` names no response headers policy of this account any more: both readers
/// answer `NoSuchResponseHeadersPolicy` with the modelled sentence, and the listing has
/// stopped reporting it.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    for (reader, answer) in [
        ("GetResponseHeadersPolicy", get_policy(router, id).await),
        (
            "GetResponseHeadersPolicyConfig",
            get_policy_config(router, id).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from {reader} after the delete, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {case:?}: expected NoSuchResponseHeadersPolicy from {reader} after the \
             delete, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_POLICY_MESSAGE,
            "case {case:?}: expected the modelled missing-policy sentence from {reader}, got \
             {message:?}"
        );
    }

    let listed = list_policies(router).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListResponseHeadersPolicies, got {} with {}",
        listed.status, listed.raw
    );
    assert!(
        !listed.raw.contains(id),
        "case {case:?}: expected the listing to stop reporting the deleted policy {id:?}, got {}",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// The suites
// ---------------------------------------------------------------------------

/// Every policy the create accepts is removed by its delete under its version token: the
/// delete answers HTTP 204 with no body, both readers then report the id as naming nothing,
/// deleting it again is `NoSuchResponseHeadersPolicy`, and the name it held is free for a NEW
/// policy under a new id.
#[tokio::test]
async fn every_stored_policy_is_removed_by_its_delete() {
    let router = emulator();

    for row in policy_table() {
        let (id, etag) = create(&router, &row).await;

        // Round trip: the policy is readable before the delete, so the 404 after it is the
        // delete's doing and not a policy that was never stored.
        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetResponseHeadersPolicy before the delete, got \
             {} with {}",
            row.case, fetched.status, fetched.raw
        );

        let deleted = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 from DeleteResponseHeadersPolicy, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {:?}: expected no body from a 204, got {}",
            row.case,
            deleted.raw
        );

        assert_gone(&router, &id, row.case).await;
        let again = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: expected HTTP 404 from a second delete, got {} with {}",
            row.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {:?}: expected NoSuchResponseHeadersPolicy from a second delete, got {}",
            row.case,
            again.raw
        );

        // "The name must be unique for response headers policies in this Amazon Web Services
        // account": the deleted policy's name is nobody's now, so a create under it mints a
        // SECOND policy rather than being refused as a repeated name.
        let (reused_id, reused_etag) = create(&router, &row).await;
        assert_ne!(
            reused_id, id,
            "case {:?}: expected a freshly minted id for the policy recreated under the freed \
             name, got the deleted one {id:?}",
            row.case
        );

        // Leave the store as this row found it, so the next row's name is the only one in
        // play.
        let cleanup = delete_policy(&router, &reused_id, Some(&reused_etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {:?}: expected HTTP 204 deleting the recreated policy, got {} with {}",
            row.case, cleanup.status, cleanup.raw
        );
    }
}

/// The delete is gated on the policy's OWN version token.
///
/// The rows span every form an `If-Match` takes: absent, empty, a value that is no version
/// token at all, a token spelled in lower case, a well-formed token belonging to another
/// policy, and the current token bare, quoted and weak — the three ways HTTP and CloudFront's
/// own `ETag` header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();

    let other = Policy {
        case: "another policy, whose token is not the subject's",
        name: "response-headers-policy-delete-gate-other".to_string(),
        ..Policy::default()
    };
    let (_, other_etag) = create(&router, &other).await;

    let subject = Policy {
        case: "the policy under the version gate",
        name: "response-headers-policy-delete-gate".to_string(),
        comment: Some("under the version gate".to_string()),
        ..Policy::default()
    };

    // Each refusal is asserted on its own freshly created policy, and the policy is read back
    // after it: a refused delete removes nothing.
    let refusals: &[(&str, Option<&str>, u16, &str, &str)] = &[
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
            "an If-Match carrying a lowercase token",
            Some("e1stale1version"),
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

    for (case, if_match, status, code, message) in refusals {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_policy(&router, &id, *if_match).await;
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
        let served = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            served, *message,
            "case {case:?}: expected the modelled sentence {message:?}, got {served:?}"
        );

        let fetched = get_policy(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the refused delete to leave the policy readable, got {} \
             with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case:?}: expected the refused delete to leave the version token alone, got \
             {:?}",
            fetched.header("etag")
        );

        // Clear the way for the next row, which creates a policy under the same name.
        let cleanup = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: expected the current token to delete the policy, got {} with {}",
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
            "case {case:?}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// Where an attachment can sit: the rows say which behaviours of one stored distribution
/// attach the policy under the delete (index 0 is that policy).
const ATTACHMENTS: &[Row] = &[
    Row {
        case: "no stored cache behaviour attaches the policy",
        default_policy: None,
        ordered: &[None],
    },
    Row {
        case: "the default cache behaviour attaches the policy",
        default_policy: Some(0),
        ordered: &[],
    },
    Row {
        case: "one ordered cache behaviour attaches the policy",
        default_policy: None,
        ordered: &[Some(0)],
    },
    Row {
        case: "the default behaviour and a later ordered one both attach the policy",
        default_policy: Some(0),
        ordered: &[None, Some(0)],
    },
];

/// A policy a stored cache behaviour attaches cannot be deleted, and once no behaviour
/// attaches it the same delete lands — the STORED `ResponseHeadersPolicyId` is the gate.
#[tokio::test]
async fn a_policy_a_cache_behaviour_attaches_cannot_be_deleted() {
    for row in ATTACHMENTS {
        let router = emulator();
        let subject = Policy {
            case: "the policy under the attachment gate",
            name: "response-headers-policy-in-use".to_string(),
            ..Policy::default()
        };
        let (id, etag) = create(&router, &subject).await;
        let policies = vec![id.clone()];

        let created = create_distribution(
            &router,
            &config_xml("response-headers-policy-in-use", row, &policies),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        let distribution_etag = created.header("etag").to_string();

        let attaches = row.default_policy == Some(0) || row.ordered.contains(&Some(0));
        let answer = delete_policy(&router, &id, Some(&etag)).await;
        if !attaches {
            // Nothing attaches the policy, so the delete lands straight away — which is what
            // makes the refusals below the attachment's doing rather than the distribution's
            // mere presence.
            assert_eq!(
                answer.status, 204,
                "case {:?}: expected HTTP 204 when no behaviour attaches the policy, got {} \
                 with {}",
                row.case, answer.status, answer.raw
            );
            assert_gone(&router, &id, row.case).await;
            continue;
        }

        assert_eq!(
            answer.status, 409,
            "case {:?}: expected HTTP 409 while a cache behaviour attaches the policy, got {} \
             with {}",
            row.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "ResponseHeadersPolicyInUse",
            "case {:?}: expected ResponseHeadersPolicyInUse while attached, got {}",
            row.case,
            answer.raw
        );
        let served = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            served, POLICY_IN_USE_MESSAGE,
            "case {:?}: expected the modelled in-use sentence, got {served:?}",
            row.case
        );
        assert_eq!(
            get_policy(&router, &id).await.status,
            200,
            "case {:?}: expected the refused delete to leave the policy readable",
            row.case
        );

        // Take the attachment off every behaviour, and the SAME delete lands.
        let detached = Row {
            case: row.case,
            default_policy: None,
            ordered: &[None],
        };
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml("response-headers-policy-in-use", &detached, &policies),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            row.case, updated.status, updated.raw
        );

        let deleted = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 once no behaviour attaches the policy, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert_gone(&router, &id, row.case).await;
    }
}

/// A policy AWS published is not this account's to delete: the refusal is `IllegalDelete`
/// whatever the request states about the version and whatever the store says about the
/// attachment, and the policy stays readable afterwards. Asserted over EVERY entry of the
/// published catalogue.
#[tokio::test]
async fn a_published_policy_is_refused_the_delete() {
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

        for (described, if_match) in [
            ("no If-Match at all", None),
            ("the policy's own current version", Some(etag.as_str())),
            ("a version token that is no policy's", Some("E1STALE1VERSIO")),
        ] {
            let refused = delete_policy(&router, managed.id, if_match).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} with {described}: expected HTTP 400, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalDelete",
                "case {case:?} with {described}: expected IllegalDelete rather than an \
                 attachment or existence refusal, got {}",
                refused.raw
            );
            let served = refused.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                served, ILLEGAL_DELETE_MESSAGE,
                "case {case:?} with {described}: expected the delete-not-allowed sentence, got \
                 {served:?}"
            );
        }

        // The refusals left the published policy exactly where it was, at the same version.
        let after = get_policy(&router, managed.id).await;
        assert_eq!(
            (after.status, after.header("etag")),
            (200, etag.as_str()),
            "case {case:?}: expected the refused deletes to leave the policy unchanged, got {}",
            after.raw
        );
    }
}

/// An id the store holds no policy of this account for is `NoSuchResponseHeadersPolicy`, and
/// the MESSAGE tells an identifier that is in no policy id's shape at all from a well-formed
/// one that names nothing — never a 204, which would claim something was removed.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused() {
    let router = emulator();
    let row = Policy {
        case: "the only policy the account holds",
        name: "response-headers-policy-the-only-one".to_string(),
        ..Policy::default()
    };
    let (known, etag) = create(&router, &row).await;

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String, &str); 5] = [
        (
            "a policy id never minted",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
            NO_SUCH_POLICY_MESSAGE,
        ),
        (
            "a known id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_POLICY_MESSAGE,
        ),
        (
            "a known id in upper case",
            known.to_uppercase(),
            ID_FORMAT_MESSAGE,
        ),
        (
            "a token that is no policy id",
            "not-a-policy-id".to_string(),
            ID_FORMAT_MESSAGE,
        ),
        ("the name rather than the id", row.name.clone(), ID_FORMAT_MESSAGE),
    ];

    for (case, candidate, message) in absent {
        let answer = delete_policy(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResponseHeadersPolicy",
            "case {case:?}: expected NoSuchResponseHeadersPolicy for {candidate:?}, got {}",
            answer.raw
        );
        let served = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            served, message,
            "case {case:?}: expected the modelled sentence {message:?}, got {served:?}"
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
