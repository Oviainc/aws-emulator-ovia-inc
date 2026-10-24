// SPDX-License-Identifier: CC0-1.0

//! The seam the resource-policy rule suites in this crate share, and the rule that a
//! resource-policy READ reports state and changes none of it.
//!
//! `PutResourcePolicy` and `GetResourcePolicy` are asserted from three angles in this
//! crate: `resource_policies` covers the attach/read contract of one resource (which
//! documents are policies, which ARN forms are refused, the modelled length bound),
//! `resource_policy_isolation` covers the registry's KEY — that a policy belongs to the
//! one resource its ARN names — and this module owns the helpers both of the latter
//! need plus the read-only rule below.
//!
//! Every case goes over the wire through the shared [`super::emulator::Emulator`]: an
//! `awsJson1_1` `POST /` with the `X-Amz-Target` an AWS SDK sends, dispatched by the
//! same `MultiServiceRouter` the production router serves, so what is asserted is what
//! a client sees — including the HTTP status and the error code.
//!
//! A dashboard is created OVER THE WIRE with `CreateDashboard`; an event data store and
//! a channel are SEEDED in the registries the handlers read, the way the
//! `resource_policies` suite seeds them, because `CreateEventDataStore` and
//! `CreateChannel` are still scaffold stubs with issues of their own.

use serde_json::{json, Value};
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, channels, EventDataStoreRecord, EventDataStoreState,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A second Region, so a table spans the Region field of a resource ARN as well as its
/// resource type.
pub const OTHER_REGION: &str = "us-west-2";

/// The identifier every resource in the shared table carries: a dashboard name, a
/// channel id and an event data store id that are all the same string, so only the
/// resource type and the Region tell their ARNs apart. It satisfies the dashboard-name
/// rule (`state::is_valid_dashboard_name`: 3..=128 alphanumerics, `_` or `-`).
pub const SHARED_ID: &str = "shared-resource-1";

/// A real-looking account id a policy may name as its principal, which is NOT the
/// account the emulator mints ARNs for.
const OTHER_ACCOUNT: &str = "123456789012";

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that needs a settled
/// dashboard does not race the instant the handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// A valid policy document naming `sid`, so each row's document is distinguishable from
/// every other row's.
pub fn document(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
            "Action": "cloudtrail:GetResourcePolicy",
        }],
    })
    .to_string()
}

/// Attach `policy` to `resource_arn`.
pub async fn put(emulator: &Emulator, resource_arn: &str, policy: &str) -> Reply {
    emulator
        .call(
            "PutResourcePolicy",
            &json!({"ResourceArn": resource_arn, "ResourcePolicy": policy}),
        )
        .await
}

/// Read the policy attached to `resource_arn`.
pub async fn get(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("GetResourcePolicy", &json!({"ResourceArn": resource_arn}))
        .await
}

/// Create the dashboard `name` in `region` over the wire and hand back its ARN, having
/// waited out the load the clock owes so it is ready to carry a policy.
pub async fn dashboard(emulator: &Emulator, name: &str, region: &str) -> String {
    let created = emulator
        .call_in("CreateDashboard", region, &json!({"Name": name}))
        .await;
    let arn = created
        .expect_ok(&format!("CreateDashboard {name} in {region}"))
        .get("DashboardArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateDashboard {name} reported no ARN: {created:?}"))
        .to_string();
    sleep(past_settle());
    arn
}

/// Seed the channel `id` in `region`, the way `CreateChannel` will once it is
/// implemented, and hand back its ARN.
pub fn channel(emulator: &Emulator, id: &str, region: &str) -> String {
    let channel_arn = channels::channel_arn(region, id);
    emulator.state().lock().channels.insert(
        (id.to_string(), region.to_string()),
        channels::ChannelRecord {
            channel_id: id.to_string(),
            name: format!("channel-{id}"),
            channel_arn: channel_arn.clone(),
            home_region: region.to_string(),
            // Every other member is one a resource-policy operation never reads.
            ..channels::ChannelRecord::default()
        },
    );
    channel_arn
}

/// Seed the event data store `id` in `region`, the way `CreateEventDataStore` will once
/// it is implemented, and hand back its ARN.
pub fn event_data_store(emulator: &Emulator, id: &str, region: &str) -> String {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status: EventDataStoreState::Enabled,
            // Every other member is one a resource-policy operation never reads.
            ..EventDataStoreRecord::default()
        },
    );
    state::event_data_store_arn(region, id)
}

/// Assert `reply` reports `policy` attached to `resource_arn`.
pub fn expect_policy(case: &str, reply: &Reply, resource_arn: &str, policy: &str) {
    let body = reply.expect_ok(case);
    assert_eq!(
        body.get("ResourceArn").and_then(Value::as_str),
        Some(resource_arn),
        "case {case:?}: expected the policy to be reported for {resource_arn}, got {body:?}"
    );
    assert_eq!(
        body.get("ResourcePolicy").and_then(Value::as_str),
        Some(policy),
        "case {case:?}: expected the document attached to {resource_arn}, got {body:?}"
    );
}

/// One addressed resource: the case that names it, its canonical ARN and the document
/// attached to it.
pub struct AddressedResource {
    pub case: &'static str,
    pub arn: String,
    pub policy: String,
}

/// The resources every table here spans: ONE identifier, five canonical ARNs.
///
/// The rows differ in resource TYPE (dashboard, channel, event data store) and in the
/// ARN's REGION, which together are everything that distinguishes two ARNs carrying the
/// same identifier. Each row's document names the row, so a document reported for the
/// wrong resource is visible in the assertion.
pub async fn addressed_resources(emulator: &Emulator) -> Vec<AddressedResource> {
    let rows: Vec<(&'static str, String)> = vec![
        (
            "a dashboard in the home Region",
            dashboard(emulator, SHARED_ID, HOME_REGION).await,
        ),
        (
            "a dashboard of the same name in another Region",
            dashboard(emulator, SHARED_ID, OTHER_REGION).await,
        ),
        (
            "a channel whose id is that same string",
            channel(emulator, SHARED_ID, HOME_REGION),
        ),
        (
            "a channel of that id in another Region",
            channel(emulator, SHARED_ID, OTHER_REGION),
        ),
        (
            "an event data store whose id is that same string",
            event_data_store(emulator, SHARED_ID, HOME_REGION),
        ),
    ];
    rows.into_iter()
        .map(|(case, arn)| AddressedResource {
            case,
            arn,
            policy: document(&case.replace(' ', "-")),
        })
        .collect()
}

/// Attach each row's own document, asserting the attach reports it back.
pub async fn attach_each(emulator: &Emulator, resources: &[AddressedResource]) {
    for resource in resources {
        expect_policy(
            &format!("attach to {}", resource.case),
            &put(emulator, &resource.arn, &resource.policy).await,
            &resource.arn,
            &resource.policy,
        );
    }
}

/// RULE: reading a resource policy reports state and changes none of it — a second read
/// of the same resource, and of every other resource, answers exactly as the first did,
/// and a resource carrying no policy keeps answering that it carries none.
#[tokio::test]
async fn reading_a_policy_does_not_change_what_a_later_read_sees() {
    let emulator = Emulator::new();
    let resources = addressed_resources(&emulator).await;
    let unattached = channel(&emulator, "unattached-channel", OTHER_REGION);
    attach_each(&emulator, &resources).await;

    // Three rounds, so a read that consumed, moved or regenerated something would show
    // up as a difference between the first round and the ones after it.
    for round in 1..=3 {
        for resource in &resources {
            expect_policy(
                &format!("{} on read {round}", resource.case),
                &get(&emulator, &resource.arn).await,
                &resource.arn,
                &resource.policy,
            );
        }
        // "This exception is thrown when the specified resource policy is not found":
        // the resource exists and carries no policy, on every read.
        get(&emulator, &unattached).await.expect_error(
            &format!("a resource with no policy on read {round}"),
            404,
            "ResourcePolicyNotFoundException",
        );
    }
}
