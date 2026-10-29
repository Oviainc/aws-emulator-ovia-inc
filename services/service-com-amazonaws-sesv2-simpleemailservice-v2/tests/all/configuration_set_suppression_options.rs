// SPDX-License-Identifier: CC0-1.0

//! The suppression list preferences of a configuration set, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: writing a configuration set's suppression list preferences
//! stores exactly the preferences the request states on the set the path names and on no
//! other — a member the request does not state is one the set no longer overrides, and a
//! request that states none leaves the set overriding nothing — answers an empty 200,
//! leaves every other member of that set as it was, and is refused, storing nothing, with
//! `NotFoundException` for a name the account holds no set under and `BadRequestException`
//! for a name outside the documented character-and-length rule or a value outside a
//! modelled value set.
//!
//! The tables below span what the rule generalises over: which members the write states
//! (each one alone, several together, none at all), the values inside them (both
//! suppression reasons, both suppression scopes, both feature statuses, every confidence
//! verdict threshold), the ORDER and the COUNT of the reasons (none, the empty list, one,
//! both, both the other way round), what the set already held (nothing, created
//! preferences, each written one), the length of the sequence of writes, the form of the
//! name (one character, the longest allowed, hyphens, underscores, digits), and the
//! refusal legs (a name never created, a name whose set was deleted, names that break the
//! character or the length rule, and an unmodelled value in each enum the request carries).

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_CONFIGURATION_SET_NAME_LEN;

use crate::configuration_sets::{Reply, Ses};

/// Write the suppression list preferences of the set `name`, with `body` as the request
/// payload.
///
/// Shared with the sibling suites of this test target, so every case drives the same wire
/// path for this operation.
pub(crate) async fn put_suppression(ses: &Ses, name: &str, body: Value) -> Reply {
    ses.call(
        "PUT",
        &format!("/v2/email/configuration-sets/{name}/suppression-options"),
        Some(body),
    )
    .await
}

/// Every member of a stored configuration set this operation's input states nothing
/// about, as a read reports them.
///
/// `ReputationOptions` is compared through its `ReputationMetricsEnabled` member alone:
/// its `LastFreshStart` is SES's own record of when the set's reputation metrics were last
/// given a fresh start, which no request carries.
fn members_this_operation_does_not_own(read: &Reply) -> Vec<(&'static str, Value)> {
    let mut members: Vec<(&'static str, Value)> = [
        "ConfigurationSetName",
        "TrackingOptions",
        "DeliveryOptions",
        "SendingOptions",
        "VdmOptions",
        "MessageSecurityOptions",
        "ArchivingOptions",
        "Tags",
    ]
    .into_iter()
    .map(|member| (member, read.member(member)))
    .collect();
    members.push((
        "ReputationOptions.ReputationMetricsEnabled",
        read.member("ReputationOptions")
            .get("ReputationMetricsEnabled")
            .cloned()
            .unwrap_or(Value::Bool(false)),
    ));
    members
}

/// One row of the write-and-read table.
struct PreferencesCase {
    case: &'static str,
    name: String,
    /// The whole `CreateConfigurationSet` body, so a row can carry as much or as little
    /// other configuration as it likes.
    create: Value,
    /// What a read must report for `SuppressionOptions` right after that create.
    initial: Value,
    /// The writes to drive, in order: the request payload, and the `SuppressionOptions` a
    /// read must report afterwards.
    steps: Vec<(Value, Value)>,
}

/// The rows of the write-and-read table.
///
/// `SuppressionListReason` is BOUNCE, COMPLAINT; `SuppressionListScope` is ACCOUNT,
/// TENANT; `FeatureStatus` is ENABLED, DISABLED; `SuppressionConfidenceVerdictThreshold`
/// is MEDIUM, HIGH, MANAGED — the value sets of the model
/// (`smithy_query(query="shape",shape="SuppressionListReason,SuppressionListScope,FeatureStatus,SuppressionConfidenceVerdictThreshold")`).
/// Built at run time because two rows carry a generated name (one character, and the
/// longest a `ConfigurationSetName` may be).
fn preferences_cases() -> Vec<PreferencesCase> {
    let longest_name = "s".repeat(MAX_CONFIGURATION_SET_NAME_LEN);
    vec![
        PreferencesCase {
            case: "one reason onto a set that overrides nothing, one-character name",
            name: "s".to_string(),
            create: json!({"ConfigurationSetName": "s"}),
            initial: Value::Null,
            steps: vec![(
                json!({"SuppressedReasons": ["BOUNCE"]}),
                json!({"SuppressedReasons": ["BOUNCE"]}),
            )],
        },
        PreferencesCase {
            case: "both reasons and each scope in turn, longest allowed name",
            name: longest_name.clone(),
            create: json!({"ConfigurationSetName": longest_name}),
            initial: Value::Null,
            steps: vec![
                (
                    json!({
                        "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                        "SuppressionScope": "ACCOUNT",
                    }),
                    json!({
                        "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                        "SuppressionScope": "ACCOUNT",
                    }),
                ),
                (
                    json!({
                        "SuppressedReasons": ["COMPLAINT"],
                        "SuppressionScope": "TENANT",
                    }),
                    json!({
                        "SuppressedReasons": ["COMPLAINT"],
                        "SuppressionScope": "TENANT",
                    }),
                ),
            ],
        },
        PreferencesCase {
            case: "the reasons are stored in the order they were supplied",
            name: "reason-order".to_string(),
            create: json!({"ConfigurationSetName": "reason-order"}),
            initial: Value::Null,
            steps: vec![
                (
                    json!({"SuppressedReasons": ["COMPLAINT", "BOUNCE"]}),
                    json!({"SuppressedReasons": ["COMPLAINT", "BOUNCE"]}),
                ),
                (
                    json!({"SuppressedReasons": ["BOUNCE", "COMPLAINT"]}),
                    json!({"SuppressedReasons": ["BOUNCE", "COMPLAINT"]}),
                ),
            ],
        },
        PreferencesCase {
            case: "the empty list overrides the preferences with no reason at all",
            name: "no_reason_0".to_string(),
            create: json!({
                "ConfigurationSetName": "no_reason_0",
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE", "COMPLAINT"]},
            }),
            initial: json!({"SuppressedReasons": ["BOUNCE", "COMPLAINT"]}),
            steps: vec![
                (
                    json!({"SuppressedReasons": []}),
                    json!({"SuppressedReasons": []}),
                ),
                (
                    json!({"SuppressedReasons": ["BOUNCE"]}),
                    json!({"SuppressedReasons": ["BOUNCE"]}),
                ),
            ],
        },
        PreferencesCase {
            case: "a write states the preferences whole, so what it omits is dropped",
            name: "whole_write".to_string(),
            create: json!({
                "ConfigurationSetName": "whole_write",
                "SuppressionOptions": {
                    "SuppressedReasons": ["BOUNCE"],
                    "SuppressionScope": "TENANT",
                },
            }),
            initial: json!({
                "SuppressedReasons": ["BOUNCE"],
                "SuppressionScope": "TENANT",
            }),
            steps: vec![
                // Only the scope: the reasons the set held are no longer overridden.
                (
                    json!({"SuppressionScope": "ACCOUNT"}),
                    json!({"SuppressionScope": "ACCOUNT"}),
                ),
                // Only the reasons: the scope is no longer overridden either.
                (
                    json!({"SuppressedReasons": ["COMPLAINT"]}),
                    json!({"SuppressedReasons": ["COMPLAINT"]}),
                ),
            ],
        },
        PreferencesCase {
            case: "a write that states nothing leaves the set overriding nothing",
            name: "states-nothing".to_string(),
            create: json!({
                "ConfigurationSetName": "states-nothing",
                "SuppressionOptions": {
                    "SuppressedReasons": ["COMPLAINT"],
                    "SuppressionScope": "ACCOUNT",
                },
            }),
            initial: json!({
                "SuppressedReasons": ["COMPLAINT"],
                "SuppressionScope": "ACCOUNT",
            }),
            steps: vec![
                (json!({}), Value::Null),
                // ... and the set takes an override again on the next write.
                (
                    json!({"SuppressionScope": "TENANT"}),
                    json!({"SuppressionScope": "TENANT"}),
                ),
                (json!({}), Value::Null),
            ],
        },
        PreferencesCase {
            case: "the same preferences twice leave the set where it is",
            name: "repeated_write_7".to_string(),
            create: json!({"ConfigurationSetName": "repeated_write_7"}),
            initial: Value::Null,
            steps: vec![
                (
                    json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"}),
                    json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"}),
                ),
                (
                    json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"}),
                    json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"}),
                ),
            ],
        },
        PreferencesCase {
            case: "validation options: the switch alone, then each confidence threshold",
            name: "validation_options".to_string(),
            create: json!({"ConfigurationSetName": "validation_options"}),
            initial: Value::Null,
            steps: vec![
                (
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {"ConditionThresholdEnabled": "DISABLED"},
                        },
                    }),
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {"ConditionThresholdEnabled": "DISABLED"},
                        },
                    }),
                ),
                (
                    json!({
                        "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                        "SuppressionScope": "TENANT",
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "MEDIUM",
                                },
                            },
                        },
                    }),
                    json!({
                        "SuppressedReasons": ["BOUNCE", "COMPLAINT"],
                        "SuppressionScope": "TENANT",
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "MEDIUM",
                                },
                            },
                        },
                    }),
                ),
                (
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "HIGH",
                                },
                            },
                        },
                    }),
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "HIGH",
                                },
                            },
                        },
                    }),
                ),
                (
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "MANAGED",
                                },
                            },
                        },
                    }),
                    json!({
                        "ValidationOptions": {
                            "ConditionThreshold": {
                                "ConditionThresholdEnabled": "ENABLED",
                                "OverallConfidenceThreshold": {
                                    "ConfidenceVerdictThreshold": "MANAGED",
                                },
                            },
                        },
                    }),
                ),
            ],
        },
        PreferencesCase {
            case: "a set that carries every other option object, digits in the name",
            name: "suppress_set_42".to_string(),
            create: json!({
                "ConfigurationSetName": "suppress_set_42",
                "TrackingOptions": {
                    "CustomRedirectDomain": "click.example.com",
                    "HttpsPolicy": "REQUIRE",
                },
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800},
                "ReputationOptions": {"ReputationMetricsEnabled": true},
                "SendingOptions": {"SendingEnabled": false},
                "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ENABLED"}},
                "Tags": [{"Key": "env", "Value": "test"}],
            }),
            initial: Value::Null,
            steps: vec![
                (
                    json!({"SuppressedReasons": ["COMPLAINT"], "SuppressionScope": "ACCOUNT"}),
                    json!({"SuppressedReasons": ["COMPLAINT"], "SuppressionScope": "ACCOUNT"}),
                ),
                (json!({}), Value::Null),
            ],
        },
    ]
}

#[tokio::test]
async fn writing_the_preferences_persists_them_and_get_reads_them_back() {
    let ses = Ses::new();

    for case in preferences_cases() {
        let created = ses.create_set(case.create.clone()).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200 from CreateConfigurationSet, got {} ({})",
            case.case,
            created.status,
            created.message()
        );

        let before = ses.get_set(&case.name).await;
        assert_eq!(
            before.member("SuppressionOptions"),
            case.initial,
            "case {:?}: expected the created set to hold SuppressionOptions {:?}, got {:?}",
            case.case,
            case.initial,
            before.member("SuppressionOptions")
        );
        let untouched = members_this_operation_does_not_own(&before);

        for (step, (asked, expected)) in case.steps.iter().enumerate() {
            let written = put_suppression(&ses, &case.name, asked.clone()).await;
            assert_eq!(
                written.status,
                200,
                "case {:?} (step {step}): expected 200 from \
                 PutConfigurationSetSuppressionOptions for {asked:?}, got {} ({})",
                case.case,
                written.status,
                written.message()
            );
            assert!(
                written.is_empty_body(),
                "case {:?} (step {step}): PutConfigurationSetSuppressionOptionsResponse declares \
                 no members, got {:?}",
                case.case,
                written.member("SuppressionOptions")
            );

            let read = ses.get_set(&case.name).await;
            assert_eq!(
                read.status,
                200,
                "case {:?} (step {step}): expected 200 from GetConfigurationSet, got {} ({})",
                case.case,
                read.status,
                read.message()
            );
            assert_eq!(
                read.member("SuppressionOptions"),
                *expected,
                "case {:?} (step {step}): expected SuppressionOptions {expected:?} after asking \
                 for {asked:?}, got {:?}",
                case.case,
                read.member("SuppressionOptions")
            );
            assert_eq!(
                members_this_operation_does_not_own(&read),
                untouched,
                "case {:?} (step {step}): writing the suppression preferences must leave every \
                 other member of the set as it was",
                case.case
            );

            // A read reports the stored set without changing it, so a second read answers
            // the same preferences.
            let again = ses.get_set(&case.name).await;
            assert_eq!(
                again.member("SuppressionOptions"),
                *expected,
                "case {:?} (step {step}): a read must not change what a later reader sees, \
                 got {:?}",
                case.case,
                again.member("SuppressionOptions")
            );
        }
    }
}

#[tokio::test]
async fn the_preferences_belong_to_the_set_the_path_names() {
    let ses = Ses::new();
    // Three sets created alike, so the only thing that distinguishes them afterwards is
    // which one a write named.
    let names = ["neighbour-a", "neighbour_b", "neighbour0c"];
    for name in names {
        let created = ses.create_set(json!({"ConfigurationSetName": name})).await;
        assert_eq!(
            created.status,
            200,
            "case {name:?}: expected 200 from CreateConfigurationSet, got {} ({})",
            created.status,
            created.message()
        );
    }

    for written_to in names {
        let asked = json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"});
        let written = put_suppression(&ses, written_to, asked.clone()).await;
        assert_eq!(
            written.status,
            200,
            "case {written_to:?}: expected 200 from PutConfigurationSetSuppressionOptions, \
             got {} ({})",
            written.status,
            written.message()
        );

        for name in names {
            let read = ses.get_set(name).await;
            let expected = if name == written_to {
                asked.clone()
            } else {
                Value::Null
            };
            assert_eq!(
                read.member("SuppressionOptions"),
                expected,
                "case {written_to:?} written: expected set {name:?} to report \
                 SuppressionOptions {expected:?}, got {:?}",
                read.member("SuppressionOptions")
            );
        }

        // Put the set back as it was, so each row of this loop starts from three sets
        // that override nothing.
        let restored = put_suppression(&ses, written_to, json!({})).await;
        assert_eq!(
            restored.status,
            200,
            "case {written_to:?}: expected 200 clearing the preferences, got {} ({})",
            restored.status,
            restored.message()
        );
    }
}

/// One row of the refusal table: the name a write names, the payload it carries, the
/// status and error code the refusal must carry, and whether the account is to hold a set
/// by that name first (and have it deleted again).
struct RefusalCase {
    case: &'static str,
    name: String,
    body: Value,
    created_then_deleted: bool,
    status: u16,
    error_code: &'static str,
}

/// The rows of the refusal table: a name the account never held, a name whose set was
/// deleted, names outside the documented character-and-length rule (one character past
/// the published bound, and characters the rule does not admit), and a value outside the
/// modelled value set of each enum this request carries.
fn refusal_cases() -> Vec<RefusalCase> {
    let past_the_bound = "n".repeat(MAX_CONFIGURATION_SET_NAME_LEN + 1);
    let reasons = json!({"SuppressedReasons": ["BOUNCE"]});
    vec![
        RefusalCase {
            case: "a name the account never held",
            name: "absent-set".to_string(),
            body: reasons.clone(),
            created_then_deleted: false,
            status: 404,
            error_code: "NotFoundException",
        },
        RefusalCase {
            case: "a name whose set was deleted",
            name: "deleted_set_1".to_string(),
            body: reasons.clone(),
            created_then_deleted: true,
            status: 404,
            error_code: "NotFoundException",
        },
        RefusalCase {
            case: "one character past the published length bound",
            name: past_the_bound,
            body: reasons.clone(),
            created_then_deleted: false,
            status: 400,
            error_code: "BadRequestException",
        },
        RefusalCase {
            case: "a dot is not a character the rule admits",
            name: "has.dot".to_string(),
            body: reasons.clone(),
            created_then_deleted: false,
            status: 400,
            error_code: "BadRequestException",
        },
        RefusalCase {
            case: "a tilde is not a character the rule admits",
            name: "has~tilde".to_string(),
            body: reasons,
            created_then_deleted: false,
            status: 400,
            error_code: "BadRequestException",
        },
    ]
}

#[tokio::test]
async fn a_write_the_service_cannot_serve_is_refused_and_stores_nothing() {
    let ses = Ses::new();

    for case in refusal_cases() {
        if case.created_then_deleted {
            let created = ses
                .create_set(json!({"ConfigurationSetName": case.name}))
                .await;
            assert_eq!(
                created.status,
                200,
                "case {:?}: expected 200 from CreateConfigurationSet, got {} ({})",
                case.case,
                created.status,
                created.message()
            );
            let deleted = ses
                .call(
                    "DELETE",
                    &format!("/v2/email/configuration-sets/{}", case.name),
                    None,
                )
                .await;
            assert_eq!(
                deleted.status,
                200,
                "case {:?}: expected 200 from DeleteConfigurationSet, got {} ({})",
                case.case,
                deleted.status,
                deleted.message()
            );
        }

        let written = put_suppression(&ses, &case.name, case.body.clone()).await;
        assert_eq!(
            (written.status, written.error_code().as_str()),
            (case.status, case.error_code),
            "case {:?}: expected {} {} for {:?}, got {} {} ({})",
            case.case,
            case.status,
            case.error_code,
            case.body,
            written.status,
            written.error_code(),
            written.message()
        );
        assert!(
            written.message().contains(&case.name),
            "case {:?}: the refusal must name the configuration set it was asked for, got {:?}",
            case.case,
            written.message()
        );

        // The refused write stored nothing: a read of that name reports no set —
        // `NotFoundException` for a name the account simply holds none under, and the same
        // input refusal for a name outside the documented rule, which no read of this
        // service serves either.
        let read = ses.get_set(&case.name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (case.status, case.error_code),
            "case {:?}: a refused write must store nothing, got {} {} ({})",
            case.case,
            read.status,
            read.error_code(),
            read.message()
        );
    }
}

/// One row of the enum table: the payload a write carries, and whether the service is to
/// accept it.
struct EnumCase {
    case: &'static str,
    body: Value,
    accepted: bool,
}

/// The rows of the enum table: for every enum this request carries, a value the model
/// declares and one it does not.
///
/// The declared values are the model's own
/// (`smithy_query(query="shape",shape="SuppressionListReason,SuppressionListScope,FeatureStatus,SuppressionConfidenceVerdictThreshold")`),
/// including entries the issue did not cite — the `MANAGED` confidence threshold and the
/// `DISABLED` feature status.
fn enum_cases() -> Vec<EnumCase> {
    vec![
        EnumCase {
            case: "SuppressedReasons COMPLAINT is declared",
            body: json!({"SuppressedReasons": ["COMPLAINT"]}),
            accepted: true,
        },
        EnumCase {
            case: "SuppressedReasons SPAM is not declared",
            body: json!({"SuppressedReasons": ["BOUNCE", "SPAM"]}),
            accepted: false,
        },
        EnumCase {
            case: "SuppressionScope ACCOUNT is declared",
            body: json!({"SuppressionScope": "ACCOUNT"}),
            accepted: true,
        },
        EnumCase {
            case: "SuppressionScope GLOBAL is not declared",
            body: json!({"SuppressionScope": "GLOBAL"}),
            accepted: false,
        },
        EnumCase {
            case: "ConditionThresholdEnabled DISABLED is declared",
            body: json!({
                "ValidationOptions": {
                    "ConditionThreshold": {"ConditionThresholdEnabled": "DISABLED"},
                },
            }),
            accepted: true,
        },
        EnumCase {
            case: "ConditionThresholdEnabled ON is not declared",
            body: json!({
                "ValidationOptions": {
                    "ConditionThreshold": {"ConditionThresholdEnabled": "ON"},
                },
            }),
            accepted: false,
        },
        EnumCase {
            case: "ConfidenceVerdictThreshold MANAGED is declared",
            body: json!({
                "ValidationOptions": {
                    "ConditionThreshold": {
                        "ConditionThresholdEnabled": "ENABLED",
                        "OverallConfidenceThreshold": {"ConfidenceVerdictThreshold": "MANAGED"},
                    },
                },
            }),
            accepted: true,
        },
        EnumCase {
            case: "ConfidenceVerdictThreshold LOW is not declared",
            body: json!({
                "ValidationOptions": {
                    "ConditionThreshold": {
                        "ConditionThresholdEnabled": "ENABLED",
                        "OverallConfidenceThreshold": {"ConfidenceVerdictThreshold": "LOW"},
                    },
                },
            }),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn an_enum_member_outside_its_modelled_value_set_is_refused_and_stores_nothing() {
    let ses = Ses::new();
    let name = "enum-preferences";
    let created = ses.create_set(json!({"ConfigurationSetName": name})).await;
    assert_eq!(
        created.status,
        200,
        "expected 200 from CreateConfigurationSet, got {} ({})",
        created.status,
        created.message()
    );

    // The preferences the set holds while the table runs, so a refused write is seen to
    // leave them exactly as they were.
    let held = json!({"SuppressedReasons": ["BOUNCE"], "SuppressionScope": "TENANT"});
    let seeded = put_suppression(&ses, name, held.clone()).await;
    assert_eq!(
        seeded.status,
        200,
        "expected 200 seeding the preferences, got {} ({})",
        seeded.status,
        seeded.message()
    );

    for case in enum_cases() {
        let written = put_suppression(&ses, name, case.body.clone()).await;
        if case.accepted {
            assert_eq!(
                written.status,
                200,
                "case {:?}: expected 200 for {:?}, got {} {} ({})",
                case.case,
                case.body,
                written.status,
                written.error_code(),
                written.message()
            );
            // Put the set back to the preferences the next refusal is measured against.
            let restored = put_suppression(&ses, name, held.clone()).await;
            assert_eq!(
                restored.status,
                200,
                "case {:?}: expected 200 restoring the preferences, got {} ({})",
                case.case,
                restored.status,
                restored.message()
            );
            continue;
        }

        assert_eq!(
            (written.status, written.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException for {:?}, got {} {} ({})",
            case.case,
            case.body,
            written.status,
            written.error_code(),
            written.message()
        );
        let read = ses.get_set(name).await;
        assert_eq!(
            read.member("SuppressionOptions"),
            held,
            "case {:?}: a refused write must store nothing, got {:?}",
            case.case,
            read.member("SuppressionOptions")
        );
    }
}
