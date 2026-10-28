// SPDX-License-Identifier: CC0-1.0

//! The per-set email-sending switch of a configuration set, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: writing the sending switch of a configuration set stores the
//! `SendingEnabled` the request carried (or the member's modelled default for a request
//! that omits it) on the set the path names and on no other, answers an empty 200, leaves
//! every other member of that set as it was, and is refused — storing nothing — with
//! `NotFoundException` for a name the account holds no set under and `BadRequestException`
//! for a name outside the documented character-and-length rule.
//!
//! The tables below span what the rule generalises over: the value asked for (both, and
//! the member omitted), the value the set already held (SES's default, and each value
//! written before), the length of the sequence of writes (one, two, three — so both
//! transitions and both repeats of a value appear), the form of the name (one character,
//! the longest allowed, hyphens, underscores, digits), how much other configuration the
//! set carries, and the refusal legs (a name never created, a name whose set was deleted,
//! and names that break the character or the length rule).

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_CONFIGURATION_SET_NAME_LEN;

use crate::configuration_sets::{Reply, Ses};

/// The modelled default of `PutConfigurationSetSendingOptionsRequest$SendingEnabled`: the
/// member targets `Enabled`, a boolean whose default is `false`
/// (`smithy_query(query="operation",operation="PutConfigurationSetSendingOptions")`), so a
/// request that omits it asks for sending to be disabled.
const SENDING_ENABLED_DEFAULT: bool = false;

/// Write the sending switch of the set `name`, with `body` as the request payload.
async fn put_sending(ses: &Ses, name: &str, body: Value) -> Reply {
    ses.call(
        "PUT",
        &format!("/v2/email/configuration-sets/{name}/sending"),
        Some(body),
    )
    .await
}

/// `SendingOptions$SendingEnabled` as a read of the stored set reports it.
///
/// Absent reads as `false` (the shared harness's own rule for a boolean member): the
/// member's modelled default is `false`, so a serializer that omits a defaulted member and
/// one that writes it out agree here.
fn stored_sending_enabled(read: &Reply) -> bool {
    read.flag("/SendingOptions/SendingEnabled")
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
        "SuppressionOptions",
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
struct SwitchCase {
    case: &'static str,
    name: String,
    /// The whole `CreateConfigurationSet` body, so a row can carry as much or as little
    /// other configuration as it likes.
    create: Value,
    /// What a read must report for the set right after that create.
    initial: bool,
    /// The writes to drive, in order: the `SendingEnabled` member the request carries
    /// (`None` for a body that omits it), and what a read must report afterwards.
    steps: Vec<(Option<bool>, bool)>,
}

/// The rows of the write-and-read table.
///
/// Built at run time because two rows carry a generated name (one character, and the
/// longest a `ConfigurationSetName` may be).
fn switch_cases() -> Vec<SwitchCase> {
    let longest_name = "s".repeat(MAX_CONFIGURATION_SET_NAME_LEN);
    vec![
        SwitchCase {
            case: "pause a set that holds SES's own default, one-character name",
            name: "s".to_string(),
            create: json!({"ConfigurationSetName": "s"}),
            initial: true,
            steps: vec![(Some(false), false)],
        },
        SwitchCase {
            case: "pause and resume, longest allowed name",
            name: longest_name.clone(),
            create: json!({"ConfigurationSetName": longest_name}),
            initial: true,
            steps: vec![(Some(false), false), (Some(true), true)],
        },
        SwitchCase {
            case: "resume a set created paused, then pause it again",
            name: "sending-created-paused".to_string(),
            create: json!({
                "ConfigurationSetName": "sending-created-paused",
                "SendingOptions": {"SendingEnabled": false},
            }),
            initial: false,
            steps: vec![(Some(true), true), (Some(false), false)],
        },
        SwitchCase {
            case: "the same value twice leaves the switch where it is",
            name: "sending_repeated_0".to_string(),
            create: json!({"ConfigurationSetName": "sending_repeated_0"}),
            initial: true,
            steps: vec![
                (Some(true), true),
                (Some(true), true),
                (Some(false), false),
                (Some(false), false),
            ],
        },
        SwitchCase {
            case: "the member omitted stores its modelled default",
            name: "sending-member-omitted".to_string(),
            create: json!({"ConfigurationSetName": "sending-member-omitted"}),
            initial: true,
            steps: vec![(None, SENDING_ENABLED_DEFAULT)],
        },
        SwitchCase {
            case: "the member omitted after an explicit enable",
            name: "sending_omitted_after_enable".to_string(),
            create: json!({
                "ConfigurationSetName": "sending_omitted_after_enable",
                "SendingOptions": {"SendingEnabled": false},
            }),
            initial: false,
            steps: vec![
                (Some(true), true),
                (None, SENDING_ENABLED_DEFAULT),
                (Some(true), true),
            ],
        },
        SwitchCase {
            case: "a set that carries every other option object, digits in the name",
            name: "sending_set_42".to_string(),
            create: json!({
                "ConfigurationSetName": "sending_set_42",
                "TrackingOptions": {
                    "CustomRedirectDomain": "click.example.com",
                    "HttpsPolicy": "REQUIRE",
                },
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 1800},
                "ReputationOptions": {"ReputationMetricsEnabled": true},
                "SendingOptions": {"SendingEnabled": true},
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE", "COMPLAINT"]},
                "VdmOptions": {"DashboardOptions": {"EngagementMetrics": "ENABLED"}},
                "Tags": [{"Key": "env", "Value": "test"}],
            }),
            initial: true,
            steps: vec![(Some(false), false), (Some(true), true)],
        },
    ]
}

#[tokio::test]
async fn writing_the_switch_persists_it_and_get_reads_it_back() {
    let ses = Ses::new();

    for case in switch_cases() {
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
            stored_sending_enabled(&before),
            case.initial,
            "case {:?}: expected the created set to hold SendingEnabled {}, got {:?}",
            case.case,
            case.initial,
            before.member("SendingOptions")
        );
        let untouched = members_this_operation_does_not_own(&before);

        for (step, (asked, expected)) in case.steps.iter().enumerate() {
            let body = match asked {
                Some(value) => json!({"SendingEnabled": value}),
                None => json!({}),
            };
            let written = put_sending(&ses, &case.name, body).await;
            assert_eq!(
                written.status,
                200,
                "case {:?} (step {step}): expected 200 from PutConfigurationSetSendingOptions, \
                 got {} ({})",
                case.case,
                written.status,
                written.message()
            );
            assert!(
                written.is_empty_body(),
                "case {:?} (step {step}): PutConfigurationSetSendingOptionsResponse declares no \
                 members, got {:?}",
                case.case,
                written.member("SendingOptions")
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
                stored_sending_enabled(&read),
                *expected,
                "case {:?} (step {step}): expected SendingEnabled {expected} after asking for \
                 {asked:?}, got {:?}",
                case.case,
                read.member("SendingOptions")
            );
            assert_eq!(
                members_this_operation_does_not_own(&read),
                untouched,
                "case {:?} (step {step}): writing the sending switch must leave every other \
                 member of the set as it was",
                case.case
            );

            // A read reports the stored set without changing it, so a second read answers
            // the same switch.
            let again = ses.get_set(&case.name).await;
            assert_eq!(
                stored_sending_enabled(&again),
                *expected,
                "case {:?} (step {step}): a read must not change what a later reader sees, \
                 got {:?}",
                case.case,
                again.member("SendingOptions")
            );
        }
    }
}

#[tokio::test]
async fn the_switch_belongs_to_the_set_the_path_names() {
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

    for paused in names {
        let written = put_sending(&ses, paused, json!({"SendingEnabled": false})).await;
        assert_eq!(
            written.status,
            200,
            "case {paused:?}: expected 200 from PutConfigurationSetSendingOptions, got {} ({})",
            written.status,
            written.message()
        );

        for name in names {
            let read = ses.get_set(name).await;
            let expected = name != paused;
            assert_eq!(
                stored_sending_enabled(&read),
                expected,
                "case {paused:?} paused: expected set {name:?} to report SendingEnabled \
                 {expected}, got {:?}",
                read.member("SendingOptions")
            );
        }

        // Put the set back as it was, so each row of this loop starts from three sets
        // whose sending is enabled.
        let restored = put_sending(&ses, paused, json!({"SendingEnabled": true})).await;
        assert_eq!(
            restored.status,
            200,
            "case {paused:?}: expected 200 restoring the switch, got {} ({})",
            restored.status,
            restored.message()
        );
    }
}

/// One row of the refusal table: the name a write names, the status and error code the
/// refusal must carry, and whether the account is to hold a set by that name first (and
/// have it deleted again).
struct RefusalCase {
    case: &'static str,
    name: String,
    created_then_deleted: bool,
    status: u16,
    error_code: &'static str,
}

/// The rows of the refusal table: a name the account never held, a name whose set was
/// deleted, and names outside the documented character-and-length rule (one character past
/// the published bound, and a character the rule does not admit).
fn refusal_cases() -> Vec<RefusalCase> {
    let past_the_bound = "n".repeat(MAX_CONFIGURATION_SET_NAME_LEN + 1);
    vec![
        RefusalCase {
            case: "a name the account never held",
            name: "absent-set".to_string(),
            created_then_deleted: false,
            status: 404,
            error_code: "NotFoundException",
        },
        RefusalCase {
            case: "a name whose set was deleted",
            name: "deleted_set_1".to_string(),
            created_then_deleted: true,
            status: 404,
            error_code: "NotFoundException",
        },
        RefusalCase {
            case: "one character past the published length bound",
            name: past_the_bound,
            created_then_deleted: false,
            status: 400,
            error_code: "BadRequestException",
        },
        RefusalCase {
            case: "a dot is not a character the rule admits",
            name: "has.dot".to_string(),
            created_then_deleted: false,
            status: 400,
            error_code: "BadRequestException",
        },
        RefusalCase {
            case: "a tilde is not a character the rule admits",
            name: "has~tilde".to_string(),
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

        for asked in [json!({"SendingEnabled": true}), json!({"SendingEnabled": false})] {
            let written = put_sending(&ses, &case.name, asked.clone()).await;
            assert_eq!(
                (written.status, written.error_code().as_str()),
                (case.status, case.error_code),
                "case {:?}: expected {} {} for {asked:?}, got {} {} ({})",
                case.case,
                case.status,
                case.error_code,
                written.status,
                written.error_code(),
                written.message()
            );
            assert!(
                written.message().contains(&case.name),
                "case {:?}: the refusal must name the configuration set it was asked for, \
                 got {:?}",
                case.case,
                written.message()
            );

            // The refused write stored nothing: a read of that name reports no set —
            // `NotFoundException` for a name the account simply holds none under, and the
            // same input refusal for a name outside the documented rule, which no read of
            // this service serves either.
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
}
