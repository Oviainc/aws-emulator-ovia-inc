// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall rule-group slice:
//! `CreateFirewallRuleGroup` and `GetFirewallRuleGroup`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, ACCOUNT, REGION,
};

/// The `FirewallRuleGroup` of a create/get response.
fn rule_group(body: &Value) -> &Value {
    body.get("FirewallRuleGroup")
        .expect("the response carries a FirewallRuleGroup")
}

/// The sentence DNS Firewall states in `FirewallRuleGroup$StatusMessage` for a
/// group that reached its created, settled lifecycle step: fixed text for the
/// step, with no part of the request in it.
const CREATED_RULE_GROUP_MESSAGE: &str = "Created Firewall Rule Group";

/// One row of the create table: what the call hands over.
struct CreateCase {
    case: &'static str,
    body: Value,
}

/// A created rule group is persisted: every member of the response is computed
/// from the request plus what the model documents, and `GetFirewallRuleGroup`
/// answers the very same rule group.
///
/// The table spans what the rule generalises over: the shape of `Name` (every
/// character class the model's `Name` pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits — letters, digits beside
/// letters, hyphen, underscore, apostrophe, space — and its published maximum
/// length of 64 as well as the minimum of one character), and `Tags` absent,
/// empty and carrying several entries.
#[tokio::test]
async fn a_created_rule_group_is_read_back_from_state() {
    // 64 characters, the maximum the model's `Name` (length 0..64) allows.
    let longest_name = "g".repeat(64);
    let cases = vec![
        CreateCase {
            case: "letters only, no Tags",
            body: json!({
                "CreatorRequestId": "req-letters",
                "Name": "blocklist-group",
            }),
        },
        CreateCase {
            case: "digits beside letters, underscores and hyphens",
            body: json!({
                "CreatorRequestId": "req-mixed",
                "Name": "group_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        },
        CreateCase {
            case: "spaces in the name, several tags",
            body: json!({
                "CreatorRequestId": "req-spaces",
                "Name": "my rule group",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                    // The shortest value the service's tag acceptance criteria
                    // admit: a tag it stores names a value.
                    { "Key": "note", "Value": "-" },
                ],
            }),
        },
        CreateCase {
            case: "one character, an empty tag list",
            body: json!({
                "CreatorRequestId": "req-shortest",
                "Name": "g",
                "Tags": [],
            }),
        },
        CreateCase {
            case: "the longest Name the model allows",
            body: json!({
                "CreatorRequestId": "req-longest",
                "Name": longest_name,
            }),
        },
        CreateCase {
            case: "an apostrophe, which the pattern admits",
            body: json!({
                "CreatorRequestId": "req-apostrophe",
                "Name": "ops' group",
            }),
        },
        CreateCase {
            case: "a CreatorRequestId that reads like a date/time stamp",
            body: json!({
                "CreatorRequestId": "2026-10-24T06:21:31Z",
                "Name": "stamped",
            }),
        },
    ];

    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for CreateCase { case, body } in cases {
        let created = ok(&router, "CreateFirewallRuleGroup", body.clone()).await;
        let made = rule_group(&created).clone();
        let id = string_member(&made, "Id");

        assert!(
            id.starts_with("rslvr-frg-"),
            "case {case:?}: a firewall rule group id is minted as rslvr-frg-<hex>, got {id:?}"
        );
        assert!(
            !id.is_empty() && id.len() <= 64,
            "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
        );
        assert_eq!(
            string_member(&made, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-rule-group/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {made}"
        );
        for member in ["Name", "CreatorRequestId"] {
            assert_eq!(
                made.get(member),
                body.get(member),
                "case {case:?}: {member} must come back exactly as requested, got {made}"
            );
        }
        assert_eq!(
            made.get("RuleCount"),
            Some(&json!(0)),
            "case {case:?}: a rule group is created empty, so RuleCount is 0, got {made}"
        );
        assert_eq!(
            string_member(&made, "Status"),
            "COMPLETE",
            "case {case:?}: an empty rule group settles at once, got {made}"
        );
        assert_eq!(
            string_member(&made, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId is the account that created the group, got {made}"
        );
        assert_eq!(
            string_member(&made, "ShareStatus"),
            "NOT_SHARED",
            "case {case:?}: a group this account just created is shared with nobody, got {made}"
        );
        // `StatusMessage` carries "additional information about the status of
        // the rule group, if available": the step the group reached is stated
        // there, the same way for every group that stands in it.
        assert_eq!(
            string_member(&made, "StatusMessage"),
            CREATED_RULE_GROUP_MESSAGE,
            "case {case:?}: a settled, created group states the step it reached, got {made}"
        );
        let created_at = parse_time(case, "CreationTime", &string_member(&made, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&made, "ModificationTime"),
        );
        assert_eq!(
            created_at, modified_at,
            "case {case:?}: a group nobody has modified carries one timestamp, got {made}"
        );

        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: each create mints a distinct Id, {id:?} is already among {seen_ids:?}"
        );
        seen_ids.push(id.clone());

        // Round trip: the reader answers the record the create wrote, and the
        // status of an empty group is terminal, so the two responses are equal.
        let fetched = ok(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await;
        assert_eq!(
            rule_group(&fetched),
            &made,
            "case {case:?}: GetFirewallRuleGroup must answer the stored group, got {fetched}"
        );
    }

    // Every group created above is still readable after the later creates, so a
    // create never disturbs the records beside it.
    for id in &seen_ids {
        let fetched = ok(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await;
        assert_eq!(
            string_member(rule_group(&fetched), "Id"),
            *id,
            "case \"all groups survive\": {id} must still be readable, got {fetched}"
        );
    }
}

/// The `Tags` a create request carried are the tags the stored group holds:
/// `FirewallRuleGroup` models no `Tags` member, so they are reported by
/// `ListTagsForResource` against the ARN the create minted.
#[tokio::test]
async fn the_tags_a_create_carried_are_the_tags_the_group_holds() {
    let router = emulator();

    for (case, requested) in [
        ("no Tags at all", json!(null)),
        ("an empty Tags list", json!([])),
        (
            "one tag",
            json!([{ "Key": "team", "Value": "net" }]),
        ),
        (
            "several tags, one with a one-character value",
            json!([
                { "Key": "team", "Value": "net" },
                { "Key": "env", "Value": "prod" },
                { "Key": "note", "Value": "-" },
            ]),
        ),
    ] {
        let mut body = json!({
            "CreatorRequestId": format!("req-tags-{case}"),
            "Name": "tagged-group",
        });
        if !requested.is_null() {
            body.as_object_mut()
                .expect("the body is a JSON object")
                .insert("Tags".to_string(), requested.clone());
        }

        let created = ok(&router, "CreateFirewallRuleGroup", body).await;
        let arn = string_member(rule_group(&created), "Arn");

        let listed = ok(&router, "ListTagsForResource", json!({ "ResourceArn": arn })).await;
        let reported = listed
            .get("Tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let expected = requested.as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported,
            expected,
            "case {case:?}: the group must report exactly the tags the create carried, got {listed}"
        );
    }
}

/// A retried create with the same `CreatorRequestId` resolves to the rule group
/// that request already made: `CreatorRequestId` "allows you to retry failed
/// requests without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_group() {
    let router = emulator();

    for (case, body) in [
        (
            "retried with the same Name",
            json!({ "CreatorRequestId": "req-retried", "Name": "once-only" }),
        ),
        (
            "retried with tags",
            json!({
                "CreatorRequestId": "req-retried-tagged",
                "Name": "once-only-tagged",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ] {
        let first = ok(&router, "CreateFirewallRuleGroup", body.clone()).await;
        let second = ok(&router, "CreateFirewallRuleGroup", body).await;
        assert_eq!(
            rule_group(&first),
            rule_group(&second),
            "case {case:?}: the retry must resolve to the group the first call made, got {second}"
        );
    }
}

/// A `CreatorRequestId` identifies a request to ONE operation: the same string
/// handed to another create names a different resource, and neither call
/// resolves to the other's record.
#[tokio::test]
async fn a_creator_request_id_is_scoped_to_its_operation() {
    let router = emulator();
    let shared = "req-shared-across-operations";

    let group = ok(
        &router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": shared, "Name": "group-side" }),
    )
    .await;
    let list = ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": shared, "Name": "list-side" }),
    )
    .await;

    let group_id = string_member(rule_group(&group), "Id");
    let list_id = string_member(
        list.get("FirewallDomainList")
            .expect("the response carries a FirewallDomainList"),
        "Id",
    );
    assert_ne!(
        group_id, list_id,
        "case \"one id, two operations\": each create must mint its own resource, got {group_id} and {list_id}"
    );

    let fetched = ok(
        &router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    assert_eq!(
        string_member(rule_group(&fetched), "Name"),
        "group-side",
        "case \"one id, two operations\": the rule group must keep its own Name, got {fetched}"
    );
}

/// `GetFirewallRuleGroup` reports a `FirewallRuleGroupId` the store holds no
/// group under with the `ResourceNotFoundException` it models — never an empty
/// success, and never a group it invented.
#[tokio::test]
async fn an_unknown_rule_group_id_is_reported_as_missing() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": "req-present", "Name": "present" }),
    )
    .await;
    let present = string_member(rule_group(&created), "Id");

    for (case, id) in [
        (
            "an id nothing minted",
            "rslvr-frg-0123456789abcdef".to_string(),
        ),
        (
            "a firewall domain list id",
            "rslvr-fdl-0123456789abcdef".to_string(),
        ),
        ("one character", "x".to_string()),
        // The stored id with its last character dropped: a near miss is still a
        // miss.
        (
            "a near miss of a stored id",
            present[..present.len() - 1].to_string(),
        ),
    ] {
        expect_error(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id.clone() }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // The refusal names the resource the caller asked for, so a caller can
        // tell which id was not found.
        let (_, body) = call(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id.clone() }),
        )
        .await;
        let message = string_member(&body, "Message") + &string_member(&body, "message");
        assert!(
            message.contains(&id),
            "case {case:?}: the refusal must name the id that was not found, got {body}"
        );
        assert_eq!(
            string_member(&body, "ResourceType"),
            "FirewallRuleGroup",
            "case {case:?}: the refusal must name the resource type, got {body}"
        );
    }

    // The refusals above are about the id, not about refusing every read.
    let fetched = ok(
        &router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": present }),
    )
    .await;
    assert_eq!(
        string_member(rule_group(&fetched), "Id"),
        present,
        "case \"the stored id\": the read must answer the stored group, got {fetched}"
    );
}

/// A `Name` outside the model's constraints is refused with HTTP 400, and
/// nothing is stored for it.
///
/// The rows walk the published bounds of `Name` (length 0..64, pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`): one character past the maximum, the
/// empty string, and the all-digits form the pattern's negative lookahead
/// excludes. Every refusal is worded as a constraint violation of that member —
/// the violation count, the rejected value, the member path and the requirement
/// — whichever of the two constraints the value missed.
#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused() {
    let router = emulator();

    // The `Name` pattern, from the model
    // (`smithy_query(query="shape",shape="Name")`), as a refusal quotes it: the
    // expression without the whole-value `^`/`$` pair the model wraps it in,
    // which is the form the recorded refusal of a digits-only name carries.
    let pattern = r"(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)";

    for (case, name, violates_pattern) in [
        (
            "one character past the 64-character maximum",
            "a".repeat(65),
            false,
        ),
        ("far past the maximum", "b".repeat(200), false),
        ("empty, which the pattern's + excludes", String::new(), true),
        ("only digits", "12345".to_string(), true),
        ("one digit", "7".to_string(), true),
        ("a character outside the pattern", "bad/name".to_string(), true),
        (
            "a dot, which the pattern excludes",
            "group.one".to_string(),
            true,
        ),
        ("a non-ASCII letter", "groupe-é".to_string(), true),
    ] {
        let (status, body) = call(
            &router,
            "CreateFirewallRuleGroup",
            json!({ "CreatorRequestId": format!("req-{case}"), "Name": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a Name outside the modelled constraints must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );

        let message = string_member(&body, "Message") + &string_member(&body, "message");
        if violates_pattern {
            assert_eq!(
                message,
                format!(
                    "1 validation error detected: Value '{name}' at 'name' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: {pattern}"
                ),
                "case {case:?}: the refusal must state the pattern the value had to satisfy, got {body}"
            );
        } else {
            assert!(
                message.starts_with("1 validation error detected: ")
                    && message.contains("at 'name' failed to satisfy constraint: ")
                    && message.contains("length less than or equal to 64"),
                "case {case:?}: the refusal must state the length bound the value missed, got {body}"
            );
        }
    }

    // The longest legal name is accepted, so the refusals above are about the
    // constraint and not about refusing long names outright.
    let created = ok(
        &router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": "req-at-the-bound", "Name": "c".repeat(64) }),
    )
    .await;
    assert_eq!(
        string_member(rule_group(&created), "Name").len(),
        64,
        "case \"at the bound\": a 64-character Name is legal, got {created}"
    );
}

/// Many creates in one store each mint their own group and each read back: the
/// emulator's own ceiling on how many groups it holds is far above the handful a
/// caller makes, so an ordinary workload is served throughout.
#[tokio::test]
async fn many_creates_in_one_store_are_each_served_and_distinct() {
    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..25 {
        let created = ok(
            &router,
            "CreateFirewallRuleGroup",
            json!({
                "CreatorRequestId": format!("req-bulk-{index}"),
                "Name": format!("bulk-{index}"),
            }),
        )
        .await;
        assert_eq!(
            string_member(rule_group(&created), "Status"),
            "COMPLETE",
            "case \"bulk create {index}\": every create below the cap is served, got {created}"
        );
        let id = string_member(rule_group(&created), "Id");
        assert!(
            !ids.contains(&id),
            "case \"bulk create {index}\": each create mints a distinct Id, got {id:?} among {ids:?}"
        );
        ids.push(id);
    }

    for (index, id) in ids.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await;
        assert_eq!(
            string_member(rule_group(&fetched), "Name"),
            format!("bulk-{index}"),
            "case \"bulk read {index}\": the read must answer the group that create wrote, got {fetched}"
        );
    }
}
