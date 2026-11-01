// SPDX-License-Identifier: CC0-1.0

//! `UpdateNetworkMigrationDefinition`, end to end over HTTP.
//!
//! The rule these tests pin: an update replaces every member the request CARRIES and
//! leaves every member it OMITS as the definition already held it; the two members that
//! arrive as their own update shapes (`targetNetwork`, `targetS3Configuration`) follow
//! the same rule one level down, per sub-member; the identity of the record (its id, its
//! ARN, its `createdAt` and the `tags` the update models no member for) never moves; and
//! what the update answers is what a later `GetNetworkMigrationDefinition` reports.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state back only through the service's own operations.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines; `call` takes the operation's modelled URI path, so
//! these operations are reached under `/network-migration/<Operation>`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::{
    CidrMapping, SourceConfiguration, SourceS3Configuration, TargetNetwork, TargetS3Configuration,
};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    NetworkMigrationDefinition, NetworkMigrationDefinitionConfiguration,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `GetNetworkMigrationDefinition`, likewise.
const GET: &str = "network-migration/GetNetworkMigrationDefinition";

/// The modelled `http.uri` of `UpdateNetworkMigrationDefinition`, likewise.
const UPDATE: &str = "network-migration/UpdateNetworkMigrationDefinition";

/// A `sourceConfigurations` element as the model requires it (every member of
/// `SourceConfiguration` and of its `sourceS3Configuration` is `@required`).
fn source(environment: &str, bucket: &str, key: &str) -> Value {
    json!({
        "sourceEnvironment": environment,
        "sourceS3Configuration": {
            "s3Bucket": bucket,
            "s3BucketOwner": EMULATOR_ACCOUNT,
            "s3Key": key,
        },
    })
}

/// The definition every row starts from: a record carrying EVERY member a create can
/// set, so each row's omissions are what the rule is read off.
fn base_request() -> Value {
    json!({
        "name": "hub-and-spoke-plan",
        "description": "two regions, one inspection VPC",
        "sourceConfigurations": [
            source("NSX", "nsx-exports", "exports/nsx.json"),
            source("VSPHERE", "vsphere-exports", "exports/vsphere.zip"),
        ],
        "targetS3Configuration": {
            "s3Bucket": "migration-artifacts",
            "s3BucketOwner": EMULATOR_ACCOUNT,
        },
        "targetNetwork": {
            "topology": "HUB_AND_SPOKE",
            "inboundCidr": "10.0.0.0/16",
            "outboundCidr": "10.1.0.0/24",
            "inspectionCidr": "192.168.1.0/28",
        },
        "targetDeployment": "SINGLE_ACCOUNT",
        "vpcProvisioningStrategy": "USE_EXISTING",
        "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.8.0.0/16"}],
        "tags": {"env": "prod", "team": "migration"},
        "scopeTags": {"BusinessUnit": "retail"},
    })
}

/// The `networkMigrationDefinitionID` of a record a call answered.
fn definition_id(definition: &Value) -> String {
    definition["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("no networkMigrationDefinitionID in {definition:?}"))
        .to_string()
}

/// `record` with `overrides` applied: a member set to a value replaces it, a member set
/// to `null` is REMOVED (the convention this crate's records report an empty list or map
/// under).
fn with_overrides(record: &Value, overrides: &Value) -> Value {
    let mut expected = record.clone();
    let members = expected.as_object_mut().expect("an object record");
    for (member, value) in overrides.as_object().expect("an object of overrides") {
        if value.is_null() {
            members.remove(member);
        } else {
            members.insert(member.clone(), value.clone());
        }
    }
    expected
}

/// `record` without `updatedAt`, which moves with the change rather than with the
/// members the rule is about and is asserted on its own.
fn without_updated_at(record: &Value) -> Value {
    let mut stripped = record.clone();
    stripped
        .as_object_mut()
        .expect("an object record")
        .remove("updatedAt");
    stripped
}

/// The class an `UpdateNetworkMigrationDefinition` request spans, one row per axis the
/// rule generalises over: which member is named, how many are named at once (none, one,
/// every one), whether a list or map member is replaced or emptied, whether a nested
/// update shape names one sub-member or all of them, and the value's size and form
/// (a single character, the modelled maxima, interior spaces, both enum values).
///
/// Each row is the patch (without the id, which the test fills in) and what it must
/// leave on the record the base create answered — `null` meaning "reported absent".
///
/// The enum values and the bounds are the model's
/// (`smithy_query(query="operation",operation="UpdateNetworkMigrationDefinition")`:
/// `NetworkMigrationDefinitionName` len 1..256, `NetworkMigrationDefinitionDescription`
/// len 0..600, `SourceConfigurationList` len 0..2, `CidrMappingsList` len 0..50,
/// `TargetDeployment` MULTI_ACCOUNT | SINGLE_ACCOUNT, `VpcProvisioningStrategy`
/// CREATE_NEW | USE_EXISTING, `TargetNetworkTopology` ISOLATED_VPC | HUB_AND_SPOKE).
fn update_cases() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "no member but the id: nothing moves",
            json!({}),
            json!({}),
        ),
        (
            "the name alone",
            json!({"name": "renamed-plan"}),
            json!({"name": "renamed-plan"}),
        ),
        (
            "a single-character name — the modelled minimum",
            json!({"name": "a"}),
            json!({"name": "a"}),
        ),
        (
            "a 256-character name — the modelled maximum",
            json!({"name": "n".repeat(256)}),
            json!({"name": "n".repeat(256)}),
        ),
        (
            "a name with interior spaces and punctuation",
            json!({"name": "SAP ERP  prod (wave 2)"}),
            json!({"name": "SAP ERP  prod (wave 2)"}),
        ),
        (
            "the description alone",
            json!({"description": "one region"}),
            json!({"description": "one region"}),
        ),
        (
            "an empty description — the modelled minimum length",
            json!({"description": ""}),
            json!({"description": ""}),
        ),
        (
            "a 600-character description — the modelled maximum",
            json!({"description": "d".repeat(600)}),
            json!({"description": "d".repeat(600)}),
        ),
        (
            "one source configuration replacing the stored two",
            json!({"sourceConfigurations": [source("CISCO_ACI", "aci-exports", "exports/aci.json")]}),
            json!({"sourceConfigurations": [source("CISCO_ACI", "aci-exports", "exports/aci.json")]}),
        ),
        (
            "an empty source-configuration list empties the member",
            json!({"sourceConfigurations": []}),
            json!({"sourceConfigurations": null}),
        ),
        (
            "the other target deployment",
            json!({"targetDeployment": "MULTI_ACCOUNT"}),
            json!({"targetDeployment": "MULTI_ACCOUNT"}),
        ),
        (
            "scope tags replaced by a different set",
            json!({"scopeTags": {"BusinessUnit": "wholesale", "Tier": "gold"}}),
            json!({"scopeTags": {"BusinessUnit": "wholesale", "Tier": "gold"}}),
        ),
        (
            "an empty scope-tag map empties the member",
            json!({"scopeTags": {}}),
            json!({"scopeTags": null}),
        ),
        (
            "one CIDR mapping replacing the stored one",
            json!({"cidrMappings": [{"originalCidr": "172.16.4.0/22", "updatedCidr": "172.20.4.0/22"}]}),
            json!({"cidrMappings": [{"originalCidr": "172.16.4.0/22", "updatedCidr": "172.20.4.0/22"}]}),
        ),
        (
            "an empty CIDR-mapping list empties the member",
            json!({"cidrMappings": []}),
            json!({"cidrMappings": null}),
        ),
        (
            // The nested update shape names ONE sub-member: the topology and the other
            // two CIDRs stay as stored.
            "targetNetwork naming only the inbound CIDR",
            json!({"targetNetwork": {"inboundCidr": "10.4.0.0/16"}}),
            json!({"targetNetwork": {
                "topology": "HUB_AND_SPOKE",
                "inboundCidr": "10.4.0.0/16",
                "outboundCidr": "10.1.0.0/24",
                "inspectionCidr": "192.168.1.0/28",
            }}),
        ),
        (
            "targetNetwork naming only the topology",
            json!({"targetNetwork": {"topology": "ISOLATED_VPC"}}),
            json!({"targetNetwork": {
                "topology": "ISOLATED_VPC",
                "inboundCidr": "10.0.0.0/16",
                "outboundCidr": "10.1.0.0/24",
                "inspectionCidr": "192.168.1.0/28",
            }}),
        ),
        (
            "targetNetwork naming every sub-member",
            json!({"targetNetwork": {
                "topology": "ISOLATED_VPC",
                "inboundCidr": "254.0.0.255/16",
                "outboundCidr": "1.2.3.0/28",
                "inspectionCidr": "172.31.0.0/16",
            }}),
            json!({"targetNetwork": {
                "topology": "ISOLATED_VPC",
                "inboundCidr": "254.0.0.255/16",
                "outboundCidr": "1.2.3.0/28",
                "inspectionCidr": "172.31.0.0/16",
            }}),
        ),
        (
            "targetS3Configuration naming only the bucket",
            json!({"targetS3Configuration": {"s3Bucket": "other-artifacts"}}),
            json!({"targetS3Configuration": {
                "s3Bucket": "other-artifacts",
                "s3BucketOwner": EMULATOR_ACCOUNT,
            }}),
        ),
        (
            "targetS3Configuration naming only the bucket owner",
            json!({"targetS3Configuration": {"s3BucketOwner": OTHER_ACCOUNT}}),
            json!({"targetS3Configuration": {
                "s3Bucket": "migration-artifacts",
                "s3BucketOwner": OTHER_ACCOUNT,
            }}),
        ),
        (
            "targetS3Configuration naming both sub-members",
            json!({"targetS3Configuration": {"s3Bucket": "a", "s3BucketOwner": OTHER_ACCOUNT}}),
            json!({"targetS3Configuration": {"s3Bucket": "a", "s3BucketOwner": OTHER_ACCOUNT}}),
        ),
        (
            // Off `USE_EXISTING` and the mappings dropped in the same call, which is what
            // the model's rule on `cidrMappings` allows.
            "the other provisioning strategy with the mappings dropped",
            json!({"vpcProvisioningStrategy": "CREATE_NEW", "cidrMappings": []}),
            json!({"vpcProvisioningStrategy": "CREATE_NEW", "cidrMappings": null}),
        ),
        (
            "every updatable member at once",
            json!({
                "name": "fully-overwritten-plan",
                "description": "rewritten",
                "sourceConfigurations": [source(
                    "PALO_ALTO_FIREWALL",
                    "firewall-exports",
                    "exports/palo-alto.json",
                )],
                "targetS3Configuration": {"s3Bucket": "rewritten.artifacts", "s3BucketOwner": OTHER_ACCOUNT},
                "targetNetwork": {"topology": "ISOLATED_VPC", "inboundCidr": "10.9.0.0/24"},
                "targetDeployment": "MULTI_ACCOUNT",
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [
                    {"originalCidr": "10.0.0.0/16", "updatedCidr": "10.32.0.0/16"},
                    {"originalCidr": "192.168.0.0/28", "updatedCidr": "192.168.9.0/28"},
                ],
                "scopeTags": {"BusinessUnit": "retail", "Tier": "silver"},
            }),
            json!({
                "name": "fully-overwritten-plan",
                "description": "rewritten",
                "sourceConfigurations": [source(
                    "PALO_ALTO_FIREWALL",
                    "firewall-exports",
                    "exports/palo-alto.json",
                )],
                "targetS3Configuration": {"s3Bucket": "rewritten.artifacts", "s3BucketOwner": OTHER_ACCOUNT},
                // The sub-members the patch omits keep the stored values.
                "targetNetwork": {
                    "topology": "ISOLATED_VPC",
                    "inboundCidr": "10.9.0.0/24",
                    "outboundCidr": "10.1.0.0/24",
                    "inspectionCidr": "192.168.1.0/28",
                },
                "targetDeployment": "MULTI_ACCOUNT",
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [
                    {"originalCidr": "10.0.0.0/16", "updatedCidr": "10.32.0.0/16"},
                    {"originalCidr": "192.168.0.0/28", "updatedCidr": "192.168.9.0/28"},
                ],
                "scopeTags": {"BusinessUnit": "retail", "Tier": "silver"},
            }),
        ),
    ]
}

/// Every row of the class: the update answers the stored record with the named members
/// replaced and every other member — including the `tags` the operation models no member
/// for, the id, the ARN and `createdAt` — untouched, and a later get answers exactly
/// that record.
#[tokio::test]
async fn an_update_replaces_the_named_members_and_preserves_the_rest() {
    for (case, patch, expected_overrides) in update_cases() {
        // One emulator per row: an identical create is an `@idempotent` replay of the
        // stored definition, so each row needs its own store to start from the base
        // record rather than from the previous row's result.
        let mgn = Mgn::new();
        let (status, created) = mgn.call(CREATE, &base_request()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the base create expected 201, got {status} with {created:?}"
        );
        let id = definition_id(&created);

        let mut request = patch.clone();
        request["networkMigrationDefinitionID"] = json!(id);
        let (status, answered) = mgn.call(UPDATE, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateNetworkMigrationDefinition expected 200, got {status} with \
             {answered:?}"
        );

        let expected = with_overrides(&created, &expected_overrides);
        assert_eq!(
            without_updated_at(&answered),
            without_updated_at(&expected),
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
        let created_at = created["createdAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: createdAt must be an epoch-seconds number, got {created:?}")
        });
        let updated_at = answered["updatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: updatedAt must be an epoch-seconds number, got {answered:?}")
        });
        assert!(
            updated_at >= created_at,
            "case {case:?}: updatedAt must not precede createdAt {created_at}, got {updated_at}"
        );

        // Round trip: the update wrote to the same record the reader reports, and
        // reading does not change it.
        let (status, fetched) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with \
             {fetched:?}"
        );
        assert_eq!(
            fetched, answered,
            "case {case:?}: a get must answer the updated record unchanged, got {fetched:?}"
        );
        let (_, again) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            again, fetched,
            "case {case:?}: a second get must answer the same record, got {again:?}"
        );

        assert_eq!(
            mgn.store().lock().network_migration_definitions.len(),
            1,
            "case {case:?}: an update must not mint a second definition"
        );
    }
}

/// A definition seeded into `account`'s store with `created_at` as both its timestamps,
/// as a create at that instant would have left it. Answers nothing: the id is the
/// caller's.
fn seed(mgn: &Mgn, account: &str, id: &str, created_at: SystemTime) {
    let stored = mgn
        .store()
        .lock()
        .insert_network_migration_definition(NetworkMigrationDefinition {
            network_migration_definition_id: id.to_string(),
            arn: format!("arn:aws:mgn:{REGION}:{account}:network-migration-definition/{id}"),
            account_id: account.to_string(),
            created_at,
            updated_at: created_at,
            configuration: NetworkMigrationDefinitionConfiguration {
                name: "seeded-plan".to_string(),
                description: Some("seeded".to_string()),
                source_configurations: vec![SourceConfiguration {
                    source_environment: "NSX".to_string(),
                    source_s3_configuration: SourceS3Configuration {
                        s3_bucket: "nsx-exports".to_string(),
                        s3_bucket_owner: EMULATOR_ACCOUNT.to_string(),
                        s3_key: "exports/nsx.json".to_string(),
                    },
                }],
                target_s3_configuration: TargetS3Configuration {
                    s3_bucket: "migration-artifacts".to_string(),
                    s3_bucket_owner: EMULATOR_ACCOUNT.to_string(),
                },
                target_network: TargetNetwork {
                    topology: "ISOLATED_VPC".to_string(),
                    ..TargetNetwork::default()
                },
                vpc_provisioning_strategy: Some("USE_EXISTING".to_string()),
                cidr_mappings: vec![CidrMapping {
                    original_cidr: "10.0.0.0/16".to_string(),
                    updated_cidr: "10.8.0.0/16".to_string(),
                }],
                ..NetworkMigrationDefinitionConfiguration::default()
            },
        });
    assert!(
        stored,
        "seeding {id:?} must store it (the store bound was not reached)"
    );
}

/// `updatedAt` is the instant the definition last CHANGED: an update that moves a member
/// moves it past `createdAt`, and a repeat of that same update — the `@idempotent` retry
/// — answers the very same record, timestamp included.
#[tokio::test]
async fn updated_at_moves_with_a_change_and_a_repeat_changes_nothing() {
    let mgn = Mgn::new();
    let id = "nmd-0A1b2C3d4E5f6G7h8";
    // A day back, so "moved forward" is a decision about the record rather than about
    // the clock's resolution.
    seed(
        &mgn,
        EMULATOR_ACCOUNT,
        id,
        SystemTime::now() - Duration::from_secs(86_400),
    );

    let (status, before) = mgn
        .call(GET, &json!({"networkMigrationDefinitionID": id}))
        .await;
    assert_eq!(status, 200, "the seeded record must be readable, got {before:?}");
    assert_eq!(
        before["createdAt"], before["updatedAt"],
        "nothing has updated the seeded definition, got {before:?}"
    );

    let patch = json!({"networkMigrationDefinitionID": id, "description": "rewritten"});
    let (status, answered) = mgn.call(UPDATE, &patch).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answered:?}");
    assert_eq!(
        answered["createdAt"], before["createdAt"],
        "an update must not move createdAt, got {answered:?}"
    );
    let created_at = answered["createdAt"].as_f64().expect("a numeric createdAt");
    let updated_at = answered["updatedAt"].as_f64().expect("a numeric updatedAt");
    assert!(
        updated_at > created_at,
        "a change must move updatedAt past createdAt {created_at}, got {updated_at}"
    );

    // The same request again, and then one naming no member at all: neither changes a
    // thing, so neither moves the timestamp either.
    for (case, repeat) in [
        ("the same patch again", patch.clone()),
        (
            "a patch naming no member",
            json!({"networkMigrationDefinitionID": id}),
        ),
    ] {
        let (status, again) = mgn.call(UPDATE, &repeat).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {again:?}"
        );
        assert_eq!(
            again, answered,
            "case {case:?}: a call that changes nothing must answer the stored record \
             unchanged, got {again:?}"
        );
    }
}

/// An id the account holds no definition for is the modelled `ResourceNotFoundException`
/// (404) — whatever its shape, and including a definition another account holds, which is
/// not this caller's to update.
#[tokio::test]
async fn updating_an_unknown_definition_is_refused_as_not_found() {
    let mgn = Mgn::new();
    let other_account_definition = "nmd-11111111111111111";
    seed(
        &mgn,
        OTHER_ACCOUNT,
        other_account_definition,
        SystemTime::now(),
    );

    let unknown = [
        "nmd-00000000000000000",
        "nmd-zzzzzzzzzzzzzzzzz",
        "nmd-0A1b2C3d4E5f6G7h8",
        other_account_definition,
    ];
    for id in unknown {
        let (status, body) = mgn
            .call(
                UPDATE,
                &json!({"networkMigrationDefinitionID": id, "name": "renamed-plan"}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "id {id:?}: an unknown definition must be 404 ResourceNotFoundException, got {status} \
             with {body:?}"
        );
    }

    // The other account's record is untouched by the refusals.
    let store = mgn.store();
    let state = store.lock();
    let definition = state
        .network_migration_definition_of(OTHER_ACCOUNT, other_account_definition)
        .expect("the seeded definition is still stored");
    assert_eq!(
        definition.configuration.name, "seeded-plan",
        "a refused update must leave the other account's definition as it was, got {:?}",
        definition.configuration.name
    );
}

/// CIDR mappings live on a definition only while it migrates into VPCs that already
/// exist — the rule the model documents on `cidrMappings` — so an update that would leave
/// mappings on a definition under any other strategy is the modelled
/// `ValidationException` (400) and stores nothing, whether the mappings or the strategy
/// are what the request names.
#[tokio::test]
async fn an_update_keeps_cidr_mappings_under_the_use_existing_strategy() {
    let mappings = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "originalCidr": format!("10.{index}.0.0/16"),
                        "updatedCidr": format!("10.{}.0.0/16", index + 100),
                    })
                })
                .collect(),
        )
    };

    // `VpcProvisioningStrategy` is CREATE_NEW | USE_EXISTING and the member is optional,
    // so "not USE_EXISTING" is CREATE_NEW or whatever the definition already carries.
    let refused: Vec<(&str, Value, Value)> = vec![
        (
            "mappings added to a definition provisioning new VPCs",
            json!({"vpcProvisioningStrategy": "CREATE_NEW"}),
            json!({"cidrMappings": mappings(1)}),
        ),
        (
            "fifty mappings — the modelled maximum — added to the same definition",
            json!({"vpcProvisioningStrategy": "CREATE_NEW"}),
            json!({"cidrMappings": mappings(50)}),
        ),
        (
            "mappings added to a definition naming no strategy at all",
            json!({"vpcProvisioningStrategy": null}),
            json!({"cidrMappings": mappings(2)}),
        ),
        (
            "the strategy moved off USE_EXISTING while the stored mappings stay",
            json!({"vpcProvisioningStrategy": "USE_EXISTING", "cidrMappings": mappings(3)}),
            json!({"vpcProvisioningStrategy": "CREATE_NEW"}),
        ),
        (
            "both named in one call, the mappings outside USE_EXISTING",
            json!({}),
            json!({"vpcProvisioningStrategy": "CREATE_NEW", "cidrMappings": mappings(1)}),
        ),
    ];

    for (case, creation_overrides, patch) in refused {
        let mgn = Mgn::new();
        let mut request = base_request();
        // The base carries mappings under USE_EXISTING; a row that starts elsewhere
        // replaces both members together, which is what the create's own rule allows.
        request["cidrMappings"] = json!([]);
        request = with_overrides(&request, &creation_overrides);
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the base create expected 201, got {status} with {created:?}"
        );
        let id = definition_id(&created);

        let mut update = patch.clone();
        update["networkMigrationDefinitionID"] = json!(id);
        let (status, body) = mgn.call(UPDATE, &update).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: mappings outside USE_EXISTING must be 400 ValidationException, got \
             {status} with {body:?}"
        );

        let (status, after) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            (status, &after),
            (200, &created),
            "case {case:?}: a refused update must store nothing, got {after:?}"
        );
    }

    // The accepted side of the same rule: mappings under USE_EXISTING, at the count
    // bounds the model declares for `CidrMappingsList` (0..50).
    for count in [0usize, 1, 50] {
        let case = format!("USE_EXISTING with {count} mapping(s)");
        let mgn = Mgn::new();
        let (status, created) = mgn.call(CREATE, &base_request()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the base create expected 201, got {status} with {created:?}"
        );
        let id = definition_id(&created);

        let (status, answered) = mgn
            .call(
                UPDATE,
                &json!({
                    "networkMigrationDefinitionID": id,
                    "vpcProvisioningStrategy": "USE_EXISTING",
                    "cidrMappings": mappings(count),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answered:?}"
        );
        let expected = if count == 0 {
            Value::Null
        } else {
            mappings(count)
        };
        assert_eq!(
            answered["cidrMappings"], expected,
            "case {case:?}: the stored mappings must be reported back, got {:?}",
            answered["cidrMappings"]
        );
    }
}

/// A `name` that violates the constraints the model declares on it — a blank name, at any
/// length — never reaches the record: the call is refused with HTTP 400 and the stored
/// definition is exactly what it was. (Which 400 CODE the front end words a modelled
/// value violation with is pinned once, in `request_body_validation.rs`.)
#[tokio::test]
async fn a_blank_name_is_refused_and_leaves_the_definition_alone() {
    let mgn = Mgn::new();
    let (status, created) = mgn.call(CREATE, &base_request()).await;
    assert_eq!(status, 201, "the base create expected 201, got {created:?}");
    let id = definition_id(&created);

    // `NetworkMigrationDefinitionName` is @length 1..256 with @pattern
    // ^[^\s\x00]( *[^\s\x00])*$: no empty name, and no name that is only whitespace or
    // that begins or ends with it.
    let blank = [
        ("an empty name", ""),
        ("a single space", " "),
        ("several spaces", "    "),
        ("a tab", "\t"),
        ("a leading space", " renamed-plan"),
        ("a trailing space", "renamed-plan "),
    ];
    for (case, name) in blank {
        let (status, body) = mgn
            .call(
                UPDATE,
                &json!({"networkMigrationDefinitionID": id, "name": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a blank name must be refused with 400, got {status} with {body:?}"
        );
        let (status, after) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            (status, &after),
            (200, &created),
            "case {case:?}: a refused update must leave the definition alone, got {after:?}"
        );
    }
}
