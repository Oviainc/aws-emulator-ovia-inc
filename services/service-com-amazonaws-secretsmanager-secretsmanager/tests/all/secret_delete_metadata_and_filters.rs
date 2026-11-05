// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a scheduled deletion does to a secret, what a
//! forced delete of a secret the account never held answers, and which secrets
//! `ListSecrets` filters select, driven over the wire through the multi-service
//! router.
//!
//! The rules under test:
//!
//! * scheduling a secret for deletion turns rotation OFF — `RotationEnabled`
//!   reads back false — and `RestoreSecret`, which cancels the deletion and
//!   nothing else, leaves it off;
//! * `DeleteSecret(ForceDeleteWithoutRecovery=true)` on a name the account holds
//!   no secret for succeeds idempotently and answers for the secret the request
//!   NAMED — ARN, Name and DeletionDate — while creating nothing; without the
//!   flag the same call is `ResourceNotFoundException`;
//! * a `ListSecrets` filter selects by a prefix of what its key names — of any
//!   WORD of it, and ignoring case, for the two keys that search text
//!   (`description` and `all`) — its plain values are alternatives, each of its
//!   `!`-prefixed values excludes what it matches, and `all` requires every word
//!   of one value;
//! * an empty `Description` describes nothing, so it is indistinguishable on
//!   read-back from a create or an update that named none.
//!
//! Every test is a TABLE over the class its rule generalises: the recovery window
//! and the rotation schedule, the form of the `SecretId` and the number of
//! repeats, the filter key and value and the case it is written in, and which
//! operation set the description.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    MAX_RECOVERY_WINDOW_DAYS, MIN_RECOVERY_WINDOW_DAYS,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below signs for, and so the Region every secret it
/// creates is homed in.
const REGION: &str = "us-west-2";

/// The ARN of a Lambda rotation function, as `RotationLambdaARN` carries it.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:RotateMySecret";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/secretsmanager/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail, and return its wire error code.
async fn err(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> String {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// Every secret `ListSecrets` reports for one filter, by name, in the order it
/// reports them.
async fn filtered(router: &MultiServiceRouter, filters: Value, case: &str) -> Vec<String> {
    let listed = ok(router, "ListSecrets", json!({"Filters": filters}), case).await;
    listed
        .get("SecretList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
        .iter()
        .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_recoverable_delete_turns_rotation_off_and_restore_leaves_it_off() {
    // The rows run over the recovery window the delete names — absent, and both
    // published bounds of `RecoveryWindowInDays` ("The number of days from 7 to
    // 30 that Secrets Manager waits", this service's Smithy model) — and over the
    // form of the rotation schedule that was turned on, since the rule is about
    // the deletion rather than about which schedule rotation was on.
    let windows: &[(&str, Option<i64>)] = &[
        ("the default window", None),
        ("the shortest window", Some(MIN_RECOVERY_WINDOW_DAYS)),
        ("the longest window", Some(MAX_RECOVERY_WINDOW_DAYS)),
    ];
    let schedules: &[(&str, Value)] = &[
        ("an interval in days", json!({"AutomaticallyAfterDays": 10})),
        ("a rate() expression", json!({"ScheduleExpression": "rate(12 hours)"})),
        (
            "a cron() expression",
            json!({"ScheduleExpression": "cron(0 8 1 * ? *)"}),
        ),
    ];
    for (window_index, (window_case, window)) in windows.iter().enumerate() {
        for (schedule_index, (schedule_case, rules)) in schedules.iter().enumerate() {
            let case = format!("{window_case} over {schedule_case}");
            let router = emulator();
            let name = format!("rotating-{window_index}-{schedule_index}");
            ok(
                &router,
                "CreateSecret",
                json!({"Name": &name, "SecretString": "s3cret"}),
                &case,
            )
            .await;
            ok(
                &router,
                "RotateSecret",
                json!({
                    "SecretId": &name,
                    "RotationLambdaARN": LAMBDA_ARN,
                    "RotationRules": rules,
                    "RotateImmediately": false,
                }),
                &case,
            )
            .await;
            let rotating = ok(&router, "DescribeSecret", json!({"SecretId": &name}), &case).await;
            assert_eq!(
                rotating.get("RotationEnabled").and_then(Value::as_bool),
                Some(true),
                "case {case:?}: expected rotation to be on before the delete, got {rotating}"
            );

            let mut delete = json!({"SecretId": &name});
            if let Some(window) = window {
                delete["RecoveryWindowInDays"] = json!(window);
            }
            ok(&router, "DeleteSecret", delete, &case).await;
            let deleted = ok(&router, "DescribeSecret", json!({"SecretId": &name}), &case).await;
            assert_eq!(
                deleted.get("RotationEnabled").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: a secret scheduled for deletion is not rotated, got {deleted}"
            );
            assert!(
                deleted.get("NextRotationDate").is_none(),
                "case {case:?}: a secret scheduled for deletion has no rotation due, got \
                 {deleted}"
            );
            // The configuration itself is kept, so the caller can turn rotation
            // back on with RotateSecret.
            assert_eq!(
                deleted.get("RotationLambdaARN").and_then(Value::as_str),
                Some(LAMBDA_ARN),
                "case {case:?}: the rotation configuration is kept, got {deleted}"
            );

            ok(&router, "RestoreSecret", json!({"SecretId": &name}), &case).await;
            let restored = ok(&router, "DescribeSecret", json!({"SecretId": &name}), &case).await;
            assert!(
                restored.get("DeletedDate").is_none(),
                "case {case:?}: the restore cancels the deletion, got {restored}"
            );
            assert_eq!(
                restored.get("RotationEnabled").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: the restore does not turn rotation back on, got {restored}"
            );

            // And asking for it explicitly does.
            ok(
                &router,
                "RotateSecret",
                json!({"SecretId": &name, "RotateImmediately": false}),
                &case,
            )
            .await;
            let again = ok(&router, "DescribeSecret", json!({"SecretId": &name}), &case).await;
            assert_eq!(
                again.get("RotationEnabled").and_then(Value::as_bool),
                Some(true),
                "case {case:?}: expected rotation to be back on once asked for, got {again}"
            );
        }
    }
}

#[tokio::test]
async fn forcing_the_delete_of_a_secret_the_account_never_held_answers_for_the_name() {
    // The rows run over the form of the `SecretId` the call names — "The ARN or
    // name of the secret to delete" (`DeleteSecretRequest$SecretId`, this
    // service's Smithy model), plus the partial ARN every `SecretId` accepts —
    // and over the charset and shape of the name inside it.
    let account = engine::EMULATOR_ACCOUNT_ID;
    let rows: &[(&str, String, &str)] = &[
        ("a plain name", "never-created".to_string(), "never-created"),
        (
            "a path-like name",
            "team/prod/db-password".to_string(),
            "team/prod/db-password",
        ),
        (
            "a long mixed-charset name",
            "Prod_Secret.With-Mixed+Chars=1234567890".to_string(),
            "Prod_Secret.With-Mixed+Chars=1234567890",
        ),
        (
            "a full ARN",
            format!("arn:aws:secretsmanager:{REGION}:{account}:secret:absent-by-arn-AbCdEf"),
            "absent-by-arn",
        ),
        (
            "a partial ARN",
            format!("arn:aws:secretsmanager:{REGION}:{account}:secret:absent-by-partial"),
            "absent-by-partial",
        ),
    ];
    let router = emulator();
    for (case, secret_id, expected_name) in rows {
        // Without the flag the same call reports the secret missing.
        let code = err(
            &router,
            "DeleteSecret",
            json!({"SecretId": secret_id}),
            case,
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected a delete with no force flag to report the secret missing, \
             got {code}"
        );

        // "If you forcibly delete an already deleted or nonexistent secret, the
        // operation does not return ResourceNotFoundException" — and it is
        // idempotent, so a repeat answers the same way.
        for attempt in 1..=3 {
            let forced = ok(
                &router,
                "DeleteSecret",
                json!({"SecretId": secret_id, "ForceDeleteWithoutRecovery": true}),
                case,
            )
            .await;
            assert_eq!(
                text(&forced, "Name", case),
                *expected_name,
                "case {case:?}: forced delete #{attempt} answered the wrong Name"
            );
            let arn = text(&forced, "ARN", case);
            let prefix = format!("arn:aws:secretsmanager:{REGION}:{account}:secret:{expected_name}-");
            assert!(
                arn.starts_with(&prefix),
                "case {case:?}: forced delete #{attempt} answered {arn:?}, expected an ARN \
                 starting {prefix:?}"
            );
            assert_eq!(
                arn.len(),
                prefix.len() + 6,
                "case {case:?}: forced delete #{attempt} answered {arn:?}, expected the name \
                 plus a hyphen and six characters"
            );
            assert!(
                forced.get("DeletionDate").and_then(Value::as_f64).is_some(),
                "case {case:?}: forced delete #{attempt} answered no DeletionDate, got {forced}"
            );
        }
    }

    // And none of it created anything.
    let listed = ok(
        &router,
        "ListSecrets",
        json!({"IncludePlannedDeletion": true}),
        "after forcing the delete of secrets that were never created",
    )
    .await;
    assert_eq!(
        listed
            .get("SecretList")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "expected the forced deletes to have created nothing, got {listed}"
    );
}

#[tokio::test]
async fn list_secrets_filters_select_by_a_prefix_of_a_word() {
    let router = emulator();
    // Three secrets whose descriptions and tags carry the words the rows search
    // for in several positions and several cases.
    for (name, description, tag) in [
        (
            "svc/payments",
            "Payment gateway credentials",
            ("env", "prod-payments"),
        ),
        ("svc/login", "LOGIN service session key", ("env", "staging")),
        ("svc/cache", "staging cache", ("team", "platform")),
    ] {
        ok(
            &router,
            "CreateSecret",
            json!({
                "Name": name,
                "SecretString": "v",
                "Description": description,
                "Tags": [{"Key": tag.0, "Value": tag.1}],
            }),
            name,
        )
        .await;
    }

    // The rows run over the filter key, the position of the matched word, the
    // case it is written in, how many words one value carries and whether it is
    // negated. The expected sets are in the name order `ListSecrets` pages in.
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "description, the first word",
            "description",
            &["Payment"],
            &["svc/payments"],
        ),
        (
            "description, a later word",
            "description",
            &["gateway"],
            &["svc/payments"],
        ),
        (
            "description, a later word in another case",
            "description",
            &["GATEWAY"],
            &["svc/payments"],
        ),
        (
            "description, a prefix of a later word",
            "description",
            &["creden"],
            &["svc/payments"],
        ),
        (
            "description, a word two secrets carry",
            "description",
            &["staging"],
            &["svc/cache"],
        ),
        (
            "description, nothing carries",
            "description",
            &["absent"],
            &[],
        ),
        (
            "all, over the name",
            "all",
            &["login"],
            &["svc/login"],
        ),
        (
            "all, over the description in another case",
            "all",
            &["GATEWAY"],
            &["svc/payments"],
        ),
        (
            "all, over a tag value",
            "all",
            &["platform"],
            &["svc/cache"],
        ),
        (
            "all, two words one secret carries between its attributes",
            "all",
            &["payment payments"],
            &["svc/payments"],
        ),
        (
            "all, two words no secret carries together",
            "all",
            &["payment login"],
            &[],
        ),
        (
            "description, two alternatives",
            "description",
            &["payment", "login"],
            &["svc/login", "svc/payments"],
        ),
        (
            "description, one negated value",
            "description",
            &["!staging"],
            &["svc/login", "svc/payments"],
        ),
        (
            "all, one negated value",
            "all",
            &["!payment"],
            &["svc/cache", "svc/login"],
        ),
        (
            "description, two negated values",
            "description",
            &["!staging", "!payment"],
            &["svc/login"],
        ),
        (
            "description, an alternative narrowed by a negation",
            "description",
            &["session", "!staging"],
            &["svc/login"],
        ),
        (
            "name, a prefix of the whole name",
            "name",
            &["svc/pay"],
            &["svc/payments"],
        ),
        // "name: Prefix match, case-sensitive." (`Filter$Key`.)
        ("name, in another case", "name", &["SVC/PAY"], &[]),
    ];
    for (case, key, values, expected) in rows {
        let selected = filtered(
            &router,
            json!([{"Key": key, "Values": values}]),
            case,
        )
        .await;
        assert_eq!(
            selected,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: filter {key}={values:?} selected the wrong set"
        );
    }

    // Separate filters all have to hold.
    let both = filtered(
        &router,
        json!([
            {"Key": "description", "Values": ["staging"]},
            {"Key": "tag-key", "Values": ["team"]},
        ]),
        "two filters",
    )
    .await;
    assert_eq!(
        both,
        vec!["svc/cache".to_string()],
        "expected both filters to have to hold"
    );
}

#[tokio::test]
async fn an_empty_description_is_the_same_as_none_at_all() {
    // The rows run over the operation that set the description and the value it
    // carried: `DescriptionType` is modelled with a minimum length of 0, so the
    // empty string is accepted on the wire — and it describes nothing.
    let router = emulator();
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        ("an absent Description", None, None),
        ("an empty Description", Some(""), None),
        ("a Description of one space", Some(" "), Some(" ")),
        ("a Description with text", Some("described"), Some("described")),
    ];
    for (index, (case, sent, expected)) in rows.iter().enumerate() {
        let name = format!("described/{index}");
        let mut create = json!({"Name": &name, "SecretString": "v"});
        if let Some(sent) = sent {
            create["Description"] = json!(sent);
        }
        let created = ok(&router, "CreateSecret", create, case).await;
        assert_eq!(
            text(&created, "Name", case),
            name,
            "case {case:?}: expected the create to answer for the secret it created"
        );
        let described = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            *expected,
            "case {case:?}: expected Description {expected:?} on read-back, got {described}"
        );

        // And the same value through UpdateSecret reads back the same way.
        let updated_name = format!("described/updated/{index}");
        ok(
            &router,
            "CreateSecret",
            json!({"Name": &updated_name, "SecretString": "v", "Description": "to be replaced"}),
            case,
        )
        .await;
        if let Some(sent) = sent {
            ok(
                &router,
                "UpdateSecret",
                json!({"SecretId": &updated_name, "Description": sent}),
                case,
            )
            .await;
            let updated = ok(
                &router,
                "DescribeSecret",
                json!({"SecretId": &updated_name}),
                case,
            )
            .await;
            assert_eq!(
                updated.get("Description").and_then(Value::as_str),
                *expected,
                "case {case:?}: expected Description {expected:?} after the update, got {updated}"
            );
        }
    }

    // The two creates that describe nothing are indistinguishable on read-back:
    // a client diffing them sees no change.
    let absent = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "described/0"}),
        "absent against empty",
    )
    .await;
    let empty = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": "described/1"}),
        "absent against empty",
    )
    .await;
    assert_eq!(
        absent.get("Description"),
        empty.get("Description"),
        "expected an empty Description and an absent one to read back alike, got {absent} and \
         {empty}"
    );
    assert!(
        empty.get("Description").is_none(),
        "expected no Description key at all, got {empty}"
    );
}
