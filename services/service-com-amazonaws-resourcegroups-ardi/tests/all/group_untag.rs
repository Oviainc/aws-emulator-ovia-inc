// SPDX-License-Identifier: CC0-1.0

//! `Untag`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suites' `Emulator`), so these exercise
//! the whole restJson1 path: routing, the URI-bound `Arn` label, boundary
//! validation, the handler, and serialization. `Untag` is `PATCH
//! /resources/{Arn}/tags` (`smithy_query(query="operation",operation="Untag")`), so
//! every call below is sent on that method and path.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * `Untag` DELETES from the addressed group's stored tag map every key its `Keys`
//!   list names, together with the value that key carried, and leaves a key it does
//!   not name at the value it had;
//! * it reports the group's stored ARN beside the keys whose tag the call actually
//!   removed, so a key the group does not carry removes nothing and is reported in
//!   nothing — which makes repeating the call idempotent;
//! * a group is addressed by its own stored ARN, so an ARN for another region or
//!   another account addresses no group and is `NotFoundException` (404);
//! * a key a caller may not change — in the reserved key namespace, or outside the
//!   modelled key length or character set — is `BadRequestException` (400) and
//!   removes NONE of its batch;
//! * the removal is visible to every later reader (`GetTags`), and removes nothing
//!   else about the group.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{
    MAX_TAG_KEY_LENGTH, RESERVED_TAG_KEY_PREFIX,
};

use super::group_tags::{filler, group_arn, pct, stored_tags, tag, tag_json};
use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// `PATCH /resources/{Arn}/tags` with `keys` as the request's `Keys` list, signed
/// (in scope only) for `region`.
async fn untag(emulator: &Emulator, region: &str, arn: &str, keys: &[String]) -> (u16, Value) {
    emulator
        .request_in(
            region,
            "PATCH",
            &format!("/resources/{}/tags", pct(arn)),
            Some(&json!({ "Keys": keys })),
        )
        .await
}

/// The `Keys` list of a response, in the order it was reported.
fn answered_keys(case: &str, body: &Value) -> Vec<String> {
    body["Keys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Keys list, got {body}"))
        .iter()
        .map(|key| {
            key.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string key, got {key}"))
                .to_string()
        })
        .collect()
}

/// The tag map every removal case below starts from, spanning what a tag key can
/// be: the modelled `TagKey` length bounds (`length 1..128`), the punctuation and
/// the Unicode letters its pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits, a key
/// that merely contains the reserved prefix later on, and a pair whose value is
/// empty (`TagValue`'s `length 0..256`)
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
fn starting_tags() -> Vec<(String, String)> {
    vec![
        ("Stage".to_string(), "Test".to_string()),
        ("Team".to_string(), "Platform".to_string()),
        ("k".to_string(), "at-the-key-minimum".to_string()),
        ("Empty".to_string(), String::new()),
        (
            filler(MAX_TAG_KEY_LENGTH, 1),
            "at-the-key-maximum".to_string(),
        ),
        (
            "ns:key_part.one/two=three+four-five@six".to_string(),
            "value with spaces and _.:/=+-@".to_string(),
        ),
        ("Équipe".to_string(), "Ωμέγα 42".to_string()),
        ("team-aws:billing".to_string(), "kept".to_string()),
    ]
}

/// Removals applied one after another to ONE group, differing along what the
/// removal rule generalises over: how many keys a list carries (none, one,
/// several), whether a key is stored or not, whether the list repeats a key, and
/// the spelling of the key.
///
/// Each row is `(case, keys sent, keys the call must report as removed)`.
fn removal_rows() -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    vec![
        (
            "one stored key",
            vec!["Stage".to_string()],
            vec!["Stage".to_string()],
        ),
        (
            "several stored keys at once",
            vec!["Team".to_string(), "Empty".to_string()],
            vec!["Team".to_string(), "Empty".to_string()],
        ),
        (
            "a key the group does not carry",
            vec!["never-stored".to_string()],
            Vec::new(),
        ),
        (
            "a stored key beside one the group does not carry",
            vec!["Équipe".to_string(), "never-stored".to_string()],
            vec!["Équipe".to_string()],
        ),
        ("no keys at all", Vec::new(), Vec::new()),
        (
            "the same stored key twice in one list",
            vec![
                "team-aws:billing".to_string(),
                "team-aws:billing".to_string(),
            ],
            vec!["team-aws:billing".to_string()],
        ),
        (
            "a key at the modelled one-character minimum",
            vec!["k".to_string()],
            vec!["k".to_string()],
        ),
        (
            "a key at the modelled 128-character maximum",
            vec![filler(MAX_TAG_KEY_LENGTH, 1)],
            vec![filler(MAX_TAG_KEY_LENGTH, 1)],
        ),
        (
            "every punctuation character the modelled pattern admits",
            vec!["ns:key_part.one/two=three+four-five@six".to_string()],
            vec!["ns:key_part.one/two=three+four-five@six".to_string()],
        ),
    ]
}

/// Untagging deletes the keys it names from the stored map, reports the keys it
/// removed, leaves every other key alone, and reads back through `GetTags` — in
/// whichever region the group was created in, and however many times the same list
/// is repeated.
#[tokio::test]
async fn untagging_removes_the_keys_it_names_and_reports_what_it_removed() {
    for (region, name) in [
        ("us-east-1", "untagged-east"),
        ("eu-west-1", "untagged.west_1"),
    ] {
        let emulator = Emulator::new();
        let (created_status, created) = emulator
            .call_in(region, "/groups", &json!({ "Name": name }))
            .await;
        assert_eq!(
            created_status, 200,
            "case {name:?}: CreateGroup expected 200, got {created_status} {created}",
        );
        let arn = group_arn(region, name);

        // The map every removal below starts from, written through `Tag` so the
        // store, not this test, decides what is stored.
        let start = starting_tags();
        let (tagged_status, tagged) = tag(&emulator, region, &arn, &tag_json(&start)).await;
        assert_eq!(
            tagged_status, 200,
            "case {name:?}: Tag expected 200, got {tagged_status} {tagged}",
        );
        let mut expected: BTreeMap<String, String> = start.iter().cloned().collect();
        let stored = stored_tags(&emulator, region, name, &arn).await;
        assert_eq!(
            stored, expected,
            "case {name:?}: expected the starting map {expected:?}, got {stored:?}",
        );

        for (case, keys, removed) in removal_rows() {
            let (status, answered) = untag(&emulator, region, &arn, &keys).await;
            assert_eq!(
                status, 200,
                "case {case:?}: Untag expected 200, got {status} {answered}",
            );
            assert_eq!(
                answered["Arn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected Untag to report the stored ARN {arn}, got {answered}",
            );
            assert_eq!(
                answered_keys(case, &answered),
                removed,
                "case {case:?}: expected Untag to report the keys it removed {removed:?}, \
                 got {answered}",
            );

            for key in &removed {
                expected.remove(key);
            }
            let stored = stored_tags(&emulator, region, case, &arn).await;
            assert_eq!(
                stored, expected,
                "case {case:?}: expected the stored map {expected:?}, got {stored:?}",
            );

            // The same list again removes nothing and reports nothing: the keys it
            // named are already gone.
            let (repeat_status, repeated) = untag(&emulator, region, &arn, &keys).await;
            assert_eq!(
                repeat_status, 200,
                "case {case:?}: repeating the list expected 200, got {repeat_status} {repeated}",
            );
            assert_eq!(
                answered_keys(case, &repeated),
                Vec::<String>::new(),
                "case {case:?}: repeating the list must remove nothing, got {repeated}",
            );
            let after = stored_tags(&emulator, region, case, &arn).await;
            assert_eq!(
                after, expected,
                "case {case:?}: repeating the list must leave the map at {expected:?}, \
                 got {after:?}",
            );
        }

        // Every row together names every stored key, so the group now carries no
        // tags at all — and it is still the same group.
        assert!(
            expected.is_empty(),
            "case {name:?}: the rows must between them remove every stored key, \
             {expected:?} were left",
        );
        let (group_status, group) = emulator
            .call_in(region, "/get-group", &json!({ "Group": name }))
            .await;
        assert_eq!(
            group_status, 200,
            "case {name:?}: GetGroup expected 200, got {group_status} {group}",
        );
        assert_eq!(
            group["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: untagging must leave the group itself alone, got {group}",
        );
    }
}

/// Keys this service will not remove, each refused with HTTP 400
/// `BadRequestException`: in the reserved key namespace (which a caller can neither
/// write nor remove), or outside the modelled `TagKey` length or character set.
///
/// The rows differ along WHY the key is refused, and the length rows sit one
/// character past the bound the accepted row in `removal_rows` sits on.
fn refused_key_lists() -> Vec<(String, Vec<String>)> {
    vec![
        (
            "a key in the reserved namespace".to_string(),
            vec![format!("{RESERVED_TAG_KEY_PREFIX}cloudformation")],
        ),
        (
            "the reserved prefix in upper case".to_string(),
            vec![format!(
                "{}billing",
                RESERVED_TAG_KEY_PREFIX.to_uppercase()
            )],
        ),
        (
            "a reserved key beside a stored one".to_string(),
            vec![
                "Stage".to_string(),
                format!("{RESERVED_TAG_KEY_PREFIX}cloudformation"),
            ],
        ),
        (
            format!(
                "a {}-character key, one past the modelled maximum",
                MAX_TAG_KEY_LENGTH + 1
            ),
            vec![filler(MAX_TAG_KEY_LENGTH + 1, 3)],
        ),
        (
            "an empty key, which names no tag".to_string(),
            vec![String::new()],
        ),
        (
            "a key carrying a character outside the modelled set".to_string(),
            vec!["star*key".to_string()],
        ),
        (
            "a key carrying a control character, which is no separator".to_string(),
            vec!["tab\tkey".to_string()],
        ),
    ]
}

/// A list holding a key this service refuses is HTTP 400 `BadRequestException`, and
/// removes none of its keys.
#[tokio::test]
async fn a_key_this_service_refuses_removes_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "refused-removals", "Tags": { "Stage": "Test", "Team": "Platform" } }),
        )
        .await;
    let arn = group_arn("us-east-1", "refused-removals");
    let expected: BTreeMap<String, String> = [
        ("Stage".to_string(), "Test".to_string()),
        ("Team".to_string(), "Platform".to_string()),
    ]
    .into_iter()
    .collect();

    for (case, keys) in refused_key_lists() {
        let (status, body) = untag(&emulator, "us-east-1", &arn, &keys).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected Untag to answer 400 BadRequestException, got {status} {body}",
        );
        let stored = stored_tags(&emulator, "us-east-1", &case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: a refused list must remove nothing, expected {expected:?}, \
             got {stored:?}",
        );
    }

    // An absent `Keys` is refused at the boundary: the model makes it `@required`.
    let (status, body) = emulator
        .request_in(
            "us-east-1",
            "PATCH",
            &format!("/resources/{}/tags", pct(&arn)),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "a request carrying no Keys must be refused, got {status} {body}",
    );
    let stored = stored_tags(&emulator, "us-east-1", "no Keys at all", &arn).await;
    assert_eq!(
        stored, expected,
        "a refused request must remove nothing, expected {expected:?}, got {stored:?}",
    );
}

/// An `Arn` that addresses no stored group is refused, and refused without touching
/// the group that IS stored: a 404 for a well-formed group ARN nothing answers to,
/// and a 400 for an identifier that is not a group ARN at all.
#[tokio::test]
async fn an_arn_that_addresses_no_stored_group_removes_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "present", "Tags": { "Stage": "Test" } }),
        )
        .await;
    let present = group_arn("us-east-1", "present");
    let expected: BTreeMap<String, String> = [("Stage".to_string(), "Test".to_string())]
        .into_iter()
        .collect();

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a group name nothing was created under",
            group_arn("us-east-1", "absent"),
            404,
            "NotFoundException",
        ),
        (
            "the stored group's name in another region",
            group_arn("eu-west-1", "present"),
            404,
            "NotFoundException",
        ),
        (
            "the stored group's name in another account",
            "arn:aws:resource-groups:us-east-1:111111111111:group/present".to_string(),
            404,
            "NotFoundException",
        ),
        (
            "a group ARN in another partition, whose account is this one",
            format!("arn:aws-us-gov:resource-groups:us-east-1:{ACCOUNT}:group/present"),
            404,
            "NotFoundException",
        ),
        (
            "an ARN of another service, which is not a group ARN",
            format!("arn:aws:ec2:us-east-1:{ACCOUNT}:group/present"),
            400,
            "BadRequestException",
        ),
        (
            "a bare group name, which is not an ARN",
            "present".to_string(),
            400,
            "BadRequestException",
        ),
    ];
    for (case, arn, status, code) in rows {
        let (answered_status, body) =
            untag(&emulator, "us-east-1", &arn, &["Stage".to_string()]).await;
        assert_eq!(
            (answered_status, error_code(&body)),
            (status, code),
            "case {case:?}: expected Untag to answer {status} {code}, \
             got {answered_status} {body}",
        );
        let stored = stored_tags(&emulator, "us-east-1", case, &present).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: the refusal must leave the stored group's tags at {expected:?}, \
             got {stored:?}",
        );
    }
}
