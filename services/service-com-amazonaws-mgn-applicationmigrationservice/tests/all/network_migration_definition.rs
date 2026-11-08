// SPDX-License-Identifier: CC0-1.0

//! mgn network migration definitions, end to end over HTTP:
//! `CreateNetworkMigrationDefinition` and `GetNetworkMigrationDefinition`.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state only through its own operations: what a create returns must be what a
//! later get sees.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines; `call` takes the operation's modelled URI path, so
//! these operations are reached under `/network-migration/<Operation>`.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::{
    TargetNetwork, TargetS3Configuration,
};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    NetworkMigrationDefinition, NetworkMigrationDefinitionConfiguration,
    NETWORK_MIGRATION_DEFINITION_QUOTA,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, REGION};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `GetNetworkMigrationDefinition`, likewise.
const GET: &str = "network-migration/GetNetworkMigrationDefinition";

/// Every member of the modelled `NetworkMigrationDefinition` output shape that a create
/// request can set and that the record reports back exactly as the request named it, so a
/// row asserts the whole echoed surface rather than a few members. The three members the
/// service maintains a value of its own for — `targetDeployment`,
/// `vpcProvisioningStrategy` and the `tags` container — are asserted beside this list.
/// Source: `smithy_query(query="shape",shape="NetworkMigrationDefinition")`.
const CONFIGURED_MEMBERS: [&str; 7] = [
    "name",
    "description",
    "sourceConfigurations",
    "targetS3Configuration",
    "targetNetwork",
    "cidrMappings",
    "scopeTags",
];

/// The `TargetDeployment` a definition created without one is maintained under.
/// Source: the service's own constant, re-stated here from the SPEC side so the test
/// pins a value rather than reading the implementation's.
const MAINTAINED_TARGET_DEPLOYMENT: &str = "SINGLE_ACCOUNT";

/// The `VpcProvisioningStrategy` a definition created without one is maintained under.
const MAINTAINED_VPC_PROVISIONING_STRATEGY: &str = "CREATE_NEW";

/// What the record must report for the two settings the service maintains and for the
/// tag container, given the create `request`: the value the request named, the service's
/// own value where it named none, and the tag container as the map it is — empty rather
/// than absent — because the definition is a resource the tag API addresses.
fn maintained_members(request: &Value) -> Value {
    let value_or = |member: &str, maintained: &str| match request[member].clone() {
        Value::Null => json!(maintained),
        named => named,
    };
    json!({
        "targetDeployment": value_or("targetDeployment", MAINTAINED_TARGET_DEPLOYMENT),
        "vpcProvisioningStrategy": value_or(
            "vpcProvisioningStrategy",
            MAINTAINED_VPC_PROVISIONING_STRATEGY,
        ),
        "tags": match request["tags"].clone() {
            Value::Null => json!({}),
            named => named,
        },
    })
}

/// `^nmd-[0-9a-zA-Z]{17}$` — the model's `NetworkMigrationDefinitionID` pattern
/// (length 21..21).
fn is_definition_id(value: &str) -> bool {
    value.len() == 21
        && value.starts_with("nmd-")
        && value[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

fn definition_id(definition: &Value) -> String {
    definition["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("no networkMigrationDefinitionID in {definition:?}"))
        .to_string()
}

/// A `targetS3Configuration` as the model requires it: a bucket name and a 12-digit
/// owner account.
fn target_s3(bucket: &str) -> Value {
    json!({"s3Bucket": bucket, "s3BucketOwner": EMULATOR_ACCOUNT})
}

/// The class a `CreateNetworkMigrationDefinition` request spans: the name's length and
/// shape, the description (absent, empty, long), the source-configuration count (0, 1,
/// and the modelled maximum of 2), both `TargetNetworkTopology` values with and without
/// the optional CIDRs, both `TargetDeployment` values, both `VpcProvisioningStrategy`
/// values, the CIDR-mapping count (0, 1, several) and the tag / scope-tag counts.
///
/// The enum values and the list bounds are the model's
/// (`smithy_query(query="closure",operation="CreateNetworkMigrationDefinition")`:
/// `TargetNetworkTopology` ISOLATED_VPC | HUB_AND_SPOKE, `TargetDeployment`
/// SINGLE_ACCOUNT | MULTI_ACCOUNT, `VpcProvisioningStrategy` CREATE_NEW | USE_EXISTING,
/// `SourceEnvironment` NSX | VSPHERE | FORTIGATE_FIREWALL | PALO_ALTO_FIREWALL |
/// CISCO_ACI | LOGICAL_MODEL | MODELIZE_IT | AWS_DISCOVERY_COLLECTOR,
/// `SourceConfigurationList` len 0..2, `CidrMappingsList` len 0..50).
fn creation_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            json!({
                "name": "isolated-vpc-plan",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "hub and spoke with all three optional CIDRs and a description",
            json!({
                "name": "hub-and-spoke-plan",
                "description": "two regions, one inspection VPC",
                "targetS3Configuration": target_s3("migration.artifacts-2"),
                "targetNetwork": {
                    "topology": "HUB_AND_SPOKE",
                    "inboundCidr": "10.0.0.0/16",
                    "outboundCidr": "10.1.0.0/24",
                    "inspectionCidr": "192.168.1.0/28",
                },
            }),
        ),
        (
            "one source configuration",
            json!({
                "name": "nsx-source",
                "sourceConfigurations": [{
                    "sourceEnvironment": "NSX",
                    "sourceS3Configuration": {
                        "s3Bucket": "nsx-exports",
                        "s3BucketOwner": EMULATOR_ACCOUNT,
                        "s3Key": "exports/nsx.json",
                    },
                }],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "two source configurations — the modelled maximum",
            json!({
                "name": "two sources",
                "sourceConfigurations": [
                    {
                        "sourceEnvironment": "VSPHERE",
                        "sourceS3Configuration": {
                            "s3Bucket": "vsphere-exports",
                            "s3BucketOwner": EMULATOR_ACCOUNT,
                            "s3Key": "exports/vsphere.zip",
                        },
                    },
                    {
                        "sourceEnvironment": "AWS_DISCOVERY_COLLECTOR",
                        "sourceS3Configuration": {
                            "s3Bucket": "discovery-exports",
                            "s3BucketOwner": EMULATOR_ACCOUNT,
                            "s3Key": "exports/discovery/collector-1.csv",
                        },
                    },
                ],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "HUB_AND_SPOKE", "inboundCidr": "172.16.0.0/16"},
            }),
        ),
        (
            "multi-account deployment provisioning new VPCs",
            json!({
                "name": "multi-account-plan",
                "targetDeployment": "MULTI_ACCOUNT",
                "vpcProvisioningStrategy": "CREATE_NEW",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "single-account deployment reusing existing VPCs with one CIDR mapping",
            json!({
                "name": "single-account-plan",
                "targetDeployment": "SINGLE_ACCOUNT",
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.8.0.0/16"}],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "three CIDR mappings",
            json!({
                "name": "remapped-plan",
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [
                    {"originalCidr": "10.0.0.0/16", "updatedCidr": "10.16.0.0/16"},
                    {"originalCidr": "172.16.4.0/22", "updatedCidr": "172.20.4.0/22"},
                    {"originalCidr": "192.168.0.0/28", "updatedCidr": "192.168.9.0/28"},
                ],
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "HUB_AND_SPOKE"},
            }),
        ),
        (
            "tags and scope tags",
            json!({
                "name": "tagged-plan",
                "tags": {"env": "prod", "team": "migration", "cost-center": "4711"},
                "scopeTags": {"BusinessUnit": "retail"},
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "256-character name and 600-character description — the modelled maxima",
            json!({
                "name": "n".repeat(256),
                "description": "d".repeat(600),
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "single-character name and an empty description — the modelled minima",
            json!({
                "name": "a",
                "description": "",
                "targetS3Configuration": target_s3("a"),
                "targetNetwork": {"topology": "ISOLATED_VPC", "outboundCidr": "1.2.3.0/28"},
            }),
        ),
        (
            "interior spaces and punctuation in the name",
            json!({
                "name": "SAP ERP  prod (wave 1)",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
    ]
}

/// Every created definition is minted, stored and reported from state: the record a
/// create answers echoes every configured member of the request, carries an id and an
/// ARN of the modelled shape, and is byte-for-byte the record a later get reports.
#[tokio::test]
async fn a_network_migration_definition_is_created_and_read_back_from_state() {
    // The model declares no `UninitializedAccountException` for either operation and
    // neither carries an `accountID` member, so no `InitializeService` runs here.
    let mgn = Mgn::new();
    let mut minted = Vec::new();

    for (case, request) in creation_cases() {
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateNetworkMigrationDefinition expected 201, got {status} with {created:?}"
        );

        let id = definition_id(&created);
        assert!(
            is_definition_id(&id),
            "case {case:?}: networkMigrationDefinitionID must match ^nmd-[0-9a-zA-Z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            created["arn"].as_str(),
            Some(
                format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:network-migration-definition/{id}")
                    .as_str()
            ),
            "case {case:?}: arn must name this region, the emulator account and the minted id, got {:?}",
            created["arn"]
        );

        for member in CONFIGURED_MEMBERS {
            assert_eq!(
                created[member], request[member],
                "case {case:?}: {member} must be the stored request member, got {:?}",
                created[member]
            );
        }
        for (member, want) in maintained_members(&request)
            .as_object()
            .expect("an object of maintained members")
        {
            assert_eq!(
                &created[member], want,
                "case {case:?}: {member} must be reported as {want:?}, got {:?}",
                created[member]
            );
        }

        let created_at = created["createdAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: createdAt must be an epoch-seconds number, got {created:?}")
        });
        let updated_at = created["updatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: updatedAt must be an epoch-seconds number, got {created:?}")
        });
        assert!(
            created_at > 0.0,
            "case {case:?}: createdAt must be after the epoch, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: nothing has updated a new definition, so updatedAt must equal createdAt, got {updated_at}"
        );

        // Round trip: the reader answers the stored record, and reading twice does not
        // change what the next caller sees.
        let (status, fetched) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with {fetched:?}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: a get must answer the created record unchanged, got {fetched:?}"
        );
        let (_, again) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            again, fetched,
            "case {case:?}: a second get must answer the same record, got {again:?}"
        );

        minted.push(id);
    }

    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every create mints a distinct id, got {minted:?}"
    );
    assert_eq!(
        mgn.store().lock().network_migration_definitions.len(),
        minted.len(),
        "every created definition is persisted exactly once"
    );
}

/// The operation is `@idempotent` and models no idempotency token, so a request whose
/// every member equals a stored definition's answers that definition — same id, same
/// `createdAt` — while a request that differs anywhere mints a new one.
///
/// This pins the chosen semantics for a DELIBERATE second identical create too: with no
/// token on the wire, a repeat of a request is indistinguishable from a retry of it, and
/// the model marks the operation `@idempotent`, so the repeat answers the definition the
/// first call created rather than a second copy of it.
#[tokio::test]
async fn an_identical_create_replays_the_stored_definition() {
    let mgn = Mgn::new();
    let base = json!({
        "name": "replayed-plan",
        "description": "first",
        "sourceConfigurations": [{
            "sourceEnvironment": "LOGICAL_MODEL",
            "sourceS3Configuration": {
                "s3Bucket": "model-exports",
                "s3BucketOwner": EMULATOR_ACCOUNT,
                "s3Key": "exports/model.json",
            },
        }],
        "targetS3Configuration": target_s3("migration-artifacts"),
        "targetNetwork": {"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/16"},
        "targetDeployment": "SINGLE_ACCOUNT",
        "vpcProvisioningStrategy": "USE_EXISTING",
        "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.4.0.0/16"}],
        "tags": {"env": "dev"},
        "scopeTags": {"BusinessUnit": "retail"},
    });

    let (status, first) = mgn.call(CREATE, &base).await;
    assert_eq!(status, 201, "the first create expected 201, got {first:?}");
    let (status, replay) = mgn.call(CREATE, &base).await;
    assert_eq!(status, 201, "a replay expected 201, got {replay:?}");
    assert_eq!(
        replay, first,
        "an identical request must answer the stored definition unchanged, got {replay:?}"
    );
    assert_eq!(
        mgn.store().lock().network_migration_definitions.len(),
        1,
        "a replay must not mint a second definition"
    );

    // One member at a time: each difference is a different definition.
    let differences: Vec<(&str, Value)> = vec![
        ("a different name", json!({"name": "other-plan"})),
        ("a different description", json!({"description": "second"})),
        ("no description", json!({"description": null})),
        (
            "a different target topology",
            json!({"targetNetwork": {"topology": "HUB_AND_SPOKE", "inboundCidr": "10.0.0.0/16"}}),
        ),
        (
            "a different target bucket",
            json!({"targetS3Configuration": target_s3("other-artifacts")}),
        ),
        ("a different deployment", json!({"targetDeployment": "MULTI_ACCOUNT"})),
        (
            "a different CIDR mapping",
            json!({"cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.5.0.0/16"}]}),
        ),
        ("no source configurations", json!({"sourceConfigurations": null})),
        ("a different tag value", json!({"tags": {"env": "prod"}})),
        ("an extra scope tag", json!({"scopeTags": {"BusinessUnit": "retail", "Tier": "gold"}})),
    ];
    let mut ids = vec![definition_id(&first)];
    for (case, overrides) in differences {
        let mut request = base.clone();
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            if value.is_null() {
                request
                    .as_object_mut()
                    .expect("an object request")
                    .remove(member);
            } else {
                request[member] = value.clone();
            }
        }
        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {created:?}"
        );
        let id = definition_id(&created);
        assert!(
            !ids.contains(&id),
            "case {case:?}: a request differing in one member must mint a new definition, got {id:?} again"
        );
        ids.push(id);
    }
    assert_eq!(
        mgn.store().lock().network_migration_definitions.len(),
        ids.len(),
        "each differing request is stored as its own definition"
    );
}

/// `cidrMappings` is accepted only under `vpcProvisioningStrategy` `USE_EXISTING` — the
/// rule the model documents on that member — and refused with the modelled
/// `ValidationException` (400) otherwise, whatever the mapping count.
#[tokio::test]
async fn cidr_mappings_need_the_use_existing_strategy() {
    let mgn = Mgn::new();
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
    let request = |strategy: Option<&str>, count: usize| -> Value {
        let mut body = json!({
            "name": format!("mapped-{}-{count}", strategy.unwrap_or("absent")),
            "targetS3Configuration": target_s3("migration-artifacts"),
            "targetNetwork": {"topology": "ISOLATED_VPC"},
            "cidrMappings": mappings(count),
        });
        if let Some(strategy) = strategy {
            body["vpcProvisioningStrategy"] = json!(strategy);
        }
        body
    };

    // `VpcProvisioningStrategy` is CREATE_NEW | USE_EXISTING, and the member is
    // optional, so "not USE_EXISTING" is CREATE_NEW or absent.
    for strategy in [None, Some("CREATE_NEW")] {
        for count in [1usize, 2, 50] {
            let case = format!("strategy {strategy:?} with {count} mapping(s)");
            let (status, body) = mgn.call(CREATE, &request(strategy, count)).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: cidrMappings outside USE_EXISTING must be 400 ValidationException, got {status} with {body:?}"
            );
        }
        // An empty list carries no mapping, so it is not the refused request.
        let case = format!("strategy {strategy:?} with no mappings");
        let (status, body) = mgn.call(CREATE, &request(strategy, 0)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {body:?}"
        );
    }
    for count in [0usize, 1, 50] {
        let case = format!("USE_EXISTING with {count} mapping(s)");
        let (status, body) = mgn.call(CREATE, &request(Some("USE_EXISTING"), count)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} with {body:?}"
        );
        // A member with no entries is reported absent rather than as an empty list,
        // the convention this crate's records already report `tags` under.
        let expected = if count == 0 {
            Value::Null
        } else {
            request(Some("USE_EXISTING"), count)["cidrMappings"].clone()
        };
        assert_eq!(
            body["cidrMappings"], expected,
            "case {case:?}: the stored mappings must be reported back, got {:?}",
            body["cidrMappings"]
        );
    }

    let stored = mgn.store().lock().network_migration_definitions.len();
    assert_eq!(
        stored, 5,
        "a refused create persists nothing: only the five accepted requests are stored, got {stored}"
    );
}

/// An id the store holds no definition for is the modelled `ResourceNotFoundException`
/// (404), whatever its shape, and creating one definition does not make another id
/// readable.
#[tokio::test]
async fn getting_an_unknown_definition_is_refused_as_not_found() {
    let mgn = Mgn::new();
    let unknown = [
        "nmd-00000000000000000",
        "nmd-zzzzzzzzzzzzzzzzz",
        "nmd-0A1b2C3d4E5f6G7h8",
    ];
    for id in unknown {
        let (status, body) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "id {id:?}: an unknown definition must be 404 ResourceNotFoundException, got {status} with {body:?}"
        );
    }

    let (status, created) = mgn
        .call(
            CREATE,
            &json!({
                "name": "only-definition",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        )
        .await;
    assert_eq!(status, 201, "expected 201, got {status} with {created:?}");
    for id in unknown {
        let (status, body) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "id {id:?}: another definition's existence does not make it readable, got {status} with {body:?}"
        );
    }
}

/// The store is bounded: past `NETWORK_MIGRATION_DEFINITION_QUOTA` a create is refused
/// with the modelled `ServiceQuotaExceededException` (402) and nothing is minted.
#[tokio::test]
async fn the_definition_store_is_bounded() {
    let mgn = Mgn::new();
    {
        let store = mgn.store();
        let mut guard = store.lock();
        for index in 0..NETWORK_MIGRATION_DEFINITION_QUOTA {
            let id = format!("nmd-{index:017}");
            let stored = guard.insert_network_migration_definition(NetworkMigrationDefinition {
                arn: format!(
                    "arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:network-migration-definition/{id}"
                ),
                network_migration_definition_id: id,
                account_id: EMULATOR_ACCOUNT.to_string(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                configuration: NetworkMigrationDefinitionConfiguration {
                    name: format!("seeded-{index}"),
                    target_s3_configuration: TargetS3Configuration {
                        s3_bucket: "migration-artifacts".to_string(),
                        s3_bucket_owner: EMULATOR_ACCOUNT.to_string(),
                    },
                    target_network: TargetNetwork {
                        topology: "ISOLATED_VPC".to_string(),
                        ..TargetNetwork::default()
                    },
                    ..NetworkMigrationDefinitionConfiguration::default()
                },
            });
            assert!(stored, "seeding definition {index} must store it");
        }
    }

    let (status, body) = mgn
        .call(
            CREATE,
            &json!({
                "name": "one-too-many",
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (402, "ServiceQuotaExceededException"),
        "a full store must be refused with the modelled quota error, got {status} with {body:?}"
    );
    assert_eq!(
        mgn.store().lock().network_migration_definitions.len(),
        NETWORK_MIGRATION_DEFINITION_QUOTA,
        "a refused create mints nothing"
    );
}

/// The modelled constraints are refused at the boundary, on each published bound and one
/// value past it, before any handler runs — so nothing out of shape reaches the store.
#[tokio::test]
async fn values_outside_the_modelled_constraints_are_refused() {
    let mgn = Mgn::new();
    let valid = json!({
        "name": "constraint-probe",
        "targetS3Configuration": target_s3("migration-artifacts"),
        "targetNetwork": {"topology": "ISOLATED_VPC"},
    });
    let with = |member: &str, value: Value| -> Value {
        let mut body = valid.clone();
        body[member] = value;
        body
    };

    // Bounds from `smithy_query(query="closure",operation="CreateNetworkMigrationDefinition")`:
    // name len 1..256, description len 0..600, sourceConfigurations len 0..2,
    // AccountID len 12..12, Cidr /16../28.
    let refused: Vec<(&str, Value)> = vec![
        ("an empty name — under the length minimum", with("name", json!(""))),
        (
            "a blank name — outside the name pattern",
            with("name", json!(" ")),
        ),
        (
            "a 257-character name — one past the maximum",
            with("name", json!("n".repeat(257))),
        ),
        (
            "a 601-character description — one past the maximum",
            with("description", json!("d".repeat(601))),
        ),
        (
            "an unmodelled topology",
            with("targetNetwork", json!({"topology": "FLAT"})),
        ),
        (
            "a /29 CIDR — one past the narrowest modelled prefix",
            with(
                "targetNetwork",
                json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/29"}),
            ),
        ),
        (
            "a /15 CIDR — one past the widest modelled prefix",
            with(
                "targetNetwork",
                json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/15"}),
            ),
        ),
        (
            "an 11-digit bucket owner — under the AccountID length",
            with("targetS3Configuration", json!({"s3Bucket": "b", "s3BucketOwner": "12345678901"})),
        ),
        (
            "three source configurations — one past the list maximum",
            with(
                "sourceConfigurations",
                json!([
                    {"sourceEnvironment": "NSX", "sourceS3Configuration": {"s3Bucket": "b", "s3BucketOwner": EMULATOR_ACCOUNT, "s3Key": "k"}},
                    {"sourceEnvironment": "VSPHERE", "sourceS3Configuration": {"s3Bucket": "b", "s3BucketOwner": EMULATOR_ACCOUNT, "s3Key": "k"}},
                    {"sourceEnvironment": "CISCO_ACI", "sourceS3Configuration": {"s3Bucket": "b", "s3BucketOwner": EMULATOR_ACCOUNT, "s3Key": "k"}},
                ]),
            ),
        ),
        (
            "no name at all — a required member",
            json!({
                "targetS3Configuration": target_s3("migration-artifacts"),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        ),
        (
            "no targetNetwork — a required member",
            json!({
                "name": "constraint-probe",
                "targetS3Configuration": target_s3("migration-artifacts"),
            }),
        ),
        (
            "a definition id outside the modelled pattern",
            json!({"networkMigrationDefinitionID": "nmd-short"}),
        ),
    ];

    for (case, body) in refused {
        let operation = if body.get("networkMigrationDefinitionID").is_some() {
            GET
        } else {
            CREATE
        };
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside a modelled constraint must be refused with 400, got {status} with {answer:?}"
        );
    }
    assert!(
        mgn.store()
            .lock()
            .network_migration_definitions
            .is_empty(),
        "no refused request may reach the store"
    );
}
