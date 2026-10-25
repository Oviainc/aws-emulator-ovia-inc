// SPDX-License-Identifier: CC0-1.0

//! The window an asynchronous deregistration leaves open, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` to assert a behaviour: the ONE read of the store here is
//! the id of the record the deregistration files, which no response hands back, and every
//! assertion about that record is then made over the wire through `GetOperation`.
//!
//! RULE under test: a call that starts asynchronous work returns at once and the resource
//! it acts on keeps resolving in the transient status that work gave it until the change
//! the clock owes falls due — so a deregistration leaves the application reporting
//! `DELETING` to every reader, refuses another operation on it meanwhile, and the
//! application stops resolving only once the removal falls due, while the record of the
//! work itself is filed like any other operation's.
//!
//! The rows span what that rule generalises over: the `ApplicationId` forms the modelled
//! pattern admits, the application type, whether the registration carried tags, and the
//! state the application is in when the deregistration starts — still registering (a
//! change already owed), activated, started, and stopped.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    APPLICATION_DISCOVERY_STATUS_VALUES, APPLICATION_STATUS_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    APPLICATION_STATUS_DELETING, DISCOVERY_STATUS_DELETING, OPERATION_TYPE_DEREGISTER_APPLICATION,
    TRANSITION_SPAN,
};

use crate::application_lifecycle::{settle, tag_pairs, valid_registration, Emulator, Reply, REGION};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code an operation already under way on an application is refused
/// with (`smithy_query(query="operation", operation="StartApplication")`:
/// `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// `OperationStatus$SUCCESS`, as the model spells it.
const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// What the application has been through by the time it is deregistered.
#[derive(Debug, Clone, Copy)]
enum Before {
    /// Nothing: the registration is still settling, so a change was already owed when
    /// the deregistration started.
    StillRegistering,
    /// The registration has settled, so the application is activated and no change is
    /// owed.
    Activated,
    /// A settled `StartApplication` on top of that.
    Started,
    /// A settled `StopApplication` on top of that, so the application is stopped.
    Stopped,
}

/// One application to deregister.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape",
    /// shape="ApplicationType")`: `HANA` | `SAP_ABAP`).
    app_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
    before: Before,
}

/// The rows: the state the application is in when the deregistration starts, across both
/// application types, tagged and untagged, with ids of different forms.
const CASES: &[Case] = &[
    Case {
        label: "an activated, tagged HANA application",
        id: "HDB",
        app_type: "HANA",
        tags: &[("env", "dev")],
        before: Before::Activated,
    },
    Case {
        label: "an application whose registration is still settling",
        id: "a",
        app_type: "HANA",
        tags: &[],
        before: Before::StillRegistering,
    },
    Case {
        label: "a started SAP_ABAP application with a dotted id",
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        tags: &[("owner", "sap-team"), ("tier", "prod")],
        before: Before::Started,
    },
    Case {
        label: "a stopped application",
        id: "HDB-TEST",
        app_type: "HANA",
        tags: &[],
        before: Before::Stopped,
    },
];

impl Emulator {
    /// Register `case`'s application and return the `OperationId` the call handed out.
    async fn register_case(&self, case: &Case) -> String {
        self.register_application(case.label, case.id, case.app_type, case.tags)
            .await
    }

    /// Register one application and return the `OperationId` the call handed out.
    async fn register_application(
        &self,
        label: &str,
        id: &str,
        app_type: &str,
        tags: &[(&str, &str)],
    ) -> String {
        let mut body = match valid_registration(id, app_type) {
            Value::Object(body) => body,
            other => panic!("case {label:?}: a registration body is an object, got {other:?}"),
        };
        if !tags.is_empty() {
            let mut rendered = Map::new();
            for (key, value) in tags {
                rendered.insert((*key).to_string(), json!(value));
            }
            body.insert("Tags".into(), Value::Object(rendered));
        }
        let reply = self.send("/register-application", Value::Object(body)).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        operation_id(label, &reply)
    }

    /// The ids of the `DEREGISTER_APPLICATION` records filed against `application_id` in
    /// [`REGION`].
    ///
    /// `DeregisterApplicationOutput` carries no `OperationId`
    /// (`smithy_query(query="shape", shape="DeregisterApplicationOutput")`: no members),
    /// so this is the one record of this slice whose id no response hands out — hence the
    /// store read. Everything asserted about the record itself goes over the wire.
    fn deregister_record_ids(&self, application_id: &str) -> Vec<String> {
        let mut ids = Vec::new();
        self.with_state(|state| {
            for (id, record) in &state.region(REGION).operations {
                if record.op_type == OPERATION_TYPE_DEREGISTER_APPLICATION
                    && record.resource_id == application_id
                {
                    ids.push(id.clone());
                }
            }
        });
        ids
    }
}

/// The `OperationId` a reply handed out.
fn operation_id(label: &str, reply: &Reply) -> String {
    reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label:?}: expected an OperationId, got {reply:?}"))
        .to_string()
}

/// Every `ApplicationSummary` a listing reported, as id -> `DiscoveryStatus`.
fn listed(reply: &Reply) -> BTreeMap<String, String> {
    reply
        .member("Applications")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("DiscoveryStatus")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN the registration of `id` mints in [`REGION`].
fn expected_arn(id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/{id}")
}

#[tokio::test]
async fn a_deregistration_reports_deleting_until_the_removal_falls_due() {
    // The two statuses a deregistration reports are the model's own
    // (`smithy_query(query="shape", shape="ApplicationStatus,ApplicationDiscoveryStatus")`),
    // so no case can pass on a value the API does not define.
    assert!(
        APPLICATION_STATUS_VALUES.contains(&APPLICATION_STATUS_DELETING)
            && APPLICATION_DISCOVERY_STATUS_VALUES.contains(&DISCOVERY_STATUS_DELETING),
        "the deleting statuses must be modelled values, got {APPLICATION_STATUS_DELETING:?} \
         and {DISCOVERY_STATUS_DELETING:?}"
    );

    for case in CASES {
        let label = case.label;
        let emulator = Emulator::new();
        // A second application, deregistered by nothing, proves the work is scoped to the
        // application the call names.
        let survivor = format!("{}-keep", case.id);
        let mut earlier_operations = vec![emulator.register_case(case).await];
        emulator
            .register_application(label, &survivor, case.app_type, &[])
            .await;

        match case.before {
            Before::StillRegistering => {}
            Before::Activated => settle().await,
            Before::Started => {
                settle().await;
                let started = emulator
                    .send("/start-application", json!({"ApplicationId": case.id}))
                    .await;
                assert_eq!(
                    started.status, 200,
                    "case {label:?}: expected 200 from StartApplication, got {started:?}"
                );
                earlier_operations.push(operation_id(label, &started));
                settle().await;
            }
            Before::Stopped => {
                settle().await;
                let stopped = emulator
                    .send("/stop-application", json!({"ApplicationId": case.id}))
                    .await;
                assert_eq!(
                    stopped.status, 200,
                    "case {label:?}: expected 200 from StopApplication, got {stopped:?}"
                );
                earlier_operations.push(operation_id(label, &stopped));
                settle().await;
            }
        }

        let deregistered = emulator
            .send("/deregister-application", json!({"ApplicationId": case.id}))
            .await;
        let started_at = Instant::now();
        assert_eq!(
            deregistered.status, 200,
            "case {label:?}: expected 200 from DeregisterApplication, got {deregistered:?}"
        );
        assert!(
            deregistered.member("Application").is_none()
                && deregistered.member("OperationId").is_none(),
            "case {label:?}: DeregisterApplicationOutput models no member, got {deregistered:?}"
        );

        // Everything a client can do while the deregistration runs, before any assertion
        // gates on the clock.
        let during = emulator
            .send("/get-application", json!({"ApplicationId": case.id}))
            .await;
        let listing = emulator.send("/list-applications", json!({})).await;
        let filtered = emulator
            .send(
                "/list-applications",
                json!({"Filters": [{
                    "Name": "DiscoveryStatus",
                    "Value": DISCOVERY_STATUS_DELETING,
                    "Operator": "Equals",
                }]}),
            )
            .await;
        let restarted = emulator
            .send("/start-application", json!({"ApplicationId": case.id}))
            .await;
        let again = emulator
            .send("/deregister-application", json!({"ApplicationId": case.id}))
            .await;
        let still = emulator
            .send("/get-application", json!({"ApplicationId": case.id})).await;
        let records_during = emulator.deregister_record_ids(case.id);

        // The transient assertions hold only while the clock still owes the removal: past
        // that instant the application was due to go whether or not anybody called.
        if started_at.elapsed() < TRANSITION_SPAN {
            assert_eq!(
                during.status, 200,
                "case {label:?}: a deregistering application still resolves, got {during:?}"
            );
            assert_eq!(
                during.app_str("Status"),
                APPLICATION_STATUS_DELETING,
                "case {label:?}: expected Status {APPLICATION_STATUS_DELETING}, got {during:?}"
            );
            assert_eq!(
                during.app_str("DiscoveryStatus"),
                DISCOVERY_STATUS_DELETING,
                "case {label:?}: expected DiscoveryStatus {DISCOVERY_STATUS_DELETING}, got \
                 {during:?}"
            );
            assert_eq!(
                during.app_str("Arn"),
                expected_arn(case.id),
                "case {label:?}: the deregistering application keeps its ARN, got {during:?}"
            );
            let expected_tags: BTreeMap<String, String> = case
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                tag_pairs(during.member("Tags")),
                expected_tags,
                "case {label:?}: its tags are still held while it is deleting, got {during:?}"
            );

            let reported = listed(&listing);
            assert_eq!(
                reported.keys().cloned().collect::<Vec<String>>(),
                {
                    let mut expected = vec![case.id.to_string(), survivor.clone()];
                    expected.sort();
                    expected
                },
                "case {label:?}: a deregistering application is still listed, got {listing:?}"
            );
            assert_eq!(
                reported.get(case.id).map(String::as_str),
                Some(DISCOVERY_STATUS_DELETING),
                "case {label:?}: it is listed as deleting, got {listing:?}"
            );
            assert_eq!(
                listed(&filtered).keys().cloned().collect::<Vec<String>>(),
                vec![case.id.to_string()],
                "case {label:?}: the deleting application is the one that filter matches, got \
                 {filtered:?}"
            );

            assert_eq!(
                restarted.status, 409,
                "case {label:?}: a deregistering application cannot be started, got {restarted:?}"
            );
            assert_eq!(
                restarted.code(),
                CONFLICT_EXCEPTION,
                "case {label:?}: expected {CONFLICT_EXCEPTION}, got {restarted:?}"
            );
            assert!(
                message(&restarted).contains(case.id),
                "case {label:?}: the refusal must name the application, got {:?}",
                message(&restarted)
            );

            assert_eq!(
                again.status, 200,
                "case {label:?}: deregistering a deregistering application is served, got \
                 {again:?}"
            );
            assert_eq!(
                still.app_str("Status"),
                APPLICATION_STATUS_DELETING,
                "case {label:?}: a second deregistration leaves it deleting, got {still:?}"
            );
            assert_eq!(
                records_during.len(),
                1,
                "case {label:?}: one deregistration files one record, got {records_during:?}"
            );
        }

        // Past the due instant the application is gone, without any further call having
        // been needed to make it so.
        settle().await;

        let after = emulator
            .send("/get-application", json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            after.status, 400,
            "case {label:?}: a completed deregistration stops resolving, got {after:?}"
        );
        assert_eq!(
            after.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {after:?}"
        );
        assert!(
            after.member("Application").is_none(),
            "case {label:?}: nothing may be reported for it, got {after:?}"
        );

        let listing_after = emulator.send("/list-applications", json!({})).await;
        assert_eq!(
            listed(&listing_after).keys().cloned().collect::<Vec<String>>(),
            vec![survivor.clone()],
            "case {label:?}: only the other application remains listed, got {listing_after:?}"
        );

        let restart_after = emulator
            .send("/start-application", json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            restart_after.status, 404,
            "case {label:?}: expected 404 once the removal completed, got {restart_after:?}"
        );

        // The record of the deregistration is filed like every other operation's, and the
        // records of the calls made before it are history the removal does not destroy.
        let records = emulator.deregister_record_ids(case.id);
        assert_eq!(
            records.len(),
            1,
            "case {label:?}: the deregistration files exactly one record, got {records:?}"
        );
        let record = emulator
            .send("/get-operation", json!({"OperationId": records[0]}))
            .await;
        assert_eq!(
            record.status, 200,
            "case {label:?}: the deregistration's own record resolves, got {record:?}"
        );
        assert_eq!(
            (
                record.op_str("Type"),
                record.op_str("Status"),
                record.op_str("ResourceId"),
                record.op_str("ResourceArn"),
            ),
            (
                OPERATION_TYPE_DEREGISTER_APPLICATION,
                OPERATION_STATUS_SUCCESS,
                case.id,
                expected_arn(case.id).as_str(),
            ),
            "case {label:?}: the record must describe the deregistration it was filed for, \
             got {record:?}"
        );
        assert!(
            record.op("EndTime").is_some(),
            "case {label:?}: a completed operation reports when it ended, got {record:?}"
        );
        for earlier in &earlier_operations {
            let reply = emulator
                .send("/get-operation", json!({"OperationId": earlier}))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: an OperationId handed out before the deregistration still \
                 resolves, got {reply:?}"
            );
            assert_eq!(
                reply.op_str("ResourceId"),
                case.id,
                "case {label:?}: that record still names the application it was filed against, \
                 got {reply:?}"
            );
        }
    }
}
