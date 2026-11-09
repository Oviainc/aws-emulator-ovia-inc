// SPDX-License-Identifier: CC0-1.0

//! RULE: the 404 for a retention rule the request addressed and this Region does not hold
//! states the rule's own IDENTIFIER and nothing else of the request —
//! `Recycle Bin Rule '<identifier>' does not exist.` — whichever way the request named the
//! rule. Five operations name it by its `Identifier` member and three by a `ResourceArn`;
//! for those three the identifier is the one the ARN's `rule/<Identifier>` resource
//! segment names, so no partition, service, Region or account segment of the submitted ARN
//! reaches the message.
//!
//! The table spans what the rule generalises over: all EIGHT rule-addressed operations,
//! identifiers that differ in form (lowercase, uppercase, mixed, all digits) and in WHY
//! they name no rule (never minted, or created and then deleted), and — for the
//! ARN-addressed three — ARNs that differ in the partition, Region and account segment
//! they carry while naming the same identifier, which the message must read the same way
//! for.
//!
//! Every case drives the service through the real router, so the wire protocol, the route
//! table, the `ResourceArn` label binding and the handlers are exercised exactly as a
//! signed SDK request would exercise them.

use serde_json::{json, Value};

use smithy_server_runtime::MultiServiceRouter;

use crate::error_response_bodies::{created_rule, error_code, message};
use crate::resource_tags::{keys, list_tags, pairs, tag_resource, untag_resource};
use crate::retention_rule_lifecycle::{
    UNLOCK_DELAY_MIN, create_rule, delete_rule, get_rule, lock_configuration, retention, router,
    update_rule,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};

/// The five operations that address a rule by its `Identifier`, each answered for an
/// identifier no rule holds.
async fn identifier_addressed(
    router: &MultiServiceRouter,
    identifier: &str,
) -> Vec<(&'static str, (u16, Value))> {
    vec![
        ("GetRule", get_rule(router, identifier).await),
        (
            "UpdateRule",
            update_rule(router, identifier, json!({ "Description": "anything" })).await,
        ),
        ("DeleteRule", delete_rule(router, identifier).await),
        (
            "LockRule",
            lock_rule(
                router,
                identifier,
                json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
            )
            .await,
        ),
        ("UnlockRule", unlock_rule(router, identifier).await),
    ]
}

/// The three operations that address a rule by its `ResourceArn`, each answered for an
/// ARN naming a rule this Region does not hold.
async fn arn_addressed(
    router: &MultiServiceRouter,
    rule_arn: &str,
) -> Vec<(&'static str, (u16, Value))> {
    vec![
        ("ListTagsForResource", list_tags(router, rule_arn).await),
        (
            "TagResource",
            tag_resource(router, rule_arn, &pairs(&[("team", "platform")])).await,
        ),
        (
            "UntagResource",
            untag_resource(router, rule_arn, &keys(&["team"])).await,
        ),
    ]
}

/// The one assertion of the rule: the 404 names the identifier, and nothing of any other
/// value the request carried.
fn assert_names_the_identifier(case: &str, identifier: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {body:?}"
    );
    let expected = format!("Recycle Bin Rule '{identifier}' does not exist.");
    let stated = message(body);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected Message {expected:?}, got {stated:?}"
    );
}

/// The identifier is the only variable part, whichever member the request addressed the
/// rule by.
#[tokio::test]
async fn an_absent_rule_is_reported_by_its_identifier_alone() {
    let router = router();

    // A rule this Region holds, so no refusal below comes from an empty store, and the
    // template every absent rule's ARN is built from.
    let (held, held_arn) = created_rule(
        &router,
        "a rule this Region holds",
        json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    // One created and then deleted: the second way a rule can be absent.
    let (deleted, _) = created_rule(
        &router,
        "a rule that is then deleted",
        json!({ "ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(6) }),
    )
    .await;
    let (status, _) = delete_rule(&router, &deleted).await;
    assert_eq!(
        status, 204,
        "setup: DeleteRule expected HTTP 204, got {status}"
    );

    let absent = [
        ("never minted, all lowercase", "zzzzzzzzzzz".to_string()),
        ("never minted, all uppercase", "ZZZZZZZZZZZ".to_string()),
        ("never minted, mixed case", "0a1B2c3D4e5".to_string()),
        ("never minted, all digits", "99999999999".to_string()),
        ("created and then deleted", deleted.clone()),
    ];

    for (why, identifier) in &absent {
        for (operation, (status, body)) in identifier_addressed(&router, identifier).await {
            let case = format!("{operation} on an identifier {why} ({identifier})");
            assert_names_the_identifier(&case, identifier, status, &body);
        }
        // The same absent rule, addressed by ARN — and by ARNs differing in every
        // segment that is not the rule id, each of which the message must leave out.
        let own = held_arn.replace(&held, identifier);
        for (segment, arn) in arn_variants(&own) {
            for (operation, (status, body)) in arn_addressed(&router, &arn).await {
                let case =
                    format!("{operation} on an ARN {segment} naming an identifier {why} ({arn})");
                assert_names_the_identifier(&case, identifier, status, &body);
            }
        }
    }

    // The rule that DOES exist still reads back, so none of the refusals touched it.
    let (status, fetched) = get_rule(&router, &held).await;
    assert_eq!(
        status, 200,
        "case \"the held rule after every refusal\": expected HTTP 200, got {status} / {fetched:?}"
    );
}

/// `rule_arn` plus the same ARN with one non-identifier segment replaced: the partition,
/// the Region and the account. Each is a value the modelled `RuleArn` pattern admits
/// (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$`, from
/// smithy_query(query="shape", shape="RuleArn")), so each reaches the handler — and none
/// of them may reach the message.
fn arn_variants(rule_arn: &str) -> Vec<(&'static str, String)> {
    vec![
        ("as this service minted it", rule_arn.to_string()),
        (
            "of another partition",
            with_segment(rule_arn, 1, "aws-us-gov"),
        ),
        ("of another Region", with_segment(rule_arn, 3, "us-east-2")),
        ("of another account", with_segment(rule_arn, 4, "111122223333")),
    ]
}

/// `rule_arn` with its `index`th `:`-separated field replaced.
fn with_segment(rule_arn: &str, index: usize, replacement: &str) -> String {
    let mut segments: Vec<&str> = rule_arn.split(':').collect();
    assert!(
        index < segments.len(),
        "a rule ARN has six colon-separated fields, got {rule_arn:?}"
    );
    segments[index] = replacement;
    segments.join(":")
}

/// The rule's own bound: an identifier-addressed operation and an ARN-addressed one that
/// name the SAME existing rule both succeed, so the 404 above reports absence rather than
/// a failure to resolve one of the two forms.
#[tokio::test]
async fn the_two_ways_of_addressing_one_rule_both_resolve_it() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        json!({
            "ResourceType": "EC2_IMAGE",
            "RetentionPeriod": retention(30),
            "Tags": [{ "Key": "env", "Value": "dev" }],
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();
    let rule_arn = created["RuleArn"].as_str().unwrap_or_default().to_string();

    let (status, fetched) = get_rule(&router, &identifier).await;
    assert_eq!(
        status, 200,
        "case \"GetRule on the identifier the create reported\": expected HTTP 200, got {status} / \
         {fetched:?}"
    );
    let (status, listed) = list_tags(&router, &rule_arn).await;
    assert_eq!(
        status, 200,
        "case \"ListTagsForResource on the ARN the create reported\": expected HTTP 200, got \
         {status} / {listed:?}"
    );
    assert_eq!(
        listed["Tags"][0]["Key"].as_str(),
        Some("env"),
        "case \"ListTagsForResource on the ARN the create reported\": expected the stored tag, \
         got {:?}",
        listed["Tags"]
    );
}
