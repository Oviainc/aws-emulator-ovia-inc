// SPDX-License-Identifier: CC0-1.0

//! Wire probes of the log delivery surface: the modelled bound on a log delivery
//! collection.
//!
//! `LogConfigurationListType` is `len 0..2`
//! (`smithy_query(query="closure",operation="SetLogDeliveryConfiguration")`), which
//! is one entry per modelled `EventSourceName`. A collection past that bound is
//! refused by the modelled constraint before `SetLogDeliveryConfiguration`'s
//! handler runs, and the rule this suite exists for is the consequence: a refused
//! set writes nothing, so the pool still reports the collection it held.
//!
//! The refusal's envelope (`N validation error detected: …`) and its clause (the
//! member and the constraint it missed) are asserted; the rendering of the
//! rejected VALUE inside that envelope belongs to the framework's constraint
//! checker, which every service shares, so it is not pinned here.
//!
//! The harness is the one `log_delivery_configuration.rs` already builds for the
//! same surface, reached through the test crate root rather than copied.

use serde_json::{json, Value};

use super::log_delivery_configuration::{
    call, error_code, error_message, ok, pool, read_back, router, user_notification, LOG_GROUP_ARN,
};

#[tokio::test]
async fn a_collection_past_the_modelled_bound_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "a Plus pool", Some("PLUS")).await;

    // The collection the refusal has to leave alone.
    let stored = json!([user_notification(LOG_GROUP_ARN)]);
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": stored }),
    )
    .await;
    let before = read_back(&router, "the stored collection", &user_pool_id).await;

    // One row per size past the bound, so the refusal is the bound's and not one
    // particular request's.
    for count in [3usize, 4] {
        let case = format!("{count} log configurations");
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                user_notification(&format!(
                    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/{index}"
                ))
            })
            .collect();
        let (status, payload) = call(
            &router,
            "SetLogDeliveryConfiguration",
            &json!({ "UserPoolId": user_pool_id, "LogConfigurations": entries }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {case:?}: the refusal keeps the validation envelope, got {message:?}"
        );
        assert!(
            message.contains(
                "at 'logConfigurations' failed to satisfy constraint: Member must have length \
                 less than or equal to 2"
            ),
            "case {case:?}: the refusal names the member and the bound it missed, \
             got {message:?}"
        );

        // Refused before anything was written: the pool still reports what it
        // held.
        let after = read_back(&router, &case, &user_pool_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused set must leave the stored collection unchanged, got {after}"
        );
    }
}
