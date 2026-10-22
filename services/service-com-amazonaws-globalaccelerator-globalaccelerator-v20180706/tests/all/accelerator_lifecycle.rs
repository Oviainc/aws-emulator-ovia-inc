// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the accelerator/listener slice, driven over the wire.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so these
//! exercise routing, deserialization, constraint validation, the handler and the response
//! envelope — not just the handler function.
//!
//! The rules under test are stated once each, as a table or an invariant over a SET of
//! inputs; the issue's own request is one row.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator: one shared engine, this service mounted, signature checking off
/// (the tests are about the service's behaviour, not SigV4).
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_globalaccelerator_globalaccelerator_v20180706::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return `(status, body)`.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "globalaccelerator.us-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("GlobalAccelerator_V20180706.{operation}"),
        )
        // The routing key comes from the SigV4 credential scope's service field.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-west-2/globalaccelerator/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation and require a 200.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code out of an awsJson error envelope's `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create one accelerator and return its record.
pub(crate) async fn create_accelerator(router: &MultiServiceRouter, name: &str, token: &str) -> Value {
    ok(
        router,
        "CreateAccelerator",
        json!({ "Name": name, "IdempotencyToken": token }),
    )
    .await["Accelerator"]
        .clone()
}

pub(crate) fn arn_of(accelerator: &Value) -> String {
    accelerator["AcceleratorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("AcceleratorArn missing from {accelerator}"))
        .to_string()
}

/// `a` + 16 hex digits, the DNS label `Accelerator$DnsName` describes.
fn assert_dns_label(case: &str, host: &str, suffix: &str) {
    let label = host
        .strip_suffix(suffix)
        .unwrap_or_else(|| panic!("case {case}: expected a host ending in {suffix}, got {host:?}"));
    let digits = label
        .strip_prefix('a')
        .unwrap_or_else(|| panic!("case {case}: expected a label starting with 'a', got {label:?}"));
    assert_eq!(
        digits.len(),
        16,
        "case {case}: expected a 16-digit hex label, got {digits:?}"
    );
    assert!(
        digits.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case}: expected lowercase hex in the label, got {digits:?}"
    );
}

#[tokio::test]
async fn an_accelerator_and_its_listener_round_trip_through_the_store() {
    let router = emulator();

    let created = ok(
        &router,
        "CreateAccelerator",
        json!({
            "Name": "bootstrap-acc",
            "IdempotencyToken": "tok-1",
            "Enabled": true,
            "Tags": [{ "Key": "team", "Value": "net" }],
        }),
    )
    .await["Accelerator"]
        .clone();
    let arn = arn_of(&created);

    assert!(
        arn.starts_with("arn:aws:globalaccelerator::") && arn.contains(":accelerator/"),
        "case create: expected a Global Accelerator ARN, got {arn:?}"
    );
    assert_eq!(
        created["Name"], "bootstrap-acc",
        "case create: expected the requested Name, got {created}"
    );
    assert_eq!(
        created["Enabled"], true,
        "case create: expected Enabled true, got {created}"
    );
    // Deterministic, not timing-dependent: the create response's own `Status` is computed
    // against the same instant that set the deployment's due instant 500 ms later.
    assert_eq!(
        created["Status"], "IN_PROGRESS",
        "case create: a new accelerator deploys before it is DEPLOYED, got {created}"
    );
    assert!(
        created["CreatedTime"].is_number() && created["LastModifiedTime"].is_number(),
        "case create: expected both timestamps, got {created}"
    );
    assert_dns_label(
        "create",
        created["DnsName"].as_str().unwrap_or_default(),
        ".awsglobalaccelerator.com",
    );
    let ip_sets = created["IpSets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        ip_sets.len(),
        1,
        "case create: an IPV4 accelerator has one IP set, got {created}"
    );
    assert_eq!(
        ip_sets[0]["IpAddressFamily"], "IPv4",
        "case create: expected an IPv4 set, got {}",
        ip_sets[0]
    );
    assert_eq!(
        ip_sets[0]["IpAddresses"].as_array().map(Vec::len),
        Some(2),
        "case create: expected two static addresses, got {}",
        ip_sets[0]
    );

    // DescribeAccelerator reports what CreateAccelerator persisted.
    let described = ok(
        &router,
        "DescribeAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await["Accelerator"]
        .clone();
    for member in [
        "AcceleratorArn",
        "Name",
        "DnsName",
        "IpSets",
        "Enabled",
        "CreatedTime",
    ] {
        assert_eq!(
            described[member], created[member],
            "case describe {member}: expected the persisted value {}, got {}",
            created[member], described[member]
        );
    }

    // ListAccelerators sees it.
    let listed = ok(&router, "ListAccelerators", json!({})).await;
    let arns: Vec<String> = listed["Accelerators"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "case list: expected exactly the created accelerator, got {listed}"
    );

    // CreateListener, then read it back.
    let listener = ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": arn,
            "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            "Protocol": "TCP",
            "IdempotencyToken": "tok-2",
        }),
    )
    .await["Listener"]
        .clone();
    let listener_arn = listener["ListenerArn"].as_str().unwrap_or_default().to_string();
    assert!(
        listener_arn.starts_with(&format!("{arn}/listener/")),
        "case create listener: expected an ARN under {arn:?}, got {listener_arn:?}"
    );
    assert_eq!(
        listener["ClientAffinity"], "NONE",
        "case create listener: ClientAffinity defaults to NONE, got {listener}"
    );

    let described_listener = ok(
        &router,
        "DescribeListener",
        json!({ "ListenerArn": listener_arn }),
    )
    .await["Listener"]
        .clone();
    assert_eq!(
        described_listener, listener,
        "case describe listener: expected the persisted listener {listener}, got {described_listener}"
    );

    let listeners = ok(
        &router,
        "ListListeners",
        json!({ "AcceleratorArn": arn }),
    )
    .await["Listeners"]
        .clone();
    assert_eq!(
        listeners,
        json!([listener]),
        "case list listeners: expected exactly the created listener, got {listeners}"
    );

    // Teardown enforces both documented preconditions before it removes anything.
    let (status, body) = call(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AcceleratorNotDisabledException"),
        "case delete while enabled: got {status} {body}"
    );

    let updated = ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": arn, "Enabled": false }),
    )
    .await["Accelerator"]
        .clone();
    assert_eq!(
        updated["Enabled"], false,
        "case update: expected Enabled false, got {updated}"
    );
    assert!(
        updated["LastModifiedTime"].as_f64() >= created["LastModifiedTime"].as_f64(),
        "case update: expected LastModifiedTime to advance, got {updated}"
    );
    let reread = ok(
        &router,
        "DescribeAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await["Accelerator"]
        .clone();
    assert_eq!(
        reread["Enabled"], false,
        "case update persisted: expected Enabled false, got {reread}"
    );

    let (status, body) = call(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AssociatedListenerFoundException"),
        "case delete with a listener: got {status} {body}"
    );

    ok(
        &router,
        "DeleteListener",
        json!({ "ListenerArn": listener_arn }),
    )
    .await;
    let listeners = ok(
        &router,
        "ListListeners",
        json!({ "AcceleratorArn": arn }),
    )
    .await["Listeners"]
        .clone();
    assert_eq!(
        listeners,
        json!([]),
        "case list after delete listener: expected no listeners, got {listeners}"
    );

    ok(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await;
    let (status, body) = call(
        &router,
        "DescribeAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "AcceleratorNotFoundException"),
        "case describe after delete: got {status} {body}"
    );
    let listed = ok(&router, "ListAccelerators", json!({})).await;
    assert_eq!(
        listed["Accelerators"],
        json!([]),
        "case list after delete: expected an empty list, got {listed}"
    );
}

/// RULE: a resource ARN the store does not hold is the operation's own
/// not-found error, with the status the model declares — never a success.
#[tokio::test]
async fn an_unknown_arn_is_the_modelled_not_found_error() {
    let router = emulator();
    let missing_accelerator = "arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-4000-8000-000000000000";
    let missing_listener = format!("{missing_accelerator}/listener/deadbeef");

    // (operation, body, expected error code)
    let cases: Vec<(&str, Value, &str, u16)> = vec![
        (
            "DescribeAccelerator",
            json!({ "AcceleratorArn": missing_accelerator }),
            "AcceleratorNotFoundException",
            404,
        ),
        (
            "DeleteAccelerator",
            json!({ "AcceleratorArn": missing_accelerator }),
            "AcceleratorNotFoundException",
            404,
        ),
        (
            "UpdateAccelerator",
            json!({ "AcceleratorArn": missing_accelerator, "Enabled": false }),
            "AcceleratorNotFoundException",
            404,
        ),
        (
            "ListListeners",
            json!({ "AcceleratorArn": missing_accelerator }),
            "AcceleratorNotFoundException",
            404,
        ),
        (
            "CreateListener",
            json!({
                "AcceleratorArn": missing_accelerator,
                "PortRanges": [{ "FromPort": 443, "ToPort": 443 }],
                "Protocol": "TCP",
                "IdempotencyToken": "tok-missing",
            }),
            "AcceleratorNotFoundException",
            404,
        ),
        (
            "DescribeListener",
            json!({ "ListenerArn": missing_listener }),
            "ListenerNotFoundException",
            404,
        ),
        (
            "DeleteListener",
            json!({ "ListenerArn": missing_listener }),
            "ListenerNotFoundException",
            404,
        ),
    ];

    for (operation, body, expected_code, expected_status) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (expected_status, expected_code),
            "case {operation}: got {status} {parsed}"
        );
    }
}

/// RULE: `IpAddressType` decides how many address families an accelerator gets and
/// whether it carries the second (dual-stack) DNS name; it defaults to IPV4.
#[tokio::test]
async fn ip_address_type_decides_the_address_families_and_dns_names() {
    let router = emulator();

    // (case, requested IpAddressType, expected effective type, expected families)
    let cases: Vec<(&str, Option<&str>, &str, Vec<&str>)> = vec![
        ("omitted", None, "IPV4", vec!["IPv4"]),
        ("ipv4", Some("IPV4"), "IPV4", vec!["IPv4"]),
        (
            "dual-stack",
            Some("DUAL_STACK"),
            "DUAL_STACK",
            vec!["IPv4", "IPv6"],
        ),
    ];

    for (case, requested, expected_type, expected_families) in cases {
        let mut body = json!({
            "Name": format!("acc-{case}"),
            "IdempotencyToken": format!("tok-{case}"),
        });
        if let Some(requested) = requested {
            body["IpAddressType"] = json!(requested);
        }
        let accelerator = ok(&router, "CreateAccelerator", body).await["Accelerator"].clone();

        assert_eq!(
            accelerator["IpAddressType"], expected_type,
            "case {case}: expected IpAddressType {expected_type}, got {accelerator}"
        );
        let families: Vec<String> = accelerator["IpSets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|set| set["IpAddressFamily"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            families, expected_families,
            "case {case}: expected the families {expected_families:?}, got {accelerator}"
        );
        for set in accelerator["IpSets"].as_array().cloned().unwrap_or_default() {
            assert_eq!(
                set["IpAddresses"].as_array().map(Vec::len),
                Some(2),
                "case {case}: every IP set holds two addresses, got {set}"
            );
            assert_eq!(
                set["IpFamily"], set["IpAddressFamily"],
                "case {case}: IpFamily is the deprecated alias of IpAddressFamily, got {set}"
            );
        }

        assert_dns_label(
            case,
            accelerator["DnsName"].as_str().unwrap_or_default(),
            ".awsglobalaccelerator.com",
        );
        if expected_type == "DUAL_STACK" {
            assert_dns_label(
                case,
                accelerator["DualStackDnsName"].as_str().unwrap_or_default(),
                ".dualstack.awsglobalaccelerator.com",
            );
        } else {
            assert!(
                accelerator["DualStackDnsName"].is_null(),
                "case {case}: an IPv4 accelerator has no DualStackDnsName, got {accelerator}"
            );
        }
    }

    // Every accelerator's static addresses are unique across the account.
    let listed = ok(&router, "ListAccelerators", json!({})).await;
    let mut addresses: Vec<String> = listed["Accelerators"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|accelerator| accelerator["IpSets"].as_array().cloned().unwrap_or_default())
        .flat_map(|set| set["IpAddresses"].as_array().cloned().unwrap_or_default())
        .map(|address| address.as_str().unwrap_or_default().to_string())
        .collect();
    let total = addresses.len();
    addresses.sort();
    addresses.dedup();
    assert_eq!(
        addresses.len(),
        total,
        "case uniqueness: allocated static addresses must not repeat, got {addresses:?}"
    );
}

/// RULE: an accelerator name may hold only alphanumerics, periods and hyphens, may not
/// begin or end with a period or hyphen, and may not exceed 64 characters
/// (`CreateAccelerator$Name` doc). The same rule governs `UpdateAccelerator$Name`.
#[tokio::test]
async fn an_accelerator_name_follows_the_documented_character_rule() {
    let router = emulator();

    // (case, name, accepted) — the boundary (64) and one past it (65) are both here.
    let cases: Vec<(&str, String, bool)> = vec![
        ("alphanumeric", "acc1".to_string(), true),
        ("hyphenated", "my-accel-01".to_string(), true),
        ("dotted", "my.accel.01".to_string(), true),
        ("at-the-64-char-bound", "a".repeat(64), true),
        ("one-past-the-bound", "a".repeat(65), false),
        ("leading-hyphen", "-acc".to_string(), false),
        ("trailing-hyphen", "acc-".to_string(), false),
        ("leading-period", ".acc".to_string(), false),
        ("trailing-period", "acc.".to_string(), false),
        ("underscore", "my_accel".to_string(), false),
        ("space", "my accel".to_string(), false),
        ("slash", "my/accel".to_string(), false),
    ];

    for (index, (case, name, accepted)) in cases.iter().enumerate() {
        let (status, parsed) = call(
            &router,
            "CreateAccelerator",
            json!({ "Name": name, "IdempotencyToken": format!("create-{index}") }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case create {case}: expected the name to be accepted, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidArgumentException"),
                "case create {case}: expected the name to be refused, got {status} {parsed}"
            );
        }
    }

    // The rule holds on the update path too, over the same table.
    let arn = arn_of(&create_accelerator(&router, "rename-me", "rename-token").await);
    for (case, name, accepted) in &cases {
        let (status, parsed) = call(
            &router,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "Name": name }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case update {case}: expected the name to be accepted, got {status} {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidArgumentException"),
                "case update {case}: expected the name to be refused, got {status} {parsed}"
            );
        }
    }
}

/// RULE: a listener's port range is the inclusive pair (`FromPort`, `ToPort`), so a range
/// that names no ports — one end missing, or `FromPort` past `ToPort` — is
/// `InvalidPortRangeException`, and nothing is persisted.
#[tokio::test]
async fn a_port_range_that_names_no_ports_is_refused() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "ports", "ports-token").await);

    // (case, PortRanges, accepted). The model's own 1..65535 bound is checked by the
    // boundary validator, so a value outside it is refused before the handler runs.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("single-port", json!([{ "FromPort": 80, "ToPort": 80 }]), true),
        ("span", json!([{ "FromPort": 80, "ToPort": 443 }]), true),
        (
            "lowest-port",
            json!([{ "FromPort": 1, "ToPort": 1 }]),
            true,
        ),
        (
            "highest-port",
            json!([{ "FromPort": 65535, "ToPort": 65535 }]),
            true,
        ),
        (
            "several-ranges",
            json!([
                { "FromPort": 80, "ToPort": 80 },
                { "FromPort": 8080, "ToPort": 8090 },
            ]),
            true,
        ),
        (
            "reversed",
            json!([{ "FromPort": 443, "ToPort": 80 }]),
            false,
        ),
        ("missing-to-port", json!([{ "FromPort": 80 }]), false),
        ("missing-from-port", json!([{ "ToPort": 80 }]), false),
        (
            "one-good-one-reversed",
            json!([
                { "FromPort": 80, "ToPort": 80 },
                { "FromPort": 9000, "ToPort": 8000 },
            ]),
            false,
        ),
    ];

    let mut accepted_count = 0usize;
    for (index, (case, port_ranges, accepted)) in cases.iter().enumerate() {
        let (status, parsed) = call(
            &router,
            "CreateListener",
            json!({
                "AcceleratorArn": arn,
                "PortRanges": port_ranges,
                "Protocol": "TCP",
                "IdempotencyToken": format!("listener-{index}"),
            }),
        )
        .await;
        if *accepted {
            accepted_count += 1;
            assert_eq!(
                status, 200,
                "case {case}: expected the port ranges to be accepted, got {status} {parsed}"
            );
            assert_eq!(
                parsed["Listener"]["PortRanges"], *port_ranges,
                "case {case}: expected the stored port ranges back, got {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidPortRangeException"),
                "case {case}: expected the port ranges to be refused, got {status} {parsed}"
            );
        }
    }

    // A refused create persisted nothing.
    let listeners = ok(&router, "ListListeners", json!({ "AcceleratorArn": arn })).await
        ["Listeners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listeners.len(),
        accepted_count,
        "case persistence: expected only the accepted listeners, got {listeners:?}"
    );
}

/// RULE: a paginated list partitions the collection — every page but the last is
/// `MaxResults` long, the pages together are the whole collection in order with no
/// repeat, and only the last page omits `NextToken`.
#[tokio::test]
async fn paginated_lists_partition_the_collection() {
    // (case, total, MaxResults)
    let cases: Vec<(&str, usize, Option<i64>)> = vec![
        ("empty-no-limit", 0, None),
        ("empty-limited", 0, Some(1)),
        ("one-no-limit", 1, None),
        ("one-page-of-one", 1, Some(1)),
        ("three-pages-of-one", 3, Some(1)),
        ("three-pages-of-two", 3, Some(2)),
        ("five-pages-of-two", 5, Some(2)),
        ("limit-above-total", 2, Some(100)),
    ];

    for (case, total, max_results) in cases {
        let router = emulator();
        let mut expected = Vec::new();
        for index in 0..total {
            let accelerator =
                create_accelerator(&router, &format!("acc-{index}"), &format!("tok-{index}")).await;
            expected.push(arn_of(&accelerator));
        }
        // ListAccelerators reports the store's own stable order.
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListAccelerators", body).await;
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );

            let items: Vec<String> = page["Accelerators"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(arn_of)
                .collect();
            next_token = page["NextToken"].as_str().map(str::to_string);
            if next_token.is_some() {
                // A list with no `MaxResults` is clamped server-side to the model's own
                // upper bound (100), so that is the effective page size here.
                assert_eq!(
                    items.len(),
                    max_results.unwrap_or(100) as usize,
                    "case {case}: a non-final page must be full, got {page}"
                );
            }
            seen.extend(items);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case}: the pages must partition the collection in order, got {seen:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not issue is `InvalidNextTokenException`, on every
/// paginated list.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "tokens", "tokens-token").await);

    // Not hex, hex that is not a key this collection holds, and a token issued for the
    // other collection.
    let tokens = [
        "not-a-token",
        "zzzz",
        "6e6f742d616e2d61726e",
        &hex::encode("arn:aws:globalaccelerator::000000000000:accelerator/nope"),
    ];

    for token in tokens {
        for (operation, body) in [
            ("ListAccelerators", json!({ "NextToken": token })),
            (
                "ListListeners",
                json!({ "AcceleratorArn": arn, "NextToken": token }),
            ),
        ] {
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidNextTokenException"),
                "case {operation} with {token:?}: got {status} {parsed}"
            );
        }
    }
}

/// RULE: repeating a create with the same `IdempotencyToken` returns the resource that
/// token already minted, rather than minting a second one.
#[tokio::test]
async fn repeating_a_create_with_one_idempotency_token_mints_once() {
    let router = emulator();

    // Several distinct tokens, each replayed, so the rule is tested as a mapping rather
    // than on one value.
    for token in ["tok-a", "tok-b", "tok-c"] {
        let first = create_accelerator(&router, "idem", token).await;
        let second = create_accelerator(&router, "idem", token).await;
        // The identity-bearing members, not the whole record: `Status` is derived from the
        // clock against the stored due instant, so two calls that straddle it legitimately
        // report different statuses for the same accelerator.
        for member in [
            "AcceleratorArn",
            "Name",
            "IpAddressType",
            "Enabled",
            "IpSets",
            "DnsName",
            "DualStackDnsName",
            "CreatedTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                first[member], second[member],
                "case accelerator {token} {member}: expected the same accelerator back, \
                 first {}, replay {}",
                first[member], second[member]
            );
        }
    }
    let listed = ok(&router, "ListAccelerators", json!({})).await["Accelerators"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        3,
        "case accelerator count: three tokens mint three accelerators, got {listed:?}"
    );

    let arn = arn_of(&listed[0]);
    for token in ["listener-a", "listener-b"] {
        let request = json!({
            "AcceleratorArn": arn,
            "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            "Protocol": "TCP",
            "IdempotencyToken": token,
        });
        let first = ok(&router, "CreateListener", request.clone()).await["Listener"].clone();
        let second = ok(&router, "CreateListener", request).await["Listener"].clone();
        assert_eq!(
            first, second,
            "case listener {token}: expected the same listener back, got {second}"
        );
    }
    let listeners = ok(&router, "ListListeners", json!({ "AcceleratorArn": arn })).await
        ["Listeners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listeners.len(),
        2,
        "case listener count: two tokens mint two listeners, got {listeners:?}"
    );

    // A listener token is scoped to its parent: the SAME token against a different
    // accelerator is a different request, and must not replay the first parent's listener.
    let other = arn_of(&listed[1]);
    let shared_token = "listener-a";
    let under_other = ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": other,
            "PortRanges": [{ "FromPort": 8443, "ToPort": 8443 }],
            "Protocol": "TCP",
            "IdempotencyToken": shared_token,
        }),
    )
    .await["Listener"]
        .clone();
    let under_other_arn = under_other["ListenerArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        under_other_arn.starts_with(&format!("{other}/listener/")),
        "case cross-parent token: expected a listener under {other:?}, got {under_other_arn:?}"
    );
    assert_eq!(
        under_other["PortRanges"],
        json!([{ "FromPort": 8443, "ToPort": 8443 }]),
        "case cross-parent token: expected this request's own port ranges, got {under_other}"
    );
    let still_first = ok(&router, "ListListeners", json!({ "AcceleratorArn": arn })).await
        ["Listeners"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        still_first.len(),
        2,
        "case cross-parent token: the first accelerator's listeners are unchanged, got {still_first:?}"
    );
}

/// RULE: the deployment a create or an update starts is owed by the clock — the
/// accelerator reads `IN_PROGRESS` straight after the call and `DEPLOYED` once the due
/// instant has passed — and reading it changes nothing a later caller sees.
#[tokio::test]
async fn a_deployment_settles_on_the_clock_and_reads_do_not_move_it() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "settles", "settles-token").await);

    async fn status_now(router: &MultiServiceRouter, arn: &str) -> String {
        ok(
            router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    // Reading before the due instant must not settle it early. The settle span is at
    // least 250 ms, so a read that completes inside that window must still report
    // IN_PROGRESS; past it, either value is legitimate and the read is still exercised.
    let started = std::time::Instant::now();
    for attempt in 0..2 {
        let observed = status_now(&router, &arn).await;
        if started.elapsed() < Duration::from_millis(250) {
            assert_eq!(
                observed, "IN_PROGRESS",
                "case read {attempt} inside the settle window: got {observed}"
            );
        } else {
            assert!(
                observed == "IN_PROGRESS" || observed == "DEPLOYED",
                "case read {attempt} past the settle window: got {observed}"
            );
        }
    }

    tokio::time::sleep(Duration::from_millis(900)).await;
    let observed = status_now(&router, &arn).await;
    assert_eq!(
        observed, "DEPLOYED",
        "case read after the due instant: got {observed}"
    );
    let listed = ok(&router, "ListAccelerators", json!({})).await;
    assert_eq!(
        listed["Accelerators"][0]["Status"], "DEPLOYED",
        "case list after the due instant: got {listed}"
    );

    // An update restarts the deployment.
    let updated = ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": arn, "Name": "settles-again" }),
    )
    .await["Accelerator"]
        .clone();
    assert_eq!(
        updated["Status"], "IN_PROGRESS",
        "case update: an update redeploys the accelerator, got {updated}"
    );
}

/// RULE: supplied `IpAddresses` lead the accelerator's IPv4 set and the service allocates
/// the rest of the two; an entry that is not an IPv4 address is `InvalidArgumentException`.
#[tokio::test]
async fn supplied_ip_addresses_lead_the_ipv4_set() {
    let router = emulator();

    // (case, IpAddresses, expected leading addresses)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("none", json!([]), vec![]),
        ("one", json!(["192.0.2.10"]), vec!["192.0.2.10"]),
        (
            "two",
            json!(["192.0.2.20", "198.51.100.30"]),
            vec!["192.0.2.20", "198.51.100.30"],
        ),
    ];

    for (index, (case, ip_addresses, expected_leading)) in cases.iter().enumerate() {
        let accelerator = ok(
            &router,
            "CreateAccelerator",
            json!({
                "Name": format!("byoip-{case}"),
                "IdempotencyToken": format!("byoip-{index}"),
                "IpAddresses": ip_addresses,
            }),
        )
        .await["Accelerator"]
            .clone();
        let addresses: Vec<String> = accelerator["IpSets"][0]["IpAddresses"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|address| address.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            addresses.len(),
            2,
            "case {case}: an accelerator has two static IPv4 addresses, got {addresses:?}"
        );
        assert_eq!(
            &addresses[..expected_leading.len()],
            expected_leading.as_slice(),
            "case {case}: expected the supplied addresses to lead, got {addresses:?}"
        );
    }

    for (case, bad) in [
        ("hostname", "not-an-ip"),
        ("ipv6", "2001:db8::1"),
        ("truncated", "10.0.0"),
        ("out-of-range-octet", "10.0.0.256"),
    ] {
        let (status, parsed) = call(
            &router,
            "CreateAccelerator",
            json!({
                "Name": "byoip-bad",
                "IdempotencyToken": format!("byoip-bad-{case}"),
                "IpAddresses": [bad],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidArgumentException"),
            "case {case}: expected {bad:?} to be refused, got {status} {parsed}"
        );
    }

    // An accelerator's two static IPv4 addresses are distinct, so the same address twice
    // is refused — on the create and the update path alike.
    let duplicated = json!(["192.0.2.40", "192.0.2.40"]);
    let (status, parsed) = call(
        &router,
        "CreateAccelerator",
        json!({
            "Name": "byoip-dup",
            "IdempotencyToken": "byoip-dup",
            "IpAddresses": duplicated,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidArgumentException"),
        "case duplicate on create: got {status} {parsed}"
    );
    let arn = arn_of(&create_accelerator(&router, "byoip-update", "byoip-update").await);
    let (status, parsed) = call(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": arn, "IpAddresses": duplicated }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidArgumentException"),
        "case duplicate on update: got {status} {parsed}"
    );
    // And the update path validates each entry the same way the create path does.
    let (status, parsed) = call(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": arn, "IpAddresses": ["not-an-ip"] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidArgumentException"),
        "case non-IPv4 on update: got {status} {parsed}"
    );
}

/// RULE: changing `IpAddressType` changes which address families the accelerator holds
/// and whether it carries the dual-stack DNS name, and is idempotent; the accelerator's
/// own DNS label never changes.
#[tokio::test]
async fn updating_the_ip_address_type_adds_and_removes_the_ipv6_set() {
    let router = emulator();
    let created = create_accelerator(&router, "families", "families-token").await;
    let arn = arn_of(&created);
    let label = created["DnsName"]
        .as_str()
        .unwrap_or_default()
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string();

    // (case, requested type, expected families, expects a DualStackDnsName). Each type is
    // requested twice, so the transition is tested for idempotence as well.
    let steps: Vec<(&str, &str, Vec<&str>, bool)> = vec![
        ("to-dual-stack", "DUAL_STACK", vec!["IPv4", "IPv6"], true),
        ("dual-stack-again", "DUAL_STACK", vec!["IPv4", "IPv6"], true),
        ("back-to-ipv4", "IPV4", vec!["IPv4"], false),
        ("ipv4-again", "IPV4", vec!["IPv4"], false),
    ];

    for (case, requested, expected_families, expects_dual_stack_name) in steps {
        let updated = ok(
            &router,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "IpAddressType": requested }),
        )
        .await["Accelerator"]
            .clone();
        let families: Vec<String> = updated["IpSets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|set| set["IpAddressFamily"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            families, expected_families,
            "case {case}: expected the families {expected_families:?}, got {updated}"
        );
        assert_eq!(
            updated["DualStackDnsName"].is_null(),
            !expects_dual_stack_name,
            "case {case}: DualStackDnsName presence is wrong, got {updated}"
        );
        assert_eq!(
            updated["DnsName"].as_str().unwrap_or_default(),
            format!("{label}.awsglobalaccelerator.com"),
            "case {case}: the accelerator's DNS label must not change, got {updated}"
        );

        // The change is what a later caller sees.
        let reread = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            reread["IpSets"], updated["IpSets"],
            "case {case}: expected the update to persist, got {reread}"
        );
    }
}

/// RULE: a create past the resource's documented quota is refused with the modelled
/// `LimitExceededException`, and deleting a resource frees a slot again — so the store a
/// caller can grow is bounded.
#[tokio::test]
async fn a_create_past_the_quota_is_refused_and_a_delete_frees_a_slot() {
    // Accelerators per account: 20; listeners per accelerator: 10 (AWS Global
    // Accelerator service quotas). Both bounds are exercised at the limit and one past.
    const ACCELERATOR_QUOTA: usize = 20;
    const LISTENER_QUOTA: usize = 10;

    let router = emulator();
    let mut arns = Vec::new();
    for index in 0..ACCELERATOR_QUOTA {
        let accelerator =
            create_accelerator(&router, &format!("acc-{index}"), &format!("tok-{index}")).await;
        arns.push(arn_of(&accelerator));
    }

    let (status, body) = call(
        &router,
        "CreateAccelerator",
        json!({ "Name": "one-too-many", "IdempotencyToken": "tok-over" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "LimitExceededException"),
        "case accelerator past the quota: got {status} {body}"
    );

    // Deleting one frees a slot, and the freed slot is usable.
    let doomed = arns.pop().expect("an accelerator to delete");
    ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": doomed, "Enabled": false }),
    )
    .await;
    ok(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": doomed }),
    )
    .await;
    let replacement = create_accelerator(&router, "replacement", "tok-replacement").await;
    arns.push(arn_of(&replacement));

    // The same rule, one level down, on a listener's parent.
    let parent = arns[0].clone();
    for index in 0..LISTENER_QUOTA {
        ok(
            &router,
            "CreateListener",
            json!({
                "AcceleratorArn": parent,
                "PortRanges": [{ "FromPort": 1000 + index, "ToPort": 1000 + index }],
                "Protocol": "TCP",
                "IdempotencyToken": format!("listener-{index}"),
            }),
        )
        .await;
    }
    let (status, body) = call(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": parent,
            "PortRanges": [{ "FromPort": 2000, "ToPort": 2000 }],
            "Protocol": "TCP",
            "IdempotencyToken": "listener-over",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "LimitExceededException"),
        "case listener past the quota: got {status} {body}"
    );

    // A second accelerator has its own listener budget.
    ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": arns[1],
            "PortRanges": [{ "FromPort": 2000, "ToPort": 2000 }],
            "Protocol": "TCP",
            "IdempotencyToken": "listener-other-parent",
        }),
    )
    .await;
}

/// RULE: a listener belongs to exactly one accelerator, and `ListListeners` reports only
/// that accelerator's listeners.
#[tokio::test]
async fn listeners_are_reported_only_under_their_own_accelerator() {
    let router = emulator();
    let first = arn_of(&create_accelerator(&router, "first", "first-token").await);
    let second = arn_of(&create_accelerator(&router, "second", "second-token").await);

    // Two listeners on the first accelerator, one on the second.
    let mut expected_first = Vec::new();
    for (index, port) in [80, 443].iter().enumerate() {
        let listener = ok(
            &router,
            "CreateListener",
            json!({
                "AcceleratorArn": first,
                "PortRanges": [{ "FromPort": port, "ToPort": port }],
                "Protocol": if index == 0 { "TCP" } else { "UDP" },
                "IdempotencyToken": format!("first-listener-{index}"),
            }),
        )
        .await["Listener"]
            .clone();
        expected_first.push(listener);
    }
    let expected_second = ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": second,
            "PortRanges": [{ "FromPort": 53, "ToPort": 53 }],
            "Protocol": "UDP",
            "ClientAffinity": "SOURCE_IP",
            "IdempotencyToken": "second-listener-0",
        }),
    )
    .await["Listener"]
        .clone();

    for (case, accelerator_arn, expected) in [
        ("first", &first, Value::Array(expected_first)),
        ("second", &second, json!([expected_second])),
    ] {
        let listeners = ok(
            &router,
            "ListListeners",
            json!({ "AcceleratorArn": accelerator_arn }),
        )
        .await["Listeners"]
            .clone();
        assert_eq!(
            listeners, expected,
            "case {case}: expected only this accelerator's listeners {expected}, got {listeners}"
        );
    }
}
