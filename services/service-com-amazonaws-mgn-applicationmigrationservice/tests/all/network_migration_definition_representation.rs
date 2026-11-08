// SPDX-License-Identifier: CC0-1.0

//! The `NetworkMigrationDefinition` representation every network-migration-definition
//! operation answers with, end to end over HTTP.
//!
//! The rule these tests pin: the service maintains a `targetDeployment` and a
//! `vpcProvisioningStrategy` on every definition — the value the create named, or the
//! service's own value where the create named none — and the definition's tag container
//! is part of that representation whether or not it holds entries, so it is reported as
//! the map it is (empty rather than absent), while `scopeTags`, a setting rather than
//! that container, stays absent while it is empty. Nothing of the three moves because a
//! later request did not mention it, and the create, the update and the read project the
//! one record.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and reads the
//! service's state back only through the service's own operations.
//!
//! The harness (`Mgn`, the region and account constants) is the one
//! `application_lifecycle.rs` defines; `call` takes the operation's modelled URI path, so
//! these operations are reached under `/network-migration/<Operation>`.

use serde_json::{json, Value};

use crate::application_lifecycle::{Mgn, EMULATOR_ACCOUNT};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `GetNetworkMigrationDefinition`, likewise.
const GET: &str = "network-migration/GetNetworkMigrationDefinition";

/// The modelled `http.uri` of `UpdateNetworkMigrationDefinition`, likewise.
const UPDATE: &str = "network-migration/UpdateNetworkMigrationDefinition";

/// The modelled `http.uri` of `ListNetworkMigrationDefinitions`, likewise.
const LIST: &str = "network-migration/ListNetworkMigrationDefinitions";

/// The `TargetDeployment` a definition created without one is maintained under, stated
/// from the spec side: one of the two values the model declares for the member
/// (`smithy_query(query="shape",shape="TargetDeployment")`: `SINGLE_ACCOUNT` |
/// `MULTI_ACCOUNT`).
const MAINTAINED_TARGET_DEPLOYMENT: &str = "SINGLE_ACCOUNT";

/// The `VpcProvisioningStrategy` a definition created without one is maintained under,
/// likewise one of the model's two values
/// (`smithy_query(query="shape",shape="VpcProvisioningStrategy")`: `CREATE_NEW` |
/// `USE_EXISTING`).
const MAINTAINED_VPC_PROVISIONING_STRATEGY: &str = "CREATE_NEW";

/// The modelled `TagKey` / `TagValue` maximum length
/// (`smithy_query(query="shape",shape="TagsMap")`: key and value `@length` 0..256).
const MAX_TAG_TEXT_LEN: usize = 256;

/// A `targetS3Configuration` as the model requires it: a bucket name and a 12-digit
/// owner account.
fn target_s3(bucket: &str) -> Value {
    json!({"s3Bucket": bucket, "s3BucketOwner": EMULATOR_ACCOUNT})
}

/// A create request carrying the required members only, plus `extra` merged over it.
fn create_request(name: &str, extra: &Value) -> Value {
    let mut request = json!({
        "name": name,
        "targetS3Configuration": target_s3("migration-artifacts"),
        "targetNetwork": {"topology": "ISOLATED_VPC"},
    });
    for (member, value) in extra.as_object().expect("an object of extra members") {
        request[member] = value.clone();
    }
    request
}

/// The class a created definition spans along what the rule generalises over: whether
/// the request names neither, one or both of the two maintained settings, which value it
/// names, how many tags it carries (none at all, an explicit empty map, one, several at
/// the modelled text bounds), and whether the `scopeTags` setting is present, empty or
/// absent beside them.
fn creation_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("neither setting and no tag member at all", json!({})),
        (
            "an explicitly empty tag map, and an empty scope-tag map beside it",
            json!({"tags": {}, "scopeTags": {}}),
        ),
        (
            "only the deployment target named, as the other of the two values",
            json!({"targetDeployment": "MULTI_ACCOUNT"}),
        ),
        (
            "only the provisioning strategy named, as the other of the two values",
            json!({
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.8.0.0/16"}],
            }),
        ),
        (
            "both named as the values the service maintains anyway, with one tag",
            json!({
                "targetDeployment": "SINGLE_ACCOUNT",
                "vpcProvisioningStrategy": "CREATE_NEW",
                "tags": {"env": "prod"},
            }),
        ),
        (
            "both named as the other values, with several tags at the modelled text bounds",
            json!({
                "targetDeployment": "MULTI_ACCOUNT",
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [{"originalCidr": "172.16.4.0/22", "updatedCidr": "172.20.4.0/22"}],
                "tags": {
                    "owner": "",
                    "note": "wave 1, phase 2 (pilot)",
                    "k".repeat(MAX_TAG_TEXT_LEN): "v".repeat(MAX_TAG_TEXT_LEN),
                },
            }),
        ),
        (
            "no setting named, scope tags present and no tags",
            json!({"scopeTags": {"BusinessUnit": "retail"}}),
        ),
        (
            "no setting named, a description and two source configurations",
            json!({
                "description": "two sources, no settings",
                "sourceConfigurations": [
                    {
                        "sourceEnvironment": "NSX",
                        "sourceS3Configuration": {
                            "s3Bucket": "nsx-exports",
                            "s3BucketOwner": EMULATOR_ACCOUNT,
                            "s3Key": "exports/nsx.json",
                        },
                    },
                    {
                        "sourceEnvironment": "VSPHERE",
                        "sourceS3Configuration": {
                            "s3Bucket": "vsphere-exports",
                            "s3BucketOwner": EMULATOR_ACCOUNT,
                            "s3Key": "exports/vsphere.zip",
                        },
                    },
                ],
            }),
        ),
    ]
}

/// What `request` must leave on the record for the two maintained settings and for the
/// tag container: the value the request named, the service's own where it named none,
/// and the tag container as a map — empty rather than absent.
fn expected_maintained(request: &Value) -> Value {
    let named_or = |member: &str, maintained: &str| match request[member].clone() {
        Value::Null => json!(maintained),
        named => named,
    };
    json!({
        "targetDeployment": named_or("targetDeployment", MAINTAINED_TARGET_DEPLOYMENT),
        "vpcProvisioningStrategy": named_or(
            "vpcProvisioningStrategy",
            MAINTAINED_VPC_PROVISIONING_STRATEGY,
        ),
        "tags": match request["tags"].clone() {
            Value::Null => json!({}),
            named => named,
        },
    })
}

/// Assert that `record` reports exactly `want`, member by member, naming the case and
/// the value it actually carried.
fn assert_maintained(case: &str, where_from: &str, record: &Value, want: &Value) {
    for (member, value) in want.as_object().expect("an object of members") {
        assert_eq!(
            &record[member], value,
            "case {case:?}: {where_from} must report {member} as {value:?}, got {:?} in \
             {record:?}",
            record[member]
        );
    }
    assert!(
        record["tags"].is_object(),
        "case {case:?}: {where_from} must report the tag container as a map, got {:?}",
        record["tags"]
    );
}

fn definition_id(record: &Value) -> String {
    record["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("no networkMigrationDefinitionID in {record:?}"))
        .to_string()
}

/// The create answers both maintained settings and the tag container, a read answers the
/// same record, and a partial update that mentions none of the three leaves all three as
/// the create left them — for every row of the class.
#[tokio::test]
async fn the_maintained_settings_and_the_tag_container_are_reported_by_every_projection() {
    // Every row gets its own emulator: `CreateNetworkMigrationDefinition` is
    // `@idempotent` with no token, so two rows whose members agreed would share one
    // record.
    for (case, extra) in creation_cases() {
        let mgn = Mgn::new();
        let request = create_request("maintained-settings-plan", &extra);
        let want = expected_maintained(&request);

        let (status, created) = mgn.call(CREATE, &request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateNetworkMigrationDefinition expected 201, got {status} with \
             {created:?}"
        );
        assert_maintained(case, "the create", &created, &want);
        // `scopeTags` is a setting, not the tag container: an empty one is absent.
        let expected_scope_tags = match request["scopeTags"].clone() {
            Value::Object(pairs) if pairs.is_empty() => Value::Null,
            other => other,
        };
        assert_eq!(
            created["scopeTags"], expected_scope_tags,
            "case {case:?}: scopeTags must be {expected_scope_tags:?}, got {:?}",
            created["scopeTags"]
        );

        let id = definition_id(&created);
        let (status, fetched) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with \
             {fetched:?}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: a read must answer the created record unchanged, got {fetched:?}"
        );

        // Partial updates that mention none of the three, applied one after another: each
        // answers the three as the create left them, and so does a read afterwards.
        let patches = vec![
            ("naming no member but the id", json!({})),
            ("naming only the description", json!({"description": "rewritten"})),
            ("naming only the name", json!({"name": "renamed-plan"})),
            (
                "naming only the scope tags",
                json!({"scopeTags": {"BusinessUnit": "wholesale"}}),
            ),
        ];
        for (patch_case, patch) in patches {
            let case = format!("{case} / a partial update {patch_case}");
            let mut update = patch.clone();
            update["networkMigrationDefinitionID"] = json!(id);
            let (status, answered) = mgn.call(UPDATE, &update).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateNetworkMigrationDefinition expected 200, got {status} \
                 with {answered:?}"
            );
            assert_maintained(&case, "the update", &answered, &want);

            let (status, after) = mgn
                .call(GET, &json!({"networkMigrationDefinitionID": id}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with \
                 {after:?}"
            );
            assert_maintained(&case, "the read after the update", &after, &want);
        }

        // The listing projects the same record: its summary declares the tag container
        // too, so it reports the same map.
        let (status, listed) = mgn.call(LIST, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNetworkMigrationDefinitions expected 200, got {status} with \
             {listed:?}"
        );
        let summary = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no items array in {listed:?}"))
            .iter()
            .find(|item| item["networkMigrationDefinitionID"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is not listed in {listed:?}"))
            .clone();
        assert_eq!(
            summary["tags"], want["tags"],
            "case {case:?}: the summary must report the same tag container {:?}, got {:?}",
            want["tags"], summary["tags"]
        );
    }
}

/// The other side of the rule: an update that NAMES one of the two settings replaces it —
/// to either of the modelled values, including the one the service maintains by default —
/// and the replacement is what a later read reports.
#[tokio::test]
async fn an_update_that_names_a_maintained_setting_replaces_it() {
    // Both values of each member, in both directions, so no row depends on the stored
    // value being the service's default. CIDR mappings travel with the strategy because
    // the model allows them only under `USE_EXISTING`.
    let moves: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the deployment target to the other value",
            json!({"targetDeployment": "MULTI_ACCOUNT"}),
            "targetDeployment",
            "MULTI_ACCOUNT",
        ),
        (
            "the deployment target to the value it already holds",
            json!({"targetDeployment": "SINGLE_ACCOUNT"}),
            "targetDeployment",
            "SINGLE_ACCOUNT",
        ),
        (
            "the provisioning strategy to the other value",
            json!({"vpcProvisioningStrategy": "USE_EXISTING"}),
            "vpcProvisioningStrategy",
            "USE_EXISTING",
        ),
        (
            "the provisioning strategy to the value it already holds",
            json!({"vpcProvisioningStrategy": "CREATE_NEW"}),
            "vpcProvisioningStrategy",
            "CREATE_NEW",
        ),
    ];

    for (case, patch, member, expected) in moves {
        let mgn = Mgn::new();
        let (status, created) = mgn
            .call(CREATE, &create_request("replaced-settings-plan", &json!({})))
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the base create expected 201, got {status} with {created:?}"
        );
        let id = definition_id(&created);

        let mut update = patch.clone();
        update["networkMigrationDefinitionID"] = json!(id);
        let (status, answered) = mgn.call(UPDATE, &update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateNetworkMigrationDefinition expected 200, got {status} with \
             {answered:?}"
        );
        assert_eq!(
            answered[member].as_str(),
            Some(expected),
            "case {case:?}: the update must report {member} as {expected:?}, got {:?}",
            answered[member]
        );

        let (status, after) = mgn
            .call(GET, &json!({"networkMigrationDefinitionID": id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetNetworkMigrationDefinition expected 200, got {status} with \
             {after:?}"
        );
        assert_eq!(
            after[member].as_str(),
            Some(expected),
            "case {case:?}: the read must report {member} as {expected:?}, got {:?}",
            after[member]
        );
        assert!(
            after["tags"].is_object(),
            "case {case:?}: the read must still report the tag container as a map, got {:?}",
            after["tags"]
        );
    }
}
