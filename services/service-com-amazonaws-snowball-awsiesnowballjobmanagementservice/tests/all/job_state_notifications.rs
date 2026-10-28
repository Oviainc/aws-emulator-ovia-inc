// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS notification a job's STATE CHANGE is published as, driven end to end with
//! the Amazon SNS service mounted beside Snowball on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so every assertion is made where a client sees
//! it: on the topic the order named.
//!
//! The RULE asserted here: whenever a job ENTERS a `JobState` — on being created, and on
//! each later transition this API makes — the job's own `Notification` settings decide
//! whether the service publishes that job's new state onto the topic they name. "When the
//! notification settings are defined during job creation, you can choose to notify based on
//! a specific set of job states using the `JobStatesToNotify` array of strings, or you can
//! specify that you want to have Amazon SNS notifications sent out for all job states with
//! `NotifyAll` set to true" (`Notification`), and `JobStatesToNotify` is "the list of job
//! states that will trigger a notification for this job", so a state the settings select is
//! published and a state they do not select is not. Each record that moves is a job in its
//! own right, so each is published on its own.
//!
//! The tables span the class rather than one recorded request: the first varies the
//! SELECTION against one transition (nine ways of configuring it, published and not), the
//! second varies the TRANSITION against settings that do and do not select it — every state
//! change this API makes, including one that is refused and therefore moves nothing — and
//! the third carries the rule onto the orders that move SEVERAL records at once (an export
//! order's main job and its job parts, a cluster's node jobs, a node added later).
//!
//! The Snowball harness (`router_with_engine`, `ok`, `refused`, `complete_order`,
//! `create_address`, `deliverable_address`, `string`, `job_metadata`, `base_cluster`,
//! `create_cluster`, `set_job_state`) and the Amazon SNS call helpers
//! (`order_named_resources::{owning_service, create_topic, element, form_encode, REGION}`)
//! are the ones this crate's other suites already drive the services with.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, create_cluster, job_metadata};
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, string,
};
use super::job_unlock_code::set_job_state;
use super::order_named_resources::{create_topic, form_encode, owning_service, REGION};

/// Snowball with the Amazon SNS service mounted beside it on the SAME engine, together with
/// that engine: a notification is published by an in-process call into the mounted SNS, and
/// the engine is what a row reads the deliveries back from and what a row that needs a job
/// already on a leg of its journey sets that state in.
fn router_with_sns() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = super::job_lifecycle::router_with_engine();
    router.register_service(
        ::service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// `Subscribe` one SQS queue to `topic_arn` on the mounted Amazon SNS service, so a publish
/// onto that topic is observable: SNS "delivers the message to each endpoint that is
/// subscribed to the topic", and a topic with no subscription holds no delivery to read
/// back. The queue is named as an ARN of this account and is never dialled by these rows.
async fn subscribe(router: &MultiServiceRouter, topic_arn: &str) {
    let case = format!("subscribe to {topic_arn}");
    let queue = format!(
        "arn:aws:sqs:{REGION}:{}:{}-inbox",
        ::emulator_server::emulator_account_id(),
        topic_arn.rsplit(':').next().unwrap_or_default()
    );
    let body = format!(
        "Action=Subscribe&Version=2010-03-31&TopicArn={}&Protocol=sqs&Endpoint={}",
        form_encode(topic_arn),
        form_encode(&queue)
    );
    let (status, answered) = owning_service(
        router,
        "sns",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        &body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Subscribe to answer 200, got {status} with {answered}"
    );
}

/// A topic on the mounted Amazon SNS service with a subscription on it, answering the ARN
/// SNS minted for it.
async fn given_topic(router: &MultiServiceRouter, name: &str) -> String {
    let topic_arn = create_topic(router, name).await;
    subscribe(router, &topic_arn).await;
    topic_arn
}

/// Every notification Amazon SNS has delivered, in the order it delivered them, each as
/// `(the topic it was published onto, the document published)`.
fn delivered(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, Value)> {
    let state = engine.state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>();
    let store = state.lock();
    let mut found = Vec::new();
    for messages in store.delivered.values() {
        for published in messages {
            let parsed = serde_json::from_str::<Value>(&published.message).unwrap_or_else(|error| {
                panic!(
                    "a published notification is not JSON ({error}): {:?}",
                    published.message
                )
            });
            found.push((published.topic_arn.clone(), parsed));
        }
    }
    found
}

/// The notifications delivered since `seen` of them had been, advancing it — so each row
/// reads only what its own calls published.
fn since(engine: &Arc<engine::InMemoryEngine>, seen: &mut usize) -> Vec<(String, Value)> {
    let all = delivered(engine);
    let fresh = all[(*seen).min(all.len())..].to_vec();
    *seen = all.len();
    fresh
}

/// One member of a published notification, as a string.
fn field(case: &str, document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the notification to carry {member}, got {document}")
        })
        .to_string()
}

/// Assert the notifications `published` are exactly one per expected `(job id, job state)`
/// pair, each onto `topic_arn`, each carrying an instant that PARSES as a timestamp.
///
/// The pairs are compared as a set: the order several records of one request are published
/// in is not part of the rule, and the state each record is in is.
fn assert_published(
    case: &str,
    published: &[(String, Value)],
    topic_arn: &str,
    expected: &[(String, String)],
) {
    let mut reported: Vec<(String, String)> = published
        .iter()
        .map(|(topic, document)| {
            assert_eq!(
                topic, topic_arn,
                "case {case:?}: expected the notification on {topic_arn}, got {topic}"
            );
            let instant = field(case, document, "Time");
            let parsed = ::aws_smithy_types::DateTime::from_str(
                &instant,
                ::aws_smithy_types::date_time::Format::DateTime,
            );
            assert!(
                parsed.is_ok(),
                "case {case:?}: expected the notification's instant to be a timestamp, got \
                 {instant:?}"
            );
            (
                field(case, document, "JobId"),
                field(case, document, "JobState"),
            )
        })
        .collect();
    reported.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the notifications {expected:?}, got {reported:?} from \
         {published:?}"
    );
}

/// The `JobState` `DescribeJob` reports for `job_id`, which is what a published notification
/// has to agree with.
async fn described_state(router: &MultiServiceRouter, job_id: &str, case: &str) -> String {
    let metadata = job_metadata(router, job_id, case).await;
    string(&metadata, "JobState", case)
}

/// Place an order on `address_id` carrying `notification`, and answer the `JobId` minted.
async fn create_job(
    router: &MultiServiceRouter,
    address_id: &str,
    notification: Option<&Value>,
    submitted: &Value,
    case: &str,
) -> String {
    let mut body = complete_order(submitted);
    body["AddressId"] = json!(address_id);
    if let Some(notification) = notification {
        body["Notification"] = notification.clone();
    }
    let created = ok(router, "CreateJob", body, case).await;
    string(&created, "JobId", case)
}

/// An address every order in this suite ships to.
async fn notified_address(router: &MultiServiceRouter) -> String {
    create_address(
        router,
        deliverable_address("Notified Receiver", "7 Notify Lane", json!({})),
        "the orders' shipping address",
    )
    .await
}

/// A created job is published onto its topic exactly when its own settings select the state
/// it was created in, over the ways a `Notification` can be configured.
#[tokio::test]
async fn a_created_job_is_published_only_when_its_settings_select_its_state() {
    let (router, engine) = router_with_sns();
    let topic = given_topic(&router, "snow-job-states").await;
    let pickup_topic = given_topic(&router, "snow-job-pickup").await;
    let address_id = notified_address(&router).await;
    let mut seen = delivered(&engine).len();

    // Each row is one way of configuring the notification of an order, and whether a job
    // created in `New` is published for it. The states named are the wire spelling of the
    // modelled `JobState` enum.
    let rows: Vec<(&str, Option<Value>, Value, bool)> = vec![
        (
            "NotifyAll over every state",
            Some(json!({ "SnsTopicARN": topic, "NotifyAll": true })),
            json!({}),
            true,
        ),
        (
            "the entered state listed among others",
            Some(json!({
                "SnsTopicARN": topic,
                "JobStatesToNotify": ["New", "Cancelled", "Complete"],
                "NotifyAll": false
            })),
            json!({}),
            true,
        ),
        (
            "a single-entry list naming the entered state",
            Some(json!({ "SnsTopicARN": topic, "JobStatesToNotify": ["New"] })),
            json!({}),
            true,
        ),
        (
            "NotifyAll on a local-use order for another device",
            Some(json!({ "SnsTopicARN": topic, "NotifyAll": true })),
            json!({ "JobType": "LOCAL_USE", "SnowballType": "SNC1_SSD" }),
            true,
        ),
        (
            "a list naming only states the job is not in",
            Some(json!({
                "SnsTopicARN": topic,
                "JobStatesToNotify": ["WithCustomer", "Complete"]
            })),
            json!({}),
            false,
        ),
        (
            "an empty list of states",
            Some(json!({ "SnsTopicARN": topic, "JobStatesToNotify": [] })),
            json!({}),
            false,
        ),
        (
            "a topic and no selection at all",
            Some(json!({ "SnsTopicARN": topic })),
            json!({}),
            false,
        ),
        (
            "NotifyAll with no topic to publish onto",
            Some(json!({ "NotifyAll": true })),
            json!({}),
            false,
        ),
        (
            "no Notification member at all",
            None,
            json!({}),
            false,
        ),
        (
            // "Used to send SNS notifications for the person picking up the device"
            // (`Notification$DevicePickupSnsTopicARN`): a different audience from the job's
            // state changes, so a job state is not published onto it.
            "NotifyAll with the pickup topic alone",
            Some(json!({ "DevicePickupSnsTopicARN": pickup_topic, "NotifyAll": true })),
            json!({}),
            false,
        ),
        (
            "NotifyAll with both topics, which publishes onto the job's own",
            Some(json!({
                "SnsTopicARN": topic,
                "DevicePickupSnsTopicARN": pickup_topic,
                "NotifyAll": true
            })),
            json!({}),
            true,
        ),
    ];

    for (case, notification, submitted, published) in rows {
        let job_id = create_job(
            &router,
            &address_id,
            notification.as_ref(),
            &submitted,
            case,
        )
        .await;
        let state = described_state(&router, &job_id, case).await;
        assert_eq!(
            state, "New",
            "case {case:?}: expected the created job in New, got {state:?}"
        );
        let fresh = since(&engine, &mut seen);
        let expected = if published {
            vec![(job_id.clone(), state)]
        } else {
            Vec::new()
        };
        assert_published(case, &fresh, &topic, &expected);
    }
}

/// Every later transition this API makes is published when the job's settings select the
/// state it moves the job INTO, and a call that moves nothing publishes nothing.
#[tokio::test]
async fn each_transition_is_published_when_the_settings_select_the_state_entered() {
    let (router, engine) = router_with_sns();
    let topic = given_topic(&router, "snow-job-transitions").await;
    let address_id = notified_address(&router).await;
    let mut seen = delivered(&engine).len();

    // Each row is one transition this API makes: the state the job is in when the call is
    // made, the call itself, and the state the call leaves the job in — or `None` for a call
    // the service refuses, which moves nothing.
    let transitions: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "CancelJob from the state a job is created in",
            "New",
            json!({ "__operation": "CancelJob" }),
            Some("Cancelled"),
        ),
        (
            "a device reported RECEIVED while it is in transit to the customer",
            "InTransitToCustomer",
            json!({ "__operation": "UpdateJobShipmentState", "ShipmentState": "RECEIVED" }),
            Some("WithCustomer"),
        ),
        (
            "a device reported RETURNED while it is with the customer",
            "WithCustomer",
            json!({ "__operation": "UpdateJobShipmentState", "ShipmentState": "RETURNED" }),
            Some("InTransitToAWS"),
        ),
        (
            "a device reported RECEIVED before it ever shipped, which is refused",
            "New",
            json!({ "__operation": "UpdateJobShipmentState", "ShipmentState": "RECEIVED" }),
            None,
        ),
        (
            "CancelJob on a job past its cancellation window, which is refused",
            "PreparingShipment",
            json!({ "__operation": "CancelJob" }),
            None,
        ),
    ];

    for (transition, from_state, call, entered) in transitions {
        // Each transition is driven against settings that select the state it enters and
        // settings that do not, so every row asserts both halves of the rule.
        for (selection, notification, selects) in [
            (
                "NotifyAll",
                json!({ "SnsTopicARN": topic, "NotifyAll": true }),
                true,
            ),
            (
                "the entered state listed",
                json!({
                    "SnsTopicARN": topic,
                    "JobStatesToNotify": [entered.unwrap_or("New"), "Listing"]
                }),
                true,
            ),
            (
                "a list naming only another state",
                json!({ "SnsTopicARN": topic, "JobStatesToNotify": ["Pending"] }),
                false,
            ),
        ] {
            let case = &format!("{transition} with {selection}");
            let job_id = create_job(
                &router,
                &address_id,
                Some(&notification),
                &json!({}),
                case,
            )
            .await;
            // A job this emulator mints is `New` and no operation of this API ships a
            // device, so a row that needs a job already on a leg sets that state in the
            // store the handler reads before calling.
            if from_state != "New" {
                set_job_state(&engine, &job_id, from_state);
            }
            // What the create itself published is the first suite's subject, not this one's.
            let _ = since(&engine, &mut seen);

            let mut body = call.clone();
            let operation = string(&body, "__operation", case);
            body.as_object_mut()
                .expect("the call is a JSON object")
                .remove("__operation");
            body["JobId"] = json!(job_id);
            match entered {
                Some(_) => {
                    ok(&router, &operation, body, case).await;
                }
                // "Updates the state when a shipment state changes to a different state",
                // and "you can only cancel a job before its JobState value changes to
                // PreparingAppliance": a call outside its window moves nothing, so there is
                // no state change to publish.
                None => {
                    refused(&router, &operation, body, "InvalidJobStateException", case).await;
                }
            }

            let reported = described_state(&router, &job_id, case).await;
            let expected_state = entered.unwrap_or(from_state);
            assert_eq!(
                reported, expected_state,
                "case {case:?}: expected the job in {expected_state}, got {reported:?}"
            );
            let fresh = since(&engine, &mut seen);
            let expected = if selects && entered.is_some() {
                vec![(job_id.clone(), reported)]
            } else {
                Vec::new()
            };
            assert_published(case, &fresh, &topic, &expected);
        }
    }
}

/// An order that moves SEVERAL job records publishes one notification per record: each job
/// part of an export order and each node job of a cluster is a job in its own right, with an
/// id of its own and a state of its own.
#[tokio::test]
async fn every_record_an_order_moves_is_published_on_its_own() {
    let (router, engine) = router_with_sns();
    let topic = given_topic(&router, "snow-order-records").await;
    let address_id = notified_address(&router).await;
    let notification = json!({ "SnsTopicARN": topic, "NotifyAll": true });
    let mut seen = delivered(&engine).len();

    // An export order mints a main job plus a job part per device
    // ("each main job will have at least one job part", `JobListEntry$IsMaster`), and every
    // one of them is created in `New`.
    let case = "an export order's main job and its job parts";
    let main_job_id = create_job(
        &router,
        &address_id,
        Some(&notification),
        &json!({ "JobType": "EXPORT" }),
        case,
    )
    .await;
    let described = ok(
        &router,
        "DescribeJob",
        json!({ "JobId": main_job_id }),
        case,
    )
    .await;
    let parts: Vec<String> = described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected SubJobMetadata, got {described}"))
        .iter()
        .map(|part| string(part, "JobId", case))
        .collect();
    assert!(
        !parts.is_empty(),
        "case {case:?}: expected the export order to hold job parts, got {described}"
    );
    let mut expected: Vec<(String, String)> = vec![(main_job_id.clone(), "New".to_string())];
    expected.extend(parts.iter().map(|part| (part.clone(), "New".to_string())));
    assert_published(case, &since(&engine, &mut seen), &topic, &expected);

    // Cancelling the main job cancels the order, so every record that moves is published
    // again, now in the state it moved to.
    let case = "cancelling an export order";
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": main_job_id }),
        case,
    )
    .await;
    let mut expected: Vec<(String, String)> = vec![(main_job_id.clone(), "Cancelled".to_string())];
    expected.extend(
        parts
            .iter()
            .map(|part| (part.clone(), "Cancelled".to_string())),
    );
    assert_published(case, &since(&engine, &mut seen), &topic, &expected);

    // A cluster's node jobs inherit the cluster's notification settings, and each node is
    // created in `New` ("You use the CreateJob action separately to create the jobs for each
    // of these nodes", `CreateCluster`, whose `InitialClusterSize` creates them with it).
    let case = "a cluster created with its node jobs";
    let nodes_asked = 3;
    let mut cluster_order = base_cluster(&address_id);
    cluster_order["Notification"] = notification.clone();
    cluster_order["InitialClusterSize"] = json!(nodes_asked);
    cluster_order["ForceCreateJobs"] = json!(true);
    let (cluster_id, entries) = create_cluster(&router, cluster_order, case).await;
    let nodes: Vec<String> = entries
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect();
    assert_eq!(
        nodes.len(),
        nodes_asked,
        "case {case:?}: expected {nodes_asked} node jobs, got {entries:?}"
    );
    let expected: Vec<(String, String)> = nodes
        .iter()
        .map(|node| (node.clone(), "New".to_string()))
        .collect();
    assert_published(case, &since(&engine, &mut seen), &topic, &expected);

    // A node added to the cluster later is one more job entering `New`, under the settings
    // it inherits from the cluster: a node names its cluster and nothing of the order
    // besides ("you only need to provide the clusterId value").
    let case = "a node job added to the cluster";
    let created = ok(
        &router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    let added = string(&created, "JobId", case);
    assert_published(
        case,
        &since(&engine, &mut seen),
        &topic,
        &[(added.clone(), "New".to_string())],
    );

    // Cancelling the cluster cancels every node it ordered, so each of them is published in
    // the state it moved to.
    let case = "cancelling the cluster";
    ok(
        &router,
        "CancelCluster",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    let mut expected: Vec<(String, String)> = nodes
        .iter()
        .map(|node| (node.clone(), "Cancelled".to_string()))
        .collect();
    expected.push((added, "Cancelled".to_string()));
    assert_published(case, &since(&engine, &mut seen), &topic, &expected);
}
