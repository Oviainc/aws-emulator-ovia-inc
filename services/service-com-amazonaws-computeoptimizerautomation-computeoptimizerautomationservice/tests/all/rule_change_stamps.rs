// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `lastUpdatedTimestamp` every write path of this
//! service leaves on an automation rule.
//!
//! The rule under test: whichever call changes a stored automation rule stamps
//! that rule's `lastUpdatedTimestamp` with the instant the store applied the
//! change — the member means "The timestamp when the automation rule was last
//! updated"
//! (`smithy_query(query="doc",shape="AutomationRule",member="lastUpdatedTimestamp")`)
//! — and a call that leaves the stored rule exactly as it stood moves it
//! nowhere. The stamp does not depend on WHICH call changed the rule: a caller's
//! update, a tag addition, a tag removal and the service-driven stop an opt-out
//! performs all record it, while `ruleRevision` advances for the caller's update
//! alone.
//!
//! The rows here differ along the WRITE PATH — which operation changes the rule
//! and which members it carries. How a tag write behaves across tag payloads
//! (key and value lengths, multi-key merges, a removal, idempotent replays) is
//! the subject of [`super::tag_write_timestamps`]; this suite is the same rule
//! over the other operations that reach a stored rule.
//!
//! Every case drives the service over the wire through the same router the
//! emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]) and reads the rule back through two
//! independent readers, so a timestamp that is not the one the store holds fails
//! here. The one exception is the opt-out case, which closes the enrollment gate
//! every reader of a rule sits behind, so the rule it changed is read from the
//! store the mounted service wrote it to.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use tokio::time::sleep;

use super::automation_rule_lifecycle::{
    call_ok, create_rule, emulator, emulator_with_engine, listed_rule, member, read_rule,
    string_member, timestamp,
};

/// A pause between the create and the write under test, so "the timestamp moved"
/// is a statement about the write rather than about the resolution of the clock
/// the two calls read.
const CLOCK_MARGIN: Duration = Duration::from_millis(25);

/// One row of the table: a rule created in `created_status` carrying
/// `created_tags`, one write addressed at that rule, and what the stored rule
/// must look like afterwards.
struct Case {
    name: &'static str,
    /// The `status` the rule is created with (`RuleStatus`: Active | Inactive,
    /// `smithy_query(query="shape",shape="RuleStatus")`).
    created_status: &'static str,
    /// The `tags` the rule is created with; an empty list creates it untagged.
    created_tags: Value,
    /// The operation this row writes with: `TagResource` (which names the rule
    /// `resourceArn`) or `UpdateAutomationRule` (which names it `ruleArn`).
    operation: &'static str,
    /// The members the write carries beside the rule's ARN and revision.
    members: Value,
    /// What `ruleRevision` must read afterwards, as an offset from the revision
    /// the create stored.
    revision_delta: i64,
    /// The `status` the stored rule must report afterwards.
    expected_status: &'static str,
}

/// Every row's write CHANGES the stored rule, so every row must leave a later
/// `lastUpdatedTimestamp` behind; the other half of the rule — a call that
/// leaves the rule exactly as it stood moves nothing — is asserted by
/// [`an_opt_out_stamps_only_the_rules_it_stops`] below and, for the tag
/// payloads, by [`super::tag_write_timestamps`]. Only ONE tagging row is kept
/// here, the addition this issue was filed about, so this table does not grow a
/// second copy of that one.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "tag-adds-a-key-the-rule-does-not-carry",
            created_status: "Active",
            created_tags: json!([]),
            operation: "TagResource",
            members: json!({"tags": [{"key": "environment", "value": "production"}]}),
            revision_delta: 0,
            expected_status: "Active",
        },
        Case {
            name: "update-replaces-the-description",
            created_status: "Active",
            created_tags: json!([]),
            operation: "UpdateAutomationRule",
            members: json!({"description": "rewritten by the update"}),
            revision_delta: 1,
            expected_status: "Active",
        },
        Case {
            name: "update-deactivates-the-rule",
            created_status: "Active",
            created_tags: json!([{"key": "environment", "value": "production"}]),
            operation: "UpdateAutomationRule",
            members: json!({"status": "Inactive"}),
            revision_delta: 1,
            expected_status: "Inactive",
        },
        Case {
            // An update carrying only the required members still advances the
            // revision, so the stored rule changed and the stamp moves with it.
            name: "update-carries-only-the-required-members",
            created_status: "Inactive",
            created_tags: json!([]),
            operation: "UpdateAutomationRule",
            members: json!({}),
            revision_delta: 1,
            expected_status: "Inactive",
        },
    ]
}

/// Whichever call a caller changes one rule with stamps its
/// `lastUpdatedTimestamp`; `createdTimestamp` never moves, `ruleRevision`
/// advances for the update alone, and both readers of a stored rule report the
/// same instant.
#[tokio::test]
async fn every_write_that_changes_a_rule_stamps_last_updated_timestamp() {
    let router = emulator();

    for (index, case) in cases().iter().enumerate() {
        let name = case.name;
        let (rule_arn, revision) = create_rule(
            &router,
            &format!("stamp-rule-{index}"),
            case.created_status,
            &case.created_tags,
            name,
        )
        .await;

        let before = read_rule(&router, &rule_arn, name).await;
        let created_at = timestamp(&before, "createdTimestamp", name);
        let before_write = timestamp(&before, "lastUpdatedTimestamp", name);
        assert_eq!(
            before_write, created_at,
            "case {name:?}: a rule never updated was last changed when it was created, got \
             {before_write} against {created_at}"
        );

        sleep(CLOCK_MARGIN).await;

        // `UpdateAutomationRule` names the rule `ruleArn`; the two tagging
        // operations name it `resourceArn`. Both carry the revision the rule
        // currently holds, which is the create's.
        let arn_member = if case.operation == "UpdateAutomationRule" {
            "ruleArn"
        } else {
            "resourceArn"
        };
        let mut request = json!({arn_member: &rule_arn, "ruleRevision": revision});
        for (member_name, value) in case
            .members
            .as_object()
            .expect("the row's members are an object")
        {
            request[member_name.as_str()] = value.clone();
        }
        call_ok(&router, case.operation, &request, name).await;

        let after = read_rule(&router, &rule_arn, name).await;
        let stamped = timestamp(&after, "lastUpdatedTimestamp", name);
        assert!(
            stamped > before_write,
            "case {name:?}: a write that changes the rule stamps the instant it was applied, \
             expected a lastUpdatedTimestamp later than {before_write}, got {stamped}"
        );

        assert_eq!(
            member(&after, "ruleRevision").as_i64(),
            Some(revision + case.revision_delta),
            "case {name:?}: expected ruleRevision {}, got {after}",
            revision + case.revision_delta
        );
        assert_eq!(
            string_member(&after, "status"),
            case.expected_status,
            "case {name:?}: expected the stored status {:?}, got {after}",
            case.expected_status
        );
        let created_after = timestamp(&after, "createdTimestamp", name);
        assert_eq!(
            created_after, created_at,
            "case {name:?}: no write moves createdTimestamp, expected {created_at}, got \
             {created_after}"
        );

        // The second reader of the same stored rule agrees, so the instant is
        // the store's and not one reader's invention.
        let summary = listed_rule(&router, &rule_arn, name).await;
        let listed_stamp = timestamp(&summary, "lastUpdatedTimestamp", name);
        assert_eq!(
            listed_stamp, stamped,
            "case {name:?}: the listed rule must carry the lastUpdatedTimestamp Get reports, \
             expected {stamped}, got {listed_stamp}"
        );
    }
}

/// The same rule for the write no caller addresses at a rule at all: opting the
/// account out "stops all of your automation rules"
/// (`smithy_query(query="doc",shape="UpdateEnrollmentConfigurationRequest",member="status")`),
/// so it changes every rule it stops and stamps each of those — and stamps no
/// rule it found stopped already. The revision is the caller's token, so a
/// service-driven stop leaves it where it was.
///
/// An account that is not opted in may not read its rules, so the two wire
/// readers are unreachable once this write lands; the rules are read from the
/// store the mounted service wrote them to instead.
#[tokio::test]
async fn an_opt_out_stamps_only_the_rules_it_stops() {
    let (router, engine) = emulator_with_engine();
    let case = "opt-out";

    // (the rule's name, the status it is created in, the tags it carries,
    // whether the opt-out changes it): two rules it must stop, one it finds
    // stopped already.
    let plan: Vec<(&str, &str, Value, bool)> = vec![
        ("stamp-opt-out-active", "Active", json!([]), true),
        (
            "stamp-opt-out-active-2",
            "Active",
            json!([{"key": "environment", "value": "production"}]),
            true,
        ),
        ("stamp-opt-out-inactive", "Inactive", json!([]), false),
    ];

    // Each rule as it stood before the opt-out: its ARN, the revision and the
    // two instants the create stored, and whether the opt-out changes it.
    let mut before = Vec::new();
    for (name, status, tags, changed) in &plan {
        let (arn, revision) = create_rule(&router, name, status, tags, case).await;
        let service_state =
            state::service_state(&*engine).expect("the engine bears this service's state");
        let guard = service_state.lock();
        let stored = guard
            .rules
            .get(&arn)
            .expect("the created rule is in the store");
        let stamps = (
            stored.last_updated_timestamp.as_secs_f64(),
            stored.created_timestamp.as_secs_f64(),
        );
        drop(guard);
        before.push((arn, revision, stamps, *changed));
    }

    sleep(CLOCK_MARGIN).await;

    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Inactive"}),
        case,
    )
    .await;

    let service_state =
        state::service_state(&*engine).expect("the engine bears this service's state");
    let guard = service_state.lock();
    for (arn, revision, (stamped_before, created_at), changed) in before {
        let rule = guard.rules.get(&arn).expect("the rule is still in the store");
        let stamped = rule.last_updated_timestamp.as_secs_f64();
        assert_eq!(
            rule.status, "Inactive",
            "case {case:?}: every rule of the account is stopped, got {:?} for {arn}",
            rule.status
        );
        assert_eq!(
            rule.rule_revision, revision,
            "case {case:?}: a service-driven stop is not a rule revision, expected {revision} \
             for {arn}, got {}",
            rule.rule_revision
        );
        if changed {
            assert!(
                stamped > stamped_before,
                "case {case:?}: stopping {arn} changed it, expected a lastUpdatedTimestamp \
                 later than {stamped_before}, got {stamped}"
            );
        } else {
            assert_eq!(
                stamped, stamped_before,
                "case {case:?}: {arn} was stopped already, so the opt-out changed nothing, \
                 expected lastUpdatedTimestamp to stay {stamped_before}, got {stamped}"
            );
        }
        assert_eq!(
            rule.created_timestamp.as_secs_f64(),
            created_at,
            "case {case:?}: no write moves createdTimestamp, expected {created_at} for {arn}, \
             got {}",
            rule.created_timestamp.as_secs_f64()
        );
    }
}
