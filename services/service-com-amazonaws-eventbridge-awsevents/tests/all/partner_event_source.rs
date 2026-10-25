// SPDX-License-Identifier: CC0-1.0

//! The partner event source — the one resource of this service two accounts
//! share — end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! Three rules are asserted here, each over a TABLE of sources rather than one
//! recorded name:
//!
//! 1. Authoring a partner event source — creating, deleting, describing or
//!    listing one, and publishing to one — belongs to the SaaS partner the
//!    namespace `aws.partner/<partner_name>` is registered to. A caller holding
//!    no registration for the namespace it named is refused on that alone, before
//!    anything is looked up, so it learns nothing about what the namespace holds.
//! 2. The customer side (`DescribeEventSource`, `ActivateEventSource`,
//!    `DeactivateEventSource`) resolves a source only among the ones SHARED WITH
//!    THIS ACCOUNT: any other name reports a missing entity.
//! 3. For the namespace's registered partner, every answer is computed from the
//!    stored record: a create is readable back through both describes, the state
//!    a read reports follows the bus the customer matched the source with and the
//!    activate/deactivate calls made on it, and `PutPartnerEvents` reaches that
//!    bus's rules only while the source is `ACTIVE`.
//!
//! The rows differ along the partner name's form, how many segments the source
//! name carries, the account the source is offered to, the size of the batch
//! published, and the page size a list is asked for.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region segment of
/// every ARN these tests build.
const REGION: &str = "us-west-2";

/// How long a source that is waiting to be matched with an event bus has left,
/// spelled out here rather than imported: "if it remains in PENDING state for
/// more than two weeks, it is deleted" (`DeactivateEventSource`).
const PENDING_WINDOW: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// `EventSourceState`'s three values
/// (`smithy_query(query="shape", shape="EventSourceState")`), spelled out here so
/// a wrong value in the implementation cannot make an assertion pass.
const PENDING: &str = "PENDING";
const ACTIVE: &str = "ACTIVE";
const DELETED: &str = "DELETED";

/// One row of the source table: a label for the assertion messages, the partner
/// name the namespace is built from, and the whole source name.
struct Case {
    label: &'static str,
    partner: &'static str,
    name: &'static str,
}

/// The table every test below spans. The rows differ along the form of the
/// `partner_name` segment (a reverse-DNS name, a hyphenated one, one carrying
/// digits, underscores and dots) and along how many segments follow it — two,
/// which is the fewest `EventSourceName`'s modelled pattern
/// `^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$` admits, three and four.
fn source_cases() -> Vec<Case> {
    vec![
        Case {
            label: "reverse-dns partner, the fewest segments the pattern admits",
            partner: "example.com",
            name: "aws.partner/example.com/stream",
        },
        Case {
            label: "hyphenated partner name, three segments",
            partner: "acme-analytics",
            name: "aws.partner/acme-analytics/orders/created",
        },
        Case {
            label: "partner name carrying digits, four segments",
            partner: "saas42",
            name: "aws.partner/saas42/eu/orders/created",
        },
        Case {
            label: "partner name carrying an underscore and a dot",
            partner: "my_saas.io",
            name: "aws.partner/my_saas.io/tickets/opened",
        },
    ]
}

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so a test can read the store the handlers wrote and install
/// the partner registration AWS keeps outside this API.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// This emulator's account id — the account segment of every ARN it mints, and
/// the account a source has to be offered to for this caller to see it.
fn account() -> &'static str {
    engine::emulator_account_id()
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.us-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `arn:aws:events:<region>:<account>:event-source/<name>` — the ARN form this
/// service mints for a partner event source, built here rather than read from the
/// implementation.
fn event_source_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-source/{name}",
        account()
    )
}

/// The refusal an authoring call gets when the caller is not the namespace's
/// registered SaaS partner. Written out here, so the implementation cannot be the
/// source of the wording this test checks.
fn not_entitled(namespace: &str) -> String {
    format!("Caller is not whitelisted for the event source partner namespace {namespace}")
}

/// Install the registration that makes this account the SaaS partner of
/// `aws.partner/<partner>`: in AWS that happens during partner registration,
/// outside EventBridge, so it is written straight into the store the handlers
/// read rather than through an API call.
fn register(emulator: &Emulator, partner: &str) {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .register_partner_namespace(&format!("aws.partner/{partner}"));
}

/// Backdate the PENDING window of the source `name` by `elapsed`, so a window the
/// clock has already run out on can be observed without waiting two weeks for it.
fn age_pending_window(emulator: &Emulator, name: &str, elapsed: Duration) {
    let state = emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>();
    let mut store = state.lock();
    let record = store
        .partner_source_mut(name)
        .unwrap_or_else(|| panic!("the store holds the source {name:?}"));
    record.pending_since = record.pending_since.map(|since| since - elapsed);
}

/// The routing log, as `(EventId, bus, source, target ARNs)` per published
/// event — so what a rule's targets received is read from state rather than
/// inferred from a response.
fn delivered(emulator: &Emulator) -> Vec<(String, String, String, Vec<String>)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .iter()
        .map(|event| {
            (
                event.event_id.clone(),
                event.bus.clone(),
                event.source.clone().unwrap_or_default(),
                event.target_arns.clone(),
            )
        })
        .collect()
}

/// Every authoring call of the partner slice, as `(operation, request)` for the
/// source `case`: the five operations only an SaaS partner makes.
fn authoring_calls(case: &Case) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreatePartnerEventSource",
            json!({ "Name": case.name, "Account": account() }),
        ),
        (
            "DeletePartnerEventSource",
            json!({ "Name": case.name, "Account": account() }),
        ),
        ("DescribePartnerEventSource", json!({ "Name": case.name })),
        (
            "ListPartnerEventSources",
            json!({ "NamePrefix": format!("aws.partner/{}/", case.partner) }),
        ),
        (
            "ListPartnerEventSourceAccounts",
            json!({ "EventSourceName": case.name }),
        ),
    ]
}

/// RULE 1 — a caller holding no registration for the namespace it named is
/// refused by every authoring operation, with the namespace it may not author in
/// and nothing else. The store comes up with no registration at all, which is
/// what an ordinary account is.
#[tokio::test]
async fn every_partner_authoring_operation_refuses_a_caller_outside_the_namespace() {
    let emulator = emulator();
    for case in source_cases() {
        let expected = not_entitled(&format!("aws.partner/{}", case.partner));
        for (operation, request) in authoring_calls(&case) {
            let (status, body) = call(&emulator, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {label:?} {operation}: expected a refusal, got {status} {body}",
                label = case.label
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?} {operation}: expected ValidationException, got {body}",
                label = case.label
            );
            assert_eq!(
                message(&body),
                expected,
                "case {label:?} {operation}: expected {expected:?}, got {body}",
                label = case.label
            );
        }
        // Nothing was created by the refused calls, so a later read of the
        // namespace still finds nothing.
        assert!(
            emulator
                .engine
                .state::<ComAmazonawsEventbridgeAwseventsState>()
                .lock()
                .partner_sources
                .is_empty(),
            "case {label:?}: a refused authoring call must store nothing",
            label = case.label
        );
    }
}

/// RULE 1, the data plane — `PutPartnerEvents` fails every entry whose `Source`
/// lies in a namespace the caller is not the registered SaaS partner of, through
/// the per-entry failure channel: the CALL answers 200, each such entry carries
/// the code for a source the caller may not publish under and no `EventId`,
/// `FailedEntryCount` counts them, and nothing is routed. The rows differ along
/// the batch size and along whether an entry carries `Time` and `Resources`.
#[tokio::test]
async fn put_partner_events_fails_an_entry_outside_the_namespace_it_publishes_to() {
    let emulator = emulator();
    let cases = source_cases();
    let one = &cases[0];
    let batches: Vec<(&str, Value)> = vec![
        (
            "one minimal entry",
            json!([{ "Source": one.name, "DetailType": "t", "Detail": "{}" }]),
        ),
        (
            "one entry carrying Time and Resources",
            json!([{
                "Source": one.name,
                "DetailType": "Order Created",
                "Detail": r#"{"amount":1}"#,
                "Time": 1_767_225_600,
                "Resources": [format!("arn:aws:s3:::bucket-{}", one.partner)],
            }]),
        ),
        (
            "a batch spanning every namespace in the table",
            Value::Array(
                cases
                    .iter()
                    .map(|case| {
                        json!({
                            "Source": case.name,
                            "DetailType": "Order Created",
                            "Detail": r#"{"amount":1}"#,
                        })
                    })
                    .collect(),
            ),
        ),
        (
            "the largest batch the model admits",
            Value::Array(
                (0..20)
                    .map(|index| {
                        json!({
                            "Source": one.name,
                            "DetailType": format!("Order {index}"),
                            "Detail": r#"{"amount":1}"#,
                        })
                    })
                    .collect(),
            ),
        ),
    ];

    for (label, entries) in batches {
        let submitted = entries.as_array().map(Vec::len).unwrap_or_default();
        let (status, body) =
            call(&emulator, "PutPartnerEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutPartnerEvents answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"].as_u64(),
            Some(submitted as u64),
            "case {label:?}: expected every one of the {submitted} entries failed, got {body}"
        );
        let results = body["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            submitted,
            "case {label:?}: expected one result per entry, got {body}"
        );
        for (index, result) in results.iter().enumerate() {
            assert_eq!(
                (
                    result.get("ErrorCode").and_then(Value::as_str),
                    result.get("ErrorMessage").and_then(Value::as_str),
                    result.get("EventId").and_then(Value::as_str),
                ),
                (
                    Some("NotAuthorizedForSourceException"),
                    Some("Not authorized for the source."),
                    None
                ),
                "case {label:?} entry {index}: expected the source refusal and no EventId, got \
                 {result}"
            );
        }
        assert!(
            delivered(&emulator).is_empty(),
            "case {label:?}: a refused entry publishes nothing, got {:?}",
            delivered(&emulator)
        );
    }
}

/// RULE 2 — the customer-side operations report a source nobody shared with this
/// account as a missing entity, in the wording each of them uses: a plain read
/// names the source, and the two that act through the matching bus name both.
#[tokio::test]
async fn the_customer_side_reports_a_source_nobody_shared_as_missing() {
    let emulator = emulator();
    for case in source_cases() {
        let name = case.name;
        let expectations: Vec<(&str, Value, String)> = vec![
            (
                "DescribeEventSource",
                json!({ "Name": name }),
                format!("Event source {name} does not exist."),
            ),
            (
                "ActivateEventSource",
                json!({ "Name": name }),
                format!("Event source {name} or event bus {name} does not exist."),
            ),
            (
                "DeactivateEventSource",
                json!({ "Name": name }),
                format!("Event source {name} or event bus {name} does not exist."),
            ),
        ];
        for (operation, request, expected) in expectations {
            let (status, body) = call(&emulator, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {label:?} {operation}: expected a refusal, got {status} {body}",
                label = case.label
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?} {operation}: expected ResourceNotFoundException, got {body}",
                label = case.label
            );
            assert_eq!(
                message(&body),
                expected,
                "case {label:?} {operation}: expected {expected:?}, got {body}",
                label = case.label
            );
        }
    }
}

/// RULE 2, the other half — a source the partner authored for ANOTHER account is
/// not shared with this one, so it reads exactly like one that was never
/// authored, while the same source offered to this account is reported.
#[tokio::test]
async fn a_source_offered_to_another_account_is_not_shared_with_this_one() {
    for case in source_cases() {
        for (label, offered_to, shared) in [
            ("offered to this account", account().to_string(), true),
            ("offered to another account", "210987654321".to_string(), false),
        ] {
            let emulator = emulator();
            register(&emulator, case.partner);
            let (status, body) = call(
                &emulator,
                "CreatePartnerEventSource",
                &json!({ "Name": case.name, "Account": offered_to }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: CreatePartnerEventSource answered {status} {body}",
                case = case.label
            );

            let (status, body) = call(
                &emulator,
                "DescribeEventSource",
                &json!({ "Name": case.name }),
            )
            .await;
            if shared {
                assert_eq!(
                    status, 200,
                    "case {case} {label}: DescribeEventSource answered {status} {body}",
                    case = case.label
                );
                assert_eq!(
                    body["Name"].as_str(),
                    Some(case.name),
                    "case {case} {label}: expected the source's own name, got {body}",
                    case = case.label
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} {label}: expected a refusal, got {status} {body}",
                    case = case.label
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case} {label}: expected ResourceNotFoundException, got {body}",
                    case = case.label
                );
            }

            // Either way the PARTNER sees its own source, which is what tells
            // the two refusals apart: not shared is not the same as not there.
            let (status, body) = call(
                &emulator,
                "DescribePartnerEventSource",
                &json!({ "Name": case.name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: DescribePartnerEventSource answered {status} {body}",
                case = case.label
            );
            assert_eq!(
                body["Arn"].as_str(),
                Some(event_source_arn(case.name).as_str()),
                "case {case} {label}: expected the stored source's ARN, got {body}",
                case = case.label
            );

            // The customer-side list follows the same rule as the read: it
            // carries the source only when it was shared with this account.
            let (status, body) = call(&emulator, "ListEventSources", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case} {label}: ListEventSources answered {status} {body}",
                case = case.label
            );
            let listed = body["EventSources"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                listed,
                usize::from(shared),
                "case {case} {label}: expected {expected} shared source(s) listed, got {body}",
                case = case.label,
                expected = usize::from(shared)
            );
        }
    }
}

/// RULE 3 — for the namespace's registered partner the whole flow computes from
/// the stored record: the create is readable back through both describes and both
/// lists, the state follows the bus the customer matched the source with and the
/// activate/deactivate calls, and a second create under the same name is refused
/// as an existing resource.
#[tokio::test]
async fn a_registered_partner_authors_a_source_the_customer_reads_back() {
    for case in source_cases() {
        let emulator = emulator();
        register(&emulator, case.partner);

        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": case.name, "Account": account() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreatePartnerEventSource answered {status} {body}",
            label = case.label
        );
        let created_arn = body["EventSourceArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            created_arn,
            event_source_arn(case.name),
            "case {label:?}: expected the minted ARN {expected:?}, got {body}",
            label = case.label,
            expected = event_source_arn(case.name)
        );

        // ROUND TRIP, partner side: the create is readable back under the ARN it
        // reported.
        let (status, body) = call(
            &emulator,
            "DescribePartnerEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribePartnerEventSource answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            (body["Arn"].as_str(), body["Name"].as_str()),
            (Some(created_arn.as_str()), Some(case.name)),
            "case {label:?}: expected the created source read back, got {body}",
            label = case.label
        );

        // ROUND TRIP, customer side: the source is shared with this account and
        // still waiting to be matched with a bus, so it is PENDING and carries
        // the end of that window.
        let (status, body) = call(
            &emulator,
            "DescribeEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeEventSource answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            body["State"].as_str(),
            Some(PENDING),
            "case {label:?}: an unmatched source is {PENDING}, got {body}",
            label = case.label
        );
        assert_eq!(
            body["CreatedBy"].as_str(),
            Some(case.partner),
            "case {label:?}: expected the partner that created it, got {body}",
            label = case.label
        );
        let creation = body["CreationTime"].as_f64().unwrap_or_default();
        let expiration = body["ExpirationTime"].as_f64().unwrap_or_default();
        let window = expiration - creation;
        assert!(
            (window - PENDING_WINDOW.as_secs_f64()).abs() < 1.0,
            "case {label:?}: expected the PENDING window {expected} seconds wide, got {window} \
             ({body})",
            label = case.label,
            expected = PENDING_WINDOW.as_secs()
        );

        // The customer-side LIST reports the same source with the same members,
        // read out of the same record.
        let (status, listed) = call(
            &emulator,
            "ListEventSources",
            &json!({ "NamePrefix": format!("aws.partner/{}/", case.partner) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListEventSources answered {status} {listed}",
            label = case.label
        );
        assert_eq!(
            listed["EventSources"],
            json!([body]),
            "case {label:?}: expected the shared source listed exactly as the read reports it, \
             got {listed}",
            label = case.label
        );

        // The accounts list reports the one account the source was offered to,
        // with the same state and window the customer read.
        let (status, body) = call(
            &emulator,
            "ListPartnerEventSourceAccounts",
            &json!({ "EventSourceName": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListPartnerEventSourceAccounts answered {status} {body}",
            label = case.label
        );
        let accounts = body["PartnerEventSourceAccounts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            accounts.len(),
            1,
            "case {label:?}: expected exactly the one account the source was offered to, got \
             {body}",
            label = case.label
        );
        assert_eq!(
            (
                accounts[0]["Account"].as_str(),
                accounts[0]["State"].as_str()
            ),
            (Some(account()), Some(PENDING)),
            "case {label:?}: expected the offered account PENDING, got {body}",
            label = case.label
        );

        // The namespace's own list reports it too.
        let (status, body) = call(
            &emulator,
            "ListPartnerEventSources",
            &json!({ "NamePrefix": format!("aws.partner/{}/", case.partner) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListPartnerEventSources answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            body["PartnerEventSources"],
            json!([{ "Arn": created_arn, "Name": case.name }]),
            "case {label:?}: expected the created source listed, got {body}",
            label = case.label
        );

        // "This name must be unique": the second create under the stored name is
        // refused and leaves the record alone.
        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": case.name, "Account": "210987654321" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the second create refused, got {status} {body}",
            label = case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {label:?}: expected ResourceAlreadyExistsException, got {body}",
            label = case.label
        );
        let (status, body) = call(
            &emulator,
            "ListPartnerEventSourceAccounts",
            &json!({ "EventSourceName": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListPartnerEventSourceAccounts answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            body["PartnerEventSourceAccounts"][0]["Account"].as_str(),
            Some(account()),
            "case {label:?}: a refused create must not re-offer the source, got {body}",
            label = case.label
        );
    }
}

/// RULE 3 — the state a source reports follows the matching bus and the
/// activate/deactivate calls made on it, and `PutPartnerEvents` reaches that
/// bus's rules exactly while the source is `ACTIVE`.
#[tokio::test]
async fn the_matching_bus_and_the_activation_calls_decide_what_is_published() {
    for case in source_cases() {
        let emulator = emulator();
        register(&emulator, case.partner);
        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": case.name, "Account": account() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreatePartnerEventSource answered {status} {body}",
            label = case.label
        );

        // An entry published before the customer matched the source with a bus
        // is accepted and reaches nothing: the customer "can use that event bus
        // to receive events from the partner" only once it has created it.
        let entry = json!({
            "Source": case.name,
            "DetailType": "Order Created",
            "Detail": r#"{"amount":1}"#,
        });
        let (status, body) = call(
            &emulator,
            "PutPartnerEvents",
            &json!({ "Entries": [entry.clone()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutPartnerEvents answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            body["FailedEntryCount"].as_i64(),
            Some(0),
            "case {label:?}: an entry on an authored source is accepted, got {body}",
            label = case.label
        );
        assert!(
            delivered(&emulator).is_empty(),
            "case {label:?}: with no matching bus nothing is routed, got {:?}",
            delivered(&emulator),
            label = case.label
        );

        // The customer matches the source with a bus, which starts its
        // ingestion, and puts a rule with a target on it.
        let (status, body) = call(
            &emulator,
            "CreateEventBus",
            &json!({ "Name": case.name, "EventSourceName": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateEventBus answered {status} {body}",
            label = case.label
        );
        let queue_arn = format!("arn:aws:sqs:{REGION}:{}:partner-probe", account());
        for (operation, request) in [
            (
                "PutRule",
                json!({
                    "Name": "partner-catch-all",
                    "EventBusName": case.name,
                    "EventPattern": format!(r#"{{"source":["{}"]}}"#, case.name),
                }),
            ),
            (
                "PutTargets",
                json!({
                    "Rule": "partner-catch-all",
                    "EventBusName": case.name,
                    "Targets": [{ "Id": "queue", "Arn": queue_arn }],
                }),
            ),
        ] {
            let (status, body) = call(&emulator, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {operation} answered {status} {body}",
                label = case.label
            );
        }

        // Matched, so ACTIVE, and the PENDING window is over.
        let (status, body) = call(
            &emulator,
            "DescribeEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeEventSource answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            body["State"].as_str(),
            Some(ACTIVE),
            "case {label:?}: a matched source is {ACTIVE}, got {body}",
            label = case.label
        );
        assert_eq!(
            body["ExpirationTime"].as_f64(),
            None,
            "case {label:?}: a matched source does not expire, got {body}",
            label = case.label
        );

        // Now the same entry reaches the rule's target on that bus.
        let (status, body) = call(
            &emulator,
            "PutPartnerEvents",
            &json!({ "Entries": [entry.clone()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutPartnerEvents answered {status} {body}",
            label = case.label
        );
        let event_id = body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !event_id.is_empty(),
            "case {label:?}: an accepted entry carries an EventId, got {body}",
            label = case.label
        );
        assert_eq!(
            delivered(&emulator),
            vec![(
                event_id,
                case.name.to_string(),
                case.name.to_string(),
                vec![format!("arn:aws:sqs:{REGION}:{}:partner-probe", account())],
            )],
            "case {label:?}: expected the entry routed to the rule's target",
            label = case.label
        );

        // Deactivating the source stops that ingestion without deleting the bus,
        // and puts the source back into PENDING.
        let (status, body) = call(
            &emulator,
            "DeactivateEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeactivateEventSource answered {status} {body}",
            label = case.label
        );
        let (_, body) = call(
            &emulator,
            "DescribeEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            body["State"].as_str(),
            Some(PENDING),
            "case {label:?}: a deactivated source is {PENDING}, got {body}",
            label = case.label
        );
        let before = delivered(&emulator);
        let (status, body) = call(
            &emulator,
            "PutPartnerEvents",
            &json!({ "Entries": [entry.clone()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: PutPartnerEvents answered {status} {body}",
            label = case.label
        );
        assert_eq!(
            delivered(&emulator),
            before,
            "case {label:?}: a deactivated source receives no events",
            label = case.label
        );

        // Activating it again resumes ingestion onto the same bus.
        let (status, body) = call(
            &emulator,
            "ActivateEventSource",
            &json!({ "Name": case.name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ActivateEventSource answered {status} {body}",
            label = case.label
        );
        let (_, body) = call(
            &emulator,
            "PutPartnerEvents",
            &json!({ "Entries": [entry] }),
        )
        .await;
        let resumed = body["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            delivered(&emulator).len(),
            before.len() + 1,
            "case {label:?}: a reactivated source receives events again ({resumed})",
            label = case.label
        );
    }
}

/// RULE 3 — deleting the source while the customer's bus still exists leaves it
/// visible in the one state that says so, and nothing can be activated or
/// published through it afterwards; deleting one nobody matched removes it
/// outright, and deleting what is already gone is answered rather than refused.
#[tokio::test]
async fn deleting_a_source_reports_deleted_while_the_matching_bus_exists() {
    for case in source_cases() {
        for (label, matched) in [("matched with a bus", true), ("never matched", false)] {
            let emulator = emulator();
            register(&emulator, case.partner);
            let (status, body) = call(
                &emulator,
                "CreatePartnerEventSource",
                &json!({ "Name": case.name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: CreatePartnerEventSource answered {status} {body}",
                case = case.label
            );
            if matched {
                let (status, body) = call(
                    &emulator,
                    "CreateEventBus",
                    &json!({ "Name": case.name, "EventSourceName": case.name }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case} {label}: CreateEventBus answered {status} {body}",
                    case = case.label
                );
            }

            let (status, body) = call(
                &emulator,
                "DeletePartnerEventSource",
                &json!({ "Name": case.name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: DeletePartnerEventSource answered {status} {body}",
                case = case.label
            );

            let (status, body) = call(
                &emulator,
                "DescribeEventSource",
                &json!({ "Name": case.name }),
            )
            .await;
            if matched {
                assert_eq!(
                    status, 200,
                    "case {case} {label}: DescribeEventSource answered {status} {body}",
                    case = case.label
                );
                assert_eq!(
                    body["State"].as_str(),
                    Some(DELETED),
                    "case {case} {label}: a withdrawn source reads {DELETED}, got {body}",
                    case = case.label
                );
                // No state this source could be put in is a valid one any more.
                for operation in ["ActivateEventSource", "DeactivateEventSource"] {
                    let (status, body) =
                        call(&emulator, operation, &json!({ "Name": case.name })).await;
                    assert_eq!(
                        status, 400,
                        "case {case} {label} {operation}: expected a refusal, got {status} {body}",
                        case = case.label
                    );
                    assert_eq!(
                        error_code(&body),
                        "InvalidStateException",
                        "case {case} {label} {operation}: expected InvalidStateException, got \
                         {body}",
                        case = case.label
                    );
                }
                let (status, body) = call(
                    &emulator,
                    "PutPartnerEvents",
                    &json!({ "Entries": [{
                        "Source": case.name,
                        "DetailType": "Order Created",
                        "Detail": "{}",
                    }] }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case} {label}: PutPartnerEvents answered {status} {body}",
                    case = case.label
                );
                assert_eq!(
                    body["Entries"][0]["ErrorCode"].as_str(),
                    Some("ResourceNotFoundException"),
                    "case {case} {label}: a withdrawn source publishes nothing, got {body}",
                    case = case.label
                );
                assert!(
                    delivered(&emulator).is_empty(),
                    "case {case} {label}: a withdrawn source receives nothing, got {:?}",
                    delivered(&emulator),
                    case = case.label
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case} {label}: expected a refusal, got {status} {body}",
                    case = case.label
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case} {label}: expected ResourceNotFoundException, got {body}",
                    case = case.label
                );
            }

            // Deleting what is already gone asks for the state the store is in,
            // and the model carries no missing-resource error for it.
            let (status, body) = call(
                &emulator,
                "DeletePartnerEventSource",
                &json!({ "Name": case.name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: a repeated delete answered {status} {body}",
                case = case.label
            );

            // A withdrawn name is free again: creating it offers the source
            // afresh, and the customer reads it as waiting to be matched rather
            // than as the withdrawn one.
            let (status, body) = call(
                &emulator,
                "CreatePartnerEventSource",
                &json!({ "Name": case.name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: re-creating the withdrawn source answered {status} {body}",
                case = case.label
            );
            let (status, body) = call(
                &emulator,
                "DescribeEventSource",
                &json!({ "Name": case.name }),
            )
            .await;
            assert_eq!(
                (status, body["State"].as_str()),
                (200, Some(PENDING)),
                "case {case} {label}: expected the re-created source {PENDING}, got {status} \
                 {body}",
                case = case.label
            );
        }
    }
}

/// RULE 3 — a source nobody matched with a bus is gone once its PENDING window
/// has run out, whether or not anybody called in the meantime: the window is a
/// due instant on the record, so a read at the far end of it reports the source
/// as missing and the partner's own list no longer carries it.
#[tokio::test]
async fn a_source_nobody_matched_expires_at_the_end_of_its_pending_window() {
    for case in source_cases() {
        for (label, elapsed, still_there) in [
            ("one second inside the window", PENDING_WINDOW - Duration::from_secs(1), true),
            ("one second past the window", PENDING_WINDOW + Duration::from_secs(1), false),
        ] {
            let emulator = emulator();
            register(&emulator, case.partner);
            let (status, body) = call(
                &emulator,
                "CreatePartnerEventSource",
                &json!({ "Name": case.name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: CreatePartnerEventSource answered {status} {body}",
                case = case.label
            );
            age_pending_window(&emulator, case.name, elapsed);

            let (status, body) = call(
                &emulator,
                "DescribeEventSource",
                &json!({ "Name": case.name }),
            )
            .await;
            let expected_status = if still_there { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case {case} {label}: expected {expected_status}, got {status} {body}",
                case = case.label
            );

            let (status, body) = call(
                &emulator,
                "ListPartnerEventSources",
                &json!({ "NamePrefix": format!("aws.partner/{}/", case.partner) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} {label}: ListPartnerEventSources answered {status} {body}",
                case = case.label
            );
            let listed = body["PartnerEventSources"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                listed,
                usize::from(still_there),
                "case {case} {label}: expected {expected} source(s) listed, got {body}",
                case = case.label,
                expected = usize::from(still_there)
            );
        }
    }
}

/// RULE 3 — `ListPartnerEventSources` pages in name order: the prefix selects the
/// namespace's sources, `Limit` bounds a page, and following `NextToken` walks
/// every source exactly once. The rows differ along the page size asked for,
/// including one above the stored count and one at the modelled maximum.
#[tokio::test]
async fn listing_a_namespace_pages_through_every_source_once() {
    let partner = "paging.example";
    let names: Vec<String> = (0..7)
        .map(|index| format!("aws.partner/{partner}/stream/{index:02}"))
        .collect();
    // A source in ANOTHER namespace, which the prefix must exclude.
    let other = "aws.partner/other.example/stream/00";

    for page_size in [1_i64, 2, 3, 7, 50, 100] {
        let emulator = emulator();
        register(&emulator, partner);
        register(&emulator, "other.example");
        for name in names.iter().chain(std::iter::once(&other.to_string())) {
            let (status, body) = call(
                &emulator,
                "CreatePartnerEventSource",
                &json!({ "Name": name, "Account": account() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "page size {page_size}: creating {name} answered {status} {body}"
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "NamePrefix": format!("aws.partner/{partner}/"),
                "Limit": page_size,
            });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&emulator, "ListPartnerEventSources", &request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListPartnerEventSources answered {status} {body}"
            );
            let page = body["PartnerEventSources"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "page size {page_size}: a page carried {} sources, got {body}",
                page.len()
            );
            for source in &page {
                seen.push(source["Name"].as_str().unwrap_or_default().to_string());
            }
            token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, names,
            "page size {page_size}: expected every source of the namespace once, in name order"
        );
    }
}

/// RULE 3, per entry — a batch from a registered partner is judged entry by
/// entry: an entry naming no event, one whose `Detail` is not a JSON object, one
/// past the size the entry documents, one in a namespace this partner is not
/// registered for and one naming a source it has not authored each fail on their
/// own row, while the entries beside them are published. The batch is sent in
/// both orders, so no verdict depends on an entry's position.
#[tokio::test]
async fn a_partner_batch_fails_only_the_entries_that_cannot_be_published() {
    let case = &source_cases()[0];
    let unauthored = "aws.partner/example.com/no-such-stream";
    let foreign = "aws.partner/other.example/stream";

    for (order, reverse) in [("as listed", false), ("reversed", true)] {
        let emulator = emulator();
        register(&emulator, case.partner);
        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": case.name, "Account": account() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "order {order}: CreatePartnerEventSource answered {status} {body}"
        );

        // (label, entry, the ErrorCode expected for it, or None when it is
        // published)
        let mut rows: Vec<(&str, Value, Option<&str>)> = vec![
            (
                "an authored source",
                json!({ "Source": case.name, "DetailType": "t", "Detail": "{}" }),
                None,
            ),
            (
                "no Detail at all",
                json!({ "Source": case.name, "DetailType": "t" }),
                Some("InvalidArgument"),
            ),
            (
                "a Detail that is not a JSON object",
                json!({ "Source": case.name, "DetailType": "t", "Detail": "[1,2]" }),
                Some("MalformedDetail"),
            ),
            (
                "a DetailType past the 128 characters the entry documents",
                json!({
                    "Source": case.name,
                    "DetailType": "d".repeat(129),
                    "Detail": "{}",
                }),
                Some("InvalidArgument"),
            ),
            (
                "a source in another partner's namespace",
                json!({ "Source": foreign, "DetailType": "t", "Detail": "{}" }),
                Some("NotAuthorizedForSourceException"),
            ),
            (
                "a source this partner never authored",
                json!({ "Source": unauthored, "DetailType": "t", "Detail": "{}" }),
                Some("ResourceNotFoundException"),
            ),
            (
                "the authored source again, carrying Resources",
                json!({
                    "Source": case.name,
                    "DetailType": "t",
                    "Detail": r#"{"amount":2}"#,
                    "Resources": ["arn:aws:s3:::probe"],
                }),
                None,
            ),
        ];
        if reverse {
            rows.reverse();
        }

        let entries: Vec<Value> = rows.iter().map(|(_, entry, _)| entry.clone()).collect();
        let expected_failures = rows.iter().filter(|(_, _, code)| code.is_some()).count();
        let (status, body) =
            call(&emulator, "PutPartnerEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 200,
            "order {order}: PutPartnerEvents answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"].as_u64(),
            Some(expected_failures as u64),
            "order {order}: expected FailedEntryCount {expected_failures}, got {body}"
        );
        let results = body["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            rows.len(),
            "order {order}: expected one result per entry ({}), got {body}",
            rows.len()
        );
        for (index, (label, _, expected_code)) in rows.iter().enumerate() {
            let result = &results[index];
            assert_eq!(
                result.get("ErrorCode").and_then(Value::as_str),
                *expected_code,
                "order {order} entry {index} {label:?}: expected ErrorCode {expected_code:?}, \
                 got {result}"
            );
            let event_id = result.get("EventId").and_then(Value::as_str);
            assert_eq!(
                event_id.is_some(),
                expected_code.is_none(),
                "order {order} entry {index} {label:?}: an EventId belongs to an accepted entry \
                 alone, got {result}"
            );
        }
    }
}

