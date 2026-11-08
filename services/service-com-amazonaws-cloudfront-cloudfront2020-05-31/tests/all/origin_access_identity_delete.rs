// SPDX-License-Identifier: CC0-1.0

//! `DeleteCloudFrontOriginAccessIdentity`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's origin access identity
//! surface states for a delete:
//!
//! * a delete REMOVES the stored record, so it answers 204 with no body and every later
//!   read of that id answers `NoSuchCloudFrontOriginAccessIdentity` (404) while every other
//!   stored identity stays readable at its own version — and the caller reference the
//!   removed identity held is free again, so a create repeating it mints a new identity;
//! * the delete is a conditional request on the identity's current version ("The value of
//!   the ETag header you received from a previous GET or PUT request"): an `If-Match` that
//!   is missing or is not a version token at all is `InvalidIfMatchVersion` (400), one
//!   carrying a well-formed token that is not the current one is `PreconditionFailed`
//!   (412), and a refused delete changes nothing;
//! * an id no identity holds is `NoSuchCloudFrontOriginAccessIdentity` (404), with the
//!   sentence CloudFront answers that code with and nothing from the request;
//! * an identity a stored Amazon S3 origin still names is in use —
//!   `CloudFrontOriginAccessIdentityInUse` (409) — and stays readable, and the same delete
//!   lands once no origin names it. "To delete the origin access identity from an existing
//!   distribution, update the distribution configuration and include an empty
//!   `OriginAccessIdentity` element."
//!
//! The table rows differ along everything these rules generalise over: the configuration
//! deleted (every row of the lifecycle suite's table — each caller-reference form and each
//! comment length and character set), how many identities the account holds around the one
//! being deleted, the FORM the version token is presented in (absent, bare, quoted, weak,
//! malformed, stale), and WHERE the reference to the identity sits (each position in a
//! distribution's origin list, and a streaming distribution's single S3 origin).
//!
//! The harness (the router, the request helpers and the identity table) is the one the
//! origin access identity lifecycle and distribution lifecycle suites use, reused rather
//! than copied.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, update_distribution, Answer, API_VERSION,
};
use crate::origin_access_identity_lifecycle::{
    assert_config, assert_identity, config_table, create_identity, get_identity,
    get_identity_config, is_cloudfront_token, Config,
};

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/origin-access-identity/cloudfront/{Id}`, with `If-Match` carried
/// exactly as `if_match` gives it (and left off entirely when it is `None`).
async fn delete_identity(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront/{id}"),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/origin-access-identity/cloudfront` — the whole listing.
async fn list_identities(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
        None,
        None,
    )
    .await
}

/// The ids the listing reports, in the order it carries them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("CloudFrontOriginAccessIdentitySummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create one identity through the API and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, config: &Config) -> (String, String) {
    let created = create_identity(router, &config.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateCloudFrontOriginAccessIdentity, got {} with {}",
        config.case, created.status, created.raw
    );
    let (id, _) = assert_identity(&created, config, "the create");
    let etag = created.header("etag").to_string();
    assert!(
        is_cloudfront_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        config.case
    );
    (id, etag)
}

// ---------------------------------------------------------------------------
// A delete removes the record every read answered from
// ---------------------------------------------------------------------------

/// Deleting each stored identity in turn removes exactly that identity: it answers 204 with
/// no body, its id answers 404 on both reads afterwards, and every identity still stored
/// answers its own record at the version it was created with.
#[tokio::test]
async fn a_delete_removes_exactly_the_identity_it_names() {
    let router = emulator();

    // The whole table is stored first, so every delete below runs against an account that
    // holds other identities too — the delete has to leave each of them alone.
    let mut stored: Vec<(Config, String, String)> = Vec::new();
    for config in config_table() {
        let (id, etag) = create(&router, &config).await;
        stored.push((config, id, etag));
    }

    while let Some((config, id, etag)) = stored.pop() {
        let case = config.case;
        let deleted = delete_identity(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from DeleteCloudFrontOriginAccessIdentity, got {} \
             with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no body on a successful delete, got {}",
            deleted.raw
        );

        // Gone from both reads it was answered from...
        for (what, answer) in [
            ("the get", get_identity(&router, &id).await),
            ("the config get", get_identity_config(&router, &id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} ({what}): expected HTTP 404 after the delete, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchCloudFrontOriginAccessIdentity",
                "case {case} ({what}): expected NoSuchCloudFrontOriginAccessIdentity after the \
                 delete, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
        }

        // ...and gone from the listing, which still reports every identity left.
        let listed = list_identities(&router).await;
        let ids = listed_ids(&listed);
        assert!(
            !ids.contains(&id),
            "case {case}: expected {id} to be gone from the listing, got {ids:?}"
        );
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some(stored.len().to_string().as_str()),
            "case {case}: expected {} identities left, got {}",
            stored.len(),
            listed.raw
        );

        // Each surviving identity still answers its own record, at its own version.
        for (other_config, other_id, other_etag) in &stored {
            assert!(
                ids.contains(other_id),
                "case {case}: expected {other_id} to survive the delete, got {ids:?}"
            );
            let other = get_identity(&router, other_id).await;
            assert_eq!(
                other.status, 200,
                "case {case}: expected {other_id} to stay readable, got {} with {}",
                other.status, other.raw
            );
            assert_eq!(
                other.header("etag"),
                *other_etag,
                "case {case}: expected {other_id} to stay at version {other_etag}, got {:?}",
                other.header("etag")
            );
            assert_identity(
                &other,
                other_config,
                "a surviving identity after another was deleted",
            );
        }
    }

    // The account that deleted every identity it had holds none: the listing reports no
    // items at all, as it does for an account that never created one.
    let listed = list_identities(&router).await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case all-deleted: expected an empty listing after deleting every identity, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case all-deleted: expected no Items element for an account holding no identities, \
         got {}",
        listed.raw
    );
}

/// The caller reference an identity held is released with it: the conflict a repeated
/// reference raises is about a STORED identity, so once the identity is deleted the very
/// same configuration mints a new one, under an identity of its own.
#[tokio::test]
async fn the_caller_reference_a_deleted_identity_held_is_free_again() {
    let router = emulator();

    for config in config_table() {
        let case = config.case;
        let (id, etag) = create(&router, &config).await;

        // While it is stored, the same reference with another configuration is the modelled
        // conflict.
        let changed = Config {
            comment: "a configuration that differs from the first".to_string(),
            ..config.clone()
        };
        let conflicted = create_identity(&router, &changed.xml()).await;
        assert_eq!(
            conflicted.status, 409,
            "case {case}: expected HTTP 409 while the identity is stored, got {} with {}",
            conflicted.status, conflicted.raw
        );

        let deleted = delete_identity(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from DeleteCloudFrontOriginAccessIdentity, got {} \
             with {}",
            deleted.status, deleted.raw
        );

        // Once it is gone, that reference names no identity, so the configuration that was
        // refused a moment ago is created — with a new id and a new version token.
        let (again_id, again_etag) = create(&router, &changed).await;
        assert_ne!(
            again_id, id,
            "case {case}: expected a new id for the re-created identity, got {again_id} again"
        );
        assert_ne!(
            again_etag, etag,
            "case {case}: expected a new version token for the re-created identity, got \
             {again_etag} again"
        );
        let fetched = get_identity_config(&router, &again_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the re-created identity to be readable, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_config(
            fetched.root(),
            &changed,
            "the config get after re-creating a deleted identity",
            &fetched.raw,
        );

        // Leave the account empty for the next row.
        let cleaned = delete_identity(&router, &again_id, Some(&again_etag)).await;
        assert_eq!(
            cleaned.status, 204,
            "case {case}: expected HTTP 204 when clearing the re-created identity, got {} \
             with {}",
            cleaned.status, cleaned.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The conditional request
// ---------------------------------------------------------------------------

/// How a row presents the version token to the delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// No `If-Match` header at all.
    Absent,
    /// The identity's current version, exactly as the `ETag` header reported it.
    Current,
    /// The current version in HTTP entity-tag quotes.
    QuotedCurrent,
    /// The current version as a weak entity tag.
    WeakCurrent,
    /// A literal that is not a version token at all.
    Malformed(&'static str),
    /// A well-formed version token that is another identity's — so not this one's.
    OtherIdentity,
}

/// One row of the precondition table.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    version: Version,
    /// The status the delete is expected to answer with.
    status: u16,
    /// The error code the delete is expected to answer with, or `None` when it lands.
    code: Option<&'static str>,
}

/// Every form the `If-Match` value takes, and what each makes of the delete.
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "no-if-match",
        version: Version::Absent,
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "empty-if-match",
        version: Version::Malformed(""),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-that-is-not-a-token",
        version: Version::Malformed("not-a-version"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-token-of-the-wrong-length",
        version: Version::Malformed("E123"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-naming-another-identity",
        version: Version::OtherIdentity,
        status: 412,
        code: Some("PreconditionFailed"),
    },
    Precondition {
        case: "if-match-current",
        version: Version::Current,
        status: 204,
        code: None,
    },
    Precondition {
        case: "if-match-current-quoted",
        version: Version::QuotedCurrent,
        status: 204,
        code: None,
    },
    Precondition {
        case: "if-match-current-weak",
        version: Version::WeakCurrent,
        status: 204,
        code: None,
    },
];

/// A delete lands only when the caller holds the identity's current version: every other
/// form of `If-Match` is the refusal the operation models for it, and leaves the identity
/// exactly as readable as it was.
#[tokio::test]
async fn a_delete_lands_only_on_the_current_version() {
    let rows = config_table();
    for precondition in PRECONDITIONS {
        let case = precondition.case;
        let router = emulator();
        // Two identities, so a well-formed token that is not the target's is available
        // without inventing one.
        let target = &rows[0];
        let other = &rows[1];
        let (id, etag) = create(&router, target).await;
        let (other_id, other_etag) = create(&router, other).await;

        let quoted = format!("\"{etag}\"");
        let weak = format!("W/\"{etag}\"");
        let if_match: Option<&str> = match precondition.version {
            Version::Absent => None,
            Version::Current => Some(&etag),
            Version::QuotedCurrent => Some(&quoted),
            Version::WeakCurrent => Some(&weak),
            Version::Malformed(literal) => Some(literal),
            Version::OtherIdentity => Some(&other_etag),
        };

        let deleted = delete_identity(&router, &id, if_match).await;
        assert_eq!(
            deleted.status, precondition.status,
            "case {case}: expected HTTP {} from DeleteCloudFrontOriginAccessIdentity, got {} \
             with {}",
            precondition.status, deleted.status, deleted.raw
        );
        match precondition.code {
            None => {
                let fetched = get_identity(&router, &id).await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case}: expected the identity to be gone, got {} with {}",
                    fetched.status, fetched.raw
                );
            }
            Some(code) => {
                assert_eq!(
                    deleted.error_code(),
                    code,
                    "case {case}: expected {code}, got {:?} with {}",
                    deleted.error_code(),
                    deleted.raw
                );
                // A refused delete destroys nothing: the identity is still readable, at the
                // same version and with the configuration it held.
                let after = get_identity(&router, &id).await;
                assert_eq!(
                    after.status, 200,
                    "case {case}: expected the refused delete to leave the identity readable, \
                     got {} with {}",
                    after.status, after.raw
                );
                assert_eq!(
                    after.header("etag"),
                    etag,
                    "case {case}: expected the refused delete to leave the version at {etag}, \
                     got {:?}",
                    after.header("etag")
                );
                assert_identity(&after, target, "the get after a refused delete");
            }
        }

        // The identity the row did not target is untouched either way.
        let untouched = get_identity(&router, &other_id).await;
        assert_eq!(
            untouched.status, 200,
            "case {case}: expected the other identity to stay readable, got {} with {}",
            untouched.status, untouched.raw
        );
        assert_eq!(
            untouched.header("etag"),
            other_etag,
            "case {case}: expected the other identity to stay at version {other_etag}, got {:?}",
            untouched.header("etag")
        );
    }
}

/// An id no identity holds is `NoSuchCloudFrontOriginAccessIdentity`, whatever the request
/// says about the version — and nothing the account holds is touched.
#[tokio::test]
async fn an_id_no_identity_holds_is_refused() {
    let router = emulator();
    let rows = config_table();
    let (id, etag) = create(&router, &rows[0]).await;

    // Ids no identity holds: one of the shape this service mints, the stored id in another
    // case, a prefix of it, and a value of no identity shape at all.
    let absent: [(&str, String); 4] = [
        ("minted-shape", "E1A2B3C4D5E6F7".to_string()),
        ("another-case", id.to_lowercase()),
        ("a-prefix", id[..6].to_string()),
        ("not-an-id", "no-such-identity".to_string()),
    ];
    for (case, missing) in absent {
        for (version_case, if_match) in [
            ("no-if-match", None),
            ("current-of-another", Some(etag.as_str())),
        ] {
            let answer = delete_identity(&router, &missing, if_match).await;
            assert_eq!(
                answer.status, 404,
                "case {case}/{version_case}: expected HTTP 404 for an id no identity holds, \
                 got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchCloudFrontOriginAccessIdentity",
                "case {case}/{version_case}: expected NoSuchCloudFrontOriginAccessIdentity, \
                 got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            // The refusal is about the STORE, so it carries the fixed sentence and nothing
            // out of the request. The sentence is spelled out here as well as compared to
            // the exported constant, so a changed constant fails this suite.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, "The specified CloudFront origin access identity does not exist.",
                "case {case}/{version_case}: expected the sentence CloudFront answers this \
                 code with, got {message:?}"
            );
            assert_eq!(
                message, NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE,
                "case {case}/{version_case}: expected the modelled sentence, got {message:?}"
            );
            assert!(
                !message.contains(&missing),
                "case {case}/{version_case}: expected nothing from the request in the message, \
                 got {message:?}"
            );
        }
    }

    // The identity that does exist is untouched.
    let listed = list_identities(&router).await;
    assert_eq!(
        listed_ids(&listed),
        vec![id.clone()],
        "case missing-ids: expected the stored identity to be untouched, got {}",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// An identity an Amazon S3 origin still names
// ---------------------------------------------------------------------------

/// The value a configuration carries to attach the identity `id` to an Amazon S3 origin.
///
/// `S3OriginConfig$OriginAccessIdentity` documents the form exactly — "The format of the
/// value is: `origin-access-identity/cloudfront/ID-of-origin-access-identity`" — and
/// `S3Origin$OriginAccessIdentity` names the same identity the same way. Spelled out here
/// rather than taken from the crate, so a changed reference form fails this suite.
fn reference(id: &str) -> String {
    format!("origin-access-identity/cloudfront/{id}")
}

/// The `DistributionConfig` a client sends for a distribution whose origins are `origins`
/// — each `(Id, OriginAccessIdentity)`, with `None` for an origin that names no identity.
///
/// Built here rather than taken from the distribution harness's builder because the member
/// under test (`S3OriginConfig$OriginAccessIdentity`) is not one that builder carries.
/// Members follow the model's declaration order for `Origin`.
fn distribution_xml(caller_reference: &str, origins: &[(&str, Option<String>)]) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>{}</Quantity><Items>",
        origins.len()
    );
    for (origin_id, identity) in origins {
        out.push_str(&format!(
            "<Origin><Id>{origin_id}</Id>\
             <DomainName>{origin_id}.s3.us-east-1.amazonaws.com</DomainName>\
             <S3OriginConfig><OriginAccessIdentity>{}</OriginAccessIdentity>\
             </S3OriginConfig></Origin>",
            // "If you want viewers to be able to access objects using either the CloudFront
            // URL or the Amazon S3 URL, specify an empty OriginAccessIdentity element."
            identity.clone().unwrap_or_default()
        ));
    }
    out.push_str(&format!(
        "</Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>{}</TargetOriginId>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>a distribution naming an origin access identity</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled></DistributionConfig>",
        origins.first().map(|(id, _)| *id).unwrap_or_default()
    ));
    out
}

/// The `StreamingDistributionConfig` a client sends for a streaming distribution whose
/// single Amazon S3 origin names `identity`. Members follow the model's declaration order.
fn streaming_distribution_xml(caller_reference: &str, identity: &str) -> String {
    format!(
        "<StreamingDistributionConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <S3Origin><DomainName>media.s3.us-east-1.amazonaws.com</DomainName>\
         <OriginAccessIdentity>{identity}</OriginAccessIdentity></S3Origin>\
         <Comment>a streaming distribution naming an origin access identity</Comment>\
         <TrustedSigners><Enabled>false</Enabled><Quantity>0</Quantity></TrustedSigners>\
         <Enabled>true</Enabled></StreamingDistributionConfig>"
    )
}

/// Where the reference to the identity under test sits in the configuration that names it.
struct Attachment {
    case: &'static str,
    /// For a distribution: one entry per origin, `true` for the origins that name the
    /// identity under test. Empty for the streaming-distribution row, which has exactly
    /// one Amazon S3 origin.
    attached: Vec<bool>,
    /// Whether the configuration naming the identity is a streaming distribution's.
    streaming: bool,
}

/// An identity a stored Amazon S3 origin still names cannot be deleted: it is in use, so
/// it stays readable — and the delete lands once the documented remedy has been applied
/// ("update the distribution configuration and include an empty `OriginAccessIdentity`
/// element").
#[tokio::test]
async fn an_identity_an_s3_origin_still_names_cannot_be_deleted() {
    let attachments = vec![
        Attachment {
            case: "the only origin of a distribution",
            attached: vec![true],
            streaming: false,
        },
        Attachment {
            case: "the first of three origins",
            attached: vec![true, false, false],
            streaming: false,
        },
        Attachment {
            case: "the last of three origins",
            attached: vec![false, false, true],
            streaming: false,
        },
        Attachment {
            case: "two of three origins",
            attached: vec![true, false, true],
            streaming: false,
        },
        Attachment {
            case: "a streaming distribution's S3 origin",
            attached: Vec::new(),
            streaming: true,
        },
    ];

    let rows = config_table();
    for attachment in &attachments {
        let case = attachment.case;
        let router = emulator();
        // Two identities: the one the configuration names, and one it does not — which
        // stays deletable throughout, so the refusal is about the reference and not about
        // there being a distribution at all.
        let (id, etag) = create(&router, &rows[0]).await;
        let (spare_id, spare_etag) = create(&router, &rows[1]).await;

        let caller_reference = format!("ref-{}", case.replace(' ', "-"));
        let mut detach: Option<(String, String, Vec<String>)> = None;
        // The id of the distribution that names the identity, whichever family it belongs
        // to: the refusal reports it, and it is the one detail a caller cannot work out.
        let holder_id;
        if attachment.streaming {
            let created = call(
                &router,
                "POST",
                &format!("/{API_VERSION}/streaming-distribution"),
                Some(&streaming_distribution_xml(&caller_reference, &reference(&id))),
                None,
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected HTTP 201 from CreateStreamingDistribution, got {} with {}",
                created.status, created.raw
            );
            holder_id = created.root().text_at("Id").unwrap_or_default().to_string();
        } else {
            let origin_ids: Vec<String> = (0..attachment.attached.len())
                .map(|index| format!("origin-{index}"))
                .collect();
            let attached: Vec<(&str, Option<String>)> = origin_ids
                .iter()
                .zip(&attachment.attached)
                .map(|(origin_id, holds)| {
                    (
                        origin_id.as_str(),
                        if *holds { Some(reference(&id)) } else { None },
                    )
                })
                .collect();
            let created =
                create_distribution(&router, &distribution_xml(&caller_reference, &attached)).await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
                created.status, created.raw
            );
            let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
            let distribution_etag = created.header("etag").to_string();
            holder_id = distribution_id.clone();
            detach = Some((distribution_id, distribution_etag, origin_ids));
        }
        assert!(
            !holder_id.is_empty(),
            "case {case}: the create answered no Id for the distribution naming the identity"
        );

        let refused = delete_identity(&router, &id, Some(&etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 for an identity an S3 origin still names, got {} \
             with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "CloudFrontOriginAccessIdentityInUse",
            "case {case}: expected CloudFrontOriginAccessIdentityInUse, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        // The refusal states the condition, and names neither the distribution holding the
        // identity nor anything else out of the store.
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, "The CloudFront origin access identity is still being used.",
            "case {case}: expected the sentence CloudFront answers this code with, got \
             {message:?}"
        );
        assert!(
            !message.contains(&holder_id),
            "case {case}: expected the refusal to name no distribution, got {message:?}"
        );
        // And it destroys nothing: the identity is still readable at the same version.
        let after = get_identity(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected the refused delete to leave the identity readable, got {} \
             with {}",
            after.status, after.raw
        );
        assert_eq!(
            after.header("etag"),
            etag,
            "case {case}: expected the refused delete to leave the version at {etag}, got {:?}",
            after.header("etag")
        );

        // An identity NO origin names is deletable while that same distribution stands.
        let spare = delete_identity(&router, &spare_id, Some(&spare_etag)).await;
        assert_eq!(
            spare.status, 204,
            "case {case}: expected the unreferenced identity to be deletable, got {} with {}",
            spare.status, spare.raw
        );

        // The documented remedy, where the configuration can be updated: include an empty
        // `OriginAccessIdentity` element on every origin, then delete the identity.
        if let Some((distribution_id, distribution_etag, origin_ids)) = detach {
            let detached: Vec<(&str, Option<String>)> = origin_ids
                .iter()
                .map(|origin_id| (origin_id.as_str(), None))
                .collect();
            let updated = update_distribution(
                &router,
                &distribution_id,
                &distribution_xml(&caller_reference, &detached),
                Some(&distribution_etag),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
                updated.status, updated.raw
            );

            let deleted = delete_identity(&router, &id, Some(&etag)).await;
            assert_eq!(
                deleted.status, 204,
                "case {case}: expected the delete to land once no origin names the identity, \
                 got {} with {}",
                deleted.status, deleted.raw
            );
            let gone = get_identity(&router, &id).await;
            assert_eq!(
                gone.status, 404,
                "case {case}: expected the identity to be gone after the delete, got {} with {}",
                gone.status, gone.raw
            );
        }
    }
}

/// An empty `OriginAccessIdentity` element names NO identity — "If you want viewers to be
/// able to access objects using either the CloudFront URL or the Amazon S3 URL, specify an
/// empty `OriginAccessIdentity` element" — so a distribution whose origins all carry one
/// holds no identity in use, and every stored identity is deletable.
#[tokio::test]
async fn an_empty_origin_access_identity_element_holds_no_identity() {
    let router = emulator();
    let rows = config_table();
    let mut stored: Vec<(Config, String, String)> = Vec::new();
    for config in rows.iter().take(3) {
        let (id, etag) = create(&router, config).await;
        stored.push((config.clone(), id, etag));
    }

    let origins: Vec<(&str, Option<String>)> = vec![
        ("origin-0", None),
        ("origin-1", Some(String::new())),
        ("origin-2", None),
    ];
    let created = create_distribution(&router, &distribution_xml("empty-oai", &origins)).await;
    assert_eq!(
        created.status, 201,
        "case empty-element: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );

    for (config, id, etag) in &stored {
        let deleted = delete_identity(&router, id, Some(etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {}: expected HTTP 204 while only empty OriginAccessIdentity elements are \
             stored, got {} with {}",
            config.case, deleted.status, deleted.raw
        );
    }
}
