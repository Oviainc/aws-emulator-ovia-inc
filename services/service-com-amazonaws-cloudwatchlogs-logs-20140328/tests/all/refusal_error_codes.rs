// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ERROR CODE a refused request carries, over the wire.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! code that is not the one the service answers cannot pass.
//!
//! The rule under test: a request refused for violating a MODELLED input constraint —
//! a value outside a declared `@range`, `@length`, `@pattern` or enum, or an absent
//! `@required` member — is answered HTTP 400 with the refusal error the SERVED OPERATION
//! declares, not with the framework's generic `ValidationException`:
//!
//! * `InvalidParameterException` ("A parameter is specified incorrectly") on every
//!   operation that declares it, which is all but the two families below;
//! * `ValidationException` on the delivery, scheduled-query and S3-table-source
//!   operations, whose modelled error set declares that and no
//!   `InvalidParameterException`;
//! * the framework's own wording on the two `@deprecated` tag operations, which declare
//!   NEITHER refusal shape — naming one would answer with an error the operation cannot
//!   return.
//!
//! Each operation's modelled error set is listed in the crate root's `OPERATIONS.md`,
//! which is generated from the same Smithy model as the handlers; the tables below cite
//! the constraint each row violates from that same file.
//!
//! The tables span the class the rule generalises over: the constraint CLASS (range at
//! each published bound and one past it, length, pattern, enum, an absent required
//! member, a violation nested inside a union and a structure), the operation FAMILY (the
//! three refusal-error classes above), and whether the operation is implemented or still
//! answers 501 — a boundary refusal is decided before any handler runs, so a scaffolded
//! operation is refused exactly as an implemented one is. Any single operation an issue
//! happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{group_with_stream, Logs, REGION};

/// The code CloudWatch Logs answers for a parameter it will not accept, on every
/// operation that declares it.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code the delivery / scheduled-query / S3-table-source operations answer, and the
/// framework's own default for an operation that declares neither refusal shape.
const VALIDATION: &str = "ValidationException";

/// One refused request: the operation, the body, the member whose constraint it
/// violates, and the error code the operation must answer with.
struct Row {
    /// What this row varies, for the assertion message.
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member the refusal must name, as the message's clause spells it (the
    /// dot-separated path from the input inwards).
    member: &'static str,
    expected: &'static str,
}

/// A `PutIntegration` body that satisfies every other modelled constraint, so a row can
/// violate exactly one. `ResourceConfig` is a union with one member, and
/// `OpenSearchResourceConfig` requires the role, the viewer principals and a retention
/// in `1..30`.
fn put_integration(integration_type: &str, retention_days: i64) -> Value {
    json!({
        "integrationName": "opensearch-integration",
        "integrationType": integration_type,
        "resourceConfig": {
            "openSearchResourceConfig": {
                "dataSourceRoleArn": "arn:aws:iam::000000000000:role/ingest",
                "dashboardViewerPrincipals": ["arn:aws:iam::000000000000:role/viewer"],
                "retentionDays": retention_days,
            }
        }
    })
}

/// Every refusal class, against the code the operation serving it declares.
fn rows() -> Vec<Row> {
    vec![
        // ---- operations that declare `InvalidParameterException` ----
        Row {
            case: "a limit one above DescribeLimit's maximum of 50",
            operation: "DescribeLogStreams",
            body: json!({"logGroupName": "refusals", "limit": 51}),
            member: "limit",
            expected: INVALID_PARAMETER,
        },
        Row {
            case: "a limit one below DescribeLimit's minimum of 1",
            operation: "DescribeLogStreams",
            body: json!({"logGroupName": "refusals", "limit": 0}),
            member: "limit",
            expected: INVALID_PARAMETER,
        },
        Row {
            case: "a limit one above EventsLimit's maximum of 10000",
            operation: "GetLogEvents",
            body: json!({
                "logGroupName": "refusals",
                "logStreamName": "stream",
                "limit": 10_001,
            }),
            member: "limit",
            expected: INVALID_PARAMETER,
        },
        Row {
            case: "a limit one below EventsLimit's minimum of 1",
            operation: "GetLogEvents",
            body: json!({
                "logGroupName": "refusals",
                "logStreamName": "stream",
                "limit": 0,
            }),
            member: "limit",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `logStreamName` is `@required`, so absence is found while reading the
            // request rather than while walking it — the same refusal to a caller.
            case: "an absent required member",
            operation: "GetLogEvents",
            body: json!({"logGroupName": "refusals"}),
            member: "logStreamName",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `LogStreamName` is `pattern ^[^:*]*$`.
            case: "a log stream name outside its pattern",
            operation: "GetLogEvents",
            body: json!({"logGroupName": "refusals", "logStreamName": "has:colon"}),
            member: "logStreamName",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `StorageTier` is `INTELLIGENT_TIERING | STANDARD`.
            case: "a value outside a required enum",
            operation: "PutStorageTierPolicy",
            body: json!({"storageTier": "GLACIER"}),
            member: "storageTier",
            expected: INVALID_PARAMETER,
        },
        Row {
            case: "an absent required enum",
            operation: "PutStorageTierPolicy",
            body: json!({}),
            member: "storageTier",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `lookupTableName` is `len 1..256, pattern ^[a-zA-Z0-9_]+$`.
            case: "a lookup table name outside its pattern",
            operation: "CreateLookupTable",
            body: json!({"lookupTableName": "not a valid name!"}),
            member: "lookupTableName",
            expected: INVALID_PARAMETER,
        },
        Row {
            case: "a lookup table name below its minimum length",
            operation: "CreateLookupTable",
            body: json!({"lookupTableName": ""}),
            member: "lookupTableName",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `IntegrationType` declares the single value OPENSEARCH.
            case: "a value outside a single-value enum",
            operation: "PutIntegration",
            body: put_integration("ELASTICSEARCH", 7),
            member: "integrationType",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `CollectionRetentionDays` is `range 1..30`, reached through the
            // `ResourceConfig` union and the `OpenSearchResourceConfig` structure.
            case: "a violation nested inside a union and a structure",
            operation: "PutIntegration",
            body: put_integration("OPENSEARCH", 31),
            member: "retentionDays",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `ListAggregateLogGroupSummariesGroupBy` is
            // `DATA_SOURCE_NAME_AND_TYPE | DATA_SOURCE_NAME_TYPE_AND_FORMAT`.
            case: "an aggregation grouped by a value outside the enum",
            operation: "ListAggregateLogGroupSummaries",
            body: json!({"groupBy": "NOT_A_REAL_ENUM"}),
            member: "groupBy",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `LogGroupName` is `len 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$`.
            case: "a log group name outside its pattern",
            operation: "CreateLogGroup",
            body: json!({"logGroupName": "has spaces"}),
            member: "logGroupName",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `logEvents` is `len 1..10000`: an empty batch is below the minimum.
            case: "a list below its minimum length",
            operation: "PutLogEvents",
            body: json!({
                "logGroupName": "refusals",
                "logStreamName": "stream",
                "logEvents": [],
            }),
            member: "logEvents",
            expected: INVALID_PARAMETER,
        },
        Row {
            // `resourceArn` is `len 1..1011, pattern ^[\w+=/:,.@-]*$`.
            case: "a resource ARN outside its pattern",
            operation: "TagResource",
            body: json!({"resourceArn": "not an arn!", "tags": {"team": "logs"}}),
            member: "resourceArn",
            expected: INVALID_PARAMETER,
        },
        // ---- operations that declare `ValidationException` and no
        // ---- `InvalidParameterException`
        Row {
            // `DeliveryId` is `len 1..64, pattern ^[0-9A-Za-z]+$`.
            case: "a delivery id outside its pattern",
            operation: "GetDelivery",
            body: json!({"id": "not a valid id!"}),
            member: "id",
            expected: VALIDATION,
        },
        Row {
            case: "a delivery id below its minimum length",
            operation: "DeleteDelivery",
            body: json!({"id": ""}),
            member: "id",
            expected: VALIDATION,
        },
        Row {
            // `DeliveryDestinationName` is `len 1..60, pattern ^[\w-]*$`.
            case: "a delivery destination name outside its pattern",
            operation: "DeleteDeliveryDestination",
            body: json!({"name": "not a valid name!"}),
            member: "name",
            expected: VALIDATION,
        },
        Row {
            case: "a scheduled-query page size one above its maximum of 1000",
            operation: "ListScheduledQueries",
            body: json!({"maxResults": 1001}),
            member: "maxResults",
            expected: VALIDATION,
        },
        Row {
            // `ScheduledQueryState` is `DISABLED | ENABLED`.
            case: "a scheduled-query state outside the enum",
            operation: "ListScheduledQueries",
            body: json!({"state": "PAUSED"}),
            member: "state",
            expected: VALIDATION,
        },
        Row {
            case: "an absent required member on an S3-table-source operation",
            operation: "ListSourcesForS3TableIntegration",
            body: json!({}),
            member: "integrationArn",
            expected: VALIDATION,
        },
        Row {
            case: "an S3-table-source page size one above its maximum of 100",
            operation: "ListSourcesForS3TableIntegration",
            body: json!({
                "integrationArn": "arn:aws:logs:eu-west-2:000000000000:integration:opensearch",
                "maxResults": 101,
            }),
            member: "maxResults",
            expected: VALIDATION,
        },
        // ---- operations that declare NEITHER refusal shape: the framework's own
        // ---- wording stands, rather than this service naming an error the operation
        // ---- cannot return
        Row {
            case: "a deprecated tag operation that declares neither refusal shape",
            operation: "UntagLogGroup",
            body: json!({"logGroupName": "has spaces", "tags": ["team"]}),
            member: "logGroupName",
            expected: VALIDATION,
        },
        Row {
            case: "a deprecated tag read that declares neither refusal shape",
            operation: "ListTagsLogGroup",
            body: json!({"logGroupName": ""}),
            member: "logGroupName",
            expected: VALIDATION,
        },
    ]
}

/// A modelled-constraint refusal carries the SERVED OPERATION's own refusal error, and
/// states which member failed which constraint.
#[tokio::test]
async fn a_constraint_refusal_carries_the_operations_own_error_code() {
    let logs = Logs::new();
    // A log group and a stream exist, so a row that names them is refused for its
    // constraint rather than for a resource that is not there.
    group_with_stream(&logs, "refusals", "stream", None).await;

    for row in rows() {
        let (status, body) = logs.call(row.operation, &row.body).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({operation}): a modelled-constraint refusal is HTTP 400, got {status} {body}",
            case = row.case,
            operation = row.operation
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, row.expected,
            "case {case:?} ({operation}): expected {expected}, got {code} / {body}",
            case = row.case,
            operation = row.operation,
            expected = row.expected
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {}: a refusal carries a message, got {body}", row.case));
        assert!(
            message.contains("validation error") && message.contains(row.member),
            "case {case:?} ({operation}): the refusal names {member} and the constraint it \
             failed, got {message:?}",
            case = row.case,
            operation = row.operation,
            member = row.member
        );
    }
}

/// The refusal code belongs to the operation being served, not to the service: two
/// operations refused for the SAME constraint class in the same process answer with
/// their own codes.
#[tokio::test]
async fn two_operations_refused_alike_answer_with_their_own_codes() {
    let logs = Logs::new();
    // Both members are a `@pattern`-constrained required string; the operations differ
    // only in which refusal error their model declares.
    let cases = [
        (
            "DescribeLogStreams, which declares InvalidParameterException",
            "DescribeLogStreams",
            json!({"logGroupName": "has spaces"}),
            INVALID_PARAMETER,
        ),
        (
            "GetDelivery, which declares only ValidationException",
            "GetDelivery",
            json!({"id": "has spaces"}),
            VALIDATION,
        ),
    ];
    for (case, operation, body, expected) in cases {
        let code = logs.err(operation, &body).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }
}

/// Only the BOUNDARY refusal is reworded: an error a handler returns itself keeps its
/// own modelled shape, and a request that violates nothing is served normally.
#[tokio::test]
async fn a_handlers_own_error_and_a_valid_request_are_untouched() {
    let logs = Logs::new();
    group_with_stream(&logs, "handler-errors", "stream", None).await;

    // A handler's own modelled refusal: the stream is within every modelled constraint,
    // so the handler is reached and answers for the state it found.
    let code = logs
        .err(
            "GetLogEvents",
            &json!({"logGroupName": "absent-group", "logStreamName": "stream"}),
        )
        .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case an absent log group: the handler's own error is untouched, got {code}"
    );

    // A valid request still routes to its operation and reads the shared store.
    let page = logs
        .ok(
            "DescribeLogStreams",
            &json!({"logGroupName": "handler-errors", "limit": 50}),
        )
        .await;
    let names: Vec<&str> = page
        .get("logStreams")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogStreams must return logStreams, got {page}"))
        .iter()
        .filter_map(|stream| stream.get("logStreamName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["stream"],
        "case limit at its maximum of 50: the created stream is reported, got {page}"
    );
}

/// A request naming no modelled operation keeps the framework's routing answer: the
/// refusal wording this service chooses is for a request that REACHED an operation.
#[tokio::test]
async fn a_request_naming_no_modelled_operation_is_not_a_refusal() {
    let logs = Logs::new();
    let (status, body) = logs.call("NotARealOperation", &json!({})).await;
    assert_eq!(
        status, 501,
        "case an unrouted operation: the awsJson routing answer is 501, got {status} {body}"
    );
    let code = crate::harness::error_code(&body);
    assert_eq!(
        code, "NotImplemented",
        "case an unrouted operation: expected NotImplemented, got {code} / {body}"
    );
}

/// The clause a refusal states names the member and the requirement it failed, and quotes
/// NO value back: not the submitted one, and not `null` for a member that was absent.
/// An enum's admissible set is listed in ascending order of its values, whatever order
/// the model declares them in.
#[tokio::test]
async fn a_clause_names_the_member_and_the_requirement_without_a_value() {
    let logs = Logs::new();
    group_with_stream(&logs, "clauses", "stream", None).await;

    // (case, operation, body, the whole clause the refusal must state)
    let cases = [
        (
            // `StorageTier` is declared `[STANDARD, INTELLIGENT_TIERING]`, so the sorted
            // set is not the declaration order.
            "a value outside an enum",
            "PutStorageTierPolicy",
            json!({"storageTier": "GLACIER"}),
            "1 validation error detected: Value at 'storageTier' failed to satisfy \
             constraint: Member must satisfy enum value set: [INTELLIGENT_TIERING, STANDARD]",
        ),
        (
            // The same operation's member, absent: no `null` in the subject either.
            "an absent required member",
            "PutStorageTierPolicy",
            json!({}),
            "1 validation error detected: Value at 'storageTier' failed to satisfy \
             constraint: Member must not be null",
        ),
        (
            // `ListAggregateLogGroupSummariesGroupBy` is declared
            // `[DATA_SOURCE_NAME_TYPE_AND_FORMAT, DATA_SOURCE_NAME_AND_TYPE]`.
            "a value outside another enum, declared in the other order",
            "ListAggregateLogGroupSummaries",
            json!({"groupBy": "NOT_A_REAL_ENUM"}),
            "1 validation error detected: Value at 'groupBy' failed to satisfy constraint: \
             Member must satisfy enum value set: [DATA_SOURCE_NAME_AND_TYPE, \
             DATA_SOURCE_NAME_TYPE_AND_FORMAT]",
        ),
        (
            // A `@pattern` keeps the anchors the model writes it with.
            "a value outside a pattern",
            "CreateLookupTable",
            json!({"lookupTableName": "bad-name-with-hyphen"}),
            "1 validation error detected: Value at 'lookupTableName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_]+$",
        ),
        (
            "a value above a bound",
            "GetLogEvents",
            json!({"logGroupName": "clauses", "logStreamName": "stream", "limit": 10_001}),
            "1 validation error detected: Value at 'limit' failed to satisfy constraint: \
             Member must have value less than or equal to 10000",
        ),
        (
            "a value below a bound",
            "DescribeLogStreams",
            json!({"logGroupName": "clauses", "limit": 0}),
            "1 validation error detected: Value at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1",
        ),
    ];
    for (case, operation, body, expected) in cases {
        let (_, message) = logs.refusal(operation, &body).await;
        assert_eq!(
            message, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {message:?}"
        );
    }
}

/// The log-group selector pair is checked ACROSS two members — `logGroupName` and
/// `logGroupIdentifier` are mutually exclusive and one is required — which no single
/// member's constraint can express. The service answers that check with 400
/// `ValidationException`, on every operation carrying the pair, whichever way the request
/// gets it wrong.
#[tokio::test]
async fn the_log_group_selector_pair_is_refused_as_a_validation_exception() {
    let logs = Logs::new();
    group_with_stream(&logs, "selectors", "stream", None).await;

    // Each operation carrying the pair, against each way of getting it wrong. The
    // GetLogEvents rows carry `logStreamName`, which that operation requires, so the
    // selector check is what refuses them.
    let cases = [
        (
            "both selectors on DescribeLogStreams",
            "DescribeLogStreams",
            json!({"logGroupName": "selectors", "logGroupIdentifier": "selectors"}),
            "LogGroup name and LogGroup ARN are mutually exclusive parameters.",
        ),
        (
            "neither selector on DescribeLogStreams",
            "DescribeLogStreams",
            json!({}),
            "Either LogGroupName or LogGroupArn must be provided.",
        ),
        (
            "both selectors on GetLogEvents",
            "GetLogEvents",
            json!({
                "logGroupName": "selectors",
                "logGroupIdentifier": "selectors",
                "logStreamName": "stream",
            }),
            "LogGroup name and LogGroup ARN are mutually exclusive parameters.",
        ),
        (
            "neither selector on GetLogEvents",
            "GetLogEvents",
            json!({"logStreamName": "stream"}),
            "Either LogGroupName or LogGroupArn must be provided.",
        ),
    ];
    for (case, operation, body, expected_message) in cases {
        let (code, message) = logs.refusal(operation, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code} / {message}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }

    // One selector, named either way, is served rather than refused.
    for (case, body) in [
        ("by name", json!({"logGroupName": "selectors"})),
        (
            "by ARN",
            json!({"logGroupIdentifier": format!(
                "arn:aws:logs:{REGION}:{}:log-group:selectors",
                engine::emulator_account_id()
            )}),
        ),
    ] {
        let page = logs.ok("DescribeLogStreams", &body).await;
        let names: Vec<&str> = page
            .get("logStreams")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: a success carries logStreams, got {page}"))
            .iter()
            .filter_map(|stream| stream.get("logStreamName").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec!["stream"],
            "case {case}: one selector names the group, got {page}"
        );
    }
}
