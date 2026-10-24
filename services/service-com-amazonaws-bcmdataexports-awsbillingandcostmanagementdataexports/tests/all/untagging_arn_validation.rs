// SPDX-License-Identifier: CC0-1.0

//! The `ResourceArn` `UntagResource` accepts, over the awsJson1_1 wire.
//!
//! The rule under test: a tagging operation judges its `ResourceArn`'s ACCOUNT segment
//! BEFORE any existence lookup, so an ARN naming an account other than this one is refused
//! with the modelled `ValidationException` (HTTP 400) and never looked up, while an ARN of
//! this account IS looked up and answers either the served call or the modelled
//! `ResourceNotFoundException`. The two codes mean different things to a client:
//! not-found invites a create-then-retry, a validation failure says the ARN itself is
//! unusable. `tests/all/tagging_arn_validation.rs` asserts that rule across the three
//! tagging operations; this suite pins the part of it that no table of refusals can show —
//! that `UntagResource`'s refusal of a foreign ARN is reached WITHOUT consulting the
//! store, and removes nothing.
//!
//! It is asserted by answering the same request twice: once against a store that holds a
//! tagged export of this account and once against a store that holds nothing at all. A
//! decision taken before any lookup cannot tell the two apart, so every pair of answers
//! has to be identical — which is also what keeps a foreign caller from probing what this
//! account holds. The rows differ along what the rule generalises over: the account (two
//! foreign accounts, one of them a single digit away from this one), the partition, the
//! region, the resource segment (including, byte for byte, the resource segment of the
//! export the holding store really has), and the `ResourceTagKeys` list at the published
//! bounds of `ResourceTagKeyList` (`@length 0..200`) and `ResourceTagKey`
//! (`@length 1..128`) — including a key the held export really carries.

use std::collections::BTreeMap;

use serde_json::json;

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::RESOURCE_NOT_TAGGABLE_MESSAGE;

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// `ResourceTagKey`: `@length 1..128` (smithy_query shape `ResourceTagKey`).
const TAG_KEY_MAX_LEN: usize = 128;

/// The account every ARN this emulator mints names (`engine::emulator_account_id`).
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// An ARN of this service naming `account`, in `partition` and `region`, for `resource`.
fn arn(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:bcm-data-exports:{region}:{account}:{resource}")
}

/// The resource segment of a minted export ARN.
fn resource_segment_of(arn: &str) -> String {
    arn.splitn(6, ':')
        .nth(5)
        .expect("a minted ARN carries a resource segment")
        .to_string()
}

/// Foreign-account ARNs, all satisfying the modelled `Arn` constraints (`@length
/// 20..2048`, `@pattern
/// ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$` —
/// smithy_query shape `Arn`), so every answer comes from the handler and not from boundary
/// validation. `held_resource` is the resource segment of an export the holding store
/// really has, so one row names it under somebody else's account.
fn foreign_arns(held_resource: &str) -> Vec<(String, String)> {
    let mut rows = vec![
        (
            "the recorded cross-account ARN".to_string(),
            arn("aws", "us-east-1", "111122223333", "export/other_tenant"),
        ),
        (
            "a foreign account in another region".to_string(),
            arn("aws", "eu-west-3", "210987654321", "export/absent/0"),
        ),
        (
            "a foreign account in the gov partition".to_string(),
            arn("aws-us-gov", "us-gov-east-1", "444455556666", "export/absent"),
        ),
        (
            "a foreign account in the china partition".to_string(),
            arn("aws-cn", "cn-northwest-1", "777788889999", "a_single_segment"),
        ),
        (
            "a foreign account with an empty region".to_string(),
            arn("aws", "", "123456789012", "export/absent"),
        ),
        // The cross-tenant probe: the resource segment of an export the holding store
        // really has, under another account.
        (
            "another account naming an export this account holds".to_string(),
            arn("aws", "us-east-1", "999999999999", held_resource),
        ),
    ];
    // One account a single digit away from this emulator's own, so the comparison is on
    // the whole account and not on a prefix of it.
    let mut neighbour: Vec<char> = own_account().chars().collect();
    let last = neighbour.len() - 1;
    neighbour[last] = if neighbour[last] == '9' { '8' } else { '9' };
    rows.push((
        "an account one digit from this one".to_string(),
        arn(
            "aws",
            "us-east-1",
            &neighbour.iter().collect::<String>(),
            held_resource,
        ),
    ));
    rows
}

/// The `ResourceTagKeys` lists the rule has to hold for, at the published bounds of the
/// list and of one key, and including `held_key` — a key the holding store's export really
/// carries, so a refusal cannot be mistaken for a removal that found nothing to do.
fn key_lists(held_key: &str) -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("no keys at all", Vec::new()),
        ("one key", vec!["env".to_string()]),
        (
            "several keys",
            (0..4).map(|index| format!("probe-{index}")).collect(),
        ),
        (
            "a key at the published maximum length",
            vec!["k".repeat(TAG_KEY_MAX_LEN)],
        ),
        ("a key the held export carries", vec![held_key.to_string()]),
    ]
}

/// The tag set `ListTagsForResource` reports for `arn`, as a map.
async fn listed_tags(client: &Client, case: &str, arn: &str) -> BTreeMap<String, String> {
    let body = client
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTags list, got {body}"))
        .iter()
        .map(|entry| {
            let key = entry["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a Key: {entry}"))
                .to_string();
            let value = entry["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a Value: {entry}"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// An export of this account, tagged, on `client`: its ARN and the tag set it carries.
async fn tagged_export(client: &Client, name: &str) -> (String, BTreeMap<String, String>) {
    let mut case = cases().remove(0);
    case.set_name(name);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "owner", "Value": "finops" },
                ],
            }),
        )
        .await;
    let tags = listed_tags(client, name, &arn).await;
    assert!(
        tags.contains_key("env") && tags.contains_key("owner"),
        "case {name:?}: the setup must leave both seeded tags on the export, got {tags:?}"
    );
    (arn, tags)
}

#[tokio::test]
async fn untagging_a_foreign_arn_is_refused_without_consulting_the_store() {
    // One store that HOLDS a tagged export of this account, and one that holds nothing at
    // all: a decision taken before any lookup answers both the same.
    let holder = Client::new();
    let (held_arn, held_tags) = tagged_export(&holder, "untag_arn_validation_holder").await;
    let held_resource = resource_segment_of(&held_arn);
    let held_key = held_tags
        .keys()
        .next()
        .expect("the setup tagged the export")
        .clone();
    let empty = Client::new();

    for (arn_label, foreign) in foreign_arns(&held_resource) {
        for (keys_label, keys) in key_lists(&held_key) {
            let case = format!("{arn_label} / {keys_label}");
            let request = json!({ "ResourceArn": &foreign, "ResourceTagKeys": keys });

            let (status, body) = holder.call("UntagResource", &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException for {foreign}, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(RESOURCE_NOT_TAGGABLE_MESSAGE),
                "case {case:?}: expected the fixed tagging refusal, got {body}"
            );
            // The refusal reports nothing about the ARN: it was never looked up.
            assert!(
                !body["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&foreign),
                "case {case:?}: the refusal must not restate the ARN, got {body}"
            );

            let (empty_status, empty_body) = empty.call("UntagResource", &request).await;
            assert_eq!(
                (empty_status, &empty_body),
                (status, &body),
                "case {case:?}: the refusal must not depend on what the account holds, got \
                 {empty_status} with {empty_body} against {status} with {body}"
            );
        }
    }

    // Not one refusal removed a tag.
    let after = listed_tags(&holder, "after every refusal", &held_arn).await;
    assert_eq!(
        after, held_tags,
        "a refused UntagResource must leave the tag set untouched, got {after:?}"
    );
}

#[tokio::test]
async fn untagging_an_arn_of_this_account_is_looked_up() {
    let client = Client::new();
    let (held_arn, held_tags) = tagged_export(&client, "untag_arn_validation_lookup").await;

    // ARNs of this account that name nothing: looked up, and not found — including the
    // multi-segment, short and other-class resource forms the account check does not
    // judge.
    let absent = [
        ("never created", resource_segment_of(&held_arn) + "_absent"),
        ("a deeper resource path", "export/absent/00000000".to_string()),
        ("a single-character resource", "a".to_string()),
        (
            "another resource class",
            "table/COST_AND_USAGE_REPORT".to_string(),
        ),
    ];
    for (label, resource) in absent {
        let candidate = arn("aws", "us-east-1", own_account(), &resource);
        let (status, body) = client
            .call(
                "UntagResource",
                &json!({ "ResourceArn": &candidate, "ResourceTagKeys": ["env"] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {label:?}: an ARN of this account is looked up, so it must answer the \
             not-found refusal, got {status} with {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(candidate.as_str()),
            "case {label:?}: the refusal must report the ARN it could not find, got {body}"
        );
    }

    // The ARN of this account that names the export: served, and exactly the named key is
    // gone when the export is read back.
    client
        .ok(
            "UntagResource",
            &json!({ "ResourceArn": &held_arn, "ResourceTagKeys": ["env"] }),
        )
        .await;
    let mut expected = held_tags.clone();
    expected.remove("env");
    let after = listed_tags(&client, "the served untag", &held_arn).await;
    assert_eq!(
        after, expected,
        "the served call must remove exactly the key it named, got {after:?}"
    );

    // An empty key list on that same export is served too, and removes nothing: the list's
    // published lower bound is a request, not a refusal.
    client
        .ok(
            "UntagResource",
            &json!({ "ResourceArn": &held_arn, "ResourceTagKeys": [] }),
        )
        .await;
    let after_empty = listed_tags(&client, "an empty key list", &held_arn).await;
    assert_eq!(
        after_empty, expected,
        "an empty ResourceTagKeys list must leave the tag set untouched, got {after_empty:?}"
    );
}
