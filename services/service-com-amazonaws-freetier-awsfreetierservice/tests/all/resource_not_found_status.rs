// SPDX-License-Identifier: CC0-1.0

//! One rule about this service's not-found answers, driven over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`)
//! through the same multi-service router the emulator boots.
//!
//! Every operation of this service that raises the modelled
//! `ResourceNotFoundException` answers it with ONE status — the status this
//! service sends for that shape — while the body stays the modelled one
//! (`__type` `ResourceNotFoundException` plus `message`). The status a client
//! reads for a missing resource is therefore a property of the service and the
//! error shape, never of which operation was asked.
//!
//! The suite is a table over the whole class of ways to reach that condition:
//! both operations that can raise it, several forms of a well-formed identifier
//! that names nothing, and the request shaped with and without its optional
//! members. The companion test checks the restated status did not spill onto any
//! other outcome.

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::state::PLAN_TYPE_FREE;

use crate::free_tier::{
    call, error_code, harness, ok, refused, text, ACTIVITIES_PAGE_SIZE_CEILING,
};

/// The modelled error shape this suite is about, as it appears on the wire.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The status this service answers that shape with, from its recorded wire
/// behaviour (the model annotates the shape `@httpError 404`, and the service
/// sends 400 — see `src/scaffolded.rs`).
const NOT_FOUND_STATUS: u16 = 400;

/// One way of asking this service for a resource it does not hold.
struct Case {
    /// What makes this row different from the others.
    name: &'static str,
    /// Whether the caller established an account plan first — the one call that
    /// brings plan data into being.
    establish_plan: bool,
    operation: &'static str,
    body: Value,
}

/// Well-formed `ActivityId`s (exactly 32 of `[a-zA-Z0-9]`, as the model's
/// `ActivityId` length and pattern constraints require) that name no activity.
/// The rows differ in the character classes they use, so the answer cannot
/// depend on the shape of the identifier.
const UNKNOWN_ACTIVITY_IDS: [&str; 4] = [
    "00000000000000000000000000000000",
    "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
    "A1b2C3d4E5f60718293a4b5c6d7e8f90",
];

/// The activity ids the seeded catalog actually holds.
async fn catalog_ids(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let listed = ok(
        router,
        "ListAccountActivities",
        &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING}),
    )
    .await;
    listed["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|summary| text(summary, "activityId"))
        .collect()
}

/// Every row of the class is answered with the same status and the same modelled
/// body, whichever operation raised it.
#[tokio::test]
async fn one_status_answers_every_not_found_condition() {
    let mut cases = vec![
        // The plan reader: an account the store holds no plan record for.
        Case {
            name: "account plan state, account with no plan data",
            establish_plan: false,
            operation: "GetAccountPlanState",
            body: json!({}),
        },
        // The same reader asked twice over in one request shape it accepts: the
        // input models no members, so there is nothing else to vary.
        Case {
            name: "account plan state, body carrying an unmodelled member",
            establish_plan: false,
            operation: "GetAccountPlanState",
            body: json!({"ignored": "value"}),
        },
    ];
    // The activity reader, over the identifier forms and with the optional
    // `languageCode` both absent and set to a value the model declares.
    for activity_id in UNKNOWN_ACTIVITY_IDS {
        cases.push(Case {
            name: "activity detail, unknown id",
            establish_plan: false,
            operation: "GetAccountActivity",
            body: json!({"activityId": activity_id}),
        });
        cases.push(Case {
            name: "activity detail, unknown id with a languageCode",
            establish_plan: false,
            operation: "GetAccountActivity",
            body: json!({"activityId": activity_id, "languageCode": "ja-JP"}),
        });
        // And once with plan data in the store, so the answer cannot depend on
        // what else the account holds.
        cases.push(Case {
            name: "activity detail, unknown id with an established plan",
            establish_plan: true,
            operation: "GetAccountActivity",
            body: json!({"activityId": activity_id}),
        });
    }

    let mut statuses: Vec<(String, u16)> = Vec::new();
    for case in cases {
        let (router, _engine) = harness();
        if case.establish_plan {
            ok(
                &router,
                "UpgradeAccountPlan",
                &json!({"accountPlanType": PLAN_TYPE_FREE}),
            )
            .await;
        }
        if let Some(activity_id) = case.body["activityId"].as_str() {
            let known = catalog_ids(&router).await;
            assert!(
                !known.contains(&activity_id.to_string()),
                "case {:?}: the probe id {activity_id} must name no seeded activity",
                case.name
            );
        }

        let label = format!("{} {}", case.name, case.body);
        let (status, payload) = call(&router, case.operation, &case.body).await;
        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {label:?}: expected HTTP {NOT_FOUND_STATUS}, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            NOT_FOUND_CODE,
            "case {label:?}: expected the modelled not-found shape, got {payload}"
        );
        assert!(
            !text(&payload, "message").is_empty(),
            "case {label:?}: the not-found body carries a message, got {payload}"
        );
        statuses.push((label, status));
    }

    // The rule itself: one shape, one status across the service — asserted
    // without naming the number again, so a row answering any other status
    // fails here even if the expectation above were relaxed.
    let first = statuses.first().expect("the table has rows").1;
    for (label, status) in &statuses {
        assert_eq!(
            *status, first,
            "case {label:?}: every operation must answer this shape with one status, \
             got {status} beside {first}"
        );
    }
}

/// The restated status is for that one shape and nothing else: a resource the
/// store DOES hold is still a success, and a request refused for another reason
/// still carries that reason's own shape.
#[tokio::test]
async fn the_restated_status_does_not_reclassify_other_outcomes() {
    let (router, _engine) = harness();

    // Every seeded activity still reads back as a success.
    for activity_id in catalog_ids(&router).await {
        let detail = ok(&router, "GetAccountActivity", &json!({"activityId": activity_id})).await;
        assert_eq!(
            text(&detail, "activityId"),
            activity_id,
            "case {activity_id}: a held activity is reported, got {detail}"
        );
    }

    // An established plan reads back as a success, where the same read was
    // refused as not-found before the write.
    refused(
        &router,
        "GetAccountPlanState",
        &json!({}),
        NOT_FOUND_STATUS,
        NOT_FOUND_CODE,
    )
    .await;
    ok(
        &router,
        "UpgradeAccountPlan",
        &json!({"accountPlanType": PLAN_TYPE_FREE}),
    )
    .await;
    let reported = ok(&router, "GetAccountPlanState", &json!({})).await;
    assert_eq!(
        text(&reported, "accountPlanType"),
        PLAN_TYPE_FREE,
        "case established plan: the read reports the stored plan, got {reported}"
    );

    // Rows that miss the modelled `ActivityId` constraints are a different
    // claim, and keep their own error shape: a malformed id, and an optional
    // member outside its enum.
    let malformed: [(&str, Value); 4] = [
        ("too short", json!({"activityId": "short"})),
        (
            "one character short",
            json!({"activityId": "0000000000000000000000000000000"}),
        ),
        (
            "one character long",
            json!({"activityId": "000000000000000000000000000000000"}),
        ),
        (
            "an illegal character at length 32",
            json!({"activityId": "0000000000000000000000000000000-"}),
        ),
    ];
    for (case, body) in malformed {
        let (status, payload) = call(&router, "GetAccountActivity", &body).await;
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: a malformed id is a validation failure, got {status} with {payload}"
        );
    }

    let (status, payload) = call(
        &router,
        "GetAccountActivity",
        &json!({
            "activityId": UNKNOWN_ACTIVITY_IDS[0],
            "languageCode": "xx-XX",
        }),
    )
    .await;
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case a languageCode outside the enum: expected a validation failure, got {status} \
         with {payload}"
    );
}
