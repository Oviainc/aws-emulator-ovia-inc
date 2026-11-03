// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the custom routing accelerator slice, driven over the wire.
//!
//! Every call goes through the real router, using the harness in
//! [`super::accelerator_lifecycle`] (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so these
//! exercise routing, deserialization, constraint validation, the handler and the response
//! envelope — not just the handler function.
//!
//! The rules under test are stated once each, as a table or an invariant over a SET of
//! inputs; a recorded request is one row.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{arn_of, call, create_accelerator, emulator, error_code, ok};
// The envelope's message, in either spelling, already has one reader in this crate.
use super::input_refusals::error_message;

/// `CreateCustomRoutingAccelerator` with the members a row names, plus the required ones.
async fn create_custom_routing(
    router: &MultiServiceRouter,
    name: &str,
    token: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({ "Name": name, "IdempotencyToken": token });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    call(router, "CreateCustomRoutingAccelerator", body).await
}

/// The created custom routing accelerator, requiring a 200.
async fn created_custom_routing(
    router: &MultiServiceRouter,
    name: &str,
    token: &str,
    extra: Value,
) -> Value {
    let (status, body) = create_custom_routing(router, name, token, extra).await;
    assert_eq!(
        status, 200,
        "case {name}: expected HTTP 200 from CreateCustomRoutingAccelerator, got {status} \
         with {body}"
    );
    body["Accelerator"].clone()
}

/// `DescribeCustomRoutingAccelerator` for one ARN.
async fn describe_custom_routing(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeCustomRoutingAccelerator",
        json!({ "AcceleratorArn": arn }),
    )
    .await
}

/// `a` + 16 lowercase hex digits, then `.awsglobalaccelerator.com`: the naming convention
/// `CustomRoutingAccelerator$DnsName` documents.
fn assert_dns_name(case: &str, host: &str) {
    let label = host
        .strip_suffix(".awsglobalaccelerator.com")
        .unwrap_or_else(|| {
            panic!("case {case}: expected a host under .awsglobalaccelerator.com, got {host:?}")
        });
    let digits = label.strip_prefix('a').unwrap_or_else(|| {
        panic!("case {case}: expected a label starting with 'a', got {label:?}")
    });
    assert_eq!(
        digits.len(),
        16,
        "case {case}: expected a 16-digit hex label, got {digits:?}"
    );
    assert!(
        digits
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case}: expected lowercase hex in the label, got {digits:?}"
    );
}

/// The addresses of an accelerator's single IPv4 address set.
fn ipv4_addresses(case: &str, accelerator: &Value) -> Vec<String> {
    let sets = accelerator["IpSets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: IpSets missing from {accelerator}"));
    assert_eq!(
        sets.len(),
        1,
        "case {case}: a custom routing accelerator is IPv4-only, so it has one IP address \
         set, got {accelerator}"
    );
    assert_eq!(
        sets[0]["IpAddressFamily"], "IPv4",
        "case {case}: expected the IPv4 family, got {accelerator}"
    );
    let addresses: Vec<String> = sets[0]["IpAddresses"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: IpAddresses missing from {accelerator}"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        addresses.len(),
        2,
        "case {case}: an accelerator has two static IPv4 addresses, got {addresses:?}"
    );
    let details = sets[0]["IpAddressDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: IpAddressDetails missing from {accelerator}"));
    assert_eq!(
        details.len(),
        addresses.len(),
        "case {case}: expected one detail entry per address, got {accelerator}"
    );
    for (index, detail) in details.iter().enumerate() {
        assert_eq!(
            detail["IpAddress"], addresses[index],
            "case {case}: detail {index} must name the address in the same position, \
             got {accelerator}"
        );
        assert!(
            detail["NetworkZone"].as_str().is_some_and(|zone| !zone.is_empty()),
            "case {case}: detail {index} must name a network zone, got {accelerator}"
        );
    }
    addresses
}

/// RULE: a create mints its resource from the service's own state — a fresh ARN, the
/// documented DNS name, the static addresses it allocates, the members the request
/// supplied and the defaults it did not — persists it, and the matching describe reports
/// exactly what was persisted.
#[tokio::test]
async fn a_custom_routing_accelerator_is_minted_from_state_and_reads_back() {
    let router = emulator();

    // The rows differ along what the rule generalises over: which optional members the
    // request carries (none, `Enabled`, `IpAddressType`, `Tags`), how many BYOIP
    // addresses lead the IPv4 set (none, one, two) and the form of the name (short,
    // hyphenated, at the documented 64-character maximum).
    let cases: Vec<(&str, String, Value, bool, Vec<&str>)> = vec![
        ("defaults", "cra-defaults".to_string(), json!({}), true, vec![]),
        (
            "disabled",
            "cra-disabled".to_string(),
            json!({ "Enabled": false }),
            false,
            vec![],
        ),
        (
            "explicit-ipv4",
            "cra-ipv4".to_string(),
            json!({ "IpAddressType": "IPV4", "Enabled": true }),
            true,
            vec![],
        ),
        (
            "tagged",
            "cra-tagged".to_string(),
            json!({ "Tags": [{ "Key": "team", "Value": "net" }] }),
            true,
            vec![],
        ),
        (
            "one-byoip",
            "cra-byoip-1".to_string(),
            json!({ "IpAddresses": ["198.51.100.7"] }),
            true,
            vec!["198.51.100.7"],
        ),
        (
            "two-byoip",
            "cra-byoip-2".to_string(),
            json!({ "IpAddresses": ["198.51.100.8", "203.0.113.9"] }),
            true,
            vec!["198.51.100.8", "203.0.113.9"],
        ),
        ("name-at-max", "c".repeat(64), json!({}), true, vec![]),
    ];

    let mut seen_arns: Vec<String> = Vec::new();
    let mut seen_addresses: Vec<String> = Vec::new();
    for (case, name, extra, expected_enabled, expected_leading) in cases {
        let created =
            created_custom_routing(&router, &name, &format!("{case}-token"), extra).await;
        let arn = arn_of(&created);

        assert!(
            arn.starts_with("arn:aws:globalaccelerator::") && arn.contains(":accelerator/"),
            "case {case}: expected an accelerator ARN, got {arn:?}"
        );
        assert!(
            !seen_arns.contains(&arn),
            "case {case}: every create mints a distinct ARN, got {arn:?}"
        );
        seen_arns.push(arn.clone());

        assert_eq!(
            created["Name"].as_str().unwrap_or_default(),
            name.as_str(),
            "case {case}: expected the requested Name, got {created}"
        );
        assert_eq!(
            created["IpAddressType"], "IPV4",
            "case {case}: a custom routing accelerator supports only IPV4, got {created}"
        );
        assert_eq!(
            created["Enabled"],
            Value::Bool(expected_enabled),
            "case {case}: expected Enabled {expected_enabled}, got {created}"
        );
        // Deterministic, not timing-dependent: the create response's own `Status` is
        // computed against the same instant that set the deployment's due instant.
        assert_eq!(
            created["Status"], "IN_PROGRESS",
            "case {case}: a new accelerator deploys before it is DEPLOYED, got {created}"
        );
        assert!(
            created["CreatedTime"].is_number() && created["LastModifiedTime"].is_number(),
            "case {case}: expected both timestamps, got {created}"
        );
        assert!(
            created.get("DualStackDnsName").is_none(),
            "case {case}: `CustomRoutingAccelerator` models no DualStackDnsName, got {created}"
        );
        assert_dns_name(case, created["DnsName"].as_str().unwrap_or_default());

        let addresses = ipv4_addresses(case, &created);
        for (index, expected) in expected_leading.iter().enumerate() {
            assert_eq!(
                addresses[index], *expected,
                "case {case}: a supplied address leads the IPv4 set, got {addresses:?}"
            );
        }
        for address in &addresses {
            if expected_leading.contains(&address.as_str()) {
                continue;
            }
            assert!(
                !seen_addresses.contains(address),
                "case {case}: an allocated static address is not handed out twice, \
                 got {address:?}"
            );
            seen_addresses.push(address.clone());
        }

        // The round trip: the describe reports the persisted record, not the request.
        let (status, body) = describe_custom_routing(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the created accelerator to read back, got {status} {body}"
        );
        let read_back = body["Accelerator"].clone();
        for member in [
            "AcceleratorArn",
            "Name",
            "IpAddressType",
            "Enabled",
            "IpSets",
            "DnsName",
            "CreatedTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                read_back[member], created[member],
                "case {case}: {member} must read back as persisted, got {read_back}"
            );
        }
        let status_read = read_back["Status"].as_str().unwrap_or_default();
        assert!(
            status_read == "IN_PROGRESS" || status_read == "DEPLOYED",
            "case {case}: expected a CustomRoutingAcceleratorStatus, got {status_read:?}"
        );
    }
}

/// RULE: an `IdempotencyToken` identifies one create request within its own operation —
/// repeating that request returns the resource the token already minted, a different
/// token mints another resource, and the same token sent to a different create does not
/// replay this one.
#[tokio::test]
async fn an_idempotency_token_replays_only_its_own_create() {
    let router = emulator();

    // The rows differ in the members the repeated request carries, so the replay is
    // tested as a property of the token rather than of one body.
    let cases: Vec<(&str, Value)> = vec![
        ("plain", json!({})),
        ("disabled", json!({ "Enabled": false })),
        ("byoip", json!({ "IpAddresses": ["192.0.2.10"] })),
    ];

    for (case, extra) in cases {
        let token = format!("replay-{case}");
        let first =
            created_custom_routing(&router, &format!("cra-{case}"), &token, extra.clone()).await;
        let again =
            created_custom_routing(&router, &format!("cra-{case}"), &token, extra.clone()).await;
        assert_eq!(
            arn_of(&again),
            arn_of(&first),
            "case {case}: the same token must replay the accelerator it minted, got {again}"
        );
        assert_eq!(
            again["IpSets"], first["IpSets"],
            "case {case}: a replay returns the persisted record, got {again}"
        );

        let fresh = created_custom_routing(
            &router,
            &format!("cra-{case}"),
            &format!("{token}-other"),
            extra,
        )
        .await;
        assert_ne!(
            arn_of(&fresh),
            arn_of(&first),
            "case {case}: a different token mints a different accelerator, got {fresh}"
        );
    }

    // One token, two different creates: each kind of accelerator is its own resource, so
    // neither replays the other's.
    let shared = "shared-token";
    let standard = create_accelerator(&router, "shared-standard", shared).await;
    let custom = created_custom_routing(&router, "shared-custom", shared, json!({})).await;
    assert_ne!(
        arn_of(&custom),
        arn_of(&standard),
        "case cross-kind token: a custom routing create must not replay the standard \
         accelerator, got {custom}"
    );
}

/// RULE: a name is refused unless it fits the rule the model documents for that kind of
/// accelerator — at most 64 characters, alphanumerics plus the punctuation that kind
/// admits, and never that punctuation at either end.
#[tokio::test]
async fn a_name_outside_the_documented_character_set_is_refused() {
    let router = emulator();

    // The catalog is `CreateCustomRoutingAccelerator$Name`: "The name can have a maximum
    // of 64 characters, must contain only alphanumeric characters or hyphens (-), and
    // must not begin or end with a hyphen." The rows walk each clause, the 64-character
    // bound and one past it.
    let cases: Vec<(&str, String, bool)> = vec![
        ("alphanumeric", "cra1".to_string(), true),
        ("leading-digit", "9cra".to_string(), true),
        ("inner-hyphens", "cra-one-two".to_string(), true),
        ("single-character", "c".to_string(), true),
        ("at-the-maximum", "n".repeat(64), true),
        ("one-past-the-maximum", "n".repeat(65), false),
        ("leading-hyphen", "-cra".to_string(), false),
        ("trailing-hyphen", "cra-".to_string(), false),
        ("underscore", "cra_one".to_string(), false),
        ("space", "cra one".to_string(), false),
        // A period is in the STANDARD accelerator's set and not in this one.
        ("period", "cra.one".to_string(), false),
    ];

    for (case, name, accepted) in cases {
        let (status, body) =
            create_custom_routing(&router, &name, &format!("name-{case}"), json!({})).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected the name {name:?} to be accepted, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidArgumentException"),
            "case {case}: expected the name {name:?} to be refused, got {status} {body}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with("Name ")
                && message.contains("alphanumeric characters or hyphens (-)")
                && message.contains("maximum of 64 characters"),
            "case {case}: expected the refusal to state the name rule, got {message:?}"
        );
        assert!(
            !message.contains("periods (.)"),
            "case {case}: a custom routing accelerator's name admits no period, \
             got {message:?}"
        );
    }

    // The same name a period made illegal here is legal for a standard accelerator,
    // whose documented set includes periods: the two rules are read from their own docs.
    ok(
        &router,
        "CreateAccelerator",
        json!({ "Name": "cra.one", "IdempotencyToken": "standard-period" }),
    )
    .await;
}

/// RULE: a create stores only an `IpAddressType` the resource supports — for a custom
/// routing accelerator IPV4, which is also what an absent member takes — and refuses any
/// other with `InvalidArgumentException`.
#[tokio::test]
async fn the_ip_address_type_of_a_custom_routing_accelerator_must_be_ipv4() {
    let router = emulator();

    // `IpAddressType` is the model enum of that name: DUAL_STACK | IPV4. Every value it
    // offers is a row, plus the absent member.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("absent", json!({}), true),
        ("ipv4", json!({ "IpAddressType": "IPV4" }), true),
        ("dual-stack", json!({ "IpAddressType": "DUAL_STACK" }), false),
    ];

    for (case, extra, accepted) in cases {
        let (status, body) =
            create_custom_routing(&router, &format!("cra-{case}"), &format!("type-{case}"), extra)
                .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected the type to be accepted, got {status} {body}"
            );
            assert_eq!(
                body["Accelerator"]["IpAddressType"], "IPV4",
                "case {case}: expected IPV4 to be persisted, got {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidArgumentException"),
            "case {case}: expected the type to be refused, got {status} {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("IpAddressType DUAL_STACK") && message.contains("must be IPV4"),
            "case {case}: expected the refusal to state the rule, got {message:?}"
        );
    }

    // The same value is legal for a STANDARD accelerator, which supports both types.
    let dual_stack = ok(
        &router,
        "CreateAccelerator",
        json!({
            "Name": "standard-dual-stack",
            "IdempotencyToken": "standard-dual-stack",
            "IpAddressType": "DUAL_STACK",
        }),
    )
    .await["Accelerator"]
        .clone();
    assert_eq!(
        dual_stack["IpAddressType"], "DUAL_STACK",
        "case standard dual-stack: expected the requested type, got {dual_stack}"
    );
}

/// RULE: the two kinds of accelerator are distinct resources — each is reported only by
/// the operations of its own kind, and an ARN of the other kind is as absent as one that
/// was never created.
#[tokio::test]
async fn each_accelerator_kind_is_reported_only_by_its_own_operations() {
    let router = emulator();

    let standard: Vec<String> = {
        let mut arns = Vec::new();
        for index in 0..2 {
            let accelerator =
                create_accelerator(&router, &format!("std-{index}"), &format!("std-{index}")).await;
            arns.push(arn_of(&accelerator));
        }
        arns
    };
    let mut custom = Vec::new();
    for index in 0..2 {
        let accelerator = created_custom_routing(
            &router,
            &format!("cra-{index}"),
            &format!("cra-{index}"),
            json!({}),
        )
        .await;
        custom.push(arn_of(&accelerator));
    }

    // Every cross-kind read is a row, plus an ARN that names nothing at all.
    let unknown = format!("{}-gone", standard[0]);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("custom via DescribeAccelerator", "DescribeAccelerator", &custom[0]),
        ("custom via DescribeAccelerator, second", "DescribeAccelerator", &custom[1]),
        (
            "standard via DescribeCustomRoutingAccelerator",
            "DescribeCustomRoutingAccelerator",
            &standard[0],
        ),
        (
            "unknown via DescribeCustomRoutingAccelerator",
            "DescribeCustomRoutingAccelerator",
            &unknown,
        ),
    ];
    for (case, operation, arn) in cases {
        let (status, body) = call(&router, operation, json!({ "AcceleratorArn": arn })).await;
        // The status this service answers a missing resource with is pinned, for every
        // operation that can answer one, by [`super::not_found_error_status`].
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AcceleratorNotFoundException"),
            "case {case}: expected the ARN to be absent, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Accelerator {arn} not found."),
            "case {case}: expected the refusal to name the ARN the request carried, got {body}"
        );
    }

    // And each kind reads back through its own describe.
    for arn in &custom {
        let (status, body) = describe_custom_routing(&router, arn).await;
        assert_eq!(
            (status, body["Accelerator"]["AcceleratorArn"].as_str().unwrap_or_default()),
            (200, arn.as_str()),
            "case custom describe: expected {arn} to read back, got {body}"
        );
    }

    // `ListAccelerators` lists standard accelerators; a custom routing one is not one.
    let listed = ok(&router, "ListAccelerators", json!({})).await;
    let mut listed_arns: Vec<String> = listed["Accelerators"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["AcceleratorArn"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_arns.sort();
    let mut expected_arns = standard.clone();
    expected_arns.sort();
    assert_eq!(
        listed_arns, expected_arns,
        "case list: expected only the standard accelerators, got {listed}"
    );
}

/// RULE: the deployment a create starts is owed by the clock — the accelerator reads
/// `IN_PROGRESS` straight after the call and `DEPLOYED` once the due instant has passed —
/// and reading it changes nothing a later caller sees.
#[tokio::test]
async fn a_custom_routing_deployment_settles_on_the_clock() {
    let router = emulator();
    let arn = arn_of(&created_custom_routing(&router, "cra-settles", "settles", json!({})).await);

    async fn status_now(router: &MultiServiceRouter, arn: &str) -> String {
        let (status, body) = describe_custom_routing(router, arn).await;
        assert_eq!(status, 200, "case read: expected a 200, got {status} {body}");
        body["Accelerator"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    // A read before the due instant must not settle it early. The settle span is at
    // least 250 ms, so a read that completes inside that window still reports
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
    assert_eq!(
        status_now(&router, &arn).await,
        "DEPLOYED",
        "case second read: a read must not move the deployment on"
    );
}

/// RULE: a create past the documented per-account quota is refused with the modelled
/// `LimitExceededException`, and the quota is counted per accelerator kind — a standard
/// accelerator and a custom routing one are distinct resources created by distinct
/// operations.
#[tokio::test]
async fn a_custom_routing_create_past_the_account_quota_is_refused() {
    // Accelerators per account: 20 (AWS Global Accelerator service quotas).
    const ACCELERATOR_QUOTA: usize = 20;

    let router = emulator();
    for index in 0..ACCELERATOR_QUOTA {
        created_custom_routing(
            &router,
            &format!("cra-{index}"),
            &format!("cra-{index}"),
            json!({}),
        )
        .await;
    }

    let (status, body) =
        create_custom_routing(&router, "one-too-many", "over-quota", json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "LimitExceededException"),
        "case custom routing accelerator past the quota: got {status} {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("the account already holds the maximum of {ACCELERATOR_QUOTA} accelerators"),
        "case custom routing accelerator past the quota: expected the quota to be stated, \
         got {body}"
    );

    // The standard create's own quota is untouched by this kind's resources.
    let standard = create_accelerator(&router, "std-after-quota", "std-after-quota").await;
    assert!(
        !arn_of(&standard).is_empty(),
        "case standard create after the custom routing quota is full: got {standard}"
    );
}
