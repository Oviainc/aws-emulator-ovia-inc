// SPDX-License-Identifier: CC0-1.0

//! What `DeleteTarget` does to the account's target registry and to the rules subscribed
//! to an address, over the wire.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * `ForceUnsubscribeAll` authorises dropping the associations, so a forced delete
//!   removes the address from EVERY rule of the region that carries it, leaves every
//!   other address of those rules standing, and drops the target from `ListTargets`;
//! * without that authorisation an address a rule still subscribes to is not deleted:
//!   the call is refused with the modelled `ValidationException` and nothing changes;
//! * an address no rule subscribes to is a success that deletes nothing, whatever
//!   `ForceUnsubscribeAll` says, so the delete is idempotent;
//! * a delete is served by a regional endpoint, so it touches only the rules of its own
//!   region.
//!
//! The harness (`Emulator`, `error_code`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{
    error_code, message, string, target_addresses_of, Emulator,
};

/// The target types these suites subscribe, which are `TargetSummary$TargetType`'s own
/// documented values ("Amazon Q Developer in chat applications topics are specified as
/// `SNS`", clients for Slack as `AWSChatbotSlack`, for Microsoft Teams as
/// `AWSChatbotMicrosoftTeams`).
const TARGET_TYPES: [&str; 3] = ["SNS", "AWSChatbotSlack", "AWSChatbotMicrosoftTeams"];

/// An address of the form the target type uses, carrying the region so no two regions of
/// one suite share an address by accident.
fn address_for(region: &str, label: &str, target_type: &str) -> String {
    match target_type {
        "SNS" => format!("arn:aws:sns:{region}:123456789012:{label}"),
        other => format!(
            "arn:aws:chatbot::123456789012:chat-configuration/{}/{label}-{region}",
            other.to_ascii_lowercase()
        ),
    }
}

/// Create a rule in `region` carrying `targets`, and return its ARN.
async fn create_rule(
    emulator: &Emulator,
    case: &str,
    region: &str,
    name: &str,
    targets: &[(&str, String)],
) -> String {
    let body = json!({
        "Name": name,
        "Resource": format!("arn:aws:codecommit:{region}:123456789012:{name}"),
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-pull-request-created"],
        "Targets": targets
            .iter()
            .map(|(target_type, address)| json!({
                "TargetType": target_type,
                "TargetAddress": address,
            }))
            .collect::<Vec<Value>>(),
    });
    let (status, created) = emulator
        .post_in(region, "/createNotificationRule", &body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding rule {name:?} must succeed, got {status} with {created}"
    );
    let arn = string(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "case {case:?}: the seeded rule must report an ARN, got {created}"
    );
    arn
}

/// The addresses `DescribeNotificationRule` reports for one rule, in the order served.
async fn rule_addresses(emulator: &Emulator, case: &str, region: &str, arn: &str) -> Vec<String> {
    let (status, described) = emulator
        .post_in(region, "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from /describeNotificationRule, got {status} with {described}"
    );
    target_addresses_of(&described)
}

/// The addresses `ListTargets` reports in `region`, in the order served.
async fn listed_addresses(emulator: &Emulator, case: &str, region: &str) -> Vec<String> {
    let (status, listed) = emulator.post_in(region, "/listTargets", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from /listTargets, got {status} with {listed}"
    );
    target_addresses_of(&listed)
}

/// RULE: a delete that carries the authorisation to drop the associations removes the
/// address from every rule of the region that subscribes to it, leaves every other
/// address those rules carry standing, and drops the target from the region's registry.
///
/// The rows differ in how many rules share the address and in the type it was subscribed
/// with — the dimensions the rule generalises over.
#[tokio::test]
async fn a_forced_delete_removes_the_address_from_every_rule_that_carries_it() {
    for (case, sharing_rules, target_type) in [
        ("one rule, an SNS topic", 1usize, TARGET_TYPES[0]),
        ("two rules, a Slack client", 2, TARGET_TYPES[1]),
        ("four rules, a Microsoft Teams client", 4, TARGET_TYPES[2]),
    ] {
        let emulator = Emulator::new();
        let region = "us-east-1";
        let shared = address_for(region, "shared", target_type);

        // Every rule carries the shared address AND one address of its own, so the
        // assertion proves only the address named is removed.
        let mut rules: Vec<(String, String)> = Vec::new();
        for index in 0..sharing_rules {
            let private = address_for(region, &format!("private-{index}"), TARGET_TYPES[0]);
            let arn = create_rule(
                &emulator,
                case,
                region,
                &format!("sharer-{index}"),
                &[
                    (target_type, shared.clone()),
                    (TARGET_TYPES[0], private.clone()),
                ],
            )
            .await;
            rules.push((arn, private));
        }

        let (status, deleted) = emulator
            .post_in(
                region,
                "/deleteTarget",
                &json!({ "TargetAddress": shared, "ForceUnsubscribeAll": true }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a forced delete must succeed, got {status} with {deleted}"
        );
        assert_eq!(
            deleted.as_object().map_or(0, serde_json::Map::len),
            0,
            "case {case:?}: DeleteTarget models no output members, got {deleted}"
        );

        for (arn, private) in &rules {
            let remaining = rule_addresses(&emulator, case, region, arn).await;
            assert_eq!(
                remaining,
                vec![private.clone()],
                "case {case:?}: rule {arn} must keep only its own address, got {remaining:?}"
            );
        }

        let listed = listed_addresses(&emulator, case, region).await;
        let expected: Vec<String> = rules.iter().map(|(_, private)| private.clone()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the deleted target must leave the registry, got {listed:?}"
        );

        // And deleting it again is a success that changes nothing: the operation models
        // no not-found error.
        let (status, again) = emulator
            .post_in(
                region,
                "/deleteTarget",
                &json!({ "TargetAddress": shared, "ForceUnsubscribeAll": true }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the delete must succeed, got {status} with {again}"
        );
        let listed_again = listed_addresses(&emulator, case, region).await;
        assert_eq!(
            listed_again, expected,
            "case {case:?}: repeating the delete must change nothing, got {listed_again:?}"
        );
    }
}

/// RULE: without the authorisation to drop the associations, an address a rule still
/// subscribes to is not deleted — the call is refused with the one client error this
/// operation models, and every rule keeps its target list.
///
/// The rows differ in how many rules hold the address, and in whether the request leaves
/// `ForceUnsubscribeAll` to its documented default (FALSE) or states it.
#[tokio::test]
async fn an_address_in_use_is_not_deleted_without_the_force_member() {
    for (case, sharing_rules, force_member) in [
        ("one rule, the member omitted", 1usize, None),
        ("one rule, the member stated false", 1, Some(false)),
        ("three rules, the member omitted", 3, None),
        ("three rules, the member stated false", 3, Some(false)),
    ] {
        let emulator = Emulator::new();
        let region = "us-east-1";
        let shared = address_for(region, "in-use", TARGET_TYPES[0]);

        let mut rules: Vec<String> = Vec::new();
        for index in 0..sharing_rules {
            rules.push(
                create_rule(
                    &emulator,
                    case,
                    region,
                    &format!("holder-{index}"),
                    &[(TARGET_TYPES[0], shared.clone())],
                )
                .await,
            );
        }

        let mut request = json!({ "TargetAddress": shared });
        if let Some(force) = force_member {
            request["ForceUnsubscribeAll"] = json!(force);
        }
        let (status, refused) = emulator.post_in(region, "/deleteTarget", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an address still in use must be refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: got {refused}"
        );
        let message = message(&refused);
        assert!(
            message.contains("ForceUnsubscribeAll"),
            "case {case:?}: the refusal must name the member that authorises the \
             removal, got {refused}"
        );
        assert!(
            !message.contains(&shared),
            "case {case:?}: the refusal must not repeat the sensitive address, got {refused}"
        );

        // Nothing moved: every rule still carries the address, and so does the registry.
        for arn in &rules {
            let remaining = rule_addresses(&emulator, case, region, arn).await;
            assert_eq!(
                remaining,
                vec![shared.clone()],
                "case {case:?}: a refused delete must leave rule {arn} untouched, got {remaining:?}"
            );
        }
        let listed = listed_addresses(&emulator, case, region).await;
        assert_eq!(
            listed,
            vec![shared.clone()],
            "case {case:?}: a refused delete must leave the registry untouched, got {listed:?}"
        );
    }
}

/// RULE: an address no rule of the region subscribes to is a success that deletes
/// nothing, whatever `ForceUnsubscribeAll` says.
///
/// The rows differ in why the address is unsubscribed (never seen, released by an
/// `Unsubscribe`, held only by another region) and in the value of the force member.
#[tokio::test]
async fn deleting_an_unsubscribed_address_succeeds_and_deletes_nothing() {
    for force in [None, Some(false), Some(true)] {
        let emulator = Emulator::new();
        let region = "us-east-1";
        let kept = address_for(region, "kept", TARGET_TYPES[0]);
        let released = address_for(region, "released", TARGET_TYPES[1]);
        let elsewhere = address_for("eu-west-1", "other-region", TARGET_TYPES[0]);

        let case = format!("ForceUnsubscribeAll={force:?}");
        let arn = create_rule(
            &emulator,
            &case,
            region,
            "holder",
            &[
                (TARGET_TYPES[0], kept.clone()),
                (TARGET_TYPES[1], released.clone()),
            ],
        )
        .await;
        create_rule(
            &emulator,
            &case,
            "eu-west-1",
            "elsewhere",
            &[(TARGET_TYPES[0], elsewhere.clone())],
        )
        .await;
        let (status, unsubscribed) = emulator
            .post_in(
                region,
                "/unsubscribe",
                &json!({ "Arn": arn, "TargetAddress": released }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: releasing an address must succeed, got {status} with {unsubscribed}"
        );

        let never_seen = address_for(region, "never-subscribed", TARGET_TYPES[2]);
        for address in [&never_seen, &released, &elsewhere] {
            let mut request = json!({ "TargetAddress": address });
            if let Some(value) = force {
                request["ForceUnsubscribeAll"] = json!(value);
            }
            let (status, deleted) = emulator.post_in(region, "/deleteTarget", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting an unsubscribed address must succeed, got \
                 {status} with {deleted}"
            );
        }

        let remaining = rule_addresses(&emulator, &case, region, &arn).await;
        assert_eq!(
            remaining,
            vec![kept.clone()],
            "case {case:?}: the rule must keep the address nothing deleted, got {remaining:?}"
        );
        let listed = listed_addresses(&emulator, &case, region).await;
        assert_eq!(
            listed,
            vec![kept.clone()],
            "case {case:?}: the registry must keep that target, got {listed:?}"
        );
        let elsewhere_listed = listed_addresses(&emulator, &case, "eu-west-1").await;
        assert_eq!(
            elsewhere_listed,
            vec![elsewhere.clone()],
            "case {case:?}: another region's target must be untouched, got {elsewhere_listed:?}"
        );
    }
}

/// RULE: a delete is served by a regional endpoint, so it drops only the subscriptions of
/// its own region's rules — a rule of another region subscribed to the SAME address keeps
/// its target, and so does that region's registry.
#[tokio::test]
async fn a_delete_touches_only_the_rules_of_its_own_region() {
    let emulator = Emulator::new();
    // One address subscribed in three regions, so the rows differ in which region the
    // delete is signed for and which ones must be left alone.
    let shared = "arn:aws:sns:us-east-1:123456789012:cross-region-topic".to_string();
    let regions = ["us-east-1", "eu-west-1", "ap-south-1"];
    let mut rules: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let arn = create_rule(
            &emulator,
            "cross region",
            region,
            "cross-region-holder",
            &[(TARGET_TYPES[0], shared.clone())],
        )
        .await;
        rules.push((region, arn));
    }

    let mut deleted_in: Vec<&str> = Vec::new();
    for (region, _) in rules.clone() {
        let case = format!("delete in {region}");
        let (status, deleted) = emulator
            .post_in(
                region,
                "/deleteTarget",
                &json!({ "TargetAddress": shared, "ForceUnsubscribeAll": true }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a forced delete must succeed, got {status} with {deleted}"
        );
        deleted_in.push(region);

        for (other, arn) in &rules {
            let expected: Vec<String> = if deleted_in.contains(other) {
                Vec::new()
            } else {
                vec![shared.clone()]
            };
            let remaining = rule_addresses(&emulator, &case, other, arn).await;
            assert_eq!(
                remaining, expected,
                "case {case:?}: region {other} must report {expected:?}, got {remaining:?}"
            );
            let listed = listed_addresses(&emulator, &case, other).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: region {other}'s registry must report {expected:?}, got {listed:?}"
            );
        }
    }
}
