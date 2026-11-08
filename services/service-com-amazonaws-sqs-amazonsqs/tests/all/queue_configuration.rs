// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SQS queue-configuration slice: `CreateQueue`,
//! `GetQueueUrl`, `GetQueueAttributes` and `SetQueueAttributes`, driven over the wire
//! (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.
//!
//! Every assertion here is about state: what a request reports is what an earlier
//! request wrote. Nothing reaches into `crate::state` to read a queue — a create is
//! only believed once `GetQueueUrl`/`GetQueueAttributes` report it back.
//!
//! The tables span the classes the rules generalise over rather than one recorded
//! example: the attribute kinds (integer with documented bounds, boolean, documented
//! value set, JSON document, opaque text), the queue kinds (standard and FIFO), the
//! name forms and the name-length bound, and the attribute names a caller may not set.
//! The integer-bound rows are GENERATED from
//! [`service_com_amazonaws_sqs_amazonsqs::state::SETTABLE_ATTRIBUTES`], which carries
//! the bounds together with the documentation sentence each one comes from, so every
//! integer attribute the model has — not only the ones a test author remembered — is
//! exercised on both published bounds and one value past each.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    AttributeKind, DERIVED_ATTRIBUTES, MAX_QUEUE_NAME_LENGTH, MESSAGE_PLANE_ATTRIBUTES,
    SETTABLE_ATTRIBUTES,
};

// The wire harness is shared with the other suites in this crate.
use super::support::{
    account, all_attributes, as_map, call, create, create_ok, emulator, error_code, expected_arn,
    expected_url, ok, HOST,
};

/// A minimal but well-formed queue policy, used wherever a row needs a JSON document.
const QUEUE_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"allow-foreign","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"sqs:SendMessage","Resource":"*"}]}"#;

/// The documented default of every attribute that has one and that APPLIES to the kind of
/// queue under test: an attribute that "applies only to FIFO (first-in-first-out) queues"
/// has no default on a standard queue, which does not carry it at all.
fn documented_defaults(fifo: bool) -> BTreeMap<String, String> {
    SETTABLE_ATTRIBUTES
        .iter()
        .filter(|spec| fifo || !spec.fifo_only)
        .filter_map(|spec| {
            spec.default
                .map(|value| (spec.name.to_string(), value.to_string()))
        })
        .collect()
}

/// One row of the create/read-back table.
struct CreateRow {
    case: &'static str,
    name: String,
    attributes: Value,
}

/// A create is observable through BOTH readers, and reports exactly what it stored:
/// the supplied attributes, the documented default of every attribute the request
/// named no value for, and the three attributes the record itself derives.
#[tokio::test]
async fn a_created_queue_reports_its_url_arn_supplied_attributes_and_documented_defaults() {
    let router = emulator();
    let rows = vec![
        CreateRow {
            case: "standard queue, no attributes supplied",
            name: "orders".to_string(),
            attributes: json!({}),
        },
        CreateRow {
            case: "underscored name, one integer attribute",
            name: "orders_dead_letter".to_string(),
            attributes: json!({"VisibilityTimeout": "120"}),
        },
        CreateRow {
            case: "resource policy granting a foreign principal",
            name: "policy-queue".to_string(),
            attributes: json!({"Policy": QUEUE_POLICY}),
        },
        CreateRow {
            case: "every documented default overridden at once",
            name: "MixedCase123".to_string(),
            attributes: json!({
                "DelaySeconds": "900",
                "MaximumMessageSize": "1024",
                "MessageRetentionPeriod": "60",
                "ReceiveMessageWaitTimeSeconds": "20",
                "VisibilityTimeout": "43200",
            }),
        },
        CreateRow {
            case: "FIFO queue with the FIFO-only attributes",
            name: "events.fifo".to_string(),
            attributes: json!({
                "FifoQueue": "true",
                "ContentBasedDeduplication": "true",
                "DeduplicationScope": "messageGroup",
                "FifoThroughputLimit": "perMessageGroupId",
            }),
        },
        CreateRow {
            case: "encryption and dead-letter attributes",
            name: "secure-queue".to_string(),
            attributes: json!({
                "KmsMasterKeyId": "alias/aws/sqs",
                "KmsDataKeyReusePeriodSeconds": "300",
                "SqsManagedSseEnabled": "false",
                "RedrivePolicy": r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:orders_dead_letter","maxReceiveCount":5}"#,
                "RedriveAllowPolicy": r#"{"redrivePermission":"allowAll"}"#,
            }),
        },
        CreateRow {
            case: "the longest name a queue may carry",
            name: "q".repeat(MAX_QUEUE_NAME_LENGTH),
            attributes: json!({}),
        },
    ];

    for row in &rows {
        let url = create_ok(&router, &row.name, &row.attributes).await;
        assert_eq!(
            url,
            expected_url(&row.name),
            "case {}: CreateQueue minted the wrong url, got {url}",
            row.case
        );

        // The same queue, reached by name rather than by url.
        let resolved = ok(
            &router,
            "GetQueueUrl",
            json!({"QueueName": row.name.clone()}),
        )
        .await;
        assert_eq!(
            resolved["QueueUrl"].as_str(),
            Some(url.as_str()),
            "case {}: GetQueueUrl reported {resolved} rather than the created url {url}",
            row.case
        );

        let reported = all_attributes(&router, &url).await;
        assert_eq!(
            reported.get("QueueArn").map(String::as_str),
            Some(expected_arn(&row.name).as_str()),
            "case {}: wrong QueueArn in {reported:?}",
            row.case
        );

        // Every supplied attribute is reported with the value the create carried.
        for (name, value) in as_map(&row.attributes) {
            assert_eq!(
                reported.get(&name),
                Some(&value),
                "case {}: attribute {name} was not stored as supplied, got {reported:?}",
                row.case
            );
        }

        // Every attribute with a documented default that the request named no value
        // for carries that default.
        for (name, default) in documented_defaults(row.name.ends_with(".fifo")) {
            if as_map(&row.attributes).contains_key(&name) {
                continue;
            }
            assert_eq!(
                reported.get(&name),
                Some(&default),
                "case {}: attribute {name} did not default to {default}, got {reported:?}",
                row.case
            );
        }

        // Every attribute the record derives is reported; the three that count
        // messages are reported too, as the counts over the queue's own message deque,
        // and a freshly created queue holds nothing.
        for derived in DERIVED_ATTRIBUTES {
            assert!(
                reported.contains_key(*derived),
                "case {}: {derived} is missing from {reported:?}",
                row.case
            );
        }
        for counter in MESSAGE_PLANE_ATTRIBUTES {
            assert_eq!(
                reported.get(*counter).map(String::as_str),
                Some("0"),
                "case {}: {counter} must count the queue's messages, and a new queue holds none, got {reported:?}",
                row.case
            );
        }

        // Both timestamps are whole epoch seconds, and nothing has changed the queue
        // since it was created.
        let created = reported.get("CreatedTimestamp").cloned().unwrap_or_default();
        let modified = reported
            .get("LastModifiedTimestamp")
            .cloned()
            .unwrap_or_default();
        assert!(
            created.parse::<u64>().is_ok(),
            "case {}: CreatedTimestamp {created} is not epoch seconds",
            row.case
        );
        assert_eq!(
            created, modified,
            "case {}: an untouched queue reported CreatedTimestamp {created} and LastModifiedTimestamp {modified}",
            row.case
        );
    }
}

/// `AttributeNames` selects: absent or empty returns no `Attributes` member at all,
/// `All` returns every attribute the queue has, a name list returns just those, and a
/// list naming only attributes the queue has no value for returns no member either.
#[tokio::test]
async fn get_queue_attributes_returns_only_the_attribute_names_the_request_asks_for() {
    let router = emulator();
    let url = create_ok(&router, "selective", &json!({"VisibilityTimeout": "45"})).await;

    for (case, body) in [
        ("no AttributeNames member", json!({"QueueUrl": url})),
        (
            "an empty AttributeNames list",
            json!({"QueueUrl": url, "AttributeNames": []}),
        ),
    ] {
        let reported = ok(&router, "GetQueueAttributes", body).await;
        assert!(
            reported.get("Attributes").is_none_or(Value::is_null),
            "case {case}: expected empty results, got {reported}"
        );
    }

    for (case, requested, expected) in [
        (
            "one derived attribute",
            vec!["QueueArn"],
            vec![("QueueArn", expected_arn("selective"))],
        ),
        (
            "one configured attribute",
            vec!["VisibilityTimeout"],
            vec![("VisibilityTimeout", "45".to_string())],
        ),
        (
            "two attributes at once",
            vec!["VisibilityTimeout", "DelaySeconds"],
            vec![
                ("DelaySeconds", "0".to_string()),
                ("VisibilityTimeout", "45".to_string()),
            ],
        ),
        (
            "an attribute the queue has a value for beside one it has not",
            vec!["VisibilityTimeout", "RedrivePolicy"],
            vec![("VisibilityTimeout", "45".to_string())],
        ),
    ] {
        let reported = ok(
            &router,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": requested}),
        )
        .await;
        let reported = as_map(&reported["Attributes"]);
        let expected: BTreeMap<String, String> = expected
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case}: asking for {requested:?} reported {reported:?}"
        );
    }

    // A selection the queue has no value for ANY name of is the same empty result as a
    // request that selected nothing: no `Attributes` member at all, so a client cannot read
    // a configuration into a queue that carries none. A queue has a `RedrivePolicy` or a
    // `Policy` only once a write gave it one, and this one never did.
    for (case, requested) in [
        ("one name the queue has no value for", vec!["RedrivePolicy"]),
        (
            "two names the queue has no value for",
            vec!["RedrivePolicy", "Policy"],
        ),
    ] {
        let reported = ok(
            &router,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": requested}),
        )
        .await;
        assert!(
            reported.get("Attributes").is_none(),
            "case {case}: asking for {requested:?} expected no Attributes member at all, got \
             {reported}"
        );
    }

    // `All` is a superset of every selectable name, and never fewer than the
    // attributes a single-name request reports.
    let all = all_attributes(&router, &url).await;
    assert!(
        all.contains_key("QueueArn") && all.contains_key("VisibilityTimeout"),
        "case All: expected every attribute the queue has, got {all:?}"
    );
}

/// An integer attribute accepts exactly its documented range, inclusive: the rows are
/// generated from the spec table, so each bound and one value past it is exercised for
/// every integer attribute the model declares.
#[tokio::test]
async fn an_integer_attribute_accepts_its_documented_range_and_refuses_anything_else() {
    let router = emulator();
    for (index, spec) in SETTABLE_ATTRIBUTES.iter().enumerate() {
        let AttributeKind::Integer { min, max } = spec.kind else {
            continue;
        };
        assert!(
            !spec.create_only && !spec.fifo_only,
            "case {}: this test creates standard queues, so an integer attribute must be settable on one",
            spec.name
        );
        for (label, value, accepted) in [
            ("the lower bound", min.to_string(), true),
            ("the upper bound", max.to_string(), true),
            ("one below the lower bound", (min - 1).to_string(), false),
            ("one above the upper bound", (max + 1).to_string(), false),
            ("a value that is not an integer", "many".to_string(), false),
        ] {
            let name = format!("bounds-{index}-{}", value.replace('-', "minus"));
            let attributes = json!({ spec.name.to_string(): value.clone() });
            let (status, parsed) = create(&router, &name, &attributes).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {} = {value} ({label}): expected HTTP 200, got {status} with {parsed}",
                    spec.name
                );
                let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
                let reported = all_attributes(&router, &url).await;
                assert_eq!(
                    reported.get(spec.name).map(String::as_str),
                    Some(value.as_str()),
                    "case {} = {value} ({label}): the stored value is {reported:?}",
                    spec.name
                );
            } else {
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "InvalidAttributeValue"),
                    "case {} = {value} ({label}): expected a 400 InvalidAttributeValue, got {status} with {parsed}",
                    spec.name
                );
            }
        }
    }
}

/// The non-integer attribute kinds: a boolean takes `true`/`false`, a value-set
/// attribute takes one of its documented values, a policy attribute takes a JSON
/// document, and an opaque text attribute takes anything.
#[tokio::test]
async fn a_non_integer_attribute_accepts_exactly_the_values_its_documentation_lists() {
    let router = emulator();
    // The dead-letter queue the redrive row names: a `deadLetterTargetArn` has to name a
    // queue there is, which is `redrive_policy_document`'s rule — this table is about the
    // attribute's FORM.
    let _ = create_ok(&router, "values-dlq", &json!({})).await;
    // (attribute, value, FIFO queue?, accepted)
    let rows: &[(&str, &str, bool, bool)] = &[
        ("SqsManagedSseEnabled", "true", false, true),
        ("SqsManagedSseEnabled", "false", false, true),
        ("SqsManagedSseEnabled", "TRUE", false, false),
        ("SqsManagedSseEnabled", "yes", false, false),
        ("ContentBasedDeduplication", "true", true, true),
        ("DeduplicationScope", "messageGroup", true, true),
        ("DeduplicationScope", "queue", true, true),
        ("DeduplicationScope", "perQueue", true, false),
        ("FifoThroughputLimit", "perQueue", true, true),
        // `perMessageGroupId` is in the value set too, but "is allowed only when the
        // value for DeduplicationScope is messageGroup", so a row that supplies it alone
        // belongs to the pair rule rather than to this value-set table: it is covered,
        // with both halves of the pair, in `fifo_receive_idempotency`.
        ("FifoThroughputLimit", "messageGroup", true, false),
        ("Policy", QUEUE_POLICY, false, true),
        ("Policy", "{}", false, true),
        ("Policy", "not a policy", false, false),
        // A redrive policy is reported in its canonical rendering, so this row supplies the
        // canonical document: what that rendering IS — and that a policy written any other
        // way reads back as it — is `redrive_policy_document`'s own table.
        ("RedrivePolicy", r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:values-dlq","maxReceiveCount":5}"#, false, true),
        ("RedrivePolicy", "maxReceiveCount=5", false, false),
        ("RedriveAllowPolicy", r#"{"redrivePermission":"denyAll"}"#, false, true),
        ("RedriveAllowPolicy", "denyAll", false, false),
        ("KmsMasterKeyId", "alias/aws/sqs", false, true),
        ("KmsMasterKeyId", "1234abcd-12ab-34cd-56ef-1234567890ab", false, true),
    ];

    for (index, (attribute, value, fifo, accepted)) in rows.iter().enumerate() {
        let name = if *fifo {
            format!("values-{index}.fifo")
        } else {
            format!("values-{index}")
        };
        let mut attributes = json!({ attribute.to_string(): value.to_string() });
        if *fifo {
            attributes["FifoQueue"] = json!("true");
        }
        let (status, parsed) = create(&router, &name, &attributes).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {attribute} = {value}: expected HTTP 200, got {status} with {parsed}"
            );
            let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
            let reported = all_attributes(&router, &url).await;
            assert_eq!(
                reported.get(*attribute).map(String::as_str),
                Some(*value),
                "case {attribute} = {value}: the stored value is {reported:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidAttributeValue"),
                "case {attribute} = {value}: expected a 400 InvalidAttributeValue, got {status} with {parsed}"
            );
        }
    }
}

/// An attribute value whose FORM admits text of any length — a policy document, or an
/// opaque key id — is bounded: the store keeps it for the queue's lifetime, so a value
/// at the documented ceiling is accepted and one byte past it is refused.
///
/// The ceiling is the one AWS publishes for the largest of them ("The policy size is
/// limited to 8,192 bytes", the `Policy` attribute's documentation), which this suite
/// spells out rather than importing, so a wrong constant cannot make the rows pass.
#[tokio::test]
async fn an_unbounded_attribute_value_is_refused_past_the_documented_ceiling() {
    const CEILING: usize = 8 * 1024;
    let router = emulator();
    // Each row builds a value of an exact byte length: a JSON document for the policy
    // attributes, plain text for the key id. The rows differ along the attribute, its
    // kind, and whether the length is at the ceiling or one byte past it.
    let json_of = |len: usize| {
        let overhead = r#"{"Pad":""}"#.len();
        format!(r#"{{"Pad":"{}"}}"#, "p".repeat(len - overhead))
    };
    // A `RedrivePolicy` names a dead-letter queue there is and carries the two members the
    // attribute documents, so a row of an exact length pads a member of its own: only the
    // VALUE's length is at issue here, and what the document itself must carry is
    // `redrive_policy_document`'s table.
    let dead_letter = create_ok(&router, "bounded-dlq", &json!({})).await;
    assert!(
        !dead_letter.is_empty(),
        "case the dead-letter queue the redrive rows name: it must have been created"
    );
    let redrive_json_of = |len: usize| {
        let document = |pad: &str| {
            json!({
                "deadLetterTargetArn": expected_arn("bounded-dlq"),
                "maxReceiveCount": 1,
                "Pad": pad,
            })
            .to_string()
        };
        let overhead = document("").len();
        document(&"p".repeat(len - overhead))
    };
    let rows: Vec<(&str, String, bool)> = vec![
        ("Policy", json_of(CEILING), true),
        ("Policy", json_of(CEILING + 1), false),
        ("RedrivePolicy", redrive_json_of(CEILING), true),
        ("RedrivePolicy", redrive_json_of(CEILING + 1), false),
        ("RedriveAllowPolicy", json_of(CEILING), true),
        ("RedriveAllowPolicy", json_of(CEILING + 1), false),
        ("KmsMasterKeyId", "k".repeat(CEILING), true),
        ("KmsMasterKeyId", "k".repeat(CEILING + 1), false),
    ];

    for (index, (attribute, value, accepted)) in rows.iter().enumerate() {
        assert_eq!(
            value.len(),
            if *accepted { CEILING } else { CEILING + 1 },
            "case {attribute} #{index}: the row's value must be exactly at or past the ceiling"
        );
        let name = format!("bounded-{index}");
        let attributes = json!({ attribute.to_string(): value.clone() });
        let (status, parsed) = create(&router, &name, &attributes).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {attribute} at {} bytes: expected HTTP 200, got {status} with {parsed}",
                value.len()
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidAttributeValue"),
                "case {attribute} at {} bytes: expected a 400 InvalidAttributeValue, got \
                 {status} with {parsed}",
                value.len()
            );
            let (status, parsed) = call(
                &router,
                "ListQueues",
                &json!({ "QueueNamePrefix": name.clone() }),
            )
            .await;
            assert_eq!(status, 200, "case {attribute}: ListQueues got {parsed}");
            assert!(
                parsed["QueueUrls"].as_array().is_none_or(Vec::is_empty),
                "case {attribute} at {} bytes: a refused create must store nothing, got {parsed}",
                value.len()
            );
        }
    }
}

/// An attribute name that does not exist, one that is reported rather than set, one
/// that may be supplied only at create, and one that applies only to FIFO queues are
/// all refused as `InvalidAttributeName` — on whichever call supplied it.
#[tokio::test]
async fn an_attribute_name_a_call_may_not_supply_is_refused_as_invalid_attribute_name() {
    let router = emulator();
    let url = create_ok(&router, "names", &json!({})).await;

    // (case, attribute, value) — refused by CreateQueue.
    let mut on_create: Vec<(&str, &str, &str)> = vec![
        ("a name the model does not declare", "NotAnAttribute", "1"),
        ("the All selector", "All", "1"),
        (
            "a FIFO-only attribute on a standard queue",
            "ContentBasedDeduplication",
            "true",
        ),
        (
            "another FIFO-only attribute on a standard queue",
            "DeduplicationScope",
            "queue",
        ),
    ];
    // Every attribute the queue record derives, and every message counter, is
    // reported rather than set — so supplying any of them is refused.
    on_create.extend(
        DERIVED_ATTRIBUTES
            .iter()
            .chain(MESSAGE_PLANE_ATTRIBUTES)
            .map(|name| ("an attribute that is reported, not set", *name, "0")),
    );
    for (index, (case, attribute, value)) in on_create.iter().enumerate() {
        let (status, parsed) = create(
            &router,
            &format!("refused-{index}"),
            &json!({ attribute.to_string(): value.to_string() }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidAttributeName"),
            "case CreateQueue with {case} ({attribute}): expected a 400 InvalidAttributeName, got {status} with {parsed}"
        );
    }

    // Refused by SetQueueAttributes: the same names, plus the create-only one.
    let on_set: &[(&str, &str, &str)] = &[
        ("a name the model does not declare", "NotAnAttribute", "1"),
        ("a create-only attribute", "FifoQueue", "true"),
        ("a derived attribute", "QueueArn", "arn:aws:sqs:us-east-1:000000000000:names"),
        (
            "a FIFO-only attribute on a standard queue",
            "ContentBasedDeduplication",
            "true",
        ),
    ];
    for (case, attribute, value) in on_set {
        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": { attribute.to_string(): value.to_string() }}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidAttributeName"),
            "case SetQueueAttributes with {case} ({attribute}): expected a 400 InvalidAttributeName, got {status} with {parsed}"
        );
    }

    // And by GetQueueAttributes, which selects by name.
    let (status, parsed) = call(
        &router,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["NotAnAttribute"]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidAttributeName"),
        "case GetQueueAttributes with an undeclared name: expected a 400 InvalidAttributeName, got {status} with {parsed}"
    );

    // The queue is unchanged by every refusal above.
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get("CreatedTimestamp"),
        reported.get("LastModifiedTimestamp"),
        "case refusals: a refused SetQueueAttributes must not touch the queue, got {reported:?}"
    );
}

/// What `SetQueueAttributes` sets is what `GetQueueAttributes` then reports; every
/// attribute the call did not name keeps the value it had.
#[tokio::test]
async fn set_queue_attributes_changes_exactly_the_attributes_it_names() {
    let router = emulator();
    // (case, created with, then set to, FIFO queue?)
    let rows: &[(&str, Value, Value, bool)] = &[
        (
            "a default replaced by the lower bound",
            json!({}),
            json!({"VisibilityTimeout": "0"}),
            false,
        ),
        (
            "a default replaced by the upper bound",
            json!({}),
            json!({"VisibilityTimeout": "43200"}),
            false,
        ),
        (
            "a policy attached to a queue created without one",
            json!({}),
            json!({"Policy": QUEUE_POLICY}),
            false,
        ),
        (
            "a policy replaced by another",
            json!({"Policy": "{}"}),
            json!({"Policy": QUEUE_POLICY}),
            false,
        ),
        (
            "several attributes at once",
            json!({"DelaySeconds": "10"}),
            json!({"DelaySeconds": "0", "MessageRetentionPeriod": "60", "ReceiveMessageWaitTimeSeconds": "20"}),
            false,
        ),
        (
            "a FIFO-only attribute on a FIFO queue",
            json!({"FifoQueue": "true"}),
            json!({"ContentBasedDeduplication": "true", "DeduplicationScope": "messageGroup"}),
            true,
        ),
    ];

    for (index, (case, created_with, updates, fifo)) in rows.iter().enumerate() {
        let name = if *fifo {
            format!("updated-{index}.fifo")
        } else {
            format!("updated-{index}")
        };
        let url = create_ok(&router, &name, created_with).await;
        let before = all_attributes(&router, &url).await;

        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": updates}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from SetQueueAttributes, got {status} with {parsed}"
        );

        let after = all_attributes(&router, &url).await;
        for (attribute, value) in as_map(updates) {
            assert_eq!(
                after.get(&attribute),
                Some(&value),
                "case {case}: {attribute} was not set to {value}, got {after:?}"
            );
        }
        for (attribute, value) in &before {
            if as_map(updates).contains_key(attribute) || attribute == "LastModifiedTimestamp" {
                continue;
            }
            assert_eq!(
                after.get(attribute),
                Some(value),
                "case {case}: {attribute} changed although the call did not name it, got {after:?}"
            );
        }
        let created = after.get("CreatedTimestamp").cloned().unwrap_or_default();
        let modified = after
            .get("LastModifiedTimestamp")
            .cloned()
            .unwrap_or_default();
        assert!(
            modified.parse::<u64>().unwrap_or_default() >= created.parse::<u64>().unwrap_or(u64::MAX),
            "case {case}: LastModifiedTimestamp {modified} is before CreatedTimestamp {created}"
        );
    }

    // A map that sets nothing is refused rather than silently touching the queue.
    let url = create_ok(&router, "empty-update", &json!({})).await;
    let (status, parsed) = call(
        &router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "MissingParameter"),
        "case an empty Attributes map: expected a 400 MissingParameter, got {status} with {parsed}"
    );
}

/// Re-creating a name with the same values for every COMPARED attribute returns the
/// existing url; re-creating it with a different value for one of them is refused as
/// `QueueNameExists`. The attributes left out of that comparison have their own rows in
/// `queue_lifecycle`.
#[tokio::test]
async fn re_creating_a_name_is_idempotent_only_for_an_identical_attribute_set() {
    let router = emulator();
    // (case, first create, second create, same url expected)
    let rows: &[(&str, Value, Value, bool)] = &[
        (
            "the same request twice",
            json!({}),
            json!({}),
            true,
        ),
        (
            "the second naming a value equal to the default the first took",
            json!({}),
            json!({"VisibilityTimeout": "30"}),
            true,
        ),
        (
            "the same explicit attributes twice",
            json!({"DelaySeconds": "30", "Policy": QUEUE_POLICY}),
            json!({"DelaySeconds": "30", "Policy": QUEUE_POLICY}),
            true,
        ),
        (
            "the second changing one value",
            json!({}),
            json!({"VisibilityTimeout": "60"}),
            false,
        ),
        (
            "the second dropping an attribute the first set",
            json!({"SqsManagedSseEnabled": "true"}),
            json!({}),
            false,
        ),
        (
            "the second adding an attribute the first did not set",
            json!({}),
            json!({"SqsManagedSseEnabled": "true"}),
            false,
        ),
    ];

    for (index, (case, first, second, idempotent)) in rows.iter().enumerate() {
        let name = format!("recreate-{index}");
        let url = create_ok(&router, &name, first).await;
        let (status, parsed) = create(&router, &name, second).await;
        if *idempotent {
            assert_eq!(
                (status, parsed["QueueUrl"].as_str()),
                (200, Some(url.as_str())),
                "case {case}: expected the existing url {url}, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueNameExists"),
                "case {case}: expected a 400 QueueNameExists, got {status} with {parsed}"
            );
        }
        // Either way the queue the first create made is untouched.
        let reported = all_attributes(&router, &url).await;
        for (attribute, value) in as_map(first) {
            assert_eq!(
                reported.get(&attribute),
                Some(&value),
                "case {case}: the stored queue changed, got {reported:?}"
            );
        }
    }
}

/// A queue name holds up to 80 alphanumeric, hyphen and underscore characters, with
/// `.fifo` the only other form a name may take.
#[tokio::test]
async fn a_queue_name_is_accepted_only_in_the_documented_form() {
    let router = emulator();
    // (case, name, FIFO queue?, accepted)
    let rows: Vec<(&str, String, bool, bool)> = vec![
        ("a single character", "q".to_string(), false, true),
        ("hyphens", "orders-dead-letter".to_string(), false, true),
        ("underscores", "orders_dead_letter".to_string(), false, true),
        ("digits and mixed case", "Orders2024".to_string(), false, true),
        (
            "the longest permitted name",
            "n".repeat(MAX_QUEUE_NAME_LENGTH),
            false,
            true,
        ),
        (
            "the longest permitted FIFO name",
            format!("{}.fifo", "f".repeat(MAX_QUEUE_NAME_LENGTH - 5)),
            true,
            true,
        ),
        (
            "one character past the length bound",
            "n".repeat(MAX_QUEUE_NAME_LENGTH + 1),
            false,
            false,
        ),
        ("an empty name", String::new(), false, false),
        ("a space", "two words".to_string(), false, false),
        ("a dot that is not the FIFO suffix", "orders.v2".to_string(), false, false),
        ("a slash", "orders/v2".to_string(), false, false),
        ("a colon", "orders:v2".to_string(), false, false),
    ];

    for (case, name, fifo, accepted) in &rows {
        let attributes = if *fifo {
            json!({"FifoQueue": "true"})
        } else {
            json!({})
        };
        let (status, parsed) = create(&router, name, &attributes).await;
        if *accepted {
            assert_eq!(
                (status, parsed["QueueUrl"].as_str()),
                (200, Some(expected_url(name).as_str())),
                "case {case} ({name}): expected HTTP 200 and the minted url, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "case {case} ({name}): expected a 400 InvalidParameterValue, got {status} with {parsed}"
            );
        }
    }

    // Designating a queue FIFO is only consistent with a name that carries the suffix:
    // the fault is in the NAME the request supplied, so it is refused as that
    // parameter's value and not as the attribute's.
    let (status, parsed) = create(
        &router,
        "not-suffixed",
        &json!({"FifoQueue": "true"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case FifoQueue=true on a name without the .fifo suffix: expected a 400 InvalidParameterValue, got {status} with {parsed}"
    );
}

/// A `QueueUrl` no queue answers to is refused as `QueueDoesNotExist` by every
/// operation that takes one; a url for the right account and name resolves whatever
/// endpoint it was built against.
#[tokio::test]
async fn a_queue_url_no_queue_answers_to_is_refused_as_queue_does_not_exist() {
    let router = emulator();
    let url = create_ok(&router, "reachable", &json!({})).await;

    let unknown: Vec<(&str, String)> = vec![
        (
            "a name that was never created",
            format!("https://{HOST}/{}/never-created", account()),
        ),
        (
            "another account's queue of the same name",
            format!("https://{HOST}/210987654321/reachable"),
        ),
        ("a value that is not a url at all", "reachable".to_string()),
        ("an empty url", String::new()),
        (
            "a url with no account segment",
            format!("https://{HOST}/reachable"),
        ),
    ];
    for (case, bad) in &unknown {
        let (status, parsed) = call(
            &router,
            "GetQueueAttributes",
            &json!({"QueueUrl": bad, "AttributeNames": ["All"]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case GetQueueAttributes on {case} ({bad}): expected a 400 QueueDoesNotExist, got {status} with {parsed}"
        );

        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": bad, "Attributes": {"VisibilityTimeout": "10"}}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case SetQueueAttributes on {case} ({bad}): expected a 400 QueueDoesNotExist, got {status} with {parsed}"
        );
    }

    // The same account and queue name reached through a different endpoint is the
    // same queue: the url a client holds stays usable whichever endpoint it names.
    let through_another_endpoint = format!("http://127.0.0.1:9297/{}/reachable", account());
    let reported = all_attributes(&router, &through_another_endpoint).await;
    assert_eq!(
        reported.get("QueueArn").map(String::as_str),
        Some(expected_arn("reachable").as_str()),
        "case a url built against another endpoint: expected the same queue, got {reported:?}"
    );

    // And GetQueueUrl refuses a name that was never created, or one owned elsewhere.
    for (case, body) in [
        ("a name that was never created", json!({"QueueName": "absent"})),
        (
            "a queue owned by another account",
            json!({"QueueName": "reachable", "QueueOwnerAWSAccountId": "210987654321"}),
        ),
    ] {
        let (status, parsed) = call(&router, "GetQueueUrl", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case GetQueueUrl on {case}: expected a 400 QueueDoesNotExist, got {status} with {parsed}"
        );
    }

    // The queue that does exist is still reachable by its own url and owner.
    let owned = ok(
        &router,
        "GetQueueUrl",
        json!({"QueueName": "reachable", "QueueOwnerAWSAccountId": account()}),
    )
    .await;
    assert_eq!(
        owned["QueueUrl"].as_str(),
        Some(url.as_str()),
        "case GetQueueUrl naming this emulator's own account: got {owned}"
    );
}

/// Reading a queue changes nothing a later caller sees: two identical reads, with a
/// `GetQueueUrl` between them, report exactly the same attribute map.
#[tokio::test]
async fn reading_a_queue_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let url = create_ok(&router, "stable", &json!({"Policy": QUEUE_POLICY})).await;

    let first = all_attributes(&router, &url).await;
    let _ = ok(&router, "GetQueueUrl", json!({"QueueName": "stable"})).await;
    let second = all_attributes(&router, &url).await;

    assert_eq!(
        first, second,
        "case repeated reads: the reported attributes changed from {first:?} to {second:?}"
    );
}
