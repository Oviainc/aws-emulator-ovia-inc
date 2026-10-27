// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, on every path of this service that WRITES a
//! group's tags.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the `resource_group_lifecycle` suite's
//! `Emulator`), so these exercise the whole restJson1 path: routing, the URI-bound
//! `Arn` label, boundary validation, the handler, and serialization.
//!
//! The rule under test, asserted over a TABLE of keys rather than one row: a tag
//! key a caller supplies inside the reserved `aws:` namespace — the prefix
//! compared without regard to case, since re-casing it does not leave the
//! namespace — is `BadRequestException` (400) wherever tags are written, on
//! `CreateGroup`'s `Tags` member as on `Tag`, and the refused request stores
//! nothing at all; a key OUTSIDE that namespace is stored and reads back.
//!
//! The refused rows differ along what the rule generalises over: how much of the
//! key the prefix is, how the prefix is cased, how long the key is, and what the
//! rest of the key carries (ASCII, another colon, non-ASCII letters). The accepted
//! rows differ along how NEARLY reserved the key is: the prefix's letters without
//! its colon, the service's own application tag key, the prefix later in the key,
//! and a key whose fourth character is multi-byte, so the comparison is shown to be
//! made on characters rather than on bytes.

use std::collections::BTreeMap;

use serde_json::json;

use service_com_amazonaws_resourcegroups_ardi::state::{
    APPLICATION_TAG_KEY, MAX_TAG_KEY_LENGTH, RESERVED_TAG_KEY_PREFIX,
};

use super::group_tags::{filler, group_arn, stored_tags, tag, tag_json, tag_pairs};
use super::resource_group_lifecycle::{error_code, Emulator};

/// `prefix` with every other character upper-cased.
///
/// Tag keys are case-sensitive, but AWS reserves the prefix itself rather than one
/// spelling of it, so a key does not leave the reserved namespace by re-casing it.
fn re_cased(prefix: &str) -> String {
    prefix
        .chars()
        .enumerate()
        .map(|(index, c)| {
            if index % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// Tag keys inside the reserved namespace, each paired with the case it names.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    // The key length bound comes from the modelled `TagKey` (`length 1..128`),
    // `smithy_query(query="shape",shape="TagKey")`: the longest key there is still
    // a reserved one when it opens with the prefix.
    let longest = format!(
        "{prefix}{}",
        filler(MAX_TAG_KEY_LENGTH - prefix.chars().count(), 7),
    );
    vec![
        (
            "the bare prefix, which is the whole key".to_string(),
            prefix.to_string(),
        ),
        (
            "the prefix as AWS writes it, with an ordinary suffix".to_string(),
            format!("{prefix}createdBy"),
        ),
        (
            "the prefix in upper case".to_string(),
            format!("{}createdBy", prefix.to_uppercase()),
        ),
        (
            "the prefix re-cased, with a suffix carrying another colon".to_string(),
            format!("{}cloudformation:stack-name", re_cased(prefix)),
        ),
        (
            format!("a reserved key at the modelled {MAX_TAG_KEY_LENGTH}-character maximum"),
            longest,
        ),
        (
            "the prefix followed by non-ASCII letters".to_string(),
            format!("{prefix}créateur"),
        ),
    ]
}

/// Tag keys OUTSIDE the reserved namespace that come close to it, each paired with
/// the case it names and the value it is written with.
fn accepted_keys() -> Vec<(String, String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        (
            "the prefix's letters without the colon that closes it".to_string(),
            prefix.trim_end_matches(':').to_string(),
            "kept".to_string(),
        ),
        (
            // AWS's documented way to associate a resource with an application is to
            // apply this very key to it, so it is a key a CALLER writes; the modelled
            // `ApplicationTagKey` (`^awsApplication$`) only spells the key this
            // service REPORTS an application group's membership in.
            "the application tag key, which is no reserved prefix".to_string(),
            APPLICATION_TAG_KEY.to_string(),
            "kept".to_string(),
        ),
        (
            "the prefix later in the key rather than at its start".to_string(),
            format!("team-{prefix}billing"),
            "kept".to_string(),
        ),
        (
            "a key whose fourth character is multi-byte".to_string(),
            format!("aw\u{015f}{prefix}marker"),
            "kept".to_string(),
        ),
    ]
}

/// `CreateGroup` refuses a reserved tag key with 400 `BadRequestException`, and the
/// refused create creates NOTHING: the group it names is absent afterwards.
///
/// The batch carries a legal pair beside the reserved one, so the refusal is shown
/// to be atomic over the whole `Tags` map.
#[tokio::test]
async fn create_group_refuses_a_reserved_tag_key_and_creates_nothing() {
    let emulator = Emulator::new();
    for (index, (case, key)) in reserved_keys().into_iter().enumerate() {
        let name = format!("reserved-create-{index}");
        let tags = tag_json(&[
            ("Legal".to_string(), "yes".to_string()),
            (key.clone(), "system".to_string()),
        ]);
        let (status, body) = emulator
            .call("/groups", &json!({ "Name": name, "Tags": tags }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected CreateGroup to refuse the tag key {key:?} with \
             400 BadRequestException, got {status} {body}",
        );
        let (read_status, read_body) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            (read_status, error_code(&read_body)),
            (404, "NotFoundException"),
            "case {case:?}: a create refused for the tag key {key:?} must create nothing, \
             got {read_status} {read_body}",
        );
    }
}

/// `Tag` refuses a reserved tag key the same way, and the refused batch stores none
/// of its pairs: the addressed group's stored map is the one it already had.
#[tokio::test]
async fn tag_refuses_a_reserved_tag_key_and_stores_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "reserved-tag-target", "Tags": { "Stage": "Test" } }),
        )
        .await;
    let arn = group_arn("us-east-1", "reserved-tag-target");
    let expected: BTreeMap<String, String> = [("Stage".to_string(), "Test".to_string())]
        .into_iter()
        .collect();
    for (case, key) in reserved_keys() {
        let batch = tag_json(&[
            ("Added".to_string(), "no".to_string()),
            (key.clone(), "system".to_string()),
        ]);
        let (status, body) = tag(&emulator, "us-east-1", &arn, &batch).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected Tag to refuse the tag key {key:?} with \
             400 BadRequestException, got {status} {body}",
        );
        let stored = stored_tags(&emulator, "us-east-1", &case, &arn).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: a batch refused for the tag key {key:?} must store nothing, \
             expected {expected:?}, got {stored:?}",
        );
    }
}

/// A key OUTSIDE the reserved namespace is stored by both write paths and reads
/// back: `CreateGroup` reports and persists it, and `Tag` adds it to a stored
/// group.
#[tokio::test]
async fn a_key_outside_the_reserved_namespace_is_stored_by_both_write_paths() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "outside-tagged" })).await;
    let tagged_arn = group_arn("us-east-1", "outside-tagged");
    let mut merged: BTreeMap<String, String> = BTreeMap::new();

    for (index, (case, key, value)) in accepted_keys().into_iter().enumerate() {
        let name = format!("outside-create-{index}");
        let pairs = vec![(key.clone(), value.clone())];
        let created = emulator
            .ok(
                "/groups",
                &json!({ "Name": name, "Tags": tag_json(&pairs) }),
            )
            .await;
        let expected: BTreeMap<String, String> =
            [(key.clone(), value.clone())].into_iter().collect();
        let reported = tag_pairs(&case, &created["Tags"]);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected CreateGroup to report {expected:?}, got {reported:?}",
        );
        let stored = stored_tags(&emulator, "us-east-1", &case, &group_arn("us-east-1", &name)).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the created group to have stored {expected:?}, \
             got {stored:?}",
        );

        let (status, body) = tag(&emulator, "us-east-1", &tagged_arn, &tag_json(&pairs)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Tag to store the key {key:?}, got {status} {body}",
        );
        merged.insert(key, value);
        let stored = stored_tags(&emulator, "us-east-1", &case, &tagged_arn).await;
        assert_eq!(
            stored, merged,
            "case {case:?}: expected the tagged group to hold {merged:?}, got {stored:?}",
        );
    }
}
