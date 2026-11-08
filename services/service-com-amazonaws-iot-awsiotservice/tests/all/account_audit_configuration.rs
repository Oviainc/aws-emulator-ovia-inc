// SPDX-License-Identifier: CC0-1.0

//! The account's DEVICE DEFENDER AUDIT CONFIGURATION, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's service
//! is mounted in, so routing, the restJson1 `GET /audit/configuration` and
//! `PATCH /audit/configuration` bindings, the handlers and the response serialization are
//! all exercised.
//!
//! The rules under test, none of them specific to one configuration:
//!
//! * `DescribeAccountAuditConfiguration` reports the configuration this service HOLDS —
//!   the role, the per-check settings and the notification targets the last
//!   `UpdateAccountAuditConfiguration` persisted — over the WHOLE catalog of audit checks
//!   and of notification target types, each check or target nobody configured reported
//!   disabled, because the read is documented as the way to see "the list of all checks,
//!   including those that are currently enabled"
//!   (`smithy_query(query="doc",shape="UpdateAccountAuditConfigurationRequest",member="auditCheckConfigurations")`);
//! * an account that has never updated it still has a configuration: no role, every check
//!   disabled, every notification target disabled;
//! * the read is a read: repeating it, with no update between, reports the same
//!   configuration and leaves the store alone;
//! * a member the update SUPPLIES is merged key by key into the stored configuration and
//!   one it OMITS is left exactly as it was, so a later update enabling one check neither
//!   disables the others nor clears the role;
//! * the first update must supply `roleArn` and at least one enabled check ("On the first
//!   call to UpdateAccountAuditConfiguration, this parameter is required and must specify
//!   at least one enabled check"), and a key of `auditCheckConfigurations`, of a check's
//!   `configuration` or of `auditNotificationTargetConfigurations` that names nothing in
//!   its catalog is refused — each with the modelled `InvalidRequestException` (400),
//!   changing nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::{
    AUDIT_NOTIFICATION_TYPE_VALUES, CONFIG_NAME_VALUES,
};
use service_com_amazonaws_iot_awsiotservice::state::AUDIT_CHECK_NAMES;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. The configuration is
/// account-wide, so no assertion depends on it.
const REGION: &str = "ap-south-1";

/// A role ARN of exactly the modelled minimum length (`RoleArn`, len 20..2048), so a row
/// can vary the role's LENGTH down to its published bound without leaving the constraint.
const SHORT_ROLE_ARN: &str = "arn:aws:iam::1:r/abc";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}/audit/configuration"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the audit-configuration request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn describe_account_audit_configuration(&self) -> Reply {
        self.dispatch("GET", None).await
    }

    async fn update_account_audit_configuration(&self, body: &Value) -> Reply {
        self.dispatch("PATCH", Some(body)).await
    }

    /// The configuration a successful read reports, as JSON.
    async fn reported(&self, case: &str) -> Value {
        let reply = self.describe_account_audit_configuration().await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected DescribeAccountAuditConfiguration to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }

    /// Apply `request` and require it to succeed.
    async fn apply(&self, case: &str, request: &Value) {
        let reply = self.update_account_audit_configuration(request).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected UpdateAccountAuditConfiguration to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// One audit check configuration, in the JSON form the model binds it to.
fn check(enabled: bool) -> Value {
    json!({ "enabled": enabled })
}

/// One audit check configuration carrying a `configName`/`configValue` pair.
fn configured_check(enabled: bool, config_name: &str, config_value: &str) -> Value {
    json!({ "enabled": enabled, "configuration": { config_name: config_value } })
}

/// An `auditCheckConfigurations` map built name by name, because the check NAME is what
/// varies between rows.
fn check_map(entries: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (check_name, configuration) in entries {
        map.insert((*check_name).to_string(), configuration.clone());
    }
    Value::Object(map)
}

/// Every audit check in the catalog, enabled — the catalog is the emulator's own record of
/// the Device Defender audit checks (`service_..::state::AUDIT_CHECK_NAMES`), so this row
/// covers names the issue never cited.
fn every_check_enabled() -> Value {
    let entries: Vec<(&str, Value)> = AUDIT_CHECK_NAMES
        .iter()
        .map(|check_name| (*check_name, check(true)))
        .collect();
    check_map(&entries)
}

/// Whether `report`'s `auditCheckConfigurations` says `check_name` is enabled.
///
/// A member with a modelled default is absent from the wire when it carries that default,
/// so a missing `enabled` reads as `false`; the ENTRY itself must be there, which
/// [`assert_reports`] checks.
fn reported_enabled(report: &Value, check_name: &str) -> bool {
    report["auditCheckConfigurations"][check_name]["enabled"]
        .as_bool()
        .unwrap_or(false)
}

/// What `member` of `supplied` says about `key`, if the request supplied it at all.
fn supplied_entry<'a>(supplied: &'a Value, member: &str, key: &str) -> Option<&'a Value> {
    supplied.get(member).and_then(|map| map.get(key))
}

/// Assert that a read reports exactly `expected_role_arn` and the configurations
/// `expected` (the accumulated effect of every update applied so far) over the whole
/// catalog.
fn assert_reports(case: &str, report: &Value, expected_role_arn: Option<&str>, expected: &Value) {
    assert_eq!(
        report["roleArn"].as_str(),
        expected_role_arn,
        "case {case}: expected roleArn {expected_role_arn:?}, got {report}"
    );
    let checks = report["auditCheckConfigurations"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: no auditCheckConfigurations map in {report}"));
    let reported_names: Vec<&String> = checks.keys().collect();
    let catalog: Vec<&str> = AUDIT_CHECK_NAMES.to_vec();
    assert_eq!(
        reported_names.len(),
        catalog.len(),
        "case {case}: expected the whole catalog of {} checks, got {reported_names:?}",
        catalog.len()
    );
    for check_name in &catalog {
        let entry = checks
            .get(*check_name)
            .unwrap_or_else(|| panic!("case {case}: {check_name} missing from {report}"));
        let expected_entry = supplied_entry(expected, "auditCheckConfigurations", check_name);
        let expected_enabled = expected_entry
            .and_then(|entry| entry.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(
            reported_enabled(report, check_name),
            expected_enabled,
            "case {case}: expected {check_name} enabled={expected_enabled}, got {entry}"
        );
        let expected_configuration = expected_entry.and_then(|entry| entry.get("configuration"));
        assert_eq!(
            entry.get("configuration"),
            expected_configuration,
            "case {case}: expected {check_name} configuration {expected_configuration:?}, \
             got {entry}"
        );
    }
    let targets = report["auditNotificationTargetConfigurations"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case}: no auditNotificationTargetConfigurations map in {report}")
        });
    assert_eq!(
        targets.len(),
        AUDIT_NOTIFICATION_TYPE_VALUES.len(),
        "case {case}: expected one entry per modelled notification type, got {targets:?}"
    );
    for notification_type in AUDIT_NOTIFICATION_TYPE_VALUES {
        let entry = targets
            .get(*notification_type)
            .unwrap_or_else(|| panic!("case {case}: {notification_type} missing from {report}"));
        let expected_entry =
            supplied_entry(expected, "auditNotificationTargetConfigurations", notification_type);
        for member in ["targetArn", "roleArn"] {
            let expected_member = expected_entry.and_then(|entry| entry.get(member));
            assert_eq!(
                entry.get(member),
                expected_member,
                "case {case}: expected {notification_type} {member} {expected_member:?}, \
                 got {entry}"
            );
        }
        let expected_enabled = expected_entry
            .and_then(|entry| entry.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let reported = entry["enabled"].as_bool().unwrap_or(false);
        assert_eq!(
            reported, expected_enabled,
            "case {case}: expected {notification_type} enabled={expected_enabled}, got {entry}"
        );
    }
}

/// Merge `request`'s maps into `expected`, which is what the store holds after the request
/// is applied: a supplied key replaces that key, an omitted one is left alone.
fn merge_into(expected: &mut Value, request: &Value) {
    for member in [
        "auditCheckConfigurations",
        "auditNotificationTargetConfigurations",
    ] {
        let Some(supplied) = request.get(member).and_then(Value::as_object) else {
            continue;
        };
        let target = expected
            .as_object_mut()
            .expect("the expectation is a JSON object")
            .entry(member.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let target = target
            .as_object_mut()
            .expect("the expected map is a JSON object");
        for (key, value) in supplied {
            target.insert(key.clone(), value.clone());
        }
    }
}

/// One update that must round-trip through the reader.
struct RoundTripCase {
    name: &'static str,
    request: fn() -> Value,
}

/// The role ARN a first update supplies.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/device-defender-audit";

/// Configurations spanning what the rule generalises over: how many checks the request
/// configures, whether each is enabled or disabled, whether a check carries a custom
/// configuration (one row per modelled `ConfigName`), the length of the role ARN, and
/// whether a notification target is supplied, enabled or disabled.
const ROUND_TRIP_CASES: &[RoundTripCase] = &[
    RoundTripCase {
        name: "one check enabled, nothing else",
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(true))]),
            })
        },
    },
    RoundTripCase {
        name: "every check in the catalog enabled",
        request: || json!({ "roleArn": ROLE_ARN, "auditCheckConfigurations": every_check_enabled() }),
    },
    RoundTripCase {
        name: "one check enabled and two disabled",
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[
                    ("CA_CERTIFICATE_EXPIRING_CHECK", check(true)),
                    ("CONFLICTING_CLIENT_IDS_CHECK", check(false)),
                    ("REVOKED_CA_CERTIFICATE_STILL_ACTIVE_CHECK", check(false)),
                ]),
            })
        },
    },
    RoundTripCase {
        name: "a check carrying each modelled configName",
        request: || {
            // One check per value of the modelled `ConfigName` enum
            // (`smithy_query(query="shape",shape="ConfigName")`), each paired with the
            // check whose threshold it sets.
            let entries: &[(&str, Value)] = &[
                (
                    "DEVICE_CERTIFICATE_AGE_CHECK",
                    configured_check(true, "CERT_AGE_THRESHOLD_IN_DAYS", "365"),
                ),
                (
                    "CA_CERTIFICATE_EXPIRING_CHECK",
                    configured_check(true, "CERT_EXPIRATION_THRESHOLD_IN_DAYS", "7"),
                ),
            ];
            assert_eq!(
                entries.len(),
                CONFIG_NAME_VALUES.len(),
                "case a check carrying each modelled configName: expected one row per \
                 modelled configName {CONFIG_NAME_VALUES:?}, got {} rows",
                entries.len()
            );
            json!({ "roleArn": ROLE_ARN, "auditCheckConfigurations": check_map(entries) })
        },
    },
    RoundTripCase {
        name: "a role ARN of the modelled minimum length",
        request: || {
            json!({
                "roleArn": SHORT_ROLE_ARN,
                "auditCheckConfigurations": check_map(&[(
                    "DEVICE_CERTIFICATE_SHARED_CHECK",
                    check(true),
                )]),
            })
        },
    },
    RoundTripCase {
        name: "an enabled notification target",
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(true))]),
                "auditNotificationTargetConfigurations": {
                    "SNS": {
                        "targetArn": "arn:aws:sns:ap-south-1:123456789012:audit-findings",
                        "roleArn": ROLE_ARN,
                        "enabled": true,
                    },
                },
            })
        },
    },
    RoundTripCase {
        name: "a notification target that is supplied but disabled",
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[(
                    "IOT_POLICY_OVERLY_PERMISSIVE_CHECK",
                    check(true),
                )]),
                "auditNotificationTargetConfigurations": {
                    "SNS": {
                        "targetArn": "arn:aws:sns:ap-south-1:123456789012:audit-findings",
                        "roleArn": ROLE_ARN,
                        "enabled": false,
                    },
                },
            })
        },
    },
];

/// An account that has never configured auditing still HAS a configuration: no role, and
/// every audit check and every notification target type reported disabled.
#[tokio::test]
async fn a_fresh_account_reports_the_disabled_configuration() {
    let emulator = Emulator::new();
    let report = emulator.reported("fresh account").await;
    assert_reports(
        "fresh account",
        &report,
        None,
        &Value::Object(Map::new()),
    );
}

/// What an update persists is what the read reports, over configurations spanning the
/// class.
#[tokio::test]
async fn an_update_round_trips_through_the_read() {
    for case in ROUND_TRIP_CASES {
        let emulator = Emulator::new();
        let request = (case.request)();
        emulator.apply(case.name, &request).await;
        let report = emulator.reported(case.name).await;
        assert_reports(
            case.name,
            &report,
            request["roleArn"].as_str(),
            &request,
        );
    }
}

/// The read is a READ: repeating it with no update between reports the same configuration,
/// both for a fresh account and for a configured one.
#[tokio::test]
async fn repeating_the_read_reports_the_same_configuration() {
    let emulator = Emulator::new();
    let first = emulator.reported("fresh account, first read").await;
    let second = emulator.reported("fresh account, second read").await;
    assert_eq!(
        first, second,
        "case fresh account: the second read reported {second} after {first}"
    );

    let request = json!({
        "roleArn": ROLE_ARN,
        "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(true))]),
    });
    emulator.apply("configured account", &request).await;
    let first = emulator.reported("configured account, first read").await;
    let second = emulator.reported("configured account, second read").await;
    assert_eq!(
        first, second,
        "case configured account: the second read reported {second} after {first}"
    );
}

/// A later update is MERGED into what the account holds: a key it supplies replaces that
/// key, a key and a member it omits are left exactly as they were — so enabling one more
/// check neither disables the others nor clears the role, and disabling a check leaves the
/// rest enabled.
#[tokio::test]
async fn a_later_update_merges_into_the_stored_configuration() {
    let emulator = Emulator::new();
    let mut expected = json!({
        "roleArn": ROLE_ARN,
        "auditCheckConfigurations": every_check_enabled(),
    });
    let mut expected_role_arn = ROLE_ARN;
    emulator.apply("first update", &expected.clone()).await;

    let steps: &[(&str, Value, Option<&str>)] = &[
        (
            "disable one check",
            json!({
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(false))]),
            }),
            None,
        ),
        (
            "re-enable it with a custom configuration",
            json!({
                "auditCheckConfigurations": check_map(&[(
                    "DEVICE_CERTIFICATE_AGE_CHECK",
                    configured_check(true, "CERT_AGE_THRESHOLD_IN_DAYS", "365"),
                )]),
            }),
            None,
        ),
        (
            "add a notification target, touching no check",
            json!({
                "auditNotificationTargetConfigurations": {
                    "SNS": { "targetArn": "arn:aws:sns:ap-south-1:123456789012:findings", "enabled": true },
                },
            }),
            None,
        ),
        (
            "replace the role, touching nothing else",
            json!({ "roleArn": SHORT_ROLE_ARN }),
            Some(SHORT_ROLE_ARN),
        ),
    ];

    for (name, request, new_role_arn) in steps {
        emulator.apply(name, request).await;
        merge_into(&mut expected, request);
        if let Some(role_arn) = new_role_arn {
            expected_role_arn = role_arn;
        }
        let report = emulator.reported(name).await;
        assert_reports(name, &report, Some(expected_role_arn), &expected);
    }
}

/// One update the service must refuse, and why.
struct RefusalCase {
    name: &'static str,
    /// Whether the account is already configured when the refused request arrives.
    configured_first: bool,
    request: fn() -> Value,
}

/// The requests the documented rules refuse: a first update missing the role or enabling
/// no check, and any update naming a key that is not an audit check (the catalog is
/// case-sensitive, so a lower-case spelling of a real check is not one), not a modelled
/// `configName`, or not a modelled `AuditNotificationType` — a map KEY carries no modelled
/// constraint the boundary validator can refuse, so the catalogs are enforced here.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        name: "first update without a role",
        configured_first: false,
        request: || {
            json!({
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(true))]),
            })
        },
    },
    RefusalCase {
        name: "first update enabling no check",
        configured_first: false,
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(false))]),
            })
        },
    },
    RefusalCase {
        name: "first update with no check at all",
        configured_first: false,
        request: || json!({ "roleArn": ROLE_ARN }),
    },
    RefusalCase {
        name: "a key that names no audit check",
        configured_first: false,
        request: || {
            json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[("NO_SUCH_AUDIT_CHECK", check(true))]),
            })
        },
    },
    RefusalCase {
        name: "a lower-case spelling of an audit check",
        configured_first: true,
        request: || {
            json!({
                "auditCheckConfigurations": check_map(&[("logging_disabled_check", check(true))]),
            })
        },
    },
    RefusalCase {
        name: "one good check beside a key that names none",
        configured_first: true,
        request: || {
            json!({
                "auditCheckConfigurations": check_map(&[
                    ("CONFLICTING_CLIENT_IDS_CHECK", check(true)),
                    ("CONFLICTING_CLIENT_IDS", check(true)),
                ]),
            })
        },
    },
    RefusalCase {
        name: "a check configuration key that names no modelled configName",
        configured_first: true,
        request: || {
            json!({
                "auditCheckConfigurations": check_map(&[(
                    "DEVICE_CERTIFICATE_AGE_CHECK",
                    configured_check(true, "CERT_AGE_THRESHOLD", "30"),
                )]),
            })
        },
    },
    RefusalCase {
        name: "a notification target type the model does not declare",
        configured_first: true,
        request: || {
            json!({
                "auditNotificationTargetConfigurations": {
                    "SQS": { "targetArn": "arn:aws:sqs:ap-south-1:123456789012:q", "enabled": true },
                },
            })
        },
    },
];

/// A refused update answers the modelled `InvalidRequestException` (400) and changes
/// nothing: the read after it reports exactly what the read before it did.
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    for case in REFUSAL_CASES {
        let emulator = Emulator::new();
        let mut expected = Value::Object(Map::new());
        let mut expected_role_arn = None;
        if case.configured_first {
            let first = json!({
                "roleArn": ROLE_ARN,
                "auditCheckConfigurations": check_map(&[(
                    "DEVICE_CERTIFICATE_EXPIRING_CHECK",
                    check(true),
                )]),
            });
            emulator.apply(case.name, &first).await;
            merge_into(&mut expected, &first);
            expected_role_arn = Some(ROLE_ARN);
        }
        let before = emulator.reported(case.name).await;

        let reply = emulator
            .update_account_audit_configuration(&(case.request)())
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {}: expected 400, got {} body {}",
            case.name,
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {}: expected InvalidRequestException, got {}",
            case.name,
            reply.text()
        );

        let after = emulator.reported(case.name).await;
        assert_eq!(
            before, after,
            "case {}: a refused update changed the configuration to {after}",
            case.name
        );
        assert_reports(case.name, &after, expected_role_arn, &expected);
    }
}

/// A `roleArn` one character outside either published bound of `RoleArn` (len 20..2048) is
/// refused at the boundary with a 400 `ValidationException`, and changes nothing — the
/// bound itself is exercised by the round-trip row that supplies a role of exactly the
/// minimum length.
#[tokio::test]
async fn a_role_arn_outside_the_modelled_length_is_refused() {
    let one_short = "arn:aws:iam::1:r/ab";
    let one_long = format!("arn:aws:iam::1:role/{}", "a".repeat(2029));
    let cases: &[(&str, &str)] = &[
        ("one character under the minimum", one_short),
        ("one character over the maximum", one_long.as_str()),
    ];
    for (name, role_arn) in cases {
        assert!(
            role_arn.len() == 19 || role_arn.len() == 2049,
            "case {name}: expected a length one past a bound, got {}",
            role_arn.len()
        );
        let emulator = Emulator::new();
        let reply = emulator
            .update_account_audit_configuration(&json!({
                "roleArn": role_arn,
                "auditCheckConfigurations": check_map(&[("LOGGING_DISABLED_CHECK", check(true))]),
            }))
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {name}: expected ValidationException, got {}",
            reply.text()
        );
        let after = emulator.reported(name).await;
        assert_reports(name, &after, None, &Value::Object(Map::new()));
    }
}
