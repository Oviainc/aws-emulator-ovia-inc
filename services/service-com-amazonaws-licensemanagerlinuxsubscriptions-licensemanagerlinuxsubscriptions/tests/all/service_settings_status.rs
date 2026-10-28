// SPDX-License-Identifier: CC0-1.0

//! What the Linux subscriptions service settings report for an account that never
//! applied a settings change, over the real wire path (every call goes through the
//! router's `dispatch`).
//!
//! The rule under test: the settings reporting answers a member only from what the
//! account actually holds, so `Status` — which reports on a settings change HAVING been
//! applied — is absent until a change was applied, and carries the settled value from
//! then on. `LinuxSubscriptionsDiscovery` is the one member the no-settings account does
//! report, because discovery being off IS the state of an account that never enabled it.
//!
//! Each rule is asserted over a SET of inputs, not the one request that exposed the
//! defect: the table rows differ along what else the account holds, how many times and
//! from which signed Region the settings are read, and which switch value the change
//! that settles them carried.

use serde_json::{json, Value};

use crate::harness::{
    call_in_region, get_settings, harness, list_tags, seed_discovered_instance, seed_provider,
    tag, update_settings, Harness, Reply, ACCOUNT, REGION,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::DiscoveredInstanceRecord;

/// The `Status` a settled settings change reports — a member of the modelled `Status`
/// enum (`smithy_query(query="shape",shape="Status")`: `Completed | Failed | InProgress |
/// Successful`).
const SETTLED: &str = "Completed";
/// The `LinuxSubscriptionsDiscovery` value an account that never enabled discovery has
/// (`smithy_query(query="operation",operation="GetServiceSettings")`: `Disabled |
/// Enabled`).
const DISCOVERY_OFF: &str = "Disabled";

/// Regions the settings may be read from, to show the account's settings are not
/// per-Region state: entries of the published Region catalog across partitions, none of
/// them the Region the other suites sign with.
const READ_REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-northeast-1", "us-gov-west-1"];

/// Assert `body` carries no `member` at all (absent, or JSON `null`).
fn assert_absent(body: &Value, case: &str, member: &str) {
    assert!(
        body.get(member).is_none_or(Value::is_null),
        "case {case:?}: expected no {member}, got {:?}",
        body.get(member)
    );
}

/// The `Status` a settings body reports, or `None` when it reports none.
fn status(reply: &Reply, case: &str) -> Option<String> {
    reply
        .ok(case)
        .get("Status")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: Status must be a string, got {value:?}"))
                .to_string()
        })
}

/// What the whole no-settings report must look like, whatever else the account holds:
/// discovery off, and every member the account stored nothing for absent — `Status`
/// included, since no change was ever requested.
async fn assert_reports_no_applied_change(h: &Harness, case: &str) {
    for region in READ_REGIONS {
        let case = format!("{case} read in {region}");
        // Reading twice from the same Region as well, because a read must not change
        // what a later caller sees.
        for attempt in 0..2 {
            let case = format!("{case} (attempt {attempt})");
            let reply = call_in_region(
                h,
                region,
                "/subscription/GetServiceSettings",
                &json!({}),
            )
            .await;
            let body = reply.ok(&case).clone();
            assert_eq!(
                body.get("LinuxSubscriptionsDiscovery").and_then(Value::as_str),
                Some(DISCOVERY_OFF),
                "case {case:?}: expected discovery {DISCOVERY_OFF}, got {body:?}"
            );
            for member in [
                "Status",
                "StatusMessage",
                "LinuxSubscriptionsDiscoverySettings",
                "HomeRegions",
            ] {
                assert_absent(&body, &case, member);
            }
        }
    }
}

/// An account that never called `UpdateServiceSettings` reports NO `Status`, whatever
/// else of this service's state it holds.
#[tokio::test]
async fn settings_of_an_account_with_no_applied_change_report_no_status() {
    // Row 1: an entirely empty store.
    let h = harness();
    assert_reports_no_applied_change(&h, "empty store").await;

    // Row 2: an account holding a registered subscription provider — other state of this
    // service exists, but no settings change was applied.
    let h = harness();
    let arn = seed_provider(
        &h,
        "provider-with-no-settings",
        &format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:redhat-token"),
        &[],
    );
    assert_reports_no_applied_change(&h, "one registered provider").await;

    // Row 3: the same account after a tag write landed on that provider, so the store has
    // been mutated by an unrelated operation.
    let tagged = tag(&h, &arn, &json!({ "team": "linux" })).await;
    tagged.ok("tagging a provider");
    list_tags(&h, &arn).await.ok("tags readable back");
    assert_reports_no_applied_change(&h, "a tagged registered provider").await;

    // Row 4: an account whose discovery inventory holds instances (placed the way a
    // discovery writes them) but which still applied no settings change.
    let h = harness();
    for index in 0..3 {
        seed_discovered_instance(
            &h,
            DiscoveredInstanceRecord {
                instance_id: format!("i-0000000000000000{index}"),
                account_id: ACCOUNT.to_string(),
                region: REGION.to_string(),
                subscription_name: "RHEL".to_string(),
                subscription_type: "License included".to_string(),
                ..Default::default()
            },
        );
    }
    assert_reports_no_applied_change(&h, "discovered instances, no settings").await;
}

/// `Status` appears — with the settled value — exactly from the call that applies a
/// change, for either switch value, and stays there for every later read.
#[tokio::test]
async fn status_appears_only_once_a_change_has_been_applied() {
    // The rule generalises over the switch value the change carried and over the number
    // of changes applied, so the table walks a disable-first account and an enable-first
    // one through a sequence of changes.
    for first in [DISCOVERY_OFF, "Enabled"] {
        let h = harness();
        let case = format!("first change {first}");

        // Before the change: no status at all.
        assert_eq!(
            status(&get_settings(&h).await, &case),
            None,
            "case {case:?}: no Status is reported before any change is applied"
        );

        for (step, discovery) in [first, "Enabled", DISCOVERY_OFF, first].iter().enumerate() {
            let case = format!("{case}, step {step}: {discovery}");
            let applied = update_settings(
                &h,
                &json!({
                    "LinuxSubscriptionsDiscovery": discovery,
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": ["us-west-2"],
                        "OrganizationIntegration": DISCOVERY_OFF,
                    },
                    "AllowUpdate": true,
                }),
            )
            .await;
            assert_eq!(
                status(&applied, &case).as_deref(),
                Some(SETTLED),
                "case {case:?}: the call that applies the change reports the settled Status"
            );
            // Readable back, from any Region the settings are read from: the singleton is
            // the account's, not a per-Region value.
            for region in READ_REGIONS {
                let case = format!("{case} read in {region}");
                let fetched = call_in_region(
                    &h,
                    region,
                    "/subscription/GetServiceSettings",
                    &json!({}),
                )
                .await;
                assert_eq!(
                    status(&fetched, &case).as_deref(),
                    Some(SETTLED),
                    "case {case:?}: a later read reports the settled Status"
                );
                assert_eq!(
                    fetched
                        .body
                        .get("LinuxSubscriptionsDiscovery")
                        .and_then(Value::as_str),
                    Some(*discovery),
                    "case {case:?}: expected the persisted switch {discovery}, got {:?}",
                    fetched.body
                );
            }
        }
    }
}
