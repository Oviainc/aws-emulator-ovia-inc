// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a create refused on its `CallerReference` carries, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test, over the resource kinds whose create is refused when the
//! `CallerReference` it submits already names a resource of that kind:
//!
//! * the conflict is HTTP 409 under the error code the model binds for that resource kind, and
//!   its message is the FIXED sentence the service states for that code — it names the kind of
//!   resource the reference is already associated with and nothing else;
//! * so the sentence carries neither the reference submitted nor the identifier of the
//!   resource that holds it: two callers whose only difference is the reference they chose
//!   read the same sentence, and the same caller reading it twice reads the same one;
//! * and the refusal changes nothing — the resource that holds the reference is still readable
//!   at the identity its own create minted.
//!
//! The rows span what the rule generalises over: the resource KIND (four of them, four error
//! codes between them), and for each kind the values a `CallerReference` may carry. The member
//! is an unconstrained string ("A unique value (for example, a date-time stamp)",
//! `smithy_query(query="shape",shape="CloudFrontOriginAccessIdentityConfig,PublicKeyConfig,
//! FieldLevelEncryptionConfig,FieldLevelEncryptionProfileConfig")`), so the rows span the forms
//! a caller actually sends: the shortest, a date-time stamp, a 64-character value, one
//! carrying whitespace and XML-significant characters, and a non-ASCII one.
//!
//! Each row's repeat carries a configuration that DIFFERS from the stored one (the kinds whose
//! create replays an identical request answer that replay with the stored resource instead),
//! and a different name where the kind has one, so the repeat reaches the reference rather than
//! the name.
//!
//! The expected sentences are LITERALS in the table below rather than the crate's own
//! constants, so a wrong constant cannot make this suite pass.
//!
//! The DISTRIBUTION pair is not under test here: the sentence CloudFront serves for a
//! distribution's own caller-reference conflict names the distribution holding the reference,
//! and `crate::error_envelope` and `crate::streaming_distribution_list` pin those two.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, Answer};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, simple as simple_config,
};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, store_profile, upload_key, Entity, Profile,
};
use crate::origin_access_identity_lifecycle::{
    create_identity, get_identity, Config as IdentityConfig,
};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

// ---------------------------------------------------------------------------
// The kinds under test
// ---------------------------------------------------------------------------

/// One resource kind whose create is refused when the `CallerReference` it submits already
/// names a resource of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    OriginAccessIdentity,
    FieldLevelEncryptionConfig,
    FieldLevelEncryptionProfile,
    PublicKey,
}

/// Every kind under test, with the error code the model binds for its conflict and the
/// sentence the service states for that code.
const KINDS: &[(Kind, &str, &str)] = &[
    (
        Kind::OriginAccessIdentity,
        "CloudFrontOriginAccessIdentityAlreadyExists",
        "The caller reference you are using to create the CloudFront origin access identity \
         is associated with another identity.",
    ),
    (
        Kind::FieldLevelEncryptionConfig,
        "FieldLevelEncryptionConfigAlreadyExists",
        "The caller reference that you are using to create a field level encryption config \
         is associated with another field level encryption config in your account.",
    ),
    (
        Kind::FieldLevelEncryptionProfile,
        "FieldLevelEncryptionProfileAlreadyExists",
        "The caller reference that you are using to create a field level encryption profile \
         is associated with another field level encryption profile in your account.",
    ),
    (
        Kind::PublicKey,
        "PublicKeyAlreadyExists",
        "The caller reference that you are using to create a CloudFront public key is \
         associated with another public key in your account.",
    ),
];

/// The references every kind is driven over: what this row is a case of, and the value.
///
/// A `CallerReference` carries no modelled constraint on any of these configurations, so the
/// rows span the forms a caller sends rather than the ends of a published range. None of them
/// is a substring of any sentence in [`KINDS`], so the "carries nothing from the request"
/// assertion below is about the value and not about an accident of wording.
fn references() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest reference", "r4".to_string()),
        (
            "a date-time stamp, as the model's example has it",
            "2026-01-02T03:04:05.678Z".to_string(),
        ),
        ("a 64-character reference", "n".repeat(64)),
        (
            "a reference carrying whitespace and XML-significant characters",
            "ref \"one\" & <two> 2026".to_string(),
        ),
        ("a non-ASCII reference", "참조-77".to_string()),
    ]
}

// ---------------------------------------------------------------------------
// The requests each kind is driven with
// ---------------------------------------------------------------------------

/// The resources every row's requests point at: a stored field-level encryption profile for the
/// configuration rows' mapping, and an uploaded public key for the profile rows' encryption
/// entity. Both are created through their own surface, so no row names an identifier made up
/// here.
struct Support {
    profile_id: String,
    public_key_id: String,
}

/// Stand up what [`Support`] holds over the one router under test.
async fn support(router: &MultiServiceRouter) -> Support {
    Support {
        profile_id: store_profile(router, "caller-reference-wording-support").await,
        // Index 1: `store_profile` above uploads the key at index 0, and each index carries its
        // own caller reference.
        public_key_id: upload_key(router, 1).await,
    }
}

/// The two request documents one row sends: the create that is accepted, and the repeat under
/// the SAME reference that must be refused on it.
struct Documents {
    first: String,
    repeat: String,
}

/// Build them for `kind`, under `reference`, with `label` making every name this row submits
/// its own.
fn documents(kind: Kind, reference: &str, label: &str, support: &Support) -> Documents {
    match kind {
        Kind::OriginAccessIdentity => {
            let first = IdentityConfig {
                case: "a caller reference another identity holds",
                caller_reference: reference.to_string(),
                comment: format!("the identity that holds {label}"),
            };
            let repeat = IdentityConfig {
                comment: "a configuration that differs from the stored one".to_string(),
                ..first.clone()
            };
            Documents {
                first: first.xml(),
                repeat: repeat.xml(),
            }
        }
        Kind::FieldLevelEncryptionConfig => {
            let first = simple_config(label, reference, &support.profile_id);
            let mut repeat = first.clone();
            repeat.comment = Some("a configuration that differs from the stored one".to_string());
            Documents {
                first: first.xml(),
                repeat: repeat.xml(),
            }
        }
        Kind::FieldLevelEncryptionProfile => {
            let mut first = Profile::new(
                label,
                reference,
                None,
                vec![Entity::new(
                    &support.public_key_id,
                    "provider-one",
                    &["CreditCard"],
                )],
            );
            first.name = format!("profile-{label}");
            let mut repeat = first.clone();
            // A profile is refused on its NAME first, so the repeat carries another name: what
            // it collides on is the reference alone.
            repeat.name = format!("profile-{label}-renamed");
            repeat.comment = Some("a configuration that differs from the stored one".to_string());
            Documents {
                first: first.xml(),
                repeat: repeat.xml(),
            }
        }
        Kind::PublicKey => {
            let first = Key {
                case: label.to_string(),
                caller_reference: reference.to_string(),
                name: format!("key-{label}"),
                encoded_key: pem(2, 'B'),
                comment: None,
            };
            let repeat = Key {
                // A public key is refused on its NAME first, for the reason a profile is.
                name: format!("key-{label}-renamed"),
                comment: Some("a configuration that differs from the stored one".to_string()),
                ..first.clone()
            };
            Documents {
                first: first.xml(),
                repeat: repeat.xml(),
            }
        }
    }
}

/// Create one resource of `kind`, through the route the model binds that kind's create to.
async fn create(router: &MultiServiceRouter, kind: Kind, body: &str) -> Answer {
    match kind {
        Kind::OriginAccessIdentity => create_identity(router, body).await,
        Kind::FieldLevelEncryptionConfig => create_config(router, body).await,
        Kind::FieldLevelEncryptionProfile => create_profile(router, body).await,
        Kind::PublicKey => create_key(router, body).await,
    }
}

/// Read the resource `id` names back, through the reader the model binds for that kind.
async fn read_back(router: &MultiServiceRouter, kind: Kind, id: &str) -> Answer {
    match kind {
        Kind::OriginAccessIdentity => get_identity(router, id).await,
        Kind::FieldLevelEncryptionConfig => get_config(router, id).await,
        Kind::FieldLevelEncryptionProfile => get_profile(router, id).await,
        Kind::PublicKey => get_key(router, id).await,
    }
}

/// The message one answer carries, or `""`.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// A create under a `CallerReference` another resource of the same kind already holds is
/// refused 409 under that kind's code, with the service's FIXED sentence for it: the kind of
/// resource the reference is associated with, carrying neither the reference submitted nor the
/// identifier of the resource that holds it — so the sentence is the same whichever reference
/// was chosen — and the resource that holds the reference survives the refusal.
#[tokio::test]
async fn a_create_under_a_taken_caller_reference_carries_the_services_fixed_sentence() {
    let router = emulator();
    let support = support(&router).await;

    for (kind, code, expected) in KINDS {
        for (index, (case, reference)) in references().into_iter().enumerate() {
            let case = format!("{kind:?}/{case}");
            let label = format!("cr-wording-{index}-{}", kind_slug(*kind));
            let documents = documents(*kind, &reference, &label, &support);

            let first = create(&router, *kind, &documents.first).await;
            assert_eq!(
                first.status, 201,
                "case {case}: expected HTTP 201 from the first create, got {} with {}",
                first.status, first.raw
            );
            let id = first.root().find_text("Id").unwrap_or_default().to_string();
            assert!(
                !id.is_empty(),
                "case {case}: expected the create to mint an id, got {}",
                first.raw
            );

            let refused = create(&router, *kind, &documents.repeat).await;
            assert_eq!(
                refused.status, 409,
                "case {case}: expected HTTP 409 under a taken caller reference, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                *code,
                "case {case}: expected {code}, got {:?} with {}",
                refused.error_code(),
                refused.raw
            );
            let actual = message(&refused);
            assert_eq!(
                actual, *expected,
                "case {case}: expected the service's fixed sentence {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&id),
                "case {case}: the sentence must not name the resource holding the reference \
                 ({id}), got {actual:?}"
            );
            assert!(
                !actual.contains(reference.as_str()),
                "case {case}: the sentence must not echo the reference submitted ({reference}), \
                 got {actual:?}"
            );

            // The refusal changed nothing: the resource that holds the reference is still
            // readable at the identity its own create minted.
            let read = read_back(&router, *kind, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the stored resource to be readable after the refusal, \
                 got {} with {}",
                read.status, read.raw
            );
            assert_eq!(
                read.root().find_text("Id"),
                Some(id.as_str()),
                "case {case}: expected the read to report {id:?}, got {:?}",
                read.root().find_text("Id")
            );
        }
    }
}

/// Two callers whose only difference is the reference they chose read the SAME sentence, and
/// one caller reading it twice reads the same one: the message is a property of the error class
/// rather than of the request or of the store.
#[tokio::test]
async fn the_sentence_is_the_same_whichever_reference_collided() {
    let router = emulator();
    let support = support(&router).await;

    for (kind, _, _) in KINDS {
        let mut seen: Vec<(String, String)> = Vec::new();
        for (index, (case, reference)) in references().into_iter().enumerate() {
            let case = format!("{kind:?}/{case}");
            let label = format!("cr-repeat-{index}-{}", kind_slug(*kind));
            let documents = documents(*kind, &reference, &label, &support);

            let first = create(&router, *kind, &documents.first).await;
            assert_eq!(
                first.status, 201,
                "case {case}: expected HTTP 201 from the first create, got {} with {}",
                first.status, first.raw
            );

            let refused = create(&router, *kind, &documents.repeat).await;
            let once = message(&refused).to_string();
            // The same refused request again: a refusal stores nothing, so the second one
            // reports exactly what the first did.
            let again = create(&router, *kind, &documents.repeat).await;
            assert_eq!(
                message(&again),
                once.as_str(),
                "case {case}: expected the repeated refusal to read the same, got {:?} after \
                 {once:?}",
                message(&again)
            );
            seen.push((case, once));
        }
        let (first_case, first_message) = seen[0].clone();
        for (case, actual) in &seen {
            assert_eq!(
                actual, &first_message,
                "case {case}: expected the sentence {first_case} read ({first_message:?}), got \
                 {actual:?}"
            );
        }
    }
}

/// A short name for `kind`, so every row's submitted names are its own.
fn kind_slug(kind: Kind) -> &'static str {
    match kind {
        Kind::OriginAccessIdentity => "identity",
        Kind::FieldLevelEncryptionConfig => "config",
        Kind::FieldLevelEncryptionProfile => "profile",
        Kind::PublicKey => "key",
    }
}
