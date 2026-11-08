// SPDX-License-Identifier: CC0-1.0

//! The order an enumerated type's values are listed in when a request is refused for
//! carrying a value outside them, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the constraint walk and the error envelope are all
//! exercised.
//!
//! The rule these cases pin: the bracketed value list in an enum-value-set refusal is
//! the order THIS SERVICE publishes for that enum, which the order its model shape
//! happens to declare the values in does not decide; one order per enum, stated
//! whichever operation carried the member and whatever value was sent. The rows span
//! what the rule generalises over: the value sent (several shapes of out-of-set value),
//! and the enum carried — one whose published order differs from the model's and one
//! whose does not, which is therefore listed exactly as the model declares it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise the request boundary, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `PutAccountPricingAttributes`, whose one member targets `PricingPlan`.
    async fn put_pricing_plan(&self, plan: &str) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/pricing-attributes",
            json!({"Plan": plan}),
        )
        .await
    }

    /// `CreateImportJob`, whose `ImportDataSource$DataFormat` targets `DataFormat` —
    /// an enum this service publishes in the order its model declares.
    async fn create_import_job(&self, data_format: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/import-jobs",
            json!({
                "ImportDataSource": {"S3Url": "s3://bucket/object.csv", "DataFormat": data_format},
                "ImportDestination": {
                    "ContactListDestination": {
                        "ContactListName": "newsletter",
                        "ContactListImportAction": "PUT",
                    },
                },
            }),
        )
        .await
    }
}

/// The clause separator the framework renders, shared by every requirement.
const SEPARATOR: &str = " failed to satisfy constraint: ";

/// The value list this service states for `PricingPlan`.
///
/// `smithy_query(query="shape",shape="PricingPlan")` declares the values in the order
/// `NONE, ESSENTIALS, PRO, ENTERPRISE`; the order below is the one the service states
/// when it refuses a plan outside the set, which is what this rule is about.
const PRICING_PLAN_LIST: &str = "[ENTERPRISE, ESSENTIALS, PRO, NONE]";

/// The value list this service states for `DataFormat`, the control: its published
/// order IS the order `smithy_query(query="shape",shape="DataFormat")` declares.
const DATA_FORMAT_LIST: &str = "[CSV, JSON]";

#[tokio::test]
async fn a_refused_pricing_plan_lists_the_values_in_the_published_order() {
    // Rows spanning the shapes an out-of-set value takes: a plan name the service does
    // not have, a value of the set in the wrong case, a value from another enum, the
    // empty value, and a long one. None of them changes the list that is stated.
    for value in [
        "PLATINUM",
        "essentials",
        "Pro",
        "FREE_TIER",
        "CSV",
        "",
        "a-plan-nobody-offers-with-a-very-long-name",
    ] {
        let ses = Ses::new();
        let reply = ses.put_pricing_plan(value).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {value:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value at 'plan'{SEPARATOR}Member must satisfy enum \
                 value set: {PRICING_PLAN_LIST}"
            ),
            "case {value:?}: expected the value set in the published order, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn every_published_pricing_plan_is_accepted_as_a_plan() {
    // The other side of the rule: each value of the list the refusal states is a value
    // the member really takes, so the list is the enum's own and not a sentence that
    // drifted from it. A plan inside the set draws no `BadRequestException` and no
    // clause about a value's form, whatever else the operation answers.
    for plan in ["ENTERPRISE", "ESSENTIALS", "PRO", "NONE"] {
        let ses = Ses::new();
        let reply = ses.put_pricing_plan(plan).await;
        assert_ne!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {plan:?}: expected no input refusal for a published plan, got {:?}",
            reply.body
        );
        assert!(
            !reply.message().contains(SEPARATOR) && !reply.message().contains("enum value set"),
            "case {plan:?}: expected no constraint refusal, got {} {:?}",
            reply.status,
            reply.message()
        );
    }
}

#[tokio::test]
async fn an_enum_published_in_its_model_order_is_listed_in_that_order() {
    // The boundary of the rule: an enum whose published order is the model's is listed
    // exactly as the model declares it, so the one enum that differs is the only one
    // restated.
    for value in ["NOT_A_FORMAT", "csv", ""] {
        let ses = Ses::new();
        let reply = ses.create_import_job(value).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {value:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value at 'importDataSource.dataFormat'{SEPARATOR}\
                 Member must satisfy enum value set: {DATA_FORMAT_LIST}"
            ),
            "case {value:?}: expected the model's own order, got {:?}",
            reply.message()
        );
    }
}
