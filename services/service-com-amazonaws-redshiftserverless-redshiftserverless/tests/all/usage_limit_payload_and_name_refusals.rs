// SPDX-License-Identifier: CC0-1.0

//! The usage-limit payload, the usage-limit lifecycle, and how this service refuses an
//! identifier that resolves to nothing — end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: RedshiftServerless.<Operation>` and an awsJson1_1 body — so the boundary
//! validator, the protocol codec and the handler all take part, exactly as they do for an
//! SDK client.
//!
//! Three rules are under test, each asserted over a table that spans what it generalises
//! over rather than the one request that exposed it:
//!
//! 1. Every usage-limit payload this service emits carries the feature the limit is taken
//!    out over and the metered dimension its `amount` is counted in, both following from
//!    the submitted `usageType`, on the create, on every read and on the delete alike.
//! 2. A usage limit is addressed by the id the service minted for it, so an id that
//!    resolves to nothing — never minted, or already deleted — is refused as a bad INPUT:
//!    HTTP 400 `ValidationException`, worded from the id the request carried, on the read,
//!    the update and the delete alike.
//! 3. A resource NAME that resolves to nothing — a namespace's or a workgroup's alike —
//!    is refused with the modelled `ResourceNotFoundException` carried on HTTP 400.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// Every value `UsageLimitUsageType` accepts, from
/// `smithy_query(query="shape", shape="UsageLimitUsageType")`, paired with the metered
/// dimension a limit over it counts its `amount` in: RPU-hours of serverless compute, or
/// terabytes of data for cross-Region data sharing (`CreateUsageLimitRequest$amount`: "If
/// time-based, this amount is in Redshift Processing Units (RPU) consumed per hour. If
/// data-based, this amount is in terabytes (TB) of data transferred between Regions").
const USAGE_TYPE_DIMENSIONS: [(&str, &str); 2] = [
    ("serverless-compute", "compute-used"),
    ("cross-region-datasharing", "data-scanned"),
];

/// Every value `UsageLimitPeriod` accepts, from
/// `smithy_query(query="shape", shape="UsageLimitPeriod")`.
const PERIODS: [&str; 3] = ["daily", "weekly", "monthly"];

/// A SigV4 `Authorization` header scoped to this service in `region`: it routes the
/// request to this service and names the region its ARNs carry. Signatures are not
/// verified here, only the scope is read.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/redshift-serverless/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One emulator with an empty store.
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
/// dropped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('#')
        .next_back()
        .unwrap_or("")
        .to_string()
}

/// Create a namespace in `region` and answer its ARN.
async fn namespace_arn_in(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, body) =
        call_in(router, region, "CreateNamespace", json!({ "namespaceName": name })).await;
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

/// Assert that `payload` reports `usage_type` as the feature it limits and the dimension
/// that usage type is metered in.
fn assert_dimensions(case: &str, where_from: &str, payload: &Value, usage_type: &str) {
    let dimension = USAGE_TYPE_DIMENSIONS
        .iter()
        .find(|(known, _)| *known == usage_type)
        .map(|(_, dimension)| *dimension)
        .unwrap_or_else(|| panic!("case {case:?}: {usage_type} is not a modelled usage type"));
    assert_eq!(
        payload["featureType"],
        json!(usage_type),
        "case {case:?}: {where_from} featureType expected {usage_type:?}, got {:?}",
        payload["featureType"]
    );
    assert_eq!(
        payload["limitType"],
        json!(dimension),
        "case {case:?}: {where_from} limitType expected {dimension:?}, got {:?}",
        payload["limitType"]
    );
}

// ---------------------------------------------------------------------------
// The payload every usage-limit reader emits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_usage_limit_payload_reports_the_feature_and_the_metered_dimension() {
    let router = emulator();
    let namespace_arn = namespace_arn_in(&router, "us-east-1", "dimensions-ns").await;
    let workgroup_arn =
        workgroup_arn_in(&router, "us-east-1", "dimensions-wg", "dimensions-ns").await;

    // Every modelled usage type over either kind of resource a limit can be taken out
    // over, crossed with every modelled period and a spread of amounts: nothing about the
    // rule is specific to one of them.
    let mut cases: Vec<(String, Value, &str)> = Vec::new();
    for (kind, resource_arn) in [
        ("namespace", namespace_arn.as_str()),
        ("workgroup", workgroup_arn.as_str()),
    ] {
        for (index, period) in PERIODS.iter().enumerate() {
            for (usage_type, _) in USAGE_TYPE_DIMENSIONS {
                cases.push((
                    format!("{kind}/{usage_type}/{period}"),
                    json!({
                        "resourceArn": resource_arn,
                        "usageType": usage_type,
                        "period": period,
                        "amount": 1 + (index as i64) * 512,
                    }),
                    usage_type,
                ));
            }
        }
    }

    for (case, request, usage_type) in &cases {
        let (status, body) = call(&router, "CreateUsageLimit", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        let created = body["usageLimit"].clone();
        assert_dimensions(case, "CreateUsageLimit", &created, usage_type);

        let id = created["usageLimitId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The read reports the same record, the two members included.
        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &created),
            "case {case:?}: GetUsageLimit must report the created record, got {status} with {body}"
        );
        assert_dimensions(case, "GetUsageLimit", &body["usageLimit"], usage_type);

        // So does the listing.
        let (status, body) = call(
            &router,
            "ListUsageLimits",
            json!({ "resourceArn": request["resourceArn"], "usageType": usage_type }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListUsageLimits expected 200, got {status} with {body}"
        );
        let listed = body["usageLimits"]
            .as_array()
            .map(|limits| limits.to_vec())
            .unwrap_or_default();
        let found = listed
            .iter()
            .find(|limit| limit["usageLimitId"] == created["usageLimitId"])
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListUsageLimits did not report the created limit: {listed:?}")
            });
        assert_dimensions(case, "ListUsageLimits", found, usage_type);

        // And the delete, which answers with the record it removed.
        let (status, body) = call(&router, "DeleteUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &created),
            "case {case:?}: DeleteUsageLimit must answer the record it removed, \
             got {status} with {body}"
        );
        assert_dimensions(case, "DeleteUsageLimit", &body["usageLimit"], usage_type);
    }
}

#[tokio::test]
async fn a_usage_limit_arn_carries_the_unhyphenated_resource_type() {
    // The ARN's fixed part is the partition, the service prefix and the resource type
    // `usagelimit`; the region, the account and the id come from the request and from
    // state. The rule generalises over region and over usage type.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-3"] {
        let router = emulator();
        let namespace_arn = namespace_arn_in(&router, region, "arn-form-ns").await;
        for (usage_type, _) in USAGE_TYPE_DIMENSIONS {
            let (status, body) = call_in(
                &router,
                region,
                "CreateUsageLimit",
                json!({
                    "resourceArn": namespace_arn,
                    "usageType": usage_type,
                    "amount": 42,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {region}/{usage_type}: CreateUsageLimit expected 200, \
                 got {status} with {body}"
            );
            let id = body["usageLimit"]["usageLimitId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected = format!("arn:aws:redshift-serverless:{region}:{ACCOUNT}:usagelimit/{id}");
            assert_eq!(
                body["usageLimit"]["usageLimitArn"],
                json!(expected),
                "case {region}/{usage_type}: usageLimitArn expected {expected:?}, got {:?}",
                body["usageLimit"]["usageLimitArn"]
            );

            // The read mints nothing of its own: it reports the stored ARN.
            let (status, read) = call_in(
                &router,
                region,
                "GetUsageLimit",
                json!({ "usageLimitId": id }),
            )
            .await;
            assert_eq!(
                (status, &read["usageLimit"]["usageLimitArn"]),
                (200, &json!(expected)),
                "case {region}/{usage_type}: GetUsageLimit got {status} with {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A usage-limit id that resolves to nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_usage_limit_id_that_resolves_to_nothing_is_refused_as_invalid() {
    let router = emulator();
    let namespace_arn = namespace_arn_in(&router, "us-east-1", "absent-id-ns").await;
    let (_, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": namespace_arn,
            "usageType": "serverless-compute",
            "amount": 60,
        }),
    )
    .await;
    let live = body["usageLimit"]["usageLimitId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second limit, created and then deleted: its id named something once and names
    // nothing now, which is the same miss as an id nothing ever minted.
    let (_, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": namespace_arn,
            "usageType": "cross-region-datasharing",
            "amount": 3,
        }),
    )
    .await;
    let deleted = body["usageLimit"]["usageLimitId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, body) = call(
        &router,
        "DeleteUsageLimit",
        json!({ "usageLimitId": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteUsageLimit of a live limit: expected 200, got {status} with {body}"
    );

    let cases: [(&str, String); 6] = [
        (
            "a well-formed id nothing minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("an id that was deleted", deleted.clone()),
        ("a single character", "0".to_string()),
        (
            "the ARN instead of the id",
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:usagelimit/{live}"),
        ),
        ("a live id with a character appended", format!("{live}0")),
        (
            "a live id with its last character dropped",
            live[..live.len() - 1].to_string(),
        ),
    ];

    // Every operation that addresses a usage limit by id refuses the miss the same way.
    for operation in ["GetUsageLimit", "UpdateUsageLimit", "DeleteUsageLimit"] {
        for (case, id) in &cases {
            let (status, body) = call(&router, operation, json!({ "usageLimitId": id })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({operation}): expected 400 ValidationException, \
                 got {status} with {body}"
            );
            assert_eq!(
                body["message"],
                json!(format!("The usage limit ID is not valid. {id} does not exist")),
                "case {case:?} ({operation}): the message must name the id the request \
                 carried, got {:?}",
                body["message"]
            );
        }
    }

    // None of those refusals touched the limit that does exist.
    let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": live })).await;
    assert_eq!(
        (status, &body["usageLimit"]["usageLimitId"]),
        (200, &json!(live)),
        "the live id must still answer, got {status} with {body}"
    );
}

#[tokio::test]
async fn deleting_a_usage_limit_makes_it_unresolvable_and_frees_what_it_capped() {
    let router = emulator();
    let namespace_arn = namespace_arn_in(&router, "us-east-1", "delete-ns").await;

    // One limit per period, so there is something to keep as well as something to remove.
    let mut ids: Vec<(&str, String)> = Vec::new();
    for period in PERIODS {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": namespace_arn,
                "usageType": "serverless-compute",
                "period": period,
                "amount": 8,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {period}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        ids.push((
            period,
            body["usageLimit"]["usageLimitId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    // Remove them one at a time: each delete takes exactly its own record out of every
    // reader and leaves the others answering.
    let mut remaining = ids.clone();
    while let Some((period, id)) = remaining.pop() {
        let (status, body) = call(&router, "DeleteUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]["usageLimitId"]),
            (200, &json!(id)),
            "case {period}: DeleteUsageLimit got {status} with {body}"
        );

        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {period}: GetUsageLimit after the delete got {status} with {body}"
        );

        let (status, body) = call(
            &router,
            "ListUsageLimits",
            json!({ "resourceArn": namespace_arn }),
        )
        .await;
        let listed: Vec<String> = body["usageLimits"]
            .as_array()
            .map(|limits| {
                limits
                    .iter()
                    .map(|limit| {
                        limit["usageLimitId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(status, 200, "case {period}: ListUsageLimits got {body}");
        assert!(
            !listed.contains(&id),
            "case {period}: ListUsageLimits still reports the deleted limit {id:?}: {listed:?}"
        );
        for (still, kept) in &remaining {
            assert!(
                listed.contains(kept),
                "case {period}: ListUsageLimits dropped the {still} limit {kept:?}: {listed:?}"
            );
        }

        // The (resourceArn, usageType, period) triple the removed limit capped is free
        // again, so a create over it is accepted rather than a conflict — and it is a new
        // record, under a new id.
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": namespace_arn,
                "usageType": "serverless-compute",
                "period": period,
                "amount": 9,
            }),
        )
        .await;
        let fresh = body["usageLimit"]["usageLimitId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            status, 200,
            "case {period}: a create over the freed triple got {status} with {body}"
        );
        assert_ne!(
            fresh, id,
            "case {period}: the create over the freed triple must mint a new id, got {fresh:?}"
        );
        let (status, body) = call(
            &router,
            "DeleteUsageLimit",
            json!({ "usageLimitId": fresh }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {period}: tidying the replacement limit got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The members an update can change
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_changes_only_the_members_it_names_and_every_reader_reports_them() {
    // `UpdateUsageLimit` carries `amount` and `breachAction` and nothing else: "You can't
    // update the usage type or period of a usage limit."
    let cases: [(&str, Value, Option<i64>, Option<&str>); 4] = [
        ("neither member", json!({}), None, None),
        ("only the amount", json!({ "amount": 4096 }), Some(4096), None),
        (
            "only the breach action",
            json!({ "breachAction": "deactivate" }),
            None,
            Some("deactivate"),
        ),
        (
            "both members",
            json!({ "amount": 1, "breachAction": "emit-metric" }),
            Some(1),
            Some("emit-metric"),
        ),
    ];

    let router = emulator();
    let namespace_arn = namespace_arn_in(&router, "us-east-1", "update-ns").await;
    for (index, (case, update, new_amount, new_breach_action)) in cases.iter().enumerate() {
        // A distinct (usageType, period) per row, so each row owns its own limit.
        let (usage_type, _) = USAGE_TYPE_DIMENSIONS[index % USAGE_TYPE_DIMENSIONS.len()];
        let period = PERIODS[index % PERIODS.len()];
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": namespace_arn,
                "usageType": usage_type,
                "period": period,
                "amount": 60,
                "breachAction": "log",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUsageLimit expected 200, got {status} with {body}"
        );
        let created = body["usageLimit"].clone();
        let id = created["usageLimitId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let mut request = json!({ "usageLimitId": id });
        let object = request.as_object_mut().expect("an object request");
        for (key, value) in update.as_object().into_iter().flatten() {
            object.insert(key.clone(), value.clone());
        }
        let (status, body) = call(&router, "UpdateUsageLimit", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateUsageLimit expected 200, got {status} with {body}"
        );

        // What the update is expected to answer: the created record with the members the
        // request named replaced, and nothing else touched.
        let mut expected = created.clone();
        if let Some(amount) = new_amount {
            expected["amount"] = json!(amount);
        }
        if let Some(breach_action) = new_breach_action {
            expected["breachAction"] = json!(breach_action);
        }
        assert_eq!(
            body["usageLimit"], expected,
            "case {case:?}: UpdateUsageLimit expected {expected}, got {:?}",
            body["usageLimit"]
        );
        assert_dimensions(case, "UpdateUsageLimit", &body["usageLimit"], usage_type);

        // The write is observable through the read, and through the listing.
        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &expected),
            "case {case:?}: GetUsageLimit after the update got {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "ListUsageLimits",
            json!({ "resourceArn": namespace_arn, "usageType": usage_type }),
        )
        .await;
        let listed = body["usageLimits"]
            .as_array()
            .map(|limits| limits.to_vec())
            .unwrap_or_default();
        assert_eq!(status, 200, "case {case:?}: ListUsageLimits got {body}");
        assert!(
            listed.contains(&expected),
            "case {case:?}: ListUsageLimits must report the updated record {expected}, \
             got {listed:?}"
        );

        // And so is the record the delete hands back.
        let (status, body) = call(&router, "DeleteUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &expected),
            "case {case:?}: DeleteUsageLimit after the update got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_update_amount_that_is_not_positive_is_refused_and_stores_nothing() {
    // `UpdateUsageLimitRequest$amount`: "The value must be a positive number." The bound
    // and the first value past it are both covered, as is the smallest `Long`.
    let refused: [(&str, i64); 4] = [
        ("the bound itself", 0),
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
    let namespace_arn = namespace_arn_in(&router, "us-east-1", "update-amount-ns").await;
    let (_, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": namespace_arn,
            "usageType": "serverless-compute",
            "amount": 60,
        }),
    )
    .await;
    let created = body["usageLimit"].clone();
    let id = created["usageLimitId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, amount) in refused {
        let (status, body) = call(
            &router,
            "UpdateUsageLimit",
            json!({ "usageLimitId": id, "amount": amount }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} (amount {amount}): expected 400 ValidationException, \
             got {status} with {body}"
        );
        // The refusal stored nothing: the record still carries what the create stored.
        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &created),
            "case {case:?} (amount {amount}): the record must be untouched, \
             got {status} with {body}"
        );
    }

    for (case, amount) in accepted {
        let (status, body) = call(
            &router,
            "UpdateUsageLimit",
            json!({ "usageLimitId": id, "amount": amount }),
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

// ---------------------------------------------------------------------------
// A resource name that resolves to nothing
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_resource_name_that_resolves_to_nothing_is_refused_with_the_name_refusal_status() {
    let router = emulator();

    // A namespace and a workgroup that exist, and are then deleted: a name that resolved
    // once and resolves no longer is the same miss as a name nothing ever created.
    namespace_arn_in(&router, "us-east-1", "gone-ns").await;
    workgroup_arn_in(&router, "us-east-1", "gone-wg", "gone-ns").await;
    for (operation, request) in [
        ("DeleteWorkgroup", json!({ "workgroupName": "gone-wg" })),
        ("DeleteNamespace", json!({ "namespaceName": "gone-ns" })),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "{operation} of the resource to remove: got {status} with {body}"
        );
    }

    // Every operation that addresses one of this service's named resources, over names
    // that resolve to nothing: deleted, never created, and well-formed but unused
    // (`^[a-z0-9-]+$`, 3-64 characters).
    let workgroup_names = ["gone-wg", "absent-wg", "wg-0123456789"];
    let namespace_names = ["gone-ns", "absent-ns", "ns-0123456789"];
    let mut cases: Vec<(&str, Value, String)> = Vec::new();
    for name in workgroup_names {
        cases.push((
            "GetWorkgroup",
            json!({ "workgroupName": name }),
            name.to_string(),
        ));
        cases.push((
            "UpdateWorkgroup",
            json!({ "workgroupName": name, "baseCapacity": 32 }),
            name.to_string(),
        ));
        cases.push((
            "DeleteWorkgroup",
            json!({ "workgroupName": name }),
            name.to_string(),
        ));
    }
    for name in namespace_names {
        cases.push((
            "GetNamespace",
            json!({ "namespaceName": name }),
            name.to_string(),
        ));
        cases.push((
            "DeleteNamespace",
            json!({ "namespaceName": name }),
            name.to_string(),
        ));
    }

    for (operation, request, name) in &cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation}({name}): expected 400 ResourceNotFoundException, \
             got {status} with {body}"
        );
        // `ResourceNotFoundException$resourceName` is "the name of the resource that could
        // not be found", so it names what the request asked for.
        assert_eq!(
            body["resourceName"],
            json!(name),
            "case {operation}({name}): resourceName got {:?}",
            body["resourceName"]
        );
    }
}
