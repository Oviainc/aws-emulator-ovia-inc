// SPDX-License-Identifier: CC0-1.0

//! `UpdateCloudFrontOriginAccessIdentity` and the records the identity reads then report,
//! end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for updating an
//! origin access identity:
//!
//! * an accepted update REPLACES the stored configuration with the one submitted, under
//!   the id the create minted and beside the Amazon S3 canonical user id minted with it,
//!   and mints a fresh version token — so `GetCloudFrontOriginAccessIdentity` and
//!   `GetCloudFrontOriginAccessIdentityConfig` then answer from that record, and the token
//!   the update matched on no longer satisfies an `If-Match`;
//! * the version token a mutation hands back is the only one an `If-Match` against that
//!   identity satisfies afterwards: an absent or malformed header is
//!   `InvalidIfMatchVersion` (HTTP 400, "The If-Match version is missing or not valid"), a
//!   well-formed token that is not the current one is `PreconditionFailed` (HTTP 412), and
//!   the current one is accepted however HTTP spells an entity tag — bare, quoted or weak;
//! * every constraint the create path refuses a configuration with holds on an update too
//!   ("The comment cannot be longer than 128 characters"), and the `CallerReference` that
//!   names an identity for its whole life cannot be moved — that is `IllegalUpdate` (HTTP
//!   400, "The update contains modifications that are not allowed"); a refused update
//!   writes NOTHING, so the stored configuration and its version token are what they were;
//! * an id no stored identity holds is `NoSuchCloudFrontOriginAccessIdentity` (HTTP 404),
//!   whatever the `If-Match` header says.
//!
//! The table rows differ along everything these rules generalise over: the comment's
//! length (the empty comment, one character, the documented bound of 128 and past it) and
//! character set (ASCII, XML-significant characters, non-ASCII), the caller reference's
//! form and length, the shape of the id the update names, and the form the `If-Match`
//! header takes.
//!
//! The harness (the router, the request helpers, the element scanner) and the
//! configuration table are the origin access identity lifecycle suite's and the
//! distribution lifecycle suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ORIGIN_ACCESS_IDENTITY_COMMENT, NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::origin_access_identity_lifecycle::{
    assert_config, assert_identity, config_table, create_identity, get_identity,
    get_identity_config, Config,
};

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/origin-access-identity/cloudfront/{Id}/config` with the
/// `CloudFrontOriginAccessIdentityConfig` document `body` and the `If-Match` header
/// `if_match` (left off entirely when `None`).
async fn update_identity(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront/{id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// Create the identity `config` describes and report the id and version token the create
/// minted.
async fn seed(router: &MultiServiceRouter, config: &Config) -> (String, String) {
    let created = create_identity(router, &config.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        config.case, created.status, created.raw
    );
    let (id, _) = assert_identity(&created, config, "the create");
    let etag = created.header("etag").to_string();
    (id, etag)
}

/// Assert both identity readers answer for `id` with `config`'s configuration beside
/// `etag` — the one stored record, read back through the two operations that report it.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    config: &Config,
    etag: &str,
    context: &str,
) {
    let case = config.case;

    let fetched = get_identity(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case} ({context}): expected HTTP 200 from the get, got {} with {}",
        fetched.status, fetched.raw
    );
    let (fetched_id, _) = assert_identity(&fetched, config, context);
    assert_eq!(
        fetched_id, id,
        "case {case} ({context}): expected the identity to keep the id {id}, got {fetched_id}"
    );
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        fetched.header("etag")
    );

    let config_answer = get_identity_config(router, id).await;
    assert_eq!(
        config_answer.status, 200,
        "case {case} ({context}): expected HTTP 200 from the config get, got {} with {}",
        config_answer.status, config_answer.raw
    );
    assert_config(config_answer.root(), config, context, &config_answer.raw);
    assert_eq!(
        config_answer.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag} from the config \
         get, got {:?}",
        config_answer.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An accepted update replaces the stored configuration and mints a fresh version token:
/// both reads then report the submitted configuration under the id the create minted,
/// beside that new token — and the token the update matched on no longer satisfies an
/// `If-Match`.
///
/// The rows walk the whole configuration table, each identity updated to the NEXT row's
/// comment so the submitted configuration differs from the stored one along what the
/// update may change (the caller reference is immutable, so each row keeps its own).
#[tokio::test]
async fn an_update_replaces_the_stored_configuration_both_reads_then_report() {
    let router = emulator();
    let table = config_table();
    let mut etags = BTreeSet::new();

    for (index, config) in table.iter().enumerate() {
        let case = config.case;
        let created = Config {
            caller_reference: format!("update-from-{index}-{}", config.caller_reference),
            ..config.clone()
        };
        let (id, create_etag) = seed(&router, &created).await;
        assert!(
            etags.insert(create_etag.clone()),
            "case {case}: the version token {create_etag} repeated"
        );

        // The next row's comment, under this identity's own caller reference.
        let next = &table[(index + 1) % table.len()];
        let target = Config {
            case,
            comment: next.comment.clone(),
            ..created.clone()
        };

        let updated = update_identity(&router, &id, &target.xml(), Some(&create_etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from the update, got {} with {}",
            updated.status, updated.raw
        );
        let (updated_id, _) = assert_identity(&updated, &target, "the update");
        assert_eq!(
            updated_id, id,
            "case {case}: expected the update to report the identity's own id {id}, got \
             {updated_id}"
        );

        let etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the update's ETag header, got {etag:?}"
        );
        assert_ne!(
            etag, create_etag,
            "case {case}: expected the update to mint a fresh version token, got {etag} again"
        );
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        assert_stored(&router, &id, &target, &etag, "after the update").await;

        // The token the update matched on is superseded: it is a well-formed version that
        // is no longer current, so a second update carrying it fails the precondition.
        let stale = update_identity(&router, &id, &created.xml(), Some(&create_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the superseded token {create_etag}, got {} \
             with {}",
            stale.status, stale.raw
        );
        assert_eq!(
            stale.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {}",
            stale.raw
        );
        assert_stored(&router, &id, &target, &etag, "after the stale update").await;
    }
}

/// What the `If-Match` header says about the stored version decides the update: absent or
/// not a version token at all is `InvalidIfMatchVersion` (HTTP 400); a well-formed token
/// that is not this identity's current one is `PreconditionFailed` (HTTP 412); the current
/// one is accepted however HTTP spells an entity tag — bare, quoted or weak.
///
/// Each refused row leaves the stored configuration and its version token untouched; each
/// accepted row replaces them, and the row that follows is checked against the token the
/// accepted one minted.
#[tokio::test]
async fn the_if_match_precondition_decides_the_update() {
    let router = emulator();
    let mut stored = Config {
        case: "the identity under update",
        caller_reference: "precondition-subject".to_string(),
        comment: "the stored configuration".to_string(),
    };
    let (id, mut current) = seed(&router, &stored).await;

    // A second identity, so one of the rows below can carry a version token that is
    // current for SOME identity and still not this one's.
    let other = Config {
        case: "another identity, holding a live token of its own",
        caller_reference: "precondition-other".to_string(),
        comment: "another identity".to_string(),
    };
    let (_, other_etag) = seed(&router, &other).await;

    /// How the `If-Match` header is spelled for one row.
    #[derive(Debug, Clone, Copy)]
    enum Header {
        /// No `If-Match` header at all.
        Absent,
        /// A literal value, carried verbatim.
        Literal(&'static str),
        /// Another identity's current token.
        Other,
        /// This identity's current token, rendered by the row's own format string.
        Current(&'static str),
    }

    // (case, the header, expected status, expected wire code — 200 and "" when accepted)
    let rows: [(&str, Header, u16, &str); 9] = [
        (
            "no If-Match header at all",
            Header::Absent,
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty If-Match header",
            Header::Literal(""),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match that is no version token",
            Header::Literal("not-a-version-token"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match in the token shape but lower case",
            Header::Literal("e1234567890123"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a well-formed token never minted",
            Header::Literal("E1234567890123"),
            412,
            "PreconditionFailed",
        ),
        (
            "another identity's current token",
            Header::Other,
            412,
            "PreconditionFailed",
        ),
        (
            "the current token, bare",
            Header::Current("{current}"),
            200,
            "",
        ),
        (
            "the current token, quoted as an entity tag",
            Header::Current("\"{current}\""),
            200,
            "",
        ),
        (
            "the current token, as a weak entity tag",
            Header::Current("W/\"{current}\""),
            200,
            "",
        ),
    ];

    for (round, (case, header, status, code)) in rows.iter().enumerate() {
        // A comment that differs from the stored one in every round, so an update that
        // landed when it should not have is visible in the read that follows.
        let submitted = Config {
            case: "the submitted configuration",
            caller_reference: stored.caller_reference.clone(),
            comment: format!("round {round}"),
        };

        let value = match header {
            Header::Absent => None,
            Header::Literal(literal) => Some((*literal).to_string()),
            Header::Other => Some(other_etag.clone()),
            Header::Current(format) => Some(format.replace("{current}", &current)),
        };
        let answer = update_identity(&router, &id, &submitted.xml(), value.as_deref()).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status} for If-Match {value:?}, got {} with {}",
            answer.status, answer.raw
        );

        if *status != 200 {
            assert_eq!(
                answer.error_code(),
                *code,
                "case {case}: expected {code} for If-Match {value:?}, got {}",
                answer.raw
            );
            assert_stored(&router, &id, &stored, &current, &format!("after {case}")).await;
            continue;
        }

        let fresh = answer.header("etag").to_string();
        assert!(
            is_version_token(&fresh),
            "case {case}: expected a version token in the ETag header, got {fresh:?}"
        );
        assert_ne!(
            fresh, current,
            "case {case}: expected a fresh version token, got {fresh} again"
        );
        stored = Config {
            case: stored.case,
            ..submitted
        };
        current = fresh;
        assert_stored(&router, &id, &stored, &current, &format!("after {case}")).await;
    }
}

/// A refused update writes NOTHING: the stored configuration and its version token are
/// exactly what they were, so the identity is still readable and still updatable with the
/// token it already had.
///
/// The rows span the refusals an update of a configuration carries: a comment one
/// character past the documented bound, a comment far past it, a comment past it in
/// characters rather than bytes, and a `CallerReference` moved off the one the identity was
/// created under (including one that differs only in case or by trailing text).
#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let router = emulator();
    let stored = Config {
        case: "the identity under update",
        caller_reference: "refusal-subject".to_string(),
        comment: "the stored configuration".to_string(),
    };
    let (id, etag) = seed(&router, &stored).await;

    // (case, the submitted configuration, expected status, expected wire code)
    let rows: Vec<(&str, Config, u16, &str)> = vec![
        (
            "a comment one character past the documented bound",
            Config {
                case: "past the bound by one",
                caller_reference: stored.caller_reference.clone(),
                comment: "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT + 1),
            },
            400,
            "InvalidArgument",
        ),
        (
            "a comment far past the documented bound",
            Config {
                case: "far past the bound",
                caller_reference: stored.caller_reference.clone(),
                comment: "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT * 4),
            },
            400,
            "InvalidArgument",
        ),
        (
            "a non-ASCII comment past the bound in characters",
            Config {
                case: "non-ASCII past the bound",
                caller_reference: stored.caller_reference.clone(),
                comment: "é".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT + 1),
            },
            400,
            "InvalidArgument",
        ),
        (
            "a CallerReference moved to another value",
            Config {
                case: "another caller reference",
                caller_reference: "refusal-elsewhere".to_string(),
                comment: "within the bound".to_string(),
            },
            400,
            "IllegalUpdate",
        ),
        (
            "a CallerReference differing only in case",
            Config {
                case: "the caller reference in upper case",
                caller_reference: stored.caller_reference.to_uppercase(),
                comment: "within the bound".to_string(),
            },
            400,
            "IllegalUpdate",
        ),
        (
            "a CallerReference with text appended",
            Config {
                case: "the caller reference with a suffix",
                caller_reference: format!("{}-2", stored.caller_reference),
                comment: "within the bound".to_string(),
            },
            400,
            "IllegalUpdate",
        ),
    ];

    for (case, submitted, status, code) in rows {
        let answer = update_identity(&router, &id, &submitted.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} from the update, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        // Nothing was written: the configuration and the version token are untouched, so
        // the token the refused update carried is still the current one.
        assert_stored(&router, &id, &stored, &etag, &format!("after {case}")).await;
    }

    // A comment AT the documented bound is accepted, under the same token every refusal
    // left current.
    let accepted = Config {
        case: "a comment at the documented bound",
        caller_reference: stored.caller_reference.clone(),
        comment: "c".repeat(MAX_ORIGIN_ACCESS_IDENTITY_COMMENT),
    };
    let answer = update_identity(&router, &id, &accepted.xml(), Some(&etag)).await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected HTTP 200, got {} with {}",
        accepted.case, answer.status, answer.raw
    );
    let fresh = answer.header("etag").to_string();
    assert_stored(&router, &id, &accepted, &fresh, "after the accepted update").await;
}

/// An id no stored identity holds is `NoSuchCloudFrontOriginAccessIdentity` (HTTP 404) from
/// the update, whatever shape the id takes and whatever the `If-Match` header says — the id
/// is what the update is about, so it is answered before any precondition. Nothing is
/// created under the refused id, and the identity that does exist is untouched.
#[tokio::test]
async fn the_update_refuses_an_id_no_identity_holds() {
    let router = emulator();
    let present = Config {
        case: "the one identity that exists",
        caller_reference: "present-under-update".to_string(),
        comment: "present".to_string(),
    };
    let (id, etag) = seed(&router, &present).await;

    let submitted = Config {
        case: "the configuration the refused updates submit",
        caller_reference: "never-stored".to_string(),
        comment: "never stored".to_string(),
    };
    let body = submitted.xml();

    // Rows: an id of the right shape never minted, the existing id lowercased, a prefix of
    // it, a value of no identity shape and the identity's caller reference — each with a
    // different `If-Match` form, so the 404 is shown to come before the precondition
    // rather than beside one particular header.
    let rows: [(&str, String, Option<String>); 5] = [
        (
            "an id of the right shape, never minted, with the live identity's token",
            "E2QWRUHAPOMQZL".to_string(),
            Some(etag.clone()),
        ),
        (
            "the existing id in another case, with no If-Match at all",
            id.to_lowercase(),
            None,
        ),
        (
            "a prefix of the existing id, with a malformed If-Match",
            id[..6].to_string(),
            Some("not-a-version-token".to_string()),
        ),
        (
            "a value of no identity shape, with a well-formed token never minted",
            "not-an-identity".to_string(),
            Some("E1234567890123".to_string()),
        ),
        (
            "the identity's caller reference rather than its id",
            present.caller_reference.clone(),
            Some(etag.clone()),
        ),
    ];

    for (case, candidate, if_match) in rows {
        let answer = update_identity(&router, &candidate, &body, if_match.as_deref()).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchCloudFrontOriginAccessIdentity",
            "case {case}: expected NoSuchCloudFrontOriginAccessIdentity for {candidate:?}, \
             got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE,
            "case {case}: expected the modelled sentence, got {message:?}"
        );

        // The refused update created nothing under the id it named.
        let fetched = get_identity_config(&router, &candidate).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected {candidate:?} to still name no identity, got {} with {}",
            fetched.status, fetched.raw
        );
    }

    assert_stored(&router, &id, &present, &etag, "after the refused updates").await;
}
