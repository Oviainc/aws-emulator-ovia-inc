// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's alerts.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * an alert evaluates its query under an access profile of ITS space that a trust
//!   grant authorizes assuming — a profile no grant names cannot back an alert, so the
//!   `profileId` naming it is refused with `ValidationException` (400), and the SAME
//!   request succeeds once a grant records that trust;
//! * `CreateAlert` mints the alert in an EXISTING space and `GetAlert` answers from that
//!   same store;
//! * every derived member (`alertId`, `alertArn`, `accountId`, `notificationStatus`,
//!   `createdAt`/`updatedAt`) is computed from the request and the store rather than
//!   fixed, and an alert's `name` is not its identity, so two alerts of one space may
//!   share a name;
//! * `Alert.state` is absent on create and computed on read: the store holds no
//!   telemetry, so every evaluation produces no data and the rule's own `noData.treatAs`
//!   carries the resulting state (`NODATA` when the rule configures none);
//! * the rule completeness and value rules the model documents are enforced, over the
//!   whole published catalog of evaluation intervals and enum values, and nothing is
//!   persisted for a refused request;
//! * an alert is addressed by its space AND its id, so neither an unknown id nor a known
//!   id under another space resolves;
//! * `@idempotencyToken` replays return the original alert.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_grant::CreateAccessGrant;
use omni::ops::create_access_profile::CreateAccessProfile;
use omni::ops::create_alert::{CreateAlert, CreateAlertError};
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::get_alert::{GetAlert, GetAlertError};
use omni::ops::types::{
    AccessGrantPrincipal, Alert, AlertCondition, AlertEvaluation, AlertRuleQuery,
    CreateAccessGrantInput, CreateAccessProfileInput, CreateAlertInput, CreateAlertOutput,
    CreateDomainInput, CreateSpaceInput, GetAlertInput, GetAlertOutput, NoData, NotificationRule,
    NotificationTarget, NotificationTrigger, ResourceScope, Rule, ScopedActions, TelemetryRule,
    ALERT_STATE_VALUES, COMPARATOR_VALUES, CREATE_ALERT_INPUT_SCHEMA, GET_ALERT_INPUT_SCHEMA,
    NOTIFICATION_TARGET_TYPE_VALUES, QUERY_LANGUAGE_VALUES, THRESHOLD_MODE_VALUES,
};
use omni::state::{
    ACCESS_PROFILE_RESOURCE_TYPE, ALERT_EVALUATION_INTERVALS, ALERT_STATE_NODATA,
    NOTIFICATION_STATUS_DISABLED, NOTIFICATION_STATUS_ENABLED,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// An SNS topic ARN for a notification target, satisfying `Arn`'s `^arn:.+$`.
const TOPIC_ARN: &str = "arn:aws:sns:us-west-2:000000000000:omni-alerts";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `ProfileId` (its modelled `^[a-zA-Z0-9_-]+$`) that names nothing.
const MISSING_PROFILE_ID: &str = "no-such-access-profile";

/// A well-formed `AlertId` (its modelled `^[0-9a-f]{32}$`) that names nothing.
const MISSING_ALERT_ID: &str = "0123456789abcdef0123456789abcdef";

/// Whether `value` satisfies `AlertId`'s modelled `^[0-9a-f]{32}$` with its
/// `@length(32..32)`, from `smithy_query(query="shape",shape="AlertId")`.
///
/// Checked by hand rather than through a regex engine, so this crate declares no
/// dependency beyond the ones it already has. The predicate is exercised on values that
/// must FAIL it as well as pass ([`the_alert_id_pattern_is_checked_as_written`]).
fn is_alert_id(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Whether `value` satisfies `Arn`'s modelled `^arn:.+$`.
fn is_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether `value` satisfies `AccountId`'s modelled `^[0-9]{12}$`.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and `ACCESS_KEY_ID`.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A domain, through the handler that owns it.
    async fn domain(&self, case: &str, name: &str) -> String {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id
    }

    /// A space of `domain_id`, through the handler that owns it.
    async fn space(&self, case: &str, domain_id: &str, name: &str) -> String {
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.to_string(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id
    }

    /// An access profile of `space_id`, and its ARN. No trust grant names it yet, so it
    /// is not assumable until [`Omni::trust_grant`] records one.
    async fn access_profile(&self, case: &str, space_id: &str, name: &str) -> (String, String) {
        let profile = CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile;
        (profile.profile_id, profile.arn)
    }

    /// An access grant whose scoped actions name `profile_arn`, which is what records
    /// the trust that lets an alert assume that access profile.
    async fn trust_grant(
        &self,
        case: &str,
        domain_id: &str,
        space_id: &str,
        name: &str,
        profile_arn: &str,
    ) {
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: domain_id.to_string(),
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    permission: "CUSTOM".to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: "ALERT".to_string(),
                        principal_id: Some(name.to_string()),
                        principal_attributes: None,
                    },
                    scoped_actions: Some(vec![ScopedActions {
                        actions: vec!["cloudwatch:GetTelemetry".to_string()],
                        resources: Some(vec![ResourceScope {
                            resource_type: ACCESS_PROFILE_RESOURCE_TYPE.to_string(),
                            resource_arns: Some(vec![profile_arn.to_string()]),
                            tags: None,
                            signal_types: None,
                            row_scope_groups: None,
                        }]),
                        context_conditions: None,
                    }]),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"));
    }

    /// A space holding an access profile an alert may assume: the domain, the space, the
    /// profile and the trust grant that records it.
    async fn space_with_assumable_profile(&self, case: &str, name: &str) -> (String, String) {
        let domain_id = self.domain(case, name).await;
        let space_id = self.space(case, &domain_id, name).await;
        let (profile_id, profile_arn) = self.access_profile(case, &space_id, name).await;
        self.trust_grant(case, &domain_id, &space_id, name, &profile_arn)
            .await;
        (space_id, profile_id)
    }

    async fn create_alert(
        &self,
        input: &CreateAlertInput,
    ) -> Result<CreateAlertOutput, CreateAlertError> {
        CreateAlert::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_alert(
        &self,
        space_id: &str,
        alert_id: &str,
    ) -> Result<GetAlertOutput, GetAlertError> {
        GetAlert::new(Arc::clone(&self.engine))
            .handle(
                GetAlertInput {
                    space_id: space_id.to_string(),
                    alert_id: alert_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

/// The smallest rule `CreateAlert` admits: a query to run, one threshold tier with the
/// mode and comparator to compare it, and an evaluation cadence.
fn complete_telemetry() -> TelemetryRule {
    TelemetryRule {
        query: Some(AlertRuleQuery {
            language: "SQL".to_string(),
            expression: "SELECT count(*) FROM logs WHERE level = 'ERROR'".to_string(),
        }),
        condition: Some(AlertCondition {
            threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
            threshold_field: None,
            comparator: Some("GT".to_string()),
            warning_threshold: None,
            critical_threshold: Some(10.0),
        }),
        evaluation: Some(AlertEvaluation {
            interval_seconds: 60,
            pending_duration_seconds: None,
            recovery_duration_seconds: None,
        }),
        no_data: None,
    }
}

/// One notification rule: the transitions that trigger it and where it is sent.
fn notification_rule(target_type: &str, states: &[&str]) -> NotificationRule {
    NotificationRule {
        trigger: NotificationTrigger {
            state_values: Some(states.iter().map(|state| state.to_string()).collect()),
        },
        target: NotificationTarget {
            r#type: target_type.to_string(),
            arn: TOPIC_ARN.to_string(),
            metadata: None,
        },
    }
}

/// The smallest alert request the model admits, for the parents `space_id`/`profile_id`.
fn create_input(space_id: &str, profile_id: &str, name: &str) -> CreateAlertInput {
    CreateAlertInput {
        space_id: space_id.to_string(),
        profile_id: profile_id.to_string(),
        name: name.to_string(),
        description: None,
        rule: Rule::telemetryRule(complete_telemetry()),
        notifications_enabled: None,
        tags: None,
        notification_rules: None,
        client_token: None,
    }
}

/// The state an alert reports when nothing it could evaluate produced data: the rule's
/// own `noData.treatAs`, or `NODATA` when the rule configures none (`NoData`'s model
/// doc: "defaults to `NODATA` when omitted").
fn expected_no_data_state(telemetry: &TelemetryRule) -> String {
    telemetry
        .no_data
        .as_ref()
        .map(|no_data| no_data.treat_as.clone())
        .unwrap_or_else(|| ALERT_STATE_NODATA.to_string())
}

/// One row of the `CreateAlert` table: the request members that vary.
struct AlertCase {
    case: &'static str,
    name: String,
    description: Option<String>,
    telemetry: TelemetryRule,
    notifications_enabled: Option<bool>,
    notification_rules: Option<Vec<NotificationRule>>,
    tags: Option<HashMap<String, String>>,
}

/// The rows span what the rules generalise over: both `QueryLanguage` values, both
/// `ThresholdMode` values, several `Comparator` values, a warning tier alone, a critical
/// tier alone and both together, evaluation intervals at both published bounds with
/// pending/recovery durations absent, zero and a multiple, `noData` absent and set,
/// `notificationsEnabled` absent/true/false, `notificationRules` absent, one and the
/// model's `@length(max: 5)`, `name` over its whole `^[a-zA-Z0-9_.@~()-]+$` class, a
/// `description` absent and at `@length(max: 1024)`, and `tags` absent, one and at
/// `@length(max: 50)`.
fn alert_cases() -> Vec<AlertCase> {
    vec![
        AlertCase {
            case: "the smallest rule: one critical tier, no notification rules",
            name: "a".to_string(),
            description: None,
            telemetry: complete_telemetry(),
            notifications_enabled: None,
            notification_rules: None,
            tags: None,
        },
        AlertCase {
            case: "PROMQL, a field-value warning tier at the lowest interval, notifications off",
            name: "Checkout_latency.p99@~(prod)-alert".to_string(),
            description: Some("an optional description".to_string()),
            telemetry: TelemetryRule {
                query: Some(AlertRuleQuery {
                    language: "PROMQL".to_string(),
                    expression: "histogram_quantile(0.99, latency_seconds)".to_string(),
                }),
                condition: Some(AlertCondition {
                    threshold_mode: Some("FIELD_VALUE".to_string()),
                    threshold_field: Some("latency_seconds".to_string()),
                    comparator: Some("GTE".to_string()),
                    warning_threshold: Some(0.5),
                    critical_threshold: None,
                }),
                evaluation: Some(AlertEvaluation {
                    interval_seconds: 30,
                    pending_duration_seconds: Some(0),
                    recovery_duration_seconds: Some(60),
                }),
                no_data: Some(NoData {
                    treat_as: "OK".to_string(),
                }),
            },
            notifications_enabled: Some(false),
            notification_rules: Some(vec![notification_rule("sns", &["CRITICAL"])]),
            tags: None,
        },
        AlertCase {
            case: "both tiers at the highest interval, five notification rules, 50 tags",
            name: "both-tiers".to_string(),
            description: Some("d".repeat(1024)),
            telemetry: TelemetryRule {
                condition: Some(AlertCondition {
                    threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
                    threshold_field: None,
                    comparator: Some("LT".to_string()),
                    warning_threshold: Some(5.0),
                    critical_threshold: Some(1.0),
                }),
                evaluation: Some(AlertEvaluation {
                    interval_seconds: 3600,
                    pending_duration_seconds: Some(7200),
                    recovery_duration_seconds: Some(0),
                }),
                no_data: Some(NoData {
                    treat_as: "CRITICAL".to_string(),
                }),
                ..complete_telemetry()
            },
            notifications_enabled: Some(true),
            notification_rules: Some(
                NOTIFICATION_TARGET_TYPE_VALUES
                    .iter()
                    .cycle()
                    .take(5)
                    .map(|target_type| notification_rule(target_type, &["WARNING", "CRITICAL"]))
                    .collect(),
            ),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key-{index}"), format!("value-{index}")))
                    .collect(),
            ),
        },
        AlertCase {
            case: "a rule that triggers on every state, one tag, notifications defaulted",
            name: "every.state~alert".to_string(),
            description: None,
            telemetry: TelemetryRule {
                condition: Some(AlertCondition {
                    threshold_mode: Some("FIELD_VALUE".to_string()),
                    threshold_field: Some("errors".to_string()),
                    comparator: Some("LTE".to_string()),
                    warning_threshold: Some(-1.5),
                    critical_threshold: Some(0.0),
                }),
                evaluation: Some(AlertEvaluation {
                    interval_seconds: 900,
                    pending_duration_seconds: Some(1800),
                    recovery_duration_seconds: None,
                }),
                no_data: Some(NoData {
                    treat_as: "WARNING".to_string(),
                }),
                ..complete_telemetry()
            },
            notifications_enabled: None,
            notification_rules: Some(vec![notification_rule("pagerduty", ALERT_STATE_VALUES)]),
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        },
    ]
}

#[tokio::test]
async fn an_alert_is_persisted_on_create_and_read_back_by_get_alert() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("alert round trip", "alerts")
        .await;

    let mut seen_ids: Vec<String> = Vec::new();
    for case in alert_cases() {
        let label = case.case;
        let input = CreateAlertInput {
            name: case.name.clone(),
            description: case.description.clone(),
            rule: Rule::telemetryRule(case.telemetry.clone()),
            notifications_enabled: case.notifications_enabled,
            notification_rules: case.notification_rules.clone(),
            tags: case.tags.clone(),
            ..create_input(&space_id, &profile_id, &case.name)
        };
        let created = omni
            .create_alert(&input)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: CreateAlert failed: {error:?}"))
            ;

        let alert = created.alert;
        let alert_id = alert
            .alert_id
            .clone()
            .unwrap_or_else(|| panic!("case {label:?}: the created alert carries no alertId"));
        assert!(
            is_alert_id(&alert_id),
            "case {label:?}: expected an alertId matching ^[0-9a-f]{{32}}$, got {alert_id:?}"
        );
        assert!(
            !seen_ids.contains(&alert_id),
            "case {label:?}: alertId {alert_id:?} was already minted for another alert"
        );
        seen_ids.push(alert_id.clone());

        // The ARN is derived: this service's namespace, the signed region, the
        // emulator's account and the minted id.
        let expected_arn_tail = format!(":{REGION}:{}:alert/{alert_id}", alert.account_id);
        assert!(
            is_arn(&alert.alert_arn) && alert.alert_arn.ends_with(&expected_arn_tail),
            "case {label:?}: expected an ARN ending {expected_arn_tail:?}, got {:?}",
            alert.alert_arn
        );
        assert_eq!(
            created.alert_arn.as_deref(),
            Some(alert.alert_arn.as_str()),
            "case {label:?}: the deprecated alertArn must carry alert.alertArn, got {:?}",
            created.alert_arn
        );
        assert!(
            is_account_id(&alert.account_id),
            "case {label:?}: expected a 12-digit accountId, got {:?}",
            alert.account_id
        );
        assert_eq!(
            (alert.space_id.as_deref(), alert.profile_id.as_deref()),
            (Some(space_id.as_str()), Some(profile_id.as_str())),
            "case {label:?}: the alert must report the parents it was created under, got {:?}",
            (&alert.space_id, &alert.profile_id)
        );
        assert_eq!(
            alert.name, case.name,
            "case {label:?}: expected name {:?}, got {:?}",
            case.name, alert.name
        );
        assert_eq!(
            alert.description, case.description,
            "case {label:?}: expected description {:?}, got {:?}",
            case.description, alert.description
        );
        // `notificationsEnabled` "Defaults to true when omitted", and
        // `notificationStatus` is what reports it back.
        let expected_status = if case.notifications_enabled.unwrap_or(true) {
            NOTIFICATION_STATUS_ENABLED
        } else {
            NOTIFICATION_STATUS_DISABLED
        };
        assert_eq!(
            alert.notification_status.as_deref(),
            Some(expected_status),
            "case {label:?}: expected notificationStatus {expected_status}, got {:?}",
            alert.notification_status
        );
        assert_eq!(
            format!("{:?}", alert.notification_rules),
            format!("{:?}", case.notification_rules),
            "case {label:?}: the notification rules must round-trip verbatim, got {:?}",
            alert.notification_rules
        );
        assert_eq!(
            format!("{:?}", alert.rule),
            format!("{:?}", Rule::telemetryRule(case.telemetry.clone())),
            "case {label:?}: the rule must round-trip verbatim, got {:?}",
            alert.rule
        );
        // `Alert.state` is "Absent on CreateAlert: a newly created alert has never been
        // evaluated".
        assert!(
            alert.state.is_none(),
            "case {label:?}: CreateAlert must report no state, got {:?}",
            alert.state
        );
        assert_eq!(
            alert.updated_at.secs(),
            alert.created_at.secs(),
            "case {label:?}: a created alert's updatedAt must equal its createdAt, got {:?}",
            (alert.created_at, alert.updated_at)
        );
        assert!(
            alert.created_at.secs() > 0,
            "case {label:?}: expected a real creation timestamp, got {:?}",
            alert.created_at
        );

        // The reader answers from the same store, and computes the state the record
        // does not carry.
        let read = omni
            .get_alert(&space_id, &alert_id)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: GetAlert failed: {error:?}"))
            .alert;
        assert_eq!(
            comparable(&read),
            comparable(&alert),
            "case {label:?}: GetAlert must answer with the persisted alert, got {read:?}"
        );
        let state = read
            .state
            .unwrap_or_else(|| panic!("case {label:?}: GetAlert reported no state"));
        let expected_state = expected_no_data_state(&case.telemetry);
        assert_eq!(
            state.value, expected_state,
            "case {label:?}: with no telemetry to evaluate, expected state {expected_state}, \
             got {:?}",
            state.value
        );
        assert_eq!(
            state.transitioned_at.map(|at| at.secs()),
            Some(alert.updated_at.secs()),
            "case {label:?}: expected the state to have been entered at the alert's updatedAt, \
             got {:?}",
            state.transitioned_at
        );
        // `contributorSummary` is "absent until the first contributor breaches a WARNING
        // or CRITICAL threshold", and `data` carries the threshold a breach crossed.
        assert!(
            state.contributor_summary.is_none() && state.data.is_none(),
            "case {label:?}: nothing has breached a threshold, so no contributor summary or \
             state data is expected, got {:?}",
            (state.contributor_summary, state.data)
        );
    }
}

/// Every member an alert reports but `state`, which only the reader computes.
fn comparable(alert: &Alert) -> String {
    format!(
        "{:?}",
        Alert {
            state: None,
            ..alert.clone()
        }
    )
}

#[tokio::test]
async fn an_alert_needs_an_access_profile_a_trust_grant_authorizes_assuming() {
    let omni = Omni::new();
    let case = "trust";
    let domain_id = omni.domain(case, "alert-trust").await;
    let space_id = omni.space(case, &domain_id, "alert-trust").await;
    let (profile_id, profile_arn) = omni.access_profile(case, &space_id, "untrusted").await;

    // A second space whose profile IS assumable, to show the refusal is about the
    // profile of THIS request's space rather than about any profile anywhere.
    let (other_space, other_profile) = omni
        .space_with_assumable_profile(case, "alert-trust-elsewhere")
        .await;

    for (label, space, profile, status, code) in [
        (
            "a profile no grant names",
            space_id.as_str(),
            profile_id.as_str(),
            400,
            "ValidationException",
        ),
        (
            "an assumable profile of ANOTHER space",
            space_id.as_str(),
            other_profile.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a profile that names nothing",
            space_id.as_str(),
            MISSING_PROFILE_ID,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a space that names nothing",
            MISSING_SPACE_ID,
            profile_id.as_str(),
            404,
            "ResourceNotFoundException",
        ),
    ] {
        let result = omni
            .create_alert(&create_input(space, profile, "refused"))
            .await;
        expect_modelled_error(label, result, status, code);
    }

    // The SAME request succeeds once a grant records the trust: the refusal is read
    // from the store, not from the shape of the request.
    omni.trust_grant(case, &domain_id, &space_id, "alert-trust", &profile_arn)
        .await;
    let created = omni
        .create_alert(&create_input(&space_id, &profile_id, "now-allowed"))
        .await
        .expect("case \"a profile a grant now names\": the alert is created")
        .alert;
    assert_eq!(
        created.profile_id.as_deref(),
        Some(profile_id.as_str()),
        "case \"a profile a grant now names\": expected the alert to use the granted profile, \
         got {:?}",
        created.profile_id
    );

    // An alert of the other space is reachable only under ITS space.
    let elsewhere = omni
        .create_alert(&create_input(&other_space, &other_profile, "elsewhere"))
        .await
        .expect("the alert of the other space is created")
        .alert
        .alert_id
        .expect("the created alert carries an alertId");
    for (label, space, alert_id) in [
        ("an unknown alert id", space_id.as_str(), MISSING_ALERT_ID),
        (
            "a known alert id under another space",
            space_id.as_str(),
            elsewhere.as_str(),
        ),
    ] {
        let result = omni.get_alert(space, alert_id).await;
        expect_modelled_error(label, result, 404, "ResourceNotFoundException");
    }
}

/// One row of the refusal table: a rule or notification rule the documented validator
/// rejects.
struct RejectedCase {
    case: &'static str,
    telemetry: TelemetryRule,
    notification_rules: Option<Vec<NotificationRule>>,
}

/// The rows span the documented rules, each row differing in the member it breaks:
/// `Rule`'s "On CreateAlert, presence is enforced by the service-side validator";
/// `AlertCondition`'s "A condition must keep at least one tier" and "`thresholdMode` and
/// `comparator` ... are required whenever a threshold is present"; `AlertEvaluation`'s
/// interval catalog and "must be multiples of `intervalSeconds`"; and the enum value
/// lists `QueryLanguage`, `ThresholdMode`, `Comparator`, `AlertState` and
/// `NotificationTargetType` declare.
fn rejected_cases() -> Vec<RejectedCase> {
    /// One row whose rule is the complete one with a single member broken.
    fn broken(case: &'static str, telemetry: TelemetryRule) -> RejectedCase {
        RejectedCase {
            case,
            telemetry,
            notification_rules: None,
        }
    }
    /// The complete rule with its condition replaced.
    fn with_condition(condition: AlertCondition) -> TelemetryRule {
        TelemetryRule {
            condition: Some(condition),
            ..complete_telemetry()
        }
    }
    /// The complete rule with its evaluation replaced.
    fn with_evaluation(
        interval_seconds: i32,
        pending_duration_seconds: Option<i32>,
        recovery_duration_seconds: Option<i32>,
    ) -> TelemetryRule {
        TelemetryRule {
            evaluation: Some(AlertEvaluation {
                interval_seconds,
                pending_duration_seconds,
                recovery_duration_seconds,
            }),
            ..complete_telemetry()
        }
    }
    vec![
        broken(
            "no query",
            TelemetryRule {
                query: None,
                ..complete_telemetry()
            },
        ),
        broken(
            "no condition",
            TelemetryRule {
                condition: None,
                ..complete_telemetry()
            },
        ),
        broken(
            "no evaluation",
            TelemetryRule {
                evaluation: None,
                ..complete_telemetry()
            },
        ),
        broken(
            "a query language the model does not declare",
            TelemetryRule {
                query: Some(AlertRuleQuery {
                    language: "LOGQL".to_string(),
                    expression: "count_over_time({job=\"omni\"}[5m])".to_string(),
                }),
                ..complete_telemetry()
            },
        ),
        broken(
            "a condition with neither tier",
            with_condition(AlertCondition {
                threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
                threshold_field: None,
                comparator: Some("GT".to_string()),
                warning_threshold: None,
                critical_threshold: None,
            }),
        ),
        broken(
            "a threshold with no thresholdMode",
            with_condition(AlertCondition {
                threshold_mode: None,
                threshold_field: None,
                comparator: Some("GT".to_string()),
                warning_threshold: Some(1.0),
                critical_threshold: None,
            }),
        ),
        broken(
            "a threshold with no comparator",
            with_condition(AlertCondition {
                threshold_mode: Some("FIELD_VALUE".to_string()),
                threshold_field: Some("errors".to_string()),
                comparator: None,
                warning_threshold: None,
                critical_threshold: Some(2.0),
            }),
        ),
        broken(
            "a thresholdMode the model does not declare",
            with_condition(AlertCondition {
                threshold_mode: Some("PERCENTILE".to_string()),
                threshold_field: None,
                comparator: Some("GT".to_string()),
                warning_threshold: Some(1.0),
                critical_threshold: None,
            }),
        ),
        broken(
            "a comparator the model does not declare",
            with_condition(AlertCondition {
                threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
                threshold_field: None,
                comparator: Some("NEQ".to_string()),
                warning_threshold: Some(1.0),
                critical_threshold: None,
            }),
        ),
        broken(
            "a pending duration that is not a multiple of the interval",
            with_evaluation(60, Some(90), None),
        ),
        broken(
            "a negative recovery duration",
            with_evaluation(60, None, Some(-60)),
        ),
        broken(
            "a noData state the model does not declare",
            TelemetryRule {
                no_data: Some(NoData {
                    treat_as: "UNKNOWN".to_string(),
                }),
                ..complete_telemetry()
            },
        ),
        RejectedCase {
            case: "a notification target type the model does not declare",
            telemetry: complete_telemetry(),
            notification_rules: Some(vec![notification_rule("email", &["CRITICAL"])]),
        },
        RejectedCase {
            case: "a notification trigger state the model does not declare",
            telemetry: complete_telemetry(),
            notification_rules: Some(vec![notification_rule("sns", &["FIRING"])]),
        },
    ]
}

#[tokio::test]
async fn a_rule_the_documented_validator_rejects_is_refused_and_persists_nothing() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("rejected rules", "alert-rules")
        .await;

    for case in rejected_cases() {
        let label = case.case;
        let input = CreateAlertInput {
            rule: Rule::telemetryRule(case.telemetry.clone()),
            notification_rules: case.notification_rules.clone(),
            // The token is what proves nothing was persisted: a token the refused
            // request carried cannot replay into an alert.
            client_token: Some(format!("token-{label}")),
            ..create_input(&space_id, &profile_id, "refused")
        };
        let result = omni.create_alert(&input).await;
        expect_modelled_error(label, result, 400, "ValidationException");

        let accepted = omni
            .create_alert(&CreateAlertInput {
                client_token: Some(format!("token-{label}")),
                ..create_input(&space_id, &profile_id, "after-refusal")
            })
            .await
            .unwrap_or_else(|error| {
                panic!("case {label:?}: the valid retry on the same token failed: {error:?}")
            })
            .alert;
        assert_eq!(
            accepted.name, "after-refusal",
            "case {label:?}: the refused request must have persisted nothing under its token, \
             got {:?}",
            accepted.name
        );
    }
}

#[tokio::test]
async fn every_published_evaluation_interval_is_accepted_and_nothing_else_is() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("intervals", "alert-intervals")
        .await;

    // The catalog `AlertEvaluation`'s model doc publishes, each with a pending duration
    // at a multiple of itself.
    for interval in ALERT_EVALUATION_INTERVALS {
        let label = format!("interval {interval}");
        let input = CreateAlertInput {
            rule: Rule::telemetryRule(TelemetryRule {
                evaluation: Some(AlertEvaluation {
                    interval_seconds: *interval,
                    pending_duration_seconds: Some(interval * 2),
                    recovery_duration_seconds: Some(0),
                }),
                ..complete_telemetry()
            }),
            ..create_input(&space_id, &profile_id, "interval-case")
        };
        omni.create_alert(&input)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: CreateAlert failed: {error:?}"));
    }

    // One below the lowest published interval, one above the highest, and values
    // between published entries.
    for interval in [0, 29, 31, 45, 1200, 3601, 7200, -60] {
        let label = format!("unsupported interval {interval}");
        let input = CreateAlertInput {
            rule: Rule::telemetryRule(TelemetryRule {
                evaluation: Some(AlertEvaluation {
                    interval_seconds: interval,
                    pending_duration_seconds: None,
                    recovery_duration_seconds: None,
                }),
                ..complete_telemetry()
            }),
            ..create_input(&space_id, &profile_id, "interval-case")
        };
        let result = omni.create_alert(&input).await;
        expect_modelled_error(&label, result, 400, "ValidationException");
    }
}

#[tokio::test]
async fn every_published_enum_value_a_rule_carries_is_accepted() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("enums", "alert-enums")
        .await;

    // The value lists come from the model's own enums
    // (`smithy_query(query="shape",shape="QueryLanguage,ThresholdMode,Comparator,AlertState,NotificationTargetType")`).
    for language in QUERY_LANGUAGE_VALUES {
        for mode in THRESHOLD_MODE_VALUES {
            for comparator in COMPARATOR_VALUES {
                let label = format!("{language}/{mode}/{comparator}");
                let input = CreateAlertInput {
                    rule: Rule::telemetryRule(TelemetryRule {
                        query: Some(AlertRuleQuery {
                            language: (*language).to_string(),
                            expression: "SELECT 1".to_string(),
                        }),
                        condition: Some(AlertCondition {
                            threshold_mode: Some((*mode).to_string()),
                            threshold_field: Some("errors".to_string()),
                            comparator: Some((*comparator).to_string()),
                            warning_threshold: Some(1.0),
                            critical_threshold: Some(2.0),
                        }),
                        ..complete_telemetry()
                    }),
                    ..create_input(&space_id, &profile_id, "enum-case")
                };
                omni.create_alert(&input)
                    .await
                    .unwrap_or_else(|error| panic!("case {label:?}: CreateAlert failed: {error:?}"));
            }
        }
    }

    for treat_as in ALERT_STATE_VALUES {
        let label = format!("noData.treatAs {treat_as}");
        let created = omni
            .create_alert(&CreateAlertInput {
                rule: Rule::telemetryRule(TelemetryRule {
                    no_data: Some(NoData {
                        treat_as: (*treat_as).to_string(),
                    }),
                    ..complete_telemetry()
                }),
                ..create_input(&space_id, &profile_id, "no-data-case")
            })
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: CreateAlert failed: {error:?}"))
            .alert
            .alert_id
            .unwrap_or_else(|| panic!("case {label:?}: the created alert carries no alertId"));
        let state = omni
            .get_alert(&space_id, &created)
            .await
            .unwrap_or_else(|error| panic!("case {label:?}: GetAlert failed: {error:?}"))
            .alert
            .state
            .unwrap_or_else(|| panic!("case {label:?}: GetAlert reported no state"));
        assert_eq!(
            &state.value, treat_as,
            "case {label:?}: an evaluation with no data reports the rule's treatAs, got {:?}",
            state.value
        );
    }

    for target_type in NOTIFICATION_TARGET_TYPE_VALUES {
        let label = format!("target type {target_type}");
        omni.create_alert(&CreateAlertInput {
            notification_rules: Some(vec![notification_rule(target_type, &["WARNING"])]),
            ..create_input(&space_id, &profile_id, "target-case")
        })
        .await
        .unwrap_or_else(|error| panic!("case {label:?}: CreateAlert failed: {error:?}"));
    }
}

#[tokio::test]
async fn a_name_is_not_an_alert_identity_and_a_token_replay_returns_the_original() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("identity", "alert-identity")
        .await;

    // `name`'s model doc: "Not the alert's identity: the backend mints a separate uuid
    // as the AlertId, so the name need not be unique within a space".
    let first = omni
        .create_alert(&create_input(&space_id, &profile_id, "same-name"))
        .await
        .expect("the first alert is created")
        .alert;
    let second = omni
        .create_alert(&create_input(&space_id, &profile_id, "same-name"))
        .await
        .expect("a second alert of the same name is created")
        .alert;
    assert_ne!(
        first.alert_id, second.alert_id,
        "two alerts sharing a name must still have distinct ids, got {:?}",
        first.alert_id
    );

    // @idempotencyToken: the token alone decides, so a replay returns the original
    // alert rather than minting a second one.
    let original = omni
        .create_alert(&CreateAlertInput {
            client_token: Some("replay-me".to_string()),
            ..create_input(&space_id, &profile_id, "original")
        })
        .await
        .expect("the token's first request is created")
        .alert;
    let replayed = omni
        .create_alert(&CreateAlertInput {
            client_token: Some("replay-me".to_string()),
            description: Some("a different body under the same token".to_string()),
            ..create_input(&space_id, &profile_id, "renamed")
        })
        .await
        .expect("the replay is answered")
        .alert;
    assert_eq!(
        comparable(&replayed),
        comparable(&original),
        "a token replay must return the original alert, got {replayed:?}"
    );
    let fresh = omni
        .create_alert(&CreateAlertInput {
            client_token: Some("another-token".to_string()),
            ..create_input(&space_id, &profile_id, "fresh")
        })
        .await
        .expect("another token creates another alert")
        .alert;
    assert_ne!(
        fresh.alert_id, original.alert_id,
        "a different token must create a different alert, got {:?}",
        fresh.alert_id
    );
}

#[tokio::test]
async fn the_alert_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let (space_id, profile_id) = omni
        .space_with_assumable_profile("wire", "alert-wire")
        .await;

    let (status, body) = omni
        .over_the_wire(
            "CreateAlert",
            CREATE_ALERT_INPUT_SCHEMA,
            &create_input(&space_id, &profile_id, "over-the-wire"),
        )
        .await;
    assert_eq!(status, 200, "CreateAlert over the wire: expected 200");
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let created = CreateAlertOutput::deserialize(&mut deserializer)
        .expect("the CreateAlert response deserializes");
    let alert_id = created
        .alert
        .alert_id
        .clone()
        .expect("the created alert carries an alertId");

    let (status, body) = omni
        .over_the_wire(
            "GetAlert",
            GET_ALERT_INPUT_SCHEMA,
            &GetAlertInput {
                space_id: space_id.clone(),
                alert_id: alert_id.clone(),
            },
        )
        .await;
    assert_eq!(status, 200, "GetAlert over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let read = GetAlertOutput::deserialize(&mut deserializer)
        .expect("the GetAlert response deserializes");
    assert_eq!(
        comparable(&read.alert),
        comparable(&created.alert),
        "GetAlert over the wire must answer with the persisted alert, got {:?}",
        read.alert
    );

    // The refusal travels as the modelled 400 over the same wire.
    let (profile_without_trust, _) = omni
        .access_profile("wire", &space_id, "untrusted-over-the-wire")
        .await;
    let (status, _) = omni
        .over_the_wire(
            "CreateAlert",
            CREATE_ALERT_INPUT_SCHEMA,
            &create_input(&space_id, &profile_without_trust, "refused-over-the-wire"),
        )
        .await;
    assert_eq!(
        status, 400,
        "an alert naming a profile no grant authorizes assuming must be refused with 400"
    );
}

#[test]
fn the_alert_id_pattern_is_checked_as_written() {
    for (case, value, expected) in [
        ("32 lowercase hex digits", "0123456789abcdef0123456789abcdef", true),
        ("31 digits", "0123456789abcdef0123456789abcde", false),
        ("33 digits", "0123456789abcdef0123456789abcdef0", false),
        ("uppercase hex", "0123456789ABCDEF0123456789ABCDEF", false),
        ("a hyphenated uuid", "01234567-89ab-cdef-0123-456789abcdef", false),
        ("a non-hex letter", "0123456789abcdeg0123456789abcdef", false),
    ] {
        assert_eq!(
            is_alert_id(value),
            expected,
            "case {case:?}: is_alert_id({value:?}) must be {expected}"
        );
    }
}
