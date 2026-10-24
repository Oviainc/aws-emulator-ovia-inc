// SPDX-License-Identifier: CC0-1.0

//! IP access control groups, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see: the request is serialized,
//! routed, validated, handled and serialized back.
//!
//! The rules under test, all of them about groups this emulator persisted rather than
//! about any one recorded request:
//!
//! * `CreateIpGroup` mints a `GroupId` matching the model's `^wsipg-[0-9a-z]{8,63}$`,
//!   persists every member the request supplied, and reports it back through
//!   `DescribeIpGroups`.
//! * A group name identifies at most one stored group: whichever create reaches the
//!   registry first is accepted, and every later or concurrent create on that name is
//!   refused with `ResourceAlreadyExistsException` (HTTP 400), leaving exactly one
//!   persisted group under the name.
//! * A rule's `ipRule` is an IP address range in CIDR notation; anything else is refused
//!   with `InvalidParameterValuesException`, as are more tags than a WorkSpaces resource
//!   can carry.
//! * `DescribeIpGroups` reports stored groups, filtered by `GroupIds`, paginated over a
//!   stable creation order, and changes nothing a later caller sees.
//! * `UpdateRulesOfIpGroup` REPLACES the stored rule set of the group it names with the
//!   submitted one — whatever that set is, including the empty one, which leaves the
//!   group with no rules and so no `userRules` member at all — and changes nothing else
//!   about that group and nothing about any other.
//! * `AuthorizeIpRules` ADDS the submitted rules to the ones the group holds, newest
//!   first, holding at most one rule per address range; `RevokeIpRules` REMOVES exactly
//!   the ranges it names and keeps every other rule in place.
//! * `DeleteIpGroup` removes the group it names, so a later `DescribeIpGroups` does not
//!   list it and a second delete of it is a `ResourceNotFoundException`.
//! * A mutation that names a group this emulator does not hold, or carries a rule list it
//!   refuses, leaves every stored group exactly as it was.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    service_state, MAX_IP_GROUP_TEXT_CHARS, MAX_RULES_PER_IP_GROUP, MAX_STORED_IP_GROUPS,
    MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::account_links::error_code;
use super::workspace_lifecycle::{call, emulator_with_engine};

/// Call `CreateIpGroup` with `request` and return `(status, body)`.
async fn create(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "CreateIpGroup", request).await
}

/// Call `UpdateRulesOfIpGroup` with `request` and return `(status, body)`.
async fn update_rules(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "UpdateRulesOfIpGroup", request).await
}

/// Call `AuthorizeIpRules` with `request` and return `(status, body)`.
async fn authorize_rules(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "AuthorizeIpRules", request).await
}

/// Call `RevokeIpRules` with `request` and return `(status, body)`.
async fn revoke_rules(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "RevokeIpRules", request).await
}

/// Assert `body` is the empty structure an operation with no output members answers with.
fn assert_no_output_members(case: &str, body: &Value) {
    assert_eq!(
        body.as_object().map(serde_json::Map::len),
        Some(0),
        "case {case:?}: the modelled output carries no members, got {body}"
    );
}

/// Call `DeleteIpGroup` with `request` and return `(status, body)`.
async fn delete_group(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "DeleteIpGroup", request).await
}

/// What `DescribeIpGroups` must report for a group whose stored rule set is `rules`.
///
/// `WorkspacesIpGroup$userRules` is optional, and a group holding NO rules reports no
/// `userRules` member rather than an empty list — which is also what `serde_json`
/// yields for an absent key, so the two sides of the assertion compare directly.
fn expected_user_rules(rules: &Value) -> Value {
    match rules.as_array() {
        Some(rules) if !rules.is_empty() => Value::Array(rules.clone()),
        _ => Value::Null,
    }
}

/// `count` distinct `ipRule` entries, one per row, with no `ruleDesc`.
fn ip_rules(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "ipRule": format!("10.{}.{}.0/24", index / 256, index % 256) }))
            .collect(),
    )
}

/// Create one group, asserting the call was accepted, and return its `GroupId`.
async fn create_one(router: &MultiServiceRouter, request: &Value) -> String {
    let (status, body) = create(router, request).await;
    assert_eq!(
        status, 200,
        "case {request}: CreateIpGroup must answer 200, got {status} with {body}"
    );
    let group_id = body["GroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request}: a minted GroupId, got {body}"));
    assert!(
        is_modelled_ip_group_id(group_id),
        "case {request}: GroupId must match ^wsipg-[0-9a-z]{{8,63}}$, got {group_id:?}"
    );
    group_id.to_string()
}

/// Whether `candidate` satisfies the model's `IpGroupId` pattern
/// `^wsipg-[0-9a-z]{8,63}$` (from `smithy_query(query="shape", shape="IpGroupId")`).
fn is_modelled_ip_group_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("wsipg-") else {
        return false;
    };
    (8..=63).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

/// Page `DescribeIpGroups` to exhaustion and return every group reported, in order.
///
/// `page_size` is the `MaxResults` each call carries, or `None` to let the service
/// choose the page size.
async fn sweep(router: &MultiServiceRouter, page_size: Option<usize>) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "DescribeIpGroups", &request).await;
        assert_eq!(
            status, 200,
            "case {request}: DescribeIpGroups must answer 200, got {status} with {body}"
        );
        let page = body["Result"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {request}: Result must be a list, got {body}"));
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {request}: a page may not exceed MaxResults, got {} entries: {page:?}",
                page.len()
            );
        }
        reported.extend(page);
        pages += 1;
        assert!(
            pages <= 512,
            "case {request}: pagination must terminate, walked {} entries",
            reported.len()
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    reported
}

/// The one group `group_id` names, read back through `DescribeIpGroups`.
async fn describe_one(router: &MultiServiceRouter, group_id: &str) -> Value {
    let request = json!({ "GroupIds": [group_id] });
    let (status, body) = call(router, "DescribeIpGroups", &request).await;
    assert_eq!(
        status, 200,
        "case {group_id}: DescribeIpGroups must answer 200, got {status} with {body}"
    );
    let reported = body["Result"]
        .as_array()
        .unwrap_or_else(|| panic!("case {group_id}: Result must be a list, got {body}"));
    assert_eq!(
        reported.len(),
        1,
        "case {group_id}: exactly the group asked for is reported, got {reported:?}"
    );
    reported[0].clone()
}

/// `MAX_TAGS_PER_RESOURCE` distinct tags: the documented maximum a WorkSpaces resource
/// carries.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// The create requests the persistence case spans. The rows differ along what the rule
/// generalises over: which optional members are present, how many rules a group carries
/// and in what form, and how many tags come with it.
fn create_table() -> Vec<(&'static str, Value)> {
    vec![
        ("the required member alone", json!({ "GroupName": "grp-minimal" })),
        (
            "a description too",
            json!({ "GroupName": "grp-described", "GroupDesc": "race" }),
        ),
        (
            "one rule, with a description",
            json!({
                "GroupName": "grp-one-rule",
                "UserRules": [{ "ipRule": "10.0.0.0/16", "ruleDesc": "the office" }],
            }),
        ),
        (
            "three rules of differing form and family",
            json!({
                "GroupName": "grp-three-rules",
                "UserRules": [
                    { "ipRule": "0.0.0.0/0" },
                    { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
                    { "ipRule": "203.0.113.7" },
                ],
            }),
        ),
        (
            "tags at the documented maximum",
            json!({ "GroupName": "grp-tagged", "Tags": tags(MAX_TAGS_PER_RESOURCE) }),
        ),
        (
            "every member at once",
            json!({
                "GroupName": "sim2real-wsipg-race_01",
                "GroupDesc": "every member at once",
                "UserRules": [{ "ipRule": "192.0.2.0/24", "ruleDesc": "example range" }],
                "Tags": tags(2),
            }),
        ),
    ]
}

#[tokio::test]
async fn created_ip_groups_are_persisted_and_described_back() {
    let (router, engine) = emulator_with_engine();
    let mut created: Vec<String> = Vec::new();

    for (case, request) in create_table() {
        let group_id = create_one(&router, &request).await;
        assert!(
            !created.contains(&group_id),
            "case {case:?}: every create mints a distinct GroupId, got {group_id} again among {created:?}"
        );

        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["groupId"], json!(group_id),
            "case {case:?}: the group reports the minted id, got {reported}"
        );
        assert_eq!(
            reported["groupName"], request["GroupName"],
            "case {case:?}: the group reports the requested name, got {reported}"
        );
        assert_eq!(
            reported["groupDesc"], request["GroupDesc"],
            "case {case:?}: the group reports the requested description, got {reported}"
        );
        let expected_rules = expected_user_rules(&request["UserRules"]);
        assert_eq!(
            reported["userRules"], expected_rules,
            "case {case:?}: the group reports the requested rules, got {reported}"
        );

        // `DescribeIpGroups` has no member for tags and `DescribeTags` is still a
        // stub, so the persisted tags are read back through the same state API the
        // handlers write them through.
        let expected_tags = request["Tags"].as_array().map_or(0, Vec::len);
        let state = service_state(&*engine).expect("the WorkSpaces state store");
        let state = state.lock();
        let record = state
            .ip_group(&group_id)
            .unwrap_or_else(|| panic!("case {case:?}: {group_id} must be persisted"));
        assert_eq!(
            record.tags.len(),
            expected_tags,
            "case {case:?}: every requested tag is persisted, got {:?}",
            record.tags
        );
        drop(state);

        created.push(group_id);
    }

    // A full sweep reports exactly the groups created, in creation order.
    let swept: Vec<String> = sweep(&router, None)
        .await
        .iter()
        .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        swept, created,
        "a sweep reports every created group once, in creation order"
    );

    // A read changes nothing a later caller sees.
    let again: Vec<String> = sweep(&router, None)
        .await
        .iter()
        .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        again, created,
        "describing the groups must not change what the next caller sees"
    );
}

#[tokio::test]
async fn a_name_already_taken_is_refused_and_leaves_exactly_one_group() {
    let (router, _engine) = emulator_with_engine();

    // The rows differ in the form and length of the shared name and in how many
    // creates follow the accepted one.
    let table: Vec<(&str, String, usize)> = vec![
        ("short name, one repeat", "g1".to_string(), 1),
        ("dotted and underscored name", "sim2real.wsipg_race-01".to_string(), 3),
        ("name with digits only", "20261022".to_string(), 2),
        ("long name", "g".repeat(64), 1),
    ];

    for (case, name, repeats) in table {
        let winner = create_one(&router, &json!({ "GroupName": name })).await;
        for repeat in 0..repeats {
            let (status, body) = create(
                &router,
                &json!({ "GroupName": name, "GroupDesc": format!("repeat {repeat}") }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: repeat {repeat} on a taken name is a 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceAlreadyExistsException",
                "case {case:?}: repeat {repeat} is a ResourceAlreadyExistsException, got {body}"
            );
        }

        let surviving: Vec<Value> = sweep(&router, Some(2))
            .await
            .into_iter()
            .filter(|group| group["groupName"] == json!(name))
            .collect();
        assert_eq!(
            surviving.len(),
            1,
            "case {case:?}: exactly one group survives under the name, got {surviving:?}"
        );
        assert_eq!(
            surviving[0]["groupId"], json!(winner),
            "case {case:?}: the surviving group is the accepted one, got {:?}",
            surviving[0]
        );
        // The refused creates left nothing behind: the survivor still carries no
        // description, which only a refused repeat ever supplied.
        assert!(
            surviving[0]["groupDesc"].is_null(),
            "case {case:?}: a refused create must not modify the stored group, got {:?}",
            surviving[0]
        );
    }
}

/// Concurrent creates on ONE name resolve to exactly one winner: the name is claimed
/// under the state lock, so every other caller is refused and one group survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_on_one_name_leave_exactly_one_group() {
    let (router, _engine) = emulator_with_engine();
    let router = Arc::new(router);

    // The rows differ in how many callers race for the one name.
    for racers in [2_usize, 4, 8] {
        let name = format!("sim2real-wsipg-race-{racers}");
        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                let request = json!({
                    "GroupName": name,
                    "GroupDesc": "race",
                    "Tags": [{ "Key": "sim2real-behav", "Value": "true" }],
                });
                let (status, body) = create(&router, &request).await;
                (racer, status, body)
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        let mut losers: Vec<usize> = Vec::new();
        for task in tasks {
            let (racer, status, body) = task.await.expect("the racing task completed");
            match status {
                200 => {
                    let group_id = body["GroupId"].as_str().unwrap_or_else(|| {
                        panic!("case {racers} racers: racer {racer} won without a GroupId: {body}")
                    });
                    assert!(
                        is_modelled_ip_group_id(group_id),
                        "case {racers} racers: a winner's GroupId matches ^wsipg-[0-9a-z]{{8,63}}$, got {group_id:?}"
                    );
                    winners.push(group_id.to_string());
                }
                400 => {
                    assert_eq!(
                        error_code(&body),
                        "ResourceAlreadyExistsException",
                        "case {racers} racers: a loser is refused with ResourceAlreadyExistsException, got {body}"
                    );
                    losers.push(racer);
                }
                other => panic!(
                    "case {racers} racers: racer {racer} answered HTTP {other} with {body}"
                ),
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {racers} racers: exactly one create is accepted, got winners {winners:?} and losers {losers:?}"
        );
        assert_eq!(
            losers.len(),
            racers - 1,
            "case {racers} racers: every other create is refused, got losers {losers:?}"
        );
        assert_eq!(
            winners.iter().collect::<BTreeSet<_>>().len(),
            winners.len(),
            "case {racers} racers: no two winners share a GroupId, got {winners:?}"
        );

        // The terminal read: one persisted survivor under the name, and it is the
        // group the accepted call named.
        let surviving: Vec<Value> = sweep(&router, Some(3))
            .await
            .into_iter()
            .filter(|group| group["groupName"] == json!(name))
            .collect();
        assert_eq!(
            surviving.len(),
            1,
            "case {racers} racers: exactly one group is persisted under the name, got {surviving:?}"
        );
        assert_eq!(
            surviving[0]["groupId"], json!(winners[0]),
            "case {racers} racers: the survivor is the accepted group, got {:?}",
            surviving[0]
        );
    }
}

#[tokio::test]
async fn describe_filters_and_paginates_over_a_stable_creation_order() {
    let (router, _engine) = emulator_with_engine();
    let mut expected = Vec::new();
    for index in 0..5 {
        expected.push(create_one(&router, &json!({ "GroupName": format!("grp-{index}") })).await);
    }

    // Every page size the model allows at or below the stored count, the bound itself
    // (`MaxResults` is `range 1..25`), and a size that divides the count exactly.
    for page_size in [1_usize, 2, 3, 5, 25] {
        let walked: Vec<String> = sweep(&router, Some(page_size))
            .await
            .iter()
            .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            walked, expected,
            "case MaxResults={page_size}: paging must walk every group once, in creation order"
        );
    }

    // The shape of a first page: MaxResults items plus a NextToken, and the follow-up
    // call resumes exactly where it ended.
    let (_, first) = call(&router, "DescribeIpGroups", &json!({ "MaxResults": 2 })).await;
    let token = first["NextToken"].as_str().unwrap_or_else(|| {
        panic!("a first page of two out of five carries a NextToken, got {first}")
    });
    let resumed = json!({ "MaxResults": 3, "NextToken": token });
    let (_, second) = call(&router, "DescribeIpGroups", &resumed).await;
    let tail: Vec<String> = second["Result"]
        .as_array()
        .unwrap_or_else(|| panic!("case {resumed}: Result must be a list, got {second}"))
        .iter()
        .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        tail,
        expected[2..].to_vec(),
        "the follow-up page resumes where the previous one ended, got {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "the last page carries no NextToken, got {second}"
    );

    // `GroupIds` selects only the groups it names, in creation order.
    for selection in [vec![0_usize], vec![0, 4], vec![1, 2, 3], vec![0, 1, 2, 3, 4]] {
        let ids: Vec<String> = selection.iter().map(|index| expected[*index].clone()).collect();
        let request = json!({ "GroupIds": ids });
        let (status, body) = call(&router, "DescribeIpGroups", &request).await;
        assert_eq!(
            status, 200,
            "case {request}: DescribeIpGroups must answer 200, got {status} with {body}"
        );
        let reported: Vec<String> = body["Result"]
            .as_array()
            .unwrap_or_else(|| panic!("case {request}: Result must be a list, got {body}"))
            .iter()
            .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, ids,
            "case {request}: only the groups named are reported, got {body}"
        );
    }

    // A `GroupIds` that names no stored group reports nothing rather than failing:
    // the operation models no not-found error.
    let request = json!({ "GroupIds": ["wsipg-ffffffff"] });
    let (status, body) = call(&router, "DescribeIpGroups", &request).await;
    assert_eq!(
        status, 200,
        "case {request}: an unknown GroupId is still a 200, got {status} with {body}"
    );
    assert_eq!(
        body["Result"], json!([]),
        "case {request}: an unknown GroupId reports no group, got {body}"
    );

    // A token this service never minted is refused, not answered with page one.
    for token in ["not-a-token", "0000000000000", "000000000000000G", "ffffffffffffffffff"] {
        let request = json!({ "NextToken": token });
        let (status, body) = call(&router, "DescribeIpGroups", &request).await;
        assert_eq!(
            status, 400,
            "case {token:?}: an invalid NextToken is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {token:?}: an invalid NextToken is an InvalidParameterValuesException, got {body}"
        );
    }

    // `MaxResults` is `range 1..25`: each published bound is accepted and the values
    // just past them are refused at the boundary.
    for (max_results, expected_status) in [(1, 200), (25, 200), (0, 400), (26, 400)] {
        let request = json!({ "MaxResults": max_results });
        let (status, body) = call(&router, "DescribeIpGroups", &request).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn rules_that_are_not_ip_ranges_are_refused() {
    let (router, _engine) = emulator_with_engine();

    // `IpRuleItem$ipRule` is documented as "The IP address range, in CIDR notation",
    // with no constraint on family or prefix length, so a range of either family, a
    // prefix at either end of its family's width and a bare address are all accepted.
    let accepted = [
        "0.0.0.0/0",
        "10.0.0.0/8",
        "192.0.2.1/32",
        "2001:db8::/32",
        "::/0",
        "2001:db8::1/128",
        "203.0.113.7",
        "2001:db8::1",
    ];
    for (index, ip_rule) in accepted.iter().enumerate() {
        let request = json!({
            "GroupName": format!("grp-ok-{index}"),
            "UserRules": [{ "ipRule": ip_rule }],
        });
        let group_id = create_one(&router, &request).await;
        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["userRules"], json!([{ "ipRule": ip_rule }]),
            "case {ip_rule:?}: the rule is persisted as sent, got {reported}"
        );
    }

    // Anything that is not an address range: a word, a prefix wider than its family,
    // an octet out of range, a missing or empty prefix, an address range with a port.
    let refused = [
        "not-a-cidr",
        "10.0.0.0/33",
        "2001:db8::/129",
        "10.0.0.256/24",
        "10.0.0.0/",
        "",
        "192.0.2.1:443",
        "10.0.0.0 - 10.0.0.255",
    ];
    for (index, ip_rule) in refused.iter().enumerate() {
        let request = json!({
            "GroupName": format!("grp-bad-{index}"),
            "UserRules": [{ "ipRule": ip_rule, "ruleDesc": "refused" }],
        });
        let (status, body) = create(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {ip_rule:?}: a rule that is not an IP range is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {ip_rule:?}: that refusal is an InvalidParameterValuesException, got {body}"
        );
    }

    // A refused create persists nothing: only the accepted names are stored.
    let stored: Vec<String> = sweep(&router, Some(4))
        .await
        .iter()
        .map(|group| group["groupName"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = (0..accepted.len())
        .map(|index| format!("grp-ok-{index}"))
        .collect();
    assert_eq!(
        stored, expected,
        "a refused create must persist nothing, stored {stored:?}"
    );
}

#[tokio::test]
async fn more_tags_than_a_resource_can_carry_are_refused() {
    let (router, _engine) = emulator_with_engine();

    // "Each WorkSpaces resource can have a maximum of 50 tags": every count up to the
    // maximum is accepted, and the first count past it is refused.
    for count in [0_usize, 1, MAX_TAGS_PER_RESOURCE - 1, MAX_TAGS_PER_RESOURCE] {
        let request = json!({ "GroupName": format!("grp-tags-{count}"), "Tags": tags(count) });
        create_one(&router, &request).await;
    }

    let count = MAX_TAGS_PER_RESOURCE + 1;
    let request = json!({ "GroupName": format!("grp-tags-{count}"), "Tags": tags(count) });
    let (status, body) = create(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {count} tags: more tags than the maximum is a 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValuesException",
        "case {count} tags: that refusal is an InvalidParameterValuesException, got {body}"
    );
    let stored = sweep(&router, None).await.len();
    assert_eq!(
        stored, 4,
        "case {count} tags: the refused create persists nothing, {stored} groups stored"
    );
}

#[tokio::test]
async fn the_ip_group_registry_refuses_to_grow_past_its_bound() {
    let (router, engine) = emulator_with_engine();
    // Fill the registry to its bound through the same state API the handler uses, so
    // the case costs one wire call rather than MAX_STORED_IP_GROUPS of them.
    {
        let state = service_state(&*engine).expect("the WorkSpaces state store");
        let mut state = state.lock();
        for index in 0..MAX_STORED_IP_GROUPS {
            state
                .create_ip_group(&format!("grp-{index}"), None, Vec::new(), Vec::new())
                .unwrap_or_else(|failure| {
                    panic!("case {index}: filling must succeed, got {failure:?}")
                });
        }
        assert_eq!(
            state.ip_group_count(),
            MAX_STORED_IP_GROUPS,
            "the registry must hold exactly its bound before the refused create"
        );
    }

    let request = json!({ "GroupName": "one-too-many" });
    let (status, body) = create(&router, &request).await;
    assert_eq!(
        status, 400,
        "a create past the registry bound is a 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceLimitExceededException",
        "a create past the registry bound is a ResourceLimitExceededException, got {body}"
    );
}

#[tokio::test]
async fn caller_sized_members_are_bounded_before_anything_is_persisted() {
    let (router, _engine) = emulator_with_engine();

    // The model constrains the length of none of these members, so the bounds are
    // the emulator's own (MAX_RULES_PER_IP_GROUP, MAX_IP_GROUP_TEXT_CHARS). Each row
    // is a member at its bound — accepted — and one value past it — refused.
    let bound = MAX_IP_GROUP_TEXT_CHARS;
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "rule count",
            json!({ "GroupName": "grp-rules-at-bound", "UserRules": ip_rules(MAX_RULES_PER_IP_GROUP) }),
            json!({ "GroupName": "grp-rules-past-bound", "UserRules": ip_rules(MAX_RULES_PER_IP_GROUP + 1) }),
        ),
        (
            "group name length",
            json!({ "GroupName": "n".repeat(bound) }),
            json!({ "GroupName": "n".repeat(bound + 1) }),
        ),
        (
            "group description length",
            json!({ "GroupName": "grp-desc-at-bound", "GroupDesc": "d".repeat(bound) }),
            json!({ "GroupName": "grp-desc-past-bound", "GroupDesc": "d".repeat(bound + 1) }),
        ),
        (
            "rule description length",
            json!({
                "GroupName": "grp-ruledesc-at-bound",
                "UserRules": [{ "ipRule": "192.0.2.0/24", "ruleDesc": "r".repeat(bound) }],
            }),
            json!({
                "GroupName": "grp-ruledesc-past-bound",
                "UserRules": [{ "ipRule": "192.0.2.0/24", "ruleDesc": "r".repeat(bound + 1) }],
            }),
        ),
    ];

    let mut accepted = 0usize;
    for (case, at_bound, past_bound) in table {
        create_one(&router, &at_bound).await;
        accepted += 1;

        let (status, body) = create(&router, &past_bound).await;
        assert_eq!(
            status, 400,
            "case {case:?}: one past the bound is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {case:?}: that refusal is an InvalidParameterValuesException, got {body}"
        );

        let stored = sweep(&router, None).await.len();
        assert_eq!(
            stored, accepted,
            "case {case:?}: the refused create persists nothing, {stored} groups stored"
        );
    }
}

/// The rule sets one group's `UserRules` is replaced by, applied in order to the SAME
/// group. The rows differ along what the replacement rule generalises over: how many
/// rules the submitted set holds (more than before, fewer, none, the stored bound),
/// which of them are new, their order, their address family and form, and whether each
/// carries a `ruleDesc`.
fn replacement_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a rule added beside the one created with",
            json!([
                { "ipRule": "192.168.1.0/24", "ruleDesc": "vpn range" },
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
            ]),
        ),
        (
            "the same two rules in the opposite order",
            json!([
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
                { "ipRule": "192.168.1.0/24", "ruleDesc": "vpn range" },
            ]),
        ),
        (
            "a wholly different single rule",
            json!([{ "ipRule": "172.16.0.0/12", "ruleDesc": "replaced range" }]),
        ),
        (
            "one rule with no description",
            json!([{ "ipRule": "198.51.100.0/24" }]),
        ),
        (
            "an IPv6 range and a bare address",
            json!([
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
                { "ipRule": "203.0.113.7" },
            ]),
        ),
        (
            "as many rules as the emulator stores",
            ip_rules(MAX_RULES_PER_IP_GROUP),
        ),
        ("no rules at all", json!([])),
        (
            "one rule again, after the empty set",
            json!([{ "ipRule": "10.1.0.0/16", "ruleDesc": "authorized again" }]),
        ),
    ]
}

#[tokio::test]
async fn updating_the_rules_of_a_group_replaces_the_stored_rule_set() {
    let (router, _engine) = emulator_with_engine();
    let group_id = create_one(
        &router,
        &json!({
            "GroupName": "grp-under-update",
            "GroupDesc": "the group whose rules are replaced",
            "UserRules": [{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }],
        }),
    )
    .await;
    // A second group, never updated: a replacement touches only the group it names.
    let bystander_rules = json!([{ "ipRule": "203.0.113.0/24", "ruleDesc": "left alone" }]);
    let bystander = create_one(
        &router,
        &json!({ "GroupName": "grp-bystander", "UserRules": bystander_rules.clone() }),
    )
    .await;

    for (case, submitted) in replacement_table() {
        let request = json!({ "GroupId": group_id, "UserRules": submitted });
        let (status, body) = update_rules(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRulesOfIpGroup must answer 200, got {status} with {body}"
        );
        assert_no_output_members(case, &body);

        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["userRules"],
            expected_user_rules(&submitted),
            "case {case:?}: the group reports exactly the submitted rules, got {reported}"
        );
        // Replacing the rule set changes nothing else about the group.
        assert_eq!(
            reported["groupId"], json!(group_id),
            "case {case:?}: the group keeps its id, got {reported}"
        );
        assert_eq!(
            reported["groupName"], json!("grp-under-update"),
            "case {case:?}: the group keeps its name, got {reported}"
        );
        assert_eq!(
            reported["groupDesc"], json!("the group whose rules are replaced"),
            "case {case:?}: the group keeps its description, got {reported}"
        );

        let other = describe_one(&router, &bystander).await;
        assert_eq!(
            other["userRules"],
            expected_user_rules(&bystander_rules),
            "case {case:?}: only the group named is updated, got {other}"
        );
    }
}

#[tokio::test]
async fn a_refused_update_leaves_the_stored_rule_set_untouched() {
    let (router, _engine) = emulator_with_engine();
    let stored_rules = json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]);
    let group_id = create_one(
        &router,
        &json!({ "GroupName": "grp-refused-updates", "UserRules": stored_rules.clone() }),
    )
    .await;

    // A group this emulator does not hold: both an id it never minted and the id of a
    // group that was deleted are `ResourceNotFoundException`, never a silent success.
    let doomed = create_one(&router, &json!({ "GroupName": "grp-doomed" })).await;
    let (status, body) = delete_group(&router, &json!({ "GroupId": doomed })).await;
    assert_eq!(
        status, 200,
        "deleting a stored group must answer 200, got {status} with {body}"
    );
    for (case, missing) in [
        ("an id never minted", "wsipg-ffffffff"),
        ("the id of a deleted group", doomed.as_str()),
    ] {
        let request = json!({ "GroupId": missing, "UserRules": [{ "ipRule": "192.0.2.0/24" }] });
        let (status, body) = update_rules(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: updating a group that is not stored is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: that refusal is a ResourceNotFoundException, got {body}"
        );
    }

    // A rule list this emulator refuses: the same list `CreateIpGroup` refuses, refused
    // the same way, and refused BEFORE anything is written.
    let desc_past_bound = "r".repeat(MAX_IP_GROUP_TEXT_CHARS + 1);
    let refused: Vec<(&str, Value)> = vec![
        (
            "a rule that is not an address range",
            json!([{ "ipRule": "not-a-cidr", "ruleDesc": "refused" }]),
        ),
        (
            "a prefix wider than its family",
            json!([{ "ipRule": "10.0.0.0/33" }]),
        ),
        (
            "an accepted rule beside a refused one",
            json!([{ "ipRule": "192.0.2.0/24" }, { "ipRule": "10.0.0.256/24" }]),
        ),
        (
            "one rule more than the emulator stores",
            ip_rules(MAX_RULES_PER_IP_GROUP + 1),
        ),
        (
            "a rule description one character past the bound",
            json!([{ "ipRule": "192.0.2.0/24", "ruleDesc": desc_past_bound }]),
        ),
    ];
    for (case, rules) in refused {
        let request = json!({ "GroupId": group_id, "UserRules": rules });
        let (status, body) = update_rules(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused rule list is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {case:?}: that refusal is an InvalidParameterValuesException, got {body}"
        );

        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["userRules"],
            expected_user_rules(&stored_rules),
            "case {case:?}: a refused update must not change the stored rules, got {reported}"
        );
    }
}

#[tokio::test]
async fn deleting_a_group_removes_it_from_the_registry() {
    let (router, _engine) = emulator_with_engine();

    // The rows differ in how many groups are stored and which of them is deleted: the
    // first, one in the middle, the last, and the only one.
    let table: Vec<(&str, usize, usize)> = vec![
        ("the only group", 1, 0),
        ("the first of three", 3, 0),
        ("the middle of three", 3, 1),
        ("the last of four", 4, 3),
    ];

    for (case, count, victim) in table {
        let mut created = Vec::new();
        for index in 0..count {
            created.push(
                create_one(
                    &router,
                    &json!({
                        "GroupName": format!("{case}-{index}"),
                        "UserRules": [{ "ipRule": format!("10.{index}.0.0/16") }],
                    }),
                )
                .await,
            );
        }
        let doomed = created[victim].clone();

        let (status, body) = delete_group(&router, &json!({ "GroupId": doomed })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteIpGroup must answer 200 for a stored group, got {status} with {body}"
        );
        assert_no_output_members(case, &body);

        // A full sweep no longer lists it, and every surviving group keeps its place
        // in creation order.
        let surviving: Vec<String> = sweep(&router, Some(2))
            .await
            .iter()
            .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = created
            .iter()
            .filter(|group_id| **group_id != doomed)
            .cloned()
            .collect();
        assert_eq!(
            surviving, expected,
            "case {case:?}: a deleted group is no longer reported, got {surviving:?}"
        );

        // Nor does asking for it by id, which the operation models no error for.
        let request = json!({ "GroupIds": [doomed] });
        let (status, body) = call(&router, "DescribeIpGroups", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describing a deleted group by id is still a 200, got {status} with {body}"
        );
        assert_eq!(
            body["Result"], json!([]),
            "case {case:?}: a deleted group is reported by nothing, got {body}"
        );

        // Deleting it again is a ResourceNotFoundException: the deletion is complete
        // when the call returns, so a second one finds nothing.
        let (status, body) = delete_group(&router, &json!({ "GroupId": doomed })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a group twice is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: that refusal is a ResourceNotFoundException, got {body}"
        );

        // The name the deleted group held is free again, and the group created under
        // it is a DIFFERENT group.
        let name = format!("{case}-{victim}");
        let recreated = create_one(&router, &json!({ "GroupName": name })).await;
        assert_ne!(
            recreated, doomed,
            "case {case:?}: recreating a deleted group's name mints a new id"
        );

        // Clear the registry for the next row: every group left, including the
        // recreated one, is deleted through the wire.
        for group_id in expected.iter().chain(std::iter::once(&recreated)) {
            let (status, body) = delete_group(&router, &json!({ "GroupId": group_id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: clearing {group_id} must answer 200, got {status} with {body}"
            );
        }
        let left = sweep(&router, None).await;
        assert!(
            left.is_empty(),
            "case {case:?}: every group was deleted, got {left:?}"
        );
    }
}

/// An id a mutation names must be the id of a stored group: `DeleteIpGroup` on an id
/// this emulator never minted is refused rather than accepted as a no-op.
#[tokio::test]
async fn deleting_a_group_that_is_not_stored_is_refused() {
    let (router, _engine) = emulator_with_engine();
    let kept = create_one(&router, &json!({ "GroupName": "grp-kept" })).await;

    // Well-formed `IpGroupId` values (`^wsipg-[0-9a-z]{8,63}$`) naming no stored group:
    // at the pattern's shortest suffix, at its longest, and in between.
    for suffix in ["00000000", "ffffffff", "deadbeef00", &"z".repeat(63)] {
        let group_id = format!("wsipg-{suffix}");
        let (status, body) = delete_group(&router, &json!({ "GroupId": group_id })).await;
        assert_eq!(
            status, 400,
            "case {group_id:?}: deleting an unknown group is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {group_id:?}: that refusal is a ResourceNotFoundException, got {body}"
        );
    }

    let surviving: Vec<String> = sweep(&router, None)
        .await
        .iter()
        .map(|group| group["groupId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        surviving,
        vec![kept],
        "a refused delete must not remove anything, got {surviving:?}"
    );
}

/// The authorize cases, as (case, the rules the group holds, the rules submitted, the
/// rule set the group must hold afterwards).
///
/// The expected column is this test's statement of the rule, written out rather than
/// computed: the rules just authorized come first, in the order submitted, then the
/// rules the group already held that the request did not name, in the order it held
/// them, with at most one rule per address range. The rows differ along what that
/// generalises over: how many rules are held and submitted, whether a submitted range
/// is new or already held, the address family and form of the ranges, and whether a
/// rule carries a description.
#[allow(clippy::type_complexity)]
fn authorize_table() -> Vec<(&'static str, Value, Value, Value)> {
    vec![
        (
            "one new rule onto a group holding one",
            json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]),
            json!([{ "ipRule": "192.168.1.0/24", "ruleDesc": "vpn range" }]),
            json!([
                { "ipRule": "192.168.1.0/24", "ruleDesc": "vpn range" },
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
            ]),
        ),
        (
            "two new rules onto a group holding two",
            json!([{ "ipRule": "10.0.0.0/16" }, { "ipRule": "10.1.0.0/16" }]),
            json!([
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
                { "ipRule": "203.0.113.7" },
            ]),
            json!([
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
                { "ipRule": "203.0.113.7" },
                { "ipRule": "10.0.0.0/16" },
                { "ipRule": "10.1.0.0/16" },
            ]),
        ),
        (
            "one new rule onto a group holding none",
            json!([]),
            json!([{ "ipRule": "0.0.0.0/0", "ruleDesc": "from anywhere" }]),
            json!([{ "ipRule": "0.0.0.0/0", "ruleDesc": "from anywhere" }]),
        ),
        (
            "a range the group already holds, with a new description",
            json!([
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
                { "ipRule": "192.0.2.0/24" },
            ]),
            json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "office range, renamed" }]),
            json!([
                { "ipRule": "10.0.0.0/16", "ruleDesc": "office range, renamed" },
                { "ipRule": "192.0.2.0/24" },
            ]),
        ),
        (
            "a new range beside one already held",
            json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]),
            json!([
                { "ipRule": "198.51.100.0/24", "ruleDesc": "branch office" },
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
            ]),
            json!([
                { "ipRule": "198.51.100.0/24", "ruleDesc": "branch office" },
                { "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" },
            ]),
        ),
        (
            "no rules at all, onto a group holding one",
            json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]),
            json!([]),
            json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]),
        ),
    ]
}

#[tokio::test]
async fn authorizing_rules_adds_them_to_the_stored_rule_set() {
    let (router, _engine) = emulator_with_engine();

    for (index, (case, held, submitted, expected)) in authorize_table().into_iter().enumerate() {
        let group_id = create_one(
            &router,
            &json!({ "GroupName": format!("grp-authorize-{index}"), "UserRules": held }),
        )
        .await;

        let request = json!({ "GroupId": group_id, "UserRules": submitted });
        let (status, body) = authorize_rules(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AuthorizeIpRules must answer 200, got {status} with {body}"
        );
        assert_no_output_members(case, &body);

        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["userRules"],
            expected_user_rules(&expected),
            "case {case:?}: the group must hold the authorized rules beside the ones it held, got {reported}"
        );
    }
}

#[tokio::test]
async fn authorizing_more_rules_than_a_group_holds_is_refused_and_changes_nothing() {
    let (router, _engine) = emulator_with_engine();
    let held = ip_rules(MAX_RULES_PER_IP_GROUP);
    let group_id = create_one(
        &router,
        &json!({ "GroupName": "grp-at-the-rule-bound", "UserRules": held.clone() }),
    )
    .await;

    // A group at the bound: one more range does not fit, but re-authorizing ranges it
    // already holds does, because a group holds at most one rule per range.
    let (status, body) = authorize_rules(
        &router,
        &json!({ "GroupId": group_id, "UserRules": [{ "ipRule": "203.0.113.0/24" }] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a rule past the bound is a 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceLimitExceededException",
        "a rule past the bound is a ResourceLimitExceededException, got {body}"
    );
    let reported = describe_one(&router, &group_id).await;
    assert_eq!(
        reported["userRules"],
        expected_user_rules(&held),
        "a refused authorize must not change the stored rules, got {reported}"
    );

    let already_held = json!([{ "ipRule": "10.0.0.0/24", "ruleDesc": "re-authorized" }]);
    let (status, body) = authorize_rules(
        &router,
        &json!({ "GroupId": group_id, "UserRules": already_held }),
    )
    .await;
    assert_eq!(
        status, 200,
        "re-authorizing a range the group holds must answer 200, got {status} with {body}"
    );
    let reported = describe_one(&router, &group_id).await;
    let rule_count = reported["userRules"].as_array().map_or(0, Vec::len);
    assert_eq!(
        rule_count, MAX_RULES_PER_IP_GROUP,
        "a group holds at most one rule per range, got {rule_count} rules: {reported}"
    );
}

/// The revoke cases, as (case, the rules the group holds, the ranges revoked, the rule
/// set the group must hold afterwards).
///
/// The rows differ along what the rule generalises over: which position in the held set
/// is revoked, how many ranges one call names, a range the group does not hold, a range
/// named twice, and revoking everything.
#[allow(clippy::type_complexity)]
fn revoke_table() -> Vec<(&'static str, Value, Value, Value)> {
    let three = json!([
        { "ipRule": "10.0.0.0/16", "ruleDesc": "office" },
        { "ipRule": "192.0.2.0/24" },
        { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
    ]);
    vec![
        (
            "the first of three",
            three.clone(),
            json!(["10.0.0.0/16"]),
            json!([
                { "ipRule": "192.0.2.0/24" },
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
            ]),
        ),
        (
            "the middle of three",
            three.clone(),
            json!(["192.0.2.0/24"]),
            json!([
                { "ipRule": "10.0.0.0/16", "ruleDesc": "office" },
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
            ]),
        ),
        (
            "two of three at once",
            three.clone(),
            json!(["2001:db8::/32", "10.0.0.0/16"]),
            json!([{ "ipRule": "192.0.2.0/24" }]),
        ),
        (
            "every rule the group holds",
            three.clone(),
            json!(["10.0.0.0/16", "192.0.2.0/24", "2001:db8::/32"]),
            json!([]),
        ),
        (
            "a range the group does not hold",
            three.clone(),
            json!(["198.51.100.0/24"]),
            three.clone(),
        ),
        (
            "one range named twice",
            three.clone(),
            json!(["10.0.0.0/16", "10.0.0.0/16"]),
            json!([
                { "ipRule": "192.0.2.0/24" },
                { "ipRule": "2001:db8::/32", "ruleDesc": "documentation range" },
            ]),
        ),
        ("no range at all", three.clone(), json!([]), three),
        (
            "the only rule of a group holding one",
            json!([{ "ipRule": "203.0.113.7", "ruleDesc": "one address" }]),
            json!(["203.0.113.7"]),
            json!([]),
        ),
    ]
}

#[tokio::test]
async fn revoking_rules_removes_exactly_the_ranges_named() {
    let (router, _engine) = emulator_with_engine();

    for (index, (case, held, revoked, expected)) in revoke_table().into_iter().enumerate() {
        let group_id = create_one(
            &router,
            &json!({ "GroupName": format!("grp-revoke-{index}"), "UserRules": held }),
        )
        .await;

        let request = json!({ "GroupId": group_id, "UserRules": revoked });
        let (status, body) = revoke_rules(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RevokeIpRules must answer 200, got {status} with {body}"
        );
        assert_no_output_members(case, &body);

        let reported = describe_one(&router, &group_id).await;
        assert_eq!(
            reported["userRules"],
            expected_user_rules(&expected),
            "case {case:?}: the group must hold exactly the rules not revoked, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_rule_mutation_on_a_group_that_is_not_stored_is_refused() {
    let (router, _engine) = emulator_with_engine();
    let stored_rules = json!([{ "ipRule": "10.0.0.0/16", "ruleDesc": "initial office range" }]);
    let group_id = create_one(
        &router,
        &json!({ "GroupName": "grp-bystander", "UserRules": stored_rules.clone() }),
    )
    .await;

    // Every rule mutation refuses an id this emulator does not hold, rather than
    // reporting a success that changed nothing.
    for missing in ["wsipg-00000000", "wsipg-ffffffff", "wsipg-deadbeef99"] {
        for (operation, request) in [
            (
                "AuthorizeIpRules",
                json!({ "GroupId": missing, "UserRules": [{ "ipRule": "192.0.2.0/24" }] }),
            ),
            (
                "RevokeIpRules",
                json!({ "GroupId": missing, "UserRules": ["192.0.2.0/24"] }),
            ),
            (
                "UpdateRulesOfIpGroup",
                json!({ "GroupId": missing, "UserRules": [{ "ipRule": "192.0.2.0/24" }] }),
            ),
            ("DeleteIpGroup", json!({ "GroupId": missing })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation} on {missing:?}: a group that is not stored is a 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} on {missing:?}: that refusal is a ResourceNotFoundException, got {body}"
            );
        }
    }

    // A value that is not an address range is refused by both rule mutations, and
    // neither touches the stored group.
    for bad in ["not-a-cidr", "10.0.0.0/33", "192.0.2.1:443", ""] {
        let (status, body) = authorize_rules(
            &router,
            &json!({ "GroupId": group_id, "UserRules": [{ "ipRule": bad }] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bad:?}: authorizing a value that is not an address range is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {bad:?}: that refusal is an InvalidParameterValuesException, got {body}"
        );

        let (status, body) = revoke_rules(
            &router,
            &json!({ "GroupId": group_id, "UserRules": [bad] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bad:?}: revoking a value that is not an address range is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValuesException",
            "case {bad:?}: that refusal is an InvalidParameterValuesException, got {body}"
        );
    }

    let reported = describe_one(&router, &group_id).await;
    assert_eq!(
        reported["userRules"],
        expected_user_rules(&stored_rules),
        "a refused rule mutation must not change the stored rules, got {reported}"
    );
}
