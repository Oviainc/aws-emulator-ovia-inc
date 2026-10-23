// SPDX-License-Identifier: CC0-1.0

//! The engagement slice — `StopEngagement`, and the `DescribeEngagement` read that proves
//! what a stop persisted — end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1
//! request (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the
//! contact suite builds, so what is asserted is what a signing AWS client would see.
//!
//! Two rules are under test, and each suite is a TABLE whose rows differ along what the
//! rule generalises over rather than a single call:
//!
//! 1. an SSM Contacts data-plane call that resolves a resource is served only while the
//!    caller's account has an ACTIVE Incident Manager replication set; until it does the
//!    call is refused with the modelled `ValidationException` (HTTP 400) BEFORE the
//!    resource it names is looked up. The rows span the `SsmContactsArn` forms the model's
//!    own pattern admits (all three partitions, a Region-less ARN), engagements that ARE
//!    stored and engagements that never existed, the `Reason` forms `StopReason` admits,
//!    the three states the account precondition can be in, and both gated operations in
//!    the slice;
//! 2. a stop records WHEN the engagement stopped, so `DescribeEngagement` reports a
//!    `StopTime` from then on while every other member keeps the value the engagement was
//!    started with. The rows span the optional members an engagement may carry and the
//!    `Reason` the stop may carry, including both published `StopReason` length bounds and
//!    one character past the maximum.
//!
//! The onboarded path is reached through the contact suite's own account fixture
//! (`contact_updates::onboard`/`set_replication_set`), which writes the replication set the
//! handlers read: that is the only way to reach it, because Incident Manager's
//! `CreateReplicationSet` is deprecated and refuses every call. Engagements are seeded into
//! the same store rather than started through `StartEngagement`, so each row's stored
//! engagement is fixed and independent of the clock; `StartEngagement`'s own suite
//! (`start_engagement.rs`) asserts what a start persists.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    encode_next_token, service_state, AccountReplicationSet, EngagementRecord,
    TOKEN_SCOPE_CONTACTS,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{onboard, set_replication_set};

/// The instant every seeded engagement is stamped with, as whole seconds since the epoch.
/// Fixed, so nothing in this suite depends on the clock.
const SEED_SECS: i64 = 1_700_000_000;

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// Persist `rows` as engagements in the very store the handlers read.
fn seed_engagements(emulator: &Emulator, rows: &[EngagementRow]) {
    let store = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let mut store = store.lock();
    for row in rows {
        let record = row.record();
        store.engagements.insert(record.arn.clone(), record);
    }
}

/// The stored engagement `arn` names, read straight from the store.
fn stored(emulator: &Emulator, arn: &str) -> EngagementRecord {
    let store = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    store
        .engagements
        .get(arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds no engagement under {arn}"))
}

// ---------------------------------------------------------------------------
// The engagement catalog
// ---------------------------------------------------------------------------

/// How a row names `StopEngagement$Reason`.
///
/// `StopReason` is `len 0..255, pattern ^[.\s\S]*$` (model: `StopEngagement` input), so the
/// rows span the member absent, both published length bounds and one character past the
/// maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasonCase {
    /// The optional member is not sent at all.
    Absent,
    /// The published minimum length, 0.
    Empty,
    Text(&'static str),
    /// The published maximum length, 255.
    AtMaxLength,
    /// One character past the published maximum: refused by the modelled constraint.
    PastMaxLength,
}

impl ReasonCase {
    /// The value to send, or `None` when the member is absent.
    fn value(self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Empty => Some(String::new()),
            Self::Text(text) => Some(text.to_string()),
            Self::AtMaxLength => Some("r".repeat(255)),
            Self::PastMaxLength => Some("r".repeat(256)),
        }
    }
}

/// A row of the engagement catalog: one engagement as `StartEngagement` persists one, plus
/// the `Reason` the stop under test carries.
///
/// The rows differ along the members the modelled engagement admits — `PublicSubject` /
/// `PublicContent` present and absent, an `IncidentId` present and absent, an empty
/// `Sender` (its published minimum length is 0), a multi-line `Content` (the modelled
/// pattern `^[.\s\S]*$` admits newlines) — and along the partition of the engagement's
/// ARN, so nothing here is keyed to one recorded request's shape.
#[derive(Debug, Clone)]
struct EngagementRow {
    case: &'static str,
    /// The partition of the engagement's ARN. Source: `SsmContactsArn`'s pattern
    /// `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:...`.
    partition: &'static str,
    /// The Region segment of the ARN; `""` is admitted by that same pattern.
    region: &'static str,
    contact_alias: &'static str,
    id: &'static str,
    sender: &'static str,
    subject: &'static str,
    content: &'static str,
    public_subject: Option<&'static str>,
    public_content: Option<&'static str>,
    incident_id: Option<&'static str>,
    /// Seconds to add to [`SEED_SECS`] for this engagement's `StartTime`.
    start_offset_secs: u64,
    reason: ReasonCase,
}

impl EngagementRow {
    /// This engagement's ARN: `arn:<partition>:ssm-contacts:<region>:<account>:engagement/
    /// <contact alias>/<id>`, the form Incident Manager names an engagement with.
    fn arn(&self) -> String {
        format!(
            "arn:{}:ssm-contacts:{}:{ACCOUNT}:engagement/{}/{}",
            self.partition, self.region, self.contact_alias, self.id
        )
    }

    /// The `ContactArn` of the contact being engaged.
    fn contact_arn(&self) -> String {
        format!(
            "arn:{}:ssm-contacts:{}:{ACCOUNT}:contact/{}",
            self.partition, self.region, self.contact_alias
        )
    }

    /// The record `StartEngagement` persists for this row.
    fn record(&self) -> EngagementRecord {
        EngagementRecord {
            arn: self.arn(),
            contact_arn: self.contact_arn(),
            sender: self.sender.to_string(),
            subject: self.subject.to_string(),
            content: self.content.to_string(),
            public_subject: self.public_subject.map(str::to_string),
            public_content: self.public_content.map(str::to_string),
            incident_id: self.incident_id.map(str::to_string),
            start_time: SystemTime::UNIX_EPOCH
                + Duration::from_secs(SEED_SECS as u64 + self.start_offset_secs),
            stop_time: None,
            stop_reason: None,
        }
    }

    /// The `StopEngagement` request body for this row.
    fn stop_request(&self) -> Value {
        let mut request = json!({"EngagementId": self.arn()});
        if let Some(reason) = self.reason.value() {
            request["Reason"] = json!(reason);
        }
        request
    }
}

fn engagement_catalog() -> Vec<EngagementRow> {
    vec![
        EngagementRow {
            case: "a VOICE/EMAIL engagement with no public message and no incident, stopped \
                   with no Reason",
            partition: "aws",
            region: REGION,
            contact_alias: "oncall-primary",
            id: "5f1d3a7c-0000-4000-8000-000000000001",
            sender: "cli",
            subject: "Database latency",
            content: "p99 latency is above the SLO.",
            public_subject: None,
            public_content: None,
            incident_id: None,
            start_offset_secs: 0,
            reason: ReasonCase::Absent,
        },
        EngagementRow {
            case: "an engagement carrying both public members and an incident, stopped with \
                   a short Reason",
            partition: "aws",
            region: REGION,
            contact_alias: "escalation-tier-1",
            id: "5f1d3a7c-0000-4000-8000-000000000002",
            sender: "incident-manager",
            subject: "Checkout errors",
            content: "Checkout error rate 12%.",
            public_subject: Some("Checkout errors"),
            public_content: Some("Error rate 12%"),
            incident_id: Some("arn:aws:ssm-incidents::000000000000:incident-record/checkout/1"),
            start_offset_secs: 30,
            reason: ReasonCase::Text("resolved by the on-call engineer"),
        },
        EngagementRow {
            case: "an engagement with an empty Sender and multi-line Content, stopped with \
                   an empty Reason",
            partition: "aws-cn",
            region: "cn-north-1",
            contact_alias: "weekend-rotation",
            id: "5f1d3a7c-0000-4000-8000-000000000003",
            sender: "",
            subject: "Backlog growing",
            content: "line one\nline two\n",
            public_subject: Some("Backlog"),
            public_content: None,
            incident_id: None,
            start_offset_secs: 60,
            reason: ReasonCase::Empty,
        },
        EngagementRow {
            case: "a Region-less GovCloud ARN, stopped with a Reason at the published \
                   maximum length",
            partition: "aws-us-gov",
            region: "",
            contact_alias: "pager_backup_2",
            id: "5f1d3a7c-0000-4000-8000-000000000004",
            sender: "ops@example.com",
            subject: "Disk pressure",
            content: "Root volume at 93%.",
            public_subject: None,
            public_content: Some("Disk pressure"),
            incident_id: Some("manual-2026-10-22"),
            start_offset_secs: 90,
            reason: ReasonCase::AtMaxLength,
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading the wire
// ---------------------------------------------------------------------------

/// `DescribeEngagement` for `arn`, as the wire reports it.
async fn describe(emulator: &Emulator, arn: &str) -> (u16, Value) {
    emulator
        .call("DescribeEngagement", json!({"EngagementId": arn}))
        .await
}

/// Assert `body` is the account-gate refusal: the modelled `ValidationException` with a
/// message, a reason and the (empty) field list present.
fn assert_account_gate_refusal(case: &str, operation: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?} on {operation}: expected a ValidationException, got {body:?}"
    );
    let message = body.get("Message").and_then(Value::as_str).unwrap_or("");
    assert!(
        !message.is_empty(),
        "case {case:?} on {operation}: expected a populated Message, got {body:?}"
    );
    let reason = body.get("Reason").and_then(Value::as_str).unwrap_or("");
    // Source: `VALIDATION_EXCEPTION_REASON_VALUES` in `src/ops/types/shared_types.rs`,
    // i.e. the model's own `ValidationExceptionReason` enum — the refusal reports a
    // modelled reason, not free text.
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?} on {operation}: expected a modelled ValidationExceptionReason, got \
         {body:?}"
    );
    assert_eq!(
        body.get("Fields").and_then(Value::as_array),
        Some(&Vec::new()),
        "case {case:?} on {operation}: expected an empty Fields list, got {body:?}"
    );
}

/// The `StopTime` a `DescribeEngagement` reply carries, in seconds since the epoch.
fn stop_time_secs(case: &str, body: &Value) -> Option<f64> {
    let value = body.get("StopTime")?;
    Some(value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected StopTime to be epoch seconds, got {value:?}")
    }))
}

/// The `StartTime` a `DescribeEngagement` reply carries, in seconds since the epoch.
fn start_time_secs(case: &str, body: &Value) -> f64 {
    let value = body
        .get("StartTime")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StartTime, got {body:?}"));
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected StartTime to be epoch seconds, got {value:?}")
    })
}

// ---------------------------------------------------------------------------
// Rule 1: the account-level replication-set precondition
// ---------------------------------------------------------------------------

/// Until the account has a replication set, every call in the engagement slice is refused
/// with the modelled `ValidationException` — whether or not the engagement it names exists,
/// which is what pins the gate BEFORE the lookup — and nothing is persisted.
#[tokio::test]
async fn the_engagement_slice_is_refused_until_the_account_has_a_replication_set() {
    let emulator = Emulator::new();
    let catalog = engagement_catalog();
    // Engagements ARE stored, so "refused" cannot be confused with "there was nothing to
    // stop": the gate is evaluated first.
    seed_engagements(&emulator, &catalog);

    for row in &catalog {
        let (status, body) = emulator.call("StopEngagement", row.stop_request()).await;
        assert_account_gate_refusal(row.case, "StopEngagement", status, &body);

        let (status, body) = describe(&emulator, &row.arn()).await;
        assert_account_gate_refusal(row.case, "DescribeEngagement", status, &body);
    }

    // The same refusal for engagements that never existed, across every partition the
    // modelled ARN pattern admits.
    for arn in never_created_arns() {
        let case = format!("a never-created engagement: {arn}");
        let (status, body) = emulator
            .call("StopEngagement", json!({"EngagementId": arn.clone()}))
            .await;
        assert_account_gate_refusal(&case, "StopEngagement", status, &body);

        let (status, body) = describe(&emulator, &arn).await;
        assert_account_gate_refusal(&case, "DescribeEngagement", status, &body);
    }

    // A refused call wrote nothing: every seeded engagement is still running.
    for row in &catalog {
        let record = stored(&emulator, &row.arn());
        assert!(
            record.stop_time.is_none() && record.stop_reason.is_none(),
            "case {:?}: a refused StopEngagement must persist nothing, got {record:?}",
            row.case
        );
    }
}

/// Only an ACTIVE replication set opens the data plane: a set that has not finished
/// replicating leaves the engagement slice refused exactly as no set at all does.
#[tokio::test]
async fn only_an_active_replication_set_opens_the_engagement_slice() {
    let cases: &[(&str, Option<AccountReplicationSet>, u16)] = &[
        ("no replication set at all", None, 400),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                arn: format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
                active: false,
            }),
            400,
        ),
        (
            "an ACTIVE replication set",
            Some(AccountReplicationSet {
                arn: format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
                active: true,
            }),
            200,
        ),
    ];

    for (case, replication_set, expected_status) in cases {
        let emulator = Emulator::new();
        let row = engagement_catalog()[0].clone();
        seed_engagements(&emulator, std::slice::from_ref(&row));
        set_replication_set(&emulator, replication_set.clone());

        let (status, body) = emulator.call("StopEngagement", row.stop_request()).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected StopEngagement to answer HTTP {expected_status}, got \
             {status} {body:?}"
        );
        let record = stored(&emulator, &row.arn());
        assert_eq!(
            record.stop_time.is_some(),
            *expected_status == 200,
            "case {case:?}: the engagement must be stopped only when the call was served, \
             got {record:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a stop records when the engagement stopped
// ---------------------------------------------------------------------------

/// A stop records the instant the engagement stopped and the `Reason` it carried, so
/// `DescribeEngagement` reports a `StopTime` from then on while every other member keeps
/// the value the engagement was started with. The engagements not named keep running.
#[tokio::test]
async fn a_stop_records_the_stop_instant_and_leaves_the_other_engagements_running() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let catalog = engagement_catalog();
    seed_engagements(&emulator, &catalog);

    for (index, row) in catalog.iter().enumerate() {
        // Before the stop: the engagement is reported, and it is still running.
        let (status, before) = describe(&emulator, &row.arn()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeEngagement to answer 200, got {status} {before:?}",
            row.case
        );
        assert_eq!(
            before.get("EngagementArn").and_then(Value::as_str),
            Some(row.arn().as_str()),
            "case {:?}: DescribeEngagement must report the stored ARN, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("ContactArn").and_then(Value::as_str),
            Some(row.contact_arn().as_str()),
            "case {:?}: DescribeEngagement must report the engaged contact, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("Sender").and_then(Value::as_str),
            Some(row.sender),
            "case {:?}: unexpected Sender, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("Subject").and_then(Value::as_str),
            Some(row.subject),
            "case {:?}: unexpected Subject, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("Content").and_then(Value::as_str),
            Some(row.content),
            "case {:?}: unexpected Content, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("PublicSubject").and_then(Value::as_str),
            row.public_subject,
            "case {:?}: unexpected PublicSubject, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("PublicContent").and_then(Value::as_str),
            row.public_content,
            "case {:?}: unexpected PublicContent, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("IncidentId").and_then(Value::as_str),
            row.incident_id,
            "case {:?}: unexpected IncidentId, got {before:?}",
            row.case
        );
        assert_eq!(
            stop_time_secs(row.case, &before),
            None,
            "case {:?}: a running engagement must report no StopTime, got {before:?}",
            row.case
        );

        let before_call = SystemTime::now();
        let (status, body) = emulator.call("StopEngagement", row.stop_request()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected StopEngagement to answer 200, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            body,
            json!({}),
            "case {:?}: StopEngagement models an empty result, got {body:?}",
            row.case
        );

        // After the stop: the same engagement, now with a StopTime, and every other
        // member unchanged.
        let (status, after) = describe(&emulator, &row.arn()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeEngagement to answer 200, got {status} {after:?}",
            row.case
        );
        let stopped_at = stop_time_secs(row.case, &after).unwrap_or_else(|| {
            panic!(
                "case {:?}: a stopped engagement must report a StopTime, got {after:?}",
                row.case
            )
        });
        let called_at = epoch_secs(before_call);
        assert!(
            stopped_at >= called_at - 1.0,
            "case {:?}: StopTime {stopped_at} must be the instant of the stop, not before \
             the call at {called_at}",
            row.case
        );
        assert!(
            stopped_at >= start_time_secs(row.case, &after),
            "case {:?}: StopTime {stopped_at} must not precede StartTime, got {after:?}",
            row.case
        );
        let mut expected_unchanged = after.clone();
        expected_unchanged
            .as_object_mut()
            .expect("the reply is a JSON object")
            .remove("StopTime");
        assert_eq!(
            expected_unchanged, before,
            "case {:?}: a stop must change only StopTime",
            row.case
        );

        // The Reason the stop carried is persisted with it.
        let record = stored(&emulator, &row.arn());
        assert_eq!(
            record.stop_reason,
            row.reason.value(),
            "case {:?}: the stored stop reason must be the one the call carried",
            row.case
        );

        // Every engagement not yet stopped is still running: a stop touches ONE resource.
        for other in catalog.iter().skip(index + 1) {
            let (status, body) = describe(&emulator, &other.arn()).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeEngagement to answer 200, got {status} {body:?}",
                other.case
            );
            assert_eq!(
                stop_time_secs(other.case, &body),
                None,
                "case {:?}: must still be running after stopping {:?}, got {body:?}",
                other.case,
                row.case
            );
        }
    }
}

/// Stopping an engagement that is already stopped is accepted and changes nothing: the
/// stop instant and reason a reader has already seen stay as they were.
#[tokio::test]
async fn a_second_stop_keeps_the_first_stop_instant_and_reason() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let catalog = engagement_catalog();
    seed_engagements(&emulator, &catalog);

    for row in &catalog {
        let case = row.case;
        let (status, body) = emulator.call("StopEngagement", row.stop_request()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first StopEngagement to answer 200, got {status} \
             {body:?}"
        );
        let (_, first) = describe(&emulator, &row.arn()).await;
        let first_stop = stop_time_secs(case, &first);
        let first_reason = stored(&emulator, &row.arn()).stop_reason;

        // A second stop, with a DIFFERENT reason, so a reason that leaked through would
        // be visible.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, body) = emulator
            .call(
                "StopEngagement",
                json!({"EngagementId": row.arn(), "Reason": "stopped again"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second StopEngagement to answer 200, got {status} \
             {body:?}"
        );
        let (_, second) = describe(&emulator, &row.arn()).await;
        assert_eq!(
            stop_time_secs(case, &second),
            first_stop,
            "case {case:?}: a second stop must not move StopTime, got {second:?}"
        );
        assert_eq!(
            stored(&emulator, &row.arn()).stop_reason,
            first_reason,
            "case {case:?}: a second stop must not replace the recorded reason"
        );
    }
}

/// A `Reason` one character past the modelled maximum is refused by the constraint, and
/// the engagement it named is left running.
#[tokio::test]
async fn a_reason_past_the_modelled_maximum_length_is_refused() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let row = engagement_catalog()[0].clone();
    seed_engagements(&emulator, std::slice::from_ref(&row));

    let cases: &[(&str, ReasonCase, u16)] = &[
        ("a Reason at the published maximum, 255", ReasonCase::AtMaxLength, 200),
        (
            "a Reason one character past the maximum, 256",
            ReasonCase::PastMaxLength,
            400,
        ),
    ];
    // The refused row runs FIRST, so the accepted one cannot mask it.
    for (case, reason, expected_status) in cases.iter().rev() {
        let mut request = json!({"EngagementId": row.arn()});
        if let Some(value) = reason.value() {
            request["Reason"] = json!(value);
        }
        let (status, body) = emulator.call("StopEngagement", request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if *expected_status == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            let record = stored(&emulator, &row.arn());
            assert!(
                record.stop_time.is_none(),
                "case {case:?}: a refused stop must leave the engagement running, got \
                 {record:?}"
            );
        }
    }
}

/// An `EngagementId` the store does not hold is the modelled `ResourceNotFoundException`
/// (HTTP 404) once the account is onboarded — a different class from the account gate's
/// 400, which is the distinction a client branches on.
#[tokio::test]
async fn an_engagement_the_store_does_not_hold_answers_404() {
    let emulator = Emulator::new();
    onboard(&emulator);
    // One live engagement, so "absent" cannot be confused with "the store is empty".
    let live = engagement_catalog()[0].clone();
    seed_engagements(&emulator, std::slice::from_ref(&live));

    for arn in never_created_arns() {
        let case = format!("a never-created engagement: {arn}");
        for (operation, request) in [
            ("StopEngagement", json!({"EngagementId": arn.clone()})),
            (
                "StopEngagement",
                json!({"EngagementId": arn.clone(), "Reason": "no such engagement"}),
            ),
            ("DescribeEngagement", json!({"EngagementId": arn.clone()})),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?} on {operation}: expected a ResourceNotFoundException, got \
                 {body:?}"
            );
            assert_eq!(
                body.get("ResourceId").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?} on {operation}: the refusal must name the ARN asked for, got \
                 {body:?}"
            );
            assert_eq!(
                body.get("ResourceType").and_then(Value::as_str),
                Some("engagement"),
                "case {case:?} on {operation}: unexpected ResourceType, got {body:?}"
            );
        }
    }

    // The live engagement is untouched by any of those refusals.
    let record = stored(&emulator, &live.arn());
    assert!(
        record.stop_time.is_none(),
        "case \"the live engagement\": a refusal must persist nothing, got {record:?}"
    );
}

/// Well-formed `SsmContactsArn` values naming no stored engagement, spanning the three
/// partitions and the Region-less form the model's own pattern admits
/// (`^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=\/,.@]*:[0-9]+:([\w+=\/,.@:-]+)*$`),
/// a contact ARN (which names no engagement), and the longest engagement id form.
fn never_created_arns() -> Vec<String> {
    vec![
        format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:engagement/never-created/0"),
        format!("arn:aws-cn:ssm-contacts:cn-northwest-1:{ACCOUNT}:engagement/never/created/1"),
        format!("arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:engagement/never-created/2"),
        format!("arn:aws:ssm-contacts::{ACCOUNT}:engagement/never-created/3"),
        format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
    ]
}

/// `at` as epoch seconds, the unit `DateTime` members are rendered in on this protocol.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// ListEngagements
// ---------------------------------------------------------------------------

/// Every `EngagementArn` `ListEngagements` reports for `request`, paging to exhaustion.
/// Also asserts no page is larger than the `MaxResults` the request names.
async fn all_engagement_arns(emulator: &Emulator, request: Value, case: &str) -> Vec<String> {
    let mut request = request;
    let limit = request["MaxResults"].as_i64();
    let mut arns: Vec<String> = Vec::new();
    loop {
        let (status, body) = emulator.call("ListEngagements", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListEngagements to answer 200, got {status} {body:?}"
        );
        let page = body["Engagements"].as_array().cloned().unwrap_or_default();
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page held {} entries for MaxResults={limit}",
                page.len()
            );
        }
        for engagement in &page {
            let arn = engagement["EngagementArn"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected every entry to carry an EngagementArn, got \
                     {engagement:?}"
                )
            });
            arns.push(arn.to_string());
        }
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    arns.len() <= 64,
                    "case {case:?}: ListEngagements paging did not terminate, collected {arns:?}"
                );
                request["NextToken"] = json!(token);
            }
            None => return arns,
        }
    }
}

/// `ListEngagements` reports exactly the stored engagements its filters admit, in the
/// store's order, whatever page size the caller asks for.
///
/// The rows span no filter, the `IncidentId` filter (matching and matching nothing), each
/// `TimeRange` bound on its own and both together, and page sizes from one entry to the
/// whole store \u2014 the model gives `MaxResults` the range 0..1024, and 0 names no page size
/// at all, so it is served as an absent member is.
#[tokio::test]
async fn list_engagements_reports_the_stored_engagements_its_filters_admit() {
    let emulator = Emulator::new();
    let catalog = engagement_catalog();
    seed_engagements(&emulator, &catalog);

    let all = sorted_arns(&catalog);
    let with_incident: Vec<String> = sorted_arns(
        &catalog
            .iter()
            .filter(|row| row.incident_id == catalog[1].incident_id)
            .cloned()
            .collect::<Vec<_>>(),
    );
    // The rows started at or after SEED_SECS + 60: the last two of the catalog.
    let started_late: Vec<String> = sorted_arns(
        &catalog
            .iter()
            .filter(|row| row.start_offset_secs >= 60)
            .cloned()
            .collect::<Vec<_>>(),
    );
    // And the rows started at or before SEED_SECS + 30: the first two.
    let started_early: Vec<String> = sorted_arns(
        &catalog
            .iter()
            .filter(|row| row.start_offset_secs <= 30)
            .cloned()
            .collect::<Vec<_>>(),
    );

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), all.clone()),
        (
            "one page per entry",
            json!({"MaxResults": 1}),
            all.clone(),
        ),
        (
            "two entries per page",
            json!({"MaxResults": 2}),
            all.clone(),
        ),
        (
            "MaxResults 0, the published minimum, names no page size",
            json!({"MaxResults": 0}),
            all.clone(),
        ),
        (
            "MaxResults 1024, the published maximum",
            json!({"MaxResults": 1024}),
            all.clone(),
        ),
        (
            "the IncidentId of the one engagement that names an incident ARN",
            json!({"IncidentId": catalog[1].incident_id}),
            with_incident,
        ),
        (
            "an IncidentId no engagement carries",
            json!({"IncidentId": "never-engaged"}),
            Vec::new(),
        ),
        (
            "a TimeRange with only a StartTime",
            json!({"TimeRangeValue": {"StartTime": SEED_SECS + 60}}),
            started_late,
        ),
        (
            "a TimeRange with only an EndTime",
            json!({"TimeRangeValue": {"EndTime": SEED_SECS + 30}}),
            started_early,
        ),
        (
            "a TimeRange spanning every engagement",
            json!({"TimeRangeValue": {"StartTime": SEED_SECS, "EndTime": SEED_SECS + 90}}),
            all.clone(),
        ),
        (
            "a TimeRange that ends before the first engagement started",
            json!({"TimeRangeValue": {"EndTime": SEED_SECS - 1}}),
            Vec::new(),
        ),
        (
            "a TimeRange narrowed to the second engagement alone",
            json!({
                "TimeRangeValue": {"StartTime": SEED_SECS + 30, "EndTime": SEED_SECS + 30},
                "MaxResults": 1,
            }),
            vec![catalog[1].arn()],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = all_engagement_arns(&emulator, request, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: ListEngagements reported the wrong engagements"
        );
    }

    // A stop is visible through the list too: the summary gains the StopTime the stop
    // recorded, and the engagement stays listed. (The stop is resource-addressed, so the
    // account needs its replication set for it.)
    onboard(&emulator);
    let stopped = catalog[0].clone();
    let (status, body) = emulator
        .call("StopEngagement", stopped.stop_request())
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected StopEngagement to answer 200, got {status} {body:?}",
        stopped.case
    );
    let (status, body) = emulator.call("ListEngagements", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"after a stop\": expected ListEngagements to answer 200, got {status} {body:?}"
    );
    let entries = body["Engagements"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        catalog.len(),
        "case \"after a stop\": a stopped engagement stays listed, got {body:?}"
    );
    for entry in &entries {
        let arn = entry["EngagementArn"].as_str().unwrap_or_default();
        let has_stop_time = entry.get("StopTime").is_some();
        assert_eq!(
            has_stop_time,
            arn == stopped.arn(),
            "case \"after a stop\": only the stopped engagement carries a StopTime, got \
             {entry:?}"
        );
    }
}

/// A `NextToken` this list did not issue is refused with the modelled
/// `ValidationException`, not served as though it named a resume point.
#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    seed_engagements(&emulator, &engagement_catalog());

    // A token this service minted for ANOTHER list: readable, but not this list's.
    let foreign_token = encode_next_token(
        TOKEN_SCOPE_CONTACTS,
        &format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
    );

    let cases: &[(&str, &str)] = &[
        ("text that is not a token at all", "not-a-token"),
        ("hex that decodes to no scope", "6162636465"),
        ("a token issued for another list", foreign_token.as_str()),
    ];
    for (case, token) in cases {
        let (status, body) = emulator
            .call("ListEngagements", json!({"NextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// `ListEngagements` resolves no resource, so the account's replication set is not a
/// precondition for it: an account with none is served an empty list, where the
/// resource-addressed `StopEngagement` on the same account is refused with 400.
#[tokio::test]
async fn an_account_with_no_replication_set_is_served_an_empty_engagement_list() {
    let emulator = Emulator::new();
    let case = "an account with no replication set and no engagements";

    let (status, body) = emulator.call("ListEngagements", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListEngagements to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Engagements"].as_array(),
        Some(&Vec::new()),
        "case {case:?}: expected an empty Engagements list, got {body:?}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: a last page carries no NextToken, got {body:?}"
    );

    // The same account, the same request, through a resource-addressed call: refused.
    let row = engagement_catalog()[0].clone();
    let (status, body) = emulator.call("StopEngagement", row.stop_request()).await;
    assert_account_gate_refusal(case, "StopEngagement", status, &body);
}

/// The ARNs of `rows`, in the order the store holds them (by `EngagementArn`).
fn sorted_arns(rows: &[EngagementRow]) -> Vec<String> {
    let mut arns: Vec<String> = rows.iter().map(EngagementRow::arn).collect();
    arns.sort();
    arns
}
