// SPDX-License-Identifier: CC0-1.0

//! The record a state-changing Connect-instance call leaves behind, end to end over
//! the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rule under test:
//!
//! * a call that is asked to change a Connect instance leaves EXACTLY ONE structured
//!   record naming the operation, the access key id the request presented, the
//!   instance the request named and how the call ended — the accepted outcome when the
//!   instance changed, and the name of the modelled error otherwise — so an attempt is
//!   as visible when it was refused as when it succeeded, whichever operation was
//!   called;
//! * repeating a refused attempt leaves one record per attempt;
//! * a reporting call leaves none, and so does a request the modelled constraints
//!   refuse before any handler runs;
//! * no record carries what the instance holds — a key or resource ARN, a limits or
//!   integration config — only wire metadata and the instance id the request named.

use serde_json::{json, Value};

use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;

use super::common::{call, error_code, router, Harness};

/// The region every case here signs for; nothing in an audit record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// The access key id [`super::common::authorization`] presents, which is what the
/// record names as the caller.
const CALLER_AKID: &str = "AKIDEXAMPLE";

/// How the instance under a row has to be set up before the row's request runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// Nothing at all: the service holds no record of the instance, and neither does
    /// the account's Connect instance registry.
    Fresh,
    /// The account's Connect registry holds the instance, so there is an instance to
    /// onboard, and this service holds nothing for it yet.
    Registered,
    /// `StartInstanceOnboardingJob` has been called and the clock has NOT yet
    /// finished the job, so the instance is at the transient status.
    Onboarding,
    /// The onboarding job has finished, so the instance has a configuration.
    Onboarded,
}

/// One row: the request to drive against one instance, and the single audit record it
/// is expected to leave (`None` for a reporting call, which leaves none).
struct Row {
    case: &'static str,
    instance: &'static str,
    setup: Setup,
    method: &'static str,
    /// Appended to `/v2/connect-instance/{instance}` — the instance-scoped routes.
    path: &'static str,
    body: Value,
    status: u16,
    /// The modelled error code the response is expected to carry, if any.
    error: Option<&'static str>,
    /// `(operation, outcome)` of the record the call must leave.
    record: Option<(&'static str, &'static str)>,
}

/// An instance-wide communication-limits config, as `PutInstanceCommunicationLimits`
/// takes it.
fn limits_body() -> Value {
    json!({"communicationLimitsConfig": {"allChannelSubtypes": {
        "communicationLimitsList": [
            {"maxCountPerRecipient": 3, "frequency": 1, "unit": "DAY"},
        ],
    }}})
}

/// A Lambda integration config, as `PutConnectInstanceIntegration` takes it. The ARN
/// it carries is what no audit record may repeat.
fn integration_body() -> Value {
    json!({"integrationConfig": {"lambda": {
        "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:audited-handler",
    }}})
}

/// The `IntegrationIdentifier` naming the integration [`integration_body`]
/// configures, as `DeleteConnectInstanceIntegration` takes it.
fn integration_identifier() -> Value {
    json!({"integrationIdentifier": {"lambda": {
        "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:audited-handler",
    }}})
}

/// The table: one row per (operation, outcome) pair this service can answer an
/// instance-scoped call with, plus the reporting calls that must leave nothing.
///
/// The rows differ along the operation called and along the outcome it ends in —
/// accepted, and each modelled refusal that operation can answer — which is what the
/// rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "onboarding accepted",
            instance: "audit-onboard-ok",
            setup: Setup::Registered,
            method: "PUT",
            path: "/onboarding",
            body: json!({"encryptionConfig": {"enabled": false}}),
            status: 200,
            error: None,
            record: Some(("StartInstanceOnboardingJob", "accepted")),
        },
        Row {
            case: "onboarding refused: the account holds no such Connect instance",
            instance: "audit-onboard-unknown",
            setup: Setup::Fresh,
            method: "PUT",
            path: "/onboarding",
            body: json!({"encryptionConfig": {"enabled": false}}),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some(("StartInstanceOnboardingJob", "ResourceNotFoundException")),
        },
        Row {
            case: "onboarding refused: encryption enabled with no key",
            instance: "audit-onboard-no-key",
            setup: Setup::Fresh,
            method: "PUT",
            path: "/onboarding",
            body: json!({"encryptionConfig": {"enabled": true}}),
            status: 400,
            error: Some("ValidationException"),
            record: Some(("StartInstanceOnboardingJob", "ValidationException")),
        },
        Row {
            case: "instance limits accepted",
            instance: "audit-limits-ok",
            setup: Setup::Onboarded,
            method: "PUT",
            path: "/communication-limits",
            body: limits_body(),
            status: 200,
            error: None,
            record: Some(("PutInstanceCommunicationLimits", "accepted")),
        },
        Row {
            case: "instance limits refused: no configuration",
            instance: "audit-limits-missing",
            setup: Setup::Fresh,
            method: "PUT",
            path: "/communication-limits",
            body: limits_body(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some((
                "PutInstanceCommunicationLimits",
                "ResourceNotFoundException",
            )),
        },
        Row {
            case: "integration accepted",
            instance: "audit-integration-ok",
            setup: Setup::Onboarded,
            method: "PUT",
            path: "/integrations",
            body: integration_body(),
            status: 200,
            error: None,
            record: Some(("PutConnectInstanceIntegration", "accepted")),
        },
        Row {
            case: "integration refused: instance not onboarded",
            instance: "audit-integration-missing",
            setup: Setup::Fresh,
            method: "PUT",
            path: "/integrations",
            body: integration_body(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some(("PutConnectInstanceIntegration", "ResourceNotFoundException")),
        },
        Row {
            case: "integration delete refused: nothing under that identifier",
            instance: "audit-integration-delete",
            setup: Setup::Onboarded,
            method: "POST",
            path: "/integrations/delete",
            body: integration_identifier(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some((
                "DeleteConnectInstanceIntegration",
                "ResourceNotFoundException",
            )),
        },
        Row {
            case: "config delete accepted",
            instance: "audit-config-ok",
            setup: Setup::Onboarded,
            method: "DELETE",
            path: "/config",
            body: Value::Null,
            status: 200,
            error: None,
            record: Some(("DeleteConnectInstanceConfig", "accepted")),
        },
        Row {
            case: "config delete refused: no configuration",
            instance: "audit-config-missing",
            setup: Setup::Fresh,
            method: "DELETE",
            path: "/config",
            body: Value::Null,
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some(("DeleteConnectInstanceConfig", "ResourceNotFoundException")),
        },
        Row {
            case: "config delete refused: onboarding not finished",
            instance: "audit-config-onboarding",
            setup: Setup::Onboarding,
            method: "DELETE",
            path: "/config",
            body: Value::Null,
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some(("DeleteConnectInstanceConfig", "ResourceNotFoundException")),
        },
        // `campaignDeletionPolicy` targets a modelled enum, so an off-catalog value
        // is refused by the boundary validator BEFORE the handler runs: the request
        // never reaches an instance mutation, which is why it leaves no record of one.
        Row {
            case: "config delete refused at the boundary: off-catalog campaign deletion policy",
            instance: "audit-config-policy",
            setup: Setup::Onboarded,
            method: "DELETE",
            path: "/config?campaignDeletionPolicy=NOPE",
            body: Value::Null,
            status: 400,
            error: Some("ValidationException"),
            record: None,
        },
        Row {
            case: "onboarding job delete refused: no job",
            instance: "audit-job-missing",
            setup: Setup::Fresh,
            method: "DELETE",
            path: "/onboarding",
            body: Value::Null,
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some(("DeleteInstanceOnboardingJob", "ResourceNotFoundException")),
        },
        Row {
            case: "onboarding job delete refused: job in progress",
            instance: "audit-job-in-progress",
            setup: Setup::Onboarding,
            method: "DELETE",
            path: "/onboarding",
            body: Value::Null,
            status: 409,
            error: Some("InvalidStateException"),
            record: Some(("DeleteInstanceOnboardingJob", "InvalidStateException")),
        },
        Row {
            case: "onboarding job delete refused: job succeeded",
            instance: "audit-job-succeeded",
            setup: Setup::Onboarded,
            method: "DELETE",
            path: "/onboarding",
            body: Value::Null,
            status: 409,
            error: Some("InvalidStateException"),
            record: Some(("DeleteInstanceOnboardingJob", "InvalidStateException")),
        },
        // The reporting side of the rule: a call that only reports leaves no record,
        // whether it finds the instance or not.
        Row {
            case: "config read",
            instance: "audit-read-config",
            setup: Setup::Onboarded,
            method: "GET",
            path: "/config",
            body: Value::Null,
            status: 200,
            error: None,
            record: None,
        },
        Row {
            case: "config read of an instance with no configuration",
            instance: "audit-read-missing",
            setup: Setup::Fresh,
            method: "GET",
            path: "/config",
            body: Value::Null,
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: None,
        },
        Row {
            case: "onboarding status read",
            instance: "audit-read-onboarding",
            setup: Setup::Onboarded,
            method: "GET",
            path: "/onboarding",
            body: Value::Null,
            status: 200,
            error: None,
            record: None,
        },
        Row {
            case: "instance limits read",
            instance: "audit-read-limits",
            setup: Setup::Onboarded,
            method: "GET",
            path: "/communication-limits",
            body: Value::Null,
            status: 200,
            error: None,
            record: None,
        },
        Row {
            case: "integration list",
            instance: "audit-read-integrations",
            setup: Setup::Onboarded,
            method: "GET",
            path: "/integrations",
            body: Value::Null,
            status: 200,
            error: None,
            record: None,
        },
    ]
}

/// `StartInstanceOnboardingJob` for `instance`, asserted to have started — on an
/// instance registered in the account's Connect registry first, since onboarding
/// resolves the `connectInstanceId` against it.
async fn start_onboarding(router: &Harness, instance: &str, case: &str) {
    router.register_connect_instance(REGION, instance);
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 starting onboarding for {instance}, got {status} {body}"
    );
}

/// Every state-changing instance call leaves one record naming how it ended, and
/// every reporting one leaves none.
#[tokio::test]
async fn every_instance_mutation_records_how_it_ended() {
    let captured = audit_capture();
    let router = router();
    let rows = rows();

    // The rows needing a finished onboarding job start theirs together, so the span
    // the clock owes is waited out once rather than once per row. Nothing in the
    // emulator sleeps for that transition — the record stores the due instant.
    for row in rows.iter().filter(|row| row.setup == Setup::Onboarded) {
        start_onboarding(&router, row.instance, row.case).await;
    }
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;

    for row in &rows {
        if row.setup == Setup::Registered {
            // The instance exists in the account's Connect registry and nowhere else:
            // this row's own request is what gives it an onboarding record.
            router.register_connect_instance(REGION, row.instance);
        }
        if row.setup == Setup::Onboarding {
            // Started and NOT waited out: the job is still at the transient status
            // when this row's request runs.
            start_onboarding(&router, row.instance, row.case).await;
        }

        // Whatever the setup wrote is already in the capture, so only the records this
        // row's own request adds are judged.
        let before = records_for(captured, row.instance).len();
        let (status, body) = call(
            &router,
            REGION,
            row.method,
            &format!("/v2/connect-instance/{}{}", row.instance, row.path),
            &row.body,
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body}",
            row.case, row.status
        );
        if let Some(expected) = row.error {
            assert!(
                error_code(&body).ends_with(expected),
                "case {:?}: expected {expected}, got {:?}",
                row.case,
                error_code(&body)
            );
        }

        let mut after = records_for(captured, row.instance);
        let added = after.split_off(before);
        let outcomes: Vec<(String, String)> = added
            .iter()
            .map(|record| (field(record, "operation"), field(record, "outcome")))
            .collect();
        let expected: Vec<(String, String)> = row
            .record
            .map(|(operation, outcome)| vec![(operation.to_string(), outcome.to_string())])
            .unwrap_or_default();
        assert_eq!(
            outcomes, expected,
            "case {:?}: expected the audit records {expected:?}, got {outcomes:?} ({added:?})",
            row.case
        );

        for record in &added {
            assert_eq!(
                field(record, "caller_akid"),
                CALLER_AKID,
                "case {:?}: expected the record to name the presented access key id, \
                 got {record:?}",
                row.case
            );
            assert!(
                !record.contains("arn:"),
                "case {:?}: expected the record to carry nothing of what the instance \
                 holds, got {record:?}",
                row.case
            );
        }
    }
}

/// A refusal is not a state change, so repeating one is refused the same way — and
/// each attempt leaves its own record, because what the trail records is the attempt
/// rather than a change.
///
/// The rows differ along the operation and the refusal it answers with.
#[tokio::test]
async fn a_repeated_refusal_is_recorded_once_per_attempt() {
    let captured = audit_capture();
    let router = router();

    // Onboarded, so the onboarding-job delete is refused for the status its job is at
    // rather than for there being no job.
    let instance = "audit-repeat-instance";
    start_onboarding(&router, instance, "repeated refusals").await;
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;

    for (case, method, path, status, operation, outcome) in [
        (
            "onboarding job delete of a job that has not failed",
            "DELETE",
            "/onboarding",
            409u16,
            "DeleteInstanceOnboardingJob",
            "InvalidStateException",
        ),
        (
            "config delete of an instance with no configuration",
            "DELETE",
            "/config",
            404u16,
            "DeleteConnectInstanceConfig",
            "ResourceNotFoundException",
        ),
    ] {
        // The configuration delete rows run against an instance that has none: a
        // dedicated id per case keeps the two rows' records apart.
        let target = if path == "/config" {
            format!("{instance}-no-config")
        } else {
            instance.to_string()
        };
        let before = records_for(captured, &target).len();
        for attempt in 1..=3 {
            let (got, body) = call(
                &router,
                REGION,
                method,
                &format!("/v2/connect-instance/{target}{path}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                got, status,
                "case {case:?} attempt {attempt}: expected HTTP {status}, got {got} {body}"
            );
        }

        let mut after = records_for(captured, &target);
        let added = after.split_off(before);
        let outcomes: Vec<(String, String)> = added
            .iter()
            .map(|record| (field(record, "operation"), field(record, "outcome")))
            .collect();
        let expected: Vec<(String, String)> = (0..3)
            .map(|_| (operation.to_string(), outcome.to_string()))
            .collect();
        assert_eq!(
            outcomes, expected,
            "case {case:?}: expected one record per attempt, got {outcomes:?} ({added:?})"
        );
    }
}

/// The audit records the capture holds for `instance`, in the order they were
/// emitted.
///
/// Matched on the instance-id field's whole value — an instance id carries no
/// whitespace — so a record of an instance whose id merely starts with the same
/// characters is not picked up.
fn records_for(captured: &AuditCapture, instance: &str) -> Vec<String> {
    captured
        .audit_records()
        .into_iter()
        .filter(|record| field(record, "connect_instance_id") == instance)
        .collect()
}

/// The value one field carries in a rendered record.
fn field(record: &str, name: &str) -> String {
    record
        .split_once(&format!("{name}="))
        .map(|(_, rest)| rest.split(' ').next().unwrap_or_default())
        .unwrap_or_default()
        .to_string()
}

/// A `tracing` subscriber that keeps every event it is handed, rendered field by
/// field, so a case can assert on the records the handlers emit. Written against
/// `tracing` itself — a dependency this crate already carries — rather than pulling
/// in a collector crate.
#[derive(Clone, Default)]
struct AuditCapture {
    rendered: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl AuditCapture {
    /// Every event captured so far that carries the audit marker.
    fn audit_records(&self) -> Vec<String> {
        self.rendered
            .lock()
            .expect("the capture mutex is never poisoned")
            .iter()
            .filter(|rendered| rendered.contains("audit=true"))
            .cloned()
            .collect()
    }
}

impl tracing::Subscriber for AuditCapture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl tracing::field::Visit for Fields<'_> {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                use std::fmt::Write as _;
                let _ = write!(self.0, "{}={value} ", field.name());
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                let _ = write!(self.0, "{}={value:?} ", field.name());
            }
        }

        let mut rendered = String::new();
        event.record(&mut Fields(&mut rendered));
        self.rendered
            .lock()
            .expect("the capture mutex is never poisoned")
            .push(rendered);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture, installed ONCE as the process-wide default subscriber.
///
/// It has to be global rather than thread-local: `tracing` caches each callsite's
/// interest GLOBALLY, computed from whatever subscriber the first thread to reach
/// that callsite happened to have, so with the harness running these suites in
/// parallel another case reaching an audit callsite first — with no subscriber
/// installed — caches it as "never" and a thread-local capture then sees nothing. One
/// global capture plus an interest-cache rebuild removes that ordering dependency;
/// the assertions above are scoped to the instance ids these cases name, so records
/// other cases produce into the same capture do not affect them.
fn audit_capture() -> &'static AuditCapture {
    static CAPTURE: std::sync::OnceLock<AuditCapture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = AuditCapture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("these tests install no other global tracing subscriber");
        // Recompute interest for callsites a parallel case may already have reached —
        // and cached as "never" — before this subscriber existed.
        tracing::callsite::rebuild_interest_cache();
        capture
    })
}
