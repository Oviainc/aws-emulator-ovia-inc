// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's tags READ BY ARN, end to end over the wire: `CreateStream` /
//! `AddTagsToStream` / `TagResource` / `CreateChannel` -> `ListTagsForResource`.
//!
//! RULE: `ListTagsForResource` REPORTS the tag set stored on the Kinesis resource its
//! `ResourceARN` names, inside the region the request was signed for, as the modelled `Tags`
//! list ordered by key — whichever of the resource kinds that ARN may name (a stream or a
//! channel) and whichever operation wrote the set, since each resource carries ONE stored
//! tag set that every tag write of it writes and every read of it reports. A resource
//! carrying no tags is reported as the empty list, the read is pure (two reads of one
//! resource agree and neither changes what a later caller sees), and an ARN this account and
//! region hold no resource under is `ResourceNotFoundException`.
//!
//! The recorded flow this was derived from tags ONE stream with ONE pair and reads it back
//! by ARN, which is one row of the table below. The rows differ along what the rule
//! generalises over: how many pairs the resource carries (none, one, several, the whole
//! documented ceiling), which operation wrote them, the form of a key and a value (ASCII,
//! non-Latin scripts, an empty value), the order the keys arrived in, and the kind of
//! resource the ARN names.
//!
//! The wire harness, the ARN builders and the tag helpers are [`crate::stream_tags`]' and
//! [`crate::channel_lifecycle`]', reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{channel_arn, MAX_STREAM_TAGS};

use crate::channel_lifecycle::{
    on_demand_stream, past_channel_activation, role_arn, storage_configuration,
    stream_configuration,
};
use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, reported_tags,
    stream_arn, tag_map, Emulator, REGION,
};

/// One row of the read table: what the stream was created with, what successive writes
/// assign to it (each through the operation named), and the tag set a read by ARN must then
/// report.
struct Row {
    label: &'static str,
    /// The `Tags` the create carries, empty on a row whose create names none.
    created_with: Vec<(String, String)>,
    /// One request each, served in order, as `(operation, the pairs it assigns)`.
    writes: Vec<(&'static str, Vec<(String, String)>)>,
    /// The tags the read must report, given UNSORTED so the key order a read promises is
    /// what the assertion checks.
    expected: Vec<(String, String)>,
}

/// The table the read rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: one pair assigned by ARN, read back by ARN.
        Row {
            label: "one-pair-assigned-by-arn",
            created_with: Vec::new(),
            writes: vec![("TagResource", vec![pair("Environment", "production")])],
            expected: vec![pair("Environment", "production")],
        },
        // A resource nothing ever tagged carries the EMPTY set, which is a served read.
        Row {
            label: "a-resource-carrying-no-tags",
            created_with: Vec::new(),
            writes: Vec::new(),
            expected: Vec::new(),
        },
        // What the CREATE carried is in the same stored set.
        Row {
            label: "what-the-create-carried",
            created_with: vec![pair("Team", "platform"), pair("Environment", "staging")],
            writes: Vec::new(),
            expected: vec![pair("Environment", "staging"), pair("Team", "platform")],
        },
        // ONE stored set, three ways into it: the create, the legacy stream API and the
        // generic resource API, with the last writer of a shared key winning.
        Row {
            label: "one-set-written-by-every-tag-api",
            created_with: vec![pair("by-create", "kept"), pair("shared", "written-first")],
            writes: vec![
                (
                    "AddTagsToStream",
                    vec![pair("by-add-tags", "kept"), pair("shared", "written-second")],
                ),
                (
                    "TagResource",
                    vec![pair("by-tag-resource", "kept"), pair("shared", "written-last")],
                ),
            ],
            expected: vec![
                pair("by-add-tags", "kept"),
                pair("by-create", "kept"),
                pair("by-tag-resource", "kept"),
                pair("shared", "written-last"),
            ],
        },
        // Several pairs that arrived in an order which is not key order.
        Row {
            label: "three-pairs-out-of-key-order",
            created_with: Vec::new(),
            writes: vec![(
                "TagResource",
                vec![
                    pair("zeta", "last-by-key"),
                    pair("alpha", "first-by-key"),
                    pair("mu", "middle-by-key"),
                ],
            )],
            expected: vec![
                pair("alpha", "first-by-key"),
                pair("mu", "middle-by-key"),
                pair("zeta", "last-by-key"),
            ],
        },
        // "Tags may only contain Unicode letters, digits, white space, or these symbols:
        // _ . : / = + - @" (`TagResourceInput$Tags`): a key and a value outside the Latin
        // script round-trip through the stored set unchanged.
        Row {
            label: "unicode-keys-and-values",
            created_with: vec![pair("Ключ", "значение")],
            writes: vec![(
                "AddTagsToStream",
                vec![
                    pair("プロジェクト", "データ ストリーム"),
                    pair("Umlaut", "Grüße + Maß"),
                ],
            )],
            expected: vec![
                pair("Umlaut", "Grüße + Maß"),
                pair("Ключ", "значение"),
                pair("プロジェクト", "データ ストリーム"),
            ],
        },
        // "An optional string" with a modelled `@length` of 0..256 (`Tag$Value`): a pair
        // whose value is empty is a pair, and is reported with its empty value.
        Row {
            label: "a-pair-whose-value-is-empty",
            created_with: Vec::new(),
            writes: vec![(
                "TagResource",
                vec![pair("empty-value", ""), pair("filled-value", "v")],
            )],
            expected: vec![pair("empty-value", ""), pair("filled-value", "v")],
        },
        // The whole documented ceiling of tags one resource may carry.
        Row {
            label: "the-whole-tag-ceiling",
            created_with: Vec::new(),
            writes: vec![("TagResource", pairs(MAX_STREAM_TAGS))],
            expected: pairs(MAX_STREAM_TAGS),
        },
    ]
}

/// A `ListTagsForResource` read of `arn`, which must be served.
async fn read_tags(emulator: &Emulator, case: &str, arn: &str) -> Value {
    emulator
        .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await
}

/// Every row's writes are served through the operation it names, and a read by ARN then
/// reports the stored set in key order — whatever wrote it.
#[tokio::test]
async fn the_stored_tag_set_is_reported_by_arn_in_key_order() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every stream is created first, so the rows share ONE wait for the transition the
    // creates armed rather than one each.
    for row in &rows {
        let mut body = json!({ "StreamName": row.label, "ShardCount": 1 });
        if !row.created_with.is_empty() {
            body["Tags"] = tag_map(&row.created_with);
        }
        emulator.ok(row.label, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let arn = stream_arn(REGION, row.label);
        for (operation, assignment) in &row.writes {
            let body = match *operation {
                "TagResource" => json!({ "ResourceARN": arn, "Tags": tag_map(assignment) }),
                _ => json!({ "StreamARN": arn, "Tags": tag_map(assignment) }),
            };
            emulator.ok(row.label, operation, &body).await;
        }

        let read = read_tags(&emulator, row.label, &arn).await;
        assert_eq!(
            reported_tags(row.label, &read),
            in_key_order(&row.expected),
            "case {}: expected the stored tag set in key order, got {read}",
            row.label
        );

        // The read is pure: a second one answers exactly the same set, and the legacy read
        // of the same stored set agrees with it.
        let again = read_tags(&emulator, row.label, &arn).await;
        assert_eq!(
            reported_tags(row.label, &again),
            in_key_order(&row.expected),
            "case {}: expected a second read to report the same set, got {again}",
            row.label
        );
        let legacy = emulator
            .ok(
                row.label,
                "ListTagsForStream",
                &json!({ "StreamARN": arn, "Limit": MAX_STREAM_TAGS }),
            )
            .await;
        assert_eq!(
            reported_tags(row.label, &legacy),
            in_key_order(&row.expected),
            "case {}: expected both reads of one stored set to agree, got {legacy}",
            row.label
        );
    }
}

/// A `ResourceARN` names a resource of either kind this service holds
/// (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`), so a CHANNEL's stored tag set
/// is reported by its ARN by the same rule — including the empty set of a channel created
/// with no tags.
#[tokio::test]
async fn a_channels_stored_tag_set_is_reported_by_its_arn() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "channel-tag-source").await;
    let create = |name: &str, tags: Option<Value>| {
        let mut body = json!({
            "ChannelName": name,
            "ServiceExecutionRoleARN": role_arn("channel-delivery"),
            "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("channel-tag-bucket"),
            },
        });
        if let Some(tags) = tags {
            body["Tags"] = tags;
        }
        body
    };

    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "tagged-channel",
            vec![pair("Owner", "analytics"), pair("Environment", "production")],
        ),
        ("untagged-channel", Vec::new()),
    ];

    for (case, tags) in &rows {
        let body = create(
            case,
            if tags.is_empty() {
                None
            } else {
                Some(tag_map(tags))
            },
        );
        emulator.ok(case, "CreateChannel", &body).await;

        // A channel's tag set is metadata of the channel, so it is reported while the
        // channel is still provisioning and once it is ready.
        let arn = channel_arn(REGION, case);
        let creating = read_tags(&emulator, case, &arn).await;
        assert_eq!(
            reported_tags(case, &creating),
            in_key_order(tags),
            "case {case}: expected the tags the create carried, got {creating}"
        );

        tokio::time::sleep(past_channel_activation()).await;
        let active = read_tags(&emulator, case, &arn).await;
        assert_eq!(
            reported_tags(case, &active),
            in_key_order(tags),
            "case {case}: expected the same set once the channel is ready, got {active}"
        );
    }
}

/// An ARN this account and region hold no resource under names nothing to read, whatever
/// form it takes: a stream nothing created, a stream of another region, a channel nothing
/// created, and a channel of another region. The refusal names the resource kind the ARN
/// names.
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let held = "list-tags-for-resource-refusals";
    emulator
        .ok(
            held,
            "CreateStream",
            &json!({ "StreamName": held, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let elsewhere = "eu-west-1";
    let account = engine::EMULATOR_ACCOUNT_ID;
    // (case, the ARN the read names, what the message must say — spelled out here with only
    // the resource the request named and the account interpolated, so a drift away from the
    // wording fails rather than comparing the handler against the helper it builds the
    // message with. Either resource kind is named the way a client addresses it — by its
    // NAME, which its ARN carries as the resource — even where the request named it by ARN.)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a stream nothing created",
            stream_arn(REGION, "never-created"),
            format!("Stream never-created under account {account} not found."),
        ),
        (
            "a stream of another region",
            stream_arn(elsewhere, held),
            format!("Stream {held} under account {account} not found."),
        ),
        (
            "a channel nothing created",
            channel_arn(REGION, "never-created"),
            format!("Channel never-created under account {account} not found."),
        ),
        (
            "a channel of another region",
            channel_arn(elsewhere, "never-created"),
            format!("Channel never-created under account {account} not found."),
        ),
    ];

    for (case, arn, says) in &rows {
        let (status, response) = emulator
            .call("ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            says,
            "case {case}: expected the not-found sentence this service states for that \
             resource kind, got {response}"
        );
    }

    // The stream this account does hold is still readable, and still carries nothing.
    let read = read_tags(&emulator, "after the refusals", &stream_arn(REGION, held)).await;
    assert!(
        reported_tags("after the refusals", &read).is_empty(),
        "case after the refusals: a refused read stores nothing, got {read}"
    );
}

/// A resource "is also scoped by Amazon Web Services Region": the read answers about the
/// resource of the region its request was signed for, so one ARN read from another region
/// names no resource at all.
#[tokio::test]
async fn a_read_answers_about_the_region_it_was_signed_for() {
    let emulator = Emulator::new();
    let name = "regional-tag-read";
    let elsewhere = "eu-west-1";
    let assigned = vec![pair("Region", elsewhere)];

    emulator
        .ok_in(
            name,
            elsewhere,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1, "Tags": tag_map(&assigned) }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(elsewhere, name);

    let read = emulator
        .ok_in(
            name,
            elsewhere,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
    assert_eq!(
        reported_tags(name, &read),
        in_key_order(&assigned),
        "case {name}: expected the tags of the stream {elsewhere} holds, got {read}"
    );

    let (status, response) = emulator
        .call_in(REGION, "ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await;
    assert_eq!(
        status, 400,
        "case {name}: expected a read signed for {REGION} to find no resource, got {status} \
         with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {name}: expected ResourceNotFoundException, got {response}"
    );
}
