// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateDelivery`'s idempotence over the pair it creates.
//!
//! The rule under test: a delivery IS the pairing of one delivery source with one delivery
//! destination, so a `CreateDelivery` naming a pair the account already holds has nothing
//! left to create — it answers HTTP 200 with the STORED delivery (the same `id`, and the
//! configuration the account holds for that pair), leaves that delivery as it was, and keeps
//! the number of deliveries of the pair at one. Re-issuing the call is therefore how a client
//! converges on "this delivery exists" instead of an error it has to special-case, and what a
//! delivery's configuration is changed by is `UpdateDeliveryConfiguration`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: which vendor the
//! destination belongs to, which optional members the FIRST create carried (none, record
//! fields with a delimiter and tags, the empty record-field list, an S3 configuration), and
//! what the REPEAT carries (byte-identical, fewer members, other members entirely) — plus how
//! many times it is repeated. The recorded request an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations under test.
const CREATE: &str = "CreateDelivery";
const GET: &str = "GetDelivery";
const UPDATE: &str = "UpdateDeliveryConfiguration";
const DESCRIBE: &str = "DescribeDeliveries";

/// A page size `DescribeDeliveries` admits: `limit` is `@range(min: 1, max: 50)`, so one
/// delivery per page is the smallest page, and reading every page proves the count this suite
/// asserts is the account's rather than one page of it.
const SMALLEST_PAGE: i64 = 1;

/// A ceiling on the pages one read walks, so a service that kept handing back a `nextToken`
/// fails this suite instead of hanging it. Every case here holds a handful of deliveries.
const MAX_PAGES: usize = 64;

/// A delivery source over one log group of this account, as `PutDeliverySource` creates one.
async fn delivery_source(logs: &Logs, name: &str, group: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let (_, group_arn) = logs.group_arns(group).await;
    logs.ok(
        "PutDeliverySource",
        &json!({"name": name, "resourceArn": group_arn, "logType": "APPLICATION_LOGS"}),
    )
    .await;
}

/// A delivery destination over `resource_arn`, answering the destination's own ARN.
async fn delivery_destination(logs: &Logs, name: &str, resource_arn: &str) -> String {
    let answer = logs
        .ok(
            "PutDeliveryDestination",
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
        )
        .await;
    answer["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {answer}"))
        .to_string()
}

/// A `CreateDelivery` request for one pair, carrying the optional members `optional` names.
fn create_request(source: &str, destination_arn: &str, optional: &Value) -> Value {
    let mut body = json!({
        "deliverySourceName": source,
        "deliveryDestinationArn": destination_arn,
    });
    if let Some(members) = optional.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The `delivery` member of a response that carries one.
fn delivery(answer: &Value, case: &str) -> Value {
    answer
        .get("delivery")
        .filter(|delivery| !delivery.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a delivery, got {answer}"))
}

/// Every delivery the account holds, read through `DescribeDeliveries` paged to exhaustion at
/// the smallest page the model admits.
async fn described_to_exhaustion(logs: &Logs) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 0..MAX_PAGES {
        let mut body = json!({"limit": SMALLEST_PAGE});
        if let Some(next) = &token {
            body["nextToken"] = json!(next);
        }
        let page = logs.ok(DESCRIBE, &body).await;
        let items = page["deliveries"].as_array().unwrap_or_else(|| {
            panic!("page {page_number}: DescribeDeliveries reports a deliveries list, got {page}")
        });
        all.extend(items.iter().cloned());
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return all,
        }
    }
    panic!("DescribeDeliveries kept handing back a nextToken after {MAX_PAGES} pages")
}

/// How many deliveries the account holds for one pair, as the paged read reports them.
async fn held_for_pair(logs: &Logs, source: &str, destination_arn: &str) -> usize {
    described_to_exhaustion(logs)
        .await
        .iter()
        .filter(|delivery| {
            delivery["deliverySourceName"].as_str() == Some(source)
                && delivery["deliveryDestinationArn"].as_str() == Some(destination_arn)
        })
        .count()
}

/// Repeating a create of one pair answers the stored delivery, whatever the repeat carries.
#[tokio::test]
async fn repeating_a_create_of_one_pair_answers_the_stored_delivery() {
    let logs = Logs::new();
    delivery_source(&logs, "vpc-flow-logs", "/vended/idempotent-one").await;
    delivery_source(&logs, "waf_logs", "/vended/idempotent-two").await;
    let cwl_destination = {
        logs.ok(
            "CreateLogGroup",
            &json!({"logGroupName": "/vended/idempotent-destination"}),
        )
        .await;
        let (_, group_arn) = logs.group_arns("/vended/idempotent-destination").await;
        delivery_destination(&logs, "to-logs", &group_arn).await
    };
    let s3_destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-idempotent").await;
    let firehose_destination = delivery_destination(
        &logs,
        "to-firehose",
        "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs",
    )
    .await;

    // (case, the source, the destination ARN, what the FIRST create carries, what each REPEAT
    // carries, how many times the create is repeated)
    let cases: Vec<(&str, &str, &str, Value, Value, usize)> = vec![
        (
            "a CloudWatch Logs destination, no optional member, repeated once",
            "vpc-flow-logs",
            &cwl_destination,
            json!({}),
            json!({}),
            1,
        ),
        (
            "an S3 destination configured by the create, repeated byte-identically twice",
            "vpc-flow-logs",
            &s3_destination,
            json!({
                "recordFields": ["event_timestamp", "log_stream", "message"],
                // `FieldDelimiter` is `len 0..5`: the maximum.
                "fieldDelimiter": "|||||",
                "s3DeliveryConfiguration": {
                    "suffixPath": "AWSLogs/{account-id}/{source-type}",
                    "enableHiveCompatiblePath": true,
                },
                "tags": {"env": "prod", "team": "logs"},
            }),
            json!({
                "recordFields": ["event_timestamp", "log_stream", "message"],
                "fieldDelimiter": "|||||",
                "s3DeliveryConfiguration": {
                    "suffixPath": "AWSLogs/{account-id}/{source-type}",
                    "enableHiveCompatiblePath": true,
                },
                "tags": {"env": "prod", "team": "logs"},
            }),
            2,
        ),
        (
            "a Firehose destination configured by the create, repeated with no optional member",
            "waf_logs",
            &firehose_destination,
            json!({
                // `RecordFields` is `@length(min: 0)`: the empty list is a list the caller
                // named, which is not the same as naming none.
                "recordFields": [],
                "fieldDelimiter": "",
            }),
            json!({}),
            1,
        ),
        (
            "an unconfigured create, repeated with record fields and tags of its own",
            "waf_logs",
            &cwl_destination,
            json!({}),
            json!({
                "recordFields": ["message"],
                "fieldDelimiter": "\t",
                "tags": {"team": "platform"},
            }),
            3,
        ),
    ];

    let mut created_ids: Vec<String> = Vec::new();
    for (case, source, destination_arn, first, repeat, repeats) in cases {
        let created = delivery(
            &logs
                .ok(CREATE, &create_request(source, destination_arn, &first))
                .await,
            case,
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: a created delivery carries an id, got {created}")
            })
            .to_string();
        assert!(
            !created_ids.contains(&id),
            "case {case}: a create of a pair the account does not hold mints a fresh id, got {id}"
        );
        created_ids.push(id.clone());

        for attempt in 1..=repeats {
            let body = create_request(source, destination_arn, &repeat);
            let (status, answer) = logs.call(CREATE, &body).await;
            assert_eq!(
                status, 200,
                "case {case}: repeat {attempt} of the same pair is accepted, got {status} {answer}"
            );
            let again = delivery(&answer, case);
            assert_eq!(
                again, created,
                "case {case}: repeat {attempt} answers the stored delivery, got {again:?}"
            );
            // The stored delivery is what both readers report, so the answer above was not a
            // response assembled for this call alone.
            let fetched = delivery(&logs.ok(GET, &json!({"id": id})).await, case);
            assert_eq!(
                fetched, created,
                "case {case}: repeat {attempt} left the stored delivery as it was, got {fetched:?}"
            );
            let held = held_for_pair(&logs, source, destination_arn).await;
            assert_eq!(
                held, 1,
                "case {case}: repeat {attempt} keeps the pair's cardinality at one, got {held}"
            );
        }
    }

    let all = described_to_exhaustion(&logs).await;
    assert_eq!(
        all.len(),
        created_ids.len(),
        "the account holds exactly one delivery per pair created, got {all:?}"
    );
}

/// The delivery a repeat answers is read from stored state: an update to the delivery is what
/// the next repeat reports, and a repeat never writes the configuration it carries.
#[tokio::test]
async fn the_answer_to_a_repeat_is_the_delivery_as_stored() {
    let logs = Logs::new();
    delivery_source(&logs, "route53-resolver-logs", "/vended/idempotent-updates").await;
    let destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-idempotent-updates").await;
    let case = "a delivery whose configuration was updated after it was created";

    let created = delivery(
        &logs
            .ok(
                CREATE,
                &create_request("route53-resolver-logs", &destination, &json!({})),
            )
            .await,
        case,
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a created delivery carries an id, got {created}"))
        .to_string();

    // "To update an existing delivery configuration, use UpdateDeliveryConfiguration": the
    // configuration a later create carries is NOT what changes it.
    logs.ok(
        UPDATE,
        &json!({
            "id": id,
            "recordFields": ["event_timestamp", "message", "bucket_arn"],
            "fieldDelimiter": ",",
        }),
    )
    .await;
    let updated = delivery(&logs.ok(GET, &json!({"id": id})).await, case);
    assert_eq!(
        updated["recordFields"],
        json!(["event_timestamp", "message", "bucket_arn"]),
        "case {case}: the update is reflected by GetDelivery, got {updated:?}"
    );

    // (case, what the repeat carries)
    let repeats: Vec<(&str, Value)> = vec![
        ("a repeat naming no optional member", json!({})),
        (
            "a repeat naming the configuration the create originally had",
            json!({"recordFields": [], "fieldDelimiter": ""}),
        ),
        (
            "a repeat naming a configuration of its own",
            json!({
                "recordFields": ["message"],
                "fieldDelimiter": "|",
                "s3DeliveryConfiguration": {"enableHiveCompatiblePath": false},
            }),
        ),
    ];
    for (repeat_case, optional) in repeats {
        let answer = logs
            .ok(
                CREATE,
                &create_request("route53-resolver-logs", &destination, &optional),
            )
            .await;
        let again = delivery(&answer, repeat_case);
        assert_eq!(
            again, updated,
            "case {repeat_case}: the repeat answers the delivery AS STORED, got {again:?}"
        );
        let fetched = delivery(&logs.ok(GET, &json!({"id": id})).await, repeat_case);
        assert_eq!(
            fetched, updated,
            "case {repeat_case}: the repeat changed nothing about the delivery, got {fetched:?}"
        );
        let held = held_for_pair(&logs, "route53-resolver-logs", &destination).await;
        assert_eq!(
            held, 1,
            "case {repeat_case}: the pair still names one delivery, got {held}"
        );
    }

    // Nothing above consumed the account's delivery ids: a pair the account does NOT hold is
    // still created, under an id of its own.
    let other = delivery_destination(
        &logs,
        "to-s3-other",
        "arn:aws:s3:::vended-logs-idempotent-updates-other",
    )
    .await;
    let another = delivery(
        &logs
            .ok(
                CREATE,
                &create_request("route53-resolver-logs", &other, &json!({})),
            )
            .await,
        "another pair",
    );
    assert_ne!(
        another["id"], updated["id"],
        "case another pair: another pair is another delivery, got {another:?}"
    );
    let all = described_to_exhaustion(&logs).await;
    assert_eq!(
        all.len(),
        2,
        "the account holds one delivery per pair created, got {all:?}"
    );
}
