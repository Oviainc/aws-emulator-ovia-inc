// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the endpoint-group slice, driven over the wire.
//!
//! Every call goes through the real router, using the harness in
//! [`super::accelerator_lifecycle`] (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so these
//! exercise routing, deserialization, constraint validation, the handler and the response
//! envelope — not just the handler function.
//!
//! The rules under test are stated once each, as a table or an invariant over a SET of
//! inputs; a recorded request is one row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{arn_of, call, create_accelerator, emulator, error_code, ok};
// The envelope's message, in either spelling, already has one reader in this crate.
use super::input_refusals::error_message;

/// One accelerator plus one listener, the parents every endpoint group needs.
/// Returns the listener ARN.
async fn fixture(router: &MultiServiceRouter, suffix: &str) -> String {
    let accelerator = create_accelerator(router, &format!("eg-{suffix}"), &format!("acc-{suffix}")).await;
    listener(router, &arn_of(&accelerator), 80, 80, suffix).await
}

/// One listener on `accelerator_arn`, over the inclusive port range given.
async fn listener(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    from_port: i32,
    to_port: i32,
    token: &str,
) -> String {
    ok(
        router,
        "CreateListener",
        json!({
            "AcceleratorArn": accelerator_arn,
            "PortRanges": [{ "FromPort": from_port, "ToPort": to_port }],
            "Protocol": "TCP",
            "IdempotencyToken": format!("lsn-{token}"),
        }),
    )
    .await["Listener"]["ListenerArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CreateEndpointGroup` with the members a row names, plus the required ones.
async fn create_group(
    router: &MultiServiceRouter,
    listener_arn: &str,
    region: &str,
    token: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({
        "ListenerArn": listener_arn,
        "EndpointGroupRegion": region,
        "IdempotencyToken": token,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    call(router, "CreateEndpointGroup", body).await
}

fn group_arn(group: &Value) -> String {
    group["EndpointGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("EndpointGroupArn missing from {group}"))
        .to_string()
}

/// A created endpoint group, as `DescribeEndpointGroup` reports it.
async fn describe_group(router: &MultiServiceRouter, arn: &str) -> Value {
    ok(
        router,
        "DescribeEndpointGroup",
        json!({ "EndpointGroupArn": arn }),
    )
    .await["EndpointGroup"]
        .clone()
}

#[tokio::test]
async fn an_endpoint_group_round_trips_through_the_store() {
    let router = emulator();
    let listener_arn = fixture(&router, "roundtrip").await;

    // Rows differ along what the rule generalises over: which optional members the
    // caller names, so each row pins both the echoed values and the documented
    // defaults for the members it leaves out. The expectations are the member docs of
    // `CreateEndpointGroup`/`EndpointGroup`: TrafficDialPercentage 100, ThresholdCount 3,
    // HealthCheckIntervalSeconds 30, HealthCheckProtocol TCP, HealthCheckPort the
    // listener's first port, HealthCheckPath "/" for an HTTP/S check.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "all-defaults",
            json!({}),
            json!({
                "TrafficDialPercentage": 100.0,
                "HealthCheckPort": 80,
                "HealthCheckProtocol": "TCP",
                "HealthCheckIntervalSeconds": 30,
                "ThresholdCount": 3,
                "EndpointDescriptions": [],
                "PortOverrides": [],
            }),
        ),
        (
            "http-health-check",
            json!({ "HealthCheckProtocol": "HTTP", "HealthCheckIntervalSeconds": 10 }),
            json!({
                "TrafficDialPercentage": 100.0,
                "HealthCheckPort": 80,
                "HealthCheckProtocol": "HTTP",
                "HealthCheckPath": "/",
                "HealthCheckIntervalSeconds": 10,
                "ThresholdCount": 3,
            }),
        ),
        (
            "every-member-named",
            json!({
                "HealthCheckProtocol": "HTTPS",
                "HealthCheckPath": "/healthz",
                "HealthCheckPort": 8443,
                "HealthCheckIntervalSeconds": 30,
                "ThresholdCount": 10,
                "TrafficDialPercentage": 42.5,
                "PortOverrides": [
                    { "ListenerPort": 80, "EndpointPort": 1080 },
                    { "ListenerPort": 443, "EndpointPort": 1443 },
                ],
                "EndpointConfigurations": [
                    { "EndpointId": "i-0123456789abcdef0", "Weight": 7,
                      "ClientIPPreservationEnabled": false },
                    { "EndpointId": "198.51.100.7" },
                ],
            }),
            json!({
                "TrafficDialPercentage": 42.5,
                "HealthCheckPort": 8443,
                "HealthCheckProtocol": "HTTPS",
                "HealthCheckPath": "/healthz",
                "HealthCheckIntervalSeconds": 30,
                "ThresholdCount": 10,
                "PortOverrides": [
                    { "ListenerPort": 80, "EndpointPort": 1080 },
                    { "ListenerPort": 443, "EndpointPort": 1443 },
                ],
                "EndpointDescriptions": [
                    // The weight a caller names is kept; an endpoint without one takes
                    // the documented default of 128 (Endpoint weights, Global
                    // Accelerator Developer Guide). The emulator runs no health checks,
                    // so every endpoint is reported INITIAL.
                    { "EndpointId": "i-0123456789abcdef0", "Weight": 7,
                      "HealthState": "INITIAL", "ClientIPPreservationEnabled": false },
                    { "EndpointId": "198.51.100.7", "Weight": 128,
                      "HealthState": "INITIAL" },
                ],
            }),
        ),
    ];

    for (case, extra, expected) in rows {
        // Each row gets its own Region: one listener holds one group per Region.
        let region = format!("us-west-{}", case.len());
        let (status, body) = create_group(&router, &listener_arn, &region, case, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let created = body["EndpointGroup"].clone();
        let arn = group_arn(&created);
        assert_eq!(
            arn,
            format!("{listener_arn}/endpoint-group/{}", arn.rsplit('/').next().unwrap_or_default()),
            "case {case:?}: an endpoint group ARN hangs off its listener, got {arn:?}"
        );
        assert_eq!(
            created["EndpointGroupRegion"], region.as_str(),
            "case {case:?}: expected Region {region:?}, got {created}"
        );
        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                created.get(member).unwrap_or(&Value::Null),
                want,
                "case {case:?}: expected {member} {want}, got {created}"
            );
        }

        // The read-back is the same record, not a second computation of the request.
        let read = describe_group(&router, &arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: expected DescribeEndpointGroup to report the created group \
             {created}, got {read}"
        );
    }
}

#[tokio::test]
async fn the_health_check_interval_is_ten_or_thirty_seconds() {
    let router = emulator();
    let listener_arn = fixture(&router, "interval").await;

    // `HealthCheckIntervalSeconds`: "The time—10 seconds or 30 seconds—between each
    // health check for an endpoint." The model's own @range is 10..30, so every value
    // strictly between the two ends reaches the handler and must be refused there, and
    // the values outside the range are refused at the boundary. Each row is tried in a
    // fresh Region, and a refused create must leave that Region free.
    let rows: [(i32, bool); 8] = [
        (10, true),
        (30, true),
        (11, false),
        (15, false),
        (20, false),
        (29, false),
        // Past each published bound of the modelled range.
        (9, false),
        (31, false),
    ];

    for (interval, accepted) in rows {
        let case = format!("interval-{interval}");
        let region = format!("eu-west-{interval}");
        let (status, body) = create_group(
            &router,
            &listener_arn,
            &region,
            &case,
            json!({ "HealthCheckIntervalSeconds": interval }),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {interval} is a supported interval, got {status} with {body}"
            );
            assert_eq!(
                body["EndpointGroup"]["HealthCheckIntervalSeconds"], interval,
                "case {case:?}: expected the stored interval {interval}, got {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: {interval} is not a supported interval, got {status} with {body}"
        );
        // Inside the modelled range the refusal is the operation's own modelled error;
        // outside it, the boundary validator refuses the request before the handler.
        if (10..=30).contains(&interval) {
            assert_eq!(
                error_code(&body),
                "InvalidArgumentException",
                "case {case:?}: expected the modelled InvalidArgumentException, got {body}"
            );
        }
        // The refused create persisted nothing: the Region it named is still free.
        let (retry_status, retry) = create_group(
            &router,
            &listener_arn,
            &region,
            &format!("{case}-retry"),
            json!({ "HealthCheckIntervalSeconds": 10 }),
        )
        .await;
        assert_eq!(
            retry_status, 200,
            "case {case:?}: a refused create must write nothing, got {retry_status} with {retry}"
        );
    }
}

#[tokio::test]
async fn a_listener_holds_one_endpoint_group_per_region() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "eg-regions", "acc-regions").await;
    let accelerator_arn = arn_of(&accelerator);
    let first = listener(&router, &accelerator_arn, 80, 80, "regions-1").await;
    let second = listener(&router, &accelerator_arn, 443, 443, "regions-2").await;

    // Rows differ in the Region, the datum the rule is keyed on: each is free once per
    // listener, and the second create for the same Region is refused.
    let regions = [
        "us-west-2",
        "eu-central-1",
        "ap-southeast-1",
        "us-gov-west-1",
    ];

    for region in regions {
        let (status, body) =
            create_group(&router, &first, region, &format!("tok-{region}"), json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: the first group in a Region is created, got {status} with {body}"
        );
        let arn = group_arn(&body["EndpointGroup"]);

        // A second create for the same Region, with a DIFFERENT token, is refused.
        let (dup_status, dup) = create_group(
            &router,
            &first,
            region,
            &format!("tok-{region}-again"),
            json!({}),
        )
        .await;
        assert_eq!(
            dup_status, 400,
            "case {region:?}: a Region already covered is refused, got {dup_status} with {dup}"
        );
        assert_eq!(
            error_code(&dup),
            "EndpointGroupAlreadyExistsException",
            "case {region:?}: expected EndpointGroupAlreadyExistsException, got {dup}"
        );

        // The SAME token is the same request: it replays the group it minted.
        let (replay_status, replay) =
            create_group(&router, &first, region, &format!("tok-{region}"), json!({})).await;
        assert_eq!(
            replay_status, 200,
            "case {region:?}: a repeated token replays, got {replay_status} with {replay}"
        );
        assert_eq!(
            group_arn(&replay["EndpointGroup"]),
            arn,
            "case {region:?}: a repeated token must mint once, got {replay}"
        );

        // The rule is per LISTENER: the other listener may cover the same Region.
        let (sibling_status, sibling) = create_group(
            &router,
            &second,
            region,
            &format!("tok-{region}-sibling"),
            json!({}),
        )
        .await;
        assert_eq!(
            sibling_status, 200,
            "case {region:?}: another listener may cover the Region, got {sibling_status} \
             with {sibling}"
        );
        assert_ne!(
            group_arn(&sibling["EndpointGroup"]),
            arn,
            "case {region:?}: a sibling listener's group is its own resource, got {sibling}"
        );
    }
}

#[tokio::test]
async fn a_listener_with_an_endpoint_group_cannot_be_deleted() {
    let router = emulator();
    let listener_arn = fixture(&router, "dependents").await;

    // Rows: a listener with 1 and with 2 endpoint groups is refused the same way; with
    // none it deletes.
    let mut arns: Vec<String> = Vec::new();
    for region in ["us-east-1", "sa-east-1"] {
        let (status, body) =
            create_group(&router, &listener_arn, region, &format!("dep-{region}"), json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {body}"
        );
        arns.push(group_arn(&body["EndpointGroup"]));

        let (delete_status, refusal) = call(
            &router,
            "DeleteListener",
            json!({ "ListenerArn": listener_arn }),
        )
        .await;
        assert_eq!(
            delete_status, 400,
            "case {} group(s): a listener with endpoint groups is not deletable, got \
             {delete_status} with {refusal}",
            arns.len()
        );
        assert_eq!(
            error_code(&refusal),
            "AssociatedEndpointGroupFoundException",
            "case {} group(s): expected AssociatedEndpointGroupFoundException, got {refusal}",
            arns.len()
        );
        // A refusal about a named resource identifies it by the ARN the request carried.
        assert!(
            error_message(&refusal).contains(&listener_arn),
            "case {} group(s): expected the refusal to name the listener {listener_arn}, got {refusal}",
            arns.len()
        );
    }

    // The refused deletes were no-ops: the listener and both groups are still there.
    ok(
        &router,
        "DescribeListener",
        json!({ "ListenerArn": listener_arn }),
    )
    .await;
    for arn in &arns {
        let read = describe_group(&router, arn).await;
        assert_eq!(
            group_arn(&read),
            *arn,
            "case survivor: expected endpoint group {arn} to survive a refused listener \
             delete, got {read}"
        );
    }

    // Removing every dependent resource makes the listener deletable, and a deleted
    // group is gone from the store.
    for arn in &arns {
        ok(
            &router,
            "DeleteEndpointGroup",
            json!({ "EndpointGroupArn": arn }),
        )
        .await;
        let (status, body) = call(
            &router,
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case deleted {arn}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "EndpointGroupNotFoundException",
            "case deleted {arn}: expected EndpointGroupNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Endpoint group {arn} not found."),
            "case deleted {arn}: expected the refusal to name the endpoint group, got {body}"
        );
    }
    ok(
        &router,
        "DeleteListener",
        json!({ "ListenerArn": listener_arn }),
    )
    .await;
}

#[tokio::test]
async fn an_endpoint_group_needs_a_listener_the_store_holds() {
    let router = emulator();
    let listener_arn = fixture(&router, "missing").await;

    // Rows: an ARN of the right shape that was never minted, and one whose listener was
    // deleted. Both name no listener, so both are the modelled 404.
    let deleted = listener(
        &router,
        &arn_of(&create_accelerator(&router, "eg-gone", "acc-gone").await),
        90,
        90,
        "gone",
    )
    .await;
    ok(&router, "DeleteListener", json!({ "ListenerArn": deleted })).await;

    for (case, arn) in [
        ("never-minted", format!("{listener_arn}-not-a-listener")),
        ("deleted", deleted.clone()),
    ] {
        let (status, body) = create_group(&router, &arn, "us-west-2", case, json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ListenerNotFoundException",
            "case {case:?}: expected ListenerNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Listener {arn} not found."),
            "case {case:?}: expected the refusal to name the listener, got {body}"
        );
    }

    // The same rule for the readers of a group that does not exist.
    for operation in ["DescribeEndpointGroup", "DeleteEndpointGroup"] {
        let (status, body) = call(
            &router,
            operation,
            json!({ "EndpointGroupArn": format!("{listener_arn}/endpoint-group/deadbeef") }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {operation}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "EndpointGroupNotFoundException",
            "case {operation}: expected EndpointGroupNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_malformed_endpoint_or_port_override_is_refused() {
    let router = emulator();
    let listener_arn = fixture(&router, "refusals").await;

    // Rows: each names ONE way a request describes no endpoint or no mapping, with the
    // modelled error it earns. `AttachmentArn` names a cross-account attachment, and the
    // store holds none (the attachment operations are not implemented), so no attachment
    // grants permission to add that endpoint.
    let rows: [(&str, Value, u16, &str); 6] = [
        (
            "endpoint-without-id",
            json!({ "EndpointConfigurations": [{ "Weight": 10 }] }),
            400,
            "InvalidArgumentException",
        ),
        (
            "endpoint-named-twice",
            json!({ "EndpointConfigurations": [
                { "EndpointId": "i-00000000000000001" },
                { "EndpointId": "i-00000000000000001", "Weight": 3 },
            ] }),
            400,
            "InvalidArgumentException",
        ),
        (
            "override-without-endpoint-port",
            json!({ "PortOverrides": [{ "ListenerPort": 80 }] }),
            400,
            "InvalidArgumentException",
        ),
        (
            "override-listener-port-twice",
            json!({ "PortOverrides": [
                { "ListenerPort": 80, "EndpointPort": 1080 },
                { "ListenerPort": 80, "EndpointPort": 1081 },
            ] }),
            400,
            "InvalidArgumentException",
        ),
        (
            "empty-region",
            json!({ "EndpointGroupRegion": "" }),
            400,
            "InvalidArgumentException",
        ),
        (
            "unknown-attachment",
            json!({ "EndpointConfigurations": [
                { "EndpointId": "i-00000000000000002",
                  "AttachmentArn": "arn:aws:globalaccelerator::000000000000:attachment/abc" },
            ] }),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, extra, expected_status, expected_code) in rows {
        let (status, body) = create_group(&router, &listener_arn, "ca-central-1", case, extra).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A list longer than the model's own `@length` (0..10) on `EndpointConfigurations`
    // and `PortOverrides` is refused too, so neither the work a create does nor what it
    // stores is proportional to a length the caller chose.
    for (case, extra) in [
        (
            "eleven-endpoints",
            json!({ "EndpointConfigurations": (0..11)
                .map(|index| json!({ "EndpointId": format!("i-{index:017}") }))
                .collect::<Vec<Value>>() }),
        ),
        (
            "eleven-port-overrides",
            json!({ "PortOverrides": (0..11)
                .map(|index| json!({ "ListenerPort": 80 + index, "EndpointPort": 1080 + index }))
                .collect::<Vec<Value>>() }),
        ),
    ] {
        let (status, body) = create_group(&router, &listener_arn, "ca-central-1", case, extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // None of them wrote: the Region every row named is still free.
    let (status, body) =
        create_group(&router, &listener_arn, "ca-central-1", "after-refusals", json!({})).await;
    assert_eq!(
        status, 200,
        "case after-refusals: a refused create must write nothing, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_create_past_the_endpoint_group_quota_is_refused_and_a_delete_frees_a_slot() {
    let router = emulator();
    let accelerator = create_accelerator(&router, "eg-quota", "acc-quota").await;
    let accelerator_arn = arn_of(&accelerator);
    // Two listeners, so the quota is tested as a PER-ACCELERATOR total rather than a
    // per-listener one: the groups below are split across both.
    let listeners = [
        listener(&router, &accelerator_arn, 80, 80, "quota-1").await,
        listener(&router, &accelerator_arn, 443, 443, "quota-2").await,
    ];

    // The quota as a boundary: every create up to it succeeds, the one past it is the
    // modelled LimitExceededException on EITHER listener, and deleting a group frees
    // exactly one slot.
    let quota = 10;
    let mut arns: Vec<String> = Vec::new();
    for index in 0..quota {
        let region = format!("quota-region-{index}");
        let parent = &listeners[index % listeners.len()];
        let (status, body) =
            create_group(&router, parent, &region, &format!("q-{index}"), json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: group {} of {quota} is within the quota, got {status} with {body}",
            index + 1
        );
        arns.push(group_arn(&body["EndpointGroup"]));
    }

    for parent in &listeners {
        let (status, body) =
            create_group(&router, parent, "quota-region-over", "q-over", json!({})).await;
        assert_eq!(
            status, 403,
            "case over-quota on {parent}: expected HTTP 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceededException",
            "case over-quota on {parent}: expected LimitExceededException, got {body}"
        );
        assert!(
            error_message(&body).contains(&accelerator_arn),
            "case over-quota on {parent}: expected the refusal to name the accelerator, got {body}"
        );
    }

    ok(
        &router,
        "DeleteEndpointGroup",
        json!({ "EndpointGroupArn": arns[0] }),
    )
    .await;
    let (status, body) = create_group(
        &router,
        &listeners[1],
        "quota-region-over",
        "q-over-2",
        json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case freed-slot: a delete frees one slot for any listener of the accelerator, \
         got {status} with {body}"
    );
}
