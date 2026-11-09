// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules a scheduled query's CONFIGURATION is held to, on both
//! operations that state one — `CreateScheduledQuery` and `UpdateScheduledQuery`.
//!
//! Three rules are under test, each asserted over a set of inputs rather than one request:
//!
//! * `executionRoleArn` must be an IAM ROLE ARN. A value that is not one fails the expression the
//!   service publishes for that member and is refused HTTP 400 `ValidationException` with the
//!   member-constraint sentence, before anything is stored or written; a value that is one is
//!   accepted, in every AWS partition the expression names.
//! * a `logGroupIdentifiers` entry names a log group the ACCOUNT HOLDS. A write naming one it does
//!   not hold is refused HTTP 400 `ResourceNotFoundException` about THAT log group — resolved from
//!   either form of the identifier, at any position in the list — and, on the update, ahead of the
//!   scheduled query the identifier names, so a request that names both a missing log group and a
//!   missing query is refused about the log group.
//! * `endTimeOffset` has an EFFECTIVE value whether or not the write states one, so every reader
//!   of the query reports it rather than omitting the member.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — and every write
//! is read back through `GetScheduledQuery`, so a response that is not computed from the shared
//! store cannot pass.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, scheduled_query_arn,
};

use crate::harness::{without_sdk_envelope, Logs, REGION};

/// The operations under test, and the reader every write is proved through.
const CREATE: &str = "CreateScheduledQuery";
const UPDATE: &str = "UpdateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The codes the two refusals travel under, and the status both carry.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";
const REFUSAL_STATUS: u16 = 400;

/// An IAM role ARN every accepted case executes under.
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The log group every accepted case reads, and one more to vary the list with.
const GROUP: &str = "/aws/lambda/orders";
const OTHER_GROUP: &str = "/aws/lambda/payments";

/// The account the emulator serves, which the missing-log-group refusal names.
const ACCOUNT: &str = "000000000000";

/// The sentence a member outside its constraint is refused with, pinned LITERALLY here rather
/// than built with the helper the handler calls, so a wording that is wrong but consistent with
/// the implementation cannot pass. The expression is the one AWS publishes for a scheduled
/// query's `executionRoleArn`, rendered unanchored as AWS renders it.
fn execution_role_refusal() -> String {
    "1 validation error detected: Value at 'executionRoleArn' failed to satisfy constraint: \
     Member must satisfy regular expression pattern: \
     arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\\/(.+)"
        .to_string()
}

/// The sentence a log group the account does not hold is refused with, likewise pinned here. The
/// variable parts are the group's NAME — whichever form the request named it by — and the account
/// it was looked for in.
fn log_group_refusal(log_group_name: &str) -> String {
    format!("Log group '{log_group_name}' does not exist for account ID '{ACCOUNT}'")
}

/// The offset a write that states no `endTimeOffset` resolves to: the lookback window ends at the
/// execution instant.
const DEFAULT_END_TIME_OFFSET: i64 = 0;

/// The log groups a case reads, created over the wire like any client's call.
async fn seed_log_groups(logs: &Logs) {
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
}

/// A create body for `name`, with the members a create must state plus whatever `extras` adds.
fn create_body(name: &str, extras: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("queryLanguage".to_string(), json!("CWLI"));
    body.insert("queryString".to_string(), json!("fields @timestamp, @message"));
    body.insert("scheduleExpression".to_string(), json!("cron(0 * * * ? *)"));
    body.insert("executionRoleArn".to_string(), json!(ROLE));
    if let Value::Object(extras) = extras {
        for (key, value) in extras {
            body.insert(key, value);
        }
    }
    Value::Object(body)
}

/// An update body addressing `identifier`, with the members an update must state plus `extras`.
fn update_body(identifier: &str, extras: Value) -> Value {
    let mut body = Map::new();
    body.insert("identifier".to_string(), json!(identifier));
    body.insert("queryLanguage".to_string(), json!("CWLI"));
    body.insert("queryString".to_string(), json!("fields @message"));
    body.insert("scheduleExpression".to_string(), json!("cron(5 * * * ? *)"));
    body.insert("executionRoleArn".to_string(), json!(ROLE));
    if let Value::Object(extras) = extras {
        for (key, value) in extras {
            body.insert(key, value);
        }
    }
    Value::Object(body)
}

/// One integer member of a response, or `None` when the response omits it.
fn number(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(Value::as_i64)
}

/// Every value outside the published expression is refused — on the create and on the update
/// alike — with the member-constraint sentence, and nothing is stored or written for it.
#[tokio::test]
async fn an_execution_role_that_is_not_an_iam_role_arn_is_refused() {
    // The class the rule generalises over: a value that is no ARN, an ARN of another service, an
    // IAM ARN naming another resource kind, an account id of the wrong width, a role with no
    // name, an IAM ARN carrying a Region, a partition outside the published set, and the empty
    // value the member's `@length(min: 1)` still admits as a string.
    let refused: [(&str, &str); 8] = [
        ("not an ARN at all", "scheduled-query-execution"),
        (
            "an ARN of another service",
            "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/lambda/orders",
        ),
        ("an IAM user rather than a role", "arn:aws:iam::000000000000:user/someone"),
        ("an account id that is too short", "arn:aws:iam::12345:role/svc"),
        ("a role with no name", "arn:aws:iam::000000000000:role/"),
        (
            "an IAM ARN naming a Region",
            "arn:aws:iam:eu-west-2:000000000000:role/svc",
        ),
        ("a partition outside the published set", "arn:acme:iam::000000000000:role/svc"),
        ("a value that is only a space", " "),
    ];

    let logs = Logs::new();
    seed_log_groups(&logs).await;
    // One stored query for the update cases to address, so a refusal cannot be the query's
    // absence.
    const STORED: &str = "orders-role-rules";
    logs.ok(CREATE, &create_body(STORED, json!({}))).await;
    let before = logs.ok(GET, &json!({"identifier": STORED})).await;

    let expected = execution_role_refusal();
    for (case, role) in refused {
        let create = create_body("orders-refused-role", json!({"executionRoleArn": role}));
        let (code, message) = logs.refusal(CREATE, &create).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?} on {CREATE}: expected {VALIDATION}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?} on {CREATE}: expected {expected:?}, got {message:?}"
        );

        let update = update_body(STORED, json!({"executionRoleArn": role}));
        let (code, message) = logs.refusal(UPDATE, &update).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?} on {UPDATE}: expected {VALIDATION}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?} on {UPDATE}: expected {expected:?}, got {message:?}"
        );
    }

    // A refused write stores nothing and writes nothing: the account holds the one query it
    // held, exactly as it was.
    let (code, message) = logs
        .refusal(GET, &json!({"identifier": "orders-refused-role"}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"a refused create\": the refused create stored no query, got {code} {message:?}"
    );
    let after = logs.ok(GET, &json!({"identifier": STORED})).await;
    assert_eq!(
        after, before,
        "case \"a refused update\": the refused updates wrote nothing, expected {before}, got \
         {after}"
    );
}

/// Every IAM role ARN the published expression admits is accepted, in each partition it names.
#[tokio::test]
async fn an_iam_role_arn_of_each_published_partition_is_accepted() {
    // The partitions the expression names: `aws`, `aws-cn`, `aws-us-gov` and the ISO partitions
    // its `aws-iso-{0,1}[a-z]{0,1}` alternative covers — plus a role under a path, which the
    // expression's `role\/(.+)` admits. Source: the expression AWS publishes for this member,
    // stated in the refusal above.
    let accepted: [(&str, &str); 6] = [
        ("the standard partition", "arn:aws:iam::000000000000:role/svc"),
        ("the China partition", "arn:aws-cn:iam::000000000000:role/svc"),
        ("the GovCloud partition", "arn:aws-us-gov:iam::000000000000:role/svc"),
        ("an ISO partition", "arn:aws-iso:iam::000000000000:role/svc"),
        ("an ISO-B partition", "arn:aws-iso-b:iam::000000000000:role/svc"),
        (
            "a role under a path",
            "arn:aws:iam::000000000000:role/service-role/svc",
        ),
    ];

    for (index, (case, role)) in accepted.into_iter().enumerate() {
        let logs = Logs::new();
        let name = format!("orders-role-{index}");
        logs.ok(CREATE, &create_body(&name, json!({"executionRoleArn": role})))
            .await;
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        let reported = read.get("executionRoleArn").and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(role),
            "case {case:?}: the stored query executes under the role the create named, got {read}"
        );

        // The same value is accepted by the write that re-states the whole configuration.
        logs.ok(UPDATE, &update_body(&name, json!({"executionRoleArn": role})))
            .await;
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        let reported = read.get("executionRoleArn").and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(role),
            "case {case:?} on {UPDATE}: the update keeps the role it stated, got {read}"
        );
    }
}

/// A log group the account does not hold refuses the write that names it, about that log group —
/// whichever form of the identifier named it, wherever in the list it sits.
#[tokio::test]
async fn a_log_group_the_account_does_not_hold_refuses_the_write() {
    const MISSING: &str = "/aws/lambda/absent";

    // (case, the identifiers the write names, the log group the refusal must name): the forms a
    // `logGroupIdentifiers` entry takes — a name, a log-group ARN of this Region, one of another
    // Region — and the positions a missing one takes in a list of one, two and three.
    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        ("the only identifier, by name", vec![MISSING.to_string()], MISSING),
        (
            "the only identifier, by ARN",
            vec![log_group_arn(REGION, MISSING, false)],
            MISSING,
        ),
        (
            "the only identifier, by an ARN naming another Region",
            vec![log_group_arn("us-east-1", MISSING, false)],
            MISSING,
        ),
        (
            "the first of two identifiers",
            vec![MISSING.to_string(), GROUP.to_string()],
            MISSING,
        ),
        (
            "the last of two identifiers",
            vec![GROUP.to_string(), MISSING.to_string()],
            MISSING,
        ),
        (
            "the middle of three identifiers",
            vec![GROUP.to_string(), MISSING.to_string(), OTHER_GROUP.to_string()],
            MISSING,
        ),
        (
            "the first missing of two missing identifiers",
            vec![MISSING.to_string(), "/aws/lambda/gone".to_string()],
            MISSING,
        ),
    ];

    let logs = Logs::new();
    seed_log_groups(&logs).await;
    const STORED: &str = "orders-group-rules";
    logs.ok(
        CREATE,
        &create_body(STORED, json!({"logGroupIdentifiers": [GROUP]})),
    )
    .await;
    let before = logs.ok(GET, &json!({"identifier": STORED})).await;

    for (case, identifiers, named) in cases {
        let expected = log_group_refusal(named);

        let create = create_body(
            "orders-refused-group",
            json!({"logGroupIdentifiers": identifiers}),
        );
        let (code, said) = logs.refusal(CREATE, &create).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?} on {CREATE}: expected {NOT_FOUND}, got {code} {said:?}"
        );
        let sentence = without_sdk_envelope(case, &said, REFUSAL_STATUS, NOT_FOUND);
        assert_eq!(
            sentence, expected,
            "case {case:?} on {CREATE}: expected {expected:?}, got {sentence:?}"
        );

        let update = update_body(STORED, json!({"logGroupIdentifiers": identifiers}));
        let (code, said) = logs.refusal(UPDATE, &update).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?} on {UPDATE}: expected {NOT_FOUND}, got {code} {said:?}"
        );
        let sentence = without_sdk_envelope(case, &said, REFUSAL_STATUS, NOT_FOUND);
        assert_eq!(
            sentence, expected,
            "case {case:?} on {UPDATE}: expected {expected:?}, got {sentence:?}"
        );
    }

    // A refused write stores nothing and writes nothing.
    let (code, message) = logs
        .refusal(GET, &json!({"identifier": "orders-refused-group"}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"a refused create\": the refused create stored no query, got {code} {message:?}"
    );
    let after = logs.ok(GET, &json!({"identifier": STORED})).await;
    assert_eq!(
        after, before,
        "case \"a refused update\": the refused updates wrote nothing, expected {before}, got \
         {after}"
    );
}

/// The missing log group is resolved BEFORE the scheduled query the identifier names, so a
/// request naming a log group that is not there is refused about that log group whether or not
/// the query it addresses exists.
#[tokio::test]
async fn the_missing_log_group_is_refused_ahead_of_the_scheduled_query() {
    const MISSING_GROUP: &str = "/aws/lambda/absent";

    // (case, the identifier the update addresses): a query the account holds, a name it holds
    // none under, the ARN form of such a name, and a value that is neither.
    let logs = Logs::new();
    seed_log_groups(&logs).await;
    const STORED: &str = "orders-order-of-checks";
    logs.ok(CREATE, &create_body(STORED, json!({}))).await;

    let cases: Vec<(&str, String)> = vec![
        ("a query the account holds", STORED.to_string()),
        ("a name the account holds no query under", "orders-missing".to_string()),
        (
            "the ARN form of such a name",
            scheduled_query_arn(REGION, "orders-missing"),
        ),
        ("a value that is neither a name nor an ARN", "?".to_string()),
    ];

    let expected = log_group_refusal(MISSING_GROUP);
    for (case, identifier) in cases {
        let (code, said) = logs
            .refusal(
                UPDATE,
                &update_body(
                    &identifier,
                    json!({"logGroupIdentifiers": [MISSING_GROUP]}),
                ),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {said:?}"
        );
        let sentence = without_sdk_envelope(case, &said, REFUSAL_STATUS, NOT_FOUND);
        assert_eq!(
            sentence, expected,
            "case {case:?}: the refusal is about the missing log group, expected {expected:?}, \
             got {sentence:?}"
        );
    }
}

/// `endTimeOffset` has an effective value whether or not the write states one: every reader of
/// the query reports the offset it resolves to, and the update's own response reports it too.
#[tokio::test]
async fn the_end_offset_is_reported_whether_or_not_the_write_states_one() {
    // (case, the offset the write states, the offset every reader must report): a write that
    // states none, one that states the default itself, and ones that state a window of seconds,
    // an hour and a day.
    let cases: [(&str, Option<i64>, i64); 5] = [
        ("no offset at all", None, DEFAULT_END_TIME_OFFSET),
        ("the default stated explicitly", Some(0), 0),
        ("a window ending 60 seconds back", Some(60), 60),
        ("a window ending an hour back", Some(3_600), 3_600),
        ("a window ending a day back", Some(86_400), 86_400),
    ];

    for (index, (case, stated, expected)) in cases.into_iter().enumerate() {
        let logs = Logs::new();
        seed_log_groups(&logs).await;
        let name = format!("orders-end-offset-{index}");

        let extras = match stated {
            Some(offset) => json!({"endTimeOffset": offset, "logGroupIdentifiers": [GROUP]}),
            None => json!({"logGroupIdentifiers": [GROUP]}),
        };
        logs.ok(CREATE, &create_body(&name, extras.clone())).await;

        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            number(&read, "endTimeOffset"),
            Some(expected),
            "case {case:?} after {CREATE}: expected endTimeOffset {expected}, got {read}"
        );

        // The update re-states the whole configuration, so it resolves the member the same way
        // and its own response reports what it stored.
        let updated = logs.ok(UPDATE, &update_body(&name, extras)).await;
        assert_eq!(
            number(&updated, "endTimeOffset"),
            Some(expected),
            "case {case:?} in the {UPDATE} response: expected endTimeOffset {expected}, got \
             {updated}"
        );
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            number(&read, "endTimeOffset"),
            Some(expected),
            "case {case:?} after {UPDATE}: expected endTimeOffset {expected}, got {read}"
        );

        // An update that states no offset resolves it the same way, whatever the create stored.
        let cleared = logs.ok(UPDATE, &update_body(&name, json!({}))).await;
        assert_eq!(
            number(&cleared, "endTimeOffset"),
            Some(DEFAULT_END_TIME_OFFSET),
            "case {case:?}: an update stating no offset resolves to {DEFAULT_END_TIME_OFFSET}, \
             got {cleared}"
        );
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            number(&read, "endTimeOffset"),
            Some(DEFAULT_END_TIME_OFFSET),
            "case {case:?}: the stored query carries the resolved offset, got {read}"
        );
    }
}
