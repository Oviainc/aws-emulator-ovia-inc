// SPDX-License-Identifier: CC0-1.0

//! The CloudFront origin access identity surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's origin access identity
//! surface states:
//!
//! * a create PERSISTS the identity it mints — an `Id`, the Amazon S3 canonical user id a
//!   bucket policy names it by, and a version token — so a later
//!   `GetCloudFrontOriginAccessIdentity` and `GetCloudFrontOriginAccessIdentityConfig`
//!   answer from that record rather than from the request being served;
//! * `CallerReference` identifies ONE identity for its whole life: "If the
//!   `CallerReference` is a value already sent in a previous identity request, and the
//!   content of the `CloudFrontOriginAccessIdentityConfig` is identical to the original
//!   request (ignoring white space), the response includes the same information returned
//!   to the original request", and a repeat that changes the configuration "returns a
//!   `CloudFrontOriginAccessIdentityAlreadyExists` error";
//! * an id no identity holds is not found, whatever shape the id takes;
//! * "The comment cannot be longer than 128 characters": a longer one is refused and
//!   nothing is stored for it.
//!
//! The table rows differ along everything these rules generalise over: the caller
//! reference's form (a timestamp, a UUID, a sentence with whitespace and XML-significant
//! characters) and length, and the comment's length (the empty comment, one character,
//! and the documented bound of 128) and character set.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ORIGIN_ACCESS_IDENTITY_COMMENT, NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE,
    S3_CANONICAL_USER_ID_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION, HOST};

// ---------------------------------------------------------------------------
// Calling the origin access identity surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/origin-access-identity/cloudfront` with the configuration document
/// `body`.
async fn create_identity(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-identity/cloudfront/{Id}`.
async fn get_identity(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-identity/cloudfront/{Id}/config`.
async fn get_identity_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront/{id}/config"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `CloudFrontOriginAccessIdentityConfig` a client submits. Both members are required
/// on the wire, so every row carries both.
#[derive(Debug, Clone)]
struct Config {
    case: &'static str,
    caller_reference: String,
    comment: String,
}

impl Config {
    /// The `CloudFrontOriginAccessIdentityConfig` XML a client sends: the operation's
    /// `httpPayload` member, so the configuration IS the request document.
    fn xml(&self) -> String {
        format!(
            "<CloudFrontOriginAccessIdentityConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>\
             <Comment>{}</Comment>\
             </CloudFrontOriginAccessIdentityConfig>",
            escape(&self.caller_reference),
            escape(&self.comment)
        )
    }
}

/// Escape the five entities XML predefines, so a reference or comment may carry any text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The configurations every rule below is asserted over.
///
/// `CloudFrontOriginAccessIdentityConfig$CallerReference` is "A unique value (for example,
/// a date-time stamp)" and `$Comment` is "A comment to describe the origin access
/// identity. The comment cannot be longer than 128 characters" (both from `smithy_query`
/// on `CloudFrontOriginAccessIdentityConfig`); neither member carries a modelled
/// constraint, so the rows span the forms a caller actually sends and sit at the
/// documented comment bound.
fn config_table() -> Vec<Config> {
    vec![
        Config {
            case: "a date-time stamp reference, the empty comment",
            caller_reference: "2026-01-02T03:04:05.678Z".to_string(),
            comment: String::new(),
        },
        Config {
            case: "a UUID reference, a one-character comment",
            caller_reference: "3f6b1c1e-7a4d-4e5f-9b2c-8d0e1f2a3b4c".to_string(),
            comment: "s".to_string(),
        },
        Config {
            case: "a reference carrying whitespace and XML-significant characters",
            caller_reference: "oai for \"assets\" & <static> 2026".to_string(),
            comment: "serves s3://assets via a bucket policy".to_string(),
        },
        Config {
            case: "a non-ASCII comment",
            caller_reference: "참조-42".to_string(),
            comment: "プロダクション — 配信元アクセス".to_string(),
        },
        Config {
            case: "the longest documented comment (128)",
            caller_reference: "c".repeat(256),
            comment: "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT),
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service minted
// ---------------------------------------------------------------------------

/// Whether `value` has the shape of the id CloudFront mints for an origin access identity
/// — `E` followed by 13 uppercase alphanumerics, as `CloudFrontOriginAccessIdentity$Id`'s
/// own example (`E74FTE3AJFJ256A`) takes, and the shape this service's version tokens take
/// too.
fn is_cloudfront_token(value: &str) -> bool {
    match value.strip_prefix('E') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Whether `value` has the shape of an Amazon S3 canonical user id: 64 lowercase hex
/// characters, the form a bucket policy or ACL grantee takes.
fn is_s3_canonical_user_id(value: &str) -> bool {
    value.len() == S3_CANONICAL_USER_ID_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The text of one member of a response document, or a panic naming the case.
fn member(root: &Node, path: &str, case: &str, raw: &str) -> String {
    root.text_at(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {raw}"))
        .to_string()
}

/// Assert the `CloudFrontOriginAccessIdentity` document `answer` carries is the record
/// `config` asked for, and answer its `(Id, S3CanonicalUserId)`.
fn assert_identity(answer: &Answer, config: &Config, context: &str) -> (String, String) {
    let case = config.case;
    let root = answer.root();
    let raw = answer.raw.as_str();

    let id = member(root, "Id", case, raw);
    assert!(
        is_cloudfront_token(&id),
        "case {case} ({context}): expected a minted identity Id, got {id:?}"
    );

    let canonical = member(root, "S3CanonicalUserId", case, raw);
    assert!(
        is_s3_canonical_user_id(&canonical),
        "case {case} ({context}): expected a 64-character lowercase hex S3 canonical user \
         id, got {canonical:?}"
    );

    assert_config(
        root.at("CloudFrontOriginAccessIdentityConfig")
            .unwrap_or_else(|| {
                panic!("case {case} ({context}): the answer carries no configuration: {raw}")
            }),
        config,
        context,
        raw,
    );

    (id, canonical)
}

/// Assert the `CloudFrontOriginAccessIdentityConfig` element `stored` is the one `config`
/// submitted, member for member.
fn assert_config(stored: &Node, config: &Config, context: &str, raw: &str) {
    let case = config.case;

    let caller_reference = member(stored, "CallerReference", case, raw);
    assert_eq!(
        caller_reference, config.caller_reference,
        "case {case} ({context}): expected the submitted CallerReference, got \
         {caller_reference:?}"
    );

    // The empty comment serialises as an empty element, which the reader reports as the
    // empty text rather than as a missing member.
    let comment = stored.text_at("Comment").unwrap_or("").to_string();
    assert_eq!(
        comment, config.comment,
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the identity it mints, and both reads answer from that one record:
/// the configuration as submitted, a minted `Id`, the S3 canonical user id and the version
/// token the create handed back. Across the table, no two identities share an id, a
/// canonical user id or a version token.
#[tokio::test]
async fn create_persists_the_identity_the_reads_then_report() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut canonicals = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for config in config_table() {
        let case = config.case;
        let created = create_identity(&router, &config.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let (id, canonical) = assert_identity(&created, &config, "the create");
        let etag = created.header("etag").to_string();
        assert!(
            is_cloudfront_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/origin-access-identity/cloudfront/{id}"),
            "case {case}: expected the new identity's URI in Location, got {location:?}"
        );

        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(
            canonicals.insert(canonical.clone()),
            "case {case}: the canonical user id {canonical} repeated"
        );
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The identity read answers from the stored record, not from any request.
        let fetched = get_identity(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
            fetched.status, fetched.raw
        );
        let (fetched_id, fetched_canonical) = assert_identity(&fetched, &config, "the get");
        assert_eq!(
            (fetched_id, fetched_canonical),
            (id.clone(), canonical),
            "case {case}: the get must report the record the create stored"
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case}: expected the create's version token from the get, got {:?}",
            fetched.header("etag")
        );

        // ... and so does the configuration read.
        let config_answer = get_identity_config(&router, &id).await;
        assert_eq!(
            config_answer.status, 200,
            "case {case}: expected HTTP 200 from the config get, got {} with {}",
            config_answer.status, config_answer.raw
        );
        assert_config(
            config_answer.root(),
            &config,
            "the config get",
            &config_answer.raw,
        );
        assert_eq!(
            config_answer.header("etag"),
            etag,
            "case {case}: expected the create's version token from the config get, got {:?}",
            config_answer.header("etag")
        );
    }
}

/// "If the `CallerReference` is a value already sent in a previous identity request, and
/// the content of the `CloudFrontOriginAccessIdentityConfig` is identical to the original
/// request (ignoring white space), the response includes the same information returned to
/// the original request": the repeat creates NOTHING — it answers with the identity the
/// first create minted, at its version.
#[tokio::test]
async fn a_repeated_caller_reference_with_the_same_config_creates_nothing() {
    let router = emulator();

    for config in config_table() {
        let case = config.case;
        let first = create_identity(&router, &config.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let (id, canonical) = assert_identity(&first, &config, "the first create");
        let etag = first.header("etag").to_string();

        let repeat = create_identity(&router, &config.xml()).await;
        assert_eq!(
            repeat.status, 201,
            "case {case}: expected HTTP 201 for the repeat, got {} with {}",
            repeat.status, repeat.raw
        );
        let (repeat_id, repeat_canonical) = assert_identity(&repeat, &config, "the repeat");
        assert_eq!(
            (repeat_id, repeat_canonical),
            (id.clone(), canonical),
            "case {case}: the repeat must answer with the identity the first create minted"
        );
        assert_eq!(
            repeat.header("etag"),
            etag,
            "case {case}: the repeat mints no version token, got {:?}",
            repeat.header("etag")
        );
        let location = repeat.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/origin-access-identity/cloudfront/{id}"),
            "case {case}: the repeat must name the existing identity, got {location:?}"
        );
    }
}

/// A repeat under a taken `CallerReference` whose configuration DIFFERS "returns a
/// `CloudFrontOriginAccessIdentityAlreadyExists` error" (HTTP 409), and the identity that
/// holds the reference keeps its configuration and its version token.
#[tokio::test]
async fn a_repeated_caller_reference_with_another_config_is_refused() {
    let router = emulator();

    for config in config_table() {
        let case = config.case;
        let first = create_identity(&router, &config.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let (id, _) = assert_identity(&first, &config, "the first create");
        let etag = first.header("etag").to_string();

        // A comment that differs from every row's own, and sits within the documented
        // bound, so what the repeat changes is the CONTENT of the configuration.
        let changed = Config {
            comment: "a configuration that differs from the first".to_string(),
            ..config.clone()
        };
        let second = create_identity(&router, &changed.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a taken caller reference, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "CloudFrontOriginAccessIdentityAlreadyExists",
            "case {case}: expected CloudFrontOriginAccessIdentityAlreadyExists, got {}",
            second.raw
        );

        // The refused create changed nothing about the identity that holds the reference.
        let after = get_identity_config(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected HTTP 200 after the refusal, got {} with {}",
            after.status, after.raw
        );
        assert_config(after.root(), &config, "after a refusal", &after.raw);
        assert_eq!(
            after.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// An id no identity holds is not found — `NoSuchCloudFrontOriginAccessIdentity` (HTTP
/// 404) — on BOTH reads, whatever shape the id takes, and a create under one id leaves
/// every other id unfound.
#[tokio::test]
async fn the_reads_refuse_an_id_no_identity_holds() {
    let router = emulator();
    let present = Config {
        case: "the one identity that exists",
        caller_reference: "the-identity-that-exists".to_string(),
        comment: "present".to_string(),
    };
    let created = create_identity(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );
    let (id, _) = assert_identity(&created, &present, "the create");

    // Rows: an id of the right shape that was never minted, the existing id lowercased
    // (ids are compared as given), a prefix of it, and a value of another shape entirely.
    let absent: [(&str, String); 4] = [
        ("an id of the right shape, never minted", "E2QWRUHAPOMQZL".to_string()),
        ("the existing id in another case", id.to_lowercase()),
        ("a prefix of the existing id", id[..6].to_string()),
        ("a value of no identity shape", "not-an-identity".to_string()),
    ];
    for (case, candidate) in absent {
        for (what, answer) in [
            ("the get", get_identity(&router, &candidate).await),
            ("the config get", get_identity_config(&router, &candidate).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} ({what}): expected HTTP 404 for {candidate:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchCloudFrontOriginAccessIdentity",
                "case {case} ({what}): expected NoSuchCloudFrontOriginAccessIdentity for \
                 {candidate:?}, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE,
                "case {case} ({what}): expected the modelled sentence, got {message:?}"
            );
        }
    }

    // The identity that does exist is still readable.
    let still_there = get_identity(&router, &id).await;
    assert_eq!(
        still_there.status, 200,
        "case the one identity that exists: expected HTTP 200, got {} with {}",
        still_there.status, still_there.raw
    );
}

/// "The comment cannot be longer than 128 characters": a comment AT the bound is accepted
/// and one past it is refused with `InvalidArgument` (HTTP 400), which stores nothing — so
/// the caller reference the refused create named is still free.
#[tokio::test]
async fn a_comment_past_the_documented_bound_is_refused() {
    let router = emulator();

    // Rows: the bound itself, one character past it, and far past it; the non-ASCII row
    // pins that the bound counts CHARACTERS rather than bytes.
    let cases: [(&str, String, u16); 4] = [
        (
            "at the bound",
            "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT),
            201,
        ),
        (
            "one character past the bound",
            "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT + 1),
            400,
        ),
        (
            "far past the bound",
            "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT * 4),
            400,
        ),
        (
            "128 non-ASCII characters, under the bound in characters",
            "é".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT),
            201,
        ),
    ];

    for (case, comment, expected) in cases {
        let config = Config {
            case: "the comment bound",
            caller_reference: format!("comment-bound-{case}"),
            comment,
        };
        let answer = create_identity(&router, &config.xml()).await;
        assert_eq!(
            answer.status, expected,
            "case {case}: expected HTTP {expected}, got {} with {}",
            answer.status, answer.raw
        );
        if expected == 400 {
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}: expected InvalidArgument, got {}",
                answer.raw
            );
            // Nothing was stored, so the reference is free for a create that fits.
            let within = Config {
                comment: "within the bound".to_string(),
                ..config.clone()
            };
            let retry = create_identity(&router, &within.xml()).await;
            assert_eq!(
                retry.status, 201,
                "case {case}: the refused create must leave the caller reference free, got \
                 {} with {}",
                retry.status, retry.raw
            );
        }
    }
}
