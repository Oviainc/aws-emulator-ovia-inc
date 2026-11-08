// SPDX-License-Identifier: CC0-1.0

//! The PUBLISHER gate on `PublishType`, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **publishing is open only to a calling
//! account that holds a verified CloudFormation extension-publisher registration, and an
//! account that holds none is refused with the registry's modelled `CFNRegistryException`
//! (400) carrying `The account <caller-account-id> is not verified as a publisher. Please
//! register as publisher first` — fixed wording whose one variable part is the 12-digit
//! account id the request was signed under — BEFORE the registry is consulted: the refusal is
//! the same whatever the request identifies and whether or not a record answers to it, no
//! registration is written, and nothing about this account's registry reaches a caller the gate
//! is shut for. The one thing read ahead of the gate is the request's own shape — which
//! extension it identifies, and the VERSION an `Arn` of this account pins past that extension,
//! both properties of the request alone.**
//!
//! The table spans what the rule generalises over: every way a request identifies an extension
//! (`Type` and `TypeName`, the extension's own ARN, an ARN pinning a version, an ARN of
//! another account) over every record such an identifier can resolve to — a private
//! registration of this account and Region of each `RegistryType` (`RESOURCE`, `MODULE`,
//! `HOOK`), a public third-party extension activated here, one of the public extensions Amazon
//! Web Services publishes, a record only another Region's registry holds, a registered record
//! carrying no version at all, and an extension nothing registered anywhere — each with and
//! without a `PublicVersionNumber` beside it. Every row is driven TWICE — once as an account
//! that is not a publisher, once as one that is — so the registration is the only difference
//! between the refusal and the answer the identifier is owed, which is what makes this an
//! ORDERING assertion rather than a wording one. Two rows are the recorded traces this suite
//! was written for, both from an account that is not a publisher: an unregistered extension's
//! ARN, answered the gate, and the same ARN pinning a version, answered the ARN's own shape.
//!
//! `publish_type.rs` is the suite for what a publication WRITES and
//! `publish_type_version_in_arn.rs` the one for which refusal an `Arn` reaches; this one is the
//! gate's position in the operation. The records come
//! from `extension_registry.rs`, this crate's one registry fixture, and every call goes through
//! the real request path (form-encoded body in, XML envelope out).

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    public_type_arn, registry_type, type_arn, type_version_arn, visibility, StoredExtension,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{router_with_registry, OTHER_REGION};
use crate::publish_type::{
    assert_not_found, assert_registry_refusal, describe_type, hook_arn, module_arn,
    not_a_verified_publisher, pass_the_contract_tests, publish_type, register_publisher,
    registry_snapshot, resource_arn, seed_a_schema_for_each_default_version,
    NOT_FOUND_IN_YOUR_ACCOUNT, ONLY_DEFAULT_VERSION,
};
use crate::stack_lifecycle::{store, xml_text, REGION};

/// A registered private record the fixture does not hold: one carrying NO version at all, which
/// is the identifier the modelled 404 is the answer to once the pre-flight is open.
const VERSIONLESS: &str = "My::Versionless::Resource";

/// What the SAME request is answered once the calling account IS a verified publisher: the
/// extension-level outcome the gate was standing in front of.
enum Served {
    /// 200, naming the public extension's ARN — the record the publication wrote, which
    /// `DescribeType` reads back at that ARN.
    Published,
    /// The registry's modelled 400, carrying that message.
    Refused(String),
    /// The modelled 404, naming what the request named.
    NotFound(&'static str),
}

/// One row: what the request identifies, and what identifying it is owed once the gate is
/// open.
struct Row {
    case: &'static str,
    /// The members identifying the extension, which both passes of the row carry.
    members: Vec<(&'static str, String)>,
    /// The refusal the request's own SHAPE reaches ahead of the gate, where it has one: an
    /// `Arn` of this account pinning a version is refused from the ARN alone, so the caller's
    /// registration never comes into it.
    read_before_the_gate: Option<&'static str>,
    /// The kind and name of the extension a published row writes a public record for.
    published: Option<(&'static str, &'static str)>,
    served: Served,
}

/// The message a record the registry holds but this account did not REGISTER is refused with:
/// only a private registration of this account and Region is publishable.
fn not_a_private_extension(arn: &str) -> String {
    format!(
        "Cannot publish {arn}: only a private extension registered in this account and Region \
         can be published"
    )
}

fn name_form(type_kind: &'static str, type_name: &str) -> Vec<(&'static str, String)> {
    vec![("Type", type_kind.to_string()), ("TypeName", type_name.to_string())]
}

fn arn_form(arn: String) -> Vec<(&'static str, String)> {
    vec![("Arn", arn)]
}

/// Every identifier the gate stands in front of, with the answer it is owed once the
/// caller is a publisher.
fn rows() -> Vec<Row> {
    // The first entry of the catalogue of public extensions Amazon Web Services publishes,
    // taken from the catalogue rather than written down here.
    let aws_published = IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the catalogue holds at least one resource type");
    vec![
        Row {
            case: "a registered private resource type, by name",
            members: name_form(registry_type::RESOURCE, "My::Sample::Resource"),
            read_before_the_gate: None,
            published: Some((registry_type::RESOURCE, "My::Sample::Resource")),
            served: Served::Published,
        },
        Row {
            case: "a registered private resource type, by its ARN",
            members: arn_form(resource_arn()),
            read_before_the_gate: None,
            published: Some((registry_type::RESOURCE, "My::Sample::Resource")),
            served: Served::Published,
        },
        Row {
            case: "a registered private module, by name",
            members: name_form(registry_type::MODULE, "My::Sample::Thing::MODULE"),
            read_before_the_gate: None,
            published: Some((registry_type::MODULE, "My::Sample::Thing::MODULE")),
            served: Served::Published,
        },
        Row {
            case: "a registered private hook, by its ARN",
            members: arn_form(hook_arn()),
            read_before_the_gate: None,
            published: Some((registry_type::HOOK, "My::Sample::Hook")),
            served: Served::Published,
        },
        Row {
            // The ARN's own shape is what this one is refused for, ahead of the gate: only an
            // extension's default version is published.
            case: "a version ARN of a registered private extension of this account",
            members: arn_form(type_version_arn(&module_arn(), "00000001")),
            read_before_the_gate: Some(ONLY_DEFAULT_VERSION),
            published: None,
            served: Served::Refused(ONLY_DEFAULT_VERSION.to_string()),
        },
        Row {
            // The recorded Cause-2 class: an ARN of this account pinning a version, naming an
            // extension registered nowhere, from an account that is not a publisher.
            case: "a version ARN of an extension no registry holds",
            members: arn_form(type_version_arn(
                &type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
                "00000001",
            )),
            read_before_the_gate: Some(ONLY_DEFAULT_VERSION),
            published: None,
            served: Served::Refused(ONLY_DEFAULT_VERSION.to_string()),
        },
        Row {
            // The recorded Cause-1 class: an extension's own ARN, registered nowhere.
            case: "the ARN of an extension no registry holds",
            members: arn_form(type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource")),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            case: "a name no registry holds",
            members: name_form(registry_type::RESOURCE, "No::Such::Resource"),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            case: "an ARN of ANOTHER account",
            members: arn_form(format!(
                "arn:aws:cloudformation:{REGION}:123456789012:type/resource/My-Sample-Resource"
            )),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            // An ARN of another account addresses nothing of this one, so its version segment
            // is never read: the gate is what it reaches instead.
            case: "a version ARN of ANOTHER account",
            members: arn_form(format!(
                "arn:aws:cloudformation:{REGION}:123456789012:type/resource/\
                 My-Sample-Resource/00000001"
            )),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            case: "a record another Region's registry holds, by name",
            members: name_form(registry_type::RESOURCE, "Other::Region::Resource"),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            case: "a record another Region's registry holds, by its ARN",
            members: arn_form(type_arn(
                OTHER_REGION,
                registry_type::RESOURCE,
                "Other::Region::Resource",
            )),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(NOT_FOUND_IN_YOUR_ACCOUNT.to_string()),
        },
        Row {
            case: "a public third-party extension activated in this account, by name",
            members: name_form(registry_type::RESOURCE, "MyAlias::Sample::Resource"),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(not_a_private_extension(&type_arn(
                REGION,
                registry_type::RESOURCE,
                "MyAlias::Sample::Resource",
            ))),
        },
        Row {
            case: "a public extension Amazon Web Services publishes, by name",
            members: name_form(registry_type::RESOURCE, aws_published),
            read_before_the_gate: None,
            published: None,
            served: Served::Refused(not_a_private_extension(&public_type_arn(
                REGION,
                registry_type::RESOURCE,
                aws_published,
            ))),
        },
        Row {
            case: "a registered record carrying no version at all, by name",
            members: name_form(registry_type::RESOURCE, VERSIONLESS),
            read_before_the_gate: None,
            published: None,
            served: Served::NotFound(VERSIONLESS),
        },
    ]
}

/// A store holding the registry fixture plus the versionless record, with every private
/// record's default version carrying a schema so the publishable rows can pass their contract
/// tests.
fn router_for_a_row() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_registry();
    seed_a_schema_for_each_default_version(&engine);
    {
        let state = store(&engine);
        state.lock().extensions.push(StoredExtension::new(
            REGION,
            registry_type::RESOURCE,
            VERSIONLESS,
        ));
    }
    (router, engine)
}

/// The gate on the CALLER is read before the registry is consulted: every row of the table is
/// refused with the account-naming sentence while the account holds no registration — except
/// the rows the request's own shape settles ahead of it — and is answered what its identifier
/// is owed once it holds one.
#[tokio::test]
async fn an_account_that_is_not_a_verified_publisher_is_refused_before_the_registry_is_consulted()
{
    for Row {
        case,
        members,
        read_before_the_gate,
        published,
        served,
    } in rows()
    {
        // One store per row: a publication is a write, so a row must not read what another row
        // published.
        let (router, engine) = router_for_a_row();
        let identified: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let before = registry_snapshot(&engine);

        // The gate, with and without a version number beside the identifier: the number
        // is not what the refusal is about, so neither spelling changes it.
        for extra in [vec![], vec![("PublicVersionNumber", "1.0.0")]] {
            let mut requested = identified.clone();
            requested.extend_from_slice(&extra);
            let refused = format!("{case} (not a publisher, extra members: {extra:?})");
            let expected = read_before_the_gate
                .map_or_else(not_a_verified_publisher, ToString::to_string);
            let (status_code, body) = publish_type(&router, &requested).await;
            assert_registry_refusal(&refused, status_code, &body, &expected);
            assert_eq!(
                registry_snapshot(&engine),
                before,
                "case {refused:?}: expected the refused call to publish nothing"
            );
            assert!(
                store(&engine).lock().publisher.is_none(),
                "case {refused:?}: expected the refused call to register no publisher, got {:?}",
                store(&engine).lock().publisher
            );
        }

        // The registration is the only difference: the same request, from an account that holds
        // one, is answered what its identifier is owed.
        register_publisher(&router).await;
        match served {
            Served::Published => {
                let (type_kind, type_name) =
                    published.expect("a published row names the record it publishes");
                pass_the_contract_tests(&router, &identified).await;
                let (status_code, body) = publish_type(&router, &identified).await;
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected the publication served with 200, got {status_code} \
                     {body}"
                );
                // "The Amazon Resource Name (ARN) assigned to the public extension upon
                // publication": a public extension's ARN carries no account id.
                let expected_arn = public_type_arn(REGION, type_kind, type_name);
                let reported = xml_text(&body, "PublicTypeArn");
                assert_eq!(
                    reported.as_deref(),
                    Some(expected_arn.as_str()),
                    "case {case:?}: expected PublicTypeArn {expected_arn:?}, got {reported:?}"
                );
                // The record the publication wrote, read back at the ARN it answered: the
                // gate was in front of a real publication, not of a refusal.
                let (status_code, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected the published extension readable, got {status_code} \
                     {body}"
                );
                let reported = xml_text(&body, "Visibility");
                assert_eq!(
                    reported.as_deref(),
                    Some(visibility::PUBLIC),
                    "case {case:?}: expected a public record read back, got {reported:?}"
                );
            }
            Served::Refused(expected) => {
                let (status_code, body) = publish_type(&router, &identified).await;
                let served_case = format!("{case} (a verified publisher)");
                assert_registry_refusal(&served_case, status_code, &body, &expected);
            }
            Served::NotFound(identifier) => {
                let (status_code, body) = publish_type(&router, &identified).await;
                let served_case = format!("{case} (a verified publisher)");
                assert_not_found(&served_case, status_code, &body, identifier);
            }
        }
    }
}

/// WHAT the request identifies is a property of the request alone, so it is read before the
/// gate on the caller: a request that identifies no extension, or two, is refused for that
/// whether or not the calling account is a publisher.
#[tokio::test]
async fn a_request_that_identifies_no_extension_is_read_before_the_caller_gate() {
    let (router, engine) = router_for_a_row();
    let before = registry_snapshot(&engine);

    for registered_publisher in [false, true] {
        if registered_publisher {
            register_publisher(&router).await;
        }
        for (case, members, expected) in [
            (
                "a request that identifies no extension at all",
                vec![],
                "Either Arn, or both Type and TypeName, must be specified to identify an \
                 extension",
            ),
            (
                "half of the name form",
                vec![("Type", registry_type::RESOURCE.to_string())],
                "Either Arn, or both Type and TypeName, must be specified to identify an \
                 extension",
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
            let requested: Vec<(&str, &str)> = members
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect();
            let case = format!("{case} (publisher registered: {registered_publisher})");
            let (status_code, body) = publish_type(&router, &requested).await;
            assert_registry_refusal(&case, status_code, &body, expected);
            assert_eq!(
                registry_snapshot(&engine),
                before,
                "case {case:?}: expected the refused call to publish nothing"
            );
        }
    }
}
