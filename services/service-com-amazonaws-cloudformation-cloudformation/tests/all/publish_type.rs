// SPDX-License-Identifier: CC0-1.0

//! `PublishType` over the awsQuery wire: publishing a registered private extension as a
//! public extension of this Region.
//!
//! The rule every case here holds the service to: **`PublishType` reads an `Arn` of this
//! account for the VERSION it pins first of all, then its CALLER — an account holding no
//! verified publisher registration may publish nothing, so it is refused the registry's
//! modelled `CFNRegistryException` (400) naming it before the registry is consulted
//! (`publish_type_publisher_preflight.rs` is the table for that rule) — and only then RESOLVES
//! the extension a
//! request identifies — by `Arn`, or by `Type` and `TypeName` — against the registry of THIS
//! account and Region, refusing an identifier that
//! resolves to no record of this account and Region with that same modelled 400 and its fixed
//! not-registered sentence; for a record that DOES
//! resolve it publishes the DEFAULT version of a PRIVATE registration — never a version the
//! request picked, so an `Arn` of this account that pins one is refused that same modelled 400
//! from the ARN alone (`publish_type_version_in_arn.rs` is the table for that rule) —
//! writing a
//! PUBLIC extension record of that Region which carries the published version under the
//! public version number the publication is assigned (`1.0.0` for a publisher's first
//! publication of the extension, one minor version release past the latest otherwise, or the
//! number the request names), answered as that record's account-less ARN; a record this
//! account did not register, a default version whose contract tests have not concluded
//! `PASSED` and a request that identifies no extension are
//! each the modelled 400, and a resolved record carrying no version at all the modelled
//! `TypeNotFoundException` (404), each writing nothing.**
//!
//! The tables span the class the rule generalises over: every way a request can identify an
//! extension (`Type` and `TypeName`, the extension's ARN, a version ARN), over records
//! spanning every `RegistryType` (`RESOURCE`, `MODULE`, `HOOK`) and every visibility a
//! record can have (private, public third-party activated here, public and
//! Amazon-Web-Services-published, and one this Region's registry does not hold because it was
//! registered in another), over each contract-test status a default version can carry, and
//! over every way a publication's version number can be decided (assigned first, incremented,
//! named by the request, already published). The recorded trace this suite was written for is
//! ONE row of the ordering table: a publication addressed by an ARN of an account other than
//! this one, which is answered the not-registered refusal.
//!
//! `RegisterType`, the registry's one WRITER still a scaffold stub, has an issue of its own,
//! so the records published here are the ones
//! `extension_registry.rs` seeds straight into the store — this crate's one registry fixture,
//! so what `PublishType` writes and what `DescribeType` reports back are held to the same
//! records. Every call goes through the real request path (form-encoded body in, XML envelope
//! out), so what is asserted is what a client sees.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    public_type_arn, registry_type, type_arn, type_tests_status, type_version_arn, visibility,
    StoredExtension, StoredTypeVersion, FIRST_PUBLIC_VERSION_NUMBER, MAX_PUBLISHED_VERSIONS,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{router_with_registry, split_required_activated_types, OTHER_REGION};
use crate::stack_lifecycle::{
    call, member, store, wait_for_the_transition, xml_text, ACCOUNT_ID, REGION,
};

/// The refusal AWS answers a request whose `Arn` names one VERSION of an extension this
/// account registered: only the extension's default version is published. Entirely fixed
/// text: no part of it comes from the request or from the store.
pub(crate) const ONLY_DEFAULT_VERSION: &str =
    "Only default version can be published. Please remove versionId from Arn";

/// The refusal AWS answers a request identifying an extension that is registered in no
/// account — existence and ownership are what a publication resolves first, so this is the
/// answer whatever else the request carries. Entirely fixed text as well, spelled out here
/// rather than read from the crate, because the sentence IS the requirement.
pub(crate) const NOT_FOUND_IN_YOUR_ACCOUNT: &str = "Type not found in your account.";

/// The refusal AWS answers an account that is not a verified publisher of extensions, which is
/// what publishing requires of its caller before the registry is consulted: fixed
/// wording whose one variable part is the calling account's 12-digit id.
pub(crate) fn not_a_verified_publisher() -> String {
    format!(
        "The account {ACCOUNT_ID} is not verified as a publisher. Please register as \
         publisher first"
    )
}

pub(crate) async fn publish_type(
    router: &MultiServiceRouter,
    members: &[(&str, &str)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "PublishType", &members).await
}

pub(crate) async fn describe_type(
    router: &MultiServiceRouter,
    members: &[(&str, &str)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DescribeType", &members).await
}

async fn test_type(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "TestType", &members).await
}

/// Register this account as an extension publisher, and answer the publisher id the
/// registration was assigned: the publisher every published extension below belongs to.
pub(crate) async fn register_publisher(router: &MultiServiceRouter) -> String {
    let (status_code, body) = call(
        router,
        "RegisterPublisher",
        &[member("AcceptTermsAndConditions", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"registering as a publisher\": expected 200, got {status_code} {body}"
    );
    xml_text(&body, "PublisherId").expect("the registration answers a publisher id")
}

/// Take the default version of every private record to a `PASSED` contract-test status, which
/// is what "an extension must have a test status of PASSED before it can be published"
/// requires of a publishable version: `TestType` concludes `PASSED` for a version whose stored
/// schema declares the extension it defines, so a version the fixture holds no schema for is
/// given one first.
pub(crate) fn seed_a_schema_for_each_default_version(engine: &Arc<engine::InMemoryEngine>) {
    let state = store(engine);
    let mut guard = state.lock();
    for extension in &mut guard.extensions {
        if !extension.is_private() {
            continue;
        }
        let type_name = extension.type_name.clone();
        let Some(default_version_id) = extension.default_version_id.clone() else {
            continue;
        };
        if let Some(version) = extension.version_mut(&default_version_id) {
            if version.schema.is_none() {
                version.schema = Some(format!("{{\"typeName\":\"{type_name}\"}}"));
            }
        }
    }
}

/// Run the contract tests of the default version the request names and wait past the instant
/// their conclusion is due, so the version carries `PASSED` when the publication is made.
pub(crate) async fn pass_the_contract_tests(router: &MultiServiceRouter, members: &[(&str, &str)]) {
    let (status_code, body) = test_type(router, members).await;
    assert_eq!(
        status_code, 200,
        "case {members:?}: expected the contract tests started, got {status_code} {body}"
    );
    wait_for_the_transition().await;
}

/// Hold a refused call to the registry's modelled refusal: HTTP 400 `CFNRegistryException`,
/// carrying `message`.
pub(crate) fn assert_registry_refusal(case: &str, status_code: u16, body: &str, message: &str) {
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("CFNRegistryException"),
        "case {case:?}: expected CFNRegistryException, got {body}"
    );
    let reported = xml_text(body, "Message");
    assert_eq!(
        reported.as_deref(),
        Some(message),
        "case {case:?}: expected {message:?}, got {reported:?}"
    );
    assert_eq!(
        xml_text(body, "PublicTypeArn"),
        None,
        "case {case:?}: expected no PublicTypeArn in the refusal, got {body}"
    );
}

/// Hold a refused call to the modelled 404: `TypeNotFoundException`, "the specified extension
/// doesn't exist in the CloudFormation registry", naming what the request named.
pub(crate) fn assert_not_found(case: &str, status_code: u16, body: &str, identifier: &str) {
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the call refused with 404, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("TypeNotFoundException"),
        "case {case:?}: expected TypeNotFoundException, got {body}"
    );
    let reported = xml_text(body, "Message");
    let expected = format!("The type '{identifier}' cannot be found.");
    assert_eq!(
        reported.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
}

/// Every record the registry holds right now, as the one snapshot a case compares a refused
/// call against: a refusal must publish nothing.
pub(crate) fn registry_snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    format!("{:?}", store(engine).lock().extensions)
}

/// The ARNs of the seeded private records, which every case addresses its extension by.
pub(crate) fn resource_arn() -> String {
    type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource")
}

pub(crate) fn module_arn() -> String {
    type_arn(REGION, registry_type::MODULE, "My::Sample::Thing::MODULE")
}

pub(crate) fn hook_arn() -> String {
    type_arn(REGION, registry_type::HOOK, "My::Sample::Hook")
}

/// The refusal AWS answers a request whose `Arn` names a version is read from the ARN itself
/// where the ARN is one of THIS account, so it holds for an extension this account registered
/// whether or not the caller is a publisher: what a publication publishes is the extension's
/// default version, never a version the request picked.
#[tokio::test]
async fn an_arn_that_names_a_version_of_a_registered_extension_is_refused() {
    let (router, engine) = router_with_registry();
    let before = registry_snapshot(&engine);

    // One row per registered private record, and per position a version can hold in one: the
    // record's default version, a version that is not the default, and a record's only
    // version. An identifier that resolves NO record of this account is a different refusal
    // and has its own table (`an_identifier_registered_in_no_account_is_refused_first`), and
    // the account segment this rule is keyed on has its own suite
    // (`publish_type_version_in_arn.rs`).
    let versioned = [
        (
            "the default version of a registered private resource type",
            type_version_arn(&resource_arn(), "00000002"),
        ),
        (
            "a non-default version of a registered private resource type",
            type_version_arn(&resource_arn(), "00000001"),
        ),
        (
            "the only version of a registered private module",
            type_version_arn(&module_arn(), "00000001"),
        ),
        (
            "the only version of a registered private hook",
            type_version_arn(&hook_arn(), "00000003"),
        ),
    ];

    // Refused for an account that is not a publisher yet, which is what the recorded trace
    // was made by: the version an ARN of this account pins is read from the ARN alone, before
    // the caller is judged and before the registry is consulted.
    for (case, arn) in &versioned {
        let (status_code, body) = publish_type(&router, &[("Arn", arn)]).await;
        assert_registry_refusal(case, status_code, &body, ONLY_DEFAULT_VERSION);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }

    // And refused for one that IS a publisher, with or without a version number beside it:
    // nothing about the caller or the request's other members makes a version ARN publishable.
    register_publisher(&router).await;
    for (case, arn) in &versioned {
        for extra in [vec![], vec![("PublicVersionNumber", "1.0.0")]] {
            let mut members = vec![("Arn", arn.as_str())];
            members.extend_from_slice(&extra);
            let (status_code, body) = publish_type(&router, &members).await;
            assert_registry_refusal(case, status_code, &body, ONLY_DEFAULT_VERSION);
            assert_eq!(
                registry_snapshot(&engine),
                before,
                "case {case:?}: expected the refused call to publish nothing"
            );
        }
    }
}

/// Every identifier that names no record of this account and Region, over both forms a
/// request identifies an extension by and over the ways an identifier can be real somewhere
/// else: a name nothing registered, a name registered under another kind, a record another
/// Region's registry holds, and an ARN of ANOTHER account, which addresses nothing of this
/// one.
///
/// A version segment past the extension an ARN of THIS account addresses is read from the ARN
/// before existence is, so an in-account version ARN belongs to the version-in-Arn table
/// (`publish_type_version_in_arn.rs`) rather than here.
fn identifiers_registered_in_no_account() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let unheld_arn = type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource");
    let other_region_arn = type_arn(
        OTHER_REGION,
        registry_type::RESOURCE,
        "Other::Region::Resource",
    );
    let other_account_arn = format!(
        "arn:aws:cloudformation:{REGION}:123456789012:type/resource/Another-Account-Resource"
    );
    vec![
        (
            "a name no registry holds",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
        ),
        (
            "the ARN of an extension no registry holds",
            vec![("Arn", unheld_arn.clone())],
        ),
        (
            // An ARN of ANOTHER account: nothing of this account is addressed, so existence
            // in this account is all the call can be answered about.
            "the ARN of an extension of another account",
            vec![("Arn", other_account_arn.clone())],
        ),
        (
            "a version ARN of an extension of another account",
            vec![("Arn", type_version_arn(&other_account_arn, "00000001"))],
        ),
        (
            "a name this Region's registry holds under another kind",
            vec![
                ("Type", registry_type::HOOK.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
        ),
        (
            "a record another Region's registry holds, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "Other::Region::Resource".to_string()),
            ],
        ),
        (
            "a record another Region's registry holds, by ARN",
            vec![("Arn", other_region_arn.clone())],
        ),
    ]
}

/// A publication resolves the extension against the registry of THIS account and Region once
/// the caller may publish at all: an identifier that resolves nothing is refused the
/// not-registered sentence whatever else the request carries — a version number beside it is
/// read against a record that resolved — while an account that is not a publisher is answered
/// the caller refusal instead, since the registry is not consulted for it at all.
#[tokio::test]
async fn an_identifier_registered_in_no_account_is_refused_first() {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    let before = registry_snapshot(&engine);

    // Before the account is a publisher and after it is, with and without a version number
    // beside the identifier: four passes over the same rows, since the version number is not
    // what either refusal is about and the publisher registration decides only WHICH of the
    // two the row reaches.
    for registered_publisher in [false, true] {
        if registered_publisher {
            register_publisher(&router).await;
        }
        for (case, identifier) in identifiers_registered_in_no_account() {
            for extra in [vec![], vec![("PublicVersionNumber", "1.0.0")]] {
                let mut members: Vec<(&str, &str)> = identifier
                    .iter()
                    .map(|(key, value)| (*key, value.as_str()))
                    .collect();
                members.extend_from_slice(&extra);
                let expected = if registered_publisher {
                    NOT_FOUND_IN_YOUR_ACCOUNT.to_string()
                } else {
                    not_a_verified_publisher()
                };
                let (status_code, body) = publish_type(&router, &members).await;
                assert_registry_refusal(case, status_code, &body, &expected);
                assert_eq!(
                    registry_snapshot(&engine),
                    before,
                    "case {case:?}: expected the refused call to publish nothing"
                );
            }
        }
    }

    // The ordering itself: one and the same ARN FORM — an extension ARN carrying a version
    // segment — is answered the version-shape refusal where the ARN's account segment is this
    // account's, whether or not the extension it names is registered here, and the
    // not-registered refusal where that segment is another account's, which addresses nothing
    // of this account for either rule to be read against.
    for (case, arn) in [
        (
            "a version ARN of a REGISTERED extension of this account",
            type_version_arn(&resource_arn(), "00000002"),
        ),
        (
            "a version ARN of an UNREGISTERED extension of this account",
            type_version_arn(
                &type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
                "00000002",
            ),
        ),
    ] {
        let (status_code, body) = publish_type(&router, &[("Arn", &arn)]).await;
        assert_registry_refusal(case, status_code, &body, ONLY_DEFAULT_VERSION);
    }
    let other_account = format!(
        "arn:aws:cloudformation:{REGION}:123456789012:type/resource/My-Sample-Resource/00000002"
    );
    let (status_code, body) = publish_type(&router, &[("Arn", &other_account)]).await;
    assert_registry_refusal(
        "the same version ARN form in ANOTHER account",
        status_code,
        &body,
        NOT_FOUND_IN_YOUR_ACCOUNT,
    );
}

/// A record that DOES resolve but carries no version at all has no default version to
/// publish, which is the one modelled 404 this operation still answers: the record exists, so
/// the refusal is about the version rather than about the account.
#[tokio::test]
async fn a_registered_record_carrying_no_version_is_the_modelled_not_found() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    const VERSIONLESS: &str = "My::Versionless::Resource";
    {
        let state = store(&engine);
        state
            .lock()
            .extensions
            .push(StoredExtension::new(REGION, registry_type::RESOURCE, VERSIONLESS));
    }
    let before = registry_snapshot(&engine);

    for (case, members) in [
        (
            "a registered record carrying no version, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", VERSIONLESS.to_string()),
            ],
        ),
        (
            "a registered record carrying no version, by ARN",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, VERSIONLESS),
            )],
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        // The refusal names the EXTENSION the request identified — its type name — whether
        // the request named it by name or by the ARN that carries that name.
        assert_not_found(case, status_code, &body, VERSIONLESS);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }
}

#[tokio::test]
async fn publishing_is_refused_for_an_account_that_is_not_a_registered_publisher() {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    let before = registry_snapshot(&engine);

    // One row per thing the request can name that DOES resolve to a private registration of
    // this account and Region — a registered private extension of each kind, by name and by
    // ARN — because the gate is on the CALLER: it is read before the registry is consulted,
    // so every row is refused with the same message, and the table over identifiers that
    // resolve nothing at all is in `publish_type_publisher_preflight.rs`.
    for (case, members) in [
        (
            "a registered private resource type, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
        ),
        (
            "a registered private resource type, by ARN",
            vec![("Arn", resource_arn())],
        ),
        (
            "a registered private module, by name",
            vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
        ),
        (
            "a registered private hook, by ARN",
            vec![("Arn", hook_arn())],
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        assert_registry_refusal(case, status_code, &body, &not_a_verified_publisher());
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }

    // The very same call, once the account IS a registered publisher and the version it
    // publishes has passed its contract tests, is served: the refusal above is about the
    // caller and nothing else.
    register_publisher(&router).await;
    pass_the_contract_tests(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
    )
    .await;
    let (status_code, body) = publish_type(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a registered publisher publishing a tested private resource type\": expected \
         200, got {status_code} {body}"
    );
}

#[tokio::test]
async fn an_extension_this_account_and_region_did_not_register_is_not_published() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    let before = registry_snapshot(&engine);

    // A public extension Amazon Web Services publishes, which every account's registry holds
    // without anybody registering it: the first entry of the catalogue this emulator
    // provisions, taken from the catalogue rather than written down here.
    let aws_published = IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the catalogue holds at least one resource type");
    let aws_published_arn = public_type_arn(REGION, registry_type::RESOURCE, aws_published);
    let public_alias_arn = type_arn(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let aws_seeded_arn = type_arn(REGION, registry_type::RESOURCE, "AWS::Sample::Resource");

    // Each record the registry holds that this account did NOT register, addressed by name
    // and by ARN where the modelled `PrivateTypeArn` admits one: publishing a public
    // extension is refused rather than served.
    for (case, members, arn) in [
        (
            "a public third-party extension activated in this account, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
            ],
            public_alias_arn.clone(),
        ),
        (
            "a public third-party extension activated in this account, by ARN",
            vec![("Arn", public_alias_arn.clone())],
            public_alias_arn.clone(),
        ),
        (
            "a public record of this store, by ARN",
            vec![("Arn", aws_seeded_arn.clone())],
            aws_seeded_arn,
        ),
        (
            "a public extension Amazon Web Services publishes, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", (*aws_published).to_string()),
            ],
            aws_published_arn,
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        let expected = format!(
            "Cannot publish {arn}: only a private extension registered in this account and \
             Region can be published"
        );
        assert_registry_refusal(case, status_code, &body, &expected);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }

    // An extension no registry of this Region holds at all, and one another Region's registry
    // holds: both resolve no record of this account, so both are the not-registered refusal —
    // the whole table of those identifiers lives in
    // `an_identifier_registered_in_no_account_is_refused_first`, and these rows are here
    // because the same suite publishes the records that DO resolve.
    for (case, members) in [
        (
            "an extension the registry does not hold, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
        ),
        (
            "an extension the registry does not hold, by ARN",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
            )],
        ),
        (
            "a record another Region's registry holds, by ARN",
            vec![(
                "Arn",
                type_arn(
                    OTHER_REGION,
                    registry_type::RESOURCE,
                    "Other::Region::Resource",
                ),
            )],
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        assert_registry_refusal(case, status_code, &body, NOT_FOUND_IN_YOUR_ACCOUNT);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }

    // A request that identifies no extension, or two: the registry's modelled 400, before any
    // record is resolved.
    for (case, members, expected) in [
        (
            "a request that identifies no extension at all",
            vec![],
            "Either Arn, or both Type and TypeName, must be specified to identify an extension",
        ),
        (
            "half of the name form",
            vec![("Type", registry_type::RESOURCE.to_string())],
            "Either Arn, or both Type and TypeName, must be specified to identify an extension",
        ),
        (
            "both identification forms at once",
            vec![
                ("Arn", resource_arn()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            "Specify either Arn, or Type and TypeName, but not both: an extension is \
             identified by its ARN or by its kind and name",
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        assert_registry_refusal(case, status_code, &body, expected);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }
}

/// "An extension must have a test status of PASSED before it can be published": one row per
/// contract-test status the fixture's default versions carry, and the same records published
/// once their tests have concluded `PASSED`.
#[tokio::test]
async fn a_default_version_that_has_not_passed_its_contract_tests_is_not_published() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    let before = registry_snapshot(&engine);

    for (case, members, version_arn, status) in [
        (
            "a resource type whose default version's contract tests FAILED",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            type_version_arn(&resource_arn(), "00000002"),
            type_tests_status::FAILED,
        ),
        (
            "a module whose only version has not been tested",
            vec![("Arn", module_arn())],
            type_version_arn(&module_arn(), "00000001"),
            type_tests_status::NOT_TESTED,
        ),
        (
            "a hook whose only version has not been tested",
            vec![("Arn", hook_arn())],
            type_version_arn(&hook_arn(), "00000003"),
            type_tests_status::NOT_TESTED,
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = publish_type(&router, &members).await;
        let expected = format!(
            "Cannot publish {version_arn}: an extension must have a test status of PASSED \
             before it can be published, and this version's test status is {status}"
        );
        assert_registry_refusal(case, status_code, &body, &expected);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }

    // The tests are what the refusal is about: the same resource type, once its default
    // version's contract tests have concluded `PASSED`, is published.
    pass_the_contract_tests(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
    )
    .await;
    let (status_code, body) = publish_type(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a resource type whose default version PASSED\": expected 200, got \
         {status_code} {body}"
    );
}

/// One row of the publication table: what the request names, the extension it publishes and
/// the version of it that must be the one published.
struct PublicationCase {
    case: &'static str,
    /// The members the `TestType` and `PublishType` calls both carry, which is how the row
    /// identifies the extension.
    members: Vec<(&'static str, String)>,
    type_kind: &'static str,
    type_name: &'static str,
    /// The `TypeVersionId` of the DEFAULT version, which is the one a publication publishes.
    version_id: &'static str,
}

fn publication_cases() -> Vec<PublicationCase> {
    vec![
        PublicationCase {
            // The record's default version is its SECOND, so the published version is
            // distinguishable from the first.
            case: "private resource type by name: the DEFAULT version is published",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Resource",
            version_id: "00000002",
        },
        PublicationCase {
            case: "private resource type by ARN: the DEFAULT version is published",
            members: vec![("Arn", resource_arn())],
            type_kind: registry_type::RESOURCE,
            type_name: "My::Sample::Resource",
            version_id: "00000002",
        },
        PublicationCase {
            case: "private module by name",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
            type_kind: registry_type::MODULE,
            type_name: "My::Sample::Thing::MODULE",
            version_id: "00000001",
        },
        PublicationCase {
            // A HOOK, whose only version is not the record's first: a version is addressed by
            // the id the record carries, never by its position.
            case: "private hook by ARN",
            members: vec![("Arn", hook_arn())],
            type_kind: registry_type::HOOK,
            type_name: "My::Sample::Hook",
            version_id: "00000003",
        },
    ]
}

#[tokio::test]
async fn publishing_the_default_version_writes_the_public_extension_describe_type_reads_back() {
    for PublicationCase {
        case,
        members,
        type_kind,
        type_name,
        version_id,
    } in publication_cases()
    {
        // One store per row: a publication is a write, so a row must not read what another
        // row published.
        let (router, engine) = router_with_registry();
        seed_a_schema_for_each_default_version(&engine);
        let publisher_id = register_publisher(&router).await;
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        pass_the_contract_tests(&router, &members).await;

        let (status_code, body) = publish_type(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        // "The Amazon Resource Name (ARN) assigned to the public extension upon
        // publication": a public extension's ARN carries no account id.
        let expected_arn = public_type_arn(REGION, type_kind, type_name);
        let reported = xml_text(&body, "PublicTypeArn");
        assert_eq!(
            reported.as_deref(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected {expected_arn:?}, got {reported:?}"
        );

        // The public extension the publication wrote, read back through `DescribeType` at the
        // ARN the publication answered.
        let (status_code, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the published extension readable, got {status_code} \
             {body}"
        );
        // `RequiredActivatedType` carries a `PublisherId` of its own, so the document is
        // split around that block before any TOP-LEVEL member is read out of it.
        let (required_activated_types, reported_members) = split_required_activated_types(&body);
        for (member_name, expected) in [
            ("Visibility", visibility::PUBLIC.to_string()),
            ("Type", type_kind.to_string()),
            ("TypeName", type_name.to_string()),
            ("PublisherId", publisher_id.clone()),
            // A publisher's FIRST publication of an extension: "CloudFormation
            // automatically sets the first version number to be 1.0.0".
            (
                "PublicVersionNumber",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
            (
                "LatestPublicVersion",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
            // Publishing an extension activates it nowhere.
            ("IsActivated", "false".to_string()),
        ] {
            let reported = xml_text(&reported_members, member_name);
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member_name} {expected:?}, got {reported:?}"
            );
        }
        // "For extensions that are modules, the public third-party extensions that must be
        // activated in your account in order for the module itself to be activated": the
        // published version carries what the version it was published from declared.
        assert_eq!(
            required_activated_types.contains("MyAlias::Sample::Resource"),
            type_kind == registry_type::MODULE,
            "case {case:?}: expected the published version's RequiredActivatedTypes, got \
             {required_activated_types:?}"
        );
        // The model of the version that was published, carried onto the public extension:
        // the schema is "the schema that defines the extension", so the public extension
        // defines the same one.
        let schema = xml_text(&reported_members, "Schema");
        assert!(
            schema
                .as_deref()
                .is_some_and(|schema| schema.contains(type_name)),
            "case {case:?}: expected the published version's schema, got {schema:?}"
        );
        // Private-extension members, withheld from the public record the publication wrote.
        for withheld in ["DefaultVersionId", "TypeTestsStatus", "ExecutionRoleArn"] {
            let reported = xml_text(&reported_members, withheld);
            assert_eq!(
                reported, None,
                "case {case:?}: expected no {withheld} on the public extension, got \
                 {reported:?}"
            );
        }

        // The private registration the publication was made FROM is left exactly as it was:
        // it is still the private record, still at its own default version.
        let (status_code, body) = describe_type(
            &router,
            &[("Type", type_kind), ("TypeName", type_name)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the private registration still readable, got \
             {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Visibility").as_deref(),
            Some(visibility::PRIVATE),
            "case {case:?}: expected the registration still private, got {body}"
        );
        let reported = xml_text(&body, "DefaultVersionId");
        assert_eq!(
            reported.as_deref(),
            Some(version_id),
            "case {case:?}: expected the default version still {version_id:?}, got \
             {reported:?}"
        );
    }
}

/// How a publication's version number is decided: assigned on a publisher's first
/// publication of an extension, incremented by one minor version release on a later one that
/// names none, taken from the request where it names one, and refused where the request names
/// one the extension is already published under.
#[tokio::test]
async fn a_publications_version_number_is_decided_by_what_is_already_published() {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    register_publisher(&router).await;
    let identified = [
        ("Type", registry_type::RESOURCE),
        ("TypeName", "My::Sample::Resource"),
    ];
    pass_the_contract_tests(&router, &identified).await;
    let public_arn = public_type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource");
    // A read before the snapshot, so it is taken PAST the contract-test conclusion the clock
    // has already made due: `settle` installs that on the next call either way, and it must
    // not show up as a write of the refused publication below.
    describe_type(&router, &identified).await;
    let before = registry_snapshot(&engine);

    // "You cannot specify a version number the first time you publish a type."
    let mut numbered = identified.to_vec();
    numbered.push(("PublicVersionNumber", "2.0.0"));
    let (status_code, body) = publish_type(&router, &numbered).await;
    assert_registry_refusal(
        "a version number on the first publication",
        status_code,
        &body,
        "You cannot specify a version number the first time you publish a type. \
         CloudFormation automatically sets the first version number to be 1.0.0",
    );
    assert_eq!(
        registry_snapshot(&engine),
        before,
        "case \"a version number on the first publication\": expected the refused call to \
         publish nothing"
    );

    // Each publication in turn: the number it must be assigned, and what the request names.
    for (case, requested, expected) in [
        (
            "the first publication, naming no number",
            None,
            FIRST_PUBLIC_VERSION_NUMBER,
        ),
        (
            // "If you don't specify a version number, CloudFormation increments the version
            // number by one minor version release."
            "a second publication, naming no number",
            None,
            "1.1.0",
        ),
        (
            "a third publication, naming no number",
            None,
            "1.2.0",
        ),
        (
            "a publication naming a major version of its own",
            Some("2.0.0"),
            "2.0.0",
        ),
        (
            "a publication after a named number, naming none",
            None,
            "2.1.0",
        ),
        (
            // The patch segment is `(.*)` in the modelled pattern, so a pre-release number
            // is a number this publication can be named with.
            "a publication naming a pre-release patch segment",
            Some("3.0.0-beta"),
            "3.0.0-beta",
        ),
        (
            // A number BELOW the latest: it is not published yet, so this publication takes
            // it, and it is what the next increment counts from.
            "a publication naming a number below the latest",
            Some("2.0.1"),
            "2.0.1",
        ),
        (
            // The minor release after 2.0.1 is 2.1.0, which the publication above took, so
            // this one takes the release after that: a public version is addressed by its
            // number, so two cannot share one.
            "a publication whose increment lands on a published number",
            None,
            "2.2.0",
        ),
    ] {
        let mut members = identified.to_vec();
        if let Some(requested) = requested {
            members.push(("PublicVersionNumber", requested));
        }
        let (status_code, body) = publish_type(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "PublicTypeArn").as_deref(),
            Some(public_arn.as_str()),
            "case {case:?}: expected {public_arn:?}, got {body}"
        );

        // `LatestPublicVersion` is "the latest version of a public extension that is
        // available for use", which is the number this publication was just assigned.
        let (_, body) = describe_type(&router, &[("Arn", &public_arn)]).await;
        let reported = xml_text(&body, "LatestPublicVersion");
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected LatestPublicVersion {expected:?}, got {reported:?}"
        );

        // And every number published so far is still addressable by it: a public version is
        // addressed BY its public version number.
        let (status_code, body) = describe_type(
            &router,
            &[("Arn", &public_arn), ("PublicVersionNumber", expected)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected version {expected:?} readable, got {status_code} {body}"
        );
        let reported = xml_text(&body, "PublicVersionNumber");
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected PublicVersionNumber {expected:?}, got {reported:?}"
        );
    }

    // Every number published above is still readable, including the first: the publications
    // accumulated on one record rather than replacing one another.
    for published in [FIRST_PUBLIC_VERSION_NUMBER, "1.1.0", "1.2.0", "2.0.0"] {
        let (status_code, body) = describe_type(
            &router,
            &[("Arn", &public_arn), ("PublicVersionNumber", published)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {published:?}: expected the published version still readable, got \
             {status_code} {body}"
        );
    }

    // A number the extension is already published under addresses a version already: a
    // second publication cannot take it.
    let published = registry_snapshot(&engine);
    for already in [
        FIRST_PUBLIC_VERSION_NUMBER,
        "1.1.0",
        "2.0.0",
        "2.0.1",
        "2.2.0",
        "3.0.0-beta",
    ] {
        let mut members = identified.to_vec();
        members.push(("PublicVersionNumber", already));
        let (status_code, body) = publish_type(&router, &members).await;
        assert_registry_refusal(
            "a version number that is already published",
            status_code,
            &body,
            &format!("Version {already} of My::Sample::Resource is already published"),
        );
        assert_eq!(
            registry_snapshot(&engine),
            published,
            "case {already:?}: expected the refused call to publish nothing"
        );
    }
}

/// A public extension belongs to the publisher that published it, so a publication whose ARN
/// another publisher's public extension already holds is refused rather than written onto
/// that record.
#[tokio::test]
async fn an_extension_another_publisher_published_is_not_published_over() {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    register_publisher(&router).await;
    let identified = [
        ("Type", registry_type::RESOURCE),
        ("TypeName", "My::Sample::Resource"),
    ];
    pass_the_contract_tests(&router, &identified).await;

    // Another publisher's public extension, at the ARN a publication of this account's
    // private registration would be addressed by. Seeded straight into the store: this
    // account holds ONE publisher registration, so no sequence of wire calls can reach a
    // second publisher.
    let public_arn = public_type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource");
    {
        let state = store(&engine);
        let mut guard = state.lock();
        let mut theirs =
            StoredExtension::new(REGION, registry_type::RESOURCE, "My::Sample::Resource");
        // A PUBLIC extension's own ARN carries no account id.
        theirs.arn = public_arn.clone();
        theirs.visibility = visibility::PUBLIC.to_string();
        theirs.publisher_id = Some("0123456789ab".to_string());
        theirs.latest_public_version = Some("3.0.0".to_string());
        let mut version = StoredTypeVersion::new(&public_arn, "00000001", SystemTime::now());
        version.arn = public_arn.clone();
        version.public_version_number = Some("3.0.0".to_string());
        theirs.versions = vec![version];
        guard.extensions.push(theirs);
    }
    // A read before the snapshot, so it is taken past the contract-test conclusion the clock
    // has already made due.
    describe_type(&router, &identified).await;
    let before = registry_snapshot(&engine);

    for (case, extra) in [
        ("naming no version number", vec![]),
        (
            "naming a version number of its own",
            vec![("PublicVersionNumber", "4.0.0")],
        ),
    ] {
        let mut members = identified.to_vec();
        members.extend_from_slice(&extra);
        let (status_code, body) = publish_type(&router, &members).await;
        assert_registry_refusal(
            case,
            status_code,
            &body,
            &format!(
                "Cannot publish My::Sample::Resource: {public_arn} is published by another \
                 publisher"
            ),
        );
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }
}

/// The versions one public extension record holds are bounded, oldest reclaimed first: a
/// publisher chooses how many publications it makes, and each one adds a version to the
/// record.
#[tokio::test]
async fn the_versions_one_public_extension_holds_are_bounded_oldest_first() {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    register_publisher(&router).await;
    let identified = [
        ("Type", registry_type::RESOURCE),
        ("TypeName", "My::Sample::Resource"),
    ];
    pass_the_contract_tests(&router, &identified).await;
    let public_arn = public_type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource");

    // Two publications past the bound, each one assigned the next minor version release.
    let publications = MAX_PUBLISHED_VERSIONS + 2;
    for publication in 1..=publications {
        let (status_code, body) = publish_type(&router, &identified).await;
        assert_eq!(
            status_code, 200,
            "case \"publication {publication}\": expected 200, got {status_code} {body}"
        );
    }

    let held = store(&engine)
        .lock()
        .published_extension(REGION, registry_type::RESOURCE, "My::Sample::Resource")
        .map(|published| published.versions.len());
    assert_eq!(
        held,
        Some(MAX_PUBLISHED_VERSIONS),
        "case \"{publications} publications\": expected the record bounded at \
         {MAX_PUBLISHED_VERSIONS} versions, got {held:?}"
    );

    // The newest publication is the one the record reports, and the two oldest numbers have
    // been reclaimed: an extension no longer published under a number is not readable at it.
    let latest = format!("1.{}.0", publications - 1);
    let (_, body) = describe_type(&router, &[("Arn", &public_arn)]).await;
    let reported = xml_text(&body, "LatestPublicVersion");
    assert_eq!(
        reported.as_deref(),
        Some(latest.as_str()),
        "case \"{publications} publications\": expected LatestPublicVersion {latest:?}, got \
         {reported:?}"
    );
    for (case, number, expected_status) in [
        (
            "the reclaimed first publication",
            FIRST_PUBLIC_VERSION_NUMBER,
            404,
        ),
        ("the reclaimed second publication", "1.1.0", 404),
        ("the oldest publication still held", "1.2.0", 200),
        ("the newest publication", latest.as_str(), 200),
    ] {
        let (status_code, body) = describe_type(
            &router,
            &[("Arn", &public_arn), ("PublicVersionNumber", number)],
        )
        .await;
        assert_eq!(
            status_code, expected_status,
            "case {case:?} ({number}): expected {expected_status}, got {status_code} {body}"
        );
    }
}
