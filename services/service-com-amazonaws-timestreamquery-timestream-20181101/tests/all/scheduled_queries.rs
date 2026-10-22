// SPDX-License-Identifier: CC0-1.0

//! The scheduled-query lifecycle and endpoint discovery, end to end over the wire.
//!
//! Every test drives `POST /` with `X-Amz-Target: Timestream_20181101.<Operation>` through
//! the same `MultiServiceRouter` the production emulator serves, so what is asserted here
//! is the HTTP behaviour a client sees — not an internal call.
//!
//! The recorded request in the issue is ONE row of each table below: the tables span the
//! class each rule generalises over (the schedule expression's two forms and their units,
//! page sizes against query counts, the modelled bounds at and past their edges, the
//! region an ARN is minted in).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_timestreamquery_timestream_20181101::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The ARN namespace and account every ARN this emulator mints carries.
const ARN_PREFIX: &str = "arn:aws:timestream:";
const EMULATOR_ACCOUNT: &str = "000000000000";

fn build() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header scoped to `region` and this service's signing name.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/timestream/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    host: Option<&str>,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("http://query.timestream.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("Timestream_20181101.{operation}"))
        .header("authorization", signed_for(region));
    if let Some(host) = host {
        request = request.header("host", host);
    }
    let request = request
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable request body"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, "us-east-1", Some("query.timestream.us-east-1.amazonaws.com"), operation, body).await
}

/// The smallest `CreateScheduledQuery` the model accepts, with `name` and `expression`
/// varied by the caller.
fn minimal_create(name: &str, expression: &str) -> Value {
    json!({
        "Name": name,
        "QueryString": "SELECT 1",
        "ScheduleConfiguration": { "ScheduleExpression": expression },
        "NotificationConfiguration": {
            "SnsConfiguration": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:t" }
        },
        "ScheduledQueryExecutionRoleArn": "arn:aws:iam::000000000000:role/r",
        "ErrorReportConfiguration": { "S3Configuration": { "BucketName": "errors" } }
    })
}

/// The `Arn` of a create that must have succeeded.
async fn create_ok(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, value) = call(router, "CreateScheduledQuery", body).await;
    assert_eq!(status, 200, "create {body}: expected 200, got {status} {value}");
    value["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {body}: no Arn in {value}"))
        .to_string()
}

fn error_code(value: &Value) -> String {
    value["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

fn epoch_secs(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("expected an epoch-seconds timestamp, got {value}"))
}

/// A create round-trips through Describe: every member the request carried comes back
/// from the stored record, and the server-generated members are populated.
///
/// The rows differ in the optional members present, the schedule form, and the name's
/// shape — the recorded `demo-query` case is the first row.
#[tokio::test]
async fn a_created_scheduled_query_is_described_from_the_stored_record() {
    let router = build();

    let cases: Vec<(&str, Value)> = vec![
        ("demo-query", minimal_create("demo-query", "rate(1 hour)")),
        (
            "with_everything",
            json!({
                "Name": "with_everything",
                "QueryString": "SELECT a, b FROM db.tbl WHERE time > @scheduled_runtime",
                "ScheduleConfiguration": { "ScheduleExpression": "cron(15 3 * * ? *)" },
                "NotificationConfiguration": {
                    "SnsConfiguration": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:other" }
                },
                "ScheduledQueryExecutionRoleArn": "arn:aws:iam::000000000000:role/scheduled",
                "ErrorReportConfiguration": {
                    "S3Configuration": {
                        "BucketName": "report-bucket",
                        "ObjectKeyPrefix": "prefix/",
                        "EncryptionOption": "SSE_S3"
                    }
                },
                "KmsKeyId": "alias/my-key",
                "TargetConfiguration": {
                    "TimestreamConfiguration": {
                        "DatabaseName": "agg",
                        "TableName": "rollup",
                        "TimeColumn": "time",
                        "DimensionMappings": [
                            { "Name": "region", "DimensionValueType": "VARCHAR" }
                        ],
                        "MeasureNameColumn": "measure"
                    }
                },
                "Tags": [{ "Key": "team", "Value": "obs" }]
            }),
        ),
        (
            "A!b-c_d*e'f(g)h/i.j",
            minimal_create("A!b-c_d*e'f(g)h/i.j", "rate(15 minutes)"),
        ),
    ];

    for (name, request) in &cases {
        let arn = create_ok(&router, request).await;
        assert!(
            arn.starts_with(&format!("{ARN_PREFIX}us-east-1:{EMULATOR_ACCOUNT}:scheduled-query/{name}-")),
            "case {name:?}: ARN must be a scheduled-query ARN naming the query, got {arn:?}"
        );

        let (status, value) =
            call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": arn })).await;
        assert_eq!(status, 200, "case {name:?}: describe expected 200, got {status} {value}");
        let described = &value["ScheduledQuery"];

        assert_eq!(
            described["Arn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: described Arn, got {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            Some(*name),
            "case {name:?}: described Name, got {described}"
        );
        for member in [
            "QueryString",
            "ScheduleConfiguration",
            "NotificationConfiguration",
            "ErrorReportConfiguration",
            "TargetConfiguration",
            "KmsKeyId",
        ] {
            if let Some(submitted) = request.get(member) {
                assert_eq!(
                    &described[member], submitted,
                    "case {name:?}: {member} must come back as submitted, got {described}"
                );
            }
        }
        assert_eq!(
            described["ScheduledQueryExecutionRoleArn"],
            request["ScheduledQueryExecutionRoleArn"],
            "case {name:?}: execution role, got {described}"
        );
        assert_eq!(
            described["State"].as_str(),
            Some("ENABLED"),
            "case {name:?}: a created query is ENABLED, got {described}"
        );
        assert!(
            epoch_secs(&described["CreationTime"]) > 1_600_000_000.0,
            "case {name:?}: CreationTime must be populated, got {described}"
        );
        assert!(
            described.get("PreviousInvocationTime").is_none()
                || described["PreviousInvocationTime"].is_null(),
            "case {name:?}: nothing has run, so there is no PreviousInvocationTime: {described}"
        );
    }
}

/// `NextInvocationTime` is computed from the stored `ScheduleExpression` for a rate
/// expression — the next run is one interval after the creation time, which is documented
/// arithmetic — and is ABSENT for a cron expression, which this emulator stores verbatim
/// but does not evaluate, rather than inventing an occurrence.
#[tokio::test]
async fn next_invocation_time_is_computed_from_the_schedule_expression() {
    let router = build();

    // Rate rows span every documented unit, in both spellings, at the smallest value and
    // above it.
    let rate_cases = [
        ("rate(1 minute)", 60.0),
        ("rate(5 minutes)", 300.0),
        ("rate(1 hour)", 3_600.0),
        ("rate(12 hours)", 43_200.0),
        ("rate(1 day)", 86_400.0),
        ("rate(7 days)", 604_800.0),
    ];
    for (index, (expression, interval)) in rate_cases.iter().enumerate() {
        let arn = create_ok(&router, &minimal_create(&format!("rate-{index}"), expression)).await;
        let (_, value) =
            call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": arn })).await;
        let described = &value["ScheduledQuery"];
        let created = epoch_secs(&described["CreationTime"]);
        let next = epoch_secs(&described["NextInvocationTime"]);
        assert!(
            (next - created - interval).abs() < 1.0,
            "case {expression:?}: next invocation must be one interval ({interval}s) after \
             creation ({created}), got {next}"
        );
    }

    // Cron rows: (expression, the minutes-of-hour the field allows, hour-of-day when the
    // field pins it). A multi-value minute field may fire at any of its values, so the
    // A cron schedule is stored and reported verbatim, with NO next invocation time:
    // this emulator has no cron engine, and a hand-written subset would answer every
    // expression it did not cover wrongly. The rows span the field forms AWS documents,
    // including the extensions (`L`, `W`, `#`) and `?` in either day field.
    let cron_cases = [
        "cron(0 * * * ? *)",
        "cron(30 * * * ? *)",
        "cron(15 3 * * ? *)",
        "cron(0,30 0 1 * ? *)",
        "cron(0 12 L * ? *)",
        "cron(0 12 ? * 2#1 *)",
        "cron(0/15 * * * ? *)",
        "cron(0 18 ? * MON-FRI *)",
    ];
    for (index, expression) in cron_cases.iter().enumerate() {
        let arn = create_ok(&router, &minimal_create(&format!("cron-{index}"), expression)).await;
        let (_, value) =
            call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": arn })).await;
        let described = &value["ScheduledQuery"];
        assert_eq!(
            described["ScheduleConfiguration"]["ScheduleExpression"].as_str(),
            Some(*expression),
            "case {expression:?}: the expression must come back as submitted, got {described}"
        );
        assert!(
            described["NextInvocationTime"].is_null(),
            "case {expression:?}: no next occurrence may be invented for a cron schedule, \
             got {described}"
        );
        assert!(
            epoch_secs(&described["CreationTime"]) > 1_600_000_000.0,
            "case {expression:?}: CreationTime must still be populated, got {described}"
        );
    }
}

/// A `ScheduleExpression` that is neither documented form is refused with 400
/// `ValidationException`, and nothing is stored for the refused request.
#[tokio::test]
async fn a_schedule_expression_outside_the_two_documented_forms_is_refused() {
    let router = build();

    let refused = [
        "",
        "every 5 minutes",
        "rate()",
        "rate(5)",
        "rate(0 hours)",
        "rate(-1 hours)",
        "rate(5 parsecs)",
        "rate(1 hour extra)",
        "cron(0 12 * * ?)",
        "cron(0 12 * * ? * *)",
        // `?` is allowed only in the day-of-month / day-of-week pair.
        "cron(? * * * ? *)",
        "cron(0 ? * * ? *)",
        "cron(0 12 * ? ? *)",
        "cron(0 12 * * ? ?)",
    ];
    for expression in refused {
        let (status, value) = call(
            &router,
            "CreateScheduledQuery",
            &minimal_create("refused-query", expression),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {expression:?}: expected 400, got {status} {value}"
        );
        assert_eq!(
            error_code(&value),
            "ValidationException",
            "case {expression:?}: expected ValidationException, got {value}"
        );
    }
    let (_, listed) = call(&router, "ListScheduledQueries", &json!({})).await;
    assert_eq!(
        listed["ScheduledQueries"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed}"
    );
}

/// Paging walks every stored scheduled query exactly once, in one stable order, and emits
/// a `NextToken` only while something is left.
///
/// Swept exhaustively over every (query count, page size) pair in a small range rather
/// than sampled: for these sizes the whole class is cheap to enumerate, so there is no
/// seed to print and no case left untried.
#[tokio::test]
async fn paging_returns_every_scheduled_query_exactly_once() {
    for total in 1usize..=6 {
        let router = build();
        let mut created = BTreeSet::new();
        for index in 0..total {
            created.insert(
                create_ok(
                    &router,
                    &minimal_create(&format!("paged-{index}"), "rate(1 hour)"),
                )
                .await,
            );
        }

        for page_size in 1usize..=7 {
            let case = format!("{total} queries, MaxResults={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({ "MaxResults": page_size });
                if let Some(token) = &token {
                    request["NextToken"] = json!(token);
                }
                let (status, value) = call(&router, "ListScheduledQueries", &request).await;
                assert_eq!(status, 200, "case {case}: expected 200, got {status} {value}");
                let page = value["ScheduledQueries"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: ScheduledQueries missing in {value}"));
                assert!(
                    page.len() <= page_size,
                    "case {case}: a page may not exceed MaxResults, got {} items",
                    page.len()
                );
                for item in page {
                    seen.push(
                        item["Arn"]
                            .as_str()
                            .unwrap_or_else(|| panic!("case {case}: no Arn in {item}"))
                            .to_string(),
                    );
                }
                pages += 1;
                assert!(pages <= total + 2, "case {case}: paging did not terminate");
                token = value["NextToken"].as_str().map(ToString::to_string);
                let more_remain = seen.len() < total;
                assert_eq!(
                    token.is_some(),
                    more_remain,
                    "case {case}: NextToken present iff more remain ({} of {total} seen), got {value}",
                    seen.len()
                );
                if token.is_none() {
                    break;
                }
            }
            let mut sorted = seen.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                seen.len(),
                "case {case}: no scheduled query may be returned twice, got {seen:?}"
            );
            assert_eq!(
                sorted,
                created.iter().cloned().collect::<Vec<_>>(),
                "case {case}: paging must cover every stored query"
            );
            assert_eq!(
                seen, sorted,
                "case {case}: pages must come back in one stable order, got {seen:?}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused with 400 `ValidationException` —
/// including one that decodes cleanly but does not name a scheduled query of this
/// service's own ARN shape.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_issue_is_refused() {
    let router = build();
    create_ok(&router, &minimal_create("tokened", "rate(1 hour)")).await;
    let hex_of_abc = hex_encode("abc");
    let hex_of_another_services_arn =
        hex_encode("arn:aws:sqs:us-east-1:000000000000:queue/q");
    for token in [
        "not-a-token",
        "zz",
        "616263646566!",
        "0",
        hex_of_abc.as_str(),
        hex_of_another_services_arn.as_str(),
    ] {
        let (status, value) =
            call(&router, "ListScheduledQueries", &json!({ "NextToken": token })).await;
        assert_eq!(status, 400, "case {token:?}: expected 400, got {status} {value}");
        assert_eq!(
            error_code(&value),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {value}"
        );
    }

    // A cursor of this service's own shape IS a token, even when it sorts past every
    // stored ARN: that is the empty last page, not a refusal.
    let past_the_end = hex_encode(
        "arn:aws:timestream:us-east-1:000000000000:scheduled-query/zzzz-ffffffffffffffff",
    );
    let (status, value) =
        call(&router, "ListScheduledQueries", &json!({ "NextToken": past_the_end })).await;
    assert_eq!(status, 200, "a well-formed cursor: expected 200, got {status} {value}");
    assert_eq!(
        value["ScheduledQueries"].as_array().map(Vec::len),
        Some(0),
        "a cursor past every stored ARN returns an empty final page, got {value}"
    );
    assert!(
        value["NextToken"].is_null(),
        "the final page carries no NextToken, got {value}"
    );
}

/// The token encoding the service uses, so a test names a cursor the way a client gets
/// one back. Deliberately the SAME codec the handler uses (`hex`, a crate dependency of
/// this crate) rather than a second hand-written one.
fn hex_encode(value: &str) -> String {
    hex::encode(value)
}

/// `UpdateScheduledQuery` mutates the stored `State`, and both read views report it.
#[tokio::test]
async fn updating_state_is_visible_to_describe_and_list() {
    let router = build();
    let arn = create_ok(&router, &minimal_create("toggled", "rate(1 hour)")).await;

    for state in ["DISABLED", "ENABLED", "DISABLED"] {
        let (status, value) = call(
            &router,
            "UpdateScheduledQuery",
            &json!({ "ScheduledQueryArn": &arn, "State": state }),
        )
        .await;
        assert_eq!(status, 200, "case {state:?}: update expected 200, got {status} {value}");

        let (_, described) =
            call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": &arn })).await;
        assert_eq!(
            described["ScheduledQuery"]["State"].as_str(),
            Some(state),
            "case {state:?}: Describe must report the new state, got {described}"
        );

        let (_, listed) = call(&router, "ListScheduledQueries", &json!({})).await;
        assert_eq!(
            listed["ScheduledQueries"][0]["State"].as_str(),
            Some(state),
            "case {state:?}: List must report the new state, got {listed}"
        );
    }

    // A state the enum does not model is refused, not stored.
    for bad in ["enabled", "PAUSED", ""] {
        let (status, value) = call(
            &router,
            "UpdateScheduledQuery",
            &json!({ "ScheduledQueryArn": &arn, "State": bad }),
        )
        .await;
        assert_eq!(status, 400, "case {bad:?}: expected 400, got {status} {value}");
        let (_, described) =
            call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": &arn })).await;
        assert_eq!(
            described["ScheduledQuery"]["State"].as_str(),
            Some("DISABLED"),
            "case {bad:?}: a refused update must not change the stored state, got {described}"
        );
    }
}

/// Delete removes the record: Describe then answers 404, List no longer carries it, the
/// name is free again, and a second delete of the same ARN is 404 too.
#[tokio::test]
async fn delete_removes_the_record_and_frees_the_name() {
    let router = build();
    let first = create_ok(&router, &minimal_create("recycled", "rate(1 hour)")).await;
    let other = create_ok(&router, &minimal_create("kept", "rate(1 hour)")).await;

    let (status, value) =
        call(&router, "DeleteScheduledQuery", &json!({ "ScheduledQueryArn": &first })).await;
    assert_eq!(status, 200, "delete expected 200, got {status} {value}");

    let (status, value) =
        call(&router, "DescribeScheduledQuery", &json!({ "ScheduledQueryArn": &first })).await;
    assert_eq!(status, 404, "describe after delete: expected 404, got {status} {value}");
    assert_eq!(
        error_code(&value),
        "ResourceNotFoundException",
        "describe after delete: expected ResourceNotFoundException, got {value}"
    );

    let (status, value) =
        call(&router, "DeleteScheduledQuery", &json!({ "ScheduledQueryArn": &first })).await;
    assert_eq!(status, 404, "second delete: expected 404, got {status} {value}");

    let (_, listed) = call(&router, "ListScheduledQueries", &json!({})).await;
    let arns: Vec<&str> = listed["ScheduledQueries"]
        .as_array()
        .map(|items| items.iter().filter_map(|i| i["Arn"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![other.as_str()],
        "the deleted ARN must be gone from List and the surviving one kept, got {listed}"
    );

    // The name is free again, and the new query is a new record under a new ARN.
    let second = create_ok(&router, &minimal_create("recycled", "rate(1 hour)")).await;
    assert_ne!(
        second, first,
        "re-creating a deleted name must mint a new ARN, got {second}"
    );
}

/// Every ARN-taking operation answers 404 for an ARN that names no live scheduled query.
#[tokio::test]
async fn an_arn_naming_no_live_query_is_a_404_everywhere() {
    let router = build();
    let deleted = create_ok(&router, &minimal_create("gone", "rate(1 hour)")).await;
    call(&router, "DeleteScheduledQuery", &json!({ "ScheduledQueryArn": &deleted })).await;

    let arns = [
        deleted.as_str(),
        "arn:aws:timestream:us-east-1:000000000000:scheduled-query/never-made-0000000000000000",
        "arn:aws:timestream:eu-west-1:000000000000:scheduled-query/other-region-0000000000000000",
        "not-even-an-arn",
    ];
    let operations = [
        ("DescribeScheduledQuery", json!({})),
        ("DeleteScheduledQuery", json!({})),
        ("UpdateScheduledQuery", json!({ "State": "DISABLED" })),
    ];
    for arn in arns {
        for (operation, extra) in &operations {
            let mut request = extra.clone();
            request["ScheduledQueryArn"] = json!(arn);
            let (status, value) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {operation} {arn:?}: expected 404, got {status} {value}"
            );
            assert_eq!(
                error_code(&value),
                "ResourceNotFoundException",
                "case {operation} {arn:?}: expected ResourceNotFoundException, got {value}"
            );
            assert_eq!(
                value["ScheduledQueryArn"].as_str(),
                Some(arn),
                "case {operation} {arn:?}: the error names the ARN asked for, got {value}"
            );
        }
    }
}

/// `CreateScheduledQuery` is `@idempotent` on its `ClientToken`: the repeat answers with
/// the ARN the first call minted and stores no second record. A live `Name` reused by a
/// different request is a 409 `ConflictException`.
#[tokio::test]
async fn a_repeated_client_token_returns_the_first_arn() {
    let router = build();

    // Tokens at the modelled length bounds (32..128) and in between.
    let tokens = [
        "a".repeat(32),
        "b".repeat(64),
        "c".repeat(128),
        "0123456789abcdef-0123456789abcdef".to_string(),
    ];
    for (index, token) in tokens.iter().enumerate() {
        let mut request = minimal_create(&format!("idempotent-{index}"), "rate(1 hour)");
        request["ClientToken"] = json!(token);
        let first = create_ok(&router, &request).await;
        for attempt in 0..2 {
            let repeat = create_ok(&router, &request).await;
            assert_eq!(
                repeat, first,
                "case token #{index} attempt {attempt}: a repeat must answer with the first ARN"
            );
        }
    }
    let (_, listed) = call(&router, "ListScheduledQueries", &json!({})).await;
    assert_eq!(
        listed["ScheduledQueries"].as_array().map(Vec::len),
        Some(tokens.len()),
        "an idempotent repeat must not store a second record, got {listed}"
    );

    // A live name, claimed by a request that is not an idempotent repeat, is a conflict.
    for (index, expression) in ["rate(1 hour)", "rate(2 hours)", "cron(0 1 * * ? *)"]
        .iter()
        .enumerate()
    {
        let mut request = minimal_create("idempotent-0", expression);
        request["ClientToken"] = json!(format!("{}{index}", "z".repeat(40)));
        let (status, value) = call(&router, "CreateScheduledQuery", &request).await;
        assert_eq!(
            status, 409,
            "case {expression:?}: a live name must conflict, got {status} {value}"
        );
        assert_eq!(
            error_code(&value),
            "ConflictException",
            "case {expression:?}: expected ConflictException, got {value}"
        );
    }
}

/// The modelled input constraints are refused with 400, and the values AT each bound are
/// accepted — the bound itself is tested, not only a value well inside it.
#[tokio::test]
async fn the_modelled_input_bounds_are_enforced() {
    let router = build();

    // (case, request, expected status). Sources: ScheduledQueryName (len 1..64, pattern),
    // QueryString (len 1..262144), TagList (len 0..200), MaxScheduledQueriesResults
    // (range 1..1000) — all from the Smithy model.
    let longest_name = format!("a{}", "b".repeat(63));
    let too_long_name = format!("a{}", "b".repeat(64));
    let create_cases: Vec<(&str, Value, u16)> = vec![
        ("name at the 64-char maximum", minimal_create(&longest_name, "rate(1 hour)"), 200),
        ("name one past the maximum", minimal_create(&too_long_name, "rate(1 hour)"), 400),
        ("empty name", minimal_create("", "rate(1 hour)"), 400),
        ("name with a space", minimal_create("bad name", "rate(1 hour)"), 400),
        ("name with a '$'", minimal_create("bad$name", "rate(1 hour)"), 400),
        ("shortest name the pattern allows", minimal_create("ab", "rate(1 hour)"), 200),
        (
            "empty query string",
            {
                let mut request = minimal_create("empty-query", "rate(1 hour)");
                request["QueryString"] = json!("");
                request
            },
            400,
        ),
        (
            "200 tags, the maximum",
            {
                let mut request = minimal_create("tags-at-max", "rate(1 hour)");
                request["Tags"] = Value::Array(
                    (0..200)
                        .map(|i| json!({ "Key": format!("k{i}"), "Value": "v" }))
                        .collect(),
                );
                request
            },
            200,
        ),
        (
            "201 tags, one past the maximum",
            {
                let mut request = minimal_create("tags-past-max", "rate(1 hour)");
                request["Tags"] = Value::Array(
                    (0..201)
                        .map(|i| json!({ "Key": format!("k{i}"), "Value": "v" }))
                        .collect(),
                );
                request
            },
            400,
        ),
    ];
    for (case, request, expected) in &create_cases {
        let (status, value) = call(&router, "CreateScheduledQuery", request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status} {value}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&value),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {value}"
            );
        }
    }

    let list_cases = [(1, 200u16), (1000, 200), (0, 400), (1001, 400), (-1, 400)];
    for (max_results, expected) in list_cases {
        let (status, value) =
            call(&router, "ListScheduledQueries", &json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, expected,
            "case MaxResults={max_results}: expected {expected}, got {status} {value}"
        );
    }
}

/// A minted ARN carries the region the request was signed for, and the emulator's
/// canonical account id — never a literal from a recording.
#[tokio::test]
async fn a_minted_arn_carries_the_requested_region() {
    let router = build();
    for (index, region) in ["us-east-1", "eu-west-1", "ap-southeast-2"].iter().enumerate() {
        let body = minimal_create(&format!("regional-{index}"), "rate(1 hour)");
        let (status, value) = call_in(&router, region, None, "CreateScheduledQuery", &body).await;
        assert_eq!(status, 200, "case {region}: expected 200, got {status} {value}");
        let arn = value["Arn"].as_str().unwrap_or_default();
        assert!(
            arn.starts_with(&format!("{ARN_PREFIX}{region}:{EMULATOR_ACCOUNT}:scheduled-query/")),
            "case {region}: the ARN must carry the signed region, got {arn:?}"
        );
    }
}

/// The store holds a bounded number of scheduled queries: a create past the cap is refused
/// with the modelled `ServiceQuotaExceededException` (402), and deleting one frees a slot.
#[tokio::test]
async fn creating_past_the_quota_is_refused_and_a_delete_frees_a_slot() {
    const CAP: usize = 1_000;
    let router = build();
    let mut first = String::new();
    for index in 0..CAP {
        let arn = create_ok(&router, &minimal_create(&format!("quota-{index}"), "rate(1 hour)")).await;
        if index == 0 {
            first = arn;
        }
    }

    let (status, value) = call(
        &router,
        "CreateScheduledQuery",
        &minimal_create("one-too-many", "rate(1 hour)"),
    )
    .await;
    assert_eq!(
        status, 402,
        "case at the cap of {CAP}: expected 402, got {status} {value}"
    );
    assert_eq!(
        error_code(&value),
        "ServiceQuotaExceededException",
        "case at the cap of {CAP}: expected ServiceQuotaExceededException, got {value}"
    );

    // Releasing one record frees exactly one slot.
    let (status, _) =
        call(&router, "DeleteScheduledQuery", &json!({ "ScheduledQueryArn": &first })).await;
    assert_eq!(status, 200, "delete at the cap: expected 200");
    create_ok(&router, &minimal_create("one-too-many", "rate(1 hour)")).await;
    let (status, value) = call(
        &router,
        "CreateScheduledQuery",
        &minimal_create("and-another", "rate(1 hour)"),
    )
    .await;
    assert_eq!(
        status, 402,
        "case back at the cap: expected 402, got {status} {value}"
    );
}

/// Endpoint discovery answers with the address the request actually reached this emulator
/// on, so an SDK that re-targets itself at the discovered endpoint stays on the emulator.
#[tokio::test]
async fn describe_endpoints_reports_the_address_the_request_arrived_on() {
    let router = build();
    for host in [
        "query.timestream.us-east-1.amazonaws.com",
        "127.0.0.1:9297",
        "localhost:34567",
        "emulator.internal",
    ] {
        let (status, value) =
            call_in(&router, "us-east-1", Some(host), "DescribeEndpoints", &json!({})).await;
        assert_eq!(status, 200, "case {host:?}: expected 200, got {status} {value}");
        let endpoints = value["Endpoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {host:?}: Endpoints missing in {value}"));
        assert_eq!(
            endpoints.len(),
            1,
            "case {host:?}: one reachable endpoint, got {value}"
        );
        assert_eq!(
            endpoints[0]["Address"].as_str(),
            Some(host),
            "case {host:?}: the address must point back at this emulator, got {value}"
        );
        assert!(
            endpoints[0]["CachePeriodInMinutes"].as_i64().unwrap_or(0) > 0,
            "case {host:?}: a cache period must be reported, got {value}"
        );
    }

    // With no Host header and no URI authority (an origin-form request), the answer is
    // still an address that reaches this process, not an empty list and not an AWS
    // hostname.
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", "Timestream_20181101.DescribeEndpoints")
        .header("authorization", signed_for("us-east-1"))
        .body(bytes::Bytes::from_static(b"{}"))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "origin-form request: expected 200"
    );
    let value: Value = serde_json::from_slice(response.body()).expect("a JSON body");
    let address = value["Endpoints"][0]["Address"].as_str().unwrap_or_default();
    assert!(
        address.starts_with("127.0.0.1:"),
        "origin-form request: the fallback must be this emulator's own address, got {address:?}"
    );
}
