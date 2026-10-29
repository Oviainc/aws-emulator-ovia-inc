// SPDX-License-Identifier: CC0-1.0

//! `SetTypeDefaultVersion` over the awsQuery wire: which version of a private extension of
//! this account and Region the operations that name none use.
//!
//! The rule every case here holds the service to: **`SetTypeDefaultVersion` makes the version
//! the request names — by `VersionId` or by an extension-version `Arn` — the
//! `DefaultVersionId` of the registration the request identifies — by `Arn`, or by `Type` and
//! `TypeName` — in this account and Region, so that version is the one a later request naming
//! none resolves to and the only one reported as the default. What it can name is a private
//! registration of THIS Region holding that version in active use; anything else is the
//! modelled `TypeNotFoundException` (404) — a public extension, a record another Region holds,
//! a registration already fully deprecated, a version the record does not hold in active use,
//! and a name or ARN no record carries — and a request identifying no extension, or naming no
//! version of one, is the modelled `CFNRegistryException` (400). A refusal writes nothing, and
//! a write leaves every other record and every version exactly as it was.**
//!
//! The table spans the class the rule generalises over: both ways a request can identify an
//! extension (`Type` and `TypeName`, the extension's own ARN) and both ways it can name a
//! version (`VersionId`, an extension-version ARN), over registrations of every extension kind
//! the model admits (`RESOURCE`, `MODULE`, `HOOK`), with version counts of one and of three, a
//! record that carries no default version yet, a record whose default moves away from the
//! version that held it, and a version that is already the default; and, on the refusal side,
//! over every kind of record the registry can hold that is not a live private registration of
//! this Region, every way a version can fail to resolve, and every way the conditional input
//! rules can be broken. The recorded trace this suite was written for is ONE row of the refusal
//! table: a call naming a type name the registry holds nothing for.
//!
//! `RegisterType` is still a scaffold stub with its own issue, so the registrations below are
//! seeded straight into the store — the same device `extension_registry.rs`,
//! `type_deactivation.rs` and `type_deregistration.rs` use. Every call goes through the real
//! request path (form-encoded body in, XML envelope out), so what is asserted is what a client
//! sees, and what a write left is read BACK through `DescribeType`, the read AWS documents for
//! the registry ("to view the current ... data for an extension, refer to ... DescribeType").

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    provisioning_type, public_type_arn, registry_type, type_arn, type_version_arn, visibility,
    StoredExtension, StoredTypeVersion,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{OTHER_REGION, PUBLISHER_ID};
use crate::harness::{call, member, router_with_engine, store, xml_text, REGION};

/// A private RESOURCE registration with THREE versions in active use, the FIRST its default:
/// the record every "move the default to another version" row is read against.
const THREE_VERSION_RESOURCE: &str = "Multi::Sample::Resource";
const THREE_VERSION_IDS: [&str; 3] = ["00000001", "00000002", "00000003"];
const THREE_VERSION_DEFAULT: &str = "00000001";

/// A private RESOURCE registration holding a DEPRECATED version between two in active use: a
/// version out of active use is one no CloudFormation operation can use, so it is no version
/// this call can make the default, while the live one beside it is.
const MIXED_VERSION_RESOURCE: &str = "Mixed::Sample::Resource";
const MIXED_VERSION_IDS: [&str; 3] = ["00000001", "00000002", "00000003"];
const MIXED_DEPRECATED_VERSION: &str = "00000002";

/// A private MODULE registration with a SINGLE version, which is already its default.
/// `TypeName`'s pattern admits the trailing `::MODULE` segment a module's name carries.
const MODULE_TYPE_NAME: &str = "My::Sample::Thing::MODULE";
const MODULE_VERSION_ID: &str = "00000001";

/// A private HOOK registration that carries NO `DefaultVersionId` yet, and whose one version
/// id is not the record's first: a version is addressed by the id the record carries, never by
/// its position.
const HOOK_TYPE_NAME: &str = "My::Sample::Hook";
const HOOK_VERSION_ID: &str = "00000007";

/// A private registration ANOTHER Region's registry holds: the registry is regional, so this
/// one is not addressable from [`REGION`].
const OTHER_REGION_TYPE_NAME: &str = "Other::Region::Resource";

/// A public third-party extension this account activated, which is no private registration.
const ACTIVATED_TYPE_NAME: &str = "MyAlias::Sample::Resource";

/// A private registration every version of which is already deprecated: an extension already
/// deregistered.
const ALREADY_DEREGISTERED: &str = "Already::Sample::Resource";

/// The version id no seeded record holds, for the rows that name a version that cannot resolve.
const UNHELD_VERSION_ID: &str = "00009999";

/// The refusal the registry answers a request that states two identities for one call.
const TWO_IDENTITIES: &str = "Specify either Arn, or Type and TypeName, but not both: an \
                              extension is identified by its ARN or by its kind and name";

/// The refusal the registry answers a request that identifies no extension at all.
const NO_IDENTITY: &str =
    "Either Arn, or both Type and TypeName, must be specified to identify an extension";

/// The refusal the registry answers a request that identifies an extension but names no
/// version of it to use as its default.
const NO_VERSION: &str = "A VersionId must be specified to name the version of the extension \
                          to use as its default version";

/// An instant to register the seeded versions at, so nothing a case asserts comes from the
/// clock.
fn registered_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset_secs)
}

/// The description the seeded version `version_id` of `type_name` carries: what `DescribeType`
/// reports for the version it describes, so the version a read naming none resolves to is
/// identifiable in the response.
fn version_description(type_name: &str, version_id: &str) -> String {
    format!("{type_name}, version {version_id}")
}

/// One private registration of this account and Region: "CloudFormation marks any extensions
/// you register as PRIVATE", holding `version_ids` versions with `default` as the version "used
/// when the extension version isn't specified" — or none at all where `default` is `None`.
fn registration(
    region: &str,
    type_kind: &str,
    type_name: &str,
    version_ids: &[&str],
    default: Option<&str>,
) -> StoredExtension {
    let mut extension = StoredExtension::new(region, type_kind, type_name);
    let arn = extension.arn.clone();
    extension.default_version_id = default.map(str::to_string);
    extension.versions = version_ids
        .iter()
        .enumerate()
        .map(|(position, version_id)| {
            let mut version =
                StoredTypeVersion::new(&arn, version_id, registered_at(position as u64 * 60));
            version.description = Some(version_description(type_name, version_id));
            version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
            version
        })
        .collect();
    extension
}

/// [`MIXED_VERSION_RESOURCE`]: a registration with one of its versions already out of active
/// use, deprecated through the registry's own writer.
fn mixed_version_registration() -> StoredExtension {
    let mut extension = registration(
        REGION,
        registry_type::RESOURCE,
        MIXED_VERSION_RESOURCE,
        &MIXED_VERSION_IDS,
        Some(MIXED_VERSION_IDS[0]),
    );
    extension
        .version_mut(MIXED_DEPRECATED_VERSION)
        .expect("the seeded record holds the version it deprecates")
        .deprecate();
    extension
}

/// A registration whose every version is already deprecated, built through the same writer a
/// deregistration of the whole extension goes through.
fn already_deregistered() -> StoredExtension {
    let mut extension = registration(
        REGION,
        registry_type::RESOURCE,
        ALREADY_DEREGISTERED,
        &["00000001"],
        Some("00000001"),
    );
    extension.deprecate_every_version();
    extension
}

/// A public THIRD-PARTY extension activated in this account under a type name alias, which is
/// no private registration at all.
fn activated_public_extension() -> StoredExtension {
    let mut extension = registration(
        REGION,
        registry_type::RESOURCE,
        ACTIVATED_TYPE_NAME,
        &["00000001"],
        None,
    );
    extension.visibility = visibility::PUBLIC.to_string();
    extension.publisher_id = Some(PUBLISHER_ID.to_string());
    extension.original_type_name = Some("Publisher::Sample::Resource".to_string());
    extension.original_type_arn = Some(public_type_arn(
        REGION,
        registry_type::RESOURCE,
        "Publisher::Sample::Resource",
    ));
    extension.is_activated = Some(true);
    extension.auto_update = Some(true);
    extension
}

/// One of the extensions Amazon Web Services publishes, taken from the catalogue
/// `state::registry::aws_published` reports them from
/// (`state::template::IMPLEMENTED_RESOURCE_TYPES`) rather than restated here: it is public, so
/// it is no record this operation can name.
fn aws_published_type_name() -> &'static str {
    IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the registry publishes at least one resource type")
}

/// Every record the seeded registry holds.
fn seeded_records() -> Vec<StoredExtension> {
    vec![
        registration(
            REGION,
            registry_type::RESOURCE,
            THREE_VERSION_RESOURCE,
            &THREE_VERSION_IDS,
            Some(THREE_VERSION_DEFAULT),
        ),
        mixed_version_registration(),
        registration(
            REGION,
            registry_type::MODULE,
            MODULE_TYPE_NAME,
            &[MODULE_VERSION_ID],
            Some(MODULE_VERSION_ID),
        ),
        // A record that carries no `DefaultVersionId` of its own yet.
        registration(
            REGION,
            registry_type::HOOK,
            HOOK_TYPE_NAME,
            &[HOOK_VERSION_ID],
            None,
        ),
        registration(
            OTHER_REGION,
            registry_type::RESOURCE,
            OTHER_REGION_TYPE_NAME,
            &["00000001"],
            Some("00000001"),
        ),
        activated_public_extension(),
        already_deregistered(),
    ]
}

/// A router over a store seeded with every record above.
fn router_with_registrations() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        state.lock().extensions = seeded_records();
    }
    (router, engine)
}

async fn set_type_default_version(
    router: &MultiServiceRouter,
    members: &[(&str, String)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "SetTypeDefaultVersion", &members).await
}

/// Every record the registry holds right now, as `(ARN, the whole record)`: what a case
/// compares before and after a call, so a write it did not ask for is visible.
fn records(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, String)> {
    let state = store(engine);
    let locked = state.lock();
    locked
        .extensions
        .iter()
        .map(|extension| (extension.arn.clone(), format!("{extension:?}")))
        .collect()
}

/// What `DescribeType` reports of the record `type_kind` / `type_name` for each of
/// `version_ids`, as `(version id, DefaultVersionId, IsDefaultVersion)`: the write read BACK
/// through the registry read, version by version.
async fn reported_defaults(
    router: &MultiServiceRouter,
    type_kind: &str,
    type_name: &str,
    version_ids: &[&str],
) -> Vec<(String, Option<String>, Option<String>)> {
    let mut reported = Vec::new();
    for version_id in version_ids {
        let members = [
            member("Type", type_kind),
            member("TypeName", type_name),
            member("VersionId", version_id),
        ];
        let (status_code, body) = call(router, "DescribeType", &members).await;
        assert_eq!(
            status_code, 200,
            "reading {type_name} version {version_id} back: expected 200, got {status_code} \
             {body}"
        );
        reported.push((
            version_id.to_string(),
            xml_text(&body, "DefaultVersionId"),
            xml_text(&body, "IsDefaultVersion"),
        ));
    }
    reported
}

/// What those reports must be once `default` is the record's default version: every version
/// reports the same `DefaultVersionId`, and exactly the default one is `IsDefaultVersion`.
fn expected_defaults(
    version_ids: &[&str],
    default: &str,
) -> Vec<(String, Option<String>, Option<String>)> {
    version_ids
        .iter()
        .map(|version_id| {
            (
                version_id.to_string(),
                Some(default.to_string()),
                Some((*version_id == default).to_string()),
            )
        })
        .collect()
}

/// The description `DescribeType` reports for the record `type_kind` / `type_name` when the
/// read names NO version: "it returns information about the default extension version", so
/// this is how the write is read back through a request that names no version of its own.
async fn description_of_the_default_version(
    router: &MultiServiceRouter,
    type_kind: &str,
    type_name: &str,
) -> Option<String> {
    let members = [member("Type", type_kind), member("TypeName", type_name)];
    let (status_code, body) = call(router, "DescribeType", &members).await;
    assert_eq!(
        status_code, 200,
        "reading the default version of {type_name} back: expected 200, got {status_code} {body}"
    );
    xml_text(&body, "Description")
}

/// Hold every record other than `arn` to exactly what it was before the call.
fn assert_only_one_record_changed(
    case: &str,
    before: &[(String, String)],
    after: &[(String, String)],
    arn: &str,
) {
    assert_eq!(
        before.len(),
        after.len(),
        "case {case:?}: expected the registry to keep its {} records, got {}",
        before.len(),
        after.len()
    );
    for ((before_arn, before_record), (after_arn, after_record)) in before.iter().zip(after) {
        assert_eq!(
            before_arn, after_arn,
            "case {case:?}: expected the records in the order they entered the registry, got \
             {after_arn:?} where {before_arn:?} was"
        );
        if after_arn == arn {
            continue;
        }
        assert_eq!(
            before_record, after_record,
            "case {case:?}: expected {after_arn:?} left as it was, got {after_record:?}"
        );
    }
}

/// One row of the default-version table: a request that must be accepted, the record it writes,
/// and which version of that record must be its default afterwards.
struct DefaultVersionCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    type_kind: &'static str,
    type_name: &'static str,
    version_ids: &'static [&'static str],
    default: &'static str,
}

/// Both ways a request can identify an extension and both ways it can name a version, over
/// every extension kind, both version counts, a record carrying no default version yet and a
/// version that is already the default.
fn default_version_cases() -> Vec<DefaultVersionCase> {
    let three_version_arn = type_arn(REGION, registry_type::RESOURCE, THREE_VERSION_RESOURCE);
    vec![
        DefaultVersionCase {
            case: "a three-version RESOURCE, to its last version, by Type and TypeName",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", THREE_VERSION_RESOURCE.to_string()),
                ("VersionId", THREE_VERSION_IDS[2].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: THREE_VERSION_RESOURCE,
            version_ids: &THREE_VERSION_IDS,
            default: THREE_VERSION_IDS[2],
        },
        DefaultVersionCase {
            case: "a three-version RESOURCE, to its middle version, by the extension's ARN \
                   and VersionId",
            members: vec![
                ("Arn", three_version_arn.clone()),
                ("VersionId", THREE_VERSION_IDS[1].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: THREE_VERSION_RESOURCE,
            version_ids: &THREE_VERSION_IDS,
            default: THREE_VERSION_IDS[1],
        },
        DefaultVersionCase {
            case: "a three-version RESOURCE, by the ARN of the version alone",
            members: vec![(
                "Arn",
                type_version_arn(&three_version_arn, THREE_VERSION_IDS[2]),
            )],
            type_kind: registry_type::RESOURCE,
            type_name: THREE_VERSION_RESOURCE,
            version_ids: &THREE_VERSION_IDS,
            default: THREE_VERSION_IDS[2],
        },
        DefaultVersionCase {
            case: "a three-version RESOURCE, by a version ARN and the VersionId it pins",
            members: vec![
                (
                    "Arn",
                    type_version_arn(&three_version_arn, THREE_VERSION_IDS[1]),
                ),
                ("VersionId", THREE_VERSION_IDS[1].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: THREE_VERSION_RESOURCE,
            version_ids: &THREE_VERSION_IDS,
            default: THREE_VERSION_IDS[1],
        },
        DefaultVersionCase {
            case: "the version that is ALREADY the record's default, by Type and TypeName",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", THREE_VERSION_RESOURCE.to_string()),
                ("VersionId", THREE_VERSION_DEFAULT.to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: THREE_VERSION_RESOURCE,
            version_ids: &THREE_VERSION_IDS,
            default: THREE_VERSION_DEFAULT,
        },
        DefaultVersionCase {
            case: "a version in active use of a record that also holds a deprecated one",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", MIXED_VERSION_RESOURCE.to_string()),
                ("VersionId", MIXED_VERSION_IDS[2].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: MIXED_VERSION_RESOURCE,
            version_ids: &MIXED_VERSION_IDS,
            default: MIXED_VERSION_IDS[2],
        },
        DefaultVersionCase {
            case: "the single version of a MODULE registration, by Type and TypeName",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", MODULE_TYPE_NAME.to_string()),
                ("VersionId", MODULE_VERSION_ID.to_string()),
            ],
            type_kind: registry_type::MODULE,
            type_name: MODULE_TYPE_NAME,
            version_ids: &[MODULE_VERSION_ID],
            default: MODULE_VERSION_ID,
        },
        DefaultVersionCase {
            case: "a HOOK registration that carried no default version yet, by its ARN",
            members: vec![
                ("Arn", type_arn(REGION, registry_type::HOOK, HOOK_TYPE_NAME)),
                ("VersionId", HOOK_VERSION_ID.to_string()),
            ],
            type_kind: registry_type::HOOK,
            type_name: HOOK_TYPE_NAME,
            version_ids: &[HOOK_VERSION_ID],
            default: HOOK_VERSION_ID,
        },
    ]
}

#[tokio::test]
async fn set_type_default_version_makes_the_named_version_the_records_default() {
    for DefaultVersionCase {
        case,
        members,
        type_kind,
        type_name,
        version_ids,
        default,
    } in default_version_cases()
    {
        let (router, engine) = router_with_registrations();
        let before = records(&engine);
        let arn = type_arn(REGION, type_kind, type_name);

        let (status_code, body) = set_type_default_version(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the call accepted with 200, got {status_code} {body}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {case:?}: expected a success envelope, got {body:?}"
        );

        let reported = reported_defaults(&router, type_kind, type_name, version_ids).await;
        let expected = expected_defaults(version_ids, default);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reported defaults {expected:?}, got {reported:?}"
        );

        // "The default version of an extension will be used in CloudFormation operations": a
        // read that names no version of its own resolves to exactly that version.
        let described = description_of_the_default_version(&router, type_kind, type_name).await;
        let expected = version_description(type_name, default);
        assert_eq!(
            described.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected a read naming no version to describe {expected:?}, got \
             {described:?}"
        );

        assert_only_one_record_changed(case, &before, &records(&engine), &arn);
    }
}

/// One row of the refusal table: a request that must be refused, with the status, the code and
/// the message it must be refused with.
struct RefusedCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
    message: String,
}

/// Every kind of record that is not a live private registration of this Region, every way a
/// version can fail to resolve, and every way the conditional input rules can be broken.
fn refused_cases() -> Vec<RefusedCase> {
    let not_found = |case: &'static str, members: Vec<(&'static str, String)>, named: &str| {
        RefusedCase {
            case,
            members,
            status: 404,
            code: "TypeNotFoundException",
            message: format!("The type '{named}' cannot be found."),
        }
    };
    let refused = |case: &'static str, members: Vec<(&'static str, String)>, message: String| {
        RefusedCase {
            case,
            members,
            status: 400,
            code: "CFNRegistryException",
            message,
        }
    };
    let by_name_and_version = |type_kind: &str, type_name: &str, version_id: &str| {
        vec![
            ("Type", type_kind.to_string()),
            ("TypeName", type_name.to_string()),
            ("VersionId", version_id.to_string()),
        ]
    };
    let three_version_arn = type_arn(REGION, registry_type::RESOURCE, THREE_VERSION_RESOURCE);
    let unknown_arn = type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource");
    let other_region_arn = type_arn(
        OTHER_REGION,
        registry_type::RESOURCE,
        OTHER_REGION_TYPE_NAME,
    );

    vec![
        not_found(
            "a type name no record carries",
            by_name_and_version(registry_type::RESOURCE, "No::Such::Resource", "00000001"),
            "No::Such::Resource",
        ),
        not_found(
            "a registration another Region's registry holds",
            by_name_and_version(registry_type::RESOURCE, OTHER_REGION_TYPE_NAME, "00000001"),
            OTHER_REGION_TYPE_NAME,
        ),
        not_found(
            "a public third-party extension activated in this account",
            by_name_and_version(registry_type::RESOURCE, ACTIVATED_TYPE_NAME, "00000001"),
            ACTIVATED_TYPE_NAME,
        ),
        not_found(
            "an extension Amazon Web Services published, which is public",
            by_name_and_version(registry_type::RESOURCE, aws_published_type_name(), "00000001"),
            aws_published_type_name(),
        ),
        not_found(
            "a registration every version of which is already deprecated",
            by_name_and_version(registry_type::RESOURCE, ALREADY_DEREGISTERED, "00000001"),
            ALREADY_DEREGISTERED,
        ),
        not_found(
            "a registered name, under another extension kind",
            by_name_and_version(registry_type::HOOK, THREE_VERSION_RESOURCE, "00000001"),
            THREE_VERSION_RESOURCE,
        ),
        not_found(
            // An extension's ARN carries the extension's NAME, so a refusal that names what
            // the request identified names that name — the same sentence the name form gets.
            "an ARN no record carries",
            vec![
                ("Arn", unknown_arn.clone()),
                ("VersionId", "00000001".to_string()),
            ],
            "No::Such::Resource",
        ),
        not_found(
            "the ARN of a registration another Region's registry holds",
            vec![
                ("Arn", other_region_arn.clone()),
                ("VersionId", "00000001".to_string()),
            ],
            OTHER_REGION_TYPE_NAME,
        ),
        not_found(
            "a VersionId the record holds no version under",
            by_name_and_version(
                registry_type::RESOURCE,
                THREE_VERSION_RESOURCE,
                UNHELD_VERSION_ID,
            ),
            &format!("{THREE_VERSION_RESOURCE}/{UNHELD_VERSION_ID}"),
        ),
        not_found(
            "the ARN of a version the record does not hold",
            vec![(
                "Arn",
                type_version_arn(&three_version_arn, UNHELD_VERSION_ID),
            )],
            &format!("{three_version_arn}/{UNHELD_VERSION_ID}"),
        ),
        not_found(
            "a version the record holds DEPRECATED, out of active use",
            by_name_and_version(
                registry_type::RESOURCE,
                MIXED_VERSION_RESOURCE,
                MIXED_DEPRECATED_VERSION,
            ),
            &format!("{MIXED_VERSION_RESOURCE}/{MIXED_DEPRECATED_VERSION}"),
        ),
        refused(
            "a registration identified, with no version named at all",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", THREE_VERSION_RESOURCE.to_string()),
            ],
            NO_VERSION.to_string(),
        ),
        refused(
            "a registration identified by its ARN alone, with no version named",
            vec![("Arn", three_version_arn.clone())],
            NO_VERSION.to_string(),
        ),
        refused(
            "a request identifying no extension at all",
            vec![("VersionId", "00000001".to_string())],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "Type without TypeName",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("VersionId", "00000001".to_string()),
            ],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "TypeName without Type",
            vec![
                ("TypeName", THREE_VERSION_RESOURCE.to_string()),
                ("VersionId", "00000001".to_string()),
            ],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "Arn beside Type and TypeName",
            vec![
                ("Arn", three_version_arn.clone()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", THREE_VERSION_RESOURCE.to_string()),
                ("VersionId", "00000001".to_string()),
            ],
            TWO_IDENTITIES.to_string(),
        ),
        refused(
            "a VersionId naming another version than the Arn does",
            vec![
                (
                    "Arn",
                    type_version_arn(&three_version_arn, THREE_VERSION_IDS[0]),
                ),
                ("VersionId", THREE_VERSION_IDS[2].to_string()),
            ],
            format!(
                "VersionId {} does not name the extension version the Arn does ({})",
                THREE_VERSION_IDS[2], THREE_VERSION_IDS[0]
            ),
        ),
    ]
}

#[tokio::test]
async fn set_type_default_version_refuses_what_is_no_live_version_of_a_registration_here() {
    let (router, engine) = router_with_registrations();

    for RefusedCase {
        case,
        members,
        status,
        code,
        message,
    } in refused_cases()
    {
        let before = records(&engine);
        let (status_code, body) = set_type_default_version(&router, &members).await;
        assert_eq!(
            status_code, status,
            "case {case:?}: expected the call refused with {status}, got {status_code} {body}"
        );
        let reported_code = xml_text(&body, "Code");
        assert_eq!(
            reported_code.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {reported_code:?} from {body}"
        );
        let reported = xml_text(&body, "Message");
        assert_eq!(
            reported.as_deref(),
            Some(message.as_str()),
            "case {case:?}: expected {message:?}, got {reported:?}"
        );
        assert_eq!(
            before,
            records(&engine),
            "case {case:?}: expected a refusal to write nothing"
        );
    }
}

/// A record carries ONE default version, so moving it again moves it away from the version that
/// held it: each call leaves the version it named as the only default, and the version a read
/// naming none resolves to follows it.
#[tokio::test]
async fn the_default_version_a_record_carries_moves_with_each_call() {
    let (router, _engine) = router_with_registrations();

    for version_id in [
        THREE_VERSION_IDS[2],
        THREE_VERSION_IDS[1],
        THREE_VERSION_IDS[0],
    ] {
        let case = format!("making version {version_id} the default");
        let members = [
            ("Type", registry_type::RESOURCE.to_string()),
            ("TypeName", THREE_VERSION_RESOURCE.to_string()),
            ("VersionId", version_id.to_string()),
        ];
        let (status_code, body) = set_type_default_version(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the call accepted with 200, got {status_code} {body}"
        );

        let reported = reported_defaults(
            &router,
            registry_type::RESOURCE,
            THREE_VERSION_RESOURCE,
            &THREE_VERSION_IDS,
        )
        .await;
        let expected = expected_defaults(&THREE_VERSION_IDS, version_id);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reported defaults {expected:?}, got {reported:?}"
        );

        let described = description_of_the_default_version(
            &router,
            registry_type::RESOURCE,
            THREE_VERSION_RESOURCE,
        )
        .await;
        let expected = version_description(THREE_VERSION_RESOURCE, version_id);
        assert_eq!(
            described.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected a read naming no version to describe {expected:?}, got \
             {described:?}"
        );
    }
}
