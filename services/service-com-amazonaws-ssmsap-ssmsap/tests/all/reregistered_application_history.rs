// SPDX-License-Identifier: CC0-1.0

//! The history a registration reports when its `ApplicationId` has been used before,
//! driven over the wire.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: every record this suite
//! names is one a call reported when it filed it, and every expectation about it is read
//! back through `ListOperations` and `GetOperation`.
//!
//! RULE under test: a resource is identified by the registration that created it and not
//! by the identifier it was registered under, which is free again once its removal
//! completes — so the records a listing scoped to one application reports are the ones
//! filed against THAT registration, and a registration made under a reused identifier
//! reports its own calls alone. The records of the ended registration are not destroyed:
//! they stay the history of calls already made, each resolvable by its own `OperationId`.
//!
//! The rows span what the rule generalises over: the form of the `ApplicationId` (upper
//! case with a digit, dotted and underscored, one character, hyphenated), the application
//! type, whether the registration carried tags, how many registrations the id has had (two
//! and three), and which calls each of them made — none, one, and every control call an
//! API of this service files a record for — so the rows differ in how many records each
//! registration filed and of which types.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    OPERATION_TYPE_DEREGISTER_APPLICATION, OPERATION_TYPE_REGISTER_APPLICATION,
};

use crate::app_registry_arn::reported;
use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::configuration_checks::string_field;
use crate::validation_message_rendering::VALIDATION_EXCEPTION;

/// The routes the calls below are bound to (`smithy_query(query="digest")`: every one of
/// these operations is `POST` with a JSON body).
const LIST_OPERATIONS: &str = "/list-operations";
const GET_APPLICATION: &str = "/get-application";
const GET_OPERATION: &str = "/get-operation";
const REGISTER_APPLICATION: &str = "/register-application";
const DEREGISTER_APPLICATION: &str = "/deregister-application";
const START_APPLICATION: &str = "/start-application";
const START_APPLICATION_REFRESH: &str = "/start-application-refresh";
const STOP_APPLICATION: &str = "/stop-application";

/// A `Filter$Operator` value, as the model's `FilterOperator` enum spells it.
const EQUALS: &str = "Equals";

/// One `ApplicationId`, registered — and deregistered — a row's number of times.
struct Case {
    label: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape",
    /// shape="ApplicationType")`: `HANA` | `SAP_ABAP`).
    app_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// One entry per successive registration of `id`, in order: the routes of the calls
    /// made on that registration after it was made, each of which files one record
    /// besides the registration's own. Every registration but the last is deregistered,
    /// and its removal waited out, before the next is made.
    registrations: &'static [&'static [&'static str]],
}

/// The rows.
const CASES: &[Case] = &[
    Case {
        label: "a tagged HANA application re-registered after a start",
        id: "HDB1",
        app_type: "HANA",
        tags: &[("env", "dev")],
        registrations: &[&[START_APPLICATION], &[]],
    },
    Case {
        label: "a dotted SAP_ABAP application re-registered after every control call",
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        tags: &[],
        registrations: &[
            &[START_APPLICATION, START_APPLICATION_REFRESH, STOP_APPLICATION],
            &[START_APPLICATION],
        ],
    },
    Case {
        label: "the shortest id the model admits, registered three times",
        id: "a",
        app_type: "HANA",
        tags: &[],
        registrations: &[&[], &[START_APPLICATION], &[]],
    },
    Case {
        label: "an untouched registration made again under the same id",
        id: "HDB-TEST",
        app_type: "HANA",
        tags: &[("owner", "sap-team"), ("tier", "prod")],
        registrations: &[&[], &[]],
    },
];

/// Register one application and return the reply, which carries both the `OperationId` of
/// the record the registration filed and the `AppRegistryArn` that registration was
/// assigned.
async fn register(
    emulator: &Emulator,
    label: &str,
    id: &str,
    app_type: &str,
    tags: &[(&str, &str)],
) -> Reply {
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
    let reply = emulator.send(REGISTER_APPLICATION, Value::Object(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected 200 from RegisterApplication of {id:?}, got {reply:?}"
    );
    reply
}

/// The `OperationId` a call that files a record reported.
fn operation_id(reply: &Reply, label: &str) -> String {
    let id = reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {label:?}: the call must report the record it filed, got {reply:?}"
    );
    id
}

/// One `ListOperations` request for `id`.
async fn list(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send(LIST_OPERATIONS, json!({"ApplicationId": id}))
        .await
}

/// The `Operations` a listing reported.
fn rows(reply: &Reply) -> Vec<Value> {
    reply
        .member("Operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of every row a listing reported.
fn listed_ids(reply: &Reply) -> BTreeSet<String> {
    rows(reply)
        .iter()
        .map(|row| string_field(row, "Id"))
        .collect()
}

/// The `Type` of every row a listing reported.
fn listed_types(reply: &Reply) -> BTreeSet<String> {
    rows(reply)
        .iter()
        .map(|row| string_field(row, "Type"))
        .collect()
}

/// RULE: the records a listing reports for an application are the ones filed against the
/// registration that id currently names — its own calls, never a previous registration's —
/// while the records of an ended registration stay resolvable by their own ids.
#[tokio::test]
async fn a_registration_reports_the_records_of_its_own_calls_alone() {
    for case in CASES {
        let label = case.label;
        let emulator = Emulator::new();

        // A second application of the same Region, registered once and deregistered
        // never: the rule says a live registration keeps reporting every record filed
        // against it, so this is the row's control against a listing that narrows too far.
        let control_id = format!("{}-keep", case.id);
        let control_reply = register(&emulator, label, &control_id, case.app_type, &[]).await;
        let control_record = operation_id(&control_reply, label);

        // The records of every registration already ended, and the AppRegistry
        // applications assigned so far — one per registration, which is what makes the
        // registrations distinguishable at all (`tests/all/app_registry_arn.rs`).
        let mut ended_records: BTreeSet<String> = BTreeSet::new();
        let mut assigned: BTreeSet<String> = BTreeSet::new();

        for (index, calls) in case.registrations.iter().enumerate() {
            let registered = register(&emulator, label, case.id, case.app_type, case.tags).await;
            let registry_arn = reported(&registered).unwrap_or_else(|| {
                panic!(
                    "case {label:?}: registration {index} must report an AppRegistryArn, got \
                     {registered:?}"
                )
            });
            assert!(
                assigned.insert(registry_arn.clone()),
                "case {label:?}: registration {index} must record its own AppRegistry \
                 application, got the already-assigned {registry_arn:?}"
            );

            let mut mine: BTreeSet<String> = BTreeSet::new();
            mine.insert(operation_id(&registered, label));
            settle().await;
            for path in *calls {
                let reply = emulator
                    .send(path, json!({"ApplicationId": case.id}))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {label:?}: registration {index}: expected 200 from {path}, got {reply:?}"
                );
                mine.insert(operation_id(&reply, label));
                settle().await;
            }

            // What this registration's listing reports: exactly the records its own calls
            // filed.
            let listing = list(&emulator, case.id).await;
            assert_eq!(
                listing.status, 200,
                "case {label:?}: registration {index}: expected 200 from ListOperations, got \
                 {listing:?}"
            );
            let reported_ids = listed_ids(&listing);
            assert_eq!(
                reported_ids, mine,
                "case {label:?}: registration {index}: expected exactly the {} record(s) this \
                 registration filed ({mine:?}), got {listing:?}",
                mine.len()
            );
            let inherited: BTreeSet<String> =
                reported_ids.intersection(&ended_records).cloned().collect();
            assert!(
                inherited.is_empty(),
                "case {label:?}: registration {index}: an ended registration's records are no \
                 part of this one's history, got {inherited:?}"
            );
            assert!(
                !listed_types(&listing).contains(OPERATION_TYPE_DEREGISTER_APPLICATION),
                "case {label:?}: registration {index}: this application has not been \
                 deregistered, so no record of its history may say it was, got {listing:?}"
            );

            // The application the listing is of is the one a reader reports: a live,
            // freshly registered application, whose AppRegistry application is the one
            // this registration was assigned.
            let read = emulator
                .send(GET_APPLICATION, json!({"ApplicationId": case.id}))
                .await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: registration {index}: the registered application resolves, got \
                 {read:?}"
            );
            assert_eq!(
                reported(&read).as_deref(),
                Some(registry_arn.as_str()),
                "case {label:?}: registration {index}: the listing is of the registration the \
                 reader reports, got {read:?}"
            );

            // A filter cannot reach the dead records either: one registration filed one
            // REGISTER_APPLICATION record, however many were ever filed under this id.
            let filtered = emulator
                .send(
                    LIST_OPERATIONS,
                    json!({
                        "ApplicationId": case.id,
                        "Filters": [{
                            "Name": "Type",
                            "Value": OPERATION_TYPE_REGISTER_APPLICATION,
                            "Operator": EQUALS,
                        }],
                    }),
                )
                .await;
            assert_eq!(
                rows(&filtered).len(),
                1,
                "case {label:?}: registration {index}: one registration filed one \
                 {OPERATION_TYPE_REGISTER_APPLICATION} record, got {filtered:?}"
            );

            // Nothing of the ended registrations was destroyed: every record they filed
            // still resolves by its own id, and still names the application it was filed
            // against.
            for record_id in &ended_records {
                let record = emulator
                    .send(GET_OPERATION, json!({"OperationId": record_id}))
                    .await;
                assert_eq!(
                    record.status, 200,
                    "case {label:?}: registration {index}: a record filed before the removal \
                     still resolves by its id, got {record:?}"
                );
                assert_eq!(
                    record.op_str("ResourceId"),
                    case.id,
                    "case {label:?}: registration {index}: that record still names the \
                     application it was filed against, got {record:?}"
                );
            }

            // And the control application still reports its own record, whatever happened
            // beside it.
            let control = list(&emulator, &control_id).await;
            assert_eq!(
                listed_ids(&control),
                BTreeSet::from([control_record.clone()]),
                "case {label:?}: registration {index}: a live registration keeps reporting the \
                 records filed against it, got {control:?}"
            );

            ended_records.extend(mine);

            // Deregister, unless this was the last registration of the row, and let the
            // removal fall due — from that instant the id names no application and is free
            // to be registered again.
            if index + 1 < case.registrations.len() {
                let deregistered = emulator
                    .send(DEREGISTER_APPLICATION, json!({"ApplicationId": case.id}))
                    .await;
                assert_eq!(
                    deregistered.status, 200,
                    "case {label:?}: registration {index}: expected 200 from \
                     DeregisterApplication, got {deregistered:?}"
                );
                settle().await;
                let gone = emulator
                    .send(GET_APPLICATION, json!({"ApplicationId": case.id}))
                    .await;
                assert_eq!(
                    gone.status, 400,
                    "case {label:?}: registration {index}: expected the removal to have \
                     completed, got {gone:?}"
                );
                assert_eq!(
                    gone.code(),
                    VALIDATION_EXCEPTION,
                    "case {label:?}: registration {index}: expected {VALIDATION_EXCEPTION}, got \
                     {gone:?}"
                );
            }
        }
    }
}
