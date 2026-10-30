// SPDX-License-Identifier: CC0-1.0

//! The seam another service's crate puts an Amazon Connect instance into this
//! service's store through: `InstanceRecord::new`, the constructor this crate exposes
//! for the record its own `CreateInstance` writes.
//!
//! RULE under test: an instance registered through that constructor is a COMPLETE
//! stored instance — every reader projects it from the stored record exactly as it
//! projects one `CreateInstance` wrote, with the ARN, the service-linked role and the
//! console URL derived from the instance id, the region and the alias, the status
//! derived from the stored activation instant, and none of the resources that hang off
//! an instance present until an operation stores one, which the storage-configuration
//! association then does.
//!
//! Asserted over a TABLE spanning what the constructor derives over — three regions,
//! every modelled `DirectoryType`, an alias and none, and an activation instant already
//! past as well as one still ahead — rather than over the one fixture row another
//! crate's suite happens to register.

use std::time::{Duration, SystemTime};

use engine::EMULATOR_ACCOUNT_ID;
use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::DIRECTORY_TYPE_VALUES;
use service_com_amazonaws_connect_amazonconnectservice::state::{
    self as connect_state, INSTANCE_STATUS_ACTIVE, INSTANCE_STATUS_CREATION_IN_PROGRESS,
};

use super::harness::Connect;

/// How far ahead a row that is still activating puts its activation instant. Long
/// enough that the request this suite sends straight afterwards still arrives before
/// it, so the row reads the transient status rather than racing it.
const STILL_ACTIVATING: Duration = Duration::from_secs(60);

/// One registration: the region its ARNs are rooted in, the instance id it is keyed
/// by, its identity management type, its alias (or none), and whether its activation
/// instant is already past.
#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    region: &'static str,
    instance: &'static str,
    /// A member of the model's `DirectoryType` catalog, which
    /// [`every_case_names_a_modelled_directory_type`] checks this table against.
    directory_type: &'static str,
    alias: Option<&'static str>,
    active: bool,
}

/// The table every suite below is driven over.
const CASES: &[Case] = &[
    Case {
        name: "connect-managed, aliased, already active",
        region: "us-east-1",
        instance: "11111111-1111-4111-8111-111111111111",
        directory_type: "CONNECT_MANAGED",
        alias: Some("seam-one"),
        active: true,
    },
    Case {
        name: "saml, no alias, already active",
        region: "eu-west-2",
        instance: "22222222-2222-4222-8222-222222222222",
        directory_type: "SAML",
        alias: None,
        active: true,
    },
    Case {
        name: "existing directory, aliased, already active",
        region: "ap-southeast-1",
        instance: "33333333-3333-4333-8333-333333333333",
        directory_type: "EXISTING_DIRECTORY",
        alias: Some("seam-three"),
        active: true,
    },
    Case {
        name: "connect-managed, aliased, still activating",
        region: "us-west-2",
        instance: "44444444-4444-4444-8444-444444444444",
        directory_type: "CONNECT_MANAGED",
        alias: Some("seam-four"),
        active: false,
    },
    Case {
        name: "connect-managed, no alias, still activating",
        region: "us-east-1",
        instance: "55555555-5555-4555-8555-555555555555",
        directory_type: "CONNECT_MANAGED",
        alias: None,
        active: false,
    },
];

/// Put `case`'s instance into the store this emulator's handlers read, through the
/// constructor this crate exposes — the one step another service's fixture takes to
/// give the account an instance.
fn register(connect: &Connect, case: Case) {
    let state = connect_state::service_state(connect.engine())
        .expect("the shared in-memory engine carries this service's store");
    let now = SystemTime::now();
    let active_at = if case.active {
        now
    } else {
        now + STILL_ACTIVATING
    };
    state.lock().instances.insert(
        case.instance.to_string(),
        connect_state::InstanceRecord::new(
            case.region,
            case.instance,
            case.directory_type,
            case.alias.map(str::to_string),
            now,
            active_at,
        ),
    );
}

/// `DescribeInstance`: `GET /instance/{InstanceId}`.
async fn instance_of(connect: &Connect, instance_id: &str) -> (u16, Value) {
    connect
        .send("GET", &format!("/instance/{instance_id}"), None)
        .await
}

/// `ListQueues`: `GET /queues-summary/{InstanceId}`.
async fn queues_of(connect: &Connect, instance_id: &str) -> (u16, Value) {
    connect
        .send("GET", &format!("/queues-summary/{instance_id}"), None)
        .await
}

/// `ListContactFlows`: `GET /contact-flows-summary/{InstanceId}`.
async fn flows_of(connect: &Connect, instance_id: &str) -> (u16, Value) {
    connect
        .send(
            "GET",
            &format!("/contact-flows-summary/{instance_id}"),
            None,
        )
        .await
}

/// The table is written by hand, so it is pinned to the model's own value list: a row
/// naming anything else is a stale table, not a failing emulator.
#[test]
fn every_case_names_a_modelled_directory_type() {
    for case in CASES {
        assert!(
            DIRECTORY_TYPE_VALUES.contains(&case.directory_type),
            "case {case:?}: DirectoryType does not name {}",
            case.directory_type
        );
    }
    for directory_type in DIRECTORY_TYPE_VALUES {
        assert!(
            CASES.iter().any(|case| case.directory_type == *directory_type),
            "the table leaves the directory type {directory_type} unexercised"
        );
    }
}

/// `DescribeInstance` projects a registered instance from its stored record: the id it
/// was keyed by, the ARN of its region, the service-linked role and the console URL of
/// its alias, and the status its activation instant implies.
#[tokio::test]
async fn describe_instance_projects_a_registered_instance() {
    for case in CASES.iter().copied() {
        let connect = Connect::in_region(case.region);
        register(&connect, case);

        let (status, body) = instance_of(&connect, case.instance).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeInstance got {status} {body}",
            case.name
        );
        let instance = &body["Instance"];
        let id = case.instance;
        let region = case.region;
        let label = case.alias.unwrap_or(id);

        assert_eq!(instance["Id"], json!(id), "case {:?}: Id", case.name);
        assert_eq!(
            instance["Arn"],
            json!(format!(
                "arn:aws:connect:{region}:{EMULATOR_ACCOUNT_ID}:instance/{id}"
            )),
            "case {:?}: Arn, got {}",
            case.name,
            instance["Arn"]
        );
        assert_eq!(
            instance["IdentityManagementType"],
            json!(case.directory_type),
            "case {:?}: IdentityManagementType",
            case.name
        );
        assert_eq!(
            instance["InstanceAlias"],
            case.alias.map_or(Value::Null, |alias| json!(alias)),
            "case {:?}: InstanceAlias",
            case.name
        );
        assert_eq!(
            instance["ServiceRole"],
            json!(format!(
                "arn:aws:iam::{EMULATOR_ACCOUNT_ID}:role/aws-service-role/\
                 connect.amazonaws.com/AWSServiceRoleForAmazonConnect_{id}"
            )),
            "case {:?}: ServiceRole, got {}",
            case.name,
            instance["ServiceRole"]
        );
        assert_eq!(
            instance["InstanceAccessUrl"],
            json!(format!("https://{label}.my.connect.aws")),
            "case {:?}: InstanceAccessUrl, got {}",
            case.name,
            instance["InstanceAccessUrl"]
        );
        assert_eq!(
            instance["InboundCallsEnabled"],
            json!(true),
            "case {:?}: InboundCallsEnabled",
            case.name
        );
        assert_eq!(
            instance["OutboundCallsEnabled"],
            json!(true),
            "case {:?}: OutboundCallsEnabled",
            case.name
        );
        assert!(
            instance["CreatedTime"].is_number(),
            "case {:?}: expected an epoch-seconds CreatedTime, got {}",
            case.name,
            instance["CreatedTime"]
        );

        let expected_status = if case.active {
            INSTANCE_STATUS_ACTIVE
        } else {
            INSTANCE_STATUS_CREATION_IN_PROGRESS
        };
        assert_eq!(
            instance["InstanceStatus"],
            json!(expected_status),
            "case {:?}: InstanceStatus, got {}",
            case.name,
            instance["InstanceStatus"]
        );
        assert!(
            instance.get("Tags").is_none_or(Value::is_null),
            "case {:?}: an untagged instance reports no Tags, got {}",
            case.name,
            instance["Tags"]
        );
    }
}

/// A registered instance reports the SAME members as a created one: the record behind
/// it is the record `CreateInstance` writes, so no reader of it is left with a member
/// only one of the two carries.
#[tokio::test]
async fn a_registered_instance_reports_the_same_members_as_a_created_one() {
    for case in CASES.iter().copied() {
        let connect = Connect::in_region(case.region);
        register(&connect, case);
        // Untagged, and aliased exactly where the row is: `InstanceAlias` is absent
        // from both projections or present in both, so what is left to compare is
        // which members a reader gets rather than which optional ones were set.
        let mut body = json!({
            "IdentityManagementType": "CONNECT_MANAGED",
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
        });
        if case.alias.is_some() {
            body["InstanceAlias"] = json!("seam-created");
        }
        let (created_id, _) = connect.create_instance(body).await;

        let (_, registered) = instance_of(&connect, case.instance).await;
        let (_, created) = instance_of(&connect, &created_id).await;

        let members = |instance: &Value| -> Vec<String> {
            instance["Instance"]
                .as_object()
                .expect("an Instance object")
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(
            members(&registered),
            members(&created),
            "case {:?}: registered {} vs created {}",
            case.name,
            registered["Instance"],
            created["Instance"]
        );
    }
}

/// A registered instance starts bare — no queues and no flows hang off it, unlike the
/// default inventory a created instance is born with — and it can then be written to:
/// an association stores a storage configuration on it and the read of that
/// association answers from the stored record.
#[tokio::test]
async fn a_registered_instance_starts_bare_and_takes_writes() {
    for case in CASES.iter().copied() {
        let connect = Connect::in_region(case.region);
        register(&connect, case);
        let id = case.instance;

        let (status, queues) = queues_of(&connect, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListQueues got {status} {queues}",
            case.name
        );
        assert_eq!(
            queues["QueueSummaryList"],
            json!([]),
            "case {:?}: a registered instance holds no queues, got {}",
            case.name,
            queues["QueueSummaryList"]
        );

        let (status, flows) = flows_of(&connect, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListContactFlows got {status} {flows}",
            case.name
        );
        assert_eq!(
            flows["ContactFlowSummaryList"],
            json!([]),
            "case {:?}: a registered instance holds no flows, got {}",
            case.name,
            flows["ContactFlowSummaryList"]
        );

        // `AssociateInstanceStorageConfig` then stores one configuration on the
        // registered instance, and `DescribeInstanceStorageConfig` reads it back —
        // the round trip that proves the record this instance was registered with
        // holds the state an operation writes onto an instance.
        let config = json!({
            "StorageType": "KINESIS_STREAM",
            "KinesisStreamConfig": {
                "StreamArn":
                    format!("arn:aws:kinesis:{}:{EMULATOR_ACCOUNT_ID}:stream/{id}", case.region),
            },
        });
        let (status, associated) = connect
            .send(
                "PUT",
                &format!("/instance/{id}/storage-config"),
                Some(json!({ "ResourceType": "AGENT_EVENTS", "StorageConfig": config.clone() })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: AssociateInstanceStorageConfig got {status} {associated}",
            case.name
        );
        let association_id = associated["AssociationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected an AssociationId, got {associated}",
                    case.name
                )
            })
            .to_string();

        let (status, read_back) = connect
            .send(
                "GET",
                &format!(
                    "/instance/{id}/storage-config/{association_id}?resourceType=AGENT_EVENTS"
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeInstanceStorageConfig got {status} {read_back}",
            case.name
        );
        assert_eq!(
            read_back["StorageConfig"]["KinesisStreamConfig"],
            config["KinesisStreamConfig"],
            "case {:?}: the stored configuration reads back unchanged, got {}",
            case.name,
            read_back["StorageConfig"]
        );
        assert_eq!(
            read_back["StorageConfig"]["AssociationId"],
            json!(association_id),
            "case {:?}: the read carries the association id, got {}",
            case.name,
            read_back["StorageConfig"]
        );
    }
}
