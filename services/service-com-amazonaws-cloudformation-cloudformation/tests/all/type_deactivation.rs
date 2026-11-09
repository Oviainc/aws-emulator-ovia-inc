// SPDX-License-Identifier: CC0-1.0

//! `DeactivateType` over the awsQuery wire: deactivating a public third-party extension
//! this account and Region has activated.
//!
//! The rule every case here holds the service to: **`DeactivateType` deactivates the
//! ACTIVATION this account and Region holds for the public third-party extension the request
//! identifies — by `Arn`, or by `Type` and `TypeName` — deleting the configuration data held
//! for it and leaving it unusable: `IsActivated` and `AutoUpdate` false, and every version of
//! it `DEPRECATED`. Anything else a request can name is the modelled `TypeNotFoundException`
//! (404) — a record that is not an activated public third-party extension of THIS Region, and
//! a name or an ARN no record carries — and a request that identifies no extension at all is
//! the modelled `CFNRegistryException` (400); a refusal writes nothing, and a deactivation
//! leaves every other record in the registry exactly as it was.**
//!
//! The table spans the class the rule generalises over: both ways a request can identify an
//! extension (`Type` and `TypeName`, the extension's own ARN), over activations of every
//! extension kind the model admits (`RESOURCE`, `MODULE`, `HOOK`), with a version count above
//! one so "every version" is more than one row of the record; and, on the refusal side, over
//! every kind of record the registry can hold that is NOT an activation of this Region
//! (a private registration, an extension Amazon Web Services published, an activation already
//! deactivated, an activation another Region holds, a name under the wrong kind, an ARN naming
//! a version) and every way the conditional input rule can be broken. The recorded trace this
//! suite was written for is ONE row of the refusal table: a deactivation of a type name the
//! registry holds nothing for.
//!
//! The registry's other WRITERS (`RegisterType`, `ActivateType`, `SetTypeConfiguration`) are
//! still scaffold stubs with their own issues, so the activations below are seeded straight
//! into the store — the same device `extension_registry.rs` uses for records a bound's worth
//! of wire calls could not create. Every call goes through the real request path
//! (form-encoded body in, XML envelope out), so what is asserted is what a client sees, and
//! what a deactivation wrote is read BACK through `DescribeType`.

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

/// The alias the RESOURCE activation addressed by `Type` and `TypeName` is activated under:
/// "CloudFormation treats the alias as the extension type name within this account and
/// Region".
const RESOURCE_ALIAS: &str = "MyAlias::Sample::Resource";

/// A second RESOURCE activation, for the row that addresses one by its ARN: the two forms
/// select independently, so each gets a record of its own.
const ARN_RESOURCE_ALIAS: &str = "Another::Sample::Resource";

/// The MODULE activation. `TypeName`'s pattern admits the trailing `::MODULE` segment a
/// module's name carries.
const MODULE_ALIAS: &str = "MyAlias::Sample::Thing::MODULE";

/// The HOOK activation.
const HOOK_ALIAS: &str = "MyAlias::Sample::Hook";

/// An activation ANOTHER Region's registry holds: the registry is regional, so this one is
/// not addressable from [`REGION`].
const OTHER_REGION_ALIAS: &str = "Other::Region::Resource";

/// A private extension this account registered, which is no activation at all.
const PRIVATE_TYPE_NAME: &str = "My::Sample::Resource";

/// An activation this account has already deactivated.
const DEACTIVATED_ALIAS: &str = "Already::Sample::Resource";

/// The refusal the registry answers a request that states two identities for one call.
const TWO_IDENTITIES: &str = "Specify either Arn, or Type and TypeName, but not both: an \
                              extension is identified by its ARN or by its kind and name";

/// The refusal the registry answers a request that identifies no extension at all.
const NO_IDENTITY: &str =
    "Either Arn, or both Type and TypeName, must be specified to identify an extension";

/// An instant to activate the seeded versions at, so nothing a case asserts comes from the
/// clock.
fn activated_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset_secs)
}

/// One ACTIVATION of a public third-party extension: the record this account and Region holds
/// for a public extension it activated under `alias`, with `versions` versions of it.
fn activation(
    region: &str,
    type_kind: &str,
    alias: &str,
    original_type_name: &str,
    versions: usize,
) -> StoredExtension {
    let mut extension = StoredExtension::new(region, type_kind, alias);
    let arn = extension.arn.clone();
    extension.visibility = visibility::PUBLIC.to_string();
    extension.publisher_id = Some(PUBLISHER_ID.to_string());
    extension.original_type_name = Some(original_type_name.to_string());
    extension.original_type_arn = Some(public_type_arn(region, type_kind, original_type_name));
    extension.latest_public_version = Some("2.1.0".to_string());
    extension.is_activated = Some(true);
    extension.auto_update = Some(true);
    // What "deactivating an extension deletes the configuration details that are associated
    // with it" has to have something to delete.
    extension.configuration_schema = Some(format!("{{\"{alias}\":\"configured\"}}"));
    extension.versions = (0..versions)
        .map(|position| {
            let version_id = format!("{:08}", position + 1);
            let mut version =
                StoredTypeVersion::new(&arn, &version_id, activated_at(position as u64 * 60));
            version.description = Some(format!("{alias}, version {version_id}"));
            version.public_version_number = Some(format!("1.{position}.0"));
            version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
            version
        })
        .collect();
    extension
}

/// A private extension registered in this account and Region: "the extension is only visible
/// and usable within the account in which it is registered".
fn private_registration() -> StoredExtension {
    let mut extension =
        StoredExtension::new(REGION, registry_type::RESOURCE, PRIVATE_TYPE_NAME);
    let version = StoredTypeVersion::new(&extension.arn, "00000001", activated_at(600));
    extension.default_version_id = Some("00000001".to_string());
    extension.versions = vec![version];
    extension
}

/// An activation this account has ALREADY deactivated, built through the same writer a
/// deactivation goes through.
fn already_deactivated() -> StoredExtension {
    let mut extension = activation(
        REGION,
        registry_type::RESOURCE,
        DEACTIVATED_ALIAS,
        "Publisher::Already::Resource",
        1,
    );
    extension.deactivate();
    extension
}

/// One of the extensions Amazon Web Services publishes, taken from the catalogue
/// `state::registry::aws_published` reports them from
/// (`state::template::IMPLEMENTED_RESOURCE_TYPES`) rather than restated here: nobody activated
/// it, so it is not a record this operation can name.
fn aws_published_type_name() -> &'static str {
    IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the registry publishes at least one resource type")
}

/// Every record the seeded registry holds.
fn seeded_records() -> Vec<StoredExtension> {
    vec![
        activation(
            REGION,
            registry_type::RESOURCE,
            RESOURCE_ALIAS,
            "Publisher::Sample::Resource",
            2,
        ),
        activation(
            REGION,
            registry_type::RESOURCE,
            ARN_RESOURCE_ALIAS,
            "Publisher::Another::Resource",
            2,
        ),
        activation(
            REGION,
            registry_type::MODULE,
            MODULE_ALIAS,
            "Publisher::Sample::Thing::MODULE",
            2,
        ),
        activation(
            REGION,
            registry_type::HOOK,
            HOOK_ALIAS,
            "Publisher::Sample::Hook",
            3,
        ),
        activation(
            OTHER_REGION,
            registry_type::RESOURCE,
            OTHER_REGION_ALIAS,
            "Publisher::Other::Resource",
            1,
        ),
        private_registration(),
        already_deactivated(),
    ]
}

/// A router over a store seeded with every record above.
fn router_with_activations() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        state.lock().extensions = seeded_records();
    }
    (router, engine)
}

async fn deactivate_type(
    router: &MultiServiceRouter,
    members: &[(&str, String)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DeactivateType", &members).await
}

async fn describe_type(router: &MultiServiceRouter, members: &[(&str, String)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DescribeType", &members).await
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

/// The deprecation status of every version of the record `arn`, in the order the record
/// carries them.
fn version_statuses(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Vec<String> {
    let state = store(engine);
    let locked = state.lock();
    let extension = locked
        .extension_by_arn(REGION, arn)
        .unwrap_or_else(|| panic!("the registry still holds {arn:?}"));
    extension
        .versions
        .iter()
        .map(|version| version.deprecated_status.clone())
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

/// One row of the deactivation table: a request that must deactivate one activation, the ARN
/// of the record it must write, and how a read-back addresses that record.
struct DeactivationCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    arn: String,
    read_back: Vec<(&'static str, String)>,
}

/// Both ways a request can identify an activation, over every extension kind.
fn deactivation_cases() -> Vec<DeactivationCase> {
    vec![
        DeactivationCase {
            case: "a RESOURCE activation, by Type and TypeName",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", RESOURCE_ALIAS.to_string()),
            ],
            arn: type_arn(REGION, registry_type::RESOURCE, RESOURCE_ALIAS),
            read_back: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", RESOURCE_ALIAS.to_string()),
            ],
        },
        DeactivationCase {
            case: "a RESOURCE activation, by the ARN it carries in this account and Region",
            members: vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, ARN_RESOURCE_ALIAS),
            )],
            arn: type_arn(REGION, registry_type::RESOURCE, ARN_RESOURCE_ALIAS),
            read_back: vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, ARN_RESOURCE_ALIAS),
            )],
        },
        DeactivationCase {
            case: "a MODULE activation, by Type and TypeName",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", MODULE_ALIAS.to_string()),
            ],
            arn: type_arn(REGION, registry_type::MODULE, MODULE_ALIAS),
            read_back: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", MODULE_ALIAS.to_string()),
            ],
        },
        DeactivationCase {
            case: "a HOOK activation, by the ARN it carries in this account and Region",
            members: vec![("Arn", type_arn(REGION, registry_type::HOOK, HOOK_ALIAS))],
            arn: type_arn(REGION, registry_type::HOOK, HOOK_ALIAS),
            read_back: vec![
                ("Type", registry_type::HOOK.to_string()),
                ("TypeName", HOOK_ALIAS.to_string()),
            ],
        },
    ]
}

#[tokio::test]
async fn deactivate_type_deactivates_the_activation_the_request_identifies() {
    for DeactivationCase {
        case,
        members,
        arn,
        read_back,
    } in deactivation_cases()
    {
        let (router, engine) = router_with_activations();
        let before = records(&engine);
        let versions_before = version_statuses(&engine, &arn).len();

        let (status_code, body) = deactivate_type(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the deactivation accepted with 200, got {status_code} {body}"
        );
        assert!(
            !body.contains("<ErrorResponse"),
            "case {case:?}: expected a success envelope, got {body:?}"
        );

        // Read BACK through the registry read: what the deactivation wrote is what a client
        // sees afterwards.
        let (read_status, read_body) = describe_type(&router, &read_back).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected the deactivated extension still readable with 200, got \
             {read_status} {read_body}"
        );
        let is_activated = xml_text(&read_body, "IsActivated");
        assert_eq!(
            is_activated.as_deref(),
            Some("false"),
            "case {case:?}: expected IsActivated false, got {is_activated:?} from {read_body}"
        );
        let auto_update = xml_text(&read_body, "AutoUpdate");
        assert_eq!(
            auto_update.as_deref(),
            Some("false"),
            "case {case:?}: expected AutoUpdate false, got {auto_update:?} from {read_body}"
        );
        let configuration = xml_text(&read_body, "ConfigurationSchema");
        assert_eq!(
            configuration, None,
            "case {case:?}: expected the configuration data deleted, got \
             {configuration:?} from {read_body}"
        );
        let reported_visibility = xml_text(&read_body, "Visibility");
        assert_eq!(
            reported_visibility.as_deref(),
            Some(visibility::PRIVATE),
            // "The scope at which the extension is visible and usable": the record is this
            // account's own copy of the public extension, and leaving active use does not put
            // it into any other account's reach, so the scope it is reported in does not
            // change when it is deactivated.
            "case {case:?}: expected the deactivated activation still reported in this \
             account's own scope, got {reported_visibility:?} from {read_body}"
        );

        // "Once deactivated, an extension can't be used in any CloudFormation operation",
        // which is what DEPRECATED states — of EVERY version the record carries.
        let statuses = version_statuses(&engine, &arn);
        assert_eq!(
            statuses.len(),
            versions_before,
            "case {case:?}: expected the record's {versions_before} versions kept, got \
             {statuses:?}"
        );
        assert!(
            statuses
                .iter()
                .all(|status| status == deprecated_status::DEPRECATED),
            "case {case:?}: expected every version DEPRECATED, got {statuses:?}"
        );

        assert_only_one_record_changed(case, &before, &records(&engine), &arn);
    }
}

/// One row of the refusal table: a request that must be refused, with the status, the code
/// and the message it must be refused with.
struct RefusedCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
    message: String,
}

/// Every kind of record that is not an activation of this Region, every way a request can
/// name nothing, and every way the conditional input rule can be broken.
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
    let refused = |case: &'static str, members: Vec<(&'static str, String)>, message: &str| {
        RefusedCase {
            case,
            members,
            status: 400,
            code: "CFNRegistryException",
            message: message.to_string(),
        }
    };
    let by_name = |type_kind: &str, type_name: &str| {
        vec![
            ("Type", type_kind.to_string()),
            ("TypeName", type_name.to_string()),
        ]
    };
    let resource_arn = type_arn(REGION, registry_type::RESOURCE, RESOURCE_ALIAS);

    vec![
        not_found(
            "a type name no record carries",
            by_name(registry_type::RESOURCE, "No::Such::Resource"),
            "No::Such::Resource",
        ),
        not_found(
            "an activation another Region's registry holds",
            by_name(registry_type::RESOURCE, OTHER_REGION_ALIAS),
            OTHER_REGION_ALIAS,
        ),
        not_found(
            "a private extension registered in this account, which is no activation",
            by_name(registry_type::RESOURCE, PRIVATE_TYPE_NAME),
            PRIVATE_TYPE_NAME,
        ),
        not_found(
            "an extension Amazon Web Services published, which nobody activated",
            by_name(registry_type::RESOURCE, aws_published_type_name()),
            aws_published_type_name(),
        ),
        not_found(
            "an activation already deactivated",
            by_name(registry_type::RESOURCE, DEACTIVATED_ALIAS),
            DEACTIVATED_ALIAS,
        ),
        not_found(
            "an activated name, under another extension kind",
            by_name(registry_type::HOOK, RESOURCE_ALIAS),
            RESOURCE_ALIAS,
        ),
        not_found(
            // An extension's ARN carries the extension's NAME, so a refusal that names what
            // the request identified names that name — the same sentence the name form gets.
            "an ARN no record carries",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
            )],
            "No::Such::Resource",
        ),
        not_found(
            "the ARN of an activation another Region's registry holds",
            vec![(
                "Arn",
                type_arn(OTHER_REGION, registry_type::RESOURCE, OTHER_REGION_ALIAS),
            )],
            OTHER_REGION_ALIAS,
        ),
        not_found(
            // A VERSION ARN carries a segment past the extension's name, so it names no
            // extension of this registry and is quoted as the request spelled it.
            "an ARN naming one VERSION of an activation, which addresses no extension",
            vec![("Arn", type_version_arn(&resource_arn, "00000001"))],
            &type_version_arn(&resource_arn, "00000001"),
        ),
        not_found(
            "the ARN of a private extension registered in this account",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, PRIVATE_TYPE_NAME),
            )],
            PRIVATE_TYPE_NAME,
        ),
        refused("a request identifying no extension at all", vec![], NO_IDENTITY),
        refused(
            "Type without TypeName",
            vec![("Type", registry_type::RESOURCE.to_string())],
            NO_IDENTITY,
        ),
        refused(
            "TypeName without Type",
            vec![("TypeName", RESOURCE_ALIAS.to_string())],
            NO_IDENTITY,
        ),
        refused(
            "Arn beside Type and TypeName",
            vec![
                ("Arn", resource_arn.clone()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", RESOURCE_ALIAS.to_string()),
            ],
            TWO_IDENTITIES,
        ),
    ]
}

#[tokio::test]
async fn deactivate_type_refuses_what_is_no_activation_of_this_account_and_region() {
    let (router, engine) = router_with_activations();

    for RefusedCase {
        case,
        members,
        status,
        code,
        message,
    } in refused_cases()
    {
        let before = records(&engine);
        let (status_code, body) = deactivate_type(&router, &members).await;
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

/// Deactivating leaves no activation behind, so the SAME call again names nothing: the
/// extension is no longer activated in this account and Region.
#[tokio::test]
async fn a_second_deactivation_of_the_same_extension_is_not_found() {
    let (router, engine) = router_with_activations();
    let members = vec![
        ("Type", registry_type::RESOURCE.to_string()),
        ("TypeName", RESOURCE_ALIAS.to_string()),
    ];

    let case = "the first deactivation";
    let (status_code, body) = deactivate_type(&router, &members).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the deactivation accepted with 200, got {status_code} {body}"
    );

    let case = "the second deactivation of the same extension";
    let before = records(&engine);
    let (status_code, body) = deactivate_type(&router, &members).await;
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the call refused with 404, got {status_code} {body}"
    );
    let reported = xml_text(&body, "Message");
    let expected = format!("The type '{RESOURCE_ALIAS}' cannot be found.");
    assert_eq!(
        reported.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
    assert_eq!(
        before,
        records(&engine),
        "case {case:?}: expected the second call to write nothing"
    );
}
