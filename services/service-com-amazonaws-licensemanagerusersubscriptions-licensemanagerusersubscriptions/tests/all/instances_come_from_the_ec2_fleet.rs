// SPDX-License-Identifier: CC0-1.0

//! An instance the account has that is STOPPED is still a target the user-association
//! plane serves.
//!
//! The rule under test, the state half of the instance resolution
//! ([`super::instance_fleet_resolution`] asserts the rest): the account has every instance
//! Amazon EC2 holds for it that has not been torn down, so what decides a user association
//! is whether an instance of that id exists — not which state it is running in. An
//! instance that was stopped still exists, is still billed for its storage, and is still
//! the instance a user-based subscription is associated with; only a termination takes it
//! out of the account's instances, which the sibling suite's `terminated` row pins.
//!
//! The rows span the states a stop takes an instance through, read from EC2 itself rather
//! than assumed: the transient one the call leaves it in, and the one EC2's clock owes it.
//! Each row drives both operations that act on a caller-named instance over the real wire,
//! and the instance is one `ec2:RunInstances` launched in this run — no id is written here.
//!
//! The harness (this service and EC2 over one engine, with an EC2 client over the same
//! in-process callback the service's own resolution uses) is the sibling suite's, so there
//! is ONE such fixture rather than a copy per suite.

use crate::instance_fleet_resolution::{
    associate, list_associations, register_provider, Fleet,
};

/// The `InstanceState$Name` a stop takes an instance through: the transient state the call
/// leaves it in, and the one EC2's own clock owes it. Spelled out here rather than read
/// from a production constant, so a change to either fails a test.
const STATE_STOPPING: &str = "stopping";
const STATE_STOPPED: &str = "stopped";

/// The status a fresh association carries, spelled out for the same reason.
const STATUS_ASSOCIATING: &str = "ASSOCIATING";

/// RULE: an instance the account has in a state a stop put it in is still one the
/// user-association plane serves — `AssociateUser` mints the association on it and
/// `ListUserAssociations` reports it afterwards.
#[tokio::test]
async fn a_stopped_instance_is_still_an_instance_the_account_has() {
    let fleet = Fleet::new();
    register_provider(&fleet.h).await;

    // One instance per row, so a row's stop cannot change what another row observes.
    let launched = fleet.launch(2).await;

    for (index, wanted) in [STATE_STOPPING, STATE_STOPPED].into_iter().enumerate() {
        let instance_id = launched[index].clone();
        fleet
            .ec2
            .stop_instances()
            .instance_ids(&instance_id)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: EC2 must stop {instance_id}: {error:?}")
            });
        // The state EC2 itself reports, waited for rather than assumed: the transient one
        // is observed straight after the call, the settled one once EC2's clock owes it.
        let reported = if wanted == STATE_STOPPING {
            fleet.instance_state(&instance_id).await
        } else {
            fleet.settled_in(&instance_id, STATE_STOPPED).await
        };
        let case = format!("an instance EC2 reports as {reported}");
        assert!(
            reported == STATE_STOPPING || reported == STATE_STOPPED,
            "case {case:?}: a stopped instance must be reported stopping or stopped, \
             EC2 reports {reported:?}"
        );

        let username = format!("user{index}");
        let reply = associate(&fleet.h, &username, &instance_id).await;
        let summary = &reply.ok(&case)["InstanceUserSummary"];
        assert_eq!(
            summary.get("InstanceId").and_then(serde_json::Value::as_str),
            Some(instance_id.as_str()),
            "case {case:?}: the association must name the stopped instance, got {summary:?}"
        );
        assert_eq!(
            summary.get("Status").and_then(serde_json::Value::as_str),
            Some(STATUS_ASSOCIATING),
            "case {case:?}: a fresh association is answered {STATUS_ASSOCIATING}, \
             got {summary:?}"
        );

        // And the read of that association is authorized the same way, and reports it.
        let listed = list_associations(&fleet.h, &instance_id).await;
        let users: Vec<&str> = listed.ok(&case)["InstanceUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {listed:?}"))
            .iter()
            .filter_map(|summary| summary.get("Username").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(
            users,
            vec![username.as_str()],
            "case {case:?}: ListUserAssociations must report the association just made, \
             got {users:?}"
        );
    }
}
