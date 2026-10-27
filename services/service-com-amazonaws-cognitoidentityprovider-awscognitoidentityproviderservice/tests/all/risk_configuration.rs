// SPDX-License-Identifier: CC0-1.0

//! `SetRiskConfiguration` and `DescribeRiskConfiguration` over the wire, driven
//! through the router exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `SetRiskConfiguration` SETS the whole threat-protection
//! configuration of the level its request names — the user pool, or the one app
//! client a `ClientId` names — so the stored settings are exactly the three the
//! request carries and a request that carries none leaves that level with no
//! settings at all; both that call's response and a later
//! `DescribeRiskConfiguration` of the same level render the STORED configuration,
//! identified by the level it is stored at, with the instant of the last call that
//! set it, while a level nothing has ever configured is reported by NO
//! `RiskConfiguration` member at all. The two levels are independent.
//!
//! The pool's threat-protection add-on gates exactly one thing: a write that
//! carries the compromised-credentials setting. The read is never gated by it, and
//! neither is a write that carries only the adaptive-authentication or IP-address
//! settings.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The Amazon SES identity a notification is sent from
/// (`NotifyConfigurationType.SourceArn`, an `ArnType`).
const SOURCE_ARN: &str = "arn:aws:ses:us-east-1:000000000000:identity/notify.example.com";

/// The published maximum of `BlockedIPRangeList` and `SkippedIPRangeList`
/// (`smithy_query(query="shape",shape="RiskExceptionConfigurationType")`:
/// length 0..200).
const MAX_IP_RANGES: usize = 200;

/// The message a `UserPoolAddOnNotEnabledException` carries.
const ADD_ONS_NOT_ENABLED_MESSAGE: &str = "User pool add-ons is not enabled.";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A pool created in the add-on mode `mode` names — or with no `UserPoolAddOns`
/// member at all when `mode` is `None` — and its minted id.
async fn pool_in_mode(router: &MultiServiceRouter, case: &str, mode: Option<&str>) -> String {
    let mut body = json!({ "PoolName": "risk-configuration-pool" });
    if let Some(mode) = mode {
        body.as_object_mut()
            .expect("a request body is a JSON object")
            .insert(
                "UserPoolAddOns".to_string(),
                json!({ "AdvancedSecurityMode": mode }),
            );
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {created}"))
        .to_string()
}

/// A pool whose threat protection is on, and its minted id. `mode` is one of the
/// modelled `AdvancedSecurityModeType` values.
async fn pool_with_add_ons(router: &MultiServiceRouter, case: &str, mode: &str) -> String {
    pool_in_mode(router, case, Some(mode)).await
}

/// A pool with threat protection enforced, and its minted id.
async fn pool(router: &MultiServiceRouter, case: &str) -> String {
    pool_with_add_ons(router, case, "ENFORCED").await
}

/// Every add-on state a user pool can be in, beside whether its add-ons are on.
///
/// `AdvancedSecurityModeType` is AUDIT | ENFORCED | OFF (this service's model:
/// `smithy_query(query="shape",shape="AdvancedSecurityModeType")`), and a pool
/// that stores no `UserPoolAddOns` member at all is the fourth state — add-ons
/// off, since there is no mode to switch them on.
const ADD_ON_STATES: &[(Option<&str>, bool)] = &[
    (Some("AUDIT"), true),
    (Some("ENFORCED"), true),
    (Some("OFF"), false),
    (None, false),
];

/// An app client of `user_pool_id`, and its minted id.
async fn app_client(router: &MultiServiceRouter, case: &str, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "risk-client" }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted client id: {created}"))
        .to_string()
}

/// The `RiskConfiguration` of a `SetRiskConfiguration` or
/// `DescribeRiskConfiguration` response.
fn configuration(payload: &Value) -> Value {
    payload
        .get("RiskConfiguration")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Whether a response carries a `RiskConfiguration` member at all.
fn carries_configuration(payload: &Value) -> bool {
    payload.get("RiskConfiguration").is_some()
}

/// `value` without its `LastModifiedDate`, so a comparison against the settings a
/// request carried does not depend on the instant the call ran.
fn without_last_modified(value: &Value) -> Value {
    let mut copy = value.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("LastModifiedDate");
    }
    copy
}

/// The `LastModifiedDate` of a rendered configuration, which Amazon Cognito
/// returns "in UNIX epoch time format" (`RiskConfigurationType.LastModifiedDate`).
fn last_modified(value: &Value) -> Option<f64> {
    value.get("LastModifiedDate").and_then(Value::as_f64)
}

/// `members` as a `SetRiskConfiguration` or `DescribeRiskConfiguration` request
/// against the level `user_pool_id` and `client_id` name.
///
/// It is also exactly what a rendered configuration of that level must carry
/// beside its `LastModifiedDate`: the settings stored there, identified by the
/// level they are stored at.
fn with_level(user_pool_id: &str, client_id: Option<&str>, members: &Value) -> Value {
    let mut body = members.clone();
    let target = body
        .as_object_mut()
        .expect("a request body is a JSON object");
    target.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(client_id) = client_id {
        target.insert("ClientId".to_string(), json!(client_id));
    }
    body
}

/// `count` distinct CIDR ranges, so a case can carry a list of any length.
fn ip_ranges(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
        .collect()
}

/// A notification configuration carrying every `NotifyConfigurationType` member,
/// including all three email templates.
fn full_notify_configuration() -> Value {
    json!({
        "SourceArn": SOURCE_ARN,
        "From": "security@example.com",
        "ReplyTo": "noreply@example.com",
        "BlockEmail": {
            "Subject": "Sign-in blocked",
            "HtmlBody": "<p>We blocked a sign-in.</p>",
            "TextBody": "We blocked a sign-in."
        },
        "NoActionEmail": {
            "Subject": "Sign-in reviewed",
            "TextBody": "We reviewed a sign-in."
        },
        "MfaEmail": {
            "Subject": "Sign-in needed a second factor",
            "HtmlBody": "<p>We asked for a second factor.</p>"
        }
    })
}

/// One row of the table: the settings a `SetRiskConfiguration` request carries
/// beyond the level's identity, which are also the settings the level must report
/// afterwards. Rows differ along what the rule generalises over — which of the
/// three settings the request carries, how many members each carries, which
/// modelled enum value each action takes, and how long each IP-range list is.
struct RiskCase {
    case: &'static str,
    settings: Value,
}

/// The table, spanning the settings `SetRiskConfiguration` carries. The enum
/// values come from this service's model:
/// `EventFilterType` is SIGN_IN | PASSWORD_CHANGE | SIGN_UP,
/// `CompromisedCredentialsEventActionType` is BLOCK | NO_ACTION, and
/// `AccountTakeoverEventActionType` is BLOCK | MFA_IF_CONFIGURED | MFA_REQUIRED |
/// NO_ACTION
/// (`smithy_query(query="closure",operation="SetRiskConfiguration")`).
fn risk_cases() -> Vec<RiskCase> {
    vec![
        RiskCase {
            case: "no setting at all: the level is set to its defaults",
            settings: json!({}),
        },
        RiskCase {
            case: "compromised credentials blocked on every event filter value",
            settings: json!({
                "CompromisedCredentialsRiskConfiguration": {
                    "EventFilter": ["SIGN_IN", "PASSWORD_CHANGE", "SIGN_UP"],
                    "Actions": { "EventAction": "BLOCK" }
                }
            }),
        },
        RiskCase {
            case: "compromised credentials with no event filter and the other event action",
            settings: json!({
                "CompromisedCredentialsRiskConfiguration": {
                    "Actions": { "EventAction": "NO_ACTION" }
                }
            }),
        },
        RiskCase {
            case: "compromised credentials filtered on one event only",
            settings: json!({
                "CompromisedCredentialsRiskConfiguration": {
                    "EventFilter": ["SIGN_UP"],
                    "Actions": { "EventAction": "BLOCK" }
                }
            }),
        },
        RiskCase {
            case: "account takeover: one action per risk level, no notification",
            settings: json!({
                "AccountTakeoverRiskConfiguration": {
                    "Actions": {
                        "LowAction": { "Notify": false, "EventAction": "NO_ACTION" },
                        "MediumAction": { "Notify": false, "EventAction": "MFA_IF_CONFIGURED" },
                        "HighAction": { "Notify": false, "EventAction": "BLOCK" }
                    }
                }
            }),
        },
        RiskCase {
            case: "account takeover: the high risk level alone, notified, with every template",
            settings: json!({
                "AccountTakeoverRiskConfiguration": {
                    "NotifyConfiguration": full_notify_configuration(),
                    "Actions": {
                        "HighAction": { "Notify": true, "EventAction": "MFA_REQUIRED" }
                    }
                }
            }),
        },
        RiskCase {
            case: "account takeover: a notification carrying only its source identity",
            settings: json!({
                "AccountTakeoverRiskConfiguration": {
                    "NotifyConfiguration": { "SourceArn": SOURCE_ARN },
                    "Actions": {
                        "LowAction": { "Notify": true, "EventAction": "NO_ACTION" }
                    }
                }
            }),
        },
        RiskCase {
            case: "an always-block list of one range and an always-allow list of three",
            settings: json!({
                "RiskExceptionConfiguration": {
                    "BlockedIPRangeList": ip_ranges(1),
                    "SkippedIPRangeList": ip_ranges(3)
                }
            }),
        },
        RiskCase {
            // Both lists are modelled length 0..200, so the empty list is the
            // published minimum.
            case: "two empty IP-range lists, the published minimum of each",
            settings: json!({
                "RiskExceptionConfiguration": {
                    "BlockedIPRangeList": [],
                    "SkippedIPRangeList": []
                }
            }),
        },
        RiskCase {
            case: "an always-block list at the published maximum of 200 ranges",
            settings: json!({
                "RiskExceptionConfiguration": {
                    "BlockedIPRangeList": ip_ranges(MAX_IP_RANGES)
                }
            }),
        },
        RiskCase {
            case: "all three settings at once",
            settings: json!({
                "CompromisedCredentialsRiskConfiguration": {
                    "EventFilter": ["PASSWORD_CHANGE", "SIGN_IN"],
                    "Actions": { "EventAction": "BLOCK" }
                },
                "AccountTakeoverRiskConfiguration": {
                    "NotifyConfiguration": full_notify_configuration(),
                    "Actions": {
                        "LowAction": { "Notify": true, "EventAction": "NO_ACTION" },
                        "MediumAction": { "Notify": true, "EventAction": "MFA_IF_CONFIGURED" },
                        "HighAction": { "Notify": true, "EventAction": "BLOCK" }
                    }
                },
                "RiskExceptionConfiguration": {
                    "BlockedIPRangeList": ip_ranges(2),
                    "SkippedIPRangeList": ip_ranges(1)
                }
            }),
        },
    ]
}

/// Every row of the table, set at `client_id`'s level and read back from it: the
/// response renders the settings the request carried, and a later describe of the
/// same level renders the same stored configuration.
async fn each_case_round_trips(client_level: bool) {
    for RiskCase { case, settings } in risk_cases() {
        let router = router();
        let user_pool_id = pool(&router, case).await;
        let client = if client_level {
            Some(app_client(&router, case, &user_pool_id).await)
        } else {
            None
        };
        let client_id = client.as_deref();

        let set = configuration(
            &ok(
                &router,
                "SetRiskConfiguration",
                &with_level(&user_pool_id, client_id, &settings),
            )
            .await,
        );
        assert_eq!(
            without_last_modified(&set),
            with_level(&user_pool_id, client_id, &settings),
            "case {case:?}: the response must render the settings the request carried, \
             identified by the level they were stored at, got {set}"
        );
        assert!(
            last_modified(&set).is_some(),
            "case {case:?}: a configuration that was just set reports when it was modified, \
             got {set}"
        );

        let described = configuration(
            &ok(
                &router,
                "DescribeRiskConfiguration",
                &with_level(&user_pool_id, client_id, &json!({})),
            )
            .await,
        );
        assert_eq!(
            described, set,
            "case {case:?}: the describe must render the stored configuration the set \
             answered, got {described}"
        );

        // A read changes nothing a later caller sees.
        let again = configuration(
            &ok(
                &router,
                "DescribeRiskConfiguration",
                &with_level(&user_pool_id, client_id, &json!({})),
            )
            .await,
        );
        assert_eq!(
            again, described,
            "case {case:?}: reading the configuration twice must answer the same, got {again}"
        );
    }
}

#[tokio::test]
async fn a_pool_level_set_stores_the_settings_the_request_carries() {
    each_case_round_trips(false).await;
}

#[tokio::test]
async fn a_client_level_set_stores_the_settings_the_request_carries() {
    each_case_round_trips(true).await;
}

#[tokio::test]
async fn a_set_replaces_every_setting_the_request_leaves_out() {
    let router = router();
    let case = "all three settings, then a set that carries none";
    let user_pool_id = pool(&router, case).await;

    let everything = risk_cases()
        .into_iter()
        .find(|row| row.case == "all three settings at once")
        .expect("the table carries the row that sets all three")
        .settings;
    let full = configuration(
        &ok(
            &router,
            "SetRiskConfiguration",
            &with_level(&user_pool_id, None, &everything),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&full),
        with_level(&user_pool_id, None, &everything),
        "case {case:?}: got {full}"
    );

    // "To set the risk configuration for the user pool to defaults, send this
    // request with only the UserPoolId parameter": every setting is gone, and the
    // instant of this call is what the level reports.
    let reset = configuration(
        &ok(
            &router,
            "SetRiskConfiguration",
            &with_level(&user_pool_id, None, &json!({})),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&reset),
        json!({ "UserPoolId": user_pool_id }),
        "case {case:?}: a set that carries no setting leaves the level with none, got {reset}"
    );
    assert!(
        last_modified(&reset).is_some(),
        "case {case:?}: the reset is itself a modification, so it reports its instant, \
         got {reset}"
    );

    let described = configuration(
        &ok(
            &router,
            "DescribeRiskConfiguration",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await,
    );
    assert_eq!(
        described, reset,
        "case {case:?}: the describe must agree with the set that stored it, got {described}"
    );
}

#[tokio::test]
async fn the_two_levels_hold_separate_configurations() {
    let router = router();
    let case = "a pool-level configuration beside a client-level one";
    let user_pool_id = pool(&router, case).await;
    let client_id = app_client(&router, case, &user_pool_id).await;

    let pool_settings = json!({
        "RiskExceptionConfiguration": { "BlockedIPRangeList": ["192.0.2.0/24"] }
    });
    let client_settings = json!({
        "RiskExceptionConfiguration": { "SkippedIPRangeList": ["198.51.100.0/24"] }
    });

    // An app client that holds no settings of its own holds none: the pool-level
    // configuration is "applied to all user pool app clients that don't have
    // client-level settings" (`RiskConfigurationType.ClientId`), so it is reported
    // at the pool's level, not at the client's — and the client's level, which
    // nothing configured, is reported by no member at all.
    ok(
        &router,
        "SetRiskConfiguration",
        &with_level(&user_pool_id, None, &pool_settings),
    )
    .await;
    let inherited = ok(
        &router,
        "DescribeRiskConfiguration",
        &with_level(&user_pool_id, Some(&client_id), &json!({})),
    )
    .await;
    assert!(
        !carries_configuration(&inherited),
        "case {case:?}: a client nothing configured holds no configuration of its own, so \
         the read carries no RiskConfiguration member, got {inherited}"
    );

    // Setting the client's own settings leaves the pool's alone, and the reverse.
    ok(
        &router,
        "SetRiskConfiguration",
        &with_level(&user_pool_id, Some(&client_id), &client_settings),
    )
    .await;
    let at_pool = configuration(
        &ok(
            &router,
            "DescribeRiskConfiguration",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&at_pool),
        with_level(&user_pool_id, None, &pool_settings),
        "case {case:?}: a client-level set must not change the pool-level configuration, \
         got {at_pool}"
    );
    let at_client = configuration(
        &ok(
            &router,
            "DescribeRiskConfiguration",
            &with_level(&user_pool_id, Some(&client_id), &json!({})),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&at_client),
        with_level(&user_pool_id, Some(&client_id), &client_settings),
        "case {case:?}: the client's own settings are what its level reports, got {at_client}"
    );

    // A second app client of the same pool still holds none of its own.
    let other_client = app_client(&router, case, &user_pool_id).await;
    let other = ok(
        &router,
        "DescribeRiskConfiguration",
        &with_level(&user_pool_id, Some(&other_client), &json!({})),
    )
    .await;
    assert!(
        !carries_configuration(&other),
        "case {case:?}: one client's settings are not another's, so the client nothing \
         configured carries no RiskConfiguration member, got {other}"
    );

    // "To reset the threat protection settings of an app client to be inherited
    // from the user pool, send UserPoolId and ClientId parameters only."
    let reset = configuration(
        &ok(
            &router,
            "SetRiskConfiguration",
            &with_level(&user_pool_id, Some(&client_id), &json!({})),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&reset),
        json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        "case {case:?}: the client's reset leaves it with no settings of its own, got {reset}"
    );
    let pool_after = configuration(
        &ok(
            &router,
            "DescribeRiskConfiguration",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await,
    );
    assert_eq!(
        without_last_modified(&pool_after),
        with_level(&user_pool_id, None, &pool_settings),
        "case {case:?}: the client's reset leaves the pool's configuration alone, \
         got {pool_after}"
    );
}

#[tokio::test]
async fn a_level_nothing_configured_carries_no_configuration_member() {
    // A level that has never had a configuration written holds none, so the read
    // carries NO `RiskConfiguration` member — whatever the pool's add-on state,
    // which says nothing about what has been stored. An absent member is what
    // tells a caller "never configured" from "configured to defaults", which is
    // the stored configuration a set carrying no setting leaves behind.
    for (mode, _add_ons_on) in ADD_ON_STATES {
        let router = router();
        let case = format!("a pool and a client nothing has configured, add-ons {mode:?}");
        let user_pool_id = pool_in_mode(&router, &case, *mode).await;
        let client_id = app_client(&router, &case, &user_pool_id).await;

        for (level, body) in [
            ("the pool level", json!({ "UserPoolId": user_pool_id })),
            (
                "the client level",
                json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
            ),
        ] {
            let (status, payload) = call(&router, "DescribeRiskConfiguration", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?} at {level}: the read is served, got {status} with {payload}"
            );
            assert!(
                !carries_configuration(&payload),
                "case {case:?} at {level}: a level nothing configured carries no \
                 RiskConfiguration member, got {payload}"
            );
        }
    }

    // "Configured to defaults" is the other answer: a set carrying no setting
    // stores a configuration, so the member is present from then on.
    let router = router();
    let case = "the pool level, set to its defaults";
    let user_pool_id = pool_in_mode(&router, case, None).await;
    ok(
        &router,
        "SetRiskConfiguration",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let described = ok(
        &router,
        "DescribeRiskConfiguration",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert!(
        carries_configuration(&described),
        "case {case:?}: a level set to its defaults holds a configuration, so the read \
         carries the member, got {described}"
    );
    assert_eq!(
        without_last_modified(&configuration(&described)),
        json!({ "UserPoolId": user_pool_id }),
        "case {case:?}: that configuration holds no setting, got {described}"
    );
}

#[tokio::test]
async fn only_a_compromised_credentials_write_needs_the_pools_add_ons() {
    // The add-on precondition is scoped to the setting it governs: a write
    // carrying the compromised-credentials setting is refused while the pool's
    // add-ons are off, and every other write — and every read — is served
    // whatever the pool's add-on state. Each row of the settings table is driven
    // against each of the four add-on states.
    for (mode, add_ons_on) in ADD_ON_STATES {
        for RiskCase { case, settings } in risk_cases() {
            let router = router();
            let case = format!("{case} with add-ons {mode:?}");
            let user_pool_id = pool_in_mode(&router, &case, *mode).await;
            let gated = settings
                .get("CompromisedCredentialsRiskConfiguration")
                .is_some();

            let (status, payload) = call(
                &router,
                "SetRiskConfiguration",
                &with_level(&user_pool_id, None, &settings),
            )
            .await;

            if gated && !add_ons_on {
                assert_eq!(
                    (status, error_code(&payload)),
                    (400, "UserPoolAddOnNotEnabledException"),
                    "case {case:?}: a compromised-credentials write is refused while the \
                     pool's add-ons are off, got {status} with {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    ADD_ONS_NOT_ENABLED_MESSAGE,
                    "case {case:?}: the refusal reports that the add-ons are not enabled, \
                     got {payload}"
                );

                // The refused write stored nothing, so the level still holds no
                // configuration at all.
                let described =
                    ok(&router, "DescribeRiskConfiguration", &json!({ "UserPoolId": user_pool_id }))
                        .await;
                assert!(
                    !carries_configuration(&described),
                    "case {case:?}: a refused write persists nothing, got {described}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: this write needs no add-on, got {status} with {payload}"
            );
            assert_eq!(
                without_last_modified(&configuration(&payload)),
                with_level(&user_pool_id, None, &settings),
                "case {case:?}: the response renders the settings the request carried, \
                 got {payload}"
            );

            let described = configuration(
                &ok(
                    &router,
                    "DescribeRiskConfiguration",
                    &json!({ "UserPoolId": user_pool_id }),
                )
                .await,
            );
            assert_eq!(
                described,
                configuration(&payload),
                "case {case:?}: the read reports the stored configuration the write \
                 answered, got {described}"
            );
        }
    }

    // The gate is the POOL's add-on state at either level, so a client-level
    // write answers the same way: the compromised-credentials setting needs the
    // add-on on, the adaptive-authentication setting does not.
    for (mode, add_ons_on) in ADD_ON_STATES {
        let router = router();
        let case = format!("a client-level write with add-ons {mode:?}");
        let user_pool_id = pool_in_mode(&router, &case, *mode).await;
        let client_id = app_client(&router, &case, &user_pool_id).await;

        for (gated, settings) in [
            (
                true,
                json!({
                    "CompromisedCredentialsRiskConfiguration": {
                        "Actions": { "EventAction": "BLOCK" }
                    }
                }),
            ),
            (
                false,
                json!({
                    "AccountTakeoverRiskConfiguration": {
                        "Actions": { "HighAction": { "Notify": false, "EventAction": "BLOCK" } }
                    }
                }),
            ),
        ] {
            let (status, payload) = call(
                &router,
                "SetRiskConfiguration",
                &with_level(&user_pool_id, Some(&client_id), &settings),
            )
            .await;
            let expected = if gated && !add_ons_on { 400 } else { 200 };
            assert_eq!(
                status, expected,
                "case {case:?}: a client-level write carrying \
                 {settings} expects {expected}, got {status} with {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_level_this_emulator_does_not_hold_is_refused() {
    let router = router();
    let case = "an absent pool and an absent app client";
    let user_pool_id = pool(&router, case).await;
    let absent_pool = "us-east-1_absentpool";
    let absent_client = "absentclient";

    for operation in ["SetRiskConfiguration", "DescribeRiskConfiguration"] {
        let (status, payload) =
            call(&router, operation, &json!({ "UserPoolId": absent_pool })).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {operation} on a pool nothing created is a 404, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("User pool {absent_pool} does not exist."),
            "case {case:?}: {operation} names the pool it could not find, got {payload}"
        );

        let (status, payload) = call(
            &router,
            operation,
            &json!({ "UserPoolId": user_pool_id, "ClientId": absent_client }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: {operation} on a client the pool does not hold is a 404, got \
             {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("User pool client {absent_client} does not exist."),
            "case {case:?}: {operation} names the app client it could not find, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;

    // One value past each published bound of these operations' inputs
    // (`smithy_query(query="closure",operation="SetRiskConfiguration")`): the user
    // pool id's pattern `^[\w-]+_[0-9a-zA-Z]+$`, the client id's length 1..128 and
    // pattern `^[\w+]+$`, each IP-range list's length 0..200, and the enum values
    // each action admits.
    for (label, operation, body) in [
        (
            "a user pool id the pattern excludes",
            "SetRiskConfiguration",
            json!({ "UserPoolId": "no-underscore" }),
        ),
        (
            "an empty client id, below the length minimum 1",
            "SetRiskConfiguration",
            json!({ "UserPoolId": user_pool_id, "ClientId": "" }),
        ),
        (
            "a client id of 129 characters, past the length maximum 128",
            "DescribeRiskConfiguration",
            json!({ "UserPoolId": user_pool_id, "ClientId": "c".repeat(129) }),
        ),
        (
            "a client id carrying a dash, which the pattern excludes",
            "DescribeRiskConfiguration",
            json!({ "UserPoolId": user_pool_id, "ClientId": "has-a-dash" }),
        ),
        (
            "201 blocked ranges, one past the length maximum 200",
            "SetRiskConfiguration",
            json!({
                "UserPoolId": user_pool_id,
                "RiskExceptionConfiguration": {
                    "BlockedIPRangeList": ip_ranges(MAX_IP_RANGES + 1)
                }
            }),
        ),
        (
            "201 skipped ranges, one past the length maximum 200",
            "SetRiskConfiguration",
            json!({
                "UserPoolId": user_pool_id,
                "RiskExceptionConfiguration": {
                    "SkippedIPRangeList": ip_ranges(MAX_IP_RANGES + 1)
                }
            }),
        ),
        (
            "an event action no modelled enum value names",
            "SetRiskConfiguration",
            json!({
                "UserPoolId": user_pool_id,
                "CompromisedCredentialsRiskConfiguration": {
                    "Actions": { "EventAction": "QUARANTINE" }
                }
            }),
        ),
        (
            "an event filter value no modelled enum value names",
            "SetRiskConfiguration",
            json!({
                "UserPoolId": user_pool_id,
                "CompromisedCredentialsRiskConfiguration": {
                    "EventFilter": ["SIGN_OUT"],
                    "Actions": { "EventAction": "BLOCK" }
                }
            }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: {operation} outside a modelled constraint is a 400, got {status} \
             with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got \
             {payload}"
        );
    }
}
