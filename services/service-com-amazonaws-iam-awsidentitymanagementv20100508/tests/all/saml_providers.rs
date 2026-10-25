// SPDX-License-Identifier: CC0-1.0

//! `CreateSAMLProvider` and its reporting half `GetSAMLProvider`, end to end over the
//! awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a provider that was
//! PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rules under test are that a create PERSISTS the provider under an ARN DERIVED from
//! its `Name` and the read REPORTS what was persisted — the metadata document byte for
//! byte, the instant it was created, the expiration READ OUT of that document
//! (`validUntil`), the encryption setting the request chose, the metadata of a private key
//! it uploaded, and its tags sorted by key — while a document that describes no SAML 2.0
//! identity provider, and a second create for a name the account already holds, are both
//! refused with nothing persisted. Each suite is a TABLE whose rows differ along what
//! those rules generalise over (how the metadata document spells its namespace and nests
//! its entity, whether it states an expiration and in which form, the provider name's
//! alphabet, the encryption mode, whether a private key is uploaded, the tag count and the
//! order the tags arrive in), so no single request is the specification.

use std::collections::BTreeMap;

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// How a metadata document spells the SAML 2.0 metadata it carries. The SAME document
/// content in three equally valid XML spellings (OASIS "Metadata for the OASIS Security
/// Assertion Markup Language (SAML) V2.0", §2.3), so the rule cannot be keyed to the text
/// of one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataForm {
    /// Root `EntityDescriptor`, the metadata namespace declared as the default.
    Entity,
    /// Root `EntityDescriptor`, the metadata namespace bound to the conventional `md`
    /// prefix instead.
    PrefixedEntity,
    /// Root `EntitiesDescriptor` with the entity descriptor nested inside it — the group
    /// form, which carries the expiration on the group.
    EntityGroup,
}

/// The namespace SAML 2.0 metadata descriptors are declared in.
const METADATA_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:metadata";

/// A base64 body long enough that the document passes `SAMLMetadataDocumentType`'s
/// 1000-character minimum (`smithy_query(query="operation",
/// operation="CreateSAMLProvider")`) — which is where a real document's bulk is too: the
/// IdP's signing certificate.
fn certificate_body() -> String {
    format!("MIIC{}", "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVph".repeat(30))
}

/// A SAML 2.0 metadata document naming `entity_id` as its issuer, stating `valid_until`
/// as its expiration when one is given, spelled in the requested `form`.
pub(crate) fn metadata_document(
    form: MetadataForm,
    entity_id: &str,
    valid_until: Option<&str>,
) -> String {
    let certificate = certificate_body();
    let expiry = valid_until
        .map(|until| format!(" validUntil=\"{until}\""))
        .unwrap_or_default();
    match form {
        MetadataForm::Entity => format!(
            "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"{entity_id}\"{expiry}>\
             <IDPSSODescriptor protocolSupportEnumeration=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
             <KeyDescriptor use=\"signing\"><KeyInfo \
             xmlns=\"http://www.w3.org/2000/09/xmldsig#\"><X509Data>\
             <X509Certificate>{certificate}</X509Certificate></X509Data></KeyInfo>\
             </KeyDescriptor><SingleSignOnService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" \
             Location=\"https://idp.example.com/sso\"/></IDPSSODescriptor></EntityDescriptor>"
        ),
        MetadataForm::PrefixedEntity => format!(
            "<md:EntityDescriptor xmlns:md=\"{METADATA_NAMESPACE}\" \
             entityID=\"{entity_id}\"{expiry}><md:IDPSSODescriptor \
             protocolSupportEnumeration=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
             <md:KeyDescriptor use=\"signing\"><KeyInfo \
             xmlns=\"http://www.w3.org/2000/09/xmldsig#\"><X509Data>\
             <X509Certificate>{certificate}</X509Certificate></X509Data></KeyInfo>\
             </md:KeyDescriptor><md:SingleSignOnService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" \
             Location=\"https://idp.example.com/sso\"/></md:IDPSSODescriptor>\
             </md:EntityDescriptor>"
        ),
        MetadataForm::EntityGroup => format!(
            "<EntitiesDescriptor xmlns=\"{METADATA_NAMESPACE}\" Name=\"example-federation\"\
             {expiry}><EntityDescriptor entityID=\"{entity_id}\"><IDPSSODescriptor \
             protocolSupportEnumeration=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
             <KeyDescriptor use=\"signing\"><KeyInfo \
             xmlns=\"http://www.w3.org/2000/09/xmldsig#\"><X509Data>\
             <X509Certificate>{certificate}</X509Certificate></X509Data></KeyInfo>\
             </KeyDescriptor><SingleSignOnService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" \
             Location=\"https://idp.example.com/sso\"/></IDPSSODescriptor></EntityDescriptor>\
             </EntitiesDescriptor>"
        ),
    }
}

/// One `CreateSAMLProvider` request, with the members the row exercises.
struct ProviderCase {
    /// `Name` — "a string of characters consisting of upper and lowercase alphanumeric
    /// characters with no spaces", pattern `^[\w._-]+$` (`CreateSAMLProvider$Name`).
    name: &'static str,
    /// How the row's metadata document is spelled.
    form: MetadataForm,
    /// The issuer the document names in `entityID`.
    entity_id: &'static str,
    /// The document's `validUntil`, when it states one.
    valid_until: Option<&'static str>,
    /// The instant that `validUntil` names, written in the `Z` form: the value
    /// `GetSAMLProvider$ValidUntil` must report. Spelled out per row rather than computed,
    /// so a row carrying a UTC offset pins the instant independently of the code that
    /// resolves the offset.
    expected_valid_until: Option<&'static str>,
    /// `AssertionEncryptionMode` — one value of `assertionEncryptionModeType` (`Allowed`
    /// or `Required`, `smithy_query(query="shape",
    /// shape="assertionEncryptionModeType")`), or none at all.
    encryption_mode: Option<&'static str>,
    /// Whether the request uploads a private key in `AddPrivateKey`.
    private_key: bool,
    /// The tags the request carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl ProviderCase {
    fn document(&self) -> String {
        metadata_document(self.form, self.entity_id, self.valid_until)
    }

    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            param("Name", self.name),
            param("SAMLMetadataDocument", &self.document()),
        ];
        if let Some(mode) = self.encryption_mode {
            params.push(param("AssertionEncryptionMode", mode));
        }
        if self.private_key {
            // "The private key must be a .pem file" (`CreateSAMLProvider$AddPrivateKey`),
            // whose pattern admits this ASCII body.
            params.push(param(
                "AddPrivateKey",
                &format!(
                    "-----BEGIN PRIVATE KEY----- {} -----END PRIVATE KEY-----",
                    certificate_body()
                ),
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn sorted_tag_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.tags.iter().map(|(key, _)| (*key).to_string()).collect();
        keys.sort();
        keys
    }
}

/// The table every suite here is written against: names spanning the whole `^[\w._-]+$`
/// alphabet, each of the three document spellings, expirations stated in the `Z` form,
/// with fractional seconds, with a numeric offset, with no zone at all and not at all,
/// both encryption modes and none, a private key uploaded and not, and tag counts from
/// none to several arriving in an order that is not the sorted one.
fn provider_cases() -> Vec<ProviderCase> {
    vec![
        ProviderCase {
            name: "ExampleIdP",
            form: MetadataForm::Entity,
            entity_id: "https://idp.example.com/saml",
            valid_until: Some("2032-05-30T09:01:31Z"),
            expected_valid_until: Some("2032-05-30T09:01:31Z"),
            encryption_mode: None,
            private_key: false,
            tags: Vec::new(),
        },
        ProviderCase {
            name: "corp_idp-2.eu",
            form: MetadataForm::PrefixedEntity,
            entity_id: "urn:example:idp:corp",
            valid_until: None,
            expected_valid_until: None,
            encryption_mode: Some("Allowed"),
            private_key: false,
            tags: vec![("env", "dev")],
        },
        ProviderCase {
            name: "Federation.Partner_7",
            form: MetadataForm::EntityGroup,
            entity_id: "https://partner.example.org/metadata",
            valid_until: Some("2030-01-02T03:04:05.678Z"),
            expected_valid_until: Some("2030-01-02T03:04:05.678Z"),
            encryption_mode: Some("Required"),
            private_key: true,
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        ProviderCase {
            name: "a",
            form: MetadataForm::Entity,
            entity_id: "https://single.example.net/idp",
            // Two hours EAST of UTC, so the instant is two hours EARLIER in the `Z` form.
            valid_until: Some("2035-12-31T23:59:59+02:00"),
            expected_valid_until: Some("2035-12-31T21:59:59Z"),
            encryption_mode: None,
            private_key: true,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        ProviderCase {
            name: "LONG-NAME_WITH.EVERY-ALLOWED_CHARACTER.0123456789",
            form: MetadataForm::EntityGroup,
            entity_id: "urn:example:federation:group",
            valid_until: None,
            expected_valid_until: None,
            encryption_mode: Some("Required"),
            private_key: false,
            tags: vec![("only", "one")],
        },
        ProviderCase {
            // An `xs:dateTime` stating NO zone is local time of an undetermined zone (XML
            // Schema Part 2 §3.2.7), a form a well-formed document may use, so it names an
            // expiration rather than being refused.
            name: "ZoneLessExpiry",
            form: MetadataForm::PrefixedEntity,
            entity_id: "https://zoneless.example.com/saml",
            valid_until: Some("2031-07-04T12:00:00"),
            expected_valid_until: Some("2031-07-04T12:00:00Z"),
            encryption_mode: Some("Allowed"),
            private_key: true,
            tags: vec![("b", "second"), ("a", "first")],
        },
    ]
}

/// A SAML provider ARN is `arn:aws:iam::<account>:saml-provider/<Name>` — IAM is global,
/// so the region segment is EMPTY, and the provider is named by its `Name`.
pub(crate) fn expected_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:saml-provider/{}", account_id(), name)
}

/// The instant a reported `dateType` member names, parsed back from the wire.
fn reported_instant(text: &str) -> DateTime {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} is not a wire date-time: {error}"))
}

/// `privateKeyIdType` (which both `SAMLProviderUUID` and `SAMLPrivateKey$KeyId` target)
/// is len 22..64 with pattern `^[A-Z0-9]+$` (`smithy_query(query="shape",
/// shape="privateKeyIdType")`).
fn assert_private_key_id_shape(case: &str, member: &str, value: &str) {
    assert!(
        (22..=64).contains(&value.len())
            && value
                .chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()),
        "case {case:?}: {member} must be 22..64 uppercase alphanumerics, got {value:?}"
    );
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// Every row creates a provider that `GetSAMLProvider` then reports identically: the
/// metadata document byte for byte, the instant it was created, the expiration stated in
/// that document, the encryption mode the request chose, one private key's metadata when
/// it uploaded one, and its tags sorted by key. The ARN is derived from the name, so the
/// create's ARN is the handle the read resolves — and every provider's uuid is its own.
#[tokio::test]
async fn a_created_saml_provider_round_trips_through_get_saml_provider() {
    let router = router();
    let mut uuids: Vec<String> = Vec::new();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) = call(&router, "CreateSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {created}",
            case.name
        );

        let arn = element(&created, "SAMLProviderArn");
        assert_eq!(
            arn,
            expected_arn(case.name),
            "case {:?}: the ARN must be derived from the name, got {arn:?}",
            case.name
        );
        // `arnType` is modelled len 20..2048 — the constraint on the value this mints.
        assert!(
            arn.len() >= 20 && arn.len() <= 2048,
            "case {:?}: the ARN must be 20..2048 characters, got {} in {arn:?}",
            case.name,
            arn.len()
        );
        // "The returned list of tags is sorted by tag key" (`CreateSAMLProvider$Tags`).
        assert_eq!(
            elements(&created, "Key"),
            case.sorted_tag_keys(),
            "case {:?}: the create must report its tags sorted by key, got {created}",
            case.name
        );
        assert_eq!(
            reported_tags(&created),
            case.expected_tags(),
            "case {:?}: the create must report the tags it was given, got {created}",
            case.name
        );

        // The read reports the SAME record, resolved by the ARN the create answered with.
        let read_params = vec![param("SAMLProviderArn", &arn)];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetSAMLProvider, got {status} with body {read}",
            case.name
        );
        let document = element(&read, "SAMLMetadataDocument");
        assert_eq!(
            document,
            case.document(),
            "case {:?}: the read must report the stored document verbatim, got {document:?}",
            case.name
        );
        let uuid = element(&read, "SAMLProviderUUID");
        assert_private_key_id_shape(case.name, "SAMLProviderUUID", &uuid);
        assert!(
            !uuids.contains(&uuid),
            "case {:?}: every provider's uuid must be its own, got the held {uuid:?}",
            case.name
        );
        uuids.push(uuid);

        let create_date = element(&read, "CreateDate");
        assert!(
            !create_date.is_empty(),
            "case {:?}: the read must report a CreateDate, got {read}",
            case.name
        );
        // `ValidUntil` is the document's `validUntil`: reported when the document states
        // one, and absent when it states none.
        match case.expected_valid_until {
            None => assert_eq!(
                optional(&read, "ValidUntil"),
                None,
                "case {:?}: a document stating no expiration must report none, got {read}",
                case.name
            ),
            Some(until) => {
                let reported = element(&read, "ValidUntil");
                assert_eq!(
                    reported_instant(&reported),
                    reported_instant(until),
                    "case {:?}: ValidUntil must be the document's validUntil {until:?}, got \
                     {reported:?}",
                    case.name
                );
            }
        }
        assert_eq!(
            optional(&read, "AssertionEncryptionMode").as_deref(),
            case.encryption_mode,
            "case {:?}: the read must report the encryption mode the create chose, got {read}",
            case.name
        );
        // One uploaded key is one entry of `PrivateKeyList`, carrying the id it was given
        // and the instant it was uploaded; a create that uploaded none reports none.
        let key_ids = elements(&read, "KeyId");
        assert_eq!(
            key_ids.len(),
            usize::from(case.private_key),
            "case {:?}: expected {} private key entries, got {read}",
            case.name,
            usize::from(case.private_key)
        );
        if let Some(key_id) = key_ids.first() {
            assert_private_key_id_shape(case.name, "KeyId", key_id);
            let timestamp = element(&read, "Timestamp");
            assert_eq!(
                reported_instant(&timestamp),
                reported_instant(&create_date),
                "case {:?}: a key uploaded by the create carries the create's instant, got \
                 {timestamp:?}",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: the read must report the stored tags, got {read}",
            case.name
        );
    }
}

/// Reading is a READ: two `GetSAMLProvider` calls report the same record, and the second
/// one is not affected by the first.
#[tokio::test]
async fn reading_a_saml_provider_changes_nothing_a_later_caller_sees() {
    let router = router();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) = call(&router, "CreateSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture provider must be created, got {status}: {created}",
            case.name
        );
        let read_params = vec![param("SAMLProviderArn", &element(&created, "SAMLProviderArn"))];

        let (_, first) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        let (_, second) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        for member in [
            "SAMLProviderUUID",
            "SAMLMetadataDocument",
            "CreateDate",
            "ValidUntil",
            "AssertionEncryptionMode",
            "KeyId",
        ] {
            assert_eq!(
                elements(&first, member),
                elements(&second, member),
                "case {:?}: member {member}: two reads must report the same value",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two reads must report the same tags",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// A name is registered at most once
// ---------------------------------------------------------------------------

/// A provider is named by its `Name`, so a second create for a held name is
/// `EntityAlreadyExists` (HTTP 409) and the held provider keeps the record the FIRST
/// create wrote.
#[tokio::test]
async fn a_second_create_for_a_held_name_is_refused_and_changes_nothing() {
    let router = router();

    for case in provider_cases() {
        let params = case.params();
        let (status, created) = call(&router, "CreateSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create must succeed, got {status}: {created}",
            case.name
        );
        let arn = element(&created, "SAMLProviderArn");

        // The repeat carries a DIFFERENT document, encryption mode and tags, so a refusal
        // that silently overwrote the held record would be visible in the read below.
        let repeat = ProviderCase {
            name: case.name,
            form: MetadataForm::Entity,
            entity_id: "https://overwritten.example.com/saml",
            valid_until: Some("2029-09-09T09:09:09Z"),
            expected_valid_until: Some("2029-09-09T09:09:09Z"),
            encryption_mode: Some("Required"),
            private_key: true,
            tags: vec![("overwritten", "yes")],
        };
        let repeat_params = repeat.params();
        let (status, body) = call(&router, "CreateSAMLProvider", &borrowed(&repeat_params)).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected HTTP 409, got {status} with body {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {:?}: expected EntityAlreadyExists, got {body}",
            case.name
        );

        let read_params = vec![param("SAMLProviderArn", &arn)];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must still read back, got {status}: {read}",
            case.name
        );
        assert_eq!(
            element(&read, "SAMLMetadataDocument"),
            case.document(),
            "case {:?}: a refused create must leave the held document alone, got {read}",
            case.name
        );
        assert_eq!(
            optional(&read, "AssertionEncryptionMode").as_deref(),
            case.encryption_mode,
            "case {:?}: a refused create must leave the held encryption mode alone, got {read}",
            case.name
        );
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: a refused create must leave the held tags alone, got {read}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Filler that carries a bad document past `SAMLMetadataDocumentType`'s 1000-character
/// minimum, so the handler is what refuses it rather than the boundary validator.
fn filler() -> String {
    "0123456789".repeat(120)
}

/// A `SAMLMetadataDocument` that is not SAML 2.0 metadata — not XML at all, rooted at
/// something else, in another namespace, naming no issuer, or stating an expiration that
/// is not a date-time — is `InvalidInput` (HTTP 400), and nothing is persisted for it.
#[tokio::test]
async fn a_document_that_is_not_saml_metadata_is_refused() {
    let router = router();
    let filler = filler();
    let bad_documents: Vec<(&str, String)> = vec![
        ("not XML at all", format!("this is not a document {filler}")),
        (
            "an unclosed element",
            format!("<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"x\">{filler}"),
        ),
        (
            "rooted at another element",
            format!(
                "<NotADescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}</NotADescriptor>"
            ),
        ),
        (
            "a descriptor in another namespace",
            format!(
                "<EntityDescriptor xmlns=\"urn:example:not-saml\" entityID=\"https://i.example\">\
                 {filler}</EntityDescriptor>"
            ),
        ),
        (
            "no entityID",
            format!("<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}</EntityDescriptor>"),
        ),
        (
            "an empty entityID",
            format!(
                "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"\">{filler}\
                 </EntityDescriptor>"
            ),
        ),
        (
            "a group describing no entity",
            format!(
                "<EntitiesDescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}\
                 </EntitiesDescriptor>"
            ),
        ),
        (
            "a validUntil that is not a date-time",
            format!(
                "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"https://i.example\" \
                 validUntil=\"next tuesday\">{filler}</EntityDescriptor>"
            ),
        ),
    ];

    for (index, (what, document)) in bad_documents.iter().enumerate() {
        let name = format!("Refused{index}");
        let params = vec![
            param("Name", &name),
            param("SAMLMetadataDocument", document),
        ];
        let (status, body) = call(&router, "CreateSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {body}"
        );

        // Nothing was persisted: the ARN the create would have minted names no provider.
        let read_params = vec![param("SAMLProviderArn", &expected_arn(&name))];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused create must persist nothing, got {status}: {read}"
        );
        assert_eq!(
            error_code(&read),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {read}"
        );
    }
}

/// A reserved tag key is `InvalidInput` (HTTP 400) and "the entire request fails and the
/// resource is not created" (`CreateSAMLProvider$Tags`).
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_creates_nothing() {
    let router = router();
    let document = metadata_document(MetadataForm::Entity, "https://idp.example.com/saml", None);

    for (index, key) in ["aws:created", "AWS:Owner", "aws:"].iter().enumerate() {
        let name = format!("ReservedTag{index}");
        let params = vec![
            param("Name", &name),
            param("SAMLMetadataDocument", &document),
            param("Tags.member.1.Key", key),
            param("Tags.member.1.Value", "whatever"),
        ];
        let (status, body) = call(&router, "CreateSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {key:?}: expected InvalidInput, got {body}"
        );

        let read_params = vec![param("SAMLProviderArn", &expected_arn(&name))];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {key:?}: a refused create must persist nothing, got {status}: {read}"
        );
    }
}

/// `GetSAMLProvider` tells a value that is not an ARN (`InvalidInput`, HTTP 400) apart
/// from an ARN that names no provider the account holds (`NoSuchEntity`, HTTP 404).
#[tokio::test]
async fn reading_an_absent_provider_is_not_found_and_a_non_arn_is_invalid() {
    let router = router();

    for value in ["ExampleIdP", "iam::saml-provider/ExampleIdP", ""] {
        let params = vec![param("SAMLProviderArn", value)];
        let (status, body) = call(&router, "GetSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, got \
             {status}: {body}"
        );
    }

    for name in ["NeverCreated", "another.provider-1"] {
        let params = vec![param("SAMLProviderArn", &expected_arn(name))];
        let (status, body) = call(&router, "GetSAMLProvider", &borrowed(&params)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
    }
}
