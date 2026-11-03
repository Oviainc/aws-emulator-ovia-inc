// SPDX-License-Identifier: CC0-1.0

//! `GetProvisionedLimit` and `UpdateProvisionedLimit` over the wire, driven
//! through the router exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: an account holds one request-rate limit per API
//! category, and a limit is identified by its class and the attributes of that
//! class (`LimitClass` `API_CATEGORY` plus the `Category` attribute).
//! `GetProvisionedLimit` reports the rate in force for the identified limit
//! beside the rate that category includes at no additional cost — the included
//! rate itself while nothing has adjusted the limit — and changes nothing.
//! `UpdateProvisionedLimit` stores any rate in the account's range, which runs
//! from the limit's included rate up to the account's maximum, and refuses a
//! rate outside it: below the included rate as a malformed parameter, above the
//! account maximum as a quota excess. A refused request stores nothing.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value. The harness is the one `log_delivery_configuration.rs` already
//! builds for this service, reached through the test crate root rather than
//! copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::log_delivery_configuration::{call, error_code, error_message, ok, router};

/// One API rate limit this account holds: the `Category` attribute that
/// identifies it and the rate it includes at no additional cost, in requests
/// per second.
///
/// Hand-written from this service's own catalogue of account limits
/// (`API_CATEGORIES` in `src/state.rs`, whose category names are the ones
/// `LimitDefinitionType.Attributes` and `UpdateProvisionedLimit` document in
/// the model and whose included rates are the published per-category
/// request-rate quotas), so that a change on either side has to be deliberate.
struct Limit {
    category: &'static str,
    included: i64,
}

/// Every limit this account holds, including categories no issue cited. Their
/// included rates differ, which is what the rule generalises over.
const LIMITS: &[Limit] = &[
    Limit {
        category: "UserAuthentication",
        included: 120,
    },
    Limit {
        category: "UserFederation",
        included: 25,
    },
    Limit {
        category: "UserCreation",
        included: 50,
    },
    Limit {
        category: "UserRead",
        included: 120,
    },
    Limit {
        category: "UserUpdate",
        included: 25,
    },
    Limit {
        category: "UserAccountRecovery",
        included: 30,
    },
];

/// The highest rate the account may provision for any one limit, in requests
/// per second: `MAX_PROVISIONED_API_RATE` in `src/state.rs`, which is the
/// account-level maximum `UpdateProvisionedLimit` documents rather than a
/// per-category number.
const ACCOUNT_MAXIMUM: i64 = 10_000;

/// The definition that identifies `category`'s API rate limit.
fn definition(category: &str) -> Value {
    json!({ "LimitClass": "API_CATEGORY", "Attributes": { "Category": category } })
}

/// The limit `GetProvisionedLimit` reports for `category`.
async fn read_back(router: &MultiServiceRouter, case: &str, category: &str) -> Value {
    let payload = ok(
        router,
        "GetProvisionedLimit",
        &json!({ "LimitDefinition": definition(category) }),
    )
    .await;
    payload
        .get("Limit")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the read must carry a limit: {payload}"))
}

/// Ask for `requested` requests per second on `category`'s limit.
async fn provision(router: &MultiServiceRouter, category: &str, requested: i64) -> (u16, Value) {
    call(
        router,
        "UpdateProvisionedLimit",
        &json!({
            "LimitDefinition": definition(category),
            "RequestedLimitValue": requested,
        }),
    )
    .await
}

/// The limit a reader renders for `category` once `provisioned` is in force.
fn rendered(limit: &Limit, provisioned: i64) -> Value {
    json!({
        "LimitDefinition": definition(limit.category),
        "ProvisionedLimitValue": provisioned,
        "FreeLimitValue": limit.included,
    })
}

#[tokio::test]
async fn a_limit_nothing_has_adjusted_reports_its_included_rate() {
    let router = router();

    for limit in LIMITS {
        let case = format!("a fresh account's {} limit", limit.category);
        let reported = read_back(&router, &case, limit.category).await;
        assert_eq!(
            reported,
            rendered(limit, limit.included),
            "case {case:?}: an unadjusted limit reports the rate it includes as the rate in \
             force, got {reported}"
        );

        // The read is a read: a second one answers the same limit.
        let again = read_back(&router, &case, limit.category).await;
        assert_eq!(
            again, reported,
            "case {case:?}: reading twice must answer the same limit, got {again}"
        );
    }
}

#[tokio::test]
async fn an_adjusted_rate_is_what_every_reader_renders() {
    let router = router();

    for limit in LIMITS {
        // The rows differ along where in the account's range the rate sits:
        // both bounds, rates inside it, the same rate twice, and a release back
        // to the included rate.
        let midpoint = (limit.included + ACCOUNT_MAXIMUM) / 2;
        let rows: Vec<(String, i64)> = vec![
            ("one above the included rate".to_string(), limit.included + 1),
            ("a rate inside the range".to_string(), midpoint),
            ("that same rate again".to_string(), midpoint),
            ("the account maximum".to_string(), ACCOUNT_MAXIMUM),
            ("back down to the included rate".to_string(), limit.included),
            ("the included rate again".to_string(), limit.included),
        ];

        for (position, requested) in rows {
            let case = format!("{} provisioned at {requested} ({position})", limit.category);
            let (status, answered) = provision(&router, limit.category, requested).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a rate inside the account's range is stored, got {status} \
                 with {answered}"
            );
            assert_eq!(
                answered.get("Limit"),
                Some(&rendered(limit, requested)),
                "case {case:?}: the update must answer the limit it stored, got {answered}"
            );

            // The stored rate is what the reader renders, so the adjustment
            // round-trips rather than living in the response alone.
            let reported = read_back(&router, &case, limit.category).await;
            assert_eq!(
                reported,
                rendered(limit, requested),
                "case {case:?}: the read must render the stored limit, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn each_limit_holds_its_own_provisioned_rate() {
    let router = router();

    // Two limits with different included rates, so provisioning one is not
    // provisioning the other.
    let (first, second) = (&LIMITS[0], &LIMITS[1]);
    let requested = ACCOUNT_MAXIMUM;

    let (status, answered) = provision(&router, first.category, requested).await;
    assert_eq!(
        status, 200,
        "case {:?}: provisioning one limit succeeds, got {status} with {answered}",
        first.category
    );

    let case = format!(
        "{} after {} was provisioned",
        second.category, first.category
    );
    let reported = read_back(&router, &case, second.category).await;
    assert_eq!(
        reported,
        rendered(second, second.included),
        "case {case:?}: provisioning another limit must leave this one at its included rate, \
         got {reported}"
    );

    let case = format!("{} itself", first.category);
    let reported = read_back(&router, &case, first.category).await;
    assert_eq!(
        reported,
        rendered(first, requested),
        "case {case:?}: the provisioned limit reports its own rate, got {reported}"
    );
}

#[tokio::test]
async fn a_rate_outside_the_accounts_range_is_refused_and_stores_nothing() {
    let router = router();

    for limit in LIMITS {
        // Each row is a rate outside the account's range for this limit, with
        // the error the bound it missed carries: below the included rate is a
        // malformed parameter, above the account maximum needs a quota
        // increase. Each bound is covered one step past it, plus no rate at
        // all, a negative rate, and a rate far above the maximum.
        let rows: Vec<(String, i64, &str, String)> = vec![
            (
                "one below the included rate".to_string(),
                limit.included - 1,
                "InvalidParameterException",
                below_included_message(limit.included - 1, limit.included),
            ),
            (
                "no rate at all".to_string(),
                0,
                "InvalidParameterException",
                below_included_message(0, limit.included),
            ),
            (
                "a negative rate".to_string(),
                -1,
                "InvalidParameterException",
                below_included_message(-1, limit.included),
            ),
            (
                "one above the account maximum".to_string(),
                ACCOUNT_MAXIMUM + 1,
                "ServiceQuotaExceededException",
                above_maximum_message(ACCOUNT_MAXIMUM + 1, ACCOUNT_MAXIMUM),
            ),
            (
                "far above the account maximum".to_string(),
                2_000_000_000,
                "ServiceQuotaExceededException",
                above_maximum_message(2_000_000_000, ACCOUNT_MAXIMUM),
            ),
        ];

        // A rate in force that a refusal has to leave alone, so the assertion
        // is about the write and not about an untouched default.
        let stored = limit.included + 1;
        let (status, answered) = provision(&router, limit.category, stored).await;
        assert_eq!(
            status, 200,
            "case {:?}: the rate every refusal must leave alone is stored first, got {status} \
             with {answered}",
            limit.category
        );
        let before = read_back(
            &router,
            &format!("{} before any refusal", limit.category),
            limit.category,
        )
        .await;

        for (position, requested, expected_code, expected_message) in rows {
            let case = format!("{} asked for {requested} ({position})", limit.category);
            let (status, payload) = provision(&router, limit.category, requested).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, expected_code),
                "case {case:?}: expected a 400 {expected_code}, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );

            // The refusal is judged before anything is written, so the limit
            // still reports the rate it held.
            let after = read_back(&router, &case, limit.category).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must leave the limit unchanged, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_definition_that_identifies_no_limit_is_refused_by_both_operations() {
    let router = router();

    // A definition that identifies no limit is a definition the request has to
    // correct, whichever part of it names nothing: a category value the service
    // publishes no limit of, and attributes that name no category at all, carry
    // the same class and differ only in the sentence.
    let rows: Vec<(String, Value, u16, &str, String)> = vec![
        (
            "a category the service publishes no limit of".to_string(),
            definition("UserDeletion"),
            400,
            "InvalidParameterException",
            unknown_category_message("UserDeletion"),
        ),
        (
            "a category differing only in case".to_string(),
            definition("usercreation"),
            400,
            "InvalidParameterException",
            unknown_category_message("usercreation"),
        ),
        (
            "the empty category".to_string(),
            definition(""),
            400,
            "InvalidParameterException",
            unknown_category_message(""),
        ),
        (
            "attributes that name no category".to_string(),
            json!({ "LimitClass": "API_CATEGORY", "Attributes": {} }),
            400,
            "InvalidParameterException",
            missing_attribute_message(),
        ),
        (
            "attributes naming something else".to_string(),
            json!({
                "LimitClass": "API_CATEGORY",
                "Attributes": { "UserPoolId": "us-east-1_example" },
            }),
            400,
            "InvalidParameterException",
            missing_attribute_message(),
        ),
    ];

    for (position, limit_definition, expected_status, expected_code, expected_message) in rows {
        for operation in ["GetProvisionedLimit", "UpdateProvisionedLimit"] {
            let case = format!("{operation} with {position}");
            let mut request = json!({ "LimitDefinition": limit_definition });
            if operation == "UpdateProvisionedLimit" {
                request["RequestedLimitValue"] = json!(1_000);
            }
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&payload)),
                (expected_status, expected_code),
                "case {case:?}: expected a {expected_status} {expected_code}, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            );
        }
    }
}

/// What a refusal says for a rate below the limit's included rate.
fn below_included_message(requested: i64, included: i64) -> String {
    format!(
        "The requested limit value {requested} is below the default limit of {included} \
         requests per second for this limit."
    )
}

/// What a refusal says for a rate above the account's maximum.
fn above_maximum_message(requested: i64, maximum: i64) -> String {
    format!(
        "The requested limit value {requested} exceeds your account's maximum limit of \
         {maximum} requests per second for this limit. To increase your limit, submit a \
         Service Quotas increase request."
    )
}

/// What a refusal says for a category value the service publishes no limit of:
/// the class the definition looked it up under and the value itself.
fn unknown_category_message(category: &str) -> String {
    format!("Unknown API_CATEGORY category: {category}")
}

/// What a refusal says for a definition that names no category.
fn missing_attribute_message() -> String {
    "The limit definition must include the attribute Category to identify the API_CATEGORY \
     limit it means."
        .to_string()
}
