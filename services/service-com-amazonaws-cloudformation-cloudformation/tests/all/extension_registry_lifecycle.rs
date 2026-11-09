// SPDX-License-Identifier: CC0-1.0

//! The extension registry's two LIFECYCLES over the awsQuery wire: the public extensions a
//! publisher published, which an account activates and deactivates, and the private
//! registrations an account deregisters.
//!
//! Three rules, one test each:
//!
//! 1. **The public registry holds the extensions published to it independently of the caller's
//!    account, so `ListTypes` over the public/third-party registry answers a non-empty set
//!    whose every entry is describable at the ARN it reported, activatable by that ARN, and
//!    deactivatable afterwards. An activation is this account's own copy: it is visible and
//!    usable in this account and Region (`PRIVATE`), it carries the published extension it was
//!    made from, and deactivating it leaves it `DEPRECATED` and out of the activated
//!    listing.**
//! 2. **`DeregisterType` divides the registry's deletion between its two forms: the version
//!    form deregisters a version that is in active use and is NOT the record's default version
//!    — the version in use, which recorded behavior refuses whether or not another version is
//!    still in use — and the extension form deregisters the record, which it can do only once
//!    no more than one version is left in active use. Each refusal is the modelled
//!    `CFNRegistryException` (400) carrying the sentence that states the remedy. A record with
//!    no version left in active use is deregistered: a read that names no version of it
//!    answers the modelled `TypeNotFoundException` (404), while a read naming one of its
//!    versions still reports that version's `DEPRECATED` status.**
//! 3. **A public extension's ARN addresses the extension by the NAME in the name's position —
//!    `type/<kind>/<publisher id>/<Type-Name>`, or `type/<kind>/<Type-Name>` for an extension
//!    Amazon Web Services published, which is assigned no publisher id — so an ARN carrying no
//!    extension type name there resolves no extension and is refused with the registry's own
//!    `CFNRegistryException` (400) `TypeName is invalid`, whatever else the segments look
//!    like.**
//!
//! Each table spans what its rule generalises over: rule 1 over every third-party RESOURCE
//! extension the registry publishes (read from
//! `crate::state::registry::PUBLIC_EXTENSION_CATALOG` rather than named here, so the suite
//! cannot drift from the catalogue); rule 2 over records holding one, two and three versions
//! in active use, with the default version both first and in the middle of the record; rule 3
//! over each extension kind the model admits and each shape of ARN that puts something other
//! than a type name in the name's position.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    deprecated_status, published_catalog_arn, published_publisher_id, registry_type, type_arn,
    type_version_arn, visibility, PublishedExtension, StoredExtension, StoredTypeVersion,
    FIRST_PUBLIC_VERSION_NUMBER, INVALID_TYPE_NAME, PUBLIC_EXTENSION_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router, router_with_engine, store, xml_blocks, xml_text, REGION};

/// The execution role an activation of a public RESOURCE extension must name, inside the
/// modelled `RoleARN2` pattern `^arn:.+:iam::[0-9]{12}:role/.+$`.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/extension-execution";

/// The code every refusal below carries: "an error occurred during a CloudFormation registry
/// operation".
const REGISTRY_EXCEPTION: &str = "CFNRegistryException";

/// The public name of the publisher Amazon Web Services publishes its own extensions under,
/// which is how the catalogue's entries are told apart from the THIRD-PARTY ones.
const AWS_PUBLISHER_NAME: &str = "Amazon Web Services";

/// Every third-party RESOURCE extension the registry publishes: the candidates a client
/// discovering the public registry can activate.
///
/// Source: `crate::state::registry::PUBLIC_EXTENSION_CATALOG`, narrowed the way
/// `ListTypes(Visibility=PUBLIC, Type=RESOURCE, Filters={Category: THIRD_PARTY})` narrows the
/// registry — so a catalogue entry added later is covered by this suite without editing it.
fn published_third_party_resources() -> Vec<&'static PublishedExtension> {
    PUBLIC_EXTENSION_CATALOG
        .iter()
        .filter(|published| {
            published.type_kind == registry_type::RESOURCE
                && published.publisher_name != AWS_PUBLISHER_NAME
        })
        .collect()
}

/// One `ListTypes` read, as the summaries it reported.
async fn list_summaries(router: &MultiServiceRouter, members: &[(&str, &str)]) -> Vec<String> {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    let (status_code, body) = call(router, "ListTypes", &members).await;
    assert_eq!(
        status_code, 200,
        "listing the registry: expected 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
}

/// Read one member out of a `DescribeType` response that must succeed.
async fn describe(
    case: &str,
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> String {
    let (status_code, body) = call(router, "DescribeType", members).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the extension readable, got {status_code} {body}"
    );
    body
}

#[tokio::test]
async fn every_published_third_party_resource_activates_and_deactivates() {
    let candidates = published_third_party_resources();
    assert!(
        !candidates.is_empty(),
        "the public registry must publish at least one third-party RESOURCE extension for a \
         client to discover, got {:?}",
        PUBLIC_EXTENSION_CATALOG
            .iter()
            .map(|published| published.type_name)
            .collect::<Vec<_>>()
    );

    for published in candidates {
        // A fresh account per row, so each activation is the first one its registry saw.
        let router = router();
        let case = published.type_name;
        let published_arn = published_catalog_arn(REGION, published);
        let publisher_id = published_publisher_id(published.publisher_name);

        // Discovery: the public/third-party RESOURCE registry reports the extension, with the
        // ARN a client hands back to the activation.
        let discovered = list_summaries(
            &router,
            &[
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::RESOURCE),
                ("Filters.Category", "THIRD_PARTY"),
                ("Filters.TypeNamePrefix", published.type_name),
            ],
        )
        .await;
        assert_eq!(
            discovered.len(),
            1,
            "case {case:?}: expected the public registry to report exactly one candidate, got \
             {discovered:?}"
        );
        assert_eq!(
            xml_text(&discovered[0], "TypeArn").as_deref(),
            Some(published_arn.as_str()),
            "case {case:?}: expected the published extension's own ARN, got {:?}",
            discovered[0]
        );
        assert_eq!(
            xml_text(&discovered[0], "PublisherId").as_deref(),
            Some(publisher_id.as_str()),
            "case {case:?}: expected the publisher that published it, got {:?}",
            discovered[0]
        );

        // The ARN the listing reported describes the published extension, which nothing has
        // activated yet.
        let described = describe(case, &router, &[member("Arn", &published_arn)]).await;
        for (tag, expected) in [
            ("Visibility", visibility::PUBLIC.to_string()),
            ("TypeName", published.type_name.to_string()),
            ("PublisherId", publisher_id.clone()),
            ("IsActivated", "false".to_string()),
            (
                "PublicVersionNumber",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
        ] {
            let actual = xml_text(&described, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} from {described}"
            );
        }

        // The activation, by the ARN the listing reported.
        let (status_code, body) = call(
            &router,
            "ActivateType",
            &[
                member("PublicTypeArn", &published_arn),
                member("ExecutionRoleArn", EXECUTION_ROLE_ARN),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the activation accepted, got {status_code} {body}"
        );
        let activated_arn = type_arn(REGION, registry_type::RESOURCE, published.type_name);
        assert_eq!(
            xml_text(&body, "Arn").as_deref(),
            Some(activated_arn.as_str()),
            "case {case:?}: expected the activation's own ARN, got {body}"
        );

        // Round trip: the activation is this account's copy — usable in this account and
        // Region, carrying the published extension it was made from.
        let described = describe(case, &router, &[member("Arn", &activated_arn)]).await;
        for (tag, expected) in [
            ("Visibility", visibility::PRIVATE.to_string()),
            ("IsActivated", "true".to_string()),
            ("DeprecatedStatus", deprecated_status::LIVE.to_string()),
            ("OriginalTypeName", published.type_name.to_string()),
            ("OriginalTypeArn", published_arn.clone()),
            ("PublisherId", publisher_id.clone()),
            (
                "PublicVersionNumber",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
        ] {
            let actual = xml_text(&described, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} from {described}"
            );
        }

        // The activated listing of this account and Region reports it.
        let activated = list_summaries(
            &router,
            &[
                ("Visibility", visibility::PRIVATE),
                ("Filters.Category", "ACTIVATED"),
                ("Filters.TypeNamePrefix", published.type_name),
            ],
        )
        .await;
        assert_eq!(
            activated.len(),
            1,
            "case {case:?}: expected the activation listed in this account and Region, got \
             {activated:?}"
        );
        assert_eq!(
            xml_text(&activated[0], "TypeArn").as_deref(),
            Some(activated_arn.as_str()),
            "case {case:?}: expected the activation's own ARN listed, got {:?}",
            activated[0]
        );

        // The deactivation, and what it leaves behind.
        let (status_code, body) = call(
            &router,
            "DeactivateType",
            &[
                member("Type", registry_type::RESOURCE),
                member("TypeName", published.type_name),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the deactivation accepted, got {status_code} {body}"
        );
        let described = describe(case, &router, &[member("Arn", &activated_arn)]).await;
        for (tag, expected) in [
            ("IsActivated", "false".to_string()),
            ("DeprecatedStatus", deprecated_status::DEPRECATED.to_string()),
            // Still this account's own copy: leaving active use does not put the record into
            // any other account's reach, so the scope it is reported in is unchanged.
            ("Visibility", visibility::PRIVATE.to_string()),
        ] {
            let actual = xml_text(&described, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{tag}> {expected:?} after the deactivation, got \
                 {actual:?} from {described}"
            );
        }
        let activated = list_summaries(
            &router,
            &[
                ("Visibility", visibility::PRIVATE),
                ("Filters.Category", "ACTIVATED"),
                ("Filters.TypeNamePrefix", published.type_name),
            ],
        )
        .await;
        assert!(
            activated.is_empty(),
            "case {case:?}: expected the deactivated extension unlisted, got {activated:?}"
        );
    }
}

/// The message the registry refuses a request to deregister the WHOLE extension while more
/// than one version of it is in active use with: entirely fixed text.
const MORE_THAN_ONE_ACTIVE_VERSION: &str =
    "This type has more than one active version. Please deregister non-default active versions \
     before attempting to deregister the type.";

/// The message the registry refuses a request to deregister a record's DEFAULT version with:
/// the version the request named, and the extension to deregister instead.
///
/// The noun is part of the FIXED text rather than of the record's kind — the recorded refusal
/// words it "the resource type" for a type name ending in `::MODULE` — which is why the rows
/// below assert the same sentence for a module and a Hook.
fn default_version_refusal(version_id: &str, type_name: &str) -> String {
    format!(
        "Version '{version_id}' is the default version and cannot be deregistered. Deregister \
         the resource type '{type_name}' instead."
    )
}

/// One row of the deregistration table: a private registration holding `version_ids` versions
/// in active use, `default` among them.
struct RegistrationCase {
    case: &'static str,
    type_kind: &'static str,
    type_name: &'static str,
    version_ids: &'static [&'static str],
    default: &'static str,
}

fn registration_cases() -> Vec<RegistrationCase> {
    vec![
        RegistrationCase {
            case: "one version, which is the default",
            type_kind: registry_type::RESOURCE,
            type_name: "Lifecycle::One::Resource",
            version_ids: &["00000001"],
            default: "00000001",
        },
        RegistrationCase {
            case: "two versions, the first the default",
            type_kind: registry_type::MODULE,
            type_name: "Lifecycle::Two::Thing::MODULE",
            version_ids: &["00000001", "00000002"],
            default: "00000001",
        },
        RegistrationCase {
            case: "three versions, the middle one the default",
            type_kind: registry_type::HOOK,
            type_name: "Lifecycle::Three::Hook",
            version_ids: &["00000001", "00000002", "00000003"],
            default: "00000002",
        },
    ]
}

/// One private registration of this account and Region, seeded straight into the store: the
/// registrations a deregistration is driven against come from a handler package a client
/// stages in Amazon S3, which this suite is not about.
fn seeded_registration(row: &RegistrationCase) -> StoredExtension {
    let mut extension = StoredExtension::new(REGION, row.type_kind, row.type_name);
    let arn = extension.arn.clone();
    extension.default_version_id = Some(row.default.to_string());
    extension.versions = row
        .version_ids
        .iter()
        .enumerate()
        .map(|(position, version_id)| {
            let registered_at = SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000 + position as u64 * 60);
            StoredTypeVersion::new(&arn, version_id, registered_at)
        })
        .collect();
    extension
}

/// The `(Code, Message)` of a `DeregisterType` call that must be refused with 400.
async fn deregistration_refusal(
    case: &str,
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (String, String) {
    let (status_code, body) = call(router, "DeregisterType", members).await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the deregistration refused with 400, got {status_code} {body}"
    );
    (
        xml_text(&body, "Code").unwrap_or_default(),
        xml_text(&body, "Message").unwrap_or_default(),
    )
}

#[tokio::test]
async fn the_deregistration_protocol_holds_over_every_version_count() {
    for row in registration_cases() {
        // A fresh registry per row: a deregistration is a write.
        let (router, engine) = router_with_engine();
        store(&engine).lock().extensions = vec![seeded_registration(&row)];
        let case = row.case;
        let by_name = vec![
            member("Type", row.type_kind),
            member("TypeName", row.type_name),
        ];
        let non_default: Vec<&&str> = row
            .version_ids
            .iter()
            .filter(|version_id| **version_id != row.default)
            .collect();

        // The extension itself cannot go while more than one version is in active use.
        if row.version_ids.len() > 1 {
            let refused = deregistration_refusal(case, &router, &by_name).await;
            assert_eq!(
                refused,
                (
                    REGISTRY_EXCEPTION.to_string(),
                    MORE_THAN_ONE_ACTIVE_VERSION.to_string()
                ),
                "case {case:?}: expected the extension form refused while {} versions are in \
                 active use",
                row.version_ids.len()
            );
        }

        // The DEFAULT version never goes on its own, whatever else is in active use: the
        // refusal names it and the extension to deregister instead.
        let mut default_members = by_name.clone();
        default_members.push(member("VersionId", row.default));
        let refused = deregistration_refusal(case, &router, &default_members).await;
        assert_eq!(
            refused,
            (
                REGISTRY_EXCEPTION.to_string(),
                default_version_refusal(row.default, row.type_name)
            ),
            "case {case:?}: expected the default version's own refusal"
        );

        // Every NON-default version goes one at a time.
        for version_id in &non_default {
            let mut members = by_name.clone();
            members.push(member("VersionId", version_id));
            let (status_code, body) = call(&router, "DeregisterType", &members).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected version {version_id} deregistered, got {status_code} \
                 {body}"
            );
        }

        // With only the default version left in active use, the extension itself goes.
        let (status_code, body) = call(&router, "DeregisterType", &by_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the extension deregistered once its default version was \
             the last in active use, got {status_code} {body}"
        );

        // What the deregistration leaves: the extension is no longer in the registry for a
        // read that names no version of it, while each version still reports its own status.
        let (status_code, body) = call(&router, "DescribeType", &by_name).await;
        assert_eq!(
            status_code, 404,
            "case {case:?}: expected the deregistered extension not found, got {status_code} \
             {body}"
        );
        assert_eq!(
            (
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default()
            ),
            (
                "TypeNotFoundException".to_string(),
                format!("The type '{}' cannot be found.", row.type_name)
            ),
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );
        for version_id in row.version_ids {
            let mut members = by_name.clone();
            members.push(member("VersionId", version_id));
            let described = describe(case, &router, &members).await;
            let reported = xml_text(&described, "DeprecatedStatus");
            assert_eq!(
                reported.as_deref(),
                Some(deprecated_status::DEPRECATED),
                "case {case:?}: expected version {version_id} readable as DEPRECATED, got \
                 {reported:?} from {described}"
            );
            // The version's own ARN is the one the registry addresses it by.
            let arn = type_version_arn(
                &type_arn(REGION, row.type_kind, row.type_name),
                version_id,
            );
            assert_eq!(
                xml_text(&described, "Arn").as_deref(),
                Some(arn.as_str()),
                "case {case:?}: expected version {version_id} at its own ARN, got {described}"
            );
        }
    }
}

/// One row of the ARN table: the `type/` path a request names the public extension by, and
/// whether the registry resolves an extension from it.
struct ArnShapeCase {
    case: &'static str,
    /// The path after `type/`, exactly as the row gives it.
    path: String,
    /// `Some(local name)` for an ARN the activation resolves an extension from.
    activates: Option<&'static str>,
}

fn arn_shape_cases() -> Vec<ArnShapeCase> {
    // A publisher id inside the modelled `PublisherId` pattern `^[0-9a-zA-Z]{12,40}$`.
    const PUBLISHER_ID: &str = "abcdef0123456789abcd";
    vec![
        ArnShapeCase {
            case: "a version id where the name belongs, under a segment that is no publisher \
                   id — the shape a client naming the extension and its version builds",
            path: "resource/SomeVendor-Thing-Thing/00000001".to_string(),
            activates: None,
        },
        ArnShapeCase {
            case: "a version number where the name belongs, for a module",
            path: "module/SomeVendor-Thing-Thing-MODULE/1.0.0".to_string(),
            activates: None,
        },
        ArnShapeCase {
            case: "a publisher id where the name belongs, for a Hook",
            path: format!("hook/{PUBLISHER_ID}"),
            activates: None,
        },
        ArnShapeCase {
            case: "a name of two segments only, under a publisher",
            path: format!("resource/{PUBLISHER_ID}/SomeVendor-Thing"),
            activates: None,
        },
        ArnShapeCase {
            case: "control — a resource extension under its publisher",
            path: format!("resource/{PUBLISHER_ID}/SomeVendor-Store-Bucket"),
            activates: Some("SomeVendor::Store::Bucket"),
        },
        ArnShapeCase {
            case: "control — a module under its publisher, whose name carries MODULE",
            path: format!("module/{PUBLISHER_ID}/SomeVendor-Network-Subnets-MODULE"),
            activates: Some("SomeVendor::Network::Subnets::MODULE"),
        },
        ArnShapeCase {
            case: "control — a Hook an extension publisher published",
            path: format!("hook/{PUBLISHER_ID}/SomeVendor-Guard-Hook"),
            activates: Some("SomeVendor::Guard::Hook"),
        },
    ]
}

#[tokio::test]
async fn an_arn_carrying_no_type_name_in_the_names_position_resolves_no_extension() {
    for row in arn_shape_cases() {
        // A fresh account per row, so each activation is the first one its registry saw.
        let router = router();
        let arn = format!("arn:aws:cloudformation:{REGION}::type/{}", row.path);
        let (status_code, body) = call(
            &router,
            "ActivateType",
            &[
                member("PublicTypeArn", &arn),
                // Named on every row: a public RESOURCE activation needs one, and no other
                // kind is refused for carrying it.
                member("ExecutionRoleArn", EXECUTION_ROLE_ARN),
            ],
        )
        .await;
        let case = row.case;

        match row.activates {
            None => {
                assert_eq!(
                    status_code, 400,
                    "case {case:?}: expected {arn} refused, got {status_code} {body}"
                );
                assert_eq!(
                    (
                        xml_text(&body, "Code").unwrap_or_default(),
                        xml_text(&body, "Message").unwrap_or_default()
                    ),
                    (
                        REGISTRY_EXCEPTION.to_string(),
                        INVALID_TYPE_NAME.to_string()
                    ),
                    "case {case:?}: expected the registry's own refusal of a name it cannot \
                     resolve, got {body}"
                );
            }
            Some(local_name) => {
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected {arn} to activate, got {status_code} {body}"
                );
                let kind = row
                    .path
                    .split('/')
                    .next()
                    .expect("the path carries a kind")
                    .to_ascii_uppercase();
                let activated_arn = type_arn(REGION, &kind, local_name);
                assert_eq!(
                    xml_text(&body, "Arn").as_deref(),
                    Some(activated_arn.as_str()),
                    "case {case:?}: expected the activation held under the name the ARN \
                     carried, got {body}"
                );
                let described = describe(case, &router, &[member("Arn", &activated_arn)]).await;
                assert_eq!(
                    xml_text(&described, "TypeName").as_deref(),
                    Some(local_name),
                    "case {case:?}: expected it readable under that name, got {described}"
                );
            }
        }
    }
}
