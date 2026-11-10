// SPDX-License-Identifier: CC0-1.0

//! `DescribeReplicationConfigurationTemplates` over HTTP: what the store holds is what
//! the operation reports.
//!
//! The rules under test:
//!
//! * The operation reports the replication configuration templates of the account the
//!   request was signed for, in creation order, each one as the stored record —
//!   whichever of the optional replication settings that record carries.
//! * `replicationConfigurationTemplateIDs` SELECTS: absent or empty it selects every
//!   template of the account, a list selects the named ones, and an id the account holds
//!   no template of selects nothing.
//! * `maxResults` cuts the selection into pages a `nextToken` resumes; a string this
//!   service never issued is refused with the modelled `ValidationException`.
//! * Until `InitializeService` has run for the account, the operation is the modelled
//!   `UninitializedAccountException`; another account's templates are never this
//!   account's to read.
//!
//! Nothing writes this store yet — `CreateReplicationConfigurationTemplate` is still a
//! scaffolded stub — so the records are seeded through the service's own state, the way
//! `wave_and_connector_lifecycle.rs` seeds launch configuration templates.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::ops::types::{
    FsxOntapConfiguration, StorageConfiguration,
};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    replication_configuration_template_arn, ReplicationConfigurationTemplate,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The ARN a stored template carries, as this service words it. For the emulator's own
/// account that is what [`replication_configuration_template_arn`] mints, so the two
/// spellings cannot drift; another account's ARN differs only in the account segment.
fn template_arn(account: &str, template_id: &str) -> String {
    let spelled =
        format!("arn:aws:mgn:{REGION}:{account}:replication-configuration-template/{template_id}");
    if account == EMULATOR_ACCOUNT {
        let minted = replication_configuration_template_arn(REGION, template_id);
        assert_eq!(
            minted, spelled,
            "the state helper must spell a template ARN this way, got {minted:?}"
        );
    }
    spelled
}

/// One seeded template of `account`, with every optional member unset.
fn bare(account: &str, template_id: &str) -> ReplicationConfigurationTemplate {
    ReplicationConfigurationTemplate {
        replication_configuration_template_id: template_id.to_string(),
        arn: template_arn(account, template_id),
        account_id: account.to_string(),
        ..ReplicationConfigurationTemplate::default()
    }
}

/// The class a stored template spans: which optional replication settings it carries,
/// each modelled enum's values, both ends of the `bandwidthThrottling` range (0..10000),
/// security-group lists from none to several, and tag maps from none to several. The ids
/// differ in their body characters, inside the modelled `^rct-[0-9a-zA-Z]{17}$`
/// (length 21..21).
///
/// Sources for the value lists:
/// `smithy_query(query="shape",shape="ReplicationConfigurationDefaultLargeStagingDiskType")`
/// → `GP2`, `ST1`, `GP3`; `…shape="ReplicationConfigurationEbsEncryption"` → `DEFAULT`,
/// `CUSTOM`; `…shape="ReplicationConfigurationDataPlaneRouting"` → `PRIVATE_IP`,
/// `PUBLIC_IP`; `…shape="InternetProtocol"` → `IPV4`, `IPV6`.
fn template_cases() -> Vec<(&'static str, ReplicationConfigurationTemplate, Value)> {
    let minimal_id = "rct-00000000000000000";
    let default_id = "rct-zzzzzzzzzzzzzzzzz";
    let custom_id = "rct-A1b2C3d4E5f6G7h8I";
    let st1_id = "rct-00000000000000001";

    vec![
        (
            "only the id and the ARN: every optional setting unset",
            bare(EMULATOR_ACCOUNT, minimal_id),
            json!({
                "replicationConfigurationTemplateID": minimal_id,
                "arn": template_arn(EMULATOR_ACCOUNT, minimal_id),
                "bandwidthThrottling": 0,
            }),
        ),
        (
            "GP2 / DEFAULT / PRIVATE_IP / IPV4, three security groups, no tags, \
             bandwidth throttling at the lower bound",
            ReplicationConfigurationTemplate {
                staging_area_subnet_id: Some("subnet-0123456789abcdef".to_string()),
                associate_default_security_group: Some(true),
                replication_servers_security_groups_ids: vec![
                    "sg-00000000000000001".to_string(),
                    "sg-00000000000000002".to_string(),
                    "sg-00000000000000003".to_string(),
                ],
                replication_server_instance_type: Some("t3.small".to_string()),
                use_dedicated_replication_server: Some(false),
                default_large_staging_disk_type: Some("GP2".to_string()),
                ebs_encryption: Some("DEFAULT".to_string()),
                bandwidth_throttling: 0,
                data_plane_routing: Some("PRIVATE_IP".to_string()),
                create_public_ip: Some(false),
                internet_protocol: Some("IPV4".to_string()),
                ..bare(EMULATOR_ACCOUNT, default_id)
            },
            json!({
                "replicationConfigurationTemplateID": default_id,
                "arn": template_arn(EMULATOR_ACCOUNT, default_id),
                "stagingAreaSubnetId": "subnet-0123456789abcdef",
                "associateDefaultSecurityGroup": true,
                "replicationServersSecurityGroupsIDs": [
                    "sg-00000000000000001",
                    "sg-00000000000000002",
                    "sg-00000000000000003",
                ],
                "replicationServerInstanceType": "t3.small",
                "useDedicatedReplicationServer": false,
                "defaultLargeStagingDiskType": "GP2",
                "ebsEncryption": "DEFAULT",
                "bandwidthThrottling": 0,
                "dataPlaneRouting": "PRIVATE_IP",
                "createPublicIP": false,
                "internetProtocol": "IPV4",
            }),
        ),
        (
            "GP3 / CUSTOM / PUBLIC_IP / IPV6 with a key ARN, tags, staging-area tags, \
             a storage configuration and bandwidth throttling at the upper bound",
            ReplicationConfigurationTemplate {
                staging_area_subnet_id: Some("subnet-ABCDEF0123456789".to_string()),
                associate_default_security_group: Some(false),
                replication_servers_security_groups_ids: vec!["sg-0000000000000000a".to_string()],
                replication_server_instance_type: Some("m5.large".to_string()),
                use_dedicated_replication_server: Some(true),
                default_large_staging_disk_type: Some("GP3".to_string()),
                ebs_encryption: Some("CUSTOM".to_string()),
                ebs_encryption_key_arn: Some(format!(
                    "arn:aws:kms:{REGION}:{EMULATOR_ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
                )),
                bandwidth_throttling: 10_000,
                data_plane_routing: Some("PUBLIC_IP".to_string()),
                create_public_ip: Some(true),
                staging_area_tags: [("env".to_string(), "prod".to_string())]
                    .into_iter()
                    .collect(),
                use_fips_endpoint: Some(true),
                tags: [
                    ("env".to_string(), "prod".to_string()),
                    ("team".to_string(), "migration".to_string()),
                ]
                .into_iter()
                .collect(),
                internet_protocol: Some("IPV6".to_string()),
                store_snapshot_on_local_zone: Some(true),
                storage_configuration: Some(StorageConfiguration {
                    storage_type: "FSX_ONTAP".to_string(),
                    fsx_ontap_configuration: Some(FsxOntapConfiguration {
                        storage_virtual_machine_id: "svm-0123456789abcdef0".to_string(),
                        credentials_secret_arn: format!(
                            "arn:aws:secretsmanager:{REGION}:{EMULATOR_ACCOUNT}:secret:ontap"
                        ),
                    }),
                }),
                ..bare(EMULATOR_ACCOUNT, custom_id)
            },
            json!({
                "replicationConfigurationTemplateID": custom_id,
                "arn": template_arn(EMULATOR_ACCOUNT, custom_id),
                "stagingAreaSubnetId": "subnet-ABCDEF0123456789",
                "associateDefaultSecurityGroup": false,
                "replicationServersSecurityGroupsIDs": ["sg-0000000000000000a"],
                "replicationServerInstanceType": "m5.large",
                "useDedicatedReplicationServer": true,
                "defaultLargeStagingDiskType": "GP3",
                "ebsEncryption": "CUSTOM",
                "ebsEncryptionKeyArn": format!(
                    "arn:aws:kms:{REGION}:{EMULATOR_ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
                ),
                "bandwidthThrottling": 10_000,
                "dataPlaneRouting": "PUBLIC_IP",
                "createPublicIP": true,
                "stagingAreaTags": {"env": "prod"},
                "useFipsEndpoint": true,
                "tags": {"env": "prod", "team": "migration"},
                "internetProtocol": "IPV6",
                "storeSnapshotOnLocalZone": true,
                "storageConfiguration": {
                    "storageType": "FSX_ONTAP",
                    "fsxOntapConfiguration": {
                        "storageVirtualMachineId": "svm-0123456789abcdef0",
                        "credentialsSecretArn": format!(
                            "arn:aws:secretsmanager:{REGION}:{EMULATOR_ACCOUNT}:secret:ontap"
                        ),
                    },
                },
            }),
        ),
        (
            "ST1 disk type, staging-area tags only, a mid-range bandwidth throttling",
            ReplicationConfigurationTemplate {
                default_large_staging_disk_type: Some("ST1".to_string()),
                bandwidth_throttling: 1_500,
                staging_area_tags: [
                    ("owner".to_string(), "migration".to_string()),
                    ("wave".to_string(), "1".to_string()),
                ]
                .into_iter()
                .collect(),
                ..bare(EMULATOR_ACCOUNT, st1_id)
            },
            json!({
                "replicationConfigurationTemplateID": st1_id,
                "arn": template_arn(EMULATOR_ACCOUNT, st1_id),
                "defaultLargeStagingDiskType": "ST1",
                "bandwidthThrottling": 1_500,
                "stagingAreaTags": {"owner": "migration", "wave": "1"},
            }),
        ),
    ]
}

/// Seed `templates` into the store, in the order given.
fn seed(mgn: &Mgn, templates: Vec<ReplicationConfigurationTemplate>) {
    let store = mgn.store();
    let mut state = store.lock();
    for template in templates {
        let id = template.replication_configuration_template_id.clone();
        let stored = state.insert_replication_configuration_template(template);
        assert!(
            stored,
            "seeding {id:?} must store it (the store bound was not reached)"
        );
    }
}

/// `DescribeReplicationConfigurationTemplates` reports every stored template of the
/// account, in creation order, exactly as the record holds it — and the same record when
/// the request selects it by id.
#[tokio::test]
async fn every_stored_template_is_reported_from_state() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let cases = template_cases();
    seed(
        &mgn,
        cases
            .iter()
            .map(|(_, template, _)| template.clone())
            .collect(),
    );

    let (status, listed) = mgn
        .call("DescribeReplicationConfigurationTemplates", &json!({}))
        .await;
    assert_eq!(
        status, 200,
        "expected 200, got {status} with {listed:?}"
    );
    let expected: Vec<Value> = cases
        .iter()
        .map(|(_, _, expected)| expected.clone())
        .collect();
    assert_eq!(
        listed["items"].as_array(),
        Some(&expected),
        "the operation must report every stored template in creation order, got {listed:?}"
    );

    for (case, template, expected) in &cases {
        let id = &template.replication_configuration_template_id;
        let (status, selected) = mgn
            .call(
                "DescribeReplicationConfigurationTemplates",
                &json!({"replicationConfigurationTemplateIDs": [id]}),
            )
            .await;
        assert_eq!(
            (status, selected["items"].as_array().map(Vec::as_slice)),
            (200, Some([expected.clone()].as_slice())),
            "case {case:?}: selecting {id:?} must report exactly the stored record, got \
             {status} with {selected:?}"
        );
    }
}

/// `replicationConfigurationTemplateIDs` selects: absent or empty selects everything,
/// a list selects the named templates, and an id the account holds no template of
/// selects nothing.
#[tokio::test]
async fn the_template_ids_member_selects() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let ids = [
        "rct-00000000000000000",
        "rct-zzzzzzzzzzzzzzzzz",
        "rct-A1b2C3d4E5f6G7h8I",
    ];
    seed(
        &mgn,
        ids.iter()
            .map(|id| bare(EMULATOR_ACCOUNT, id))
            .collect::<Vec<_>>(),
    );

    // An id no template of this account carries, inside the modelled id shape.
    let unknown = "rct-fffffffffffffffff";
    let selections: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no ids member", json!({}), ids.to_vec()),
        (
            "an empty ids list",
            json!({"replicationConfigurationTemplateIDs": []}),
            ids.to_vec(),
        ),
        (
            "one id",
            json!({"replicationConfigurationTemplateIDs": [ids[1]]}),
            vec![ids[1]],
        ),
        (
            "two ids, named in the reverse of creation order",
            json!({"replicationConfigurationTemplateIDs": [ids[2], ids[0]]}),
            vec![ids[0], ids[2]],
        ),
        (
            "every id",
            json!({"replicationConfigurationTemplateIDs": ids}),
            ids.to_vec(),
        ),
        (
            "an id of no template",
            json!({"replicationConfigurationTemplateIDs": [unknown]}),
            vec![],
        ),
        (
            "a known and an unknown id",
            json!({"replicationConfigurationTemplateIDs": [ids[0], unknown]}),
            vec![ids[0]],
        ),
        (
            "the same id twice",
            json!({"replicationConfigurationTemplateIDs": [ids[0], ids[0]]}),
            vec![ids[0]],
        ),
    ];

    for (case, request, wanted) in selections {
        let (status, answer) = mgn
            .call("DescribeReplicationConfigurationTemplates", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let reported: Vec<String> = answer["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item["replicationConfigurationTemplateID"]
                    .as_str()
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: the selection must report exactly those templates, in creation \
             order, got {answer:?}"
        );
    }
}

/// The selection is paged: a `maxResults` page carries a `nextToken` only while
/// templates remain, following the tokens reports every template exactly once in
/// creation order, and a string this service never issued cannot be resumed.
#[tokio::test]
async fn the_selection_is_paged_in_creation_order() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let seeded: Vec<String> = (0..5)
        .map(|index| format!("rct-0000000000000000{index}"))
        .collect();
    seed(
        &mgn,
        seeded
            .iter()
            .map(|id| bare(EMULATOR_ACCOUNT, id))
            .collect::<Vec<_>>(),
    );

    for page_size in [1usize, 2, 5, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, page) = mgn
                .call("DescribeReplicationConfigurationTemplates", &request)
                .await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected 200, got {status} with {page:?}"
            );
            let items = page["items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page carries at most that many templates, got {}",
                items.len()
            );
            seen.extend(items.iter().map(|item| {
                item["replicationConfigurationTemplateID"]
                    .as_str()
                    .unwrap_or("<absent>")
                    .to_string()
            }));
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, seeded,
            "page size {page_size}: following the tokens must report every template once, in \
             creation order, got {seen:?}"
        );
    }

    for token in ["not-a-token", "mgn-replication-configuration-templates-rct-nosuchtemplate000"] {
        let (status, answer) = mgn
            .call(
                "DescribeReplicationConfigurationTemplates",
                &json!({"nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, "ValidationException"),
            "token {token:?}: an unresumable nextToken must be refused, got {status} with \
             {answer:?}"
        );
    }
}

/// The operation reads nothing of another account, and nothing at all until
/// `InitializeService` has run for the caller's account.
#[tokio::test]
async fn the_account_gate_and_the_account_partition_hold() {
    // Uninitialized: the modelled refusal, whatever the request selects.
    let mgn = Mgn::new();
    for (case, request) in [
        ("no ids member", json!({})),
        (
            "an id of a template the store does hold",
            json!({"replicationConfigurationTemplateIDs": ["rct-00000000000000000"]}),
        ),
        (
            // The account gate is answered ahead of the cursor check: an account that
            // was never initialized has no selection for a token to continue.
            "a nextToken this service never issued",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a page size",
            json!({"maxResults": 1}),
        ),
    ] {
        seed(&mgn, vec![bare(EMULATOR_ACCOUNT, "rct-00000000000000000")]);
        let (status, answer) = mgn
            .call("DescribeReplicationConfigurationTemplates", &request)
            .await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, "UninitializedAccountException"),
            "case {case:?}: an uninitialized account must be refused, got {status} with \
             {answer:?}"
        );
    }

    // Initialized, but the only templates belong to another account.
    let mgn = Mgn::new();
    mgn.initialize().await;
    seed(
        &mgn,
        vec![
            bare(OTHER_ACCOUNT, "rct-11111111111111111"),
            bare(OTHER_ACCOUNT, "rct-22222222222222222"),
        ],
    );
    for (case, request) in [
        ("no ids member", json!({})),
        (
            "an id of another account's template",
            json!({"replicationConfigurationTemplateIDs": ["rct-11111111111111111"]}),
        ),
    ] {
        let (status, answer) = mgn
            .call("DescribeReplicationConfigurationTemplates", &request)
            .await;
        assert_eq!(
            (status, answer["items"].as_array().map(Vec::len)),
            (200, Some(0)),
            "case {case:?}: another account's templates are not this account's to read, got \
             {status} with {answer:?}"
        );
    }

    // The separation is the store's own: an insert under an id another account already
    // holds is refused rather than taking that account's record over.
    let taken = "rct-11111111111111111";
    let stored = mgn
        .store()
        .lock()
        .insert_replication_configuration_template(bare(EMULATOR_ACCOUNT, taken));
    assert!(
        !stored,
        "case {taken:?}: an insert under another account's id must be refused"
    );
    let (status, answer) = mgn
        .call("DescribeReplicationConfigurationTemplates", &json!({}))
        .await;
    assert_eq!(
        (status, answer["items"].as_array().map(Vec::len)),
        (200, Some(0)),
        "case {taken:?}: the refused insert must leave this account holding nothing, got \
         {status} with {answer:?}"
    );
}
