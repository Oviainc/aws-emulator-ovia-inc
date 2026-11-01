// SPDX-License-Identifier: CC0-1.0

//! Which of the actions a shard event source's refusal names the execution role of the
//! invoked function actually has to be granted before `CreateEventSourceMapping` wires the
//! pair together.
//!
//! RULE: the authorization of a call that wires a function to a shard event source is
//! decided on the calls its poller makes to CONSUME the named source — describing that
//! source, opening an iterator on one of its shards, and getting that shard's records, the
//! same three whichever of the two services holds it — each with the `EventSourceArn` as the
//! resource; the rest of what the refusal lists is the wider grant it advises writing (an
//! account-wide enumeration of the service's sources, which IAM scopes to no single one of
//! them, and the source-discovery reads beside it), so no mapping is refused for the want of
//! one. When a consuming call IS withheld the refusal is worded as that source's own service
//! words this error class, ending on the tail that wording carries.
//!
//! So a least-privilege role granted exactly those three calls on the named source is
//! accepted (HTTP 202) and the mapping reads back; a role short of one of them is refused
//! (HTTP 400 `InvalidParameterValueException`) with nothing stored, however widely the
//! advised actions are granted beside it.
//!
//! The rows differ along what the rule generalises over: WHICH shard source the mapping
//! polls (each of the two, whose listed actions and refusal tails differ), WHICH of the
//! actions its refusal lists the role carries (the three consuming calls, those three short
//! of a different one each time, only the advised ones, none), and HOW the grant names the
//! action and the resource (the source's ARN, an ARN wildcard, `*` for the enumeration as
//! AWS's own managed execution-role policies grant it). Every request goes through the
//! production router with IAM mounted beside Lambda on one engine, so the documents the gate
//! reads are the ones `CreateRole`/`PutRolePolicy` persisted.

use serde_json::{json, Value};

use crate::event_source_mapping_role_access::{
    allow, assert_refused, create_body, create_function_as, given_role, logs_only, router_with_iam,
    stream_refusal, DYNAMODB_READS, DYNAMODB_TAIL, KINESIS_READS, KINESIS_TAIL,
};
use crate::event_source_mappings::{
    create_mapping, get_mapping, list_mappings, DYNAMODB_SOURCE, KINESIS_SOURCE,
};

/// The calls consuming a shard source takes, and so the ones a mapping onto one is decided
/// on: describe the source, open an iterator on a shard, get that shard's records. Both
/// shard sources' APIs name them alike, which is why one list serves both.
const CONSUMING_CALLS: [&str; 3] = ["DescribeStream", "GetShardIterator", "GetRecords"];

/// What a Kinesis stream's refusal lists BESIDE those three: the rest of the grant AWS's own
/// `AWSLambdaKinesisExecutionRole` carries (`KINESIS_READS` minus the consuming calls).
const KINESIS_ADVISED: [&str; 3] = ["DescribeStreamSummary", "ListShards", "ListStreams"];

/// The same for a DynamoDB stream, from `AWSLambdaDynamoDBExecutionRole`: the account-wide
/// enumeration alone.
const DYNAMODB_ADVISED: [&str; 1] = ["ListStreams"];

/// The only resource form a policy can grant an account-wide enumeration on.
const EVERY_RESOURCE: &str = "*";

/// An ARN wildcard over this account's Kinesis streams, the other form a resource-scoped
/// grant takes.
const KINESIS_ARN_WILDCARD: &str = "arn:aws:kinesis:us-east-1:000000000000:stream/*";

/// `CONSUMING_CALLS` without `missing`.
fn consuming_except(missing: &str) -> Vec<&'static str> {
    CONSUMING_CALLS
        .iter()
        .copied()
        .filter(|name| *name != missing)
        .collect()
}

/// One row: the source, the statements of the role's one inline policy, and the sentence the
/// create must be refused with (`None` when it must be served).
struct Row {
    case: String,
    event_source: &'static str,
    statements: Value,
    refusal: Option<String>,
}

/// What a mapping onto `event_source` of the service `namespace` is refused with, and which
/// actions its refusal advises.
struct Source {
    namespace: &'static str,
    event_source: &'static str,
    advised: &'static [&'static str],
    refusal: String,
}

/// The two shard sources, each with its own refusal.
fn sources() -> [Source; 2] {
    [
        Source {
            namespace: "kinesis",
            event_source: KINESIS_SOURCE,
            advised: &KINESIS_ADVISED,
            refusal: stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL),
        },
        Source {
            namespace: "dynamodb",
            event_source: DYNAMODB_SOURCE,
            advised: &DYNAMODB_ADVISED,
            refusal: stream_refusal(DYNAMODB_SOURCE, &DYNAMODB_READS, DYNAMODB_TAIL),
        },
    ]
}

/// The catalog, over both shard sources.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for source in sources() {
        let Source {
            namespace,
            event_source,
            advised,
            refusal,
        } = source;
        // The least-privilege role of the reported case: exactly the consuming calls, on the
        // source itself, with none of the advised actions granted anywhere.
        rows.push(Row {
            case: format!("{namespace}: granted exactly the consuming calls on the source"),
            event_source,
            statements: json!([
                logs_only(),
                allow(namespace, &CONSUMING_CALLS, event_source)
            ]),
            refusal: None,
        });
        // The same grant as AWS's managed policy writes it: the consuming calls on the
        // source, the advised actions on every source.
        rows.push(Row {
            case: format!("{namespace}: with the advised actions granted on every source too"),
            event_source,
            statements: json!([
                allow(namespace, &CONSUMING_CALLS, event_source),
                allow(namespace, advised, EVERY_RESOURCE)
            ]),
            refusal: None,
        });
        // The advised actions alone show nothing about consuming this source.
        rows.push(Row {
            case: format!("{namespace}: with only the advised actions granted"),
            event_source,
            statements: json!([
                logs_only(),
                allow(namespace, advised, EVERY_RESOURCE),
                allow(namespace, advised, event_source)
            ]),
            refusal: Some(refusal.clone()),
        });
        rows.push(Row {
            case: format!("{namespace}: with nothing of the source granted"),
            event_source,
            statements: json!([logs_only()]),
            refusal: Some(refusal.clone()),
        });
        // Each consuming call in turn is the one the role is short of, with everything else
        // the refusal lists granted as widely as IAM allows: the refusal can only be the
        // missing one.
        for missing in CONSUMING_CALLS {
            rows.push(Row {
                case: format!("{namespace}: short of {missing}"),
                event_source,
                statements: json!([
                    allow(namespace, &consuming_except(missing), event_source),
                    allow(namespace, advised, EVERY_RESOURCE)
                ]),
                refusal: Some(refusal.clone()),
            });
        }
    }
    // The resource a grant names may be an ARN wildcard over the sources rather than one
    // source's own ARN.
    rows.push(Row {
        case: "kinesis: granted the consuming calls through an ARN wildcard".to_string(),
        event_source: KINESIS_SOURCE,
        statements: json!([allow("kinesis", &CONSUMING_CALLS, KINESIS_ARN_WILDCARD)]),
        refusal: None,
    });
    rows
}

#[tokio::test]
async fn a_role_granted_the_calls_that_consume_the_source_may_wire_it_to_a_function() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case.as_str();
        let router = router_with_iam();
        let role = given_role(
            &router,
            case,
            &format!("esm-least-privilege-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let function = format!("esm-least-privilege-fn-{index}");
        create_function_as(&router, case, &function, &role).await;

        let answer = create_mapping(&router, &create_body(&function, row.event_source)).await;

        if let Some(expected) = row.refusal.as_deref() {
            assert_refused(case, &answer, expected);
            // The refusal is instead of the mapping, not beside it.
            let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
            assert_eq!(
                (listed.status, listed.listed_uuids()),
                (200, Vec::new()),
                "case {case:?}: a refused create must leave no mapping behind, got {} {}",
                listed.status,
                listed.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            202,
            "case {case:?}: expected HTTP 202, got {} {}",
            answer.status,
            answer.text()
        );
        let state = answer.string("State").unwrap_or_default();
        assert!(
            state == "Creating" || state == "Enabled",
            "case {case:?}: expected the mapping to be Creating or Enabled, got {state:?} in {}",
            answer.text()
        );
        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));
        // Round trip: the accepted mapping is one this emulator holds, on the source it was
        // made for.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("EventSourceArn")),
            (200, Some(row.event_source.to_string())),
            "case {case:?}: the mapping must read back on {}, got {} {}",
            row.event_source,
            reading.status,
            reading.text()
        );
    }
}

/// RULE (the wording): the refusal of a role that may not consume a shard source is the
/// sentence that source's own service words for this error class, including the tail it ends
/// on, which the two sources do not share.
#[tokio::test]
async fn the_refusal_of_each_shard_source_carries_that_source_s_own_sentence() {
    // (case, the event source, the whole sentence AWS answers)
    let rows: [(&str, &str, String); 2] = [
        (
            "a kinesis stream",
            KINESIS_SOURCE,
            format!(
                "Cannot access stream {KINESIS_SOURCE}. Please ensure the role can perform the \
                 GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, \
                 ListShards, and ListStreams Actions on your stream."
            ),
        ),
        (
            "a dynamodb stream",
            DYNAMODB_SOURCE,
            format!(
                "Cannot access stream {DYNAMODB_SOURCE}. Please ensure the role can perform the \
                 GetRecords, GetShardIterator, DescribeStream, and ListStreams Actions on your \
                 stream in IAM."
            ),
        ),
    ];

    for (index, (case, event_source, expected)) in rows.iter().enumerate() {
        let router = router_with_iam();
        let role = given_role(
            &router,
            case,
            &format!("esm-wording-{index}"),
            Some(json!([logs_only()])),
        )
        .await;
        let function = format!("esm-wording-fn-{index}");
        create_function_as(&router, case, &function, &role).await;

        let answer = create_mapping(&router, &create_body(&function, event_source)).await;
        assert_refused(case, &answer, expected);
    }
}
