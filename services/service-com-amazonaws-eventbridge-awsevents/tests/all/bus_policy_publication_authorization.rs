// SPDX-License-Identifier: CC0-1.0

//! A client's `PutEvents` is authorized against the resource policy of the event
//! bus its entries name, end to end over the wire through the real
//! `MultiServiceRouter` with SQS mounted beside EventBridge.
//!
//! The RULE asserted here: publishing an event onto a bus is the action
//! `events:PutEvents` on that bus, so the bus's own resource policy (the document
//! `PutPermission` installs and `DescribeEventBus` reports) authorizes the
//! publication under the calling identity, evaluated with the publication's own
//! condition context — and a statement that explicitly DENIES that identity the
//! action refuses the WHOLE request with HTTP 400 `AccessDeniedException`,
//! publishing no entry of it, not even one whose own members no statement names.
//! A policy that states nothing against the publication refuses nothing.
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along the `Effect` the statement states (none at all, an allow, a deny), along
//! the condition it is scoped by (no condition, each of the event-content
//! condition keys this service publishes, a wildcard comparison, a negated one,
//! IAM's `IfExists` suffix over a member the event omits), along the `Principal`
//! element (every principal, the caller's account, another principal), along the
//! `Action` element (the publication, a wildcard over the service, another
//! action), along the `Resource` element (this bus, another bus), and along the
//! BATCH the request carries (one entry, two entries of which one is named, two
//! entries of which neither is).
//!
//! A twin control runs on every row: a rule of the same bus whose target is a
//! queue, so what the request published is read where a client reads it. An
//! accepted request puts one copy per entry on that queue; a refused one puts
//! none.
//!
//! The second test is the RESTORE leg: `RemovePermission` takes the statement off
//! the bus, after which the identical batch that was refused is fully accepted and
//! every entry of it is delivered — which is what makes the absence during the
//! deny attributable to the policy rather than to the delivery path.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one
//! engine) is the one `target_dispatch` owns. The bus, rule and publication
//! helpers below are this suite's own because every call here NAMES a bus and
//! carries its own entries, where that suite's helpers address the account's
//! default bus and publish one fixed event.

use serde_json::{json, Value};

use crate::target_dispatch::{drain, emulator, events, given_queue, Emulator, REGION};

/// The bus every row publishes onto, and a second bus named only as another
/// statement's `Resource` — the publication never goes near it.
const BUS: &str = "policy-gated-bus";
const OTHER_BUS: &str = "another-bus";

/// The rule that carries the control queue, matching every source published
/// below.
const RULE: &str = "receives";
const TARGET_ID: &str = "t1";

/// The sources the rows publish under. They are three so a statement can name
/// one of them and a batch can mix a named one with an unnamed one.
const NAMED_SOURCE: &str = "com.demo.tenant-a";
const OTHER_SOURCE: &str = "com.demo.tenant-b";
const THIRD_SOURCE: &str = "com.demo.tenant-c";

/// The detail types the rows publish under, so a statement can scope itself to
/// one of them.
const NAMED_DETAIL_TYPE: &str = "Order Shipped";
const OTHER_DETAIL_TYPE: &str = "Order Created";

/// A `Detail` carrying every member AWS publishes an `events:detail.…` condition
/// key for, and two that carry other values or none — so a clause over one of
/// those keys can be met, missed, or left without a value.
const NAMED_DETAIL: &str = r#"{"service":"payments","eventTypeCode":"AWS_PAYMENTS_OPERATIONAL_ISSUE","userIdentity":{"principalId":"AIDAEXAMPLEPRINCIPAL"}}"#;
const OTHER_DETAIL: &str = r#"{"service":"shipping"}"#;
const PLAIN_DETAIL: &str = r#"{"orderId":"A-1"}"#;

/// The action a publication onto a bus performs, as a statement names it.
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The `Sid` every statement below carries, and the id `RemovePermission` takes
/// it off by.
const STATEMENT_ID: &str = "DenyOnePublisher";

/// The ARN of the bus `name` of the Region every call here is signed for, as a
/// statement's `Resource` names it and as a refusal names the resource.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The identity every request below is signed as — the one principal this
/// emulator's account holds, which `sts:GetCallerIdentity` reports as the caller
/// and which an authorization refusal therefore names.
fn caller_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The sentence AWS refuses a publication its resource policy explicitly denies
/// with: fixed text around the caller's ARN and the ARN of the bus the entry
/// named.
fn explicit_deny_message(bus: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} \
         with an explicit deny in a resource-based policy",
        caller_arn(),
        bus_arn(bus)
    )
}

/// One entry of a request's batch.
#[derive(Debug, Clone, Copy)]
struct Entry {
    source: &'static str,
    detail_type: &'static str,
    detail: &'static str,
}

impl Entry {
    /// The plain entry: the unnamed source, the unnamed detail type and a detail
    /// carrying none of the condition keys.
    const fn plain(source: &'static str) -> Self {
        Self {
            source,
            detail_type: OTHER_DETAIL_TYPE,
            detail: PLAIN_DETAIL,
        }
    }

    fn element(self) -> Value {
        json!({
            "EventBusName": BUS,
            "Source": self.source,
            "DetailType": self.detail_type,
            "Detail": self.detail,
        })
    }
}

/// A policy statement, as the rows write one.
fn statement(
    effect: &str,
    principal: Value,
    action: &str,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut body = json!({
        "Sid": STATEMENT_ID,
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": bus_arn(resource),
    });
    if let Some(condition) = condition {
        body["Condition"] = condition;
    }
    body
}

/// The statement most rows install: a `Deny` on every principal for the
/// publication onto this bus, scoped by `condition`.
fn deny(condition: Option<Value>) -> Value {
    statement("Deny", json!("*"), PUT_EVENTS_ACTION, BUS, condition)
}

/// The whole policy document carrying `statement`.
fn document(statement: &Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// Create the bus `name`.
async fn given_bus(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus {name} answered {status} {body}"
    );
}

/// A rule on [`BUS`] matching every source published below, delivering to the
/// queue `target_arn` names.
async fn given_rule(emulator: &Emulator, case: &str, target_arn: &str) {
    let pattern = json!({ "source": [NAMED_SOURCE, OTHER_SOURCE, THIRD_SOURCE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": BUS,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status} {body}"
    );
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [{ "Id": TARGET_ID, "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// Install `statement` on [`BUS`] as a whole policy document, and read it back
/// through `DescribeEventBus` so what the publication is decided against is what
/// a client can see on the bus.
async fn given_policy(emulator: &Emulator, case: &str, statement: &Value) {
    let (status, body) = events(
        emulator,
        "PutPermission",
        &json!({ "EventBusName": BUS, "Policy": document(statement) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermission answered {status} {body}"
    );
    let reported = described_policy(emulator, case).await;
    assert!(
        reported.contains(STATEMENT_ID),
        "case {case:?}: expected the installed statement on the bus, got {reported:?}"
    );
}

/// The `Policy` `DescribeEventBus` reports for [`BUS`], or the empty string when
/// it reports none.
async fn described_policy(emulator: &Emulator, case: &str) -> String {
    let (status, body) = events(emulator, "DescribeEventBus", &json!({ "Name": BUS })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeEventBus answered {status} {body}"
    );
    body["Policy"].as_str().unwrap_or_default().to_string()
}

/// Publish `entries` onto [`BUS`] in one request.
async fn put_events(emulator: &Emulator, entries: &[Entry]) -> (u16, Value) {
    let elements: Vec<Value> = entries.iter().map(|entry| entry.element()).collect();
    events(emulator, "PutEvents", &json!({ "Entries": elements })).await
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace; the empty string on a success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One row: the statement the bus carries, the batch the request publishes, and
/// whether the publication is refused.
struct Row {
    case: &'static str,
    /// `None` for a bus nothing has installed a statement on.
    statement: Option<Value>,
    entries: Vec<Entry>,
    refused: bool,
}

fn rows() -> Vec<Row> {
    let named_entry = Entry::plain(NAMED_SOURCE);
    let other_entry = Entry::plain(OTHER_SOURCE);
    let third_entry = Entry::plain(THIRD_SOURCE);
    // The condition key names the member of the event it reads, so each clause
    // below is written over the key for the member its entry carries.
    let on_source = |value: &str| json!({ "StringEquals": { "events:source": value } });
    vec![
        Row {
            case: "a bus carrying no resource policy at all",
            statement: None,
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:source, publishing the source it names",
            statement: Some(deny(Some(on_source(NAMED_SOURCE)))),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:source, publishing a source it does not name",
            statement: Some(deny(Some(on_source(NAMED_SOURCE)))),
            entries: vec![other_entry],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:source, a batch mixing the named source with another",
            statement: Some(deny(Some(on_source(NAMED_SOURCE)))),
            entries: vec![named_entry, other_entry],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:source, a batch mixing another source with the named \
                   one",
            statement: Some(deny(Some(on_source(NAMED_SOURCE)))),
            entries: vec![other_entry, named_entry],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:source, a batch of two sources it names neither of",
            statement: Some(deny(Some(on_source(NAMED_SOURCE)))),
            entries: vec![other_entry, third_entry],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:source through a StringLike wildcard",
            statement: Some(deny(Some(
                json!({ "StringLike": { "events:source": "com.demo.tenant-a*" } }),
            ))),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny scoped by StringNotEquals on events:source, publishing another source",
            statement: Some(deny(Some(
                json!({ "StringNotEquals": { "events:source": OTHER_SOURCE } }),
            ))),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny scoped by StringNotEquals on events:source, publishing that source",
            statement: Some(deny(Some(
                json!({ "StringNotEquals": { "events:source": OTHER_SOURCE } }),
            ))),
            entries: vec![other_entry],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:detail-type, publishing the detail type it names",
            statement: Some(deny(Some(
                json!({ "StringEquals": { "events:detail-type": NAMED_DETAIL_TYPE } }),
            ))),
            entries: vec![Entry {
                detail_type: NAMED_DETAIL_TYPE,
                ..named_entry
            }],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:detail-type, publishing another detail type",
            statement: Some(deny(Some(
                json!({ "StringEquals": { "events:detail-type": NAMED_DETAIL_TYPE } }),
            ))),
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:detail.service, publishing a detail naming it",
            statement: Some(deny(Some(
                json!({ "StringEquals": { "events:detail.service": "payments" } }),
            ))),
            entries: vec![Entry {
                detail: NAMED_DETAIL,
                ..named_entry
            }],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:detail.service, publishing a detail naming another \
                   service",
            statement: Some(deny(Some(
                json!({ "StringEquals": { "events:detail.service": "payments" } }),
            ))),
            entries: vec![Entry {
                detail: OTHER_DETAIL,
                ..named_entry
            }],
            refused: false,
        },
        Row {
            case: "a Deny scoped by events:detail.eventTypeCode",
            statement: Some(deny(Some(json!({
                "StringEquals": {
                    "events:detail.eventTypeCode": "AWS_PAYMENTS_OPERATIONAL_ISSUE",
                },
            })))),
            entries: vec![Entry {
                detail: NAMED_DETAIL,
                ..named_entry
            }],
            refused: true,
        },
        Row {
            case: "a Deny scoped by events:detail.userIdentity.principalId",
            statement: Some(deny(Some(json!({
                "StringEquals": {
                    "events:detail.userIdentity.principalId": "AIDAEXAMPLEPRINCIPAL",
                },
            })))),
            entries: vec![Entry {
                detail: NAMED_DETAIL,
                ..named_entry
            }],
            refused: true,
        },
        Row {
            case: "a Deny whose StringEqualsIfExists names a detail member the event omits, \
                   which IAM holds",
            statement: Some(deny(Some(
                json!({ "StringEqualsIfExists": { "events:detail.service": "payments" } }),
            ))),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny whose StringEquals names a detail member the event omits",
            statement: Some(deny(Some(
                json!({ "StringEquals": { "events:detail.service": "payments" } }),
            ))),
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "a Deny on every principal carrying no condition at all",
            statement: Some(deny(None)),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny naming the caller's own account by id",
            statement: Some(statement(
                "Deny",
                json!({ "AWS": engine::emulator_account_id() }),
                PUT_EVENTS_ACTION,
                BUS,
                None,
            )),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny naming a principal other than the caller",
            statement: Some(statement(
                "Deny",
                json!({ "AWS": format!("arn:aws:iam::{}:role/other", engine::emulator_account_id()) }),
                PUT_EVENTS_ACTION,
                BUS,
                None,
            )),
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "a Deny over every action of the service",
            statement: Some(statement("Deny", json!("*"), "events:*", BUS, None)),
            entries: vec![named_entry],
            refused: true,
        },
        Row {
            case: "a Deny over another action of the service",
            statement: Some(statement(
                "Deny",
                json!("*"),
                "events:PutRule",
                BUS,
                None,
            )),
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "a Deny naming another bus as its resource",
            statement: Some(statement(
                "Deny",
                json!("*"),
                PUT_EVENTS_ACTION,
                OTHER_BUS,
                None,
            )),
            entries: vec![named_entry],
            refused: false,
        },
        Row {
            case: "an Allow scoped by events:source, which refuses nothing",
            statement: Some(statement(
                "Allow",
                json!("*"),
                PUT_EVENTS_ACTION,
                BUS,
                Some(on_source(NAMED_SOURCE)),
            )),
            entries: vec![named_entry],
            refused: false,
        },
    ]
}

#[tokio::test]
async fn the_buss_resource_policy_authorizes_a_clients_put_events() {
    let expected_message = explicit_deny_message(BUS);
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, BUS).await;
        let (queue_url, queue_arn) = given_queue(&emulator, "published").await;
        given_rule(&emulator, case, &queue_arn).await;
        if let Some(statement) = &row.statement {
            given_policy(&emulator, case, statement).await;
        }

        let (status, body) = put_events(&emulator, &row.entries).await;

        if row.refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the publication to be refused, got {status} {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "AccessDeniedException",
                "case {case:?}: expected the code AccessDeniedException, got {code:?} in {body}"
            );
            let reported = message(&body);
            assert_eq!(
                reported, expected_message,
                "case {case:?}: expected the explicit-deny sentence {expected_message:?}, \
                 got {reported:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publication to be accepted, got {status} {body}"
            );
            assert_eq!(
                body["FailedEntryCount"],
                json!(0),
                "case {case:?}: expected no failed entry, got {body}"
            );
            let ids = body["Entries"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                ids.len(),
                row.entries.len(),
                "case {case:?}: expected one result per entry, got {body}"
            );
            for (at, entry) in ids.iter().enumerate() {
                assert!(
                    entry["EventId"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {case:?}: expected an EventId on entry {at}, got {entry}"
                );
            }
        }

        // What the request actually published, read where a client reads it: an
        // accepted request puts one copy per entry on the rule's queue, and a
        // refused one puts none — not even for an entry the statement's condition
        // does not name.
        let delivered = drain(&emulator, &queue_url).await;
        let expected = if row.refused { 0 } else { row.entries.len() };
        assert_eq!(
            delivered.len(),
            expected,
            "case {case:?}: expected {expected} published event(s) on the rule's queue, \
             got {delivered:?}"
        );
    }
}

/// `RemovePermission` takes the statement off the bus, and the identical batch
/// that was refused under it is then fully accepted and every entry delivered —
/// so the absence during the deny was the policy's doing and not the delivery
/// path's.
#[tokio::test]
async fn removing_the_denying_statement_restores_the_whole_request() {
    let case = "the deny installed, then removed";
    let emulator = emulator();
    given_bus(&emulator, case, BUS).await;
    let (queue_url, queue_arn) = given_queue(&emulator, "published").await;
    given_rule(&emulator, case, &queue_arn).await;

    // The path is live before any policy is installed: the same batch round-trips
    // to the queue.
    let batch = [Entry::plain(NAMED_SOURCE), Entry::plain(OTHER_SOURCE)];
    let (status, body) = put_events(&emulator, &batch).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the warm-up publication answered {status} {body}"
    );
    let warmed = drain(&emulator, &queue_url).await;
    assert_eq!(
        warmed.len(),
        batch.len(),
        "case {case:?}: expected the delivery path to be live, got {warmed:?}"
    );

    let denying = deny(Some(
        json!({ "StringEquals": { "events:source": NAMED_SOURCE } }),
    ));
    given_policy(&emulator, case, &denying).await;

    let (status, body) = put_events(&emulator, &batch).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the mixed batch to be refused, got {status} {body}"
    );
    let expected_message = explicit_deny_message(BUS);
    let reported = message(&body);
    assert_eq!(
        reported, expected_message,
        "case {case:?}: expected {expected_message:?}, got {reported:?}"
    );
    let suppressed = drain(&emulator, &queue_url).await;
    assert!(
        suppressed.is_empty(),
        "case {case:?}: a refused request publishes nothing, got {suppressed:?}"
    );

    // The positive control: the source the statement does not name is still
    // accepted on its own and still arrives, so the absence above is suppression
    // of the batch rather than a broken path.
    let control = [Entry::plain(OTHER_SOURCE)];
    let (status, body) = put_events(&emulator, &control).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the control publication answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: the control publication reported a failed entry: {body}"
    );
    let arrived = drain(&emulator, &queue_url).await;
    assert_eq!(
        arrived.len(),
        1,
        "case {case:?}: expected the control event on the queue, got {arrived:?}"
    );

    let (status, body) = events(
        &emulator,
        "RemovePermission",
        &json!({ "EventBusName": BUS, "StatementId": STATEMENT_ID }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RemovePermission answered {status} {body}"
    );
    let reported = described_policy(&emulator, case).await;
    assert!(
        !reported.contains(STATEMENT_ID),
        "case {case:?}: expected the statement off the bus, got {reported:?}"
    );

    let (status, body) = put_events(&emulator, &batch).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the identical batch to be accepted again, got {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected no failed entry after the removal, got {body}"
    );
    let restored = drain(&emulator, &queue_url).await;
    assert_eq!(
        restored.len(),
        batch.len(),
        "case {case:?}: expected both entries delivered after the removal, got {restored:?}"
    );
}
