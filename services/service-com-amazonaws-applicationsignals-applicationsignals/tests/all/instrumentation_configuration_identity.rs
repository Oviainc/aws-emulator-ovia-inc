// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the IDENTITY of a dynamic instrumentation configuration.
//!
//! The rule under test: a configuration is identified by its service, its environment, its
//! signal type and its location — the four parts the modelled
//! `InstrumentationConfigurationArn` spells — and `InstrumentationType` only decides "which
//! backing store to query", so it does not widen that identity. Two consequences, asserted
//! over a TABLE of identities rather than on one request:
//!
//! * a create at an identity some configuration already occupies is the modelled
//!   `ConflictException` (409) whatever instrumentation type the stored record and the new
//!   request carry, and the refusal identifies the stored record by its own ARN;
//! * one ARN therefore names at most one stored record, so the tag set addressed by that ARN
//!   is that one record's: a second create neither merges its tags into it nor can a record
//!   elsewhere take it away, and the ARN is free to be minted again once the record it names
//!   is deleted.
//!
//! The rows differ along what the rule generalises over: WHICH instrumentation type is stored
//! first and which the second create carries (both orders of the modelled pair, and the same
//! type twice), the SERVICE and ENVIRONMENT (including an environment carrying the `:` and
//! `/` the modelled `^[A-Za-z0-9:/+=,.@_-]+$` pattern admits, and the shortest the modelled
//! `length 1..255` admits), the programming LANGUAGE (the whole modelled
//! `ProgrammingLanguage` catalogue — `smithy_query(query="shape",shape="ProgrammingLanguage")`
//! -> `["Java", "Python", "Javascript"]`), which of `CodeLocation`'s optional members the
//! location supplies, and how many tags each create carries (none, one, several).
//!
//! The request helpers are the ones the other suites in this target drive this service's
//! `restJson1` surface with, so every suite speaks to one harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, tag, tag_list};
use crate::slo_lifecycle::{arn_uri_segment, call, router};

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The only `DynamicInstrumentationDeletionStatus` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationDeletionStatus")` ->
/// `["DELETED"]`).
const DELETED: &str = "DELETED";

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /get-instrumentation-configuration`.
async fn get(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/get-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /delete-instrumentation-configuration`.
async fn delete(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/delete-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// One row of the identity table: the service and environment two creates share, the location
/// the first occupies, a second location in the same scope, and the instrumentation type and
/// tags each create carries.
#[derive(Clone, Debug)]
struct Case {
    label: &'static str,
    service: &'static str,
    environment: &'static str,
    /// The `CodeLocation` body of the `Location` union the stored configuration occupies.
    location: Value,
    /// Another `CodeLocation` in the same service and environment — a DIFFERENT identity.
    other_location: Value,
    /// The instrumentation type the stored configuration carries.
    stored_type: &'static str,
    /// The instrumentation type the second create carries.
    second_type: &'static str,
    stored_tags: Vec<(String, String)>,
    second_tags: Vec<(String, String)>,
}

impl Case {
    /// A create request for `instrumentation_type` at `location`, carrying `tags`.
    fn create_body(
        &self,
        instrumentation_type: &str,
        location: &Value,
        tags: &[(String, String)],
    ) -> Value {
        json!({
            "InstrumentationType": instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "Location": { "CodeLocation": location },
            // `CaptureConfiguration`'s required union member, with only its own required
            // member (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`).
            "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
            "Tags": tag_list(tags),
        })
    }

    /// The identity a get or a delete names, by the `LocationHash` a create answered.
    fn identity_body(&self, instrumentation_type: &str, location_hash: &str) -> Value {
        json!({
            "InstrumentationType": instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "LocationIdentifier": { "LocationHash": location_hash },
        })
    }
}

/// The identities the rule is asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a BREAKPOINT stored, a PROBE created over it; Java, every location member",
            service: "payment-service",
            environment: "eks:cluster-prod/namespace",
            location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "validateCreditCard",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 42,
            }),
            other_location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "validateCreditCard",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 43,
            }),
            stored_type: BREAKPOINT,
            second_type: PROBE,
            stored_tags: vec![tag("team", "payments"), tag("tier", "gold")],
            second_tags: vec![tag("owner", "probe")],
        },
        Case {
            label: "a PROBE stored, a BREAKPOINT created over it; Python, required members only",
            service: "pricing",
            environment: "ec2:production",
            location: json!({
                "Language": "Python",
                "FilePath": "app/pricing/quote.py",
            }),
            other_location: json!({
                "Language": "Python",
                "FilePath": "app/pricing/discount.py",
            }),
            stored_type: PROBE,
            second_type: BREAKPOINT,
            stored_tags: Vec::new(),
            second_tags: vec![tag("team", "pricing")],
        },
        Case {
            label: "the same type twice; Javascript, an environment carrying ':' and '/'",
            service: "web",
            environment: "ecs:cluster/web-svc",
            location: json!({
                "Language": "Javascript",
                "ClassName": "Cart",
                "MethodName": "total",
                "FilePath": "src/checkout/cart.js",
                "LineNumber": 7,
            }),
            other_location: json!({
                "Language": "Javascript",
                "ClassName": "Cart",
                "MethodName": "total",
                "FilePath": "src/checkout/cart.js",
                "LineNumber": 8,
            }),
            stored_type: BREAKPOINT,
            second_type: BREAKPOINT,
            stored_tags: vec![tag("team", "web")],
            second_tags: vec![tag("team", "web-again"), tag("owner", "second")],
        },
        Case {
            label: "the same type twice; the shortest service and environment the model admits",
            service: "a",
            environment: "b",
            location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/Ship.java",
                "LineNumber": 1,
            }),
            other_location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/Ship.java",
                "LineNumber": 2,
            }),
            stored_type: PROBE,
            second_type: PROBE,
            stored_tags: vec![tag("team", "fulfilment")],
            second_tags: Vec::new(),
        },
    ]
}

/// Create one configuration and hand back the `ARN` and `LocationHash` the service answered —
/// the stored record's own, never a value the test derived.
async fn stored(router: &MultiServiceRouter, label: &str, body: Value) -> (String, String) {
    let (status, created) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected HTTP 200 from the create, got {status} with {created}"
    );
    let arn = created["ARN"].as_str().unwrap_or_default().to_string();
    let hash = created["LocationHash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty() && !hash.is_empty(),
        "case {label:?}: expected the create to answer an ARN and a LocationHash, got {created}"
    );
    (arn, hash)
}

/// The tags `ListTagsForResource` reports for `arn`, asserting the read itself succeeded.
async fn tags_of(router: &MultiServiceRouter, label: &str, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags_raw(router, &arn_uri_segment(arn)).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected HTTP 200 reading the tags of {arn}, got {status} with {body}"
    );
    reported_tags(&body)
}

/// A create at an identity a configuration already occupies is refused with the modelled
/// `ConflictException`, whatever instrumentation type either carries, and the stored record
/// and its tags are left exactly as they were.
#[tokio::test]
async fn a_create_at_an_occupied_identity_conflicts_whatever_the_instrumentation_type() {
    for case in cases() {
        let label = case.label;
        let router = router();
        let (arn, hash) = stored(
            &router,
            label,
            case.create_body(case.stored_type, &case.location, &case.stored_tags),
        )
        .await;

        let (status, refused) = create(
            &router,
            case.create_body(case.second_type, &case.location, &case.second_tags),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected the create at the occupied identity to conflict with 409, \
             got {status} with {refused}"
        );
        assert_eq!(
            refused["__type"].as_str(),
            Some("ConflictException"),
            "case {label:?}: expected a ConflictException, got {refused}"
        );
        assert!(
            refused["Message"]
                .as_str()
                .or_else(|| refused["message"].as_str())
                .is_some_and(|message| message.contains(&arn)),
            "case {label:?}: expected the conflict to identify the stored configuration by its \
             ARN {arn:?}, got {refused}"
        );

        // The refusal wrote nothing: the stored record is still served under the type it was
        // created with, nothing was stored under the refused type, and the tag set the ARN
        // addresses is the stored record's alone — the second create's tags were not merged
        // into it.
        let (status, fetched) = get(&router, case.identity_body(case.stored_type, &hash)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the stored configuration to survive the conflict, got \
             {status} with {fetched}"
        );
        assert_eq!(
            fetched["Configuration"]["ARN"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the stored record to keep its ARN, got {:?}",
            fetched["Configuration"]
        );
        if case.second_type != case.stored_type {
            let (status, error) = get(&router, case.identity_body(case.second_type, &hash)).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected nothing to be stored under the refused instrumentation \
                 type, got {status} with {error}"
            );
        }
        let reported = tags_of(&router, label, &arn).await;
        assert_eq!(
            reported,
            expected_map(&case.stored_tags),
            "case {label:?}: expected the stored record's own tags {:?}, got {reported:?}",
            expected_map(&case.stored_tags)
        );

        // The ARN names ONE record: once that record is deleted the ARN resolves to nothing,
        // and a create at the identity it spells is accepted again — minting the same ARN,
        // with the new record's own tags under it.
        let (status, deleted) = delete(&router, case.identity_body(case.stored_type, &hash)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the delete to succeed, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["DeletionStatus"].as_str(),
            Some(DELETED),
            "case {label:?}: expected the modelled deletion status, got {deleted}"
        );
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the deleted record's ARN to resolve to nothing, got \
             {status} with {error}"
        );

        let (second_arn, second_hash) = stored(
            &router,
            label,
            case.create_body(case.second_type, &case.location, &case.second_tags),
        )
        .await;
        assert_eq!(
            second_arn, arn,
            "case {label:?}: expected one identity to mint one ARN, got {second_arn:?} after \
             {arn:?}"
        );
        assert_eq!(
            second_hash, hash,
            "case {label:?}: expected one location to hash the same, got {second_hash:?} after \
             {hash:?}"
        );
        let reported = tags_of(&router, label, &second_arn).await;
        assert_eq!(
            reported,
            expected_map(&case.second_tags),
            "case {label:?}: expected the new record's own tags {:?}, got {reported:?}",
            expected_map(&case.second_tags)
        );
    }
}

/// Two configurations at DIFFERENT locations of one service and environment are two records
/// with two ARNs and two tag sets, whatever their instrumentation types: removing either one
/// takes its own tags and leaves the other's, and the surviving record is still served.
#[tokio::test]
async fn two_locations_hold_two_records_with_two_tag_sets() {
    for case in cases() {
        let label = case.label;
        let router = router();
        let (first_arn, first_hash) = stored(
            &router,
            label,
            case.create_body(case.stored_type, &case.location, &case.stored_tags),
        )
        .await;
        let (second_arn, second_hash) = stored(
            &router,
            label,
            case.create_body(case.second_type, &case.other_location, &case.second_tags),
        )
        .await;
        assert_ne!(
            first_arn, second_arn,
            "case {label:?}: expected two locations to mint two ARNs, got {first_arn:?} twice"
        );

        let reported = tags_of(&router, label, &second_arn).await;
        assert_eq!(
            reported,
            expected_map(&case.second_tags),
            "case {label:?}: expected the second record's own tags {:?}, got {reported:?}",
            expected_map(&case.second_tags)
        );

        let (status, deleted) =
            delete(&router, case.identity_body(case.stored_type, &first_hash)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the delete of the first record to succeed, got {status} \
             with {deleted}"
        );

        let (status, error) = list_tags_raw(&router, &arn_uri_segment(&first_arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the deleted record's ARN to resolve to nothing, got \
             {status} with {error}"
        );
        let (status, survivor) =
            get(&router, case.identity_body(case.second_type, &second_hash)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the other location's configuration to survive, got {status} \
             with {survivor}"
        );
        let reported = tags_of(&router, label, &second_arn).await;
        assert_eq!(
            reported,
            expected_map(&case.second_tags),
            "case {label:?}: expected the surviving record to keep its tags {:?}, got \
             {reported:?}",
            expected_map(&case.second_tags)
        );
    }
}
