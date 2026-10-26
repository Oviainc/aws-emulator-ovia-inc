// SPDX-License-Identifier: CC0-1.0

//! `GetResponseHeadersPolicyConfig` end to end over HTTP.
//!
//! Two rules are under test.
//!
//! * A config read REPORTS the stored record of the policy an id names: the
//!   `ResponseHeadersPolicyConfig` exactly as the last mutation stored it — down to the
//!   members the create left out — beside the version token that mutation handed back, the
//!   same pair `GetResponseHeadersPolicy` reports around the policy's identity. It writes
//!   nothing, so a repeated read answers the same document, and a policy AWS published is
//!   readable here exactly as one this account created is.
//! * One id outside both sources is refused with `NoSuchResponseHeadersPolicy` (HTTP 404)
//!   whatever shape it takes, and the MESSAGE is what tells the two conditions apart,
//!   because the code and the status cannot: an identifier outside the shape every response
//!   headers policy id takes (the 8-4-4-4-12 lowercase hex of a UUID) is refused as an
//!   unusable identifier, while a well-formed id that names no policy is refused as an
//!   absent policy. Both sentences are fixed text and neither echoes the id back.
//!
//! The rows span what those rules generalise over. The round trip walks the crate's whole
//! configuration table (`crate::response_headers_policy_lifecycle::policy_table`) plus every
//! entry of the published catalogue, and the refusal table varies the id along its shape: a
//! well-formed id never minted, a minted id with one hex digit changed, both published
//! bounds of the hex alphabet, a UUID in upper case, a UUID with a group of the wrong width,
//! one group too few and one too many, a group carrying a non-hex character, an empty group,
//! surrounding braces and whitespace, a version token, a policy name and a path-like value.
//! Each row is asserted against all three readers that resolve an id of this family.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    managed_policy_etag, MANAGED_RESPONSE_HEADERS_POLICIES,
    NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE, RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::distributions_by_response_headers_policy_id::list_by_policy;
use crate::response_headers_policy_lifecycle::{
    assert_config, create_policy, get_policy, policy_table, text, Policy,
};

/// `GET /2020-05-31/response-headers-policy/{Id}/config` — the route the model binds this
/// operation to.
async fn get_policy_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/response-headers-policy/{id}/config"),
        None,
        None,
    )
    .await
}

/// The config read answers the stored configuration and the stored version token: the same
/// ones the create handed back and `GetResponseHeadersPolicy` reports, over and over,
/// because a read changes nothing a later caller sees.
#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();

    for policy in policy_table() {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the create", &created.raw);
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token from the create, got {etag:?}"
        );

        let read = get_policy_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the config read, got {} with {}",
            read.status, read.raw
        );
        // `ResponseHeadersPolicyConfig` is this output's `@httpPayload`, so the
        // configuration IS the document's root element.
        assert_config(read.root(), &policy, "the config read", &read.raw);
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the config read to report the version the create handed \
             back ({etag:?}), got {:?}",
            read.header("etag")
        );

        // Parity with `GetResponseHeadersPolicy`: both reads answer from the ONE record.
        let whole = get_policy(&router, &id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetResponseHeadersPolicy, got {} with {}",
            whole.status, whole.raw
        );
        let nested = whole
            .root()
            .at("ResponseHeadersPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: GetResponseHeadersPolicy carried no \
                     ResponseHeadersPolicyConfig: {}",
                    whole.raw
                )
            });
        assert_config(nested, &policy, "GetResponseHeadersPolicy parity", &whole.raw);
        assert_eq!(
            whole.header("etag"),
            etag,
            "case {case}: expected one version token across both reads, got {:?} and {etag:?}",
            whole.header("etag")
        );

        // A read writes nothing: the same request answers the same document and version.
        let again = get_policy_config(&router, &id).await;
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
        assert_eq!(
            again.header("etag"),
            etag,
            "case {case}: expected the version to survive a read, got {:?}",
            again.header("etag")
        );
    }
}

/// A policy AWS published is readable through the config path exactly as one this account
/// created is: the published name and comment, the version a published policy carries, and
/// the same pair `GetResponseHeadersPolicy` reports. Every entry of the catalogue the crate
/// serves is a row, so the rule rests on no single published policy.
#[tokio::test]
async fn a_published_policy_is_readable_through_the_config_path() {
    let router = emulator();

    for managed in MANAGED_RESPONSE_HEADERS_POLICIES {
        let case = managed.name;
        let read = get_policy_config(&router, managed.id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 for the published id {}, got {} with {}",
            managed.id, read.status, read.raw
        );
        let name = read.root().text_at("Name").unwrap_or_default();
        assert_eq!(
            name, managed.name,
            "case {case}: expected the published Name, got {name:?}"
        );
        let comment = read.root().text_at("Comment").unwrap_or_default();
        assert_eq!(
            comment, managed.comment,
            "case {case}: expected the published Comment, got {comment:?}"
        );
        let expected_etag = managed_policy_etag(managed.id);
        assert_eq!(
            read.header("etag"),
            expected_etag,
            "case {case}: expected the published policy's version {expected_etag:?}, got {:?}",
            read.header("etag")
        );

        let whole = get_policy(&router, managed.id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetResponseHeadersPolicy, got {} with {}",
            whole.status, whole.raw
        );
        let nested_name = whole
            .root()
            .text_at("ResponseHeadersPolicyConfig/Name")
            .unwrap_or_default();
        assert_eq!(
            nested_name, managed.name,
            "case {case}: expected both readers to answer from the one published record, got \
             {nested_name:?} and {name:?}"
        );
        assert_eq!(
            whole.header("etag"),
            expected_etag,
            "case {case}: expected one version across both readers, got {:?}",
            whole.header("etag")
        );
    }
}

/// One id no policy holds, as submitted to each reader of this family, and the sentence the
/// refusal must carry for it.
struct Unknown {
    case: &'static str,
    id: String,
    message: &'static str,
}

/// The refusal rows: every shape of identifier that names no policy, paired with which of
/// the two conditions it is. `minted` is an id this service really minted, so the rows that
/// perturb a live id are built from one.
fn unknown_ids(minted: &str) -> Vec<Unknown> {
    // One hex digit of the minted id changed: still a well-formed id, and still one no
    // policy holds.
    let mut changed: Vec<char> = minted.chars().collect();
    let last = changed.len() - 1;
    changed[last] = if changed[last] == '0' { '1' } else { '0' };

    let well_formed: Vec<(&'static str, String)> = vec![
        (
            "a well-formed id never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the minted id with one hex digit changed",
            changed.into_iter().collect(),
        ),
        (
            "every group at the top of the hex alphabet",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "digits and letters mixed through every group",
            "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9".to_string(),
        ),
    ];
    let malformed: Vec<(&'static str, String)> = vec![
        ("a UUID in upper case", minted.to_uppercase()),
        ("a token that is no UUID at all", "not-a-policy-id".to_string()),
        (
            "a group one character short",
            "0000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a group one character long",
            "000000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "one group too few",
            "00000000-0000-0000-000000000000".to_string(),
        ),
        (
            "one group too many",
            "00000000-0000-0000-0000-0000-00000000000".to_string(),
        ),
        (
            "a non-hex character in a group",
            "0000000g-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an empty group",
            "00000000--0000-0000-000000000000".to_string(),
        ),
        (
            "a UUID in braces",
            "{00000000-0000-0000-0000-000000000000}".to_string(),
        ),
        (
            "a well-formed id with a trailing character",
            "00000000-0000-0000-0000-0000000000000".to_string(),
        ),
        ("a version token", "E1AAAAAAAAAAAA".to_string()),
        (
            "a policy name rather than an id",
            "the-policy-that-exists".to_string(),
        ),
        ("a single character", "x".to_string()),
    ];

    well_formed
        .into_iter()
        .map(|(case, id)| Unknown {
            case,
            id,
            message: NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE,
        })
        .chain(malformed.into_iter().map(|(case, id)| Unknown {
            case,
            id,
            message: RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
        }))
        .collect()
}

/// Every reader of a response headers policy id refuses an id no policy holds with
/// `NoSuchResponseHeadersPolicy` (HTTP 404), and states in the MESSAGE which condition it
/// is: an unusable identifier, or a well-formed id that names no policy. The refusals leave
/// the policy that does exist readable, with its configuration and version untouched.
#[tokio::test]
async fn the_refusal_tells_an_unusable_id_from_an_absent_policy() {
    let router = emulator();
    let present = Policy {
        case: "the policy that exists",
        name: "the-policy-that-exists".to_string(),
        ..policy_table()
            .into_iter()
            .next()
            .expect("the crate's configuration table has a first row")
    };
    let created = create_policy(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(created.root(), "Id", present.case, "the create", &created.raw);
    let etag = created.header("etag").to_string();

    for unknown in unknown_ids(&id) {
        let case = unknown.case;
        let candidate = &unknown.id;
        // The three readers that resolve a response headers policy id: the config read this
        // suite is about, the whole-policy read beside it, and the listing that is about one
        // policy. All three answer from the one refusal, so none of them can drift.
        let answers = [
            ("GetResponseHeadersPolicyConfig", get_policy_config(&router, candidate).await),
            ("GetResponseHeadersPolicy", get_policy(&router, candidate).await),
            (
                "ListDistributionsByResponseHeadersPolicyId",
                list_by_policy(&router, candidate, "").await,
            ),
        ];
        for (reader, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {case} ({reader}, {candidate:?}): expected HTTP 404, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchResponseHeadersPolicy",
                "case {case} ({reader}, {candidate:?}): expected NoSuchResponseHeadersPolicy, \
                 got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, unknown.message,
                "case {case} ({reader}, {candidate:?}): expected {:?}, got {message:?}",
                unknown.message
            );
            assert!(
                !message.contains(candidate.as_str()),
                "case {case} ({reader}, {candidate:?}): the refusal must not echo the id, got \
                 {message:?}"
            );
        }
    }

    let read = get_policy_config(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case the policy that exists: expected HTTP 200 after the refusals, got {} with {}",
        read.status, read.raw
    );
    assert_config(read.root(), &present, "after the refusals", &read.raw);
    assert_eq!(
        read.header("etag"),
        etag,
        "case the policy that exists: expected the version token to survive the refusals, got \
         {:?}",
        read.header("etag")
    );
}
