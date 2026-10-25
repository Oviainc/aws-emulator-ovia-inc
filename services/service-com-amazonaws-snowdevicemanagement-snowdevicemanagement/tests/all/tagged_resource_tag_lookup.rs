// SPDX-License-Identifier: CC0-1.0

//! The tagging operations resolve a `resourceArn` to a stored record through ONE rule,
//! whether they read the tags or write them.
//!
//! The rule: a tagging operation's `resourceArn` is parsed into a kind and an id
//! (`managed-device/<id>` or `task/<id>`), and that pair alone selects the stored record
//! whose tags the call reports or changes — so `ListTagsForResource`, `TagResource` and
//! `UntagResource` address exactly the same record for the same ARN, and answer the same
//! class when it addresses none. This suite drives that one path through BOTH write
//! operations and the read operation, so a write that resolved an ARN differently from a
//! read — or from the other write — fails here.
//!
//! Rows differ along what the rule generalises over: the resource KIND (device, task),
//! the ARN fields that are not part of the record's identity (its region, its
//! partition), and, for the refusal table, which field makes the ARN address no record
//! of this account (its syntax, its account, its id).
//!
//! Every case goes through the router, so nothing reaches into the state struct: a write
//! is read back through an implemented reporting operation (`ListTagsForResource`, plus
//! `DescribeDevice` / `DescribeTask`), and an answer that is not computed from persisted
//! state cannot pass.

use std::collections::BTreeMap;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    call, describe_device, describe_task, emulator, ids_of, list_tags, ok, queue_task, string,
    tag_resource, tags_uri, whole_inventory, Answer, ACCOUNT, SIGNED_REGION,
};

/// This service's ARN namespace / sigv4 signing name, from
/// `smithy_query(query="service_info")`.
const ARN_SERVICE: &str = "snow-device-management";

/// An account id that is not the one this emulator serves.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The modelled bound on `ManagedDeviceId` and `TaskId` (`@length(min: 1, max: 64)`),
/// spelled out here rather than imported so a wrong bound in the implementation cannot
/// make this suite pass.
const RESOURCE_ID_CEILING: usize = 64;

/// An ARN built field by field, so a row can vary exactly one field.
fn arn(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:{ARN_SERVICE}:{region}:{account}:{resource}")
}

/// `UntagResource` for one resource ARN: one `tagKeys` pair per key, which is how the
/// modelled `@httpQuery` list travels.
async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("{}?{query}", tags_uri(resource_arn));
    call(router, "DELETE", &uri, None).await
}

/// A JSON `TagMap` from pairs.
fn tag_map(pairs: &[(&str, String)]) -> Value {
    let map: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
        .collect();
    Value::Object(map)
}

/// The `tags` member of a report, as a comparable map. Required to be present: a record
/// carrying no tags has an EMPTY map of them, which is a different answer from the
/// member being absent.
fn reported_tags(body: &Value, case: &str) -> BTreeMap<String, String> {
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member, got {body}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a string tag value, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The bare name of the wire error code, whichever form the protocol renders it in.
/// Compared whole, because a client branches on the code and `ResourceNotFoundException`
/// ends in `NotFoundException`.
fn error_code_name(answer: &Answer) -> &str {
    let code = answer.error_code();
    code.rsplit(['#', '/']).next().unwrap_or(code)
}

/// Assert a tag write was accepted: HTTP 200 and an empty (`Unit`) payload.
fn assert_accepted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    let empty = answer.body.is_null()
        || answer
            .body
            .as_object()
            .is_some_and(|object| object.is_empty());
    assert!(
        empty,
        "case {case:?}: the tagging writes model no output, so the body must be empty, got {}",
        answer.body
    );
}

/// Which kind of record a row addresses, and how its tags are read back by the
/// operation that reports the record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Device,
    Task,
}

/// One row: an ARN spelling, the record it must resolve to, and how to read that record
/// back.
struct Addressed {
    case: &'static str,
    kind: Kind,
    /// The ARN the write and the read are addressed with.
    addressed: String,
    /// The ARN the emulator itself issued for the record, used for the read-back, so a
    /// row cannot pass by resolving its own spelling consistently but wrongly.
    issued: String,
    /// The id the record is stored under, for the reporting operation.
    id: String,
}

/// The record's tags as the operation that REPORTS the record (not the tag read) sees
/// them — a second, independent witness that the write landed on that record.
async fn described_tags(router: &MultiServiceRouter, row: &Addressed) -> BTreeMap<String, String> {
    let answer = match row.kind {
        Kind::Device => describe_device(router, &row.id).await,
        Kind::Task => describe_task(router, &row.id).await,
    };
    let body = ok(&answer, row.case);
    reported_tags(&body, row.case)
}

/// Both write paths and the read path resolve one ARN to one record, for every resource
/// kind and whatever the ARN's region or partition says.
#[tokio::test]
async fn the_tag_write_paths_resolve_a_resource_arn_the_way_the_tag_read_does() {
    let router = emulator();

    let inventory = whole_inventory(&router, "the activated inventory").await;
    let device_id = ids_of(&inventory, "the activated inventory")
        .first()
        .expect("the fixture activates devices")
        .clone();
    let device_arn = string(
        inventory
            .first()
            .expect("the fixture activates devices"),
        "managedDeviceArn",
        "the activated inventory",
    );
    let (task_id, task_arn, _) = queue_task(&router, &["reboot"], 1, None).await;

    let rows = [
        Addressed {
            case: "a device, by the ARN the emulator issued",
            kind: Kind::Device,
            addressed: device_arn.clone(),
            issued: device_arn.clone(),
            id: device_id.clone(),
        },
        Addressed {
            // A device id is unique across the account's Regions, so the ARN's region
            // field is not part of the record's identity.
            case: "a device, by an ARN naming another region of this account",
            kind: Kind::Device,
            addressed: arn(
                "aws",
                "eu-west-1",
                ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
            issued: device_arn.clone(),
            id: device_id.clone(),
        },
        Addressed {
            case: "a device, by an ARN naming another partition",
            kind: Kind::Device,
            addressed: arn(
                "aws-cn",
                SIGNED_REGION,
                ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
            issued: device_arn.clone(),
            id: device_id.clone(),
        },
        Addressed {
            case: "a task, by the ARN CreateTask returned",
            kind: Kind::Task,
            addressed: task_arn.clone(),
            issued: task_arn.clone(),
            id: task_id.clone(),
        },
        Addressed {
            case: "a task, by an ARN naming another region of this account",
            kind: Kind::Task,
            addressed: arn("aws", "us-east-1", ACCOUNT, &format!("task/{task_id}")),
            issued: task_arn.clone(),
            id: task_id.clone(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A value unique to the row, so a stale map from an earlier row cannot satisfy
        // the assertions.
        let owner = format!("fleet-ops-{index}");
        let written = tag_map(&[("owner", owner.clone()), ("env", "prod".to_string())]);
        let expected_after_write = BTreeMap::from([
            ("owner".to_string(), owner.clone()),
            ("env".to_string(), "prod".to_string()),
        ]);
        let expected_after_removal = BTreeMap::from([("owner".to_string(), owner.clone())]);

        assert_accepted(&tag_resource(&router, &row.addressed, written).await, case);

        let read = ok(&list_tags(&router, &row.issued).await, case);
        assert_eq!(
            reported_tags(&read, case),
            expected_after_write,
            "case {case:?}: a tag read of the issued ARN must report what the write \
             addressed through {:?}, got {read}",
            row.addressed
        );
        let described = described_tags(&router, row).await;
        assert_eq!(
            described, expected_after_write,
            "case {case:?}: the record's own report must carry the written tags, got \
             {described:?}"
        );

        assert_accepted(&untag_resource(&router, &row.addressed, &["env"]).await, case);

        let read = ok(&list_tags(&router, &row.issued).await, case);
        assert_eq!(
            reported_tags(&read, case),
            expected_after_removal,
            "case {case:?}: the removal must have taken env off the SAME record the \
             write landed on, got {read}"
        );
        let described = described_tags(&router, row).await;
        assert_eq!(
            described, expected_after_removal,
            "case {case:?}: the record's own report must have lost only env, got \
             {described:?}"
        );
    }

    // The two records are addressed independently: tagging a device through any of its
    // ARN spellings never touched the task, and the last row's value is what each
    // record kept.
    let case = "the device after every row";
    let device_tags = reported_tags(&ok(&list_tags(&router, &device_arn).await, case), case);
    assert_eq!(
        device_tags,
        BTreeMap::from([("owner".to_string(), "fleet-ops-2".to_string())]),
        "case {case:?}: the device must carry the last value written to IT, got {device_tags:?}"
    );
    let case = "the task after every row";
    let task_tags = reported_tags(&ok(&list_tags(&router, &task_arn).await, case), case);
    assert_eq!(
        task_tags,
        BTreeMap::from([("owner".to_string(), "fleet-ops-4".to_string())]),
        "case {case:?}: the task must carry the last value written to IT, got {task_tags:?}"
    );
}

/// A `resourceArn` that addresses no record of this account is refused the same way by
/// all three tagging operations — the read and both writes — and nothing is persisted.
#[tokio::test]
async fn the_tagging_operations_agree_on_a_resource_arn_that_addresses_no_record() {
    struct Row {
        case: &'static str,
        resource_arn: String,
        status: u16,
        code: &'static str,
    }

    let router = emulator();
    let inventory = whole_inventory(&router, "the activated inventory").await;
    let device_id = ids_of(&inventory, "the activated inventory")
        .first()
        .expect("the fixture activates devices")
        .clone();
    let device_arn = string(
        inventory
            .first()
            .expect("the fixture activates devices"),
        "managedDeviceArn",
        "the activated inventory",
    );

    let rows = [
        Row {
            case: "a value that is not an ARN at all",
            resource_arn: "managed-device/one".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            case: "an ARN of another service",
            resource_arn: format!("arn:aws:snowball:{SIGNED_REGION}:{ACCOUNT}:job/JID-1"),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            case: "an ARN whose account is not 12 digits",
            resource_arn: arn(
                "aws",
                SIGNED_REGION,
                "00000000000",
                &format!("managed-device/{device_id}"),
            ),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            case: "an ARN naming a resource type this service does not tag",
            resource_arn: arn("aws", SIGNED_REGION, ACCOUNT, &format!("device/{device_id}")),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            case: "an ARN carrying no resource id",
            resource_arn: arn("aws", SIGNED_REGION, ACCOUNT, "managed-device/"),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            // One past the modelled ceiling: no id this service can hold is that long.
            case: "an ARN whose id is one character past the modelled ceiling",
            resource_arn: arn(
                "aws",
                SIGNED_REGION,
                ACCOUNT,
                &format!("task/{}", "t".repeat(RESOURCE_ID_CEILING + 1)),
            ),
            status: 400,
            code: "BadRequestException",
        },
        Row {
            // AT the ceiling the ARN parses, so the answer is about the record, not the
            // syntax.
            case: "an ARN whose id is exactly at the modelled ceiling",
            resource_arn: arn(
                "aws",
                SIGNED_REGION,
                ACCOUNT,
                &format!("task/{}", "t".repeat(RESOURCE_ID_CEILING)),
            ),
            status: 404,
            code: "NotFoundException",
        },
        Row {
            case: "a well-formed device ARN of another account",
            resource_arn: arn(
                "aws",
                SIGNED_REGION,
                FOREIGN_ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
            status: 403,
            code: "AccessDeniedException",
        },
        Row {
            case: "a well-formed task ARN of another account",
            resource_arn: arn("aws", SIGNED_REGION, FOREIGN_ACCOUNT, "task/st-0000000000000001"),
            status: 403,
            code: "AccessDeniedException",
        },
        Row {
            case: "this account's ARN for a device id it does not hold",
            resource_arn: arn("aws", SIGNED_REGION, ACCOUNT, "managed-device/smd-ffffffffffffffff"),
            status: 404,
            code: "NotFoundException",
        },
        Row {
            case: "this account's ARN for a task id it does not hold",
            resource_arn: arn("aws", SIGNED_REGION, ACCOUNT, "task/st-ffffffffffffffff"),
            status: 404,
            code: "NotFoundException",
        },
    ];

    // A tagged record to compare against afterwards: a refused call must leave the
    // store exactly as it was.
    let seeded = tag_map(&[("owner", "fleet-ops".to_string())]);
    assert_accepted(
        &tag_resource(&router, &device_arn, seeded).await,
        "seeding the device",
    );

    for row in &rows {
        let case = row.case;
        for (operation, answer) in [
            (
                "ListTagsForResource",
                list_tags(&router, &row.resource_arn).await,
            ),
            (
                "TagResource",
                tag_resource(
                    &router,
                    &row.resource_arn,
                    tag_map(&[("owner", "fleet-ops".to_string())]),
                )
                .await,
            ),
            (
                "UntagResource",
                untag_resource(&router, &row.resource_arn, &["owner"]).await,
            ),
        ] {
            assert_eq!(
                answer.status, row.status,
                "case {case:?}: {operation} must answer HTTP {} like every other tagging \
                 operation, got {} with {}",
                row.status, answer.status, answer.body
            );
            assert_eq!(
                error_code_name(&answer),
                row.code,
                "case {case:?}: {operation} must answer the error code {} like every other \
                 tagging operation, got {}",
                row.code,
                answer.body
            );
        }
    }

    let case = "after the refusals";
    let kept = reported_tags(&ok(&list_tags(&router, &device_arn).await, case), case);
    assert_eq!(
        kept,
        BTreeMap::from([("owner".to_string(), "fleet-ops".to_string())]),
        "case {case:?}: a refused tagging call must persist nothing, got {kept:?}"
    );
    let described = reported_tags(&ok(&describe_device(&router, &device_id).await, case), case);
    assert_eq!(
        described, kept,
        "case {case:?}: the device's own report must agree with the tag read, got {described:?}"
    );
}
