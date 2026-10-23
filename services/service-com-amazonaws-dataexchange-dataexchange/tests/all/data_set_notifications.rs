// SPDX-License-Identifier: CC0-1.0

//! `SendDataSetNotification`: the `date-time` members of its body, and the refusals it
//! tells apart.
//!
//! Two rules are under test here.
//!
//! 1. A `Timestamp` member whose shape carries `@timestampFormat("date-time")` is read
//!    off the wire as an ISO-8601 STRING. Every timestamp this service's request bodies
//!    can carry - `Details.DataUpdate.DataUpdatedAt`,
//!    `Details.Deprecation.DeprecationAt` and `Details.SchemaChange.SchemaChangeAt` -
//!    targets `com.amazonaws.dataexchange#Timestamp`, which carries that trait
//!    (`smithy_query(query="shape", shape="Timestamp,DataUpdateRequestDetails,\
//!    DeprecationRequestDetails,SchemaChangeRequestDetails")`), so the rows below span
//!    all three members and several ISO-8601 instants rather than one recorded example.
//!
//! 2. The refusals this operation tells apart: a `DataSetId` that names no data set is
//!    the modelled `ResourceNotFoundException` (404), a data set that exists but is not
//!    part of an AWS Marketplace data product is the modelled `ValidationException`
//!    (400), and `Details` that do not describe the declared `Type` is a
//!    `ValidationException` too. A data set that IS part of a Marketplace product is
//!    notified: 202, recorded against the data set, idempotent on `ClientToken`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, harness, Answer};

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` (32 characters) but
/// never created.
const NEVER_CREATED: &str = "cccccccccccccccccccccccccccccccc";

/// The refusal AWS answers a notification about a data set that is not part of an AWS
/// Marketplace data product with, recorded in
/// `dataexchange.send-data-set-notification-error-semantics` and stated by
/// `crate::state::data_set_not_configured_for_marketplace`.
const NOT_CONFIGURED: &str = "Data set not configured for AWS Marketplace Data Product.";

/// ISO-8601 instants a client may send for a `date-time` member. Rows differ along the
/// form: whole seconds, sub-second precision, the epoch itself, and an instant far in the
/// future.
const DATE_TIMES: &[&str] = &[
    "2024-01-01T00:00:00Z",
    "2024-06-30T23:59:59.123Z",
    "1970-01-01T00:00:00Z",
    "2038-01-19T03:14:08Z",
];

/// One notification kind: the `Type` the request declares, and the `Details` member the
/// model declares for it, built around the instant `at`.
fn notification(notification_type: &str, at: &str) -> Value {
    let details = match notification_type {
        "DATA_UPDATE" => json!({ "DataUpdate": { "DataUpdatedAt": at } }),
        "DEPRECATION" => json!({ "Deprecation": { "DeprecationAt": at } }),
        "SCHEMA_CHANGE" => json!({
            "SchemaChange": {
                "SchemaChangeAt": at,
                "Changes": [{ "Name": "column-a", "Type": "ADD", "Description": "added" }],
            }
        }),
        other => panic!("case {other}: no detail shape is modelled for this Type"),
    };
    json!({ "Type": notification_type, "Details": details })
}

/// Send `body` as `SendDataSetNotification` for `data_set_id`.
async fn notify(router: &MultiServiceRouter, data_set_id: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/notification"),
        Some(body),
    )
    .await
}

/// The refusal message an answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Put `data_set_id` into an AWS Marketplace data product, the state
/// `SendDataSetNotification` reads to decide whether there is anyone to notify.
///
/// No operation this emulator implements publishes a data set to Marketplace (AWS does it
/// through the Marketplace Catalog APIs), so the test writes that state through the shared
/// engine - which is also what proves the handler's answer comes from the stored record
/// rather than from the request.
fn configure_for_marketplace(engine: &Arc<engine::InMemoryEngine>, data_set_id: &str) {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    let data_set = store
        .data_sets
        .get_mut(data_set_id)
        .expect("the data set the test just created");
    data_set.origin_details = Some(dataexchange::ops::types::OriginDetails {
        product_id: Some("prod-a1b2c3d4e5f6g".to_string()),
        data_grant_id: None,
    });
}

/// How many notifications state holds for `data_set_id`, and the `Type` of each.
fn recorded_notifications(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
) -> Vec<(String, Option<String>)> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .data_sets
        .get(data_set_id)
        .map(|data_set| {
            data_set
                .notifications
                .iter()
                .map(|sent| (sent.r#type.clone(), sent.client_token.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a `date-time` timestamp member is read off the wire in the modelled ISO-8601
/// form - for every such member a notification body can carry, whatever the instant.
#[tokio::test]
async fn date_time_members_are_read_as_iso_8601_strings() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "notified", "n", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    configure_for_marketplace(&engine, &data_set_id);

    for notification_type in ["DATA_UPDATE", "DEPRECATION", "SCHEMA_CHANGE"] {
        for at in DATE_TIMES {
            let case = format!("{notification_type} at {at}");
            let answer = notify(
                &router,
                &data_set_id,
                notification(notification_type, at),
            )
            .await;
            assert_eq!(
                answer.status, 202,
                "case {case}: expected the modelled 202, got {} body {:?}",
                answer.status, answer.body
            );
            assert!(
                !message(&answer).contains("timestamp"),
                "case {case}: expected the ISO-8601 instant to be read, got {:?}",
                answer.body
            );
        }
    }

    // Every one of those calls was accepted and recorded.
    let recorded = recorded_notifications(&engine, &data_set_id);
    assert_eq!(
        recorded.len(),
        3 * DATE_TIMES.len(),
        "case recorded: expected one record per accepted notification, got {recorded:?}"
    );
}

/// RULE: a timestamp that is NOT a modelled `date-time` instant is refused as a 400 -
/// the member is read in one form, so anything outside it is a bad request rather than a
/// silently different instant.
#[tokio::test]
async fn a_malformed_date_time_is_refused() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "malformed", "m", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    configure_for_marketplace(&engine, &data_set_id);

    // Rows differ along HOW the value is not an instant: prose, a date with no time, a
    // time with no date, and an empty string.
    for value in [
        "not-a-timestamp",
        "2024-01-01",
        "00:00:00Z",
        "",
    ] {
        let answer = notify(
            &router,
            &data_set_id,
            json!({
                "Type": "DATA_UPDATE",
                "Details": { "DataUpdate": { "DataUpdatedAt": value } },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case DataUpdatedAt={value:?}: expected 400, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case DataUpdatedAt={value:?}: expected ValidationException, got {:?}",
            answer.error
        );
    }
}

/// RULE: the operation resolves the data set first, so an id that names none is the
/// modelled 404 `ResourceNotFoundException`; a data set that exists but is not part of an
/// AWS Marketplace data product is the modelled 400 `ValidationException`, because a
/// notification goes to that product's subscribers.
#[tokio::test]
async fn an_unknown_data_set_is_a_404_and_an_unpublished_one_is_a_400() {
    let (router, engine) = harness();
    let unpublished = create_data_set(&router, "unpublished", "u", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let published = create_data_set(&router, "published", "p", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    configure_for_marketplace(&engine, &published);

    // Rows: the three states a `DataSetId` can be in, under the same well-formed body.
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a DataSetId that names no data set",
            NEVER_CREATED,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a data set outside any Marketplace product",
            &unpublished,
            400,
            "ValidationException",
        ),
        (
            "a data set in a Marketplace product",
            &published,
            202,
            "",
        ),
    ];

    for (case, data_set_id, status, error) in rows {
        let answer = notify(
            &router,
            data_set_id,
            notification("DATA_UPDATE", DATE_TIMES[0]),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} body {:?}",
            answer.status, answer.body
        );
        if error.is_empty() {
            assert!(
                answer.body.is_null() || answer.body == json!({}),
                "case {case}: expected no members on the 202, got {:?}",
                answer.body
            );
            continue;
        }
        assert_eq!(
            answer.error.as_deref(),
            Some(error),
            "case {case}: expected {error}, got {:?}",
            answer.error
        );
    }

    // The unpublished data set's refusal states WHY, and says nothing about the page
    // size or the timestamp.
    let answer = notify(
        &router,
        &unpublished,
        notification("DATA_UPDATE", DATE_TIMES[0]),
    )
    .await;
    assert_eq!(
        message(&answer),
        NOT_CONFIGURED,
        "case unpublished: expected the not-configured refusal, got {:?}",
        answer.body
    );

    // Nothing was recorded against the data set that refused.
    assert!(
        recorded_notifications(&engine, &unpublished).is_empty(),
        "case unpublished: a refused notification must record nothing, got {:?}",
        recorded_notifications(&engine, &unpublished)
    );
}

/// RULE: `Details` must describe the `Type` the request declares - the detail member the
/// model declares for that type, and no other.
#[tokio::test]
async fn details_must_describe_the_declared_type() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "details", "d", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    configure_for_marketplace(&engine, &data_set_id);

    let at = DATE_TIMES[0];
    // Rows span the model's whole `NotificationType` value set against the detail
    // members it declares: each type with its own detail (accepted), each type with
    // another type's detail (refused), the two types whose detail carries a `@required`
    // member with no detail at all (refused), and `DATA_DELAY`, for which the model
    // declares no detail shape (accepted bare, refused with any detail).
    let rows: Vec<(&str, Value, u16)> = vec![
        ("DATA_UPDATE with its own detail", notification("DATA_UPDATE", at), 202),
        ("DEPRECATION with its own detail", notification("DEPRECATION", at), 202),
        ("SCHEMA_CHANGE with its own detail", notification("SCHEMA_CHANGE", at), 202),
        (
            "DATA_DELAY with no detail",
            json!({ "Type": "DATA_DELAY" }),
            202,
        ),
        (
            "DATA_UPDATE with no detail (its only member is optional)",
            json!({ "Type": "DATA_UPDATE" }),
            202,
        ),
        (
            "DATA_UPDATE carrying deprecation details",
            json!({ "Type": "DATA_UPDATE", "Details": { "Deprecation": { "DeprecationAt": at } } }),
            400,
        ),
        (
            "DEPRECATION carrying data-update details",
            json!({ "Type": "DEPRECATION", "Details": { "DataUpdate": { "DataUpdatedAt": at } } }),
            400,
        ),
        (
            "DATA_DELAY carrying a schema change",
            json!({
                "Type": "DATA_DELAY",
                "Details": { "SchemaChange": { "SchemaChangeAt": at } },
            }),
            400,
        ),
        (
            "DEPRECATION with no detail",
            json!({ "Type": "DEPRECATION" }),
            400,
        ),
        (
            "SCHEMA_CHANGE with no detail",
            json!({ "Type": "SCHEMA_CHANGE" }),
            400,
        ),
        (
            "a Type outside the modelled value set",
            json!({ "Type": "NOT_A_NOTIFICATION_TYPE" }),
            400,
        ),
    ];

    for (case, body, status) in rows {
        let answer = notify(&router, &data_set_id, body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} body {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {:?}",
                answer.error
            );
        }
    }

    // The model's own `NotificationType` catalog is the one the rows span.
    assert_eq!(
        dataexchange::ops::types::NOTIFICATION_TYPE_VALUES,
        ["DATA_DELAY", "DATA_UPDATE", "DEPRECATION", "SCHEMA_CHANGE"],
        "case NotificationType catalog: the rows above span the model's value set, which \
         has changed: {:?}",
        dataexchange::ops::types::NOTIFICATION_TYPE_VALUES
    );
}

/// RULE: `ClientToken` is `@idempotencyToken`, so a repeat under a token the data set has
/// already accepted is the SAME acceptance: it answers 202 again and records nothing
/// further, while a different token is a second notification.
#[tokio::test]
async fn a_repeat_under_one_client_token_is_one_acceptance() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "idempotent", "i", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    configure_for_marketplace(&engine, &data_set_id);

    // Rows: how many times each token is sent. Every send must answer 202; only the
    // first under a given token may record anything.
    let tokens = ["token-one", "token-two"];
    for token in tokens {
        for attempt in 0..3 {
            let mut body = notification("DATA_UPDATE", DATE_TIMES[0]);
            body["ClientToken"] = json!(token);
            body["Comment"] = json!("the same notification");
            let answer = notify(&router, &data_set_id, body).await;
            assert_eq!(
                answer.status, 202,
                "case {token} attempt {attempt}: expected 202, got {} body {:?}",
                answer.status, answer.body
            );
        }
    }

    let recorded = recorded_notifications(&engine, &data_set_id);
    let accepted_tokens: Vec<Option<String>> =
        recorded.iter().map(|(_, token)| token.clone()).collect();
    assert_eq!(
        accepted_tokens,
        tokens
            .iter()
            .map(|token| Some((*token).to_string()))
            .collect::<Vec<Option<String>>>(),
        "case idempotency: expected one acceptance per distinct ClientToken, got \
         {recorded:?}"
    );
}
