// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over the awsJson1_1 wire.
//!
//! RULE: the `aws:` tag-key prefix is reserved for the tags Amazon Web Services generates
//! itself, so EVERY path that takes a caller-supplied tag key — `CreateExport`'s
//! `ResourceTags`, `TagResource`'s `ResourceTags` and `UntagResource`'s `ResourceTagKeys` —
//! refuses a key in that namespace with the modelled `ValidationException` (HTTP 400) and
//! leaves the stored tag set, and the stored exports, exactly as they were. The model
//! cannot express the rule (`ResourceTagKey` is `@length 1..128` with no `@pattern`:
//! `smithy_query(query="shape",shape="ResourceTagKey")`), so it is the handlers' and this
//! suite asserts it there.
//!
//! The prefix itself is read from the handler-side constant
//! (`state::RESERVED_TAG_KEY_PREFIX`) rather than respelled here, so a row and the rule it
//! exercises cannot drift apart.
//!
//! Both halves of the rule are asserted over TABLES whose rows differ along what the rule
//! generalises over: what follows the prefix (nothing, one character, a real
//! service-managed key, non-ASCII text, the published `@length` ceiling) and how the
//! prefix is cased — against keys that merely RESEMBLE the namespace (the prefix without
//! its colon, the prefix away from the start, a multi-byte character where the prefix
//! would end, the prefix in a VALUE), which are ordinary tags and must round-trip.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::RESERVED_TAG_KEY_PREFIX;

use crate::export_lifecycle::{cases, error_code, Client};
use crate::tagging::{listed_tags, untagged_export};

/// The modelled `@length` ceiling of a tag key (smithy_query shape `ResourceTagKey`:
/// `@length(min: 1, max: 128)`).
const MAX_TAG_KEY_CHARS: usize = 128;

/// An ordinary tag the refused calls must leave untouched.
const SURVIVOR: (&str, &str) = ("team", "billing");

/// Keys INSIDE the reserved namespace, as `(case label, key)`.
///
/// Rows differ along what follows the prefix and how the prefix is cased; the prefix is
/// read from the handler's own constant, never respelled.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    // The ceiling case: the prefix plus as many characters as the key's modelled bound
    // still admits, so the longest reserved key the boundary validator lets through is
    // judged by the handler too.
    let ceiling = format!(
        "{prefix}{}",
        "k".repeat(MAX_TAG_KEY_CHARS - prefix.chars().count())
    );
    vec![
        ("the prefix alone".to_string(), prefix.to_string()),
        (
            "the prefix and a single character".to_string(),
            format!("{prefix}x"),
        ),
        (
            "a key the service-managed namespace really holds".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "a second service-managed key".to_string(),
            format!("{prefix}created-by"),
        ),
        (
            "the prefix with a non-ASCII tail".to_string(),
            format!("{prefix}Проект"),
        ),
        (
            "the prefix upper-cased".to_string(),
            format!("{}owner", prefix.to_uppercase()),
        ),
        (
            "the prefix mixed-cased".to_string(),
            format!("Aws{}Owner", &prefix[prefix.len() - 1..]),
        ),
        (
            "the prefix at the key's published ceiling".to_string(),
            ceiling,
        ),
    ]
}

/// Keys that only RESEMBLE the reserved namespace, as `(case label, key)`: ordinary tags
/// the service must store and report.
fn lookalike_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let bare = prefix.trim_end_matches(':').to_string();
    vec![
        ("the prefix without its colon".to_string(), bare.clone()),
        (
            "the prefix with another separator".to_string(),
            format!("{bare}-billing"),
        ),
        (
            "the prefix away from the start".to_string(),
            format!("team:{prefix}owner"),
        ),
        (
            // A multi-byte character begins where the prefix would end, so the byte the
            // prefix test compares at is not a character boundary.
            "a multi-byte character where the prefix ends".to_string(),
            format!("{bare}é:owner"),
        ),
        (
            "the shortest key the model admits".to_string(),
            "a".to_string(),
        ),
    ]
}

/// Assert `operation` refused `request` as the modelled 400, naming `member` as the field
/// at fault.
async fn refused(client: &Client, case: &str, operation: &str, member: &str, request: &Value) {
    let (status, body) = client.call(operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: {operation} must refuse a reserved tag key, got {status} {body}"
    );
    let names_member = body["Fields"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| field["Name"] == json!(member)));
    assert!(
        names_member,
        "case {case:?}: the refusal must name {member} among its Fields, got {body}"
    );
}

#[tokio::test]
async fn tag_resource_refuses_every_reserved_key() {
    let client = Client::new();
    let arn = untagged_export(&client, "reserved_tag_resource").await;
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": SURVIVOR.0, "Value": SURVIVOR.1 }],
            }),
        )
        .await;
    let before = listed_tags(&client, "before any refusal", &arn).await;

    for (case, key) in reserved_keys() {
        refused(
            &client,
            &case,
            "TagResource",
            "ResourceTags",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": key, "Value": "x" }],
            }),
        )
        .await;
        let after = listed_tags(&client, &case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused TagResource must store nothing, got {after:?}"
        );
        assert!(
            !after.contains_key(&key),
            "case {case:?}: the reserved key must not be readable, got {after:?}"
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_every_reserved_key() {
    let client = Client::new();
    let arn = untagged_export(&client, "reserved_untag_resource").await;
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": SURVIVOR.0, "Value": SURVIVOR.1 }],
            }),
        )
        .await;
    let before = listed_tags(&client, "before any refusal", &arn).await;

    for (case, key) in reserved_keys() {
        refused(
            &client,
            &case,
            "UntagResource",
            "ResourceTagKeys",
            &json!({ "ResourceArn": arn, "ResourceTagKeys": [key] }),
        )
        .await;
        let after = listed_tags(&client, &case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UntagResource must remove nothing, got {after:?}"
        );
    }

    // The same request with the reserved key beside an ordinary one removes neither: the
    // call is refused whole.
    refused(
        &client,
        "a reserved key beside the ordinary one",
        "UntagResource",
        "ResourceTagKeys",
        &json!({
            "ResourceArn": arn,
            "ResourceTagKeys": [SURVIVOR.0, format!("{RESERVED_TAG_KEY_PREFIX}billing")],
        }),
    )
    .await;
    let after = listed_tags(&client, "a reserved key beside the ordinary one", &arn).await;
    assert_eq!(
        after, before,
        "a refused UntagResource must leave every key in place, got {after:?}"
    );
}

/// How many exports the store holds.
async fn export_count(client: &Client) -> usize {
    let listed = client.ok("ListExports", &json!({})).await;
    listed["Exports"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The create request of the first lifecycle row, named `name` and carrying `tags`.
fn create_request(name: &str, tags: Value) -> Value {
    let mut case = cases().remove(0);
    case.set_name(name);
    let mut request = case.create_request();
    request["ResourceTags"] = tags;
    request
}

#[tokio::test]
async fn create_export_refuses_a_reserved_key_and_creates_nothing() {
    let client = Client::new();
    let before = export_count(&client).await;

    for (index, (case, key)) in reserved_keys().into_iter().enumerate() {
        let name = format!("reserved_create_{index}");
        refused(
            &client,
            &case,
            "CreateExport",
            "ResourceTags",
            &create_request(
                &name,
                json!([
                    { "Key": SURVIVOR.0, "Value": SURVIVOR.1 },
                    { "Key": key, "Value": "x" },
                ]),
            ),
        )
        .await;
        assert_eq!(
            export_count(&client).await,
            before,
            "case {case:?}: a refused CreateExport must create no export"
        );

        // The refused create left the NAME free as well as the store unchanged: the same
        // name is accepted once the reserved key is gone, and carries only the tag that
        // remained.
        let created = client
            .ok("CreateExport", &create_request(&name, json!([
                { "Key": SURVIVOR.0, "Value": SURVIVOR.1 },
            ])))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateExport returned no ExportArn: {created}"))
            .to_string();
        let listed = listed_tags(&client, &case, &arn).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the accepted create must carry only the ordinary tag, got {listed:?}"
        );
        client
            .ok("DeleteExport", &json!({ "ExportArn": arn }))
            .await;
    }
}

#[tokio::test]
async fn keys_that_only_resemble_the_reserved_prefix_are_ordinary_tags() {
    let client = Client::new();
    let arn = untagged_export(&client, "reserved_lookalikes").await;

    for (case, key) in lookalike_keys() {
        client
            .ok(
                "TagResource",
                &json!({
                    "ResourceArn": arn,
                    "ResourceTags": [{ "Key": key, "Value": "kept" }],
                }),
            )
            .await;
        let listed = listed_tags(&client, &case, &arn).await;
        assert_eq!(
            listed.get(&key).map(String::as_str),
            Some("kept"),
            "case {case:?}: an ordinary key must read back, got {listed:?}"
        );

        client
            .ok(
                "UntagResource",
                &json!({ "ResourceArn": arn, "ResourceTagKeys": [key.clone()] }),
            )
            .await;
        let listed = listed_tags(&client, &case, &arn).await;
        assert!(
            !listed.contains_key(&key),
            "case {case:?}: an ordinary key must be removable, got {listed:?}"
        );
    }

    // The rule is about KEYS: a value spelling the prefix is an ordinary value.
    let value = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation");
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": "origin", "Value": value }],
            }),
        )
        .await;
    let listed = listed_tags(&client, "the prefix in a value", &arn).await;
    assert_eq!(
        listed.get("origin").map(String::as_str),
        Some(value.as_str()),
        "case \"the prefix in a value\": a value carrying the prefix must read back, got {listed:?}"
    );
}

#[tokio::test]
async fn a_request_mixing_a_reserved_key_with_an_ordinary_one_stores_neither() {
    let client = Client::new();
    let arn = untagged_export(&client, "reserved_mixed_request").await;

    refused(
        &client,
        "an ordinary key beside a reserved one",
        "TagResource",
        "ResourceTags",
        &json!({
            "ResourceArn": arn,
            "ResourceTags": [
                { "Key": "ordinary", "Value": "1" },
                { "Key": format!("{RESERVED_TAG_KEY_PREFIX}billing:owner"), "Value": "2" },
            ],
        }),
    )
    .await;
    let listed = listed_tags(&client, "an ordinary key beside a reserved one", &arn).await;
    assert!(
        listed.is_empty(),
        "a refused TagResource must store neither pair, got {listed:?}"
    );
}
