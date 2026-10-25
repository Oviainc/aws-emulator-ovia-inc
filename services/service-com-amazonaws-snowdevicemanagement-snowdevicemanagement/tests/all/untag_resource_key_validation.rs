// SPDX-License-Identifier: CC0-1.0

//! The tag-KEY rules on the REMOVAL path, end to end over HTTP.
//!
//! The rule under test: a tag key AWS does not let a request name is refused wherever a
//! request names it, so `UntagResource` holds every `tagKeys` entry to the same
//! AWS-wide key rules the tag-writing operations apply — an empty key, a key beginning
//! with the reserved `aws:` prefix (case-insensitively; "you can't edit or delete tag
//! names or values with this prefix") and a key past the published key-length ceiling
//! are refused HTTP 400 `ValidationException`, and the addressed record keeps every tag
//! it carried. The mirror of the rule is just as much part of it: a key the rules accept
//! is removed, so the refusals are about the key and not about the operation.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over — the key
//! form, its length at and past the published ceiling, the key's case, how many keys the
//! request names and which of them is unacceptable, and the kind of resource addressed —
//! never the one recorded request. Every row goes through the router and is read back
//! through `ListTagsForResource` and `DescribeTask` / `DescribeDevice`, so an answer that
//! is not computed from persisted state cannot pass.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    assert_validation, call, create_task, describe_device, describe_task, emulator, list_tags, ok,
    string, tag_resource, tags_uri, whole_inventory, Answer,
};

/// The longest tag key AWS accepts ("Maximum key length: 128 Unicode characters in
/// UTF-8"); the service's model constrains `TagKeys`' member with no length at all, per
/// `smithy_query(query="shape",shape="TagKeys")`. Spelled out here rather than imported,
/// so a wrong bound in the implementation cannot make this suite pass.
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The tag-key prefix AWS reserves for itself, which no request may name.
const RESERVED_PREFIX: &str = "aws:";

/// The tags every row's record carries before the removal, so a refusal has something to
/// leave intact and an acceptance something to remove.
const SEEDED_TAGS: &[(&str, &str)] = &[("Env", "prod"), ("Name", "nightly"), ("Owner", "ops")];

/// Percent-encode one URI component, keeping only the RFC 3986 unreserved set, so a tag
/// key carrying `:`, `/`, ` ` or `+` survives the trip to the server's query decoder.
fn encode(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for byte in component.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(*byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — one `tagKeys` pair per key, which is how the
/// modelled `@httpQuery` list travels.
async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("{}?{query}", tags_uri(resource_arn));
    call(router, "DELETE", &uri, None).await
}

/// The `tags` member of a report, as a comparable map. A record carrying no tags reports
/// an EMPTY map or omits the member; both read as "no tags".
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let Some(tags) = body.get("tags") else {
        return BTreeMap::new();
    };
    if tags.is_null() {
        return BTreeMap::new();
    }
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: tag {key:?} must be a string, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The seeded tags as a comparable map.
fn seeded_map() -> BTreeMap<String, String> {
    SEEDED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The seeded tags as a JSON map, for `CreateTask` / `TagResource`.
fn seeded_json() -> Value {
    SEEDED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// One activated device id, with the device ARN the emulator issued for it.
async fn a_device(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let devices = whole_inventory(router, case).await;
    let device = devices
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the emulator must activate a device"));
    (
        string(device, "managedDeviceId", case),
        string(device, "managedDeviceArn", case),
    )
}

/// A task carrying [`SEEDED_TAGS`]: `(taskId, taskArn)`.
async fn a_tagged_task(router: &MultiServiceRouter, device: &str, case: &str) -> (String, String) {
    let body = ok(
        &create_task(
            router,
            json!({
                "targets": [device],
                "command": { "reboot": {} },
                "tags": seeded_json(),
            }),
        )
        .await,
        case,
    );
    (
        string(&body, "taskId", case),
        string(&body, "taskArn", case),
    )
}

/// One row of the key table: the keys a removal names, and whether the rules accept them
/// all.
struct Row {
    case: &'static str,
    keys: Vec<String>,
    accepted: bool,
}

/// Every row. Built rather than `const`, because the length rows are generated from the
/// published ceiling above.
fn rows() -> Vec<Row> {
    let at_ceiling = "k".repeat(MAX_TAG_KEY_LENGTH);
    let past_ceiling = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    vec![
        Row {
            case: "one ordinary key",
            keys: vec!["Env".to_string()],
            accepted: true,
        },
        Row {
            case: "several ordinary keys",
            keys: vec!["Env".to_string(), "Owner".to_string()],
            accepted: true,
        },
        Row {
            case: "a key the record does not carry",
            keys: vec!["absent".to_string()],
            accepted: true,
        },
        // The reserved prefix includes its colon, and the rule is a PREFIX test, not a
        // substring one, so none of these is reserved.
        Row {
            case: "keys that only look reserved",
            keys: vec![
                "awsome".to_string(),
                "AWSNOTCOLON".to_string(),
                "team:aws:owner".to_string(),
            ],
            accepted: true,
        },
        Row {
            case: "a non-ASCII key",
            keys: vec!["propriétaire".to_string()],
            accepted: true,
        },
        Row {
            case: "a key at the length ceiling",
            keys: vec![at_ceiling],
            accepted: true,
        },
        Row {
            case: "a key one past the length ceiling",
            keys: vec![past_ceiling],
            accepted: false,
        },
        Row {
            case: "an empty key",
            keys: vec![String::new()],
            accepted: false,
        },
        Row {
            case: "the reserved prefix",
            keys: vec![format!("{RESERVED_PREFIX}cost-center")],
            accepted: false,
        },
        Row {
            case: "the reserved prefix, upper case",
            keys: vec![RESERVED_PREFIX.to_uppercase() + "created-by"],
            accepted: false,
        },
        Row {
            case: "the reserved prefix, mixed case",
            keys: vec!["Aws:cloudformation:stack-name".to_string()],
            accepted: false,
        },
        Row {
            case: "the reserved prefix alone",
            keys: vec![RESERVED_PREFIX.to_string()],
            accepted: false,
        },
        Row {
            case: "an unacceptable key after an acceptable one",
            keys: vec!["Env".to_string(), format!("{RESERVED_PREFIX}cost-center")],
            accepted: false,
        },
        Row {
            case: "an unacceptable key before an acceptable one",
            keys: vec![format!("{RESERVED_PREFIX}cost-center"), "Env".to_string()],
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn untag_resource_refuses_every_tag_key_the_rules_reject_and_removes_nothing() {
    for row in rows() {
        // A row runs against its own emulator, so one row's removal cannot change the
        // next row's answer.
        let router = emulator();
        let (device, _) = a_device(&router, row.case).await;
        let (task_id, task_arn) = a_tagged_task(&router, &device, row.case).await;

        let answer = untag_resource(&router, &task_arn, &row.keys).await;
        let expected: BTreeMap<String, String> = if row.accepted {
            seeded_map()
                .into_iter()
                .filter(|(key, _)| !row.keys.contains(key))
                .collect()
        } else {
            seeded_map()
        };

        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: an acceptable removal must be served, got {} with {}",
                row.case, answer.status, answer.body
            );
        } else {
            assert_validation(&answer, row.case);
        }

        // Read the remainder back through both readers of the record: a refused removal
        // leaves every tag in place, an accepted one removes exactly what it named.
        let described = ok(&describe_task(&router, &task_id).await, row.case);
        assert_eq!(
            tags_of(&described, row.case),
            expected,
            "case {:?}: removing {:?} must leave exactly {expected:?}, got {described}",
            row.case,
            row.keys
        );
        let read = ok(&list_tags(&router, &task_arn).await, row.case);
        assert_eq!(
            tags_of(&read, row.case),
            expected,
            "case {:?}: ListTagsForResource must agree, got {read}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_key_rules_hold_for_a_device_arn_too() {
    // The rule is about the KEY, so it does not depend on which kind of resource the
    // `resourceArn` addresses: the inventory record answers the same way the task record
    // does, and keeps the tags a refused removal named.
    let router = emulator();
    let case = "a device ARN";
    let (device_id, device_arn) = a_device(&router, case).await;
    ok(&tag_resource(&router, &device_arn, seeded_json()).await, case);

    for row in rows() {
        let answer = untag_resource(&router, &device_arn, &row.keys).await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: an acceptable removal must be served on a device too, got {} \
                 with {}",
                row.case, answer.status, answer.body
            );
            // Put back whatever this row removed, so the next row sees the same record.
            ok(
                &tag_resource(&router, &device_arn, seeded_json()).await,
                row.case,
            );
        } else {
            assert_validation(&answer, row.case);
            let described = ok(&describe_device(&router, &device_id).await, row.case);
            assert_eq!(
                tags_of(&described, row.case),
                seeded_map(),
                "case {:?}: a refused removal must leave the device's tags alone, got \
                 {described}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn one_key_gets_one_verdict_whether_it_is_written_or_removed() {
    // The rule is one rule, not one per operation: for every row, the write path and the
    // removal path agree on whether the key is acceptable, and a refusal on either is the
    // same HTTP 400 `ValidationException`.
    for row in rows() {
        let router = emulator();
        let (device, _) = a_device(&router, row.case).await;
        let (_, task_arn) = a_tagged_task(&router, &device, row.case).await;

        let written: Value = row
            .keys
            .iter()
            .map(|key| (key.clone(), json!("v")))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let write = tag_resource(&router, &task_arn, written).await;
        let removal = untag_resource(&router, &task_arn, &row.keys).await;

        assert_eq!(
            write.status, removal.status,
            "case {:?}: TagResource answered {} and UntagResource {} for the same keys \
             ({} / {})",
            row.case, write.status, removal.status, write.body, removal.body
        );
        if !row.accepted {
            assert_validation(&write, row.case);
            assert_validation(&removal, row.case);
        }
    }
}

#[tokio::test]
async fn the_resource_arn_is_resolved_before_the_keys_are_judged() {
    // Which refusal a request carrying BOTH an unresolvable `resourceArn` and an
    // unacceptable key gets is a property of the tagging surface as a whole, so the two
    // operations are asserted to agree rather than one of them being pinned alone: the
    // removal path answers whatever the write path answers for the same ARN.
    let router = emulator();
    let reserved = vec![format!("{RESERVED_PREFIX}cost-center")];
    let cases = [
        ("a resourceArn that is not an ARN", "not-an-arn".to_string()),
        (
            "a well-formed ARN naming no record",
            "arn:aws:snow-device-management:ap-southeast-2:000000000000:task/st-ffffffffffffffff"
                .to_string(),
        ),
        (
            "a well-formed ARN of another account",
            "arn:aws:snow-device-management:ap-southeast-2:210987654321:task/st-ffffffffffffffff"
                .to_string(),
        ),
    ];

    for (case, resource_arn) in &cases {
        let write = tag_resource(&router, resource_arn, json!({ "aws:cost-center": "42" })).await;
        let removal = untag_resource(&router, resource_arn, &reserved).await;
        assert_eq!(
            removal.status, write.status,
            "case {case:?}: TagResource answered {} and UntagResource {} for the same \
             resourceArn ({} / {})",
            write.status, removal.status, write.body, removal.body
        );
        assert_eq!(
            removal.error_code(),
            write.error_code(),
            "case {case:?}: the two tagging operations must answer the same error code, \
             got {} and {}",
            write.body,
            removal.body
        );
        assert_ne!(
            removal.status, 200,
            "case {case:?}: an unresolvable resourceArn must not be served, got {}",
            removal.body
        );
    }

    // The positive control: the same reserved key on a resolvable ARN is the validation
    // failure, so the answers above are about the ARN and not about the key.
    let case = "a resolvable ARN and a reserved key";
    let (device, _) = a_device(&router, case).await;
    let (_, task_arn) = a_tagged_task(&router, &device, case).await;
    assert_validation(&untag_resource(&router, &task_arn, &reserved).await, case);
}
