// SPDX-License-Identifier: CC0-1.0

//! The registry scanning configuration, end to end over the wire.
//!
//! The rule under test: the registry reports exactly the scanning configuration the last
//! `PutRegistryScanningConfiguration` stored — the put REPLACES it, defaulting `scanType`
//! to `BASIC` and `rules` to none — and reports the documented default (`BASIC` scanning
//! with no rules) while no put has stored one; a put whose rule asks for a scan frequency
//! its scan type does not support is refused and stores nothing.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use serde_json::{json, Value};

// The wire harness lives in `tests/all/wire.rs`, shared with every other suite.
use crate::wire::{call, emulator, ok, REGISTRY_ID};

/// The values `ScanType` accepts, from the model's `ScanType` enum.
const SCAN_TYPES: [&str; 2] = ["BASIC", "ENHANCED"];

/// The values `ScanFrequency` accepts, from the model's `ScanFrequency` enum.
const SCAN_FREQUENCIES: [&str; 3] = ["CONTINUOUS_SCAN", "MANUAL", "SCAN_ON_PUSH"];

/// Whether a scan type supports a scan frequency, from `RegistryScanningRule$scanFrequency`:
/// "When the `ENHANCED` scan type is specified, the supported scan frequencies are
/// `CONTINUOUS_SCAN` and `SCAN_ON_PUSH`. When the `BASIC` scan type is specified, the
/// `SCAN_ON_PUSH` scan frequency is supported. If scan on push is not specified, then the
/// `MANUAL` scan frequency is set by default." Spelled out independently of the
/// implementation's own table.
fn frequency_is_supported(scan_type: &str, frequency: &str) -> bool {
    matches!(
        (scan_type, frequency),
        ("ENHANCED", "CONTINUOUS_SCAN" | "SCAN_ON_PUSH") | ("BASIC", "MANUAL" | "SCAN_ON_PUSH")
    )
}

/// One scanning rule: a frequency and the repository filters it applies to. `WILDCARD` is
/// the only `ScanningRepositoryFilterType` the model declares.
fn rule(frequency: &str, filters: &[&str]) -> Value {
    json!({
        "scanFrequency": frequency,
        "repositoryFilters": filters
            .iter()
            .map(|filter| json!({ "filter": filter, "filterType": "WILDCARD" }))
            .collect::<Vec<Value>>(),
    })
}

/// A `RegistryScanningConfiguration` as the wire carries it.
fn configuration(scan_type: &str, rules: Vec<Value>) -> Value {
    json!({ "scanType": scan_type, "rules": rules })
}

/// The configuration a registry with nothing stored reports: `PutRegistryScanningConfiguration`'s
/// `scanType` documents "When a registry scanning configuration is not defined, by default
/// the BASIC scan type is used", and no rule has been defined either.
fn default_configuration() -> Value {
    configuration("BASIC", Vec::new())
}

/// What `GetRegistryScanningConfiguration` reports, asserting the registry id it carries.
async fn get_configuration(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> Value {
    let body = ok(router, case, "GetRegistryScanningConfiguration", &json!({})).await;
    assert_eq!(
        body["registryId"], json!(REGISTRY_ID),
        "case {case:?}: expected the registry id {REGISTRY_ID:?}, got {:?}",
        body["registryId"]
    );
    body["scanningConfiguration"].clone()
}

#[tokio::test]
async fn an_unconfigured_registry_reports_the_documented_default() {
    let router = emulator();

    // Twice: a read must not change what the next caller sees.
    for case in ["first read", "second read"] {
        let reported = get_configuration(&router, case).await;
        assert_eq!(
            reported,
            default_configuration(),
            "case {case:?}: expected the default scanning configuration {:?}, got {reported:?}",
            default_configuration()
        );
    }
}

#[tokio::test]
async fn a_put_replaces_the_configuration_and_every_get_reports_it() {
    let router = emulator();

    // The rows differ along what the rule generalises over: the scan type, whether the
    // request names one at all, the number of rules (none, one, and the modelled maximum
    // of two), the frequency each rule asks for, and the number and shape of its filters.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "basic scanning, one rule filtering one prefix",
            json!({ "scanType": "BASIC", "rules": [rule("SCAN_ON_PUSH", &["prod/*"])] }),
            configuration("BASIC", vec![rule("SCAN_ON_PUSH", &["prod/*"])]),
        ),
        (
            "basic scanning, manual frequency over every repository",
            json!({ "scanType": "BASIC", "rules": [rule("MANUAL", &["*"])] }),
            configuration("BASIC", vec![rule("MANUAL", &["*"])]),
        ),
        (
            "enhanced scanning, two rules and filters of several shapes",
            json!({
                "scanType": "ENHANCED",
                "rules": [
                    rule("CONTINUOUS_SCAN", &["*"]),
                    rule("SCAN_ON_PUSH", &["team-a/*", "lib.core_v2-rc", "x"]),
                ],
            }),
            configuration(
                "ENHANCED",
                vec![
                    rule("CONTINUOUS_SCAN", &["*"]),
                    rule("SCAN_ON_PUSH", &["team-a/*", "lib.core_v2-rc", "x"]),
                ],
            ),
        ),
        (
            "enhanced scanning with no rules at all",
            json!({ "scanType": "ENHANCED" }),
            configuration("ENHANCED", Vec::new()),
        ),
        (
            "enhanced scanning, one rule carrying an empty filter list",
            json!({ "scanType": "ENHANCED", "rules": [rule("CONTINUOUS_SCAN", &[])] }),
            configuration("ENHANCED", vec![rule("CONTINUOUS_SCAN", &[])]),
        ),
        (
            "no scanType: the registry stays on the default scan type",
            json!({ "rules": [rule("SCAN_ON_PUSH", &["*"])] }),
            configuration("BASIC", vec![rule("SCAN_ON_PUSH", &["*"])]),
        ),
        (
            "neither member: back to the default configuration",
            json!({}),
            default_configuration(),
        ),
    ];

    for (case, request, expected) in cases {
        let put = ok(
            &router,
            case,
            "PutRegistryScanningConfiguration",
            &request,
        )
        .await;
        assert_eq!(
            put["registryScanningConfiguration"], expected,
            "case {case:?}: the put reported {:?}, expected {expected:?}",
            put["registryScanningConfiguration"]
        );

        // The round trip: the reader reports what the put stored, and keeps reporting it.
        for read in ["read back", "read again"] {
            let reported = get_configuration(&router, case).await;
            assert_eq!(
                reported, expected,
                "case {case:?} ({read}): the registry reports {reported:?}, expected \
                 {expected:?}",
            );
        }
    }
}

#[tokio::test]
async fn a_frequency_its_scan_type_does_not_support_is_refused_and_stores_nothing() {
    let router = emulator();

    // A configuration to come back to: a refusal must leave it exactly as it was.
    let baseline = configuration("BASIC", vec![rule("SCAN_ON_PUSH", &["baseline/*"])]);
    ok(
        &router,
        "baseline",
        "PutRegistryScanningConfiguration",
        &json!({ "scanType": "BASIC", "rules": [rule("SCAN_ON_PUSH", &["baseline/*"])] }),
    )
    .await;

    // The whole cross product of the two modelled enums, so every pairing is exercised
    // and not only the one a recorded request happened to carry.
    for scan_type in SCAN_TYPES {
        for frequency in SCAN_FREQUENCIES {
            let case = format!("{scan_type} + {frequency}");
            let request =
                json!({ "scanType": scan_type, "rules": [rule(frequency, &["pairing/*"])] });
            let answer = call(&router, "PutRegistryScanningConfiguration", &request).await;

            if frequency_is_supported(scan_type, frequency) {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a supported pairing must be stored, got HTTP {} with {:?}",
                    answer.status, answer.body
                );
                let expected = configuration(scan_type, vec![rule(frequency, &["pairing/*"])]);
                let reported = get_configuration(&router, &case).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: the registry reports {reported:?}, expected {expected:?}",
                );

                // Back to the baseline for the next row's "stored nothing" check.
                ok(
                    &router,
                    &case,
                    "PutRegistryScanningConfiguration",
                    &json!({
                        "scanType": "BASIC",
                        "rules": [rule("SCAN_ON_PUSH", &["baseline/*"])],
                    }),
                )
                .await;
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: an unsupported pairing must be refused with HTTP 400, got \
                     {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {:?}",
                    answer.error_code()
                );
            }

            let reported = get_configuration(&router, &case).await;
            assert_eq!(
                reported,
                baseline.clone(),
                "case {case:?}: after the call the registry reports {reported:?}, expected \
                 {baseline:?}",
            );
        }
    }
}

#[tokio::test]
async fn a_filter_outside_the_modelled_constraint_is_refused_and_stores_nothing() {
    let router = emulator();

    let baseline = configuration("BASIC", vec![rule("SCAN_ON_PUSH", &["kept/*"])]);
    ok(
        &router,
        "baseline",
        "PutRegistryScanningConfiguration",
        &json!({ "scanType": "BASIC", "rules": [rule("SCAN_ON_PUSH", &["kept/*"])] }),
    )
    .await;

    // `ScanningRepositoryFilterValue` is `@length(1..255)` with pattern
    // `^[a-z0-9*](?:[._\-/a-z0-9*]?[a-z0-9*]+)*$`: one row per way to leave it, plus the
    // 255-character bound itself and one character past it.
    let at_the_bound = "a".repeat(255);
    let past_the_bound = "a".repeat(256);
    let cases: Vec<(&str, &str, bool)> = vec![
        ("a trailing separator is outside the pattern", "prod/", false),
        ("uppercase is outside the pattern", "Prod/*", false),
        ("a space is outside the pattern", "prod repo", false),
        ("a leading separator is outside the pattern", "/prod", false),
        ("the empty filter is shorter than the minimum", "", false),
        ("255 characters is the maximum", &at_the_bound, true),
        ("256 characters is past the maximum", &past_the_bound, false),
    ];

    for (case, filter, accepted) in cases {
        let request =
            json!({ "scanType": "BASIC", "rules": [rule("SCAN_ON_PUSH", &[filter])] });
        let answer = call(&router, "PutRegistryScanningConfiguration", &request).await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a filter inside the constraint must be stored, got HTTP {} \
                 with {:?}",
                answer.status, answer.body
            );
            // And it is the stored one from here on, so put the baseline back.
            let expected = configuration("BASIC", vec![rule("SCAN_ON_PUSH", &[filter])]);
            let reported = get_configuration(&router, case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the registry reports {reported:?}, expected {expected:?}",
            );
            ok(
                &router,
                case,
                "PutRegistryScanningConfiguration",
                &json!({ "scanType": "BASIC", "rules": [rule("SCAN_ON_PUSH", &["kept/*"])] }),
            )
            .await;
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a filter outside the constraint must be refused with HTTP \
                 400, got {} with {:?}",
                answer.status, answer.body
            );
            // A refusal against a constraint the model declares is stated with the
            // client-input error this service models — `InvalidParameterException`,
            // which this operation declares — naming the member and the constraint it
            // failed. See `override_refusal` in `src/service.rs`.
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.error_code()
            );
        }

        let reported = get_configuration(&router, case).await;
        assert_eq!(
            reported, baseline,
            "case {case:?}: after the call the registry reports {reported:?}, expected \
             {baseline:?}",
        );
    }
}
