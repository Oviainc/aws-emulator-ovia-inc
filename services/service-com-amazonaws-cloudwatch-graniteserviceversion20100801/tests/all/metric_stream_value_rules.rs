// SPDX-License-Identifier: CC0-1.0

//! What a metric-stream operation does with a parameter VALUE whose FORM the model does not
//! state: the characters of a stream's name, and the ARN of the IAM role it delivers through.
//!
//! Three rules are under test, each tabled along what it generalises over rather than along
//! the one request that showed it:
//!
//! 1. a metric stream's `Name` is a parameter value every operation that ACTS ON or REPORTS
//!    the stream it identifies judges — "Valid characters are A-Z, a-z, 0-9, \"-\" and
//!    \"_\"." (`PutMetricStreamInput$Name`) — so a name no stream could carry is refused as a
//!    value, before any stream is looked up, created, started or stopped. `DeleteMetricStream`
//!    asks for the ABSENCE of a stream under the name, which such a name already satisfies, so
//!    it accepts one;
//! 2. a metric stream's `RoleArn` is "the ARN of an IAM role that this metric stream will use
//!    to access Amazon Kinesis Data Firehose resources" (`PutMetricStreamInput$RoleArn`), so a
//!    value that is no IAM role ARN is refused as a value too;
//! 3. both are judged BEFORE the model's own shape checks, and the role before the name: a
//!    caller repairing what it was told about must not be refused for a fault it was never
//!    shown. The model's checks still run for a request these two rules accept.
//!
//! Each refusal is one class — `InvalidParameterValueException`, HTTP 400, whose
//! `awsQueryError` code a query-compatible client reads off the response header — and its
//! message is the fixed sentence the class carries, with nothing of the request echoed into
//! it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call, call_in_query_mode, error_code, error_message, merged, ok,
};

/// The error class a parameter value a metric-stream operation cannot use is refused with
/// (the modelled `InvalidParameterValueException`, HTTP 400, declared on all five
/// metric-stream operations).
const INVALID_VALUE: &str = "InvalidParameterValueException";

/// The wire code that class carries for a query-compatible client: its `awsQueryError` code
/// and the fault side, reported in the `x-amzn-query-error` header.
const QUERY_ERROR: &str = "InvalidParameterValue;Sender";

/// The header that report arrives in.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// How CloudWatch states a name no metric stream can carry.
const INVALID_NAME: &str = "Invalid Name";

/// How CloudWatch states a `RoleArn` that is not the ARN of an IAM role.
const INVALID_ROLE_ARN: &str = "Invalid Role ARN";

/// The class this service answers a value its MODEL refuses with — the other refusal class in
/// play here, which rule 3 keeps for a request the two value rules accept. The body names the
/// class; its legacy query code travels in the header.
const VALIDATION_CODE: &str = "ValidationException";

/// The clause the boundary walk's own refusal opens with, which pins such a refusal as the
/// MODEL's rather than one of this service's fixed sentences.
const VALIDATION_ENVELOPE: &str = "1 validation error detected";

/// The account every ARN this emulator serves carries.
const ACCOUNT: &str = "000000000000";

/// The stream the refusals below must leave alone.
const CONTROL: &str = "control-stream";

/// Every operation that ACTS ON or REPORTS the stream a name identifies, in the order one row
/// drives them: the writer first, so a name a stream CAN carry exists by the time the reader
/// and the batch operations ask for it.
///
/// `DeleteMetricStream` is deliberately not among them — it asks for the ABSENCE of a stream
/// under the name, which a name no stream can carry already satisfies, and the recorded delete
/// of such a name answers 200 ([`a_delete_accepts_a_name_no_stream_could_carry`]).
const NAME_OPERATIONS: [&str; 4] = [
    "PutMetricStream",
    "GetMetricStream",
    "StartMetricStreams",
    "StopMetricStreams",
];

/// One row of [`the_value_rules_are_judged_before_the_models_own_checks`]: the case, what the
/// row changes about the request, the error CODE the row owes, and the exact message where the
/// refusal carries a fixed sentence (`None` where the message is the boundary walk's own
/// counting envelope, which is asserted by its opening clause instead).
type OrderingRow = (
    &'static str,
    Value,
    &'static str,
    Option<&'static str>,
);

/// A `PutMetricStream` request for `name` with `extra` merged over it, built on the required
/// members every metric-stream suite in this target writes them with.
fn put_request(name: &str, extra: &Value) -> Value {
    merged(crate::metric_streams::put_request(name, "json"), extra)
}

/// The request `operation` takes to name `name`: the batch operations carry a list, the others
/// a single member, and the writer the whole configuration.
fn request_naming(operation: &str, name: &str) -> Value {
    match operation {
        "PutMetricStream" => put_request(name, &json!({})),
        "StartMetricStreams" | "StopMetricStreams" => json!({"Names": [name]}),
        _ => json!({"Name": name}),
    }
}

/// Assert that `body` is the refusal of a parameter value, reading exactly `message`.
fn assert_value_refusal(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        (status, error_code(body).as_str(), error_message(body).as_str()),
        (400, INVALID_VALUE, message),
        "case {case:?}: expected HTTP 400 {INVALID_VALUE} reading {message:?}, got {status} \
         {body}"
    );
}

/// The names `ListMetricStreams` reports, in the order it reports them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListMetricStreams", &json!({})).await;
    body.get("Entries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListMetricStreams must report an Entries array, got {body}"))
        .iter()
        .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

#[tokio::test]
async fn a_name_outside_the_published_character_set_is_refused_by_every_metric_stream_operation()
{
    // (case, the name, whether a metric stream can carry it). The character set is the
    // member's own published one ("Valid characters are A-Z, a-z, 0-9, "-" and "_"",
    // `PutMetricStreamInput$Name`); the rows differ along WHICH character outside it the name
    // carries, WHERE in the name it sits, and — for the names a stream can carry — along the
    // length and the case, including each published bound of `MetricStreamName` (1..255).
    let cases: Vec<(&str, String, bool)> = vec![
        ("a name of one letter", "a".to_string(), true),
        ("a name of one digit", "7".to_string(), true),
        ("a name that is one underscore", "_".to_string(), true),
        ("a name that is one hyphen", "-".to_string(), true),
        (
            "every character the set allows",
            "Stream-9_name".to_string(),
            true,
        ),
        ("a name at the modelled maximum", "n".repeat(255), true),
        ("a name with a space inside", "my stream".to_string(), false),
        ("a name with a leading space", " stream".to_string(), false),
        ("a name with a trailing space", "stream ".to_string(), false),
        ("a name with a slash", "team/stream".to_string(), false),
        ("a name with a dot", "stream.one".to_string(), false),
        ("a name with a colon", "stream:one".to_string(), false),
        ("a name with a plus", "stream+one".to_string(), false),
        ("a name with a hash", "stream#1".to_string(), false),
        ("a name with a star", "stream*".to_string(), false),
        (
            "a name with a non-ASCII letter",
            "str\u{eb}am".to_string(),
            false,
        ),
        (
            "a long name with one character outside the set",
            format!("{}.", "n".repeat(100)),
            false,
        ),
    ];

    for (case, name, carriable) in cases {
        let router = build_router();
        ok(&router, "PutMetricStream", &put_request(CONTROL, &json!({}))).await;

        for operation in NAME_OPERATIONS {
            let request = request_naming(operation, &name);
            let (status, body) = call(&router, operation, &request).await;
            if carriable {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} must accept a name a stream can carry, got \
                     {status} {body}"
                );
                continue;
            }
            assert_value_refusal(&format!("{case} on {operation}"), status, &body, INVALID_NAME);
        }

        // Whatever the row, the control stream is untouched: no call naming the row's name
        // reaches another stream.
        let control = ok(&router, "GetMetricStream", &json!({"Name": CONTROL})).await;
        assert_eq!(
            control.get("State").and_then(Value::as_str),
            Some("running"),
            "case {case:?}: no call naming {name:?} may touch another stream, got {control}"
        );
        // A name a stream can carry was created by the row's write; a refused one is
        // materialised nowhere.
        let mut listed = listed_names(&router).await;
        listed.sort();
        let mut expected = vec![CONTROL.to_string()];
        if carriable {
            expected.push(name.clone());
        }
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing to be {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_accepts_a_name_no_stream_could_carry() {
    // The carve-out, and why it is one: a delete asks for a state — no stream under this name
    // — and a name no stream can carry is already in it, so there is no value to refuse. Rows
    // differ along which character outside the published set the name carries; the last row is
    // a name a stream CAN carry, so the operation is shown still deleting what it names.
    let router = build_router();
    ok(&router, "PutMetricStream", &put_request(CONTROL, &json!({}))).await;

    for (case, name) in [
        ("a name with a space", "my stream"),
        ("a name with a slash", "team/stream"),
        ("a name with a dot", "stream.one"),
        ("a name with a non-ASCII letter", "str\u{eb}am"),
    ] {
        let (status, body) = call(&router, "DeleteMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete asks for no stream under {name:?}, which already holds, \
             got {status} {body}"
        );
    }

    // And the stream the store does hold is still deletable, so accepting those names cost the
    // operation nothing of its own job.
    ok(&router, "DeleteMetricStream", &json!({"Name": CONTROL})).await;
    let (status, body) = call(&router, "GetMetricStream", &json!({"Name": CONTROL})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"after the deletes\": the named stream must be gone, got {status} {body}"
    );
}

#[tokio::test]
async fn an_unknown_name_a_stream_could_carry_is_still_reported_as_a_missing_resource() {
    // The rule's boundary: judging the name as a VALUE must not turn a name that is simply not
    // in the store into a value fault. Rows differ along how the well-formed name misses.
    let router = build_router();
    ok(&router, "PutMetricStream", &put_request(CONTROL, &json!({}))).await;
    ok(&router, "PutMetricStream", &put_request("gone-stream", &json!({}))).await;
    ok(&router, "DeleteMetricStream", &json!({"Name": "gone-stream"})).await;

    for (case, name) in [
        ("a name nothing was created under", "absent-stream"),
        ("a name created and then deleted", "gone-stream"),
        ("a stored name in another case", "CONTROL-STREAM"),
        ("a stored name with a suffix", "control-stream-2"),
    ] {
        let (status, body) = call(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a name a stream could carry is a missing resource, not a bad value, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_role_that_is_no_iam_role_arn_is_refused_and_stores_nothing() {
    // (case, the RoleArn, whether it is the ARN of an IAM role). An IAM role's ARN is
    // `arn:<partition>:iam::<account>:role/<name>` — IAM is global, so the ARN names no
    // Region. The rows differ along which PART of that form the value misses: the ARN shape
    // itself, the service, the Region field, the account, the resource type, the role's name.
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a role ARN in this account",
            format!("arn:aws:iam::{ACCOUNT}:role/MetricStreamRole"),
            true,
        ),
        (
            "a role ARN carrying a path",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/CWStream"),
            true,
        ),
        (
            "a bare role name",
            "MetricStreamRole".to_string(),
            false,
        ),
        (
            "an IAM user's ARN",
            format!("arn:aws:iam::{ACCOUNT}:user/streamer"),
            false,
        ),
        (
            "a role ARN with no role name",
            format!("arn:aws:iam::{ACCOUNT}:role/"),
            false,
        ),
        (
            "the delivery stream's own ARN",
            format!("arn:aws:firehose:us-east-1:{ACCOUNT}:deliverystream/metrics"),
            false,
        ),
        (
            "an assumed-role session's ARN",
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/MetricStreamRole/session"),
            false,
        ),
        (
            "a role ARN naming a Region, which IAM has none of",
            format!("arn:aws:iam:us-east-1:{ACCOUNT}:role/MetricStreamRole"),
            false,
        ),
        (
            "a role ARN with no partition",
            format!("arn::iam::{ACCOUNT}:role/MetricStreamRole"),
            false,
        ),
        (
            "a role ARN with no account",
            "arn:aws:iam:::role/MetricStreamRole".to_string(),
            false,
        ),
        (
            "a role ARN missing its arn prefix",
            format!("aws:iam::{ACCOUNT}:role/MetricStreamRole"),
            false,
        ),
    ];

    for (case, role, is_role) in cases {
        let router = build_router();
        let name = "role-checked-stream";
        let request = put_request(name, &json!({"RoleArn": role}));

        let (status, body) = call(&router, "PutMetricStream", &request).await;
        if is_role {
            assert_eq!(
                status, 200,
                "case {case:?}: a role ARN must be accepted, got {status} {body}"
            );
            let stored = ok(&router, "GetMetricStream", &json!({"Name": name})).await;
            assert_eq!(
                stored.get("RoleArn").and_then(Value::as_str),
                Some(role.as_str()),
                "case {case:?}: the configured role must round-trip, got {stored}"
            );
            continue;
        }
        assert_value_refusal(case, status, &body, INVALID_ROLE_ARN);
        // A refused configuration persists nothing.
        let (get_status, get_body) = call(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            (get_status, error_code(&get_body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused PutMetricStream must persist nothing, got {get_status} \
             {get_body}"
        );
    }
}

#[tokio::test]
async fn the_value_rules_are_judged_before_the_models_own_checks() {
    // A role this service cannot use, for the rows that pair it with another fault.
    let bad_role = "metric-stream-role";
    // (case, what the row changes about the request, the error code it owes, the exact message
    // where the refusal carries a fixed sentence). The last two rows carry values both value
    // rules ACCEPT, which is the other half of this test: each is pinned to the code its own
    // rule owes — the model's validation code for the enum, the parameter fault for the
    // ARN-scope rule — so a wrapper that swallowed either would fail here.
    let cases: Vec<OrderingRow> = vec![
        (
            "a bad role beside a value outside the output-format enum",
            json!({"RoleArn": bad_role, "OutputFormat": "xml"}),
            INVALID_VALUE,
            Some(INVALID_ROLE_ARN),
        ),
        (
            "a bad role beside an output format outside its modelled length",
            json!({"RoleArn": bad_role, "OutputFormat": ""}),
            INVALID_VALUE,
            Some(INVALID_ROLE_ARN),
        ),
        (
            "a bad role beside both namespace filter lists",
            json!({
                "RoleArn": bad_role,
                "IncludeFilters": [{"Namespace": "AWS/EC2"}],
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
            }),
            INVALID_VALUE,
            Some(INVALID_ROLE_ARN),
        ),
        (
            "a bad role beside a name no stream can carry",
            json!({"RoleArn": bad_role, "Name": "team/stream"}),
            INVALID_VALUE,
            Some(INVALID_ROLE_ARN),
        ),
        (
            "a bad role beside a name past its modelled length",
            json!({"RoleArn": bad_role, "Name": "n".repeat(300)}),
            INVALID_VALUE,
            Some(INVALID_ROLE_ARN),
        ),
        (
            "a bad name beside a value outside the output-format enum",
            json!({"Name": "team/stream", "OutputFormat": "xml"}),
            INVALID_VALUE,
            Some(INVALID_NAME),
        ),
        (
            "a value outside the output-format enum alone",
            json!({"OutputFormat": "xml"}),
            VALIDATION_CODE,
            None,
        ),
        (
            "a destination outside the stream's own Region alone",
            json!({
                "FirehoseArn": format!("arn:aws:firehose:eu-west-1:{ACCOUNT}:deliverystream/metrics"),
            }),
            INVALID_VALUE,
            Some("FirehoseArn must be in the same region and AWS partition as the Metric Stream"),
        ),
    ];

    for (case, patch, code, message) in cases {
        let router = build_router();
        let (status, body) = call(
            &router,
            "PutMetricStream",
            &put_request("ordered-stream", &patch),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {case:?}: expected HTTP 400 {code}, got {status} {body}"
        );
        match message {
            Some(message) => assert_eq!(
                error_message(&body),
                message,
                "case {case:?}: expected the refusal to read {message:?}, got {body}"
            ),
            // The boundary walk's own refusal, whose message is its counting envelope naming
            // the member and the values the type admits — not a fixed sentence to compare, so
            // the clause that opens it is what pins the refusal as the MODEL's.
            None => assert!(
                error_message(&body).starts_with(VALIDATION_ENVELOPE),
                "case {case:?}: expected the model's own refusal, opening \
                 {VALIDATION_ENVELOPE:?}, got {body}"
            ),
        }
        // Nothing of a refused configuration is stored, whichever rule refused it.
        let (get_status, _) = call(
            &router,
            "GetMetricStream",
            &json!({"Name": "ordered-stream"}),
        )
        .await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused PutMetricStream must persist nothing, got {get_status}"
        );
    }
}

#[tokio::test]
async fn a_query_compatible_client_reads_the_legacy_code_off_the_response_header() {
    // (case, operation, request, the message the refusal reads). Rows span every operation
    // that owes one of these refusals and both rules, because the report belongs to the error
    // CLASS rather than to one operation.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a write naming no IAM role",
            "PutMetricStream",
            put_request("header-stream", &json!({"RoleArn": "not-a-role"})),
            INVALID_ROLE_ARN,
        ),
        (
            "a write naming a name no stream can carry",
            "PutMetricStream",
            put_request("header stream", &json!({})),
            INVALID_NAME,
        ),
        (
            "a read of a name no stream can carry",
            "GetMetricStream",
            json!({"Name": "header stream"}),
            INVALID_NAME,
        ),
        (
            "a start of a name no stream can carry",
            "StartMetricStreams",
            json!({"Names": ["header stream"]}),
            INVALID_NAME,
        ),
        (
            "a stop of a name no stream can carry",
            "StopMetricStreams",
            json!({"Names": ["header stream"]}),
            INVALID_NAME,
        ),
    ];

    let router = build_router();
    for (case, operation, request, message) in cases {
        let (status, headers, body) = call_in_query_mode(&router, operation, &request).await;
        assert_value_refusal(case, status, &body, message);
        let reported = headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            reported, QUERY_ERROR,
            "case {case:?}: expected {QUERY_ERROR_HEADER} to read {QUERY_ERROR:?}, got \
             {reported:?} with {body}"
        );
    }
}

#[tokio::test]
async fn a_batch_naming_one_impossible_name_moves_none_of_the_streams_it_also_names() {
    // The batch operations take a LIST, so the rule has to hold wherever in the list the
    // impossible name sits: the whole batch is refused and no stream it also named moves.
    // Rows differ along which batch operation is called, where the bad name sits and how many
    // there are.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a stop batch whose bad name comes last",
            "StopMetricStreams",
            vec![CONTROL, "team/stream"],
        ),
        (
            "a stop batch whose bad name comes first",
            "StopMetricStreams",
            vec!["team/stream", CONTROL],
        ),
        (
            "a start batch whose bad name sits between two real ones",
            "StartMetricStreams",
            vec![CONTROL, "my stream", CONTROL],
        ),
        (
            "a start batch of bad names only",
            "StartMetricStreams",
            vec!["my stream", "stream.one"],
        ),
    ];

    for (case, operation, names) in cases {
        let router = build_router();
        ok(&router, "PutMetricStream", &put_request(CONTROL, &json!({}))).await;
        // Wound to the state the row's operation would move it OUT of, so a batch that was
        // applied would be visible.
        if operation == "StartMetricStreams" {
            ok(&router, "StopMetricStreams", &json!({"Names": [CONTROL]})).await;
        }
        let before = ok(&router, "GetMetricStream", &json!({"Name": CONTROL})).await;

        let (status, body) = call(&router, operation, &json!({"Names": names})).await;
        assert_value_refusal(case, status, &body, INVALID_NAME);

        let after = ok(&router, "GetMetricStream", &json!({"Name": CONTROL})).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused batch must leave the stream it also named untouched, got \
             {after}"
        );
    }
}
