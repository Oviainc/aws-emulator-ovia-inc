// SPDX-License-Identifier: CC0-1.0

//! Every CloudFront control-plane call this front door serves is a management event of
//! `cloudfront.amazonaws.com`, recorded in the account's event history.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service — whatever the operation and whatever the outcome — carrying the
//! operation's own `eventName`, the Region the service's records are filed in, `readOnly`
//! per the operation's semantics, the parameters the request sent with its `@sensitive`
//! members withheld, and as `requestID` the id the CALLER was answered under, which is the
//! join a client holding a response makes to the record of that very call. CloudFront is a
//! GLOBAL service, so the Region its records are filed in is the partition's leader Region
//! and not the one a client happened to sign for.
//!
//! The rows below span what the rule generalises over: a create, a read of one resource, a
//! listing, a tag change, a call whose request carries a `@sensitive` member, and a call
//! the service REFUSED — six operations across four resource surfaces, each asserted on
//! the record the serving call left.
//!
//! The recording lives in the SERVING service, so the log is read here straight out of the
//! shared engine; the cross-service half — the same records read back through CloudTrail's
//! `LookupEvents`, and the EventBridge fan-out of them — is
//! `core/router/tests/all/cloudfront_api_call_events.rs`.

use std::sync::Arc;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::service::management_events::{
    management_event_log, EVENT_REGION, EVENT_SOURCE,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService;
use service_common::management_events::HIDDEN_SENSITIVE_VALUE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, config, Answer, API_VERSION};
use crate::function_lifecycle::{base64, create_function, Create};

/// One emulator whose ENGINE the suite keeps, so the log the serving calls wrote can be
/// read where CloudTrail reads it.
///
/// The crate's shared harness (`distribution_lifecycle::emulator`) builds its engine
/// inside and hands back the router alone, which is all a wire assertion needs; this
/// suite asserts on the store, so it needs both.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Every record the account's log holds, as the CloudTrail event documents they render as.
fn records(engine: &Arc<engine::InMemoryEngine>) -> Vec<serde_json::Value> {
    let log = management_event_log(&**engine).expect("the engine carries the shared log");
    let held = log.lock();
    held.iter()
        .map(|event| event.to_cloudtrail_event_json())
        .collect()
}

/// The ONE record among `held` whose `requestID` is the id `answer` was returned under.
fn record_of<'a>(
    case: &str,
    held: &'a [serde_json::Value],
    answer: &Answer,
) -> &'a serde_json::Value {
    let request_id = answer.header("x-amzn-requestid");
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected the answer to name the request it answers, got none in \
         {:?}",
        answer.raw
    );
    let found: Vec<&serde_json::Value> = held
        .iter()
        .filter(|event| event.get("requestID").and_then(|id| id.as_str()) == Some(request_id))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one record of request {request_id:?}, got {found:?}"
    );
    found[0]
}

/// The string a record holds at `pointer`, or `""`.
fn text_at(record: &serde_json::Value, pointer: &str) -> String {
    record
        .pointer(pointer)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row: a call this front door serves, and what its record must say about it.
struct Row {
    case: &'static str,
    /// The operation's modelled name, which is the record's `eventName`.
    event_name: &'static str,
    /// Whether the operation only REPORTS state, which is the record's `readOnly`.
    read_only: bool,
    /// Whether the call was refused, which is what decides the record carries an
    /// `errorCode` and no `responseElements`.
    refused: bool,
    answer: Answer,
}

#[tokio::test]
async fn every_cloudfront_call_is_recorded_as_a_management_event() {
    let (router, engine) = emulator_with_engine();

    // The resource the reads, the listing and the tag change below act on.
    let created = crate::distribution_lifecycle::create_distribution(
        &router,
        &config("audited", "ref-audited").xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: CreateDistribution must succeed, got {} with {:?}",
        created.status, created.raw
    );
    let distribution_id = created
        .root()
        .find_text("Id")
        .expect("the create answers the distribution's id")
        .to_string();
    let distribution_arn = created
        .root()
        .find_text("ARN")
        .expect("the create answers the distribution's ARN")
        .to_string();

    // A function create, whose request carries the one member kind CloudFront's model
    // declares `@sensitive`: the caller's own function source.
    let function_code = b"function handler(event) { return event.request; }".to_vec();
    let function = Create {
        case: "audited-function",
        name: "audited-function".to_string(),
        comment: "audited".to_string(),
        runtime: "cloudfront-js-2.0",
        code: function_code.clone(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let function_created = create_function(&router, &function.xml()).await;
    assert_eq!(
        function_created.status, 201,
        "setup: CreateFunction must succeed, got {} with {:?}",
        function_created.status, function_created.raw
    );

    let read = crate::distribution_lifecycle::get_distribution(&router, &distribution_id).await;
    let listed = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution"),
        None,
        None,
    )
    .await;
    let tagged = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/tagging?Operation=Tag&Resource={distribution_arn}"),
        Some(
            "<Tags><Items><Tag><Key>team</Key><Value>audit</Value></Tag></Items></Tags>",
        ),
        None,
    )
    .await;
    // A REFUSED call: an id no distribution of this account carries.
    let refused =
        crate::distribution_lifecycle::get_distribution(&router, "E0000NOSUCHDIST").await;
    assert!(
        refused.status >= 400,
        "setup: a read of an absent distribution must be refused, got {} with {:?}",
        refused.status,
        refused.raw
    );

    let rows = [
        Row {
            case: "a create",
            event_name: "CreateDistribution",
            read_only: false,
            refused: false,
            answer: created,
        },
        Row {
            case: "a create whose request carries a sensitive member",
            event_name: "CreateFunction",
            read_only: false,
            refused: false,
            answer: function_created,
        },
        Row {
            case: "a read of one resource",
            event_name: "GetDistribution",
            read_only: true,
            refused: false,
            answer: read,
        },
        Row {
            case: "a listing",
            event_name: "ListDistributions",
            read_only: true,
            refused: false,
            answer: listed,
        },
        Row {
            case: "a tag change",
            event_name: "TagResource",
            read_only: false,
            refused: false,
            answer: tagged,
        },
        Row {
            case: "a refused read",
            event_name: "GetDistribution",
            read_only: true,
            refused: true,
            answer: refused,
        },
    ];

    let held = records(&engine);
    for row in &rows {
        let record = record_of(row.case, &held, &row.answer);
        let actual_name = text_at(record, "/eventName");
        assert_eq!(
            actual_name, row.event_name,
            "case {:?}: expected eventName {:?}, got {actual_name:?} in {record}",
            row.case, row.event_name
        );
        let actual_source = text_at(record, "/eventSource");
        assert_eq!(
            actual_source, EVENT_SOURCE,
            "case {:?}: expected eventSource {EVENT_SOURCE:?}, got {actual_source:?}",
            row.case
        );
        // A GLOBAL service files every record in one Region, whatever the caller signed
        // for — and the suite's own credential scope is NOT that Region by accident: the
        // shared harness signs for `us-east-1`, which is the leader Region, so the
        // assertion is on the constant and not on the scope.
        let actual_region = text_at(record, "/awsRegion");
        assert_eq!(
            actual_region, EVENT_REGION,
            "case {:?}: expected awsRegion {EVENT_REGION:?}, got {actual_region:?}",
            row.case
        );
        assert_eq!(
            record.pointer("/readOnly"),
            Some(&serde_json::json!(row.read_only)),
            "case {:?}: expected readOnly {}, got {record}",
            row.case,
            row.read_only
        );
        assert_eq!(
            record.pointer("/eventType"),
            Some(&serde_json::json!("AwsApiCall")),
            "case {:?}: a record of an API call names that type, got {record}",
            row.case
        );
        let error_code = record.get("errorCode");
        if row.refused {
            assert!(
                error_code.is_some(),
                "case {:?}: a refused call records the fault it answered, got {record}",
                row.case
            );
            assert_eq!(
                record.pointer("/responseElements"),
                Some(&serde_json::Value::Null),
                "case {:?}: a refused call answered nothing, got {record}",
                row.case
            );
        } else {
            assert!(
                error_code.is_none(),
                "case {:?}: a served call carries no error code, got {record}",
                row.case
            );
        }
    }

    // The sensitive member: present in the record, with the redaction literal in place of
    // the value, and the caller's own bytes nowhere in the whole log.
    let function_record = record_of(
        "a create whose request carries a sensitive member",
        &held,
        &rows[1].answer,
    );
    let recorded_code = text_at(function_record, "/requestParameters/functionCode");
    assert_eq!(
        recorded_code, HIDDEN_SENSITIVE_VALUE,
        "case \"a create whose request carries a sensitive member\": expected the function \
         source withheld, got {recorded_code:?} in {function_record}"
    );
    let whole_log = serde_json::Value::Array(held.clone()).to_string();
    let submitted = base64(&function_code);
    assert!(
        !whole_log.contains(&submitted),
        "the caller's function source must reach no record, found it in {whole_log}"
    );

    // The parameters a SERVED call carried are recorded as the request sent them.
    let tag_record = record_of("a tag change", &held, &rows[4].answer);
    let recorded_resource = text_at(tag_record, "/requestParameters/resource");
    assert_eq!(
        recorded_resource, distribution_arn,
        "case \"a tag change\": expected the record to name the resource the call tagged, \
         got {recorded_resource:?} in {tag_record}"
    );
}

#[tokio::test]
async fn a_read_of_the_log_leaves_it_as_it_was() {
    let (router, engine) = emulator_with_engine();
    let created = crate::distribution_lifecycle::create_distribution(
        &router,
        &config("audited-read", "ref-audited-read").xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: CreateDistribution must succeed, got {} with {:?}",
        created.status, created.raw
    );

    let before = records(&engine);
    let after = records(&engine);
    assert_eq!(
        before.len(),
        after.len(),
        "reading the log twice must report the same records, got {} then {}",
        before.len(),
        after.len()
    );
}
