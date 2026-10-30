// SPDX-License-Identifier: CC0-1.0

//! `PutAppInstanceUserExpirationSettings`, end to end over HTTP.
//!
//! The RULE pinned here: a put of a user's expiration settings REPLACES whatever that
//! user carried with the settings in the request — and clears them when the request
//! carries none — answers 200 with the user's ARN and the settings now in effect, and
//! those settings are what every later read of the user reports.
//!
//! The tables span what the rule generalises over: the value of `ExpirationDays` (both
//! published bounds and values between them), every value `ExpirationCriterion` accepts,
//! whether the user was created with settings or without, and how many puts in a row a
//! user has had (set, replaced, cleared, set again).

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The published bounds of `ExpirationDays`.
///
/// Source: `smithy_query(query="shape",shape="ExpirationDays")` — an integer with
/// `range { min: 1, max: 5475 }`.
const MIN_EXPIRATION_DAYS: i64 = 1;
const MAX_EXPIRATION_DAYS: i64 = 5475;

/// Every value `ExpirationCriterion` accepts on the wire.
///
/// Source: `smithy_query(query="shape",shape="ExpirationCriterion")` — an enum whose
/// only value is `CREATED_TIMESTAMP`.
const CRITERIA: &[&str] = &["CREATED_TIMESTAMP"];

/// `PUT` the expiration-settings route of one user. `settings` is the `ExpirationSettings`
/// member to send, or `None` to send a body that carries none — the way a caller clears
/// them.
///
/// Source: `smithy_query(query="operation",operation="PutAppInstanceUserExpirationSettings")`
/// — `PUT /app-instance-users/{AppInstanceUserArn}/expiration-settings`, answering 200.
async fn put_expiration(
    emulator: &Emulator,
    user_arn: &str,
    settings: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(settings) = settings {
        body["ExpirationSettings"] = settings;
    }
    emulator
        .call(
            "PUT",
            &format!(
                "/app-instance-users/{}/expiration-settings",
                uri_encode(user_arn)
            ),
            Some(body),
        )
        .await
}

/// The `ExpirationSettings` member of a described user, or `Value::Null` when it carries
/// none.
async fn described_settings(emulator: &Emulator, user_arn: &str) -> (u16, Value) {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(user_arn)),
            None,
        )
        .await;
    let settings = body
        .get("AppInstanceUser")
        .and_then(|user| user.get("ExpirationSettings"))
        .cloned()
        .unwrap_or(Value::Null);
    (status, settings)
}

/// `ExpirationSettings` as the wire shape, from its two required members.
fn settings(days: i64, criterion: &str) -> Value {
    json!({ "ExpirationDays": days, "ExpirationCriterion": criterion })
}

/// Create a user, optionally with expiration settings already on it, and return its ARN.
async fn user_with(
    emulator: &Emulator,
    app_instance_arn: &str,
    user_id: &str,
    token: &str,
    initial: Option<Value>,
) -> String {
    let mut body = json!({
        "AppInstanceArn": app_instance_arn,
        "AppInstanceUserId": user_id,
        "Name": user_id,
        "ClientRequestToken": token,
    });
    if let Some(initial) = initial {
        body["ExpirationSettings"] = initial;
    }
    let (status, response) = emulator
        .call("POST", "/app-instance-users", Some(body))
        .await;
    assert_eq!(
        status, 201,
        "CreateAppInstanceUser({user_id}) answered: {response:?}"
    );
    response["AppInstanceUserArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAppInstanceUser({user_id}) returned no ARN: {response:?}"))
        .to_string()
}

/// One row: the settings a user is created with, then the settings each successive put
/// writes (`None` being a put that clears them).
struct Case {
    label: &'static str,
    initial: Option<Value>,
    puts: Vec<Option<Value>>,
}

fn cases() -> Vec<Case> {
    let criterion = CRITERIA[0];
    vec![
        Case {
            label: "set at the lower bound on a user created without settings",
            initial: None,
            puts: vec![Some(settings(MIN_EXPIRATION_DAYS, criterion))],
        },
        Case {
            label: "set at the upper bound on a user created without settings",
            initial: None,
            puts: vec![Some(settings(MAX_EXPIRATION_DAYS, criterion))],
        },
        Case {
            label: "replace the settings a create persisted",
            initial: Some(settings(30, criterion)),
            puts: vec![Some(settings(90, criterion))],
        },
        Case {
            label: "clear the settings a create persisted",
            initial: Some(settings(365, criterion)),
            puts: vec![None],
        },
        Case {
            label: "clearing a user that never had settings is still a clear",
            initial: None,
            puts: vec![None],
        },
        Case {
            label: "set, replace twice, clear, set again",
            initial: None,
            puts: vec![
                Some(settings(7, criterion)),
                Some(settings(MAX_EXPIRATION_DAYS, criterion)),
                Some(settings(1234, criterion)),
                None,
                Some(settings(MIN_EXPIRATION_DAYS, criterion)),
            ],
        },
        Case {
            label: "the same settings written twice",
            initial: None,
            puts: vec![Some(settings(42, criterion)), Some(settings(42, criterion))],
        },
    ]
}

/// RULE: a put replaces the named user's expiration settings with the request's, clears
/// them when the request carries none, answers with the user's ARN and the settings now
/// in effect, and every later read of the user reports exactly those settings.
#[tokio::test]
async fn a_put_replaces_the_users_expiration_settings_and_a_read_reports_them() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("expiration-parent", "tok-exp-parent")
        .await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let user = user_with(
            &emulator,
            &instance,
            &format!("u{index}"),
            &format!("tok-exp-{index}"),
            case.initial.clone(),
        )
        .await;

        // What the create persisted is what the user reads back with, before any put.
        let (status, before) = described_settings(&emulator, &user).await;
        assert_eq!(
            (status, &before),
            (200, &case.initial.clone().unwrap_or(Value::Null)),
            "case {label:?}: expected the created user to carry the settings it was \
             created with, got {status} {before:?}"
        );

        for (step, written) in case.puts.iter().enumerate() {
            let (status, body) = put_expiration(&emulator, &user, written.clone()).await;
            assert_eq!(
                status, 200,
                "case {label:?} step {step}: expected the put to answer 200, got \
                 {status} {body:?}"
            );
            assert_eq!(
                body["AppInstanceUserArn"].as_str(),
                Some(user.as_str()),
                "case {label:?} step {step}: expected the put to answer with {user:?}, \
                 got {body:?}"
            );
            let answered = body.get("ExpirationSettings").cloned().unwrap_or(Value::Null);
            let expected = written.clone().unwrap_or(Value::Null);
            assert_eq!(
                answered, expected,
                "case {label:?} step {step}: expected the put to answer with the \
                 settings now in effect {expected:?}, got {answered:?}"
            );

            let (status, read_back) = described_settings(&emulator, &user).await;
            assert_eq!(
                (status, &read_back),
                (200, &expected),
                "case {label:?} step {step}: expected a read of the user to report \
                 {expected:?}, got {status} {read_back:?}"
            );
        }
    }
}

/// RULE: every value `ExpirationCriterion` accepts is persisted and reported back
/// verbatim, for any `ExpirationDays` inside the published range; a value outside that
/// range, or a criterion the enum does not accept, is refused with a 4xx and leaves the
/// stored settings exactly as they were.
#[tokio::test]
async fn the_modelled_value_space_is_served_and_anything_outside_it_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("expiration-space", "tok-exp-space")
        .await;

    // Served: both published bounds, one day inside each, and a value between them —
    // crossed with every criterion the enum accepts.
    let served_days = [
        MIN_EXPIRATION_DAYS,
        MIN_EXPIRATION_DAYS + 1,
        730,
        MAX_EXPIRATION_DAYS - 1,
        MAX_EXPIRATION_DAYS,
    ];
    let user = user_with(&emulator, &instance, "spanned", "tok-exp-spanned", None).await;
    for criterion in CRITERIA {
        for days in served_days {
            let written = settings(days, criterion);
            let (status, body) = put_expiration(&emulator, &user, Some(written.clone())).await;
            assert_eq!(
                status, 200,
                "case {days} days / {criterion}: expected the put to be served, got \
                 {status} {body:?}"
            );
            let (status, read_back) = described_settings(&emulator, &user).await;
            assert_eq!(
                (status, &read_back),
                (200, &written),
                "case {days} days / {criterion}: expected the user to report \
                 {written:?}, got {status} {read_back:?}"
            );
        }
    }

    // The settings the user is left holding after the served rows: nothing refused below
    // may change them.
    let surviving = settings(MAX_EXPIRATION_DAYS, CRITERIA[0]);

    // Refused: one step past each published bound, and a criterion outside the enum.
    let refused: Vec<(String, Value)> = vec![
        (
            "one day below the lower bound".to_string(),
            settings(MIN_EXPIRATION_DAYS - 1, CRITERIA[0]),
        ),
        (
            "a negative number of days".to_string(),
            settings(-1, CRITERIA[0]),
        ),
        (
            "one day past the upper bound".to_string(),
            settings(MAX_EXPIRATION_DAYS + 1, CRITERIA[0]),
        ),
        (
            "a criterion the enum does not accept".to_string(),
            settings(30, "LAST_UPDATED_TIMESTAMP"),
        ),
        (
            "settings missing their required ExpirationDays".to_string(),
            json!({ "ExpirationCriterion": CRITERIA[0] }),
        ),
        (
            "settings missing their required ExpirationCriterion".to_string(),
            json!({ "ExpirationDays": 30 }),
        ),
    ];

    for (label, written) in refused {
        let (status, body) = put_expiration(&emulator, &user, Some(written.clone())).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected {written:?} to be refused with 400, got \
             {status} {body:?}"
        );
        let (status, read_back) = described_settings(&emulator, &user).await;
        assert_eq!(
            (status, &read_back),
            (200, &surviving),
            "case {label:?}: a refused put must leave {surviving:?} in place, got \
             {status} {read_back:?}"
        );
    }
}

/// RULE: a put keyed by an ARN this service does not hold is refused with the modelled
/// client error carrying the `NotFound` code, and writes nothing.
#[tokio::test]
async fn putting_expiration_settings_on_an_arn_the_service_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("expiration-absent", "tok-exp-absent")
        .await;
    let user = user_with(&emulator, &instance, "present", "tok-exp-present", None).await;
    let deleted = user_with(&emulator, &instance, "deleted", "tok-exp-deleted", None).await;
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/app-instance-users/{}", uri_encode(&deleted)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "DeleteAppInstanceUser answered: {status} {body:?}"
    );

    // Rows: an ARN of the right shape that names nothing, a user that has been deleted,
    // and the ARNs of resources of the OTHER kinds this service mints — none of them is
    // an `AppInstanceUser` this store holds.
    let rows: Vec<(&str, String)> = vec![
        ("a user that never existed", format!("{user}x")),
        ("a user that has been deleted", deleted.clone()),
        ("an app instance ARN in the user space", instance.clone()),
    ];

    for (label, arn) in rows {
        let (status, body) =
            put_expiration(&emulator, &arn, Some(settings(30, CRITERIA[0]))).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for {arn:?}, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("NotFound"),
            "case {label:?}: expected the NotFound code, got {body:?}"
        );
    }

    // The live user the refusals never named still carries no settings at all.
    let (status, read_back) = described_settings(&emulator, &user).await;
    assert_eq!(
        (status, &read_back),
        (200, &Value::Null),
        "expected the refusals to write nothing, got {status} {read_back:?}"
    );
}

/// RULE: writing a user's expiration settings writes the user, so its last-updated
/// instant moves to at or after the instant before the write while its created instant
/// never moves; a refused put moves neither.
#[tokio::test]
async fn a_put_stamps_the_user_and_a_refused_put_does_not() {
    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("expiration-stamp", "tok-exp-stamp")
        .await;
    let user = user_with(&emulator, &instance, "stamped", "tok-exp-stamped", None).await;

    async fn instants(emulator: &Emulator, arn: &str) -> (Option<f64>, Option<f64>) {
        let (status, body) = emulator
            .call(
                "GET",
                &format!("/app-instance-users/{}", uri_encode(arn)),
                None,
            )
            .await;
        assert_eq!(status, 200, "DescribeAppInstanceUser answered: {body:?}");
        let user = &body["AppInstanceUser"];
        (
            user["CreatedTimestamp"].as_f64(),
            user["LastUpdatedTimestamp"].as_f64(),
        )
    }

    let (created, mut stamped) = instants(&emulator, &user).await;
    assert!(
        created.is_some() && stamped.is_some(),
        "expected the created user to report both instants, got {created:?} / {stamped:?}"
    );

    // Each served put — setting, replacing and clearing — stamps the user again.
    for (step, written) in [
        Some(settings(10, CRITERIA[0])),
        Some(settings(20, CRITERIA[0])),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = put_expiration(&emulator, &user, written.clone()).await;
        assert_eq!(
            status, 200,
            "case step {step}: expected the put to answer 200, got {status} {body:?}"
        );
        let (now_created, now_stamped) = instants(&emulator, &user).await;
        assert_eq!(
            now_created, created,
            "case step {step}: a put must not move the created instant {created:?}, got \
             {now_created:?}"
        );
        assert!(
            now_stamped.is_some_and(|after| stamped.is_some_and(|before| after >= before)),
            "case step {step}: expected the last-updated instant to move to at or after \
             {stamped:?}, got {now_stamped:?}"
        );
        stamped = now_stamped;
    }

    // A refused put stamps nothing.
    let (status, body) =
        put_expiration(&emulator, &user, Some(settings(MAX_EXPIRATION_DAYS + 1, CRITERIA[0]))).await;
    assert_eq!(
        status, 400,
        "expected a put past the upper bound to be refused, got {status} {body:?}"
    );
    let (now_created, now_stamped) = instants(&emulator, &user).await;
    assert_eq!(
        (now_created, now_stamped),
        (created, stamped),
        "expected a refused put to move neither instant, got {now_created:?} / \
         {now_stamped:?}"
    );
}
