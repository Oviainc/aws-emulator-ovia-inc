// SPDX-License-Identifier: CC0-1.0

//! `PutPartnerEvents`' per-entry rejection of a `Source` the caller is not
//! whitelisted for, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.PutPartnerEvents`) through the real
//! `MultiServiceRouter`.
//!
//! ONE rule, asserted over a table of sources: this operation models no
//! top-level error for a caller that is not the registered SaaS partner of the
//! namespace its entry names (`smithy_query(query="operation",
//! operation="PutPartnerEvents")` lists only `InternalException` and
//! `OperationDisabledException`), so the call succeeds and the entry is
//! rejected on its own row — reported as what it is, an input member this
//! service will not accept: the parameter-validation `ErrorCode`, an
//! `ErrorMessage` naming the rejected member and the reason, no `EventId`, and
//! the entry counted in `FailedEntryCount`.
//!
//! The rows differ along the form of the partner name, how many segments the
//! source name carries, whether the name is in the partner namespace at all,
//! the entry's optional members, the size of the batch and the entry's position
//! in it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The per-entry `ErrorCode` of a rejected input member, spelled out here so a
/// wrong value in the implementation cannot make an assertion pass.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The input member a partner entry carries its namespace in.
const SOURCE: &str = "Source";

/// The `ErrorMessage` a rejected member is reported with: the member and the
/// reason, in this service's parameter-validation form. Written out here so the
/// implementation is not the source of the wording this suite checks.
fn rejected_member(member: &str, reason: &str) -> String {
    format!("Parameter {member} is not valid. Reason: {reason}")
}

/// Why a `Source` the caller holds no partner registration for is not accepted.
fn not_whitelisted() -> String {
    rejected_member(
        SOURCE,
        "Caller AWS account is not whitelisted for the source.",
    )
}

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so the partner registration AWS keeps outside this API can
/// be installed for the control rows below.
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

/// This emulator's account id.
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

/// Install the registration that makes this account the SaaS partner of
/// `namespace`: in AWS that happens during partner registration, outside
/// EventBridge, so it is written straight into the store the handlers read.
fn register(emulator: &Emulator, namespace: &str) {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .register_partner_namespace(namespace);
}

/// The table every row below is drawn from: source names in namespaces no
/// registration is held for. They differ along the form of the partner-name
/// segment (reverse-dns, hyphenated, carrying digits, carrying an underscore
/// and a dot, carrying capitals) and along how many segments follow it — two,
/// the fewest `EventSourceName`'s modelled pattern
/// `^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$` admits, three and four.
fn unwhitelisted_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "reverse-dns partner, the fewest segments the pattern admits",
            "aws.partner/example.com/stream",
        ),
        (
            "hyphenated partner name, three segments",
            "aws.partner/acme-analytics/orders/created",
        ),
        (
            "partner name carrying digits, four segments",
            "aws.partner/saas42/eu/orders/created",
        ),
        (
            "partner name carrying an underscore and a dot",
            "aws.partner/my_saas.io/tickets/opened",
        ),
        (
            "partner name carrying capitals",
            "aws.partner/Acme/Orders/Created",
        ),
    ]
}

/// The result entries of a `PutPartnerEvents` call that must have succeeded.
async fn put_partner_events(
    emulator: &Emulator,
    case: &str,
    entries: Vec<Value>,
) -> (u64, Vec<Value>) {
    let submitted = entries.len();
    let (status, body) = call(
        emulator,
        "PutPartnerEvents",
        &json!({ "Entries": Value::Array(entries) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: a per-entry rejection is not a top-level error, got {status} {body}"
    );
    let results = body["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        submitted,
        "case {case}: expected one result per entry, got {body}"
    );
    (
        body["FailedEntryCount"]
            .as_u64()
            .unwrap_or_else(|| panic!("case {case}: expected a FailedEntryCount, got {body}")),
        results,
    )
}

/// Assert one result row is the rejection of its `Source`: the
/// parameter-validation code, the member-and-reason message, and no `EventId`.
fn assert_rejected(case: &str, result: &Value) {
    assert_eq!(
        (
            result.get("ErrorCode").and_then(Value::as_str),
            result.get("ErrorMessage").and_then(Value::as_str),
            result.get("EventId").and_then(Value::as_str),
        ),
        (
            Some(INVALID_ARGUMENT),
            Some(not_whitelisted().as_str()),
            None,
        ),
        "case {case}: expected the rejected {SOURCE} reported as {INVALID_ARGUMENT} with {:?} \
         and no EventId, got {result}",
        not_whitelisted()
    );
}

/// Every source in a namespace this caller holds no registration for is
/// rejected on its own row, whatever optional members the entry carries. The
/// rows differ along the source name and along the entry's optional members.
#[tokio::test]
async fn an_entry_naming_an_unwhitelisted_source_is_rejected_on_its_own_row() {
    let emulator = emulator();
    for (label, source) in unwhitelisted_sources() {
        // Each row is sent three times, differing along the optional members the
        // entry carries: the verdict is about `Source` alone.
        let variants: Vec<(&str, Value)> = vec![
            (
                "the minimal entry",
                json!({ "Source": source, "DetailType": "t", "Detail": "{}" }),
            ),
            (
                "an entry carrying Time",
                json!({
                    "Source": source,
                    "DetailType": "Order Created",
                    "Detail": r#"{"amount":1}"#,
                    "Time": 1_767_225_600,
                }),
            ),
            (
                "an entry carrying Resources",
                json!({
                    "Source": source,
                    "DetailType": "Order Created",
                    "Detail": r#"{"nested":{"ok":true}}"#,
                    "Resources": [format!("arn:aws:s3:::bucket-{}", account())],
                }),
            ),
        ];
        for (variant, entry) in variants {
            let case = format!("{label}: {variant}");
            let (failed, results) = put_partner_events(&emulator, &case, vec![entry]).await;
            assert_eq!(
                failed, 1,
                "case {case}: expected FailedEntryCount 1, got {failed}"
            );
            assert_rejected(&case, &results[0]);
        }
    }
}

/// The rejection is per ENTRY: in a batch, only the rows naming a source this
/// caller is not whitelisted for fail, each in its own position, while the rows
/// naming a source it has authored are published. The batch is sent in both
/// orders, so no verdict depends on an entry's position.
#[tokio::test]
async fn a_batch_rejects_only_the_unwhitelisted_rows() {
    let authored_namespace = "aws.partner/registered.example";
    let authored = "aws.partner/registered.example/stream";

    for (order, reverse) in [("as listed", false), ("reversed", true)] {
        let emulator = emulator();
        register(&emulator, authored_namespace);
        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": authored, "Account": account() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "order {order}: CreatePartnerEventSource answered {status} {body}"
        );

        // (label, entry, whether the row is rejected for its `Source`)
        let mut rows: Vec<(String, Value, bool)> = vec![(
            "the authored source".to_string(),
            json!({ "Source": authored, "DetailType": "t", "Detail": "{}" }),
            false,
        )];
        for (label, source) in unwhitelisted_sources() {
            rows.push((
                label.to_string(),
                json!({ "Source": source, "DetailType": "t", "Detail": "{}" }),
                true,
            ));
        }
        rows.push((
            "the authored source again".to_string(),
            json!({ "Source": authored, "DetailType": "t", "Detail": r#"{"amount":2}"# }),
            false,
        ));
        if reverse {
            rows.reverse();
        }

        let case = format!("a batch of {} entries, {order}", rows.len());
        let expected_failures = rows.iter().filter(|(_, _, rejected)| *rejected).count() as u64;
        let entries: Vec<Value> = rows.iter().map(|(_, entry, _)| entry.clone()).collect();
        let (failed, results) = put_partner_events(&emulator, &case, entries).await;
        assert_eq!(
            failed, expected_failures,
            "case {case}: expected FailedEntryCount {expected_failures}, got {failed}"
        );
        for (index, (label, _, rejected)) in rows.iter().enumerate() {
            let result = &results[index];
            let row = format!("{case}, entry {index} {label:?}");
            if *rejected {
                assert_rejected(&row, result);
            } else {
                assert_eq!(
                    (
                        result.get("ErrorCode").and_then(Value::as_str),
                        result.get("ErrorMessage").and_then(Value::as_str),
                    ),
                    (None, None),
                    "case {row}: an authored source is published, got {result}"
                );
                assert!(
                    result
                        .get("EventId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty()),
                    "case {row}: a published entry is minted an EventId, got {result}"
                );
            }
        }
    }
}

/// The verdict is about the REGISTRATION, not about the name's shape: the same
/// entry that was rejected is accepted once this account holds the namespace's
/// registration and has authored the source. The rows are the sources of the
/// table.
#[tokio::test]
async fn the_same_entry_is_accepted_once_the_caller_holds_the_registration() {
    for (label, source) in unwhitelisted_sources() {
        let namespace = partner_namespace(source);
        let emulator = emulator();
        let entry = json!({ "Source": source, "DetailType": "t", "Detail": "{}" });

        let before = format!("{label}: before the registration");
        let (failed, results) = put_partner_events(&emulator, &before, vec![entry.clone()]).await;
        assert_eq!(
            failed, 1,
            "case {before}: expected FailedEntryCount 1, got {failed}"
        );
        assert_rejected(&before, &results[0]);

        register(&emulator, &namespace);
        let (status, body) = call(
            &emulator,
            "CreatePartnerEventSource",
            &json!({ "Name": source, "Account": account() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: CreatePartnerEventSource answered {status} {body}"
        );

        let after = format!("{label}: after the registration");
        let (failed, results) = put_partner_events(&emulator, &after, vec![entry]).await;
        assert_eq!(
            failed, 0,
            "case {after}: expected FailedEntryCount 0, got {failed} {results:?}"
        );
        assert!(
            results[0]
                .get("EventId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {after}: a published entry is minted an EventId, got {:?}",
            results[0]
        );
    }
}

/// `aws.partner/<partner_name>` of a source name — the unit a registration is
/// held for — built here rather than read from the implementation.
fn partner_namespace(source: &str) -> String {
    let rest = source
        .strip_prefix("aws.partner/")
        .unwrap_or_else(|| panic!("{source} is a partner source name"));
    let partner = rest.split('/').next().unwrap_or_default();
    format!("aws.partner/{partner}")
}
