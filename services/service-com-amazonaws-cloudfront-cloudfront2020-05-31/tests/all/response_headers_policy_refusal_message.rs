// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE every response-headers-policy refusal carries, end to end over HTTP.
//!
//! One code (`NoSuchResponseHeadersPolicy`) and one status (404) cover two different
//! conditions, so the message is the only thing that tells them apart. The rule under test:
//! which of the two sentences an operation answers is a property of THAT OPERATION, not of
//! the identifier. An operation that ADDRESSES the policy resource by the id (the two reads
//! at `/response-headers-policy/{Id}`) examines the identifier's shape before it looks
//! anything up, so an identifier outside the shape every policy id takes (the 8-4-4-4-12
//! lowercase hex of a UUID) is refused as an unusable identifier while a well-formed id that
//! names no policy is refused as an absent policy. An operation that resolves the id only as
//! a lookup KEY into another resource (`ListDistributionsByResponseHeadersPolicyId`, whose
//! own resource is the distribution) prechecks nothing and refuses EVERY id neither source of
//! policies holds — the account's own records or the catalogue AWS publishes — as an absent
//! policy, whatever shape the id takes. Both sentences are fixed text: neither interpolates
//! anything, and in particular neither echoes the submitted value back.
//!
//! The listing whose resume point is an identifier of this family carries the same rule for
//! its `Marker`: a value that is no policy id at all was never a resume point this service
//! handed out (`InvalidArgument`, 400, under the constraint on the parameter), while a
//! well-formed one naming no policy is the same absent-policy 404.
//!
//! The rows span the shape classes an identifier falls into rather than one recorded value,
//! and the perturbed rows are DERIVED from a live id — one this service minted in the test
//! and one from the published catalogue — so no row is a literal anybody has to maintain.
//! Every refusal is asserted against each operation that resolves such an id, and the
//! refusals are shown to change nothing a later caller sees.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_RESPONSE_HEADERS_POLICY_MARKER_MESSAGE, MANAGED_RESPONSE_HEADERS_POLICIES,
    NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE, RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, API_VERSION};
use crate::distributions_by_response_headers_policy_id::list_by_policy;
use crate::response_headers_policy_config_read::get_policy_config;
use crate::response_headers_policy_lifecycle::{
    create_policy, get_policy, policy_table, text, Policy,
};

/// `GET /2020-05-31/response-headers-policy`, with `query` appended verbatim — the route and
/// the `Marker`/`MaxItems` query binding the model gives `ListResponseHeadersPolicies`.
///
/// `pub(crate)` so the sibling suite that reads a listed policy back drives the listing
/// over this one wire path rather than growing a copy of it.
pub(crate) async fn list_policies(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/response-headers-policy{query}"),
        None,
        None,
    )
    .await
}

/// Create one policy and report `(its id, its version token)`, so the rows that perturb a
/// LIVE identifier are built from one this service really minted.
///
/// `pub(crate)` with the row tables below so the sibling suite that drives the by-policy
/// listings of every family shares these rows rather than growing a copy of them.
pub(crate) async fn create_one(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let policy = Policy {
        case: "the policy that exists",
        name: name.to_string(),
        ..policy_table()
            .into_iter()
            .next()
            .expect("the crate's configuration table has a first row")
    };
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        policy.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", policy.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();
    (id, etag)
}

/// One identifier under test, and which of the two conditions it is.
pub(crate) struct Candidate {
    pub(crate) case: &'static str,
    pub(crate) id: String,
}

/// Identifiers outside the shape a response headers policy id takes, derived from a live
/// `minted` id and a `published` one so the rows follow whatever those ids are.
pub(crate) fn unusable_identifiers(minted: &str, published: &str) -> Vec<Candidate> {
    let hex_only: String = minted.chars().filter(|c| *c != '-').collect();
    let underscores: String = minted.replace('-', "_");
    // An upper-case hex digit in the first slot: outside the shape for ANY minted value,
    // whichever digits that value happens to carry.
    let upper_hex_head: String = minted
        .char_indices()
        .map(|(index, character)| if index == 0 { 'A' } else { character })
        .collect();
    let published_short: String = published
        .rsplit_once('-')
        .map(|(head, _)| head.to_string())
        .unwrap_or_else(|| published.to_string());

    let rows = vec![
        (
            "a token of uppercase alphanumerics with no group separators",
            "NOTAPOLICYIDENTIFIER".to_string(),
        ),
        ("a decimal number", "1234567890".to_string()),
        ("a minted id with its separators removed", hex_only),
        (
            "a minted id with underscores for separators",
            underscores,
        ),
        (
            "a minted id with an upper-case hex digit in its first slot",
            upper_hex_head,
        ),
        (
            "a published id with its last group dropped",
            published_short,
        ),
        (
            "an identifier of another resource family of this service",
            "E2QWRUHAPOMQZL".to_string(),
        ),
        ("a single character", "x".to_string()),
    ];

    rows.into_iter()
        .map(|(case, id)| Candidate { case, id })
        .collect()
}

/// Well-formed identifiers that name no policy — again derived from the live ids, plus one
/// value of the shape nothing is ever minted at.
pub(crate) fn absent_policy_identifiers(minted: &str, published: &str) -> Vec<Candidate> {
    // One hex digit of a live id moved: still exactly the shape a policy id takes, and still
    // an id no policy holds.
    let shift = |value: &str| -> String {
        let mut chars: Vec<char> = value.chars().collect();
        for slot in chars.iter_mut() {
            if slot.is_ascii_hexdigit() {
                *slot = if *slot == '7' { '8' } else { '7' };
                break;
            }
        }
        chars.into_iter().collect()
    };

    let rows = vec![
        (
            "a minted id with its first hex digit moved",
            shift(minted),
        ),
        (
            "a published id with its first hex digit moved",
            shift(published),
        ),
        (
            "every group at the bottom of the hex alphabet",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "groups of mixed digits and letters",
            "9f8e7d6c-5b4a-3210-fedc-ba9876543210".to_string(),
        ),
    ];

    rows.into_iter()
        .map(|(case, id)| Candidate { case, id })
        .collect()
}

/// Whether `id` names a policy either source of this resource holds: the one this test
/// created, or an entry of the catalogue the service publishes. The rows that PERTURB a live
/// id are checked against this, so a catalogue that grows an entry one hex digit from
/// another turns into a failing precondition rather than a confusing success.
fn names_a_policy(id: &str, minted: &str) -> bool {
    id == minted
        || MANAGED_RESPONSE_HEADERS_POLICIES
            .iter()
            .any(|managed| managed.id == id)
}

/// A refusal of `candidate` by `reader`: 404, the one code, and exactly `expected` as the
/// message, with nothing of the submitted value in it.
fn assert_refusal(reader: &str, candidate: &Candidate, answer: &Answer, expected: &str) {
    let case = candidate.case;
    let id = &candidate.id;
    assert_eq!(
        answer.status, 404,
        "case {case} ({reader}, {id:?}): expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchResponseHeadersPolicy",
        "case {case} ({reader}, {id:?}): expected NoSuchResponseHeadersPolicy, got {}",
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case} ({reader}, {id:?}): expected {expected:?}, got {message:?}"
    );
    assert!(
        !echoes_the_identifier(message, id),
        "case {case} ({reader}, {id:?}): the refusal must not echo the identifier, got \
         {message:?}"
    );
}

/// Whether `message` carries the identifier SUBMITTED, which no refusal of this family may
/// do.
///
/// A containment test only MEANS an echo for a value long enough not to occur in ordinary
/// prose: a one- or two-character identifier is a substring of English words (the
/// absent-policy sentence carries "x" inside "exist"), so for a value that short the test
/// says nothing either way and the exact-message assertion beside it is what holds. The
/// bound is on the IDENTIFIER's length rather than on any row, so no row has to be chosen to
/// keep this meaningful.
pub(crate) fn echoes_the_identifier(message: &str, id: &str) -> bool {
    const SHORTEST_ECHOABLE_ID: usize = 4;
    id.len() >= SHORTEST_ECHOABLE_ID && message.contains(id)
}

/// An identifier outside the shape every policy id takes is refused as an unusable
/// identifier by every operation that ADDRESSES the policy resource by it, while the listing
/// that resolves it as a lookup key refuses it as an absent policy. The rows are the shape
/// classes, and each is checked to really be outside the shape before it is submitted.
#[tokio::test]
async fn an_identifier_outside_the_shape_of_a_policy_id_is_refused_as_unusable() {
    let router = emulator();
    let (minted, _) = create_one(&router, "the-policy-that-exists").await;
    let published = MANAGED_RESPONSE_HEADERS_POLICIES
        .first()
        .expect("the published catalogue is not empty")
        .id;

    for candidate in unusable_identifiers(&minted, published) {
        assert!(
            !is_minted_uuid(&candidate.id) && !names_a_policy(&candidate.id, &minted),
            "case {}: the row must be outside the shape of a policy id and name none, got {:?}",
            candidate.case,
            candidate.id
        );
        let answers = [
            (
                "GetResponseHeadersPolicyConfig",
                get_policy_config(&router, &candidate.id).await,
                RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
            ),
            (
                "GetResponseHeadersPolicy",
                get_policy(&router, &candidate.id).await,
                RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
            ),
            (
                "ListDistributionsByResponseHeadersPolicyId",
                list_by_policy(&router, &candidate.id, "").await,
                NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
            ),
        ];
        for (reader, answer, expected) in &answers {
            assert_refusal(reader, &candidate, answer, expected);
        }
    }
}

/// A well-formed identifier that names no policy is refused as an absent policy instead —
/// same status and same code, a different sentence — by every operation that resolves such
/// an id, including the listing that resumes at one.
#[tokio::test]
async fn a_well_formed_identifier_that_names_no_policy_is_refused_as_absent() {
    let router = emulator();
    let (minted, _) = create_one(&router, "the-policy-that-exists").await;
    let published = MANAGED_RESPONSE_HEADERS_POLICIES
        .first()
        .expect("the published catalogue is not empty")
        .id;

    for candidate in absent_policy_identifiers(&minted, published) {
        assert!(
            is_minted_uuid(&candidate.id),
            "case {}: the row must be exactly the shape of a policy id, got {:?}",
            candidate.case,
            candidate.id
        );
        assert!(
            !names_a_policy(&candidate.id, &minted),
            "case {}: the row must name no policy either source holds, got {:?}",
            candidate.case,
            candidate.id
        );
        let answers = [
            (
                "GetResponseHeadersPolicyConfig",
                get_policy_config(&router, &candidate.id).await,
            ),
            (
                "GetResponseHeadersPolicy",
                get_policy(&router, &candidate.id).await,
            ),
            (
                "ListDistributionsByResponseHeadersPolicyId",
                list_by_policy(&router, &candidate.id, "").await,
            ),
            (
                "ListResponseHeadersPolicies",
                list_policies(&router, &format!("?Marker={}", candidate.id)).await,
            ),
        ];
        for (reader, answer) in &answers {
            assert_refusal(
                reader,
                &candidate,
                answer,
                NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
            );
        }
    }
}

/// A `Marker` that is no identifier of this family was never a resume point the service
/// handed out, so the listing refuses it under the constraint on the parameter (400) rather
/// than as a policy that does not exist — and a `Marker` that names a policy either source
/// holds is served.
#[tokio::test]
async fn a_marker_that_is_no_policy_id_is_refused_as_an_unusable_resume_point() {
    let router = emulator();
    let (minted, _) = create_one(&router, "the-policy-that-exists").await;
    let published = MANAGED_RESPONSE_HEADERS_POLICIES
        .first()
        .expect("the published catalogue is not empty")
        .id;

    for candidate in unusable_identifiers(&minted, published) {
        let case = candidate.case;
        let marker = &candidate.id;
        let answer = list_policies(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({marker:?}): expected HTTP 400 for a marker that is no policy id, \
             got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case} ({marker:?}): expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, INVALID_RESPONSE_HEADERS_POLICY_MARKER_MESSAGE,
            "case {case} ({marker:?}): expected the constraint on the parameter, got \
             {message:?}"
        );
    }

    // A marker naming a policy EITHER source holds is a resume point, not a refusal.
    for (case, marker) in [("a minted id", minted.as_str()), ("a published id", published)] {
        let answer = list_policies(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 200,
            "case {case} ({marker:?}): expected HTTP 200 resuming at a policy that exists, got \
             {} with {}",
            answer.status, answer.raw
        );
    }
}

/// Every refusal above is a read that failed: the policy that does exist is readable
/// afterwards with the configuration and the version token it was created with, and the
/// listing answers the same document it did before.
#[tokio::test]
async fn the_refusals_change_nothing_a_later_caller_sees() {
    let router = emulator();
    let (minted, etag) = create_one(&router, "the-policy-that-exists").await;
    let published = MANAGED_RESPONSE_HEADERS_POLICIES
        .first()
        .expect("the published catalogue is not empty")
        .id;

    let before = list_policies(&router, "").await;
    assert_eq!(
        before.status, 200,
        "case the listing before the refusals: expected HTTP 200, got {} with {}",
        before.status, before.raw
    );
    let read_before = get_policy_config(&router, &minted).await;
    assert_eq!(
        read_before.status, 200,
        "case the policy that exists: expected HTTP 200 before the refusals, got {} with {}",
        read_before.status, read_before.raw
    );

    for candidate in unusable_identifiers(&minted, published)
        .into_iter()
        .chain(absent_policy_identifiers(&minted, published))
    {
        let _ = get_policy_config(&router, &candidate.id).await;
        let _ = get_policy(&router, &candidate.id).await;
        let _ = list_by_policy(&router, &candidate.id, "").await;
        let _ = list_policies(&router, &format!("?Marker={}", candidate.id)).await;
    }

    let read_after = get_policy_config(&router, &minted).await;
    assert_eq!(
        read_after.raw, read_before.raw,
        "case the policy that exists: expected the same configuration after the refusals, got \
         {} then {}",
        read_before.raw, read_after.raw
    );
    assert_eq!(
        read_after.header("etag"),
        etag,
        "case the policy that exists: expected the version token to survive the refusals, got \
         {:?}",
        read_after.header("etag")
    );
    let after = list_policies(&router, "").await;
    assert_eq!(
        after.raw, before.raw,
        "case the listing after the refusals: expected the same document, got {} then {}",
        before.raw, after.raw
    );
}
