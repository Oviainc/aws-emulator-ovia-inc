// SPDX-License-Identifier: CC0-1.0

//! `GetTags`, read through every identity form a group answers to, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suites' `Emulator`), so these exercise
//! the whole restJson1 path: routing, the URI-bound `Arn` label, boundary
//! validation, the handler, and serialization. `GetTags` is
//! `GET /resources/{Arn}/tags`, so every call below is sent on that method and
//! path, and a status answered for it is the handler's own — an operation that
//! served nothing would answer 501 here instead of the 200s and 404s asserted
//! below.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * `GetTags` resolves its `Arn` against the identity this service MINTED for a
//!   group — in either form the modelled `GroupArnV2` admits, the group's name or
//!   its name qualified by the 26-character id the service gave it
//!   (`smithy_query(query="shape",shape="GroupArnV2")`) — so every ARN this
//!   service reports for a group reads that group's tags, and an ARN it reports
//!   for no group reads nothing;
//! * whichever form addresses it, the answer reports the group's STORED ARN
//!   beside the tag map the store holds at the moment of the call, so the map
//!   tracks every mutator that wrote it and the answer is not the ARN the request
//!   carried;
//! * an `Arn` that addresses no stored group is `NotFoundException` (404) with
//!   this service's fixed not-found sentence;
//! * reading tags mints nothing and changes nothing a later caller sees.
//!
//! The sibling `group_tags` suite asserts the merge rule `Tag` follows and the
//! pairs this service refuses; these rows are about which ARN reaches which
//! group's map, and about the map surviving each mutator.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::group_tags::{get_tags, group_arn, tag, tag_json, tag_pairs};
use super::group_untag::untag;
use super::resource_group_lifecycle::{error_code, Emulator};
use super::tag_sync_tasks::{application_segment, role_arn};

/// The service-configuration type whose declaration makes a group an application
/// group, whose stored identity is qualified by an application id from create
/// time.
///
/// Source: `smithy_query(query="doc",shape="Group",member="ApplicationTag")`,
/// which names the group kind the membership tag is "only supported for".
const APPLICATION_GROUP: &str = "AWS::ResourceGroups::ApplicationGroup";

/// A 26-character id shaped like the one the qualified alternative of the modelled
/// `GroupArnV2` carries (`[a-z0-9]{26}`), and minted for nothing: no stored group
/// answers to an ARN qualified by it.
///
/// Source: `smithy_query(query="shape",shape="GroupArnV2")`.
const UNMINTED_ID: &str = "0123456789abcdefghijklmnop";

/// The one sentence this service answers for an identifier that addresses no
/// stored group, whatever operation was asked.
const NOT_FOUND_MESSAGE: &str = "The specified group does not exist";

/// One group whose tags are then read through every identity form it answers to.
struct IdentityCase {
    label: &'static str,
    /// The region the creating caller's credential scope names, which is the
    /// region the group lives in and the one its stored ARN carries.
    region: &'static str,
    name: &'static str,
    /// The `CreateGroup` request, which is what decides whether the group's stored
    /// identity is qualified by an application id from the start.
    request: Value,
    /// Whether a tag-sync task is started on the group — the way a group that is
    /// NOT an application group comes to carry a qualifying id, since the task's
    /// own ARN is written under one.
    hosts_task: bool,
    /// The pairs the group carries when its tags are read.
    tags: Vec<(&'static str, &'static str)>,
}

/// The rows differ along what the resolution rule generalises over: whether the
/// group's stored identity is qualified by an application id, how that id came to
/// exist (declared at create time, or minted for a resource the group hosts) or
/// whether it exists at all, the region the group lives in, the punctuation its
/// name carries, and how many pairs its map holds (none, one, several).
fn identity_cases() -> Vec<IdentityCase> {
    vec![
        IdentityCase {
            label: "a group with neither a configuration nor a query, carrying no tags",
            region: "us-east-1",
            name: "plain-group",
            request: json!({ "Name": "plain-group" }),
            hosts_task: false,
            tags: Vec::new(),
        },
        IdentityCase {
            label: "a group whose name carries the punctuation a group name admits",
            region: "us-west-2",
            name: "dots.and_dashes-1",
            request: json!({ "Name": "dots.and_dashes-1" }),
            hosts_task: false,
            tags: vec![("Stage", "Test")],
        },
        IdentityCase {
            label: "an application group, whose stored identity is qualified at create time",
            region: "eu-west-1",
            name: "app-group",
            request: json!({
                "Name": "app-group",
                "Configuration": [{ "Type": APPLICATION_GROUP }],
            }),
            hosts_task: false,
            tags: vec![("Team", "Platform"), ("Owner", "sre")],
        },
        IdentityCase {
            label: "a group that hosts a tag-sync task, which mints a qualifying id for it",
            region: "ap-southeast-2",
            name: "task-host",
            request: json!({ "Name": "task-host" }),
            hosts_task: true,
            tags: vec![("cost:center", "platform engineering"), ("Équipe", "Ωμέγα 42")],
        },
    ]
}

/// Both sides of the rule are in the table: a group whose stored identity is
/// qualified by an application id, and one whose identity is its bare name.
#[test]
fn the_table_covers_qualified_and_unqualified_identities() {
    let labels: Vec<&str> = identity_cases().iter().map(|case| case.label).collect();
    for qualified in [true, false] {
        assert!(
            identity_cases()
                .iter()
                .any(|case| (case.request["Configuration"].is_array() || case.hosts_task)
                    == qualified),
            "case {qualified:?}: expected a row whose group carries a qualifying id \
             exactly when {qualified}, got {labels:?}",
        );
    }
}

/// Every ARN this service reports for a group reads that group's tags, the answer
/// reports the group's stored ARN whichever form asked, and an ARN the service
/// reports for no group reads nothing.
#[tokio::test]
async fn get_tags_answers_every_identity_form_the_service_reports() {
    let emulator = Emulator::new();
    for case in identity_cases() {
        let label = case.label;
        let (created_status, created) = emulator.call_in(case.region, "/groups", &case.request).await;
        assert_eq!(
            created_status, 200,
            "case {label:?}: expected CreateGroup to answer 200, got {created_status} {created}",
        );
        let stored_arn = created["Group"]["GroupArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a GroupArn, got {created}"))
            .to_string();
        let unqualified = group_arn(case.region, case.name);

        let pairs: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        if !pairs.is_empty() {
            let (status, answered) =
                tag(&emulator, case.region, &stored_arn, &tag_json(&pairs)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected Tag to answer 200, got {status} {answered}",
            );
        }
        let expected: BTreeMap<String, String> = pairs.into_iter().collect();

        // The forms this service itself reports for the group: its stored ARN
        // always, and the qualified ARN its hosted resources are written under
        // where the group carries a qualifying id.
        let mut resolving = vec![("its stored ARN", stored_arn.clone())];
        let mut refused = vec![(
            "an ARN qualified by an id minted for no group",
            format!("{unqualified}/{UNMINTED_ID}"),
        )];
        if case.hosts_task {
            let started = emulator
                .ok_in(
                    case.region,
                    "/start-tag-sync-task",
                    &json!({
                        "Group": case.name,
                        "RoleArn": role_arn("ResourceGroupsTagSync"),
                        "TagKey": "Stage",
                        "TagValue": "Test",
                    }),
                )
                .await;
            let task_arn = started["TaskArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: expected a TaskArn, got {started}"));
            let minted = application_segment(task_arn, &unqualified).unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected the task ARN to nest under {unqualified} with a \
                     qualifying id, got {task_arn}"
                )
            });
            resolving.push((
                "the qualified ARN its hosted resources are written under",
                format!("{unqualified}/{minted}"),
            ));
        }
        if stored_arn != unqualified {
            // The group's identity IS the qualified ARN, so the bare one names a
            // group this service never reported.
            refused.push((
                "the unqualified ARN of a group whose stored identity is qualified",
                unqualified.clone(),
            ));
        }

        for (form, arn) in &resolving {
            // Asked twice, because a read must answer the same map however often
            // it is asked.
            for round in 0..2 {
                let (status, body) = get_tags(&emulator, case.region, arn).await;
                assert_eq!(
                    status, 200,
                    "case {label:?} ({form}, read {round}): expected 200, got {status} {body}",
                );
                assert_eq!(
                    body["Arn"].as_str(),
                    Some(stored_arn.as_str()),
                    "case {label:?} ({form}, read {round}): expected the group's stored ARN \
                     {stored_arn}, got {body}",
                );
                assert_eq!(
                    tag_pairs(label, &body["Tags"]),
                    expected,
                    "case {label:?} ({form}, read {round}): expected the stored map \
                     {expected:?}, got {body}",
                );
            }
        }
        for (form, arn) in &refused {
            let (status, body) = get_tags(&emulator, case.region, arn).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "NotFoundException"),
                "case {label:?} ({form}): expected 404 NotFoundException, got {status} {body}",
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(NOT_FOUND_MESSAGE),
                "case {label:?} ({form}): expected this service's fixed not-found sentence, \
                 got {body}",
            );
        }

        // Reading tags mints nothing: the group still reports the identity it was
        // created with, so no read gave it one it did not have.
        let read = emulator
            .ok_in(case.region, "/get-group", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            read["Group"], created["Group"],
            "case {label:?}: expected the group to still be {}, got {}",
            created["Group"], read["Group"],
        );
    }
}

/// One write against a group's tag map, and how it changes the map.
enum Mutation {
    /// `Tag`: the pairs it carries are merged into the stored map.
    Merge(Vec<(&'static str, &'static str)>),
    /// `Untag`: the keys it names are removed from the stored map.
    Remove(Vec<&'static str>),
}

/// Writes applied one after another to ONE group, differing along what each kind
/// of write generalises over: how many keys it names (none, one, several),
/// whether a key it names is already stored, and the spelling of the pair.
fn mutations() -> Vec<(&'static str, Mutation)> {
    vec![
        (
            "merging several new pairs",
            Mutation::Merge(vec![("Team", "Platform"), ("Owner", "sre")]),
        ),
        (
            "merging a pair over a stored key",
            Mutation::Merge(vec![("Stage", "Prod")]),
        ),
        ("merging no pairs at all", Mutation::Merge(Vec::new())),
        ("removing a stored key", Mutation::Remove(vec!["Team"])),
        (
            "removing a key the group does not carry",
            Mutation::Remove(vec!["never-stored"]),
        ),
        ("removing nothing", Mutation::Remove(Vec::new())),
        (
            "merging a pair written in non-ASCII letters",
            Mutation::Merge(vec![("Équipe", "Ωμέγα 42")]),
        ),
        (
            "removing several stored keys at once",
            Mutation::Remove(vec!["Stage", "Owner"]),
        ),
    ]
}

/// The map `GetTags` reports is the one the store holds, so it follows every
/// mutator that wrote it — and once the group is deleted, its ARN addresses no
/// group at all.
#[tokio::test]
async fn get_tags_reports_the_map_each_mutator_left() {
    for region in ["us-east-1", "eu-central-1"] {
        let emulator = Emulator::new();
        let name = "mutated";
        let created = emulator
            .ok_in(
                region,
                "/groups",
                &json!({ "Name": name, "Tags": { "Stage": "Test" } }),
            )
            .await;
        let arn = group_arn(region, name);
        assert_eq!(
            created["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {region:?}: expected the created group's ARN to be {arn}, got {created}",
        );
        // The map the create itself stored is the one the first read reports.
        let mut expected: BTreeMap<String, String> =
            [("Stage".to_string(), "Test".to_string())].into_iter().collect();

        for (case, mutation) in mutations() {
            match &mutation {
                Mutation::Merge(pairs) => {
                    let batch: Vec<(String, String)> = pairs
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect();
                    let (status, answered) =
                        tag(&emulator, region, &arn, &tag_json(&batch)).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?} in {region}: expected Tag to answer 200, \
                         got {status} {answered}",
                    );
                    expected.extend(batch);
                }
                Mutation::Remove(keys) => {
                    let batch: Vec<String> = keys.iter().map(|key| key.to_string()).collect();
                    let (status, answered) = untag(&emulator, region, &arn, &batch).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?} in {region}: expected Untag to answer 200, \
                         got {status} {answered}",
                    );
                    for key in &batch {
                        expected.remove(key);
                    }
                }
            }
            let (status, body) = get_tags(&emulator, region, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} in {region}: expected GetTags to answer 200, got {status} {body}",
            );
            assert_eq!(
                tag_pairs(case, &body["Tags"]),
                expected,
                "case {case:?} in {region}: expected the stored map {expected:?}, got {body}",
            );
            assert_eq!(
                body["Arn"].as_str(),
                Some(arn.as_str()),
                "case {case:?} in {region}: expected the stored ARN {arn}, got {body}",
            );
        }

        // Deleting the group takes its map with it: the same ARN now addresses no
        // group.
        emulator
            .ok_in(region, "/delete-group", &json!({ "Group": name }))
            .await;
        let (status, body) = get_tags(&emulator, region, &arn).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "NotFoundException"),
            "case {region:?}: expected a deleted group's ARN to answer 404 NotFoundException, \
             got {status} {body}",
        );

        // A group created again under the same name carries only the map ITS
        // create stored, so the answer comes from the store and not from anything
        // the earlier group left behind.
        emulator.ok_in(region, "/groups", &json!({ "Name": name })).await;
        let (status, body) = get_tags(&emulator, region, &arn).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the recreated group to answer 200, got {status} {body}",
        );
        assert_eq!(
            tag_pairs("recreated", &body["Tags"]),
            BTreeMap::new(),
            "case {region:?}: expected the recreated group to carry no tags, got {body}",
        );
    }
}
