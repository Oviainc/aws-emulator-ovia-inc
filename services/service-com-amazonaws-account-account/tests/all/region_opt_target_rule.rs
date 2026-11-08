// SPDX-License-Identifier: CC0-1.0

//! The one rule that decides whether a `RegionName` can be the TARGET of a
//! Region opt call, driven end to end through both operations that decide by
//! it: `EnableRegion` and `DisableRegion`.
//!
//! The rule: a `RegionName` the service's Region catalog does not hold is not a
//! Region code at all and is refused 400 `ValidationException` with reason
//! `fieldValidationFailed` naming the member; a Region the catalog turns on for
//! every account is not a Region-opt target and is refused 400
//! `ValidationException` with the same reason, also naming the member, and a
//! sentence of its own; every other
//! Region of the catalog is an opt target and the call proceeds. Because it is
//! ONE rule, the two operations cannot answer a given `RegionName` differently —
//! each refused case below asserts both answers are the same, byte for byte.
//!
//! Every case goes through the router, so it exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes. The tables differ along what the rule generalises over —
//! the catalog column (turned on by default, opt-in), the form of a value that
//! is not a Region code (a plausible code no Region carries, a code of another
//! partition, a Region code in the wrong case, a bare word, and the longest
//! string the modelled length admits) and the operation (both opt calls).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Regions Amazon Web Services turns on for every account, so neither opt call
/// can act on them. SOURCE: the AWS General Reference Region table "Available
/// Regions" (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>),
/// the rows marked "Enabled by default". Spelled out here rather than imported,
/// so a wrong catalog in the implementation cannot make this test pass; five
/// rows from five geographies, none of them named by the issue this suite was
/// written for.
const DEFAULT_ENABLED: [&str; 5] = [
    "ap-south-1",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "us-west-2",
];

/// Regions of the same table marked "Must be enabled": each is a Region-opt
/// target, so both opt calls proceed on it. Again five rows from five
/// geographies, none named by the issue.
const OPT_IN: [&str; 5] = [
    "ap-east-1",
    "ap-southeast-5",
    "ca-west-1",
    "me-south-1",
    "mx-central-1",
];

/// `RegionName`'s modelled `@length(min: 1, max: 50)`, from
/// `smithy_query(query="shape", shape="RegionName")`: the longest admitted
/// string is refused by the catalog rule rather than by the boundary.
const REGION_NAME_MAX: usize = 50;

/// The `ValidationExceptionReason` this rule reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `RegionOptStatus` values this suite reads back, from
/// `smithy_query(query="shape", shape="RegionOptStatus")`.
const ENABLED_BY_DEFAULT: &str = "ENABLED_BY_DEFAULT";
const DISABLED: &str = "DISABLED";
const ENABLING: &str = "ENABLING";

/// Values that are NOT Region codes of this partition, one per form the rule
/// generalises over: a plausible-looking code no Region carries, a Region code
/// of another partition (`aws-us-gov`, which this API does not opt into), a
/// catalogued Region code in the wrong case (Region codes are lower case), a
/// bare word, and the longest string the modelled length admits.
fn not_region_codes() -> Vec<String> {
    vec![
        "eu-west-9".to_string(),
        "us-gov-east-1".to_string(),
        "US-WEST-2".to_string(),
        "somewhere".to_string(),
        "r".repeat(REGION_NAME_MAX),
    ]
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The two operations that decide by the opt-target rule, each as the name this
/// suite reports it by and the URI it is reached at.
const OPT_OPERATIONS: [(&str, &str); 2] = [
    ("EnableRegion", "/enableRegion"),
    ("DisableRegion", "/disableRegion"),
];

/// Both opt calls' answers for one `RegionName`, in [`OPT_OPERATIONS`] order.
async fn opt_answers(
    router: &MultiServiceRouter,
    region_name: &str,
) -> Vec<(&'static str, Answer)> {
    let mut answers = Vec::new();
    for (operation, uri) in OPT_OPERATIONS {
        answers.push((
            operation,
            call(router, uri, json!({ "RegionName": region_name })).await,
        ));
    }
    answers
}

/// The `RegionOptStatus` the emulator reports for `region_name` on the caller's
/// own account.
async fn status_of(router: &MultiServiceRouter, region_name: &str) -> String {
    let answer = call(
        router,
        "/getRegionOptStatus",
        json!({ "RegionName": region_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {region_name:?}: GetRegionOptStatus must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("RegionOptStatus").to_string()
}

#[tokio::test]
async fn a_value_that_is_not_a_region_code_earns_the_same_refusal_from_both_opt_calls() {
    let router = emulator();

    for value in not_region_codes() {
        let answers = opt_answers(&router, &value).await;
        for (operation, answer) in &answers {
            assert_eq!(
                answer.status, 400,
                "case {value:?}/{operation}: a value that is not a Region code must answer 400, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {value:?}/{operation}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {value:?}/{operation}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
                answer.body
            );
            let fields = answer.body["fieldList"].as_array().unwrap_or_else(|| {
                panic!(
                    "case {value:?}/{operation}: the refusal must name the invalid member, \
                     got {:?}",
                    answer.body
                )
            });
            let named: Vec<&str> = fields
                .iter()
                .map(|field| field["name"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                named,
                vec!["RegionName"],
                "case {value:?}/{operation}: fieldList must name RegionName, got {:?}",
                answer.body
            );
            assert!(
                answer.member("message").contains("RegionName"),
                "case {value:?}/{operation}: the message must say which member was invalid, \
                 got {:?}",
                answer.body
            );
        }

        // One rule, so the two operations answer this `RegionName` identically.
        let (first, second) = (&answers[0].1, &answers[1].1);
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {value:?}: EnableRegion answered {} with {:?} where DisableRegion answered {} \
             with {:?}",
            first.status,
            first.body,
            second.status,
            second.body
        );
    }
}

#[tokio::test]
async fn a_region_enabled_by_default_earns_the_same_refusal_from_both_opt_calls() {
    let router = emulator();

    for region in DEFAULT_ENABLED {
        let answers = opt_answers(&router, region).await;
        for (operation, answer) in &answers {
            assert_eq!(
                answer.status, 400,
                "case {region:?}/{operation}: a Region enabled by default is not an opt target, \
                 so the call must answer 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {region:?}/{operation}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {region:?}/{operation}: expected reason {FIELD_VALIDATION_FAILED}, \
                 got {:?}",
                answer.body
            );
            assert!(
                answer.member("message").contains(region),
                "case {region:?}/{operation}: the message must name the Region, got {:?}",
                answer.body
            );
        }

        // One rule, so the two operations answer this Region identically.
        let (first, second) = (&answers[0].1, &answers[1].1);
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {region:?}: EnableRegion answered {} with {:?} where DisableRegion answered {} \
             with {:?}",
            first.status,
            first.body,
            second.status,
            second.body
        );

        // A refused call writes nothing: the status a later caller reads is the
        // one the catalog gives the Region.
        assert_eq!(
            status_of(&router, region).await,
            ENABLED_BY_DEFAULT,
            "case {region:?}: a refused opt call must leave the status alone"
        );
    }
}

#[tokio::test]
async fn an_opt_in_region_is_an_opt_target_for_both_opt_calls() {
    let router = emulator();

    for region in OPT_IN {
        assert_eq!(
            status_of(&router, region).await,
            DISABLED,
            "case {region:?}: an opt-in Region nobody enabled must report {DISABLED}"
        );

        // `DisableRegion` first, while the Region is still DISABLED: the rule
        // lets it through, and asking for the status the Region already has is
        // a no-op rather than a refusal.
        let disabled = call(&router, "/disableRegion", json!({ "RegionName": region })).await;
        assert_eq!(
            disabled.status, 200,
            "case {region:?}/DisableRegion: an opt-in Region is an opt target, so the call must \
             answer 200, got {} with {:?}",
            disabled.status, disabled.body
        );
        assert_eq!(
            status_of(&router, region).await,
            DISABLED,
            "case {region:?}: disabling an already {DISABLED} Region must leave it {DISABLED}"
        );

        // `EnableRegion` next: the rule lets it through too, and the opt-in it
        // records is visible to a later reader.
        let enabled = call(&router, "/enableRegion", json!({ "RegionName": region })).await;
        assert_eq!(
            enabled.status, 200,
            "case {region:?}/EnableRegion: an opt-in Region is an opt target, so the call must \
             answer 200, got {} with {:?}",
            enabled.status, enabled.body
        );
        assert_eq!(
            status_of(&router, region).await,
            ENABLING,
            "case {region:?}: the status straight after EnableRegion must be {ENABLING}"
        );
    }
}
