// SPDX-License-Identifier: CC0-1.0

//! Two rules about every Free Tier response, driven over the real wire (`POST /`
//! with `X-Amz-Target: AWSFreeTierService.<Operation>`) through the same
//! multi-service router the emulator boots.
//!
//! 1. The media type a response ANNOUNCES. Every response of this service
//!    announces `application/x-amz-json-1.1`, whatever the client sent and
//!    whatever the outcome: a success, a modelled failure its handler raises, a
//!    request the framework refused in front of the handler, and a target naming
//!    no modelled operation alike. The refusal classes are spanned in
//!    `refused_request_media_type.rs`; the rows here span the operations and the
//!    media type the REQUEST announced.
//!
//! 2. What a REPORTING operation answers for a resource the store does not hold.
//!    `GetAccountPlanState`'s output is required members of one stored record, so
//!    an account with no account-plan data is answered with the modelled
//!    `ResourceNotFoundException` naming that account — and reading it, however
//!    often and alongside whatever else, never brings a plan into being. Only
//!    `UpgradeAccountPlan`, the service's one plan-writing operation, does.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    ComAmazonawsFreetierAwsfreetierserviceState, PLAN_STATUS_ACTIVE, PLAN_TYPE_FREE, PLAN_TYPE_PAID,
};
use service_com_amazonaws_freetier_awsfreetierservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The media type every response of this service announces, read from the service
/// itself so the expectation and the implementation share one source.
const SERVICE_MEDIA_TYPE: &str =
    service_com_amazonaws_freetier_awsfreetierservice::service::announced_media_type::RESPONSE_MEDIA_TYPE
        .announced();

/// The media type the model's `aws.protocols#awsJson1_0` protocol trait declares.
/// A client may send it — the JSON framing of the two revisions is identical — and
/// no response announces it.
const MODEL_MEDIA_TYPE: &str =
    service_com_amazonaws_freetier_awsfreetierservice::service::announced_media_type::RESPONSE_MEDIA_TYPE
        .implied();

/// The emulator under test: this service mounted on one shared engine, plus that
/// engine, so a test can look at the store the handlers wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // In-process tests sign with a placeholder key; signature verification is
        // a front-door setting, not part of what these tests are about.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one request and report the response's status, announced media
    /// type and JSON body. `target` is the full `X-Amz-Target` value, so a test
    /// can name an operation the service does not model; `request_media_type` is
    /// `None` to send no `content-type` at all.
    async fn dispatch(
        &self,
        target: &str,
        body: &Value,
        request_media_type: Option<&str>,
    ) -> (u16, String, Value) {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("https://freetier.us-east-1.amazonaws.com/")
            .header("x-amz-target", target)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/us-east-1/freetier/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000",
            );
        if let Some(media_type) = request_media_type {
            request = request.header("content-type", media_type);
        }
        let request = request
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let media_type = response
            .headers()
            .get("content-type")
            .map(|value| value.to_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, media_type, payload)
    }

    /// Call one modelled operation of this service.
    async fn call(&self, operation: &str, body: &Value) -> (u16, String, Value) {
        self.dispatch(
            &format!("AWSFreeTierService.{operation}"),
            body,
            Some(MODEL_MEDIA_TYPE),
        )
        .await
    }

    /// Call one operation, requiring HTTP 200.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, _, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
        );
        payload
    }

    /// Whether the store holds an account-plan record for `account_id`.
    fn holds_a_plan(&self, account_id: &str) -> bool {
        self.engine
            .state::<ComAmazonawsFreetierAwsfreetierserviceState>()
            .lock()
            .plans
            .contains_key(account_id)
    }

    /// Record that the account's payment instrument has been vetted, which is
    /// what the move onto the paid plan is conditional on (that gate's own rule
    /// is pinned in `paid_plan_vetting.rs`).
    fn vet_payment_instrument(&self) {
        self.engine
            .state::<ComAmazonawsFreetierAwsfreetierserviceState>()
            .lock()
            .vet_payment_instrument(engine::emulator_account_id());
    }
}

/// The wire error code (`__type`) of an error body, without its namespace.
fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The string at `member`, required to be present.
fn text(payload: &Value, member: &str) -> String {
    payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string at {member}, got {payload}"))
        .to_string()
}

/// One row of the media-type table.
struct MediaTypeCase {
    /// What distinguishes this row.
    name: &'static str,
    /// The full `X-Amz-Target` value the request carries.
    target: &'static str,
    /// The request body.
    body: Value,
    /// The `content-type` the request announces, or `None` for none at all.
    request_media_type: Option<&'static str>,
    /// Whether the account's plan is established before the call, so a row can
    /// exercise the success path of a reporting operation.
    establish_plan: bool,
    expected_status: u16,
    expected_media_type: &'static str,
}

/// Every response this service produces announces one media type, across the
/// operations, the outcomes and whatever the client itself announced; only the
/// framework's own refusal and routing envelopes keep the model-derived one.
#[tokio::test]
async fn every_response_announces_the_services_media_type() {
    // Rows differ in operation, in outcome (success / a modelled failure the
    // handler raises / a refusal in front of the service / an unroutable
    // target), and in what media type the request itself announced.
    let cases: Vec<MediaTypeCase> = vec![
        MediaTypeCase {
            name: "GetFreeTierUsage success, awsJson1_0 request",
            target: "AWSFreeTierService.GetFreeTierUsage",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetFreeTierUsage success, awsJson1_1 request",
            target: "AWSFreeTierService.GetFreeTierUsage",
            body: json!({"maxResults": 2}),
            request_media_type: Some(SERVICE_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetFreeTierUsage success, no request media type",
            target: "AWSFreeTierService.GetFreeTierUsage",
            body: json!({}),
            request_media_type: None,
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetFreeTierUsage filtered success",
            target: "AWSFreeTierService.GetFreeTierUsage",
            body: json!({"filter": {"Dimensions": {
                "Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]
            }}}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetFreeTierUsage, a modelled failure the handler raises",
            target: "AWSFreeTierService.GetFreeTierUsage",
            body: json!({"filter": {"Dimensions": {
                "Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS", "CONTAINS"]
            }}}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 400,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetAccountPlanState, an account with no plan data",
            target: "AWSFreeTierService.GetAccountPlanState",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 400,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "GetAccountPlanState success",
            target: "AWSFreeTierService.GetAccountPlanState",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: true,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "ListAccountActivities success",
            target: "AWSFreeTierService.ListAccountActivities",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "UpgradeAccountPlan success",
            target: "AWSFreeTierService.UpgradeAccountPlan",
            body: json!({"accountPlanType": PLAN_TYPE_FREE}),
            request_media_type: Some(SERVICE_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 200,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "an absent required member, refused in front of the service",
            target: "AWSFreeTierService.UpgradeAccountPlan",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 400,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
        MediaTypeCase {
            name: "a target this service models no operation for",
            target: "AWSFreeTierService.NoSuchOperation",
            body: json!({}),
            request_media_type: Some(MODEL_MEDIA_TYPE),
            establish_plan: false,
            expected_status: 501,
            expected_media_type: SERVICE_MEDIA_TYPE,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        if case.establish_plan {
            emulator
                .ok(
                    "UpgradeAccountPlan",
                    &json!({"accountPlanType": PLAN_TYPE_FREE}),
                )
                .await;
        }

        let (status, media_type, payload) = emulator
            .dispatch(case.target, &case.body, case.request_media_type)
            .await;
        assert_eq!(
            status, case.expected_status,
            "case {:?}: expected HTTP {}, got {status} with {payload}",
            case.name, case.expected_status
        );
        assert_eq!(
            media_type, case.expected_media_type,
            "case {:?}: expected media type {:?}, got {media_type:?} with {payload}",
            case.name, case.expected_media_type
        );
    }
}

/// An account the store holds no plan record for is answered with the modelled
/// `ResourceNotFoundException` naming it — and that answer does not change
/// however often the plan is read or what else is read beside it, because a
/// reporting operation never brings the resource it reports into being.
#[tokio::test]
async fn an_account_with_no_plan_data_is_told_so_and_stays_that_way() {
    let account_id = engine::emulator_account_id();
    // Rows are what else the caller does between two reads of the plan: nothing,
    // the other reporting operations, and a refused write. None of them is a
    // call that establishes a plan, so every row must still be told the data is
    // missing.
    let interleaved: [(&str, &str, Value); 5] = [
        ("nothing", "GetAccountPlanState", json!({})),
        ("a usage report", "GetFreeTierUsage", json!({})),
        ("an activity listing", "ListAccountActivities", json!({})),
        (
            "an activity detail read",
            "GetAccountActivity",
            json!({"activityId": "a1b2c3d4e5f60718293a4b5c6d7e8f90"}),
        ),
        (
            "a refused upgrade",
            "UpgradeAccountPlan",
            json!({"accountPlanType": "GOLD"}),
        ),
    ];

    for (case, operation, body) in interleaved {
        let emulator = Emulator::new();

        // Two reads, so a read that quietly established the plan would show up
        // as a 200 on the second one.
        for attempt in 1..=2 {
            let (status, _, payload) = emulator.call("GetAccountPlanState", &json!({})).await;
            assert_eq!(
                status, 400,
                "case {case:?} read {attempt}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?} read {attempt}: expected the modelled not-found error, \
                 got {payload}"
            );
            assert_eq!(
                text(&payload, "message"),
                format!("Missing data for account: {account_id}"),
                "case {case:?} read {attempt}: the message must name the resolved account, \
                 got {payload}"
            );
        }

        let _ = emulator.call(operation, &body).await;

        let (status, _, payload) = emulator.call("GetAccountPlanState", &json!({})).await;
        assert_eq!(
            status, 400,
            "case {case:?} read after {operation}: expected HTTP 400, got {status} with {payload}"
        );
        assert!(
            !emulator.holds_a_plan(account_id),
            "case {case:?}: nothing that is not a plan write may store a plan, \
             and {operation} is not one"
        );
    }
}

/// Once the service's one plan-writing operation has established the record, the
/// same read reports it — for either plan type the model declares.
#[tokio::test]
async fn an_established_plan_is_reported_by_the_same_read_that_was_refused() {
    // Rows are the two members of the modelled `AccountPlanType` enum, each
    // establishing the record from nothing.
    for requested in [PLAN_TYPE_FREE, PLAN_TYPE_PAID] {
        let emulator = Emulator::new();
        // The paid plan is open only to an account whose payment instrument has
        // been vetted; this suite is about what the read reports once the record
        // exists, so both rows establish that precondition.
        emulator.vet_payment_instrument();
        let (status, _, payload) = emulator.call("GetAccountPlanState", &json!({})).await;
        assert_eq!(
            status, 400,
            "case establish {requested}: the account starts with no plan data, \
             got {status} with {payload}"
        );

        let upgraded = emulator
            .ok("UpgradeAccountPlan", &json!({"accountPlanType": requested}))
            .await;

        let reported = emulator.ok("GetAccountPlanState", &json!({})).await;
        assert_eq!(
            text(&reported, "accountId"),
            text(&upgraded, "accountId"),
            "case establish {requested}: the read reports the account the write stored, \
             got {reported}"
        );
        assert_eq!(
            text(&reported, "accountPlanType"),
            requested,
            "case establish {requested}: the read reports the stored plan type, got {reported}"
        );
        assert_eq!(
            text(&reported, "accountPlanStatus"),
            PLAN_STATUS_ACTIVE,
            "case establish {requested}: the plan it was moved onto is active, got {reported}"
        );
        assert!(
            emulator.holds_a_plan(engine::emulator_account_id()),
            "case establish {requested}: the write must have stored the plan it reported"
        );
    }
}
