// SPDX-License-Identifier: CC0-1.0

//! What `CreateCapacityProvider` makes of the MEMBERS of a `managedInstancesProvider`
//! configuration, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: a create judges the configuration only against what the model
//! constrains it by — the two members
//! `CreateManagedInstancesProviderConfiguration` requires
//! (`infrastructureRoleArn`, `instanceLaunchTemplate`, and inside that template
//! `ec2InstanceProfileArn` and `networkConfiguration`) have to be there, while EVERY other
//! member of the configuration and of its launch template is optional, so a request may
//! carry any combination of them or none at all and is SERVED, with the stored provider
//! reporting back the configuration it was created with member for member and settling from
//! `PROVISIONING` to `ACTIVE` on the clock. The only configuration a create refuses is one
//! whose value breaks a constraint the model states: a value outside an enumerated member's
//! set, or an integer outside its range, each refused with HTTP 400
//! `InvalidParameterException` naming the member it broke — never a well-formed optional
//! member and never an omitted one.
//!
//! The rows span the class the rule generalises over: which optional members the request
//! carries (none, one per row, and all of them at once), where the member sits (the
//! configuration itself, its launch template, and a structure nested inside that template),
//! and the kind of member it is (an enumerated one, a boolean, a bounded integer at its
//! published bound, a nested structure). The request the recorded check sends — the required
//! members with the launch template's `monitoring` spelled in lower case — is ONE row.
//!
//! The value sets and bounds below come from the model, each read with
//! `smithy_query(query="shape", shape="<Shape>")`:
//!
//! * `CapacityOptionType` — ON_DEMAND, SPOT, RESERVED
//! * `ManagedInstancesMonitoringOptions` — BASIC, DETAILED
//! * `PropagateMITags` — CAPACITY_PROVIDER, NONE
//! * `AutoRepairActionsStatus` — ENABLED, DISABLED
//! * `TaskVolumeStorageGiB` (the target of `storageConfiguration.storageSizeGiB`) — an
//!   integer whose range is `min 1`
//!
//! Which SPELLINGS of the launch template's `monitoring` option name a category is
//! [`crate::managed_instances_monitoring_spelling`]'s subject; this suite is about the
//! members BESIDE it.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::managed_instances;
use crate::ecs_wire::{create_cluster, list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION};

/// The cluster every provider below is scoped to: "Managed instances capacity providers
/// are cluster-scoped", so each create names one.
const CLUSTER: &str = "miconfigured";

/// The one error code this service answers a value that breaks a modelled constraint with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The status a provider whose capacity Amazon ECS has to stand up is created under, with
/// the create that is still running beside it.
const CREATED: (&str, &str) = ("PROVISIONING", "CREATE_IN_PROGRESS");

/// The pair the clock settles that provider into.
const SETTLED: (&str, &str) = ("ACTIVE", "CREATE_COMPLETE");

/// Long enough for the create of a provider Amazon ECS stands capacity up for to fall due
/// (`state::TRANSITION_SPAN`), with margin for a loaded test machine.
const PROVISIONED: Duration = Duration::from_millis(
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
        .as_millis() as u64
        + 400,
);

/// Write every member of `extra` onto `target`, which is the object a row adds them to.
fn with_members(target: &mut Value, extra: Value) {
    if let Value::Object(members) = extra {
        for (member, value) in members {
            target[member] = value;
        }
    }
}

/// The configuration a row sends: the members the model requires
/// ([`crate::capacity_providers::managed_instances`]), plus the optional members the row
/// adds to the configuration and to its launch template.
fn configuration(extra_configuration: Value, extra_template: Value) -> Value {
    let mut built = managed_instances();
    with_members(&mut built["instanceLaunchTemplate"], extra_template);
    with_members(&mut built, extra_configuration);
    built
}

/// One row of the accepted table: the optional members it adds, and the `monitoring` the
/// stored template reports when that is not the spelling the row sent.
struct MemberCase {
    case: &'static str,
    provider_name: &'static str,
    /// The optional members added to the configuration itself.
    extra_configuration: fn() -> Value,
    /// The optional members added to the configuration's `instanceLaunchTemplate`.
    extra_template: fn() -> Value,
    /// The option the stored launch template must report for `monitoring`, for a row whose
    /// request spelled it another way; `None` when the stored value is the one sent.
    stored_monitoring: Option<&'static str>,
}

/// Describe one capacity provider by name and return it.
async fn describe(router: &MultiServiceRouter, provider_name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": [provider_name] }),
        case,
    )
    .await;
    let providers = list(&body, "capacityProviders", case);
    assert_eq!(
        providers.len(),
        1,
        "case {case:?}: expected exactly one described provider, got {body}"
    );
    providers[0].clone()
}

/// The `(status, updateStatus)` pair a provider reports.
fn lifecycle(provider: &Value, case: &str) -> (String, String) {
    (
        string(provider, "status", case),
        string(provider, "updateStatus", case),
    )
}

/// Every row of the accepted table, in one place so the create pass and the settling pass
/// below drive exactly the same providers.
fn accepted_rows() -> [MemberCase; 12] {
    [
        MemberCase {
            case: "the required members alone, with no optional member at all",
            provider_name: "mirequiredonly",
            extra_configuration: || json!({}),
            extra_template: || json!({}),
            stored_monitoring: None,
        },
        MemberCase {
            case: "the recorded request: monitoring in the lower case its own doc publishes",
            provider_name: "mirecorded",
            extra_configuration: || json!({}),
            extra_template: || json!({ "monitoring": "basic" }),
            stored_monitoring: Some("BASIC"),
        },
        MemberCase {
            case: "an enumerated launch-template member: the capacity option type",
            provider_name: "micapacityoption",
            extra_configuration: || json!({}),
            extra_template: || json!({ "capacityOptionType": "SPOT" }),
            stored_monitoring: None,
        },
        MemberCase {
            case: "a bounded integer at the published minimum of its range",
            provider_name: "ministorageminimum",
            extra_configuration: || json!({}),
            extra_template: || json!({ "storageConfiguration": { "storageSizeGiB": 1 } }),
            stored_monitoring: None,
        },
        MemberCase {
            case: "the same integer well inside its range",
            provider_name: "ministoragelarger",
            extra_configuration: || json!({}),
            extra_template: || json!({ "storageConfiguration": { "storageSizeGiB": 500 } }),
            stored_monitoring: None,
        },
        MemberCase {
            case: "a nested structure carrying a boolean: the local storage configuration",
            provider_name: "milocalstorage",
            extra_configuration: || json!({}),
            extra_template: || json!({ "localStorageConfiguration": { "useLocalStorage": true } }),
            stored_monitoring: None,
        },
        MemberCase {
            case: "the two boolean members of the launch template",
            provider_name: "mibooleans",
            extra_configuration: || json!({}),
            extra_template: || {
                json!({ "instanceMetadataTagsPropagation": false, "fipsEnabled": true })
            },
            stored_monitoring: None,
        },
        MemberCase {
            case: "the instance requirements, a structure of its own ranges",
            provider_name: "mirequirements",
            extra_configuration: || json!({}),
            extra_template: || {
                json!({
                    "instanceRequirements": {
                        "vCpuCount": { "min": 2, "max": 8 },
                        "memoryMiB": { "min": 4096 },
                    }
                })
            },
            stored_monitoring: None,
        },
        MemberCase {
            case: "an enumerated member of the configuration itself: tag propagation",
            provider_name: "mipropagatetags",
            extra_configuration: || json!({ "propagateTags": "CAPACITY_PROVIDER" }),
            extra_template: || json!({}),
            stored_monitoring: None,
        },
        MemberCase {
            case: "the infrastructure optimization its doc disables with -1",
            provider_name: "mioptimizationoff",
            extra_configuration: || json!({ "infrastructureOptimization": { "scaleInAfter": -1 } }),
            extra_template: || json!({}),
            stored_monitoring: None,
        },
        MemberCase {
            case: "the same member at the top of the delay its doc publishes",
            provider_name: "mioptimizationmax",
            extra_configuration: || {
                json!({ "infrastructureOptimization": { "scaleInAfter": 3600 } })
            },
            extra_template: || json!({}),
            stored_monitoring: None,
        },
        MemberCase {
            case: "every optional member at once, the configuration's and the template's",
            provider_name: "mieverymember",
            extra_configuration: || {
                json!({
                    "propagateTags": "NONE",
                    "infrastructureOptimization": { "scaleInAfter": 0 },
                    "autoRepairConfiguration": { "actionsStatus": "DISABLED" },
                })
            },
            extra_template: || {
                json!({
                    "monitoring": "DETAILED",
                    "capacityOptionType": "ON_DEMAND",
                    "storageConfiguration": { "storageSizeGiB": 30 },
                    "localStorageConfiguration": { "useLocalStorage": true },
                    "instanceMetadataTagsPropagation": true,
                    "fipsEnabled": false,
                    "instanceRequirements": {
                        "vCpuCount": { "min": 1 },
                        "memoryMiB": { "min": 2048, "max": 16384 },
                    },
                })
            },
            stored_monitoring: None,
        },
    ]
}

#[tokio::test]
async fn a_configuration_carrying_any_optional_members_is_served_and_reads_back_member_for_member() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    for row in accepted_rows() {
        let requested = configuration((row.extra_configuration)(), (row.extra_template)());
        // What the stored provider must report: the configuration as sent, with
        // `monitoring` in the spelling the modelled set lists.
        let mut expected = requested.clone();
        if let Some(monitoring) = row.stored_monitoring {
            expected["instanceLaunchTemplate"]["monitoring"] = json!(monitoring);
        }

        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": row.provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider": requested,
            }),
            row.case,
        )
        .await;
        let provider = &created["capacityProvider"];
        let name = row.provider_name;
        assert_eq!(
            string(provider, "capacityProviderArn", row.case),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}"),
            "case {:?}: expected the provider's own ARN, got {created}",
            row.case
        );
        assert_eq!(
            string(provider, "type", row.case),
            "MANAGED_INSTANCES",
            "case {:?}: expected the managed-instances provider type, got {created}",
            row.case
        );
        let created_lifecycle = lifecycle(provider, row.case);
        assert_eq!(
            (created_lifecycle.0.as_str(), created_lifecycle.1.as_str()),
            CREATED,
            "case {:?}: expected the create answered with its capacity still standing up, \
             got {created}",
            row.case
        );
        assert_eq!(
            provider["managedInstancesProvider"], expected,
            "case {:?}: expected the create to answer the configuration it was given, got \
             {created}",
            row.case
        );

        let described = describe(&router, row.provider_name, row.case).await;
        assert_eq!(
            described["managedInstancesProvider"], expected,
            "case {:?}: expected the stored provider to report the configuration member for \
             member, got {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn every_such_provider_settles_to_active_whichever_optional_members_it_carries() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    for row in accepted_rows() {
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": row.provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider":
                    configuration((row.extra_configuration)(), (row.extra_template)()),
            }),
            row.case,
        )
        .await;
    }

    tokio::time::sleep(PROVISIONED).await;

    for row in accepted_rows() {
        let described = describe(&router, row.provider_name, row.case).await;
        let settled = lifecycle(&described, row.case);
        assert_eq!(
            (settled.0.as_str(), settled.1.as_str()),
            SETTLED,
            "case {:?}: expected the provider settled with its create complete, got \
             {described}",
            row.case
        );
    }
}

/// One row of the refused table: the member it breaks a modelled constraint on, and the
/// whole sentence the refusal carries.
struct ConstraintCase {
    case: &'static str,
    provider_name: &'static str,
    extra_configuration: fn() -> Value,
    extra_template: fn() -> Value,
    message: &'static str,
}

#[tokio::test]
async fn a_member_breaking_a_modelled_constraint_is_refused_and_stores_no_provider() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;

    let rows = [
        ConstraintCase {
            case: "a capacity option spelled in lower case, which that set does not list",
            provider_name: "mioptionlower",
            extra_configuration: || json!({}),
            extra_template: || json!({ "capacityOptionType": "spot" }),
            message: "capacity option type should be one of [ON_DEMAND,RESERVED,SPOT]",
        },
        ConstraintCase {
            case: "a capacity option that names none of the three",
            provider_name: "mioptionunknown",
            extra_configuration: || json!({}),
            extra_template: || json!({ "capacityOptionType": "SPOTTY" }),
            message: "capacity option type should be one of [ON_DEMAND,RESERVED,SPOT]",
        },
        ConstraintCase {
            case: "a monitoring category that names none in any case",
            provider_name: "mimonitoringunknown",
            extra_configuration: || json!({}),
            extra_template: || json!({ "monitoring": "VERBOSE" }),
            message: "monitoring should be one of [BASIC,DETAILED]",
        },
        ConstraintCase {
            case: "a tag-propagation value outside the configuration's own set",
            provider_name: "mipropagateunknown",
            extra_configuration: || json!({ "propagateTags": "capacity_provider" }),
            extra_template: || json!({}),
            message: "propagate tags should be one of [CAPACITY_PROVIDER,NONE]",
        },
        ConstraintCase {
            case: "an auto-repair status, a member nested inside the configuration",
            provider_name: "mirepairunknown",
            extra_configuration: || json!({ "autoRepairConfiguration": { "actionsStatus": "on" } }),
            extra_template: || json!({}),
            message: "actions status should be one of [DISABLED,ENABLED]",
        },
        ConstraintCase {
            case: "a storage size one below the published minimum of its range",
            provider_name: "ministorageunder",
            extra_configuration: || json!({}),
            extra_template: || json!({ "storageConfiguration": { "storageSizeGiB": 0 } }),
            // A member whose own range the model states, and which Amazon ECS words in the
            // constraint clause rather than in a domain sentence: the value, the member's
            // position in the request, and the requirement it failed.
            message: "1 validation error detected: Value '0' at \
                      'managedInstancesProvider.instanceLaunchTemplate.storageConfiguration\
                      .storageSizeGiB' failed to satisfy constraint: Member must have value \
                      greater than or equal to 1",
        },
    ];

    for row in rows {
        let (code, message) = refusal(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": row.provider_name,
                "cluster": CLUSTER,
                "managedInstancesProvider":
                    configuration((row.extra_configuration)(), (row.extra_template)()),
            }),
            row.case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, row.message),
            "case {:?}: expected the member it broke and what it accepts, got {code:?} with \
             {message:?}",
            row.case
        );

        // The refused create stored nothing: the name it asked for describes no provider,
        // and the read reports that name as one failure instead.
        let body = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": [row.provider_name] }),
            row.case,
        )
        .await;
        assert!(
            list(&body, "capacityProviders", row.case).is_empty(),
            "case {:?}: a refused create must leave no provider, got {body}",
            row.case
        );
        assert_eq!(
            list(&body, "failures", row.case).len(),
            1,
            "case {:?}: expected the name reported as one failure, got {body}",
            row.case
        );
    }
}
