// SPDX-License-Identifier: CC0-1.0

//! `ListSAMLProviders`, end to end over the awsQuery wire, cross-read through
//! `GetSAMLProvider`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, XML members) of providers that were PERSISTED by
//! `CreateSAMLProvider`, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share, and the
//! metadata documents are the ones `tests/all/saml_providers.rs` builds.
//!
//! The rule under test is that the listing reports EVERY provider the account holds,
//! exactly once, as the subset of attributes `SAMLProviderListEntry` declares — the ARN
//! that names it, the instant it was created and the expiration its metadata document
//! states — each agreeing with what `GetSAMLProvider` reports for the same provider, and
//! that it is a read: it reports no other attribute of a provider and changes nothing a
//! later caller sees.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: how many
//! providers the account holds (from none upwards), the provider name's alphabet and its
//! length at the modelled bound, how the metadata document is spelled, whether it states an
//! expiration and in which form, the encryption mode, whether a private key was uploaded
//! and the tag count — so no single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, optional, param};
use crate::saml_providers::{MetadataForm, expected_arn, metadata_document, reported_instant};
use crate::user_lifecycle::{call, element, elements, router};

/// One provider the account is made to hold, with the members the row exercises.
struct ListingCase {
    /// `Name` — pattern `^[\w._-]+$`, len 1..128 (`CreateSAMLProvider$Name`).
    name: String,
    /// How the row's metadata document is spelled.
    form: MetadataForm,
    /// The issuer the document names in `entityID`.
    entity_id: &'static str,
    /// The document's `validUntil`, when it states one.
    valid_until: Option<&'static str>,
    /// The instant that `validUntil` names, written in the `Z` form: the value
    /// `SAMLProviderListEntry$ValidUntil` must report. Spelled out per row rather than
    /// computed, so a row carrying a UTC offset pins the instant independently of the code
    /// that resolves the offset.
    expected_valid_until: Option<&'static str>,
    /// `AssertionEncryptionMode` — one value of `assertionEncryptionModeType`, or none.
    encryption_mode: Option<&'static str>,
    /// Whether the create uploads a private key in `AddPrivateKey`.
    private_key: bool,
    /// The tags the create carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl ListingCase {
    fn document(&self) -> String {
        metadata_document(self.form, self.entity_id, self.valid_until)
    }

    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            param("Name", &self.name),
            param("SAMLMetadataDocument", &self.document()),
        ];
        if let Some(mode) = self.encryption_mode {
            params.push(param("AssertionEncryptionMode", mode));
        }
        if self.private_key {
            // "The private key must be a .pem file" (`CreateSAMLProvider$AddPrivateKey`),
            // whose pattern `^[\u0009\u000A\u000D\u0020-\u00FF]+$` admits this ASCII body.
            params.push(param(
                "AddPrivateKey",
                &format!(
                    "-----BEGIN PRIVATE KEY----- {} -----END PRIVATE KEY-----",
                    "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVph".repeat(8)
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
}

/// How many characters a provider name may carry: `SAMLProviderNameType` is len 1..128
/// (`smithy_query(query="operation", operation="CreateSAMLProvider")`), so the table holds
/// a name at each published bound.
const MAX_PROVIDER_NAME_CHARACTERS: usize = 128;

/// The providers this suite's rows create, in the order they are created — which is NOT
/// the order the account enumerates them, so an assertion about the reported set cannot
/// depend on the order the creates arrived in.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            name: "ListedIdP".to_string(),
            form: MetadataForm::Entity,
            entity_id: "https://idp.example.com/saml",
            valid_until: Some("2032-05-30T09:01:31Z"),
            expected_valid_until: Some("2032-05-30T09:01:31Z"),
            encryption_mode: None,
            private_key: false,
            tags: Vec::new(),
        },
        ListingCase {
            name: "corp_idp-2.eu".to_string(),
            form: MetadataForm::PrefixedEntity,
            entity_id: "urn:example:idp:corp",
            valid_until: None,
            expected_valid_until: None,
            encryption_mode: Some("Allowed"),
            private_key: false,
            tags: vec![("env", "dev")],
        },
        ListingCase {
            name: "Federation.Partner_7".to_string(),
            form: MetadataForm::EntityGroup,
            entity_id: "https://partner.example.org/metadata",
            valid_until: Some("2030-01-02T03:04:05.678Z"),
            expected_valid_until: Some("2030-01-02T03:04:05.678Z"),
            encryption_mode: Some("Required"),
            private_key: true,
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        ListingCase {
            // The shortest name the model admits.
            name: "a".to_string(),
            form: MetadataForm::Entity,
            entity_id: "https://single.example.net/idp",
            // Two hours EAST of UTC, so the instant is two hours EARLIER in the `Z` form.
            valid_until: Some("2035-12-31T23:59:59+02:00"),
            expected_valid_until: Some("2035-12-31T21:59:59Z"),
            encryption_mode: None,
            private_key: true,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        ListingCase {
            // The longest name the model admits.
            name: "Z".repeat(MAX_PROVIDER_NAME_CHARACTERS),
            form: MetadataForm::EntityGroup,
            entity_id: "urn:example:federation:group",
            valid_until: None,
            expected_valid_until: None,
            encryption_mode: Some("Required"),
            private_key: false,
            tags: vec![("only", "one")],
        },
        ListingCase {
            name: "ZoneLessExpiry".to_string(),
            form: MetadataForm::PrefixedEntity,
            entity_id: "https://zoneless.example.com/saml",
            // An `xs:dateTime` stating no zone names an instant all the same.
            valid_until: Some("2031-07-04T12:00:00"),
            expected_valid_until: Some("2031-07-04T12:00:00Z"),
            encryption_mode: Some("Allowed"),
            private_key: true,
            tags: vec![("b", "second"), ("a", "first")],
        },
    ]
}

/// One entry of a `SAMLProviderList`, read off the members `SAMLProviderListEntry`
/// declares (`smithy_query(query="shape", shape="SAMLProviderListEntry")`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedProvider {
    arn: String,
    create_date: String,
    valid_until: Option<String>,
}

/// The entries of the `SAMLProviderList` a response carries, in document order.
///
/// A list entry is a STRUCTURE, so the shared leaf scan cannot read it: this narrows to
/// each entry block of the named list first and reads the entry's leaves inside it. A
/// response reporting an empty list carries no block at all.
///
/// An opening tag is matched up to its `>` rather than literally, because the response
/// serializer declares the document namespace as an attribute on the first child it emits.
fn listed_providers(body: &str) -> Vec<ListedProvider> {
    let Some((content, _)) = split_element(body, "SAMLProviderList") else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut rest = content;
    while let Some((entry, remainder)) = split_element(rest, "member") {
        entries.push(ListedProvider {
            arn: element(entry, "Arn"),
            create_date: element(entry, "CreateDate"),
            valid_until: optional(entry, "ValidUntil"),
        });
        rest = remainder;
    }
    entries
}

/// The content of the first `<tag ...>` element of `body` and the text that follows its
/// closing tag, or `None` when `body` carries no such element.
fn split_element<'a>(body: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = body.find(&open)?;
    let after_name = &body[start + open.len()..];
    let open_end = after_name
        .find('>')
        .unwrap_or_else(|| panic!("<{tag}>'s opening tag is never closed in {body}"));
    let inner = &after_name[open_end + 1..];
    let end = inner
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
    Some((&inner[..end], &inner[end + close.len()..]))
}

/// Create one fixture provider, answering the ARN the create reports.
async fn create_provider(router: &MultiServiceRouter, case: &ListingCase) -> String {
    let params = case.params();
    let (status, body) = call(router, "CreateSAMLProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateSAMLProvider must succeed, got {status}: {body}",
        case.name
    );
    let arn = element(&body, "SAMLProviderArn");
    assert_eq!(
        arn,
        expected_arn(&case.name),
        "case {:?}: the ARN must be derived from the name, got {arn:?}",
        case.name
    );
    arn
}

/// List the account's providers, asserting the call is served.
async fn list_providers(router: &MultiServiceRouter, case: &str) -> Vec<ListedProvider> {
    let (status, body) = call(router, "ListSAMLProviders", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListSAMLProviders, got {status} with body {body}"
    );
    listed_providers(&body)
}

// ---------------------------------------------------------------------------
// Every held provider, exactly once
// ---------------------------------------------------------------------------

/// An account holding no SAML provider reports an EMPTY list, not a refusal: the list is
/// the operation's whole answer, so "none held" is an answer.
#[tokio::test]
async fn an_account_holding_no_saml_provider_reports_an_empty_list() {
    let router = router();

    for attempt in ["first", "second"] {
        let entries = list_providers(&router, attempt).await;
        assert!(
            entries.is_empty(),
            "case {attempt:?}: an account holding no provider must report no entry, got \
             {entries:?}"
        );
    }
}

/// The listing reports every provider the account holds, exactly once, however many it
/// holds: the account is grown one provider at a time and listed after each create, so the
/// reported set is asserted at every count from one upwards.
#[tokio::test]
async fn every_held_provider_is_reported_exactly_once_at_every_count() {
    let router = router();
    let mut expected_arns: Vec<String> = Vec::new();

    for case in listing_cases() {
        expected_arns.push(create_provider(&router, &case).await);
        expected_arns.sort();

        let entries = list_providers(&router, &case.name).await;
        let mut reported: Vec<String> = entries.iter().map(|entry| entry.arn.clone()).collect();
        reported.sort();
        assert_eq!(
            reported, expected_arns,
            "case {:?}: the listing must report every held provider exactly once, got \
             {reported:?}",
            case.name
        );
        // `arnType` is modelled len 20..2048 — the constraint on every reported ARN.
        for entry in &entries {
            assert!(
                entry.arn.len() >= 20 && entry.arn.len() <= 2048,
                "case {:?}: a reported ARN must be 20..2048 characters, got {} in {:?}",
                case.name,
                entry.arn.len(),
                entry.arn
            );
        }
    }
}

/// Each entry reports the stored record: its `CreateDate` is the one `GetSAMLProvider`
/// reports for the same provider, and its `ValidUntil` is the expiration the provider's
/// metadata document states — present exactly when the document states one.
#[tokio::test]
async fn each_entry_agrees_with_get_saml_provider() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let entries = list_providers(&router, "every provider").await;
    for case in &cases {
        let arn = expected_arn(&case.name);
        let entry = entries
            .iter()
            .find(|entry| entry.arn == arn)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing must report the held provider {arn:?}, got {entries:?}",
                    case.name
                )
            });

        let read_params = vec![param("SAMLProviderArn", &arn)];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must read back, got {status}: {read}",
            case.name
        );
        assert_eq!(
            reported_instant(&entry.create_date),
            reported_instant(&element(&read, "CreateDate")),
            "case {:?}: the entry's CreateDate must be the provider's, got {:?}",
            case.name,
            entry.create_date
        );
        match case.expected_valid_until {
            None => assert_eq!(
                entry.valid_until, None,
                "case {:?}: a provider whose document states no expiration must report none, \
                 got {:?}",
                case.name, entry.valid_until
            ),
            Some(until) => {
                let reported = entry
                    .valid_until
                    .as_deref()
                    .unwrap_or_else(|| panic!("case {:?}: expected a ValidUntil", case.name));
                assert_eq!(
                    reported_instant(reported),
                    reported_instant(until),
                    "case {:?}: the entry's ValidUntil must be the document's validUntil \
                     {until:?}, got {reported:?}",
                    case.name
                );
                assert_eq!(
                    reported_instant(reported),
                    reported_instant(&element(&read, "ValidUntil")),
                    "case {:?}: the entry's ValidUntil must be the one the read reports, got \
                     {reported:?}",
                    case.name
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A subset of the attributes, and nothing written
// ---------------------------------------------------------------------------

/// "IAM resource-listing operations return a subset of the available attributes for the
/// resource. For example, this operation does not return tags" (`ListSAMLProviders`): the
/// subset is what the entry shape declares, so no other attribute of a provider — its
/// tags, its metadata document, its uuid, its encryption mode or its private key metadata
/// — appears in the response, even though every row's provider carries some of them.
#[tokio::test]
async fn the_listing_reports_only_the_attributes_the_entry_shape_declares() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let (status, body) = call(&router, "ListSAMLProviders", &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListSAMLProviders, got {status} with body {body}"
    );
    for member in [
        "Key",
        "Value",
        "SAMLMetadataDocument",
        "SAMLProviderUUID",
        "AssertionEncryptionMode",
        "KeyId",
        "Timestamp",
    ] {
        assert!(
            elements(&body, member).is_empty(),
            "the listing must not report <{member}>, got {body}"
        );
    }
    // Every entry carries the members the shape DOES declare.
    let entries = listed_providers(&body);
    assert_eq!(
        entries.len(),
        cases.len(),
        "expected {} entries, got {entries:?}",
        cases.len()
    );
}

/// Listing is a READ: two listings report the same entries, and a provider read after them
/// still reports the record the create wrote.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    let cases = listing_cases();
    for case in &cases {
        create_provider(&router, case).await;
    }

    let first = list_providers(&router, "first listing").await;
    let second = list_providers(&router, "second listing").await;
    assert_eq!(
        first, second,
        "two listings must report the same entries, got {first:?} then {second:?}"
    );

    for case in &cases {
        let read_params = vec![param("SAMLProviderArn", &expected_arn(&case.name))];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held provider must still read back, got {status}: {read}",
            case.name
        );
        assert_eq!(
            element(&read, "SAMLMetadataDocument"),
            case.document(),
            "case {:?}: a listing must leave the held document alone, got {read}",
            case.name
        );
        assert_eq!(
            optional(&read, "AssertionEncryptionMode").as_deref(),
            case.encryption_mode,
            "case {:?}: a listing must leave the held encryption mode alone, got {read}",
            case.name
        );
        assert_eq!(
            elements(&read, "KeyId").len(),
            usize::from(case.private_key),
            "case {:?}: a listing must leave the held private key metadata alone, got {read}",
            case.name
        );
    }
}
