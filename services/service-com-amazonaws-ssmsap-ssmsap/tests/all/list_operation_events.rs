// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListOperationEvents`, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. The
//! events are only believed against what the call that FILED the operation reported and
//! what `GetOperation` reads back for it; the one record no API of this service produces
//! (an operation that ended in `ERROR`) is placed in the store, and that case says so.
//!
//! RULE under test: the listing REPORTS the milestones of the one operation record its
//! `OperationId` names — the start of its work, timestamped at that record's `StartTime`
//! and `IN_PROGRESS` while it runs, and, once the clock has brought the record to a
//! terminal status, a second milestone at the instant it reached it, `COMPLETED` for a
//! success and `FAILED` for an error with whatever the record explains that status with —
//! every milestone naming the resource the record names, narrowed by every `Filter` the
//! request carries over the attributes this operation documents, and handed out in pages
//! of at most `MaxResults` whose `NextToken` resumes exactly where the previous page
//! stopped; an `OperationId` this account and Region holds no record for is refused, and
//! the call changes nothing a later caller sees.
//!
//! The rows span what the rule generalises over: every kind of operation an API of this
//! service files (register, start, stop and deregister), both sides of the clock
//! (running and settled), each `OperationEventStatus` a record can reach, every filter
//! name this listing documents and every `FilterOperator` the model declares,
//! single- and multi-valued filter values and several filters at once, page sizes from
//! one milestone per page to past the whole set including each published `MaxResults`
//! bound and one past it, and the refusals: an unknown id, an id of another Region, a
//! filter name outside the documented set and a token this service did not issue.

use std::collections::BTreeSet;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    FILTER_OPERATOR_VALUES, OPERATION_EVENT_STATUS_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    no_operation_message, FILTER_OPERATOR_EQUALS, MAX_PAGE_SIZE,
    OPERATION_EVENT_FILTER_NAMES, OPERATION_EVENT_RESOURCE_TYPE_APPLICATION,
    OPERATION_EVENT_STATUS_COMPLETED, OPERATION_EVENT_STATUS_FAILED,
    OPERATION_EVENT_STATUS_IN_PROGRESS, OPERATION_STATUS_ERROR,
    OPERATION_TYPE_DEREGISTER_APPLICATION, OPERATION_TYPE_REGISTER_APPLICATION,
    OPERATION_TYPE_START_APPLICATION, OPERATION_TYPE_STOP_APPLICATION, TRANSITION_SPAN,
};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply, REGION};
use crate::configuration_checks::string_field;
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The path `ListOperationEvents` is bound to (`smithy_query(query="operation",
/// operation="ListOperationEvents")`: POST `/list-operation-events`).
const LIST_OPERATION_EVENTS: &str = "/list-operation-events";

/// Another Region to drive the same id at: an operation record belongs to the Region the
/// call that filed it was made in, so an id filed at [`REGION`] resolves at no other.
const ELSEWHERE: &str = "eu-west-1";

/// A syntactically valid `OperationId` (the model's pattern is a UUID) that no call of
/// this suite ever files, so nothing in the store answers for it.
const UNFILED_OPERATION_ID: &str = "11111111-2222-3333-4444-555555555555";

impl Emulator {
    async fn list_events(&self, body: Value) -> Reply {
        self.send(LIST_OPERATION_EVENTS, body).await
    }

    /// `GetOperation` for `id` — the reader every expectation about a record is stated
    /// against.
    async fn read_operation(&self, id: &str) -> Reply {
        self.send("/get-operation", json!({"OperationId": id})).await
    }

    /// Register a HANA application and wait out its registration, returning the
    /// `OperationId` the registration reported.
    async fn registered(&self, id: &str) -> String {
        let reply = self
            .send("/register-application", valid_registration(id, "HANA"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        let operation_id = operation_id(&reply);
        settle().await;
        operation_id
    }
}

/// The `OperationId` a call that files an operation reported.
fn operation_id(reply: &Reply) -> String {
    reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `OperationEvents` of a listing reply.
fn listed(reply: &Reply) -> Vec<Value> {
    reply
        .member("OperationEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `NextToken` of a listing reply, absent when the page is the last.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `Status` values of some listed events, in the order the listing reported them.
fn statuses(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| string_field(event, "Status"))
        .collect()
}

/// A member of one event's `Resource`, treating JSON `null` as absent.
fn resource_field(event: &Value, name: &str) -> String {
    event
        .get("Resource")
        .and_then(|resource| resource.get(name))
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the operation table: a kind of operation an API of this service files, and
/// the call that files it.
struct OperationCase {
    /// The `Operation$Type` the record carries, read off the crate's own constant rather
    /// than written out.
    op_type: &'static str,
    /// The application the row files its operation against.
    application: &'static str,
    path: &'static str,
    /// Whether the application has to be registered before the row's call is made: the
    /// registration row's own call is what creates it.
    needs_application: bool,
    /// Whether the call's own output hands the `OperationId` back: all of them do except
    /// `DeregisterApplication`, whose modelled output carries no such member
    /// (`smithy_query(query="operation", operation="DeregisterApplication")`), so that
    /// row resolves the record's id through [`filed_operation_id`] and asserts everything
    /// else over the wire.
    reports_id: bool,
    /// The request body, beyond the `ApplicationId` every one of these calls carries.
    extra: fn() -> Value,
}

/// No member beyond `ApplicationId`.
fn no_extra() -> Value {
    json!({})
}

/// A stop asked for with both of the switches `StopApplication` takes, so the row's
/// record carries `Properties` as well.
fn stop_switches() -> Value {
    json!({"StopConnectedEntity": "DBMS", "IncludeEc2InstanceShutdown": true})
}

/// Every kind of operation an API of this service files. Each row uses its OWN
/// application, so one row's asynchronous call cannot be the change another row's call is
/// refused for.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        OperationCase {
            op_type: OPERATION_TYPE_REGISTER_APPLICATION,
            application: "HDB-EVENTS-REGISTER",
            path: "/register-application",
            needs_application: false,
            reports_id: true,
            extra: no_extra,
        },
        OperationCase {
            op_type: OPERATION_TYPE_START_APPLICATION,
            application: "HDB-EVENTS-START",
            path: "/start-application",
            needs_application: true,
            reports_id: true,
            extra: no_extra,
        },
        OperationCase {
            op_type: OPERATION_TYPE_STOP_APPLICATION,
            application: "HDB-EVENTS-STOP",
            path: "/stop-application",
            needs_application: true,
            reports_id: true,
            extra: stop_switches,
        },
        OperationCase {
            op_type: OPERATION_TYPE_DEREGISTER_APPLICATION,
            application: "HDB-EVENTS-DEREGISTER",
            path: "/deregister-application",
            needs_application: true,
            reports_id: false,
            extra: no_extra,
        },
    ]
}

impl OperationCase {
    /// The request body that files this row's operation.
    fn body(&self) -> Value {
        let mut body = (self.extra)();
        body.as_object_mut()
            .expect("the extra members are a JSON object")
            .insert("ApplicationId".to_string(), json!(self.application));
        if self.op_type == OPERATION_TYPE_REGISTER_APPLICATION {
            // A registration carries the members registering an application requires,
            // which the harness' own builder assembles.
            return valid_registration(self.application, "HANA");
        }
        body
    }

    /// File this row's operation and return its `OperationId`.
    async fn file(&self, emulator: &Emulator) -> String {
        if self.needs_application {
            emulator.registered(self.application).await;
        }
        let reply = emulator.send(self.path, self.body()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the call that files the operation, got {reply:?}",
            self.op_type
        );
        let id = if self.reports_id {
            operation_id(&reply)
        } else {
            filed_operation_id(emulator, self.op_type, self.application)
        };
        assert!(
            !id.is_empty(),
            "case {:?}: the call must file an operation, got {reply:?}",
            self.op_type
        );
        id
    }
}

/// The id of the operation of `op_type` filed against `application`, read out of the
/// store.
///
/// For the ONE call of this service whose output carries no `OperationId`
/// (`DeregisterApplication`): the record is filed like every other, but no implemented
/// API of this service hands its id back, so the row reads the id here and asserts
/// everything about its milestones over the wire.
fn filed_operation_id(emulator: &Emulator, op_type: &str, application: &str) -> String {
    let mut found = None;
    emulator.with_state(|state| {
        found = state
            .region_entry(REGION)
            .operations
            .values()
            .find(|record| record.op_type == op_type && record.resource_id == application)
            .map(|record| record.id.clone());
    });
    found.expect("the operation the call just filed")
}

/// Assert one listed event against the record `GetOperation` reports, which is the only
/// thing an expectation about an event is stated against.
fn assert_event(case: &str, event: &Value, record: &Reply, status: &str, timestamp_member: &str) {
    assert_eq!(
        string_field(event, "Status"),
        status,
        "case {case:?}: expected the milestone's Status, got {event:?}"
    );
    assert_eq!(
        event.get("Timestamp"),
        record.op(timestamp_member),
        "case {case:?}: the milestone must be timestamped at the record's {timestamp_member}, got {event:?}"
    );
    assert_eq!(
        resource_field(event, "ResourceArn"),
        record.op_str("ResourceArn"),
        "case {case:?}: the milestone must name the resource the record names, got {event:?}"
    );
    assert_eq!(
        resource_field(event, "ResourceType"),
        OPERATION_EVENT_RESOURCE_TYPE_APPLICATION,
        "case {case:?}: expected the application's OperationEventResourceType, got {event:?}"
    );
    let description = string_field(event, "Description");
    assert!(
        description.contains(record.op_str("Type"))
            && description.contains(record.op_str("ResourceId")),
        "case {case:?}: the description must name the operation and its resource, got {description:?}"
    );
}

/// Every kind of operation files a record whose milestones this listing reports: one
/// `IN_PROGRESS` milestone while the work runs, and a `COMPLETED` one at the instant the
/// clock brought the record to its terminal status.
#[tokio::test]
async fn reports_the_milestones_of_every_kind_of_operation() {
    for case in operation_cases() {
        let emulator = Emulator::new();
        let label = case.op_type;
        let filed_at = Instant::now();
        let operation_id = case.file(&emulator).await;

        let running = emulator.read_operation(&operation_id).await;
        let reply = emulator
            .list_events(json!({"OperationId": operation_id}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from ListOperationEvents, got {reply:?}"
        );
        // The running assertions hold only while the clock still owes the transition:
        // past that instant the record was due to end whether or not anybody listed.
        if filed_at.elapsed() < TRANSITION_SPAN {
            let events = listed(&reply);
            assert_eq!(
                statuses(&events),
                vec![OPERATION_EVENT_STATUS_IN_PROGRESS.to_string()],
                "case {label:?}: a running operation has reached its first milestone only, got {events:?}"
            );
            assert_event(label, &events[0], &running, OPERATION_EVENT_STATUS_IN_PROGRESS, "StartTime");
            assert!(
                events[0].get("StatusMessage").is_none_or(Value::is_null),
                "case {label:?}: work that has just begun has no status to explain, got {events:?}"
            );
            assert_eq!(
                next_token(&reply),
                None,
                "case {label:?}: a page that holds every milestone carries no NextToken, got {reply:?}"
            );
        }

        settle().await;
        let settled = emulator.read_operation(&operation_id).await;
        assert_eq!(
            settled.op_str("Status"),
            "SUCCESS",
            "case {label:?}: the clock owed this record its terminal status, got {settled:?}"
        );
        let reply = emulator
            .list_events(json!({"OperationId": operation_id}))
            .await;
        let events = listed(&reply);
        assert_eq!(
            statuses(&events),
            vec![
                OPERATION_EVENT_STATUS_IN_PROGRESS.to_string(),
                OPERATION_EVENT_STATUS_COMPLETED.to_string(),
            ],
            "case {label:?}: a settled operation reports both milestones, oldest first, got {events:?}"
        );
        assert_event(label, &events[0], &settled, OPERATION_EVENT_STATUS_IN_PROGRESS, "StartTime");
        assert_event(label, &events[1], &settled, OPERATION_EVENT_STATUS_COMPLETED, "EndTime");

        // The listing REPORTS: a second call reports the same milestones, and the record
        // the reader resolves is untouched by either.
        let again = emulator
            .list_events(json!({"OperationId": operation_id}))
            .await;
        assert_eq!(
            listed(&again),
            events,
            "case {label:?}: listing the milestones must not change them, got {again:?}"
        );
        let after = emulator.read_operation(&operation_id).await;
        assert_eq!(
            after.member("Operation"),
            settled.member("Operation"),
            "case {label:?}: listing the milestones must not change the record, got {after:?}"
        );
    }
}

/// A record that ended in `OperationStatus$ERROR` reports a `FAILED` milestone carrying
/// what the record explains that status with.
///
/// No API of this service files an operation that fails — every one of them succeeds once
/// the clock reaches it — so this is the one case whose record is placed in the store
/// rather than produced, the way `tests/all/sub_check_results.rs` places the records no
/// API produces. Everything the case then asserts goes over the wire.
#[tokio::test]
async fn a_failed_record_reports_a_failed_milestone() {
    let emulator = Emulator::new();
    let operation_id = emulator.registered("HDB-EVENTS-FAILED").await;
    let explanation = "the emulated work did not complete";
    let ended_at = SystemTime::now();
    emulator.with_state(|state| {
        let record = state
            .region_entry(REGION)
            .operations
            .get_mut(&operation_id)
            .expect("the operation the registration just filed");
        record.status = OPERATION_STATUS_ERROR.to_string();
        record.status_message = Some(explanation.to_string());
        record.end_time = Some(ended_at);
        record.last_updated_time = ended_at;
    });

    let record = emulator.read_operation(&operation_id).await;
    assert_eq!(
        record.op_str("Status"),
        OPERATION_STATUS_ERROR,
        "the placed record must read back as the failure it is, got {record:?}"
    );
    let reply = emulator
        .list_events(json!({"OperationId": operation_id}))
        .await;
    let events = listed(&reply);
    assert_eq!(
        statuses(&events),
        vec![
            OPERATION_EVENT_STATUS_IN_PROGRESS.to_string(),
            OPERATION_EVENT_STATUS_FAILED.to_string(),
        ],
        "case failed: a record that ended in error reports a FAILED milestone, got {events:?}"
    );
    assert_event(
        "failed",
        &events[1],
        &record,
        OPERATION_EVENT_STATUS_FAILED,
        "EndTime",
    );
    assert_eq!(
        string_field(&events[1], "StatusMessage"),
        explanation,
        "case failed: the milestone carries what the record explains its status with, got {events:?}"
    );

    // Every status the model declares is now accounted for by this suite: the two the
    // clock produces and this one.
    let covered: BTreeSet<&str> = [
        OPERATION_EVENT_STATUS_IN_PROGRESS,
        OPERATION_EVENT_STATUS_COMPLETED,
        OPERATION_EVENT_STATUS_FAILED,
    ]
    .into_iter()
    .collect();
    let modelled: BTreeSet<&str> = OPERATION_EVENT_STATUS_VALUES.iter().copied().collect();
    assert_eq!(
        covered, modelled,
        "the suite must cover every OperationEventStatus the model declares, got {covered:?}"
    );
}

/// One filter row: a label, the `Filters` the request carries, and the milestone statuses
/// the listing must then report.
struct FilterCase {
    label: &'static str,
    filters: Value,
    expected: Vec<&'static str>,
}

/// The rows, over a settled operation whose two milestones are `IN_PROGRESS` then
/// `COMPLETED`. Every filter name comes from the set this listing documents
/// (`service_com_amazonaws_ssmsap_ssmsap::state::OPERATION_EVENT_FILTER_NAMES`), and the
/// assertion below checks the rows cover all of them.
fn filter_cases(application: &str) -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "no Filters member at all",
            filters: Value::Null,
            expected: vec![
                OPERATION_EVENT_STATUS_IN_PROGRESS,
                OPERATION_EVENT_STATUS_COMPLETED,
            ],
        },
        FilterCase {
            label: "status of the first milestone",
            filters: json!([{"Name": "status", "Value": OPERATION_EVENT_STATUS_IN_PROGRESS, "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![OPERATION_EVENT_STATUS_IN_PROGRESS],
        },
        FilterCase {
            label: "status of the terminal milestone",
            filters: json!([{"Name": "status", "Value": OPERATION_EVENT_STATUS_COMPLETED, "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![OPERATION_EVENT_STATUS_COMPLETED],
        },
        FilterCase {
            label: "a status no milestone of a successful record has",
            filters: json!([{"Name": "status", "Value": OPERATION_EVENT_STATUS_FAILED, "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![],
        },
        FilterCase {
            label: "two statuses in one value, joined with an OR",
            filters: json!([{
                "Name": "status",
                "Value": format!("{OPERATION_EVENT_STATUS_FAILED},{OPERATION_EVENT_STATUS_COMPLETED}"),
                "Operator": FILTER_OPERATOR_EQUALS,
            }]),
            expected: vec![OPERATION_EVENT_STATUS_COMPLETED],
        },
        FilterCase {
            label: "the resource the operation acted on",
            filters: json!([{"Name": "resourceID", "Value": application, "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![
                OPERATION_EVENT_STATUS_IN_PROGRESS,
                OPERATION_EVENT_STATUS_COMPLETED,
            ],
        },
        FilterCase {
            label: "another resource entirely",
            filters: json!([{"Name": "resourceID", "Value": "HDB-SOMETHING-ELSE", "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![],
        },
        FilterCase {
            label: "the resource id in the wrong case, which filter values are sensitive to",
            filters: json!([{"Name": "resourceID", "Value": application.to_lowercase(), "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![],
        },
        FilterCase {
            label: "the resource type the milestones carry",
            filters: json!([{"Name": "resourceType", "Value": OPERATION_EVENT_RESOURCE_TYPE_APPLICATION, "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![
                OPERATION_EVENT_STATUS_IN_PROGRESS,
                OPERATION_EVENT_STATUS_COMPLETED,
            ],
        },
        FilterCase {
            label: "another resource type",
            filters: json!([{"Name": "resourceType", "Value": "AWS::EC2::Instance", "Operator": FILTER_OPERATOR_EQUALS}]),
            expected: vec![],
        },
        FilterCase {
            label: "three filters, which narrow together",
            filters: json!([
                {"Name": "status", "Value": OPERATION_EVENT_STATUS_COMPLETED, "Operator": FILTER_OPERATOR_EQUALS},
                {"Name": "resourceID", "Value": application, "Operator": FILTER_OPERATOR_EQUALS},
                {"Name": "resourceType", "Value": OPERATION_EVENT_RESOURCE_TYPE_APPLICATION, "Operator": FILTER_OPERATOR_EQUALS},
            ]),
            expected: vec![OPERATION_EVENT_STATUS_COMPLETED],
        },
        FilterCase {
            label: "filters that narrow to nothing together",
            filters: json!([
                {"Name": "status", "Value": OPERATION_EVENT_STATUS_COMPLETED, "Operator": FILTER_OPERATOR_EQUALS},
                {"Name": "resourceID", "Value": "HDB-SOMETHING-ELSE", "Operator": FILTER_OPERATOR_EQUALS},
            ]),
            expected: vec![],
        },
    ]
}

/// Every filter the request carries narrows the milestones reported, over the attributes
/// this listing documents.
#[tokio::test]
async fn filters_narrow_the_milestones_reported() {
    let application = "HDB-EVENTS-FILTERS";
    let emulator = Emulator::new();
    let operation_id = emulator.registered(application).await;

    let cases = filter_cases(application);
    // The rows between them name every filter this listing documents.
    let named: BTreeSet<String> = cases
        .iter()
        .filter_map(|case| case.filters.as_array())
        .flatten()
        .map(|filter| string_field(filter, "Name"))
        .collect();
    let documented: BTreeSet<String> = OPERATION_EVENT_FILTER_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        named, documented,
        "the rows must cover every documented filter name, got {named:?}"
    );

    for case in &cases {
        let mut body = json!({"OperationId": operation_id});
        if !case.filters.is_null() {
            body.as_object_mut()
                .expect("the request body is a JSON object")
                .insert("Filters".to_string(), case.filters.clone());
        }
        let reply = emulator.list_events(body).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {reply:?}",
            case.label
        );
        let events = listed(&reply);
        assert_eq!(
            statuses(&events),
            case.expected
                .iter()
                .map(|status| (*status).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected those milestones, got {events:?}",
            case.label
        );
    }
}

/// Follow the listing's pages from the first to the last, returning the milestones in the
/// order the pages reported them. Asserts the paging contract as it walks: a 200, no page
/// holding more than `max_results` milestones, and only the last page without a
/// `NextToken`.
async fn walk_pages(emulator: &Emulator, operation_id: &str, max_results: i32) -> Vec<Value> {
    let case = format!("MaxResults {max_results}");
    let mut body = json!({"OperationId": operation_id, "MaxResults": max_results});
    let mut events: Vec<Value> = Vec::new();
    // Comfortably more pages than the milestones of one operation can fill, so a paging
    // bug ends this walk instead of looping.
    for page in 0..8 {
        let reply = emulator.list_events(body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {page}: expected 200, got {reply:?}"
        );
        let rows = listed(&reply);
        assert!(
            rows.len() <= max_results as usize,
            "case {case:?} page {page}: a page holds at most MaxResults milestones, got {rows:?}"
        );
        events.extend(rows);
        match next_token(&reply) {
            None => return events,
            Some(token) => {
                body.as_object_mut()
                    .expect("the request body is a JSON object")
                    .insert("NextToken".to_string(), json!(token));
            }
        }
    }
    panic!("case {case:?}: the pages of one operation's milestones must run out");
}

/// A page holds at most `MaxResults` milestones and its `NextToken` resumes exactly where
/// it stopped, whatever the page size.
#[tokio::test]
async fn pages_resume_where_the_previous_page_stopped() {
    let emulator = Emulator::new();
    let operation_id = emulator.registered("HDB-EVENTS-PAGING").await;
    let whole = listed(
        &emulator
            .list_events(json!({"OperationId": operation_id}))
            .await,
    );
    assert!(
        whole.len() > 1,
        "a settled operation has more than one milestone to page over, got {whole:?}"
    );

    // Page sizes spanning one milestone per page, exactly the whole set, past it, and
    // each published `MaxResults` bound.
    let sizes: Vec<i32> = vec![
        1,
        whole.len() as i32,
        whole.len() as i32 + 1,
        MAX_PAGE_SIZE,
    ];
    for max_results in sizes {
        let paged = walk_pages(&emulator, &operation_id, max_results).await;
        assert_eq!(
            paged, whole,
            "case {max_results}: the pages together report the whole listing, in its own order"
        );
    }

    // A page size outside the model's declared range is refused at the boundary, on
    // either side of it.
    for max_results in [0, MAX_PAGE_SIZE + 1] {
        let reply = emulator
            .list_events(json!({"OperationId": operation_id, "MaxResults": max_results}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {max_results}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {max_results}: expected the modelled validation refusal, got {reply:?}"
        );
    }
}

/// What the listing refuses, and with which modelled error.
#[tokio::test]
async fn refuses_what_it_cannot_report() {
    let emulator = Emulator::new();
    let operation_id = emulator.registered("HDB-EVENTS-REFUSALS").await;

    // An id of the right form that names no record of this account and Region.
    let reply = emulator
        .list_events(json!({"OperationId": UNFILED_OPERATION_ID}))
        .await;
    assert_eq!(
        reply.status, 400,
        "case unfiled id: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case unfiled id: expected the modelled validation refusal, got {reply:?}"
    );
    assert_eq!(
        message(&reply),
        no_operation_message(UNFILED_OPERATION_ID),
        "case unfiled id: the refusal names the id the request carried, got {reply:?}"
    );

    // A record of THIS Region does not resolve at another regional endpoint.
    let reply = emulator
        .send_in_region(
            ELSEWHERE,
            LIST_OPERATION_EVENTS,
            json!({"OperationId": operation_id}),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "case another Region: expected 400, got {reply:?}"
    );
    assert_eq!(
        message(&reply),
        no_operation_message(&operation_id),
        "case another Region: the refusal names the id the request carried, got {reply:?}"
    );

    // An id outside the model's `OperationId` pattern is refused at the boundary.
    let reply = emulator
        .list_events(json!({"OperationId": "not-a-uuid"}))
        .await;
    assert_eq!(
        reply.status, 400,
        "case malformed id: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case malformed id: expected the modelled validation refusal, got {reply:?}"
    );

    // A filter name outside the documented set, including one that differs from a
    // documented name only in case: filter names are case-sensitive.
    for name in ["Status", "resourceId", "Description"] {
        let reply = emulator
            .list_events(json!({
                "OperationId": operation_id,
                "Filters": [{"Name": name, "Value": "anything", "Operator": FILTER_OPERATOR_EQUALS}],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case filter name {name:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case filter name {name:?}: expected the modelled validation refusal, got {reply:?}"
        );
        assert!(
            message(&reply).contains(name),
            "case filter name {name:?}: the refusal names the filter the request carried, got {reply:?}"
        );
    }

    // This operation documents ONE operator for its filters, so every other operator the
    // model declares is refused rather than applied as a comparison it does not offer.
    for operator in FILTER_OPERATOR_VALUES
        .iter()
        .filter(|operator| **operator != FILTER_OPERATOR_EQUALS)
    {
        let reply = emulator
            .list_events(json!({
                "OperationId": operation_id,
                "Filters": [{"Name": "status", "Value": OPERATION_EVENT_STATUS_COMPLETED, "Operator": operator}],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case operator {operator:?}: expected 400, got {reply:?}"
        );
        assert!(
            message(&reply).contains(operator),
            "case operator {operator:?}: the refusal names the operator the request carried, got {reply:?}"
        );
    }

    // A continuation token this service did not issue, long enough to satisfy the
    // model's own `NextToken` pattern.
    let reply = emulator
        .list_events(json!({
            "OperationId": operation_id,
            "NextToken": "not-a-token-this-service-issued",
        }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case foreign token: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case foreign token: expected the modelled validation refusal, got {reply:?}"
    );
}
