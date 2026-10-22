// SPDX-License-Identifier: CC0-1.0

//! The account-wide Bring Your Own License (BYOL) configuration, end to end over the
//! awsJson1_1 wire.
//!
//! The rule under test, stated about the configuration this emulator persists rather
//! than about any one recorded request:
//!
//! * `DescribeAccount` reports the BYOL configuration this service has persisted — the
//!   `DedicatedTenancySupportResultEnum` value the account currently has (`DISABLED`
//!   until a `ModifyAccount` call turns BYOL on) and the management network CIDR range
//!   stored for it (absent until one is configured) — and reporting changes nothing a
//!   later caller sees.
//! * `ModifyAccount` applies each member the request names and leaves every member it
//!   omits alone; a request that names no member, or that would turn BYOL on with no
//!   management network CIDR range anywhere, is refused with HTTP 400 and changes
//!   nothing.
//!
//! The suite reuses `workspace_lifecycle`'s harness (`emulator_with_engine` + `call`),
//! so every assertion is about what a client would see: the request is serialized,
//! routed, validated, handled and serialized back.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::service_state;

use crate::workspace_lifecycle::{call, emulator_with_engine};

/// The `DedicatedTenancySupportResultEnum` value `DescribeAccount` reports, the
/// management CIDR range it reports, and its `Message` — the three members a case
/// compares.
fn described_account(body: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        body["DedicatedTenancySupport"].as_str(),
        body["DedicatedTenancyManagementCidrRange"].as_str(),
        body["Message"].as_str(),
    )
}

#[tokio::test]
async fn an_unmodified_account_reports_byol_disabled_and_no_cidr_range() {
    let (router, _engine) = emulator_with_engine();

    let (status, body) = call(&router, "DescribeAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "case unmodified account: DescribeAccount must answer 200, body {body:?}"
    );
    let (support, range, message) = described_account(&body);
    assert_eq!(
        support,
        Some("DISABLED"),
        "case unmodified account: expected DedicatedTenancySupport DISABLED, got {body:?}"
    );
    assert_eq!(
        range, None,
        "case unmodified account: expected no DedicatedTenancyManagementCidrRange, got {body:?}"
    );
    assert!(
        message.is_some_and(|text| !text.is_empty()),
        "case unmodified account: expected a Message describing the BYOL status, got {body:?}"
    );
    // "The type of linked account": nothing in this emulator models an account link, so
    // the member stays absent rather than carrying an invented value.
    assert!(
        body.get("DedicatedTenancyAccountType").is_none(),
        "case unmodified account: expected no DedicatedTenancyAccountType, got {body:?}"
    );

    // A read must not change what a later caller sees.
    let (second_status, second_body) = call(&router, "DescribeAccount", &json!({})).await;
    assert_eq!(
        second_status, 200,
        "case repeated read: DescribeAccount must answer 200, body {second_body:?}"
    );
    assert_eq!(
        described_account(&second_body),
        described_account(&body),
        "case repeated read: expected {body:?}, got {second_body:?}"
    );
}

/// One row: the `ModifyAccount` requests to apply in order, then the
/// `DedicatedTenancySupport` and `DedicatedTenancyManagementCidrRange`
/// `DescribeAccount` must report afterwards.
struct ModificationCase {
    requests: Vec<Value>,
    expected_support: &'static str,
    expected_range: Option<&'static str>,
}

/// The modification table.
///
/// The rows differ along what the rule generalises over: which members the request
/// names, how many requests are applied, and the value of the CIDR range. The ranges
/// come from the model's `DedicatedTenancyManagementCidrRange` pattern
/// (`smithy_query(query="operation",operation="ModifyAccount")`), which admits exactly
/// `a.b.0.0/16` with each of `a` and `b` in 0..=255 — so the table carries both
/// published bounds (`0.0.0.0/16`, `255.255.0.0/16`) as well as ordinary private
/// ranges, and the refusal table below carries one value past each bound.
fn modification_table() -> Vec<ModificationCase> {
    vec![
        // A CIDR range on its own is stored without turning BYOL on.
        ModificationCase {
            requests: vec![json!({ "DedicatedTenancyManagementCidrRange": "10.0.0.0/16" })],
            expected_support: "DISABLED",
            expected_range: Some("10.0.0.0/16"),
        },
        // Both members at once.
        ModificationCase {
            requests: vec![json!({
                "DedicatedTenancySupport": "ENABLED",
                "DedicatedTenancyManagementCidrRange": "172.16.0.0/16",
            })],
            expected_support: "ENABLED",
            expected_range: Some("172.16.0.0/16"),
        },
        // Two calls: the range first, then support on its own — which is allowed
        // precisely because a range is already stored.
        ModificationCase {
            requests: vec![
                json!({ "DedicatedTenancyManagementCidrRange": "192.168.0.0/16" }),
                json!({ "DedicatedTenancySupport": "ENABLED" }),
            ],
            expected_support: "ENABLED",
            expected_range: Some("192.168.0.0/16"),
        },
        // A later call replaces the range and leaves the support value alone.
        ModificationCase {
            requests: vec![
                json!({
                    "DedicatedTenancySupport": "ENABLED",
                    "DedicatedTenancyManagementCidrRange": "10.0.0.0/16",
                }),
                json!({ "DedicatedTenancyManagementCidrRange": "203.0.0.0/16" }),
            ],
            expected_support: "ENABLED",
            expected_range: Some("203.0.0.0/16"),
        },
        // Enabling twice is the same configuration, not a different one.
        ModificationCase {
            requests: vec![
                json!({
                    "DedicatedTenancySupport": "ENABLED",
                    "DedicatedTenancyManagementCidrRange": "100.64.0.0/16",
                }),
                json!({ "DedicatedTenancySupport": "ENABLED" }),
            ],
            expected_support: "ENABLED",
            expected_range: Some("100.64.0.0/16"),
        },
        // The low bound of both octets the pattern constrains.
        ModificationCase {
            requests: vec![json!({
                "DedicatedTenancySupport": "ENABLED",
                "DedicatedTenancyManagementCidrRange": "0.0.0.0/16",
            })],
            expected_support: "ENABLED",
            expected_range: Some("0.0.0.0/16"),
        },
        // The high bound of both octets the pattern constrains.
        ModificationCase {
            requests: vec![json!({
                "DedicatedTenancySupport": "ENABLED",
                "DedicatedTenancyManagementCidrRange": "255.255.0.0/16",
            })],
            expected_support: "ENABLED",
            expected_range: Some("255.255.0.0/16"),
        },
    ]
}

#[tokio::test]
async fn modify_account_persists_the_configuration_describe_account_reports() {
    for case in modification_table() {
        // A fresh emulator per row, so each row is about its own requests alone.
        let (router, _engine) = emulator_with_engine();
        let label = format!("{:?}", case.requests);

        for request in &case.requests {
            let (status, body) = call(&router, "ModifyAccount", request).await;
            assert_eq!(
                status, 200,
                "case {label}: ModifyAccount {request:?} must answer 200, body {body:?}"
            );
            assert!(
                body["Message"].as_str().is_some_and(|text| !text.is_empty()),
                "case {label}: ModifyAccount {request:?} must report a Message, body {body:?}"
            );
        }

        let (status, body) = call(&router, "DescribeAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribeAccount must answer 200, body {body:?}"
        );
        let (support, range, _message) = described_account(&body);
        assert_eq!(
            support,
            Some(case.expected_support),
            "case {label}: expected DedicatedTenancySupport {}, got {body:?}",
            case.expected_support
        );
        assert_eq!(
            range, case.expected_range,
            "case {label}: expected DedicatedTenancyManagementCidrRange {:?}, got {body:?}",
            case.expected_range
        );
    }
}

/// One refusal row: the request, why it cannot be applied, and the error code the
/// refusal must carry.
struct RefusalCase {
    request: Value,
    why: &'static str,
    expected_code: &'static str,
}

/// The refusal table: requests that name no applicable modification, or a value the
/// model does not admit. Both are refused with HTTP 400 — the handler's own modelled
/// `InvalidParameterValuesException` for a combination it cannot apply, boundary
/// `ValidationException` for a value the member's own constraint rejects — and neither
/// may change the stored configuration.
fn refusal_table() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            request: json!({}),
            why: "names neither member, so it asks for no modification",
            expected_code: "InvalidParameterValuesException",
        },
        RefusalCase {
            request: json!({ "DedicatedTenancySupport": "ENABLED" }),
            why: "turns BYOL on with no management CIDR range stored or supplied",
            expected_code: "InvalidParameterValuesException",
        },
        RefusalCase {
            // `DedicatedTenancySupportEnum` has the single value ENABLED: BYOL cannot be
            // turned off through this operation.
            request: json!({ "DedicatedTenancySupport": "DISABLED" }),
            why: "DISABLED is not a value the input enum admits",
            expected_code: "ValidationException",
        },
        RefusalCase {
            request: json!({ "DedicatedTenancyManagementCidrRange": "10.0.0.0/24" }),
            why: "the CIDR block size must be /16",
            expected_code: "ValidationException",
        },
        RefusalCase {
            request: json!({ "DedicatedTenancyManagementCidrRange": "10.0.1.0/16" }),
            why: "the pattern admits only a range whose last two octets are 0",
            expected_code: "ValidationException",
        },
        RefusalCase {
            // One past the high bound of the first octet the pattern constrains.
            request: json!({ "DedicatedTenancyManagementCidrRange": "256.0.0.0/16" }),
            why: "256 is past the octet range the pattern admits",
            expected_code: "ValidationException",
        },
        RefusalCase {
            // One past the high bound of the second octet.
            request: json!({ "DedicatedTenancyManagementCidrRange": "10.256.0.0/16" }),
            why: "256 is past the octet range the pattern admits",
            expected_code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn requests_that_cannot_be_applied_are_refused_and_change_nothing() {
    for case in refusal_table() {
        let (router, _engine) = emulator_with_engine();
        let RefusalCase {
            request,
            why,
            expected_code,
        } = case;

        let (status, body) = call(&router, "ModifyAccount", &request).await;
        assert_eq!(
            status, 400,
            "case {request:?} ({why}): expected HTTP 400, got {status} with body {body:?}"
        );
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            code.ends_with(expected_code),
            "case {request:?} ({why}): expected {expected_code}, got {body:?}"
        );

        let (status, body) = call(&router, "DescribeAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {request:?} ({why}): DescribeAccount must answer 200, body {body:?}"
        );
        let (support, range, _message) = described_account(&body);
        assert_eq!(
            support,
            Some("DISABLED"),
            "case {request:?} ({why}): a refused request must leave BYOL DISABLED, got {body:?}"
        );
        assert_eq!(
            range, None,
            "case {request:?} ({why}): a refused request must store no CIDR range, got {body:?}"
        );
    }
}

#[tokio::test]
async fn the_configuration_is_account_wide_and_survives_between_requests() {
    let (router, engine) = emulator_with_engine();

    let (status, body) = call(
        &router,
        "ModifyAccount",
        &json!({
            "DedicatedTenancySupport": "ENABLED",
            "DedicatedTenancyManagementCidrRange": "198.18.0.0/16",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case shared store: ModifyAccount must answer 200, body {body:?}"
    );

    // The store the handlers write is the one the shared engine holds, not a per-request
    // copy: the same values are visible through the state API.
    let state = service_state(&*engine).expect("the WorkSpaces state store");
    let stored = state.lock().byol().clone();
    assert!(
        stored.is_enabled(),
        "case shared store: expected BYOL enabled in the shared store, got {stored:?}"
    );
    assert_eq!(
        stored.management_cidr_range(),
        Some("198.18.0.0/16"),
        "case shared store: expected the stored CIDR range, got {stored:?}"
    );

    // And a later request over the wire reports exactly what was stored.
    let (status, body) = call(&router, "DescribeAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "case shared store: DescribeAccount must answer 200, body {body:?}"
    );
    assert_eq!(
        described_account(&body).0,
        Some("ENABLED"),
        "case shared store: expected DedicatedTenancySupport ENABLED, got {body:?}"
    );
    assert_eq!(
        described_account(&body).1,
        Some("198.18.0.0/16"),
        "case shared store: expected the stored CIDR range, got {body:?}"
    );
}
