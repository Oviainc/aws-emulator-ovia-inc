// SPDX-License-Identifier: CC0-1.0

//! Group identity is exactly ONE selector, end to end over the wire.
//!
//! Every case drives the same front door the production router does (the
//! `Xray` harness in [`super::trace_ingest_and_groups`]), so the assertions
//! below are about HTTP responses, not about internal state.
//!
//! The rule under test: an operation that identifies a group by `GroupName` or
//! `GroupARN` — `GetGroup`, `UpdateGroup`, `DeleteGroup`
//! (`smithy_query(query="shape",shape="GetGroupRequest,UpdateGroupRequest,DeleteGroupRequest")`
//! shows the same optional pair on each) — accepts exactly one of them: a
//! request carrying BOTH states two identities at once and is refused,
//! unresolved and before any state change, with `InvalidRequestException` at
//! HTTP 400 and the fixed message "Provide either GroupName or GroupARN";
//! a request carrying exactly one selector is serviced as before.
//!
//! The rows differ along what that rule generalises over: which operation is
//! called (a reader, a mutator, a remover), whether the two selectors agree or
//! point at different stored groups, whether either names nothing stored, and
//! the published length bounds of each selector (`GroupName` 1..32, `GroupARN`
//! 1..400).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::trace_ingest_and_groups::{Answer, Xray};

/// The fixed wording X-Ray answers for a conflicting group identity; mirrors
/// `crate::state::CONFLICTING_GROUP_SELECTORS_MESSAGE`.
const CONFLICT_MESSAGE: &str = "Provide either GroupName or GroupARN";

/// Every operation in this crate that selects a group by the optional
/// `(GroupName, GroupARN)` pair, with its restJson1 path.
///
/// `GetServiceGraph`, `GetTimeSeriesServiceStatistics` and `GetInsightSummaries`
/// carry the same pair and refuse it through the same `find_group`, but each
/// also takes a required `[StartTime, EndTime]` window, so the rows below —
/// which carry selectors only — would be refused for the absent window instead;
/// their own suites own that refusal.
const GROUP_OPERATIONS: [&str; 3] = ["/GetGroup", "/UpdateGroup", "/DeleteGroup"];

/// A group seeded before the refusals, and the ARN minted for it.
#[derive(Debug, Clone)]
struct Seeded {
    name: String,
    filter: &'static str,
    arn: String,
}

/// The groups to seed. `GroupName`'s published `@length` is 1..32
/// (`smithy_query(query="shape",shape="UpdateGroupRequest")`), so one row sits
/// at each bound; the first two exist so that the two selectors can point at
/// two DIFFERENT stored groups.
fn seed_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ga", "service(\"a\")"),
        ("gb", "service(\"b\")"),
        // the shortest name the model allows ...
        ("a", "fault = true"),
        // ... and the longest.
        ("abcdefghijklmnopqrstuvwxyz012345", "responsetime > 2"),
    ]
}

/// Create every seeded group and collect the ARN each one was minted.
async fn seed(xray: &Xray) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (name, filter) in seed_rows() {
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": name, "FilterExpression": filter}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: seeding a group must succeed, got {created:?}"
        );
        let arn = created
            .get("Group")
            .get("GroupARN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {name:?}: CreateGroup must mint a GroupARN, got {created:?}"
        );
        seeded.push(Seeded {
            name: name.to_string(),
            filter,
            arn,
        });
    }
    seeded
}

/// `GroupName` -> `FilterExpression`, as `GetGroups` reports the store.
///
/// The reserved account-level `Default` group is listed too — it exists in
/// every region without being created — but it is not one of the groups these
/// cases seed, so it is left out here; `group_control_plane.rs` owns the rule
/// that it is always present.
async fn snapshot(xray: &Xray) -> BTreeMap<String, String> {
    let listed = xray.call("/Groups", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "GetGroups must succeed, got {listed:?}"
    );
    listed
        .get("Groups")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|group| {
            group.get("GroupName").and_then(Value::as_str) != Some("Default")
        })
        .map(|group| {
            (
                group
                    .get("GroupName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                group
                    .get("FilterExpression")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The conflicting-identity refusal: 400, `InvalidRequestException`, and the
/// fixed message.
fn assert_conflict_refused(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "case {case:?}: both selectors together must be refused with 400 InvalidRequestException, got {answer:?}"
    );
    // A modelled error shape renders its member's own casing (`Message`); a
    // framework refusal renders `message`.
    let message = answer
        .get("Message")
        .as_str()
        .or_else(|| answer.get("message").as_str());
    assert_eq!(
        message,
        Some(CONFLICT_MESSAGE),
        "case {case:?}: expected the fixed message {CONFLICT_MESSAGE:?}, got {answer:?}"
    );
}

/// The seeded group whose `GroupName` is exactly `length` characters long.
fn find_by_name_length(groups: &[Seeded], length: usize) -> &Seeded {
    groups
        .iter()
        .find(|group| group.name.chars().count() == length)
        .unwrap_or_else(|| panic!("the seed table must hold a GroupName of length {length}"))
}

/// The request body for one operation with the given selectors. An
/// `UpdateGroup` row also supplies the members it would have written, so a
/// serviced refusal would be visible in the store.
fn body(path: &str, group_name: Option<&str>, group_arn: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(name) = group_name {
        body["GroupName"] = json!(name);
    }
    if let Some(arn) = group_arn {
        body["GroupARN"] = json!(arn);
    }
    if path == "/UpdateGroup" {
        body["FilterExpression"] = json!("service(\"changed\")");
        body["InsightsConfiguration"] =
            json!({"InsightsEnabled": true, "NotificationsEnabled": true});
    }
    body
}

#[tokio::test]
async fn both_group_selectors_together_are_refused_and_change_nothing() {
    let xray = Xray::new();
    let groups = seed(&xray).await;
    let before = snapshot(&xray).await;

    // A GroupARN of exactly the published maximum length (400) and one of the
    // minimum (1), so the refusal does not depend on the ARN being resolvable.
    let longest_arn = "a".repeat(400);
    let shortest_arn = "a".to_string();
    let absent_arn = "arn:aws:xray:us-east-1:000000000000:group/absent/0123456789ab".to_string();

    // Picked by the property each row is about rather than by seed position, so
    // inserting a seed row cannot silently change what these rows mean.
    let any = &groups[0];
    let shortest_name = find_by_name_length(&groups, 1);
    let longest_name = find_by_name_length(&groups, 32);

    for path in GROUP_OPERATIONS {
        // Every ordered pair of stored groups: the diagonal is two selectors
        // that AGREE, the rest are two that point at different groups.
        for by_name in &groups {
            for by_arn in &groups {
                let case = format!(
                    "{path} with GroupName {:?} and the GroupARN of {:?}",
                    by_name.name, by_arn.name
                );
                let answer = xray
                    .call(path, &body(path, Some(&by_name.name), Some(&by_arn.arn)))
                    .await;
                assert_conflict_refused(&case, &answer);
            }
        }

        // Rows where one or both selectors name nothing stored, and rows at each
        // published selector bound: the conflict is refused before resolution,
        // so the outcome does not depend on either selector resolving.
        let unresolvable: Vec<(String, Option<&str>, Option<&str>)> = vec![
            (
                format!("{path} with an absent GroupName and a stored GroupARN"),
                Some("absent"),
                Some(any.arn.as_str()),
            ),
            (
                format!("{path} with a stored GroupName and an absent GroupARN"),
                Some(any.name.as_str()),
                Some(absent_arn.as_str()),
            ),
            (
                format!("{path} with neither selector resolving"),
                Some("absent"),
                Some(absent_arn.as_str()),
            ),
            (
                format!("{path} with a GroupARN at the published maximum length"),
                Some(any.name.as_str()),
                Some(longest_arn.as_str()),
            ),
            (
                format!("{path} with a GroupARN at the published minimum length"),
                Some(any.name.as_str()),
                Some(shortest_arn.as_str()),
            ),
            (
                format!("{path} with a GroupName at the published maximum length"),
                Some(longest_name.name.as_str()),
                Some(any.arn.as_str()),
            ),
            (
                format!("{path} with a GroupName at the published minimum length"),
                Some(shortest_name.name.as_str()),
                Some(any.arn.as_str()),
            ),
        ];
        for (case, group_name, group_arn) in unresolvable {
            let answer = xray.call(path, &body(path, group_name, group_arn)).await;
            assert_conflict_refused(&case, &answer);
        }
    }

    // Not one refused call wrote, renamed or removed anything.
    let after = snapshot(&xray).await;
    assert_eq!(
        after, before,
        "a refused conflicting-identity call must leave every stored group as it was: expected {before:?}, got {after:?}"
    );

    // And each group still answers through EITHER selector on its own, with the
    // filter expression it was created with.
    for group in &groups {
        for (source, request) in [
            ("GroupName", json!({"GroupName": &group.name})),
            ("GroupARN", json!({"GroupARN": &group.arn})),
        ] {
            let read = xray.call("/GetGroup", &request).await;
            assert_eq!(
                read.status, 200,
                "case {:?} via {source}: a single selector must still resolve the group, got {read:?}",
                group.name
            );
            assert_eq!(
                read.get("Group").get("FilterExpression").and_then(Value::as_str),
                Some(group.filter),
                "case {:?} via {source}: the group must keep its created FilterExpression, got {read:?}",
                group.name
            );
            assert_eq!(
                read.get("Group").get("GroupARN").and_then(Value::as_str),
                Some(group.arn.as_str()),
                "case {:?} via {source}: the group must keep the ARN generated upon creation, got {read:?}",
                group.name
            );
        }
    }
}

#[tokio::test]
async fn a_single_selector_still_updates_reads_and_deletes_one_group() {
    let xray = Xray::new();
    let groups = seed(&xray).await;

    // The positive control: every group is updated through EACH selector on its
    // own, so each call is serviced, lands in the store, and touches no other
    // group.
    for group in &groups {
        for (source, selector) in [
            ("GroupName", json!({"GroupName": &group.name})),
            ("GroupARN", json!({"GroupARN": &group.arn})),
        ] {
            let mut request = selector.clone();
            let filter = format!("annotation.stage = \"{}-{source}\"", group.name);
            request["FilterExpression"] = json!(&filter);

            let updated = xray.call("/UpdateGroup", &request).await;
            assert_eq!(
                updated.status, 200,
                "case {:?} via {source}: a single-selector UpdateGroup must succeed, got {updated:?}",
                group.name
            );
            assert_eq!(
                updated
                    .get("Group")
                    .get("FilterExpression")
                    .and_then(Value::as_str),
                Some(filter.as_str()),
                "case {:?} via {source}: the update must replace FilterExpression, got {updated:?}",
                group.name
            );

            let read = xray
                .call("/GetGroup", &json!({"GroupName": &group.name}))
                .await;
            assert_eq!(
                read.get("Group")
                    .get("FilterExpression")
                    .and_then(Value::as_str),
                Some(filter.as_str()),
                "case {:?} via {source}: the update must be readable back, got {read:?}",
                group.name
            );

            let stored = snapshot(&xray).await;
            assert_eq!(
                stored.get(&group.name).map(String::as_str),
                Some(filter.as_str()),
                "case {:?} via {source}: the listing must report the updated filter, got {stored:?}",
                group.name
            );
            assert_eq!(
                stored.len(),
                groups.len(),
                "case {:?} via {source}: an update must neither create nor remove a group, got {stored:?}",
                group.name
            );
        }
    }

    // DeleteGroup by one selector removes exactly that group.
    let removed = xray
        .call("/DeleteGroup", &json!({"GroupARN": &groups[1].arn}))
        .await;
    assert_eq!(
        removed.status, 200,
        "a single-selector DeleteGroup must succeed, got {removed:?}"
    );
    let stored = snapshot(&xray).await;
    assert_eq!(
        stored.len(),
        groups.len() - 1,
        "DeleteGroup must remove exactly one group, got {stored:?}"
    );
    assert!(
        !stored.contains_key(&groups[1].name),
        "DeleteGroup must remove the group it addressed, got {stored:?}"
    );
}
