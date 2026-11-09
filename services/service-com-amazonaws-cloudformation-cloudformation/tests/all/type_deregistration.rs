// SPDX-License-Identifier: CC0-1.0

//! `DeregisterType` over the awsQuery wire: deregistering a private extension of this
//! account and Region, or one version of it.
//!
//! The rule every case here holds the service to: **`DeregisterType` marks what the request
//! identifies — by `Arn`, or by `Type` and `TypeName`, at the version `VersionId` or an
//! extension-version ARN names — `DEPRECATED` in this account and Region's registry. A
//! request naming a VERSION deprecates that version alone, and it must be one in active use
//! and not the record's DEFAULT version, the version in use (the modelled
//! `CFNRegistryException`, 400, whose message names the remedy). A request naming NO version
//! deprecates every version of the extension, which it can do only once no more than one is
//! left in active use — "to deregister an entire extension, you must individually deregister
//! all active versions of that extension" — and is otherwise the same modelled error. What the
//! operation can name is a private registration of THIS Region still holding a version in
//! active use; anything else is the modelled `TypeNotFoundException` (404) — a public
//! extension, a record another Region holds, a registration already fully deprecated, a
//! version the record does not hold in active use, and a name or ARN no record carries — and
//! a request identifying no extension at all is the modelled `CFNRegistryException` (400). A
//! refusal writes nothing, and a deregistration leaves every other record exactly as it
//! was.**
//!
//! Which form takes the DEFAULT version out of active use is recorded behavior: the version
//! form refuses it whether or not another version is still in use, so the extension form is
//! what deprecates it — and "if you do deregister the default version of an extension, the
//! extension type itself is deregistered as well and marked as deprecated" is the outcome
//! that form leaves. The rows below hold the service to that division in both directions.
//!
//! The table spans the class the rule generalises over: both ways a request can identify an
//! extension (`Type` and `TypeName`, the extension's own ARN) and both ways it can name a
//! version (`VersionId`, an extension-version ARN), over registrations of every extension
//! kind the model admits (`RESOURCE`, `MODULE`, `HOOK`), with version counts of one, two and
//! three so a record holding one version in active use is distinguishable from one holding
//! several and a default version from a non-default one; and, on the refusal side, over every
//! kind of
//! record the registry can hold that is not a live private registration of this Region, every
//! way a version can fail to resolve, and every way the conditional input rules can be
//! broken. The recorded trace this suite was written for is ONE row of the refusal table: a
//! deregistration of a type name the registry holds nothing for.
//!
//! `RegisterType`, the registry's one WRITER still a scaffold stub, has an issue of its own,
//! so the registrations below are seeded straight into the store — the same device
//! `extension_registry.rs` and `type_deactivation.rs` use.
//! Every call goes through the real request path (form-encoded body in, XML envelope out), so
//! what is asserted is what a client sees, and what a deregistration wrote is read BACK
//! through `DescribeType` ("to view the deprecation status of an extension or extension
//! version, use DescribeType").

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    deprecated_status, provisioning_type, public_type_arn, registry_type, type_arn,
    type_version_arn, visibility, StoredExtension, StoredTypeVersion,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{OTHER_REGION, PUBLISHER_ID};
use crate::harness::{call, member, router_with_engine, store, xml_text, REGION};

/// A private RESOURCE registration with THREE versions, the middle one its default: the
/// record a non-default version, a default version with others still in use, and an
/// individually-deregistered-down-to-the-last-version case are all read against.
const MULTI_VERSION_RESOURCE: &str = "Multi::Sample::Resource";

/// A private RESOURCE registration with a SINGLE version, which is its default: "if an
/// extension has only a single active version, deregistering that version results in the
/// extension itself being deregistered".
const SINGLE_VERSION_RESOURCE: &str = "Single::Sample::Resource";

/// A private MODULE registration. `TypeName`'s pattern admits the trailing `::MODULE`
/// segment a module's name carries.
const MODULE_TYPE_NAME: &str = "My::Sample::Thing::MODULE";

/// A private HOOK registration.
const HOOK_TYPE_NAME: &str = "My::Sample::Hook";

/// A private registration ANOTHER Region's registry holds: the registry is regional, so this
/// one is not addressable from [`REGION`].
const OTHER_REGION_TYPE_NAME: &str = "Other::Region::Resource";

/// A public third-party extension this account activated, which `DeactivateType` is what
/// removes from use.
const ACTIVATED_TYPE_NAME: &str = "MyAlias::Sample::Resource";

/// A private registration every version of which is already deprecated: an extension already
/// deregistered.
const ALREADY_DEREGISTERED: &str = "Already::Sample::Resource";

/// The version ids [`MULTI_VERSION_RESOURCE`] holds, and the one that is its default.
const MULTI_VERSION_IDS: [&str; 3] = ["00000001", "00000002", "00000003"];
const MULTI_DEFAULT_VERSION_ID: &str = "00000002";

/// The single version id [`SINGLE_VERSION_RESOURCE`] holds.
const SINGLE_VERSION_ID: &str = "00000001";

/// The version ids the MODULE registration holds, and its default.
const MODULE_VERSION_IDS: [&str; 2] = ["00000001", "00000002"];

/// The one version id the HOOK registration holds, which is not the record's first: a version
/// is addressed by the id the record carries, never by its position.
const HOOK_VERSION_ID: &str = "00000007";

/// The refusal the registry answers a request that states two identities for one call.
const TWO_IDENTITIES: &str = "Specify either Arn, or Type and TypeName, but not both: an \
                              extension is identified by its ARN or by its kind and name";

/// The refusal the registry answers a request that identifies no extension at all.
const NO_IDENTITY: &str =
    "Either Arn, or both Type and TypeName, must be specified to identify an extension";

/// The refusal the registry answers a request to deregister the WHOLE extension while more
/// than one version of it is still in active use: entirely fixed text, so every row that
/// reaches it expects the same sentence.
const MORE_THAN_ONE_ACTIVE_VERSION: &str =
    "This type has more than one active version. Please deregister non-default active versions \
     before attempting to deregister the type.";

/// An instant to register the seeded versions at, so nothing a case asserts comes from the
/// clock.
fn registered_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset_secs)
}

/// One private registration of this account and Region: "CloudFormation marks any extensions
/// you register as PRIVATE", with `version_ids` versions of it and `default` as the version
/// "used when the extension version isn't specified".
fn registration(
    region: &str,
    type_kind: &str,
    type_name: &str,
    version_ids: &[&str],
    default: &str,
) -> StoredExtension {
    let mut extension = StoredExtension::new(region, type_kind, type_name);
    let arn = extension.arn.clone();
    extension.default_version_id = Some(default.to_string());
    extension.versions = version_ids
        .iter()
        .enumerate()
        .map(|(position, version_id)| {
            let mut version =
                StoredTypeVersion::new(&arn, version_id, registered_at(position as u64 * 60));
            version.description = Some(format!("{type_name}, version {version_id}"));
            version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
            version
        })
        .collect();
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
        "00000001",
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
        "00000001",
    );
    extension.visibility = visibility::PUBLIC.to_string();
    extension.default_version_id = None;
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
            MULTI_VERSION_RESOURCE,
            &MULTI_VERSION_IDS,
            MULTI_DEFAULT_VERSION_ID,
        ),
        registration(
            REGION,
            registry_type::RESOURCE,
            SINGLE_VERSION_RESOURCE,
            &[SINGLE_VERSION_ID],
            SINGLE_VERSION_ID,
        ),
        registration(
            REGION,
            registry_type::MODULE,
            MODULE_TYPE_NAME,
            &MODULE_VERSION_IDS,
            MODULE_VERSION_IDS[0],
        ),
        registration(
            REGION,
            registry_type::HOOK,
            HOOK_TYPE_NAME,
            &[HOOK_VERSION_ID],
            HOOK_VERSION_ID,
        ),
        registration(
            OTHER_REGION,
            registry_type::RESOURCE,
            OTHER_REGION_TYPE_NAME,
            &["00000001"],
            "00000001",
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

async fn deregister_type(
    router: &MultiServiceRouter,
    members: &[(&str, String)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DeregisterType", &members).await
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

/// The deprecation status `DescribeType` reports for every version of the record `type_kind` /
/// `type_name`, as `(version id, status)` in the order the record carries them: the
/// deregistration read BACK through the registry read AWS documents for it.
async fn reported_statuses(
    router: &MultiServiceRouter,
    type_kind: &str,
    type_name: &str,
    version_ids: &[&str],
) -> Vec<(String, String)> {
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
        let status = xml_text(&body, "DeprecatedStatus").unwrap_or_else(|| {
            panic!("reading {type_name} version {version_id} back: no DeprecatedStatus in {body}")
        });
        reported.push((version_id.to_string(), status));
    }
    reported
}

/// What a case expects of a record's versions afterwards, in the record's own order.
fn expected_statuses(version_ids: &[&str], deprecated: &[&str]) -> Vec<(String, String)> {
    version_ids
        .iter()
        .map(|version_id| {
            let status = if deprecated.contains(version_id) {
                deprecated_status::DEPRECATED
            } else {
                deprecated_status::LIVE
            };
            (version_id.to_string(), status.to_string())
        })
        .collect()
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

/// One row of the deregistration table: a request that must be accepted, the record it writes,
/// and which of that record's versions must be deprecated afterwards.
struct DeregistrationCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    type_kind: &'static str,
    type_name: &'static str,
    version_ids: &'static [&'static str],
    deprecated: Vec<&'static str>,
}

/// Both ways a request can identify an extension and both ways it can name a version, over
/// every extension kind and both version counts.
fn deregistration_cases() -> Vec<DeregistrationCase> {
    let multi_arn = type_arn(REGION, registry_type::RESOURCE, MULTI_VERSION_RESOURCE);
    vec![
        DeregistrationCase {
            case: "a single-version RESOURCE registration, by Type and TypeName",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", SINGLE_VERSION_RESOURCE.to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: SINGLE_VERSION_RESOURCE,
            version_ids: &[SINGLE_VERSION_ID],
            deprecated: vec![SINGLE_VERSION_ID],
        },
        DeregistrationCase {
            case: "one NON-default version of a two-version MODULE registration, by the ARN \
                   it carries in this Region",
            members: vec![
                (
                    "Arn",
                    type_arn(REGION, registry_type::MODULE, MODULE_TYPE_NAME),
                ),
                ("VersionId", MODULE_VERSION_IDS[1].to_string()),
            ],
            type_kind: registry_type::MODULE,
            type_name: MODULE_TYPE_NAME,
            version_ids: &MODULE_VERSION_IDS,
            deprecated: vec![MODULE_VERSION_IDS[1]],
        },
        DeregistrationCase {
            case: "a single-version HOOK registration, by Type and TypeName",
            members: vec![
                ("Type", registry_type::HOOK.to_string()),
                ("TypeName", HOOK_TYPE_NAME.to_string()),
            ],
            type_kind: registry_type::HOOK,
            type_name: HOOK_TYPE_NAME,
            version_ids: &[HOOK_VERSION_ID],
            deprecated: vec![HOOK_VERSION_ID],
        },
        DeregistrationCase {
            case: "one NON-default version, by Type, TypeName and VersionId",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
                ("VersionId", MULTI_VERSION_IDS[0].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: MULTI_VERSION_RESOURCE,
            version_ids: &MULTI_VERSION_IDS,
            deprecated: vec![MULTI_VERSION_IDS[0]],
        },
        DeregistrationCase {
            case: "one NON-default version, by the ARN of that version",
            members: vec![(
                "Arn",
                type_version_arn(&multi_arn, MULTI_VERSION_IDS[2]),
            )],
            type_kind: registry_type::RESOURCE,
            type_name: MULTI_VERSION_RESOURCE,
            version_ids: &MULTI_VERSION_IDS,
            deprecated: vec![MULTI_VERSION_IDS[2]],
        },
        DeregistrationCase {
            case: "one NON-default version, by a version ARN and the VersionId it pins",
            members: vec![
                ("Arn", type_version_arn(&multi_arn, MULTI_VERSION_IDS[0])),
                ("VersionId", MULTI_VERSION_IDS[0].to_string()),
            ],
            type_kind: registry_type::RESOURCE,
            type_name: MULTI_VERSION_RESOURCE,
            version_ids: &MULTI_VERSION_IDS,
            deprecated: vec![MULTI_VERSION_IDS[0]],
        },
        DeregistrationCase {
            case: "a single-version HOOK registration, by the ARN it carries in this Region",
            members: vec![(
                "Arn",
                type_arn(REGION, registry_type::HOOK, HOOK_TYPE_NAME),
            )],
            type_kind: registry_type::HOOK,
            type_name: HOOK_TYPE_NAME,
            version_ids: &[HOOK_VERSION_ID],
            deprecated: vec![HOOK_VERSION_ID],
        },
    ]
}

#[tokio::test]
async fn deregister_type_deprecates_what_the_request_identifies() {
    for DeregistrationCase {
        case,
        members,
        type_kind,
        type_name,
        version_ids,
        deprecated,
    } in deregistration_cases()
    {
        let (router, engine) = router_with_registrations();
        let before = records(&engine);
        let arn = type_arn(REGION, type_kind, type_name);

        let (status_code, body) = deregister_type(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the deregistration accepted with 200, got {status_code} \
             {body}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {case:?}: expected a success envelope, got {body:?}"
        );

        let reported = reported_statuses(&router, type_kind, type_name, version_ids).await;
        let expected = expected_statuses(version_ids, &deprecated);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the deprecation statuses {expected:?}, got {reported:?}"
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
    let by_name = |type_kind: &str, type_name: &str| {
        vec![
            ("Type", type_kind.to_string()),
            ("TypeName", type_name.to_string()),
        ]
    };
    let multi_arn = type_arn(REGION, registry_type::RESOURCE, MULTI_VERSION_RESOURCE);
    let unknown_arn = type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource");
    let other_region_arn = type_arn(
        OTHER_REGION,
        registry_type::RESOURCE,
        OTHER_REGION_TYPE_NAME,
    );

    vec![
        not_found(
            "a type name no record carries",
            by_name(registry_type::RESOURCE, "No::Such::Resource"),
            "No::Such::Resource",
        ),
        not_found(
            "a registration another Region's registry holds",
            by_name(registry_type::RESOURCE, OTHER_REGION_TYPE_NAME),
            OTHER_REGION_TYPE_NAME,
        ),
        not_found(
            "a public third-party extension activated in this account",
            by_name(registry_type::RESOURCE, ACTIVATED_TYPE_NAME),
            ACTIVATED_TYPE_NAME,
        ),
        not_found(
            "an extension Amazon Web Services published, which is public",
            by_name(registry_type::RESOURCE, aws_published_type_name()),
            aws_published_type_name(),
        ),
        not_found(
            "a registration every version of which is already deprecated",
            by_name(registry_type::RESOURCE, ALREADY_DEREGISTERED),
            ALREADY_DEREGISTERED,
        ),
        not_found(
            "a registered name, under another extension kind",
            by_name(registry_type::HOOK, MULTI_VERSION_RESOURCE),
            MULTI_VERSION_RESOURCE,
        ),
        not_found(
            // An extension's ARN carries the extension's NAME, so a refusal that names what
            // the request identified names that name — the same sentence the name form gets.
            "an ARN no record carries",
            vec![("Arn", unknown_arn.clone())],
            "No::Such::Resource",
        ),
        not_found(
            "the ARN of a registration another Region's registry holds",
            vec![("Arn", other_region_arn.clone())],
            OTHER_REGION_TYPE_NAME,
        ),
        not_found(
            "a VersionId the record holds no version under",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
                ("VersionId", "00009999".to_string()),
            ],
            &format!("{MULTI_VERSION_RESOURCE}/00009999"),
        ),
        not_found(
            "the ARN of a version the record does not hold",
            vec![("Arn", type_version_arn(&multi_arn, "00009999"))],
            &format!("{multi_arn}/00009999"),
        ),
        refused(
            "the DEFAULT version while other versions are still in active use",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
                ("VersionId", MULTI_DEFAULT_VERSION_ID.to_string()),
            ],
            format!(
                "Version '{MULTI_DEFAULT_VERSION_ID}' is the default version and cannot be \
                 deregistered. Deregister the resource type '{MULTI_VERSION_RESOURCE}' \
                 instead."
            ),
        ),
        refused(
            // Recorded behavior: the version form refuses the default version whether or not
            // another version is in active use — the remedy it names, the extension form, is
            // what takes the last version out of use.
            "the DEFAULT version of a record that holds only that one",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", SINGLE_VERSION_RESOURCE.to_string()),
                ("VersionId", SINGLE_VERSION_ID.to_string()),
            ],
            format!(
                "Version '{SINGLE_VERSION_ID}' is the default version and cannot be \
                 deregistered. Deregister the resource type '{SINGLE_VERSION_RESOURCE}' \
                 instead."
            ),
        ),
        refused(
            "the DEFAULT version by the ARN of that version",
            vec![(
                "Arn",
                type_version_arn(&multi_arn, MULTI_DEFAULT_VERSION_ID),
            )],
            format!(
                "Version '{MULTI_DEFAULT_VERSION_ID}' is the default version and cannot be \
                 deregistered. Deregister the resource type '{MULTI_VERSION_RESOURCE}' \
                 instead."
            ),
        ),
        refused(
            "the WHOLE extension while three versions are still in active use",
            by_name(registry_type::RESOURCE, MULTI_VERSION_RESOURCE),
            MORE_THAN_ONE_ACTIVE_VERSION.to_string(),
        ),
        refused(
            "the WHOLE extension, by its ARN, while two versions are still in active use",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::MODULE, MODULE_TYPE_NAME),
            )],
            MORE_THAN_ONE_ACTIVE_VERSION.to_string(),
        ),
        refused(
            "a request identifying no extension at all",
            vec![],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "Type without TypeName",
            vec![("Type", registry_type::RESOURCE.to_string())],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "TypeName without Type",
            vec![("TypeName", MULTI_VERSION_RESOURCE.to_string())],
            NO_IDENTITY.to_string(),
        ),
        refused(
            "Arn beside Type and TypeName",
            vec![
                ("Arn", multi_arn.clone()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
            ],
            TWO_IDENTITIES.to_string(),
        ),
        refused(
            "a VersionId naming another version than the Arn does",
            vec![
                ("Arn", type_version_arn(&multi_arn, MULTI_VERSION_IDS[0])),
                ("VersionId", MULTI_VERSION_IDS[2].to_string()),
            ],
            format!(
                "VersionId {} does not name the extension version the Arn does ({})",
                MULTI_VERSION_IDS[2], MULTI_VERSION_IDS[0]
            ),
        ),
    ]
}

#[tokio::test]
async fn deregister_type_refuses_what_is_no_live_registration_of_this_account_and_region() {
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
        let (status_code, body) = deregister_type(&router, &members).await;
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

/// "To deregister an entire extension, you must individually deregister all active versions of
/// that extension": the non-default versions go one at a time, and the extension itself goes
/// last — the default version, the one in use, never goes on its own, so the extension form is
/// what takes it out of use and leaves the record deregistered.
#[tokio::test]
async fn versions_deregistered_one_at_a_time_leave_the_extension_deregistered() {
    let (router, _engine) = router_with_registrations();
    let by_version = |version_id: &str| {
        vec![
            ("Type", registry_type::RESOURCE.to_string()),
            ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
            ("VersionId", version_id.to_string()),
        ]
    };
    let whole_extension = vec![
        ("Type", registry_type::RESOURCE.to_string()),
        ("TypeName", MULTI_VERSION_RESOURCE.to_string()),
    ];

    // The extension cannot go first: three of its versions are still in active use.
    let case = "deregistering the extension while every version is still in use";
    let (status_code, body) = deregister_type(&router, &whole_extension).await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );
    let reported = xml_text(&body, "Message");
    assert_eq!(
        reported.as_deref(),
        Some(MORE_THAN_ONE_ACTIVE_VERSION),
        "case {case:?}: expected {MORE_THAN_ONE_ACTIVE_VERSION:?}, got {reported:?}"
    );

    // The two non-default versions, in the order the record carries them.
    for version_id in [MULTI_VERSION_IDS[0], MULTI_VERSION_IDS[2]] {
        let case = format!("deregistering the non-default version {version_id}");
        let (status_code, body) = deregister_type(&router, &by_version(version_id)).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the deregistration accepted with 200, got {status_code} \
             {body}"
        );
    }

    // The default version is the only one left in active use, and it still cannot go on its
    // own: the refusal names the remedy, which is to deregister the extension.
    let case = "deregistering the default version, now the last in active use";
    let (status_code, body) =
        deregister_type(&router, &by_version(MULTI_DEFAULT_VERSION_ID)).await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );

    // The extension itself, now that no other version is in active use.
    let case = "deregistering the extension, its default version the last in use";
    let (status_code, body) = deregister_type(&router, &whole_extension).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the deregistration accepted with 200, got {status_code} {body}"
    );

    let reported = reported_statuses(
        &router,
        registry_type::RESOURCE,
        MULTI_VERSION_RESOURCE,
        &MULTI_VERSION_IDS,
    )
    .await;
    let expected = expected_statuses(&MULTI_VERSION_IDS, &MULTI_VERSION_IDS);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected every version deprecated ({expected:?}), got {reported:?}"
    );

    // Nothing of the extension is left in active use, so a further deregistration of it names
    // nothing: the record is deregistered.
    let case = "deregistering the extension again, after every version has gone";
    let (status_code, body) = deregister_type(&router, &whole_extension).await;
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the call refused with 404, got {status_code} {body}"
    );
    let reported = xml_text(&body, "Message");
    let expected = format!("The type '{MULTI_VERSION_RESOURCE}' cannot be found.");
    assert_eq!(
        reported.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
}
