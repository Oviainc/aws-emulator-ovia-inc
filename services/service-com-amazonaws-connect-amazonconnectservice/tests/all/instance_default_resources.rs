// SPDX-License-Identifier: CC0-1.0

//! The resources a new Amazon Connect instance is born with, driven over HTTP through
//! the same `MultiServiceRouter` the emulator serves with.
//!
//! RULE under test: `CreateInstance` stands up a complete contact center, so the
//! instance it mints already owns its default inventory — the default contact flows, the
//! default `STANDARD` queue and the hours of operation that queue is attached to — and
//! every reader of those resources answers for them from the instance's own stored
//! state, without the caller having created anything.
//!
//! Each suite asserts that over a TABLE of `CreateInstance` requests (every
//! `DirectoryType` member, an alias and none, tags and none, more than one region)
//! rather than one recorded call, and over the whole default set rather than the one
//! flow type the issue cited.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    CONTACT_FLOW_STATE_ACTIVE, CONTACT_FLOW_STATUS_PUBLISHED, DEFAULT_CONTACT_FLOWS,
    DEFAULT_CONTACT_FLOW_CONTENT, DEFAULT_QUEUE_DESCRIPTION, DEFAULT_QUEUE_NAMES,
    QUEUE_STATUS_ENABLED, QUEUE_TYPE_STANDARD,
};
use sha2::{Digest, Sha256};

use super::harness::{connect_managed, ids_of, looks_like_a_uuid, query, Connect};

/// The `CreateInstance` requests every suite here spans: the three `DirectoryType`
/// members, an alias and none, tags and none, both call flags in both positions — the
/// default inventory is a property of a created instance, not of one request's shape.
fn instance_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "connect-managed, both channels, untagged",
            connect_managed("defaultsone"),
        ),
        (
            "saml, inbound only, one tag",
            json!({
                "IdentityManagementType": "SAML",
                "InstanceAlias": "defaultstwo",
                "InboundCallsEnabled": true,
                "OutboundCallsEnabled": false,
                "Tags": { "env": "prod" },
            }),
        ),
        (
            "existing directory, no alias, outbound only, two tags",
            json!({
                "IdentityManagementType": "EXISTING_DIRECTORY",
                "DirectoryId": "d-0123456789",
                "InboundCallsEnabled": false,
                "OutboundCallsEnabled": true,
                "Tags": { "a": "1", "b": "2" },
            }),
        ),
    ]
}

/// `ListContactFlows`, optionally filtered on `contactFlowTypes`.
async fn list_default_flows(
    connect: &Connect,
    instance_id: &str,
    flow_types: &[&str],
) -> (u16, Value) {
    let uri = format!(
        "/contact-flows-summary/{instance_id}{}",
        query(None, None, "contactFlowTypes", flow_types)
    );
    connect.send("GET", &uri, None).await
}

/// `ListQueues`, optionally filtered on `queueTypes`.
async fn list_default_queues(
    connect: &Connect,
    instance_id: &str,
    queue_types: &[&str],
) -> (u16, Value) {
    let uri = format!(
        "/queues-summary/{instance_id}{}",
        query(None, None, "queueTypes", queue_types)
    );
    connect.send("GET", &uri, None).await
}

/// `DescribeContactFlow`.
async fn describe_default_flow(connect: &Connect, instance_id: &str, flow_id: &str) -> (u16, Value) {
    connect
        .send("GET", &format!("/contact-flows/{instance_id}/{flow_id}"), None)
        .await
}

/// `DescribeQueue`.
async fn describe_default_queue(
    connect: &Connect,
    instance_id: &str,
    queue_id: &str,
) -> (u16, Value) {
    connect
        .send("GET", &format!("/queues/{instance_id}/{queue_id}"), None)
        .await
}

/// Every summary in a list, keyed by its `Name` member.
fn by_name(list: &Value) -> BTreeMap<String, Value> {
    list.as_array()
        .expect("a summary list")
        .iter()
        .map(|entry| {
            (
                entry["Name"].as_str().expect("Name").to_string(),
                entry.clone(),
            )
        })
        .collect()
}

/// RULE: a created instance already owns its default contact flows — `ListContactFlows`
/// reports the whole default set for an instance nobody has created a flow in, each one
/// a stored, published flow of the instance with its own minted id and instance-scoped
/// ARN, and a type filter keeps exactly the defaults of the types it names.
#[tokio::test]
async fn a_new_instance_is_born_with_its_default_contact_flows() {
    for (case, body) in instance_requests() {
        let connect = Connect::new();
        let (instance_id, instance_arn) = connect.create_instance(body).await;

        let (status, listed) = list_default_flows(&connect, &instance_id, &[]).await;
        assert_eq!(status, 200, "case {case:?}: ListContactFlows got {listed}");
        let summaries = &listed["ContactFlowSummaryList"];
        let flows = by_name(summaries);

        assert_eq!(
            flows.len(),
            DEFAULT_CONTACT_FLOWS.len(),
            "case {case:?}: expected the {} default flows, got {listed}",
            DEFAULT_CONTACT_FLOWS.len()
        );
        assert!(
            !flows.is_empty(),
            "case {case:?}: a new instance must expose default contact flows, got {listed}"
        );

        let mut ids: BTreeSet<String> = BTreeSet::new();
        for default in DEFAULT_CONTACT_FLOWS {
            let summary = flows.get(default.name).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the default flow {:?} is missing from {listed}",
                    default.name
                )
            });
            let id = summary["Id"].as_str().expect("Id").to_string();
            assert!(
                looks_like_a_uuid(&id),
                "case {case:?}/{:?}: expected a v4-UUID flow id, got {id:?}",
                default.name
            );
            assert!(
                ids.insert(id.clone()),
                "case {case:?}/{:?}: expected a fresh id, got the already-seen {id:?}",
                default.name
            );
            assert_eq!(
                summary["Arn"],
                json!(format!("{instance_arn}/contact-flow/{id}")),
                "case {case:?}/{:?}: a default flow's ARN is its instance's ARN plus \
                 /contact-flow/<Id>",
                default.name
            );
            assert_eq!(
                summary["ContactFlowType"],
                json!(default.flow_type),
                "case {case:?}/{:?}: ContactFlowType",
                default.name
            );
            assert_eq!(
                summary["ContactFlowState"],
                json!(CONTACT_FLOW_STATE_ACTIVE),
                "case {case:?}/{:?}: a default flow is ACTIVE",
                default.name
            );
            assert_eq!(
                summary["ContactFlowStatus"],
                json!(CONTACT_FLOW_STATUS_PUBLISHED),
                "case {case:?}/{:?}: a default flow is PUBLISHED, so a contact can be \
                 routed to it",
                default.name
            );

            // The summary is a projection of a stored flow: the describe answers for the
            // same id, with the content the checksum is computed from.
            let (status, described) = describe_default_flow(&connect, &instance_id, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{:?}: DescribeContactFlow got {described}",
                default.name
            );
            let flow = &described["ContactFlow"];
            assert_eq!(
                flow["Name"],
                json!(default.name),
                "case {case:?}/{:?}: Name",
                default.name
            );
            assert_eq!(
                flow["Type"],
                json!(default.flow_type),
                "case {case:?}/{:?}: Type",
                default.name
            );
            assert_eq!(
                flow["Content"],
                json!(DEFAULT_CONTACT_FLOW_CONTENT),
                "case {case:?}/{:?}: Content",
                default.name
            );
            assert_eq!(
                flow["FlowContentSha256"],
                json!(hex::encode(Sha256::digest(
                    DEFAULT_CONTACT_FLOW_CONTENT.as_bytes()
                ))),
                "case {case:?}/{:?}: FlowContentSha256 is the SHA-256 of the stored content",
                default.name
            );
        }

        // One filter per type the default set covers, plus the pair of them: a filter
        // keeps exactly the defaults of the types it names. `CONTACT_FLOW` is the row the
        // issue cited — a client discovering a flow to route contacts to.
        let covered: BTreeSet<&str> = DEFAULT_CONTACT_FLOWS
            .iter()
            .map(|default| default.flow_type)
            .collect();
        for flow_type in &covered {
            let (status, filtered) = list_default_flows(&connect, &instance_id, &[flow_type]).await;
            assert_eq!(status, 200, "case {case:?}/{flow_type}: got {filtered}");
            let wanted: BTreeSet<String> = DEFAULT_CONTACT_FLOWS
                .iter()
                .filter(|default| default.flow_type == *flow_type)
                .map(|default| default.name.to_string())
                .collect();
            let got: BTreeSet<String> = ids_of(&filtered["ContactFlowSummaryList"], "Name")
                .into_iter()
                .collect();
            assert!(
                !got.is_empty(),
                "case {case:?}/{flow_type}: the default set covers this type, so the \
                 filtered list must be non-empty, got {filtered}"
            );
            assert_eq!(
                got, wanted,
                "case {case:?}/{flow_type}: expected exactly the defaults of this type, \
                 got {filtered}"
            );
        }

        // A type no default flow carries lists empty rather than everything.
        let uncovered = ["CAMPAIGN"];
        for flow_type in uncovered {
            assert!(
                !covered.contains(flow_type),
                "case {case:?}: {flow_type} is expected to carry no default flow"
            );
            let (status, filtered) = list_default_flows(&connect, &instance_id, &[flow_type]).await;
            assert_eq!(status, 200, "case {case:?}/{flow_type}: got {filtered}");
            assert_eq!(
                filtered["ContactFlowSummaryList"],
                json!([]),
                "case {case:?}/{flow_type}: no default flow carries this type, got {filtered}"
            );
        }
    }
}

/// RULE: a created instance already owns its default `STANDARD` queue, attached to the
/// instance's default hours of operation — `ListQueues(QueueTypes=["STANDARD"])` reports
/// it for an instance nobody has created a queue in, and `DescribeQueue` answers for the
/// same id from storage.
#[tokio::test]
async fn a_new_instance_is_born_with_its_default_standard_queue() {
    // The region travels in the SigV4 credential scope, so the same rule is asserted in
    // more than one: a default resource's ARN and region come from the request that
    // created the instance.
    for region in ["us-east-1", "eu-west-1"] {
        for (case, body) in instance_requests() {
            let case = format!("{case} in {region}");
            let connect = Connect::in_region(region);
            let (instance_id, instance_arn) = connect.create_instance(body).await;

            let (status, listed) =
                list_default_queues(&connect, &instance_id, &[QUEUE_TYPE_STANDARD]).await;
            assert_eq!(status, 200, "case {case:?}: ListQueues got {listed}");
            let queues = by_name(&listed["QueueSummaryList"]);
            assert_eq!(
                queues.len(),
                DEFAULT_QUEUE_NAMES.len(),
                "case {case:?}: expected the {} default STANDARD queue(s), got {listed}",
                DEFAULT_QUEUE_NAMES.len()
            );
            assert!(
                !queues.is_empty(),
                "case {case:?}: a new instance must expose a default STANDARD queue, \
                 got {listed}"
            );

            for name in DEFAULT_QUEUE_NAMES {
                let summary = queues
                    .get(name)
                    .unwrap_or_else(|| panic!("case {case:?}: {name} is missing from {listed}"));
                let id = summary["Id"].as_str().expect("Id").to_string();
                assert!(
                    looks_like_a_uuid(&id),
                    "case {case:?}/{name}: expected a v4-UUID queue id, got {id:?}"
                );
                assert_eq!(
                    summary["Arn"],
                    json!(format!("{instance_arn}/queue/{id}")),
                    "case {case:?}/{name}: a default queue's ARN is its instance's ARN \
                     plus /queue/<Id>"
                );
                assert_eq!(
                    summary["QueueType"],
                    json!(QUEUE_TYPE_STANDARD),
                    "case {case:?}/{name}: QueueType"
                );
                assert_eq!(
                    summary["LastModifiedRegion"],
                    json!(region),
                    "case {case:?}/{name}: a default queue carries the creating request's \
                     region"
                );

                let (status, described) =
                    describe_default_queue(&connect, &instance_id, &id).await;
                assert_eq!(status, 200, "case {case:?}/{name}: DescribeQueue got {described}");
                let queue = &described["Queue"];
                assert_eq!(queue["Name"], json!(name), "case {case:?}/{name}: Name");
                assert_eq!(
                    queue["QueueId"],
                    json!(id),
                    "case {case:?}/{name}: DescribeQueue answers for the listed id"
                );
                assert_eq!(
                    queue["Status"],
                    json!(QUEUE_STATUS_ENABLED),
                    "case {case:?}/{name}: a default queue is ENABLED"
                );
                assert_eq!(
                    queue["Description"],
                    json!(DEFAULT_QUEUE_DESCRIPTION),
                    "case {case:?}/{name}: Description"
                );

                // The default queue is attached to the instance's default hours of
                // operation: a minted id, the same one on every read.
                let hours_id = queue["HoursOfOperationId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}/{name}: expected a HoursOfOperationId in {described}")
                    })
                    .to_string();
                assert!(
                    looks_like_a_uuid(&hours_id),
                    "case {case:?}/{name}: expected a v4-UUID hours-of-operation id, \
                     got {hours_id:?}"
                );
                let (_, reread) = describe_default_queue(&connect, &instance_id, &id).await;
                assert_eq!(
                    reread["Queue"]["HoursOfOperationId"],
                    json!(hours_id),
                    "case {case:?}/{name}: a read must not change what a later read sees"
                );
            }

            // No default queue is an AGENT queue, and the unfiltered list is the same set.
            let (status, agents) = list_default_queues(&connect, &instance_id, &["AGENT"]).await;
            assert_eq!(status, 200, "case {case:?}: got {agents}");
            assert_eq!(
                agents["QueueSummaryList"],
                json!([]),
                "case {case:?}: the default inventory holds no AGENT queue, got {agents}"
            );
            let (_, unfiltered) = list_default_queues(&connect, &instance_id, &[]).await;
            assert_eq!(
                ids_of(&unfiltered["QueueSummaryList"], "Name")
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                DEFAULT_QUEUE_NAMES
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<BTreeSet<_>>(),
                "case {case:?}: an unfiltered list reports the same default queues, \
                 got {unfiltered}"
            );
        }
    }
}

/// RULE: the default inventory is STORED state of the instance that owns it — minted per
/// instance, so two instances share no default id; occupying its instance's name space,
/// so a same-name create there is the modelled duplicate refusal; and removable, so a
/// deleted default stays deleted rather than reappearing on the next read.
#[tokio::test]
async fn default_resources_are_stored_per_instance_state() {
    let connect = Connect::new();
    let (first, _) = connect.create_instance(connect_managed("orgone")).await;
    let (second, _) = connect.create_instance(connect_managed("orgtwo")).await;

    // Minted per instance: no identifier is shared between two instances' defaults.
    for (case, path, member) in [
        ("flows", "/contact-flows-summary", "ContactFlowSummaryList"),
        ("queues", "/queues-summary", "QueueSummaryList"),
    ] {
        let (_, mine) = connect.send("GET", &format!("{path}/{first}"), None).await;
        let (_, theirs) = connect.send("GET", &format!("{path}/{second}"), None).await;
        let mine: BTreeSet<String> = ids_of(&mine[member], "Id").into_iter().collect();
        let theirs: BTreeSet<String> = ids_of(&theirs[member], "Id").into_iter().collect();
        assert!(
            !mine.is_empty() && !theirs.is_empty(),
            "case {case:?}: both instances must own defaults, got {mine:?} and {theirs:?}"
        );
        assert!(
            mine.is_disjoint(&theirs),
            "case {case:?}: two instances must not share a default id, got {mine:?} \
             and {theirs:?}"
        );
    }

    // The defaults occupy their instance's name space, exactly as a created resource
    // does: a queue of the default queue's name is a 409 there, and free elsewhere once
    // the default holding the name is gone.
    for name in DEFAULT_QUEUE_NAMES {
        let (status, refused) = connect
            .send(
                "PUT",
                &format!("/queues/{first}"),
                Some(json!({ "Name": name, "HoursOfOperationId": "hop-1" })),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {name:?}: the instance already owns a queue of this name, got {status} \
             {refused}"
        );
    }
    for default in DEFAULT_CONTACT_FLOWS {
        let (status, refused) = connect
            .send(
                "PUT",
                &format!("/contact-flows/{first}"),
                Some(json!({
                    "Name": default.name,
                    "Type": default.flow_type,
                    "Content": DEFAULT_CONTACT_FLOW_CONTENT,
                })),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {:?}: the instance already owns a flow of this name, got {status} {refused}",
            default.name
        );
    }

    // Removable, and gone for good: a deleted default queue does not come back, which a
    // list synthesising defaults per request could not satisfy.
    let (_, listed) = list_default_queues(&connect, &first, &[]).await;
    let default_queue_ids = ids_of(&listed["QueueSummaryList"], "Id");
    for id in &default_queue_ids {
        let (status, _) = connect
            .send("DELETE", &format!("/queues/{first}/{id}"), None)
            .await;
        assert_eq!(status, 200, "case {id:?}: DeleteQueue of a default queue");
    }
    let (status, emptied) = list_default_queues(&connect, &first, &[]).await;
    assert_eq!(status, 200, "ListQueues got {emptied}");
    assert_eq!(
        emptied["QueueSummaryList"],
        json!([]),
        "a deleted default queue must stay deleted, got {emptied}"
    );
    for id in &default_queue_ids {
        assert_eq!(
            describe_default_queue(&connect, &first, id).await.0,
            404,
            "case {id:?}: a deleted default queue is gone from DescribeQueue"
        );
    }
    // The name it held is free again, and the queue created in its place is the only one.
    let (status, recreated) = connect
        .send(
            "PUT",
            &format!("/queues/{first}"),
            Some(json!({ "Name": DEFAULT_QUEUE_NAMES[0], "HoursOfOperationId": "hop-1" })),
        )
        .await;
    assert_eq!(
        status, 200,
        "the default queue's name is free once it is deleted, got {recreated}"
    );

    // The second instance kept its own defaults throughout.
    let (_, untouched) = list_default_queues(&connect, &second, &[QUEUE_TYPE_STANDARD]).await;
    assert_eq!(
        ids_of(&untouched["QueueSummaryList"], "Name").len(),
        DEFAULT_QUEUE_NAMES.len(),
        "deleting one instance's default must not touch another's, got {untouched}"
    );

    // And deleting the instance takes its defaults with it.
    assert_eq!(
        connect
            .send("DELETE", &format!("/instance/{second}"), None)
            .await
            .0,
        200,
        "DeleteInstance"
    );
    assert_eq!(
        list_default_queues(&connect, &second, &[]).await.0,
        404,
        "a deleted instance's default queues are gone with it"
    );
    assert_eq!(
        list_default_flows(&connect, &second, &[]).await.0,
        404,
        "a deleted instance's default flows are gone with it"
    );
}
