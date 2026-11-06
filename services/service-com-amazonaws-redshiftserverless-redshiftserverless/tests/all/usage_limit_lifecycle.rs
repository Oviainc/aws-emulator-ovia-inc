// SPDX-License-Identifier: CC0-1.0

//! Usage limits, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with
//! `POST /`, `X-Amz-Target: RedshiftServerless.<Operation>` and an awsJson1_1 body — so
//! the boundary validator, the protocol codec and the handler all take part, exactly as
//! they do for an SDK client.
//!
//! The rule under test: `CreateUsageLimit` mints a usage limit over an existing Redshift
//! Serverless resource and stores it, so `GetUsageLimit` reports back exactly what the
//! create answered, and nothing else the caller does changes it. The tables span what
//! the rule generalises over — every modelled usage type, period and breach action,
//! either kind of resource ARN, the amount's magnitude, the signed region — not the one
//! request the issue quoted.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The values `UsageLimitUsageType` accepts, from
/// `smithy_query(query="shape", shape="UsageLimitUsageType")`.
const USAGE_TYPES: [&str; 2] = ["serverless-compute", "cross-region-datasharing"];

/// The values `UsageLimitPeriod` accepts, from
/// `smithy_query(query="shape", shape="UsageLimitPeriod")`. The default is `monthly`
/// (`CreateUsageLimitRequest$period`: "The default is monthly.").
const PERIODS: [&str; 3] = ["daily", "weekly", "monthly"];

/// The values `UsageLimitBreachAction` accepts, from
/// `smithy_query(query="shape", shape="UsageLimitBreachAction")`. The default is `log`
/// (`CreateUsageLimitRequest$breachAction`: "The default is log.").
const BREACH_ACTIONS: [&str; 3] = ["log", "emit-metric", "deactivate"];

/// A SigV4 `Authorization` header scoped to this service in `region`. It is what routes
/// the request to this service AND what names the region its ARNs carry; signatures are
/// not verified here (authentication is off), only the scope is read.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/redshift-serverless/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One emulator with an empty store: the shared engine the router builds in production,
/// carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(engine),
    );
    router
}

/// Call `operation` with `body`, signed for `region`; answer its HTTP status and parsed
/// JSON body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RedshiftServerless.{operation}"))
        .header("authorization", authorization(region))
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`call_in`] in `us-east-1`.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, "us-east-1", operation, body).await
}

/// The error shape name out of an awsJson error envelope, with any namespace prefix
/// dropped (`com.amazonaws.redshiftserverless#ConflictException` -> `ConflictException`).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('#')
        .next_back()
        .unwrap_or("")
        .to_string()
}

/// A canonical hyphenated UUID: 8-4-4-4-12 lowercase hex groups.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && value
            .chars()
            .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// Create a namespace in `region` and answer its ARN.
async fn namespace_arn_in(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, body) = call_in(router, region, "CreateNamespace", json!({ "namespaceName": name }))
        .await;
    assert_eq!(
        status, 200,
        "CreateNamespace {name}: expected 200, got {status} with {body}"
    );
    body["namespace"]["namespaceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create a workgroup over `namespace` in `region` and answer its ARN.
async fn workgroup_arn_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    namespace: &str,
) -> String {
    let (status, body) = call_in(
        router,
        region,
        "CreateWorkgroup",
        json!({ "workgroupName": name, "namespaceName": namespace }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkgroup {name}: expected 200, got {status} with {body}"
    );
    body["workgroup"]["workgroupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A namespace and a workgroup in `us-east-1`, as the two ARNs a usage limit can name.
async fn resources(router: &MultiServiceRouter, suffix: &str) -> (String, String) {
    let namespace = format!("ns-{suffix}");
    let workgroup = format!("wg-{suffix}");
    let namespace_arn = namespace_arn_in(router, "us-east-1", &namespace).await;
    let workgroup_arn = workgroup_arn_in(router, "us-east-1", &workgroup, &namespace).await;
    (namespace_arn, workgroup_arn)
}

// ---------------------------------------------------------------------------
// The create stores, and every read reports the stored record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_usage_limit_is_reported_back_for_every_modelled_combination() {
    let router = emulator();
    let (namespace_arn, workgroup_arn) = resources(&router, "catalog").await;

    // Every modelled usage type over either kind of resource ARN, crossed with every
    // modelled period and breach action, and amounts spanning the magnitudes the `Long`
    // `amount` admits. One row per (resource, usageType, period) triple: that triple is
    // what a limit caps, so a second limit over it is a conflict (asserted below).
    let mut cases: Vec<(String, Value)> = Vec::new();
    for (resource_kind, resource_arn) in [
        ("namespace", namespace_arn.as_str()),
        ("workgroup", workgroup_arn.as_str()),
    ] {
        for (index, period) in PERIODS.iter().enumerate() {
            for (offset, usage_type) in USAGE_TYPES.iter().enumerate() {
                let breach_action = BREACH_ACTIONS[(index + offset) % BREACH_ACTIONS.len()];
                let amount = 1_i64 << (4 * (index + offset) as u32);
                cases.push((
                    format!("{resource_kind}/{usage_type}/{period}/{breach_action}/{amount}"),
                    json!({
                        "resourceArn": resource_arn,
                        "usageType": usage_type,
                        "period": period,
                        "breachAction": breach_action,
                        "amount": amount,
                    }),
                ));
            }
        }
    }

    let mut seen_ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let (status, body) = call(&router, "CreateUsageLimit", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        let created = body["usageLimit"].clone();

        // Each member the request carried is stored verbatim.
        for member in ["resourceArn", "usageType", "period", "breachAction", "amount"] {
            assert_eq!(
                created[member], request[member],
                "case {case:?}: {member} expected {:?}, got {:?}",
                request[member], created[member]
            );
        }

        // The id is server-minted, in the UUID form AWS mints resource ids in, and
        // unique across creates.
        let id = created["usageLimitId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid(&id),
            "case {case:?}: usageLimitId expected a UUID, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: usageLimitId {id:?} was already minted for another limit"
        );
        seen_ids.push(id.clone());

        // The ARN is derived from that id, in the signed region and this account.
        assert_eq!(
            created["usageLimitArn"],
            json!(format!(
                "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:usagelimit/{id}"
            )),
            "case {case:?}: usageLimitArn got {:?}",
            created["usageLimitArn"]
        );

        // The create's answer and the read's answer are the same record, and reading it
        // twice does not change it.
        for attempt in 1..=2 {
            let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
            assert_eq!(
                (status, &body["usageLimit"]),
                (200, &created),
                "case {case:?}: GetUsageLimit #{attempt} must report the created record, \
                 got {status} with {body}"
            );
        }
    }

    assert_eq!(
        seen_ids.len(),
        cases.len(),
        "every case must have created one limit, got {} for {} cases",
        seen_ids.len(),
        cases.len()
    );
}

#[tokio::test]
async fn the_documented_defaults_apply_when_period_and_breach_action_are_omitted() {
    // `CreateUsageLimitRequest$period`: "The default is monthly."
    // `CreateUsageLimitRequest$breachAction`: "The default is log."
    let cases: [(&str, Value, &str, &str); 4] = [
        ("neither member set", json!({}), "monthly", "log"),
        (
            "only the period",
            json!({ "period": "weekly" }),
            "weekly",
            "log",
        ),
        (
            "only the breach action",
            json!({ "breachAction": "deactivate" }),
            "monthly",
            "deactivate",
        ),
        (
            "both members set",
            json!({ "period": "daily", "breachAction": "emit-metric" }),
            "daily",
            "emit-metric",
        ),
    ];

    let router = emulator();
    for (index, (case, extra, period, breach_action)) in cases.iter().enumerate() {
        // A fresh namespace per row, so each row's (resource, usageType, period) triple
        // is its own and no row conflicts with another.
        let resource_arn =
            namespace_arn_in(&router, "us-east-1", &format!("defaults-ns-{index}")).await;
        let mut request = json!({
            "resourceArn": resource_arn,
            "usageType": USAGE_TYPES[index % USAGE_TYPES.len()],
            "amount": 60,
        });
        let object = request.as_object_mut().expect("an object request");
        for (key, value) in extra.as_object().into_iter().flatten() {
            object.insert(key.clone(), value.clone());
        }

        let (status, body) = call(&router, "CreateUsageLimit", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["usageLimit"]["period"],
            json!(period),
            "case {case:?}: period expected {period:?}, got {:?}",
            body["usageLimit"]["period"]
        );
        assert_eq!(
            body["usageLimit"]["breachAction"],
            json!(breach_action),
            "case {case:?}: breachAction expected {breach_action:?}, got {:?}",
            body["usageLimit"]["breachAction"]
        );
    }
}

#[tokio::test]
async fn a_usage_limit_arn_carries_the_region_the_request_is_signed_for() {
    // The rule generalises over region: nothing about it is specific to us-east-1.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-3"] {
        let router = emulator();
        let namespace_arn = namespace_arn_in(&router, region, "regional-ns").await;
        let (status, body) = call_in(
            &router,
            region,
            "CreateUsageLimit",
            json!({
                "resourceArn": namespace_arn,
                "usageType": "cross-region-datasharing",
                "amount": 7,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        let id = body["usageLimit"]["usageLimitId"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            body["usageLimit"]["usageLimitArn"],
            json!(format!(
                "arn:aws:redshift-serverless:{region}:{ACCOUNT}:usagelimit/{id}"
            )),
            "region {region:?}: usageLimitArn got {:?}",
            body["usageLimit"]["usageLimitArn"]
        );
    }
}

#[tokio::test]
async fn a_usage_limit_survives_the_workgroups_status_transition() {
    let router = emulator();
    let namespace = "survives-ns";
    namespace_arn_in(&router, "us-east-1", namespace).await;
    let workgroup_arn = workgroup_arn_in(&router, "us-east-1", "survives-wg", namespace).await;

    // Taken out while the workgroup is still CREATING.
    let (status, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": workgroup_arn,
            "usageType": "serverless-compute",
            "amount": 200,
            "period": "daily",
            "breachAction": "emit-metric",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateUsageLimit during provisioning: expected 200, got {status} with {body}"
    );
    let created = body["usageLimit"].clone();
    let id = created["usageLimitId"].as_str().unwrap_or_default().to_string();

    // The workgroup settles into AVAILABLE on its own; the limit is untouched by it.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (status, body) = call(
        &router,
        "GetWorkgroup",
        json!({ "workgroupName": "survives-wg" }),
    )
    .await;
    assert_eq!(
        (status, &body["workgroup"]["status"]),
        (200, &json!("AVAILABLE")),
        "GetWorkgroup once the transition fell due: got {status} with {body}"
    );

    let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
    assert_eq!(
        (status, &body["usageLimit"]),
        (200, &created),
        "GetUsageLimit after the workgroup settled: got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_amount_that_is_not_positive_is_refused() {
    // `CreateUsageLimitRequest$amount`: "The value must be a positive number." The
    // smallest accepted value and the first one past the bound are both covered.
    let refused: [(&str, i64); 4] = [
        ("zero", 0),
        ("one below the bound", -1),
        ("a large negative amount", -1_000_000),
        ("the smallest Long", i64::MIN),
    ];
    let accepted: [(&str, i64); 3] = [
        ("the smallest positive amount", 1),
        ("a typical RPU-hour budget", 512),
        ("the largest Long", i64::MAX),
    ];

    let router = emulator();
    let (_, workgroup_arn) = resources(&router, "amounts").await;
    for (case, amount) in refused {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": workgroup_arn,
                "usageType": "serverless-compute",
                "amount": amount,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} (amount {amount}): expected 400 ValidationException, \
             got {status} with {body}"
        );
    }

    for (index, (case, amount)) in accepted.iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": workgroup_arn,
                "usageType": "serverless-compute",
                // A distinct period per row: the triple is unique per accepted amount.
                "period": PERIODS[index % PERIODS.len()],
                "amount": amount,
            }),
        )
        .await;
        assert_eq!(
            (status, &body["usageLimit"]["amount"]),
            (200, &json!(amount)),
            "case {case:?} (amount {amount}): expected 200 with the amount stored, \
             got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_limit_over_a_resource_the_account_does_not_hold_is_refused_as_invalid() {
    let router = emulator();
    let (namespace_arn, workgroup_arn) = resources(&router, "unknown").await;

    // Every row names something that is not a Redshift Serverless resource of this
    // account's: a resource of a kind the service has but never created, one in another
    // region or account, another service's resource, and a string that is no ARN at all.
    let cases: [(&str, String); 6] = [
        (
            "a namespace ARN nothing created",
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:namespace/00000000-0000-4000-8000-000000000000"),
        ),
        (
            "a workgroup ARN nothing created",
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:workgroup/00000000-0000-4000-8000-000000000000"),
        ),
        (
            "an existing namespace in another region",
            namespace_arn.replace("us-east-1", "eu-west-1"),
        ),
        (
            "an existing workgroup in another account",
            workgroup_arn.replace(ACCOUNT, "111122223333"),
        ),
        (
            "another service's resource",
            format!("arn:aws:redshift:us-east-1:{ACCOUNT}:cluster:prod"),
        ),
        ("a string that is no ARN", "not-an-arn".to_string()),
    ];

    for (case, resource_arn) in &cases {
        for usage_type in USAGE_TYPES {
            let (status, body) = call(
                &router,
                "CreateUsageLimit",
                json!({
                    "resourceArn": resource_arn,
                    "usageType": usage_type,
                    "amount": 10,
                }),
            )
            .await;
            // An ARN that names no resource of this account is a bad `resourceArn`, not a
            // resource this call addressed: the recorded behaviour of the real service is
            // the 400 `ValidationException`, NOT the 404 the operation also models.
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({usage_type}): expected 400 ValidationException, \
                 got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_second_limit_over_the_same_resource_usage_type_and_period_conflicts() {
    let router = emulator();
    let (namespace_arn, workgroup_arn) = resources(&router, "conflict").await;

    let first = json!({
        "resourceArn": workgroup_arn,
        "usageType": "serverless-compute",
        "period": "weekly",
        "amount": 100,
    });
    let (status, body) = call(&router, "CreateUsageLimit", first.clone()).await;
    assert_eq!(
        status, 200,
        "the first limit: expected 200, got {status} with {body}"
    );

    // Same triple, whatever else differs -> conflict. Change any ONE member of the
    // triple and it is a different limit, which is accepted.
    let conflicting: [(&str, Value); 3] = [
        ("the identical request", first.clone()),
        (
            "a different amount",
            json!({ "resourceArn": workgroup_arn, "usageType": "serverless-compute", "period": "weekly", "amount": 999 }),
        ),
        (
            "a different breach action",
            json!({ "resourceArn": workgroup_arn, "usageType": "serverless-compute", "period": "weekly", "amount": 100, "breachAction": "deactivate" }),
        ),
    ];
    for (case, request) in &conflicting {
        let (status, body) = call(&router, "CreateUsageLimit", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} with {body}"
        );
    }

    let accepted: [(&str, Value); 3] = [
        (
            "another period",
            json!({ "resourceArn": workgroup_arn, "usageType": "serverless-compute", "period": "daily", "amount": 100 }),
        ),
        (
            "another usage type",
            json!({ "resourceArn": workgroup_arn, "usageType": "cross-region-datasharing", "period": "weekly", "amount": 100 }),
        ),
        (
            "another resource",
            json!({ "resourceArn": namespace_arn, "usageType": "serverless-compute", "period": "weekly", "amount": 100 }),
        ),
    ];
    for (case, request) in &accepted {
        let (status, body) = call(&router, "CreateUsageLimit", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_unknown_usage_limit_id_is_refused_as_invalid() {
    let router = emulator();
    let (_, workgroup_arn) = resources(&router, "get-unknown").await;
    let (_, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": workgroup_arn,
            "usageType": "serverless-compute",
            "amount": 10,
        }),
    )
    .await;
    let created = body["usageLimit"]["usageLimitId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Nothing but the id the create minted answers.
    let cases: [(&str, String); 4] = [
        (
            "a well-formed id nothing minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("an empty-looking id", "0".to_string()),
        ("the ARN instead of the id", format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:usagelimit/{created}")),
        ("the id with one character appended", format!("{created}0")),
    ];
    for (case, id) in &cases {
        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        // An id that resolves to nothing is a bad INPUT, not a resource the call
        // addressed: the modelled `ValidationException` at its 400.
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
    }

    // And the real id still answers, so the refusals above changed nothing.
    let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": created })).await;
    assert_eq!(
        status, 200,
        "the minted id must still answer, got {status} with {body}"
    );
}
