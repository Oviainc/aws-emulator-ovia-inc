// SPDX-License-Identifier: CC0-1.0

//! Request admission for Security Lake, end to end over HTTP.
//!
//! The rule under test: Security Lake decides whether the PRINCIPAL that signed a
//! request may invoke the operation before that operation runs, so
//!
//! * a request from a principal this service does not admit is refused with HTTP 401,
//!   the front door's own `{"message": …}` body and the error code
//!   `UnauthorizedException` — whatever operation it names, including one that is still a
//!   501 scaffold;
//! * the refusal is reached from the principal alone, so it is byte-identical whether the
//!   resource the request names exists or not, and nothing about this account's state
//!   reaches the caller;
//! * a refused request runs no handler, so it mutates nothing a later admitted caller
//!   sees; and
//! * an ADMITTED principal still gets this service's own modelled answers, including the
//!   404 `ResourceNotFoundException` the model declares for a resource that is not there.
//!
//! Every case drives a real `restJson1` request through the router, so the admission
//! layer, routing, the handlers and serialization are all exercised; only the
//! authorization records themselves are seeded through the store, because this service
//! models no operation that writes them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securitylake_securitylake::state::AccountRole;
use service_com_amazonaws_securitylake_securitylake::state::ComAmazonawsSecuritylakeSecuritylakeState as Store;
use service_com_amazonaws_securitylake_securitylake::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A `RoleArn`-shaped value, matching `CreateDataLakeRequest$metaStoreManagerRoleArn`'s
/// pattern `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const META_STORE_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonSecurityLakeMetaStoreManager";

/// A well-formed `UUID` (the model's `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`)
/// that names no stored subscriber.
const UNKNOWN_SUBSCRIBER: &str = "00000000-0000-0000-0000-000000000000";

/// The principals the rows below refuse. They differ in FORM — a long-term access key id,
/// a temporary (session) one, a value that is not an AWS key id at all, and the id the
/// framework attributes a request carrying no `Authorization` header to (`"default"`,
/// from the vendored `engine::extract_akid`) — because the record is keyed by whatever
/// the request presents, not by a shape of it.
const RESTRICTED_PRINCIPALS: &[(&str, Option<&str>)] = &[
    ("long-term access key id", Some("AKIARESTRICTEDEXAMPLE")),
    ("temporary access key id", Some("ASIARESTRICTEDEXAMPLE")),
    ("a principal that is not a key id", Some("restricted-caller")),
    ("an unsigned request", None),
];

/// The principal every setup call and every "still allowed" row below signs with.
const ADMITTED_PRINCIPAL: &str = "AKIDADMITTEDEXAMPLE";

/// One response, as a test reads it.
#[derive(Debug, PartialEq, Eq)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// Whether this is the front door's authorization refusal: HTTP 401, the recorded
    /// `UnauthorizedException` code and the gateway's `message` body.
    fn is_unauthorized(&self) -> bool {
        self.status == 401
            && self.error_type.as_deref() == Some("UnauthorizedException")
            && self.body == json!({ "message": "Unauthorized" })
    }
}

/// One request this suite sends, named by the operation it reaches.
struct Call {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

impl Call {
    fn new(operation: &'static str, method: &'static str, path: impl Into<String>) -> Self {
        Self {
            operation,
            method,
            path: path.into(),
            body: None,
        }
    }

    fn with_body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// A request for each shape of operation this service serves: a read keyed by a path
/// label, a delete keyed by one, a mutating POST with a body, the two list reads, a PUT
/// update, and an operation that is still a 501 scaffold (so the rows prove admission
/// precedes dispatch, not just the implemented handlers).
fn calls(region: &str, subscriber_id: &str) -> Vec<Call> {
    let subscriber_body = json!({
        "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
        "subscriberName": "sub-admission",
        "sources": [{ "awsLogSource": { "sourceName": "VPC_FLOW", "sourceVersion": "latest" } }],
    });
    vec![
        Call::new(
            "GetSubscriber",
            "GET",
            format!("/v1/subscribers/{subscriber_id}"),
        ),
        Call::new(
            "DeleteSubscriber",
            "DELETE",
            format!("/v1/subscribers/{subscriber_id}"),
        ),
        Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
            .with_body(json!({ "regions": [region] })),
        Call::new("CreateSubscriber", "POST", "/v1/subscribers").with_body(subscriber_body.clone()),
        Call::new("UpdateSubscriber", "PUT", format!("/v1/subscribers/{subscriber_id}"))
            .with_body(json!({ "subscriberName": "renamed" })),
        Call::new("CreateDataLake", "POST", "/v1/datalake").with_body(json!({
            "configurations": [{ "region": region }],
            "metaStoreManagerRoleArn": META_STORE_ROLE,
        })),
        Call::new("ListSubscribers", "GET", "/v1/subscribers"),
        Call::new("ListDataLakes", "GET", "/v1/datalakes"),
        // Still a 501 scaffold today: admission must answer ahead of that too.
        Call::new(
            "GetDataLakeExceptionSubscription",
            "GET",
            "/v1/datalake/exceptions/subscription",
        ),
    ]
}

/// The emulator under test: this one service mounted on the production router, plus the
/// shared engine so a case can seed an authorization record or read what a call stored.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // These cases are about the PRINCIPAL-admission decision, so the caller account
        // is given the Security Lake role the control plane requires of it (see
        // `account_entitlement_gate` for the gate that tests the role itself); without it
        // the data-lake calls below would be refused for a second, unrelated reason.
        engine
            .state::<Store>()
            .lock()
            .set_account_role(AccountRole::Standalone);
        let mut router = MultiServiceRouter::new();
        // Request signatures are verified by the emulator, not by the service: these
        // cases exercise the service's AUTHORIZATION decision, so the authenticator is
        // off and the Authorization header carries only the principal, the route key and
        // the Region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    fn store(&self) -> ::engine::ServiceState<Store> {
        self.engine.state::<Store>()
    }

    /// Send `call` signed by `principal`; `None` sends it with no `Authorization` header
    /// at all, which is the unsigned request the framework attributes to `"default"`.
    async fn send(&self, principal: Option<&str>, region: &str, call: &Call) -> Reply {
        let payload = call.body.clone().map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let mut request = http::Request::builder()
            .method(call.method)
            .uri(call.path.clone())
            .header("host", "securitylake.us-east-1.amazonaws.com")
            .header("content-type", "application/json");
        if let Some(principal) = principal {
            // The SigV4 credential scope names the principal, the service and the Region.
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={principal}/20260101/{region}/securitylake/aws4_request, \
                     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
                ),
            );
        }
        let request = request.body(payload).expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Enable Security Lake in `region` and store one subscriber there, as the admitted
    /// principal. Answers that subscriber's id.
    async fn seed(&self, region: &str) -> String {
        let lake = self
            .send(
                Some(ADMITTED_PRINCIPAL),
                region,
                &Call::new("CreateDataLake", "POST", "/v1/datalake").with_body(json!({
                    "configurations": [{ "region": region }],
                    "metaStoreManagerRoleArn": META_STORE_ROLE,
                })),
            )
            .await;
        assert_eq!(
            lake.status, 200,
            "setup: CreateDataLake in {region:?} answered {} with {:?}",
            lake.status, lake.body
        );

        let created = self
            .send(
                Some(ADMITTED_PRINCIPAL),
                region,
                &Call::new("CreateSubscriber", "POST", "/v1/subscribers").with_body(json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                    "subscriberName": "sub-seeded",
                    "sources": [{ "awsLogSource": { "sourceName": "ROUTE53", "sourceVersion": "2.0" } }],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup: CreateSubscriber in {region:?} answered {} with {:?}",
            created.status, created.body
        );
        created.body["subscriber"]["subscriberId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no subscriberId in {:?}", created.body))
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// The refusal itself
// ---------------------------------------------------------------------------

/// RULE: a request from a principal this service does not admit is refused with HTTP 401,
/// the front door's `{"message": …}` body and the error code `UnauthorizedException`,
/// whatever operation it names — and the very same request is served once the principal is
/// admitted again.
///
/// The rows span the forms a principal takes (a long-term key id, a temporary one, a
/// value that is no key id, and the unsigned request's `"default"`) crossed with the
/// shapes of operation this service serves (a labelled read, a labelled delete, a POST
/// with a body, a PUT, the two list reads, and an operation still answering 501).
#[tokio::test]
async fn an_unadmitted_principal_is_refused_401_on_every_operation() {
    const REGION: &str = "us-east-1";

    for (form, principal) in RESTRICTED_PRINCIPALS {
        for call in &calls(REGION, UNKNOWN_SUBSCRIBER) {
            let emulator = Emulator::new();
            let case = format!("{form} / {}", call.operation);

            let admitted_first = emulator.send(*principal, REGION, call).await;
            assert!(
                !admitted_first.is_unauthorized(),
                "case {case:?}: an unrestricted principal must not be refused, got {admitted_first:?}"
            );

            let recorded = emulator
                .store()
                .lock()
                .restrict_principal(presented(*principal));
            assert!(
                recorded,
                "case {case:?}: restricting a principal the store does not hold must record it"
            );

            let refused = emulator.send(*principal, REGION, call).await;
            assert!(
                refused.is_unauthorized(),
                "case {case:?}: expected 401 UnauthorizedException \
                 {{\"message\":\"Unauthorized\"}}, got {refused:?}"
            );

            let released = emulator
                .store()
                .lock()
                .permit_principal(&presented(*principal));
            assert!(
                released,
                "case {case:?}: permitting a restricted principal must clear its record"
            );

            let readmitted = emulator.send(*principal, REGION, call).await;
            assert!(
                !readmitted.is_unauthorized(),
                "case {case:?}: a re-admitted principal must reach the service, got {readmitted:?}"
            );
        }
    }
}

/// The principal a request signed by `principal` presents, as the store keys it: the
/// access key id itself, or the `"default"` the framework attributes an unsigned request
/// to.
fn presented(principal: Option<&str>) -> String {
    principal.unwrap_or("default").to_string()
}

/// RULE: the decision is reached from the principal alone, so the refusal is identical
/// whether the resource the request names exists or not — a refused caller learns nothing
/// about what this account holds.
#[tokio::test]
async fn the_refusal_is_identical_whether_the_target_exists() {
    const REGION: &str = "eu-west-2";

    let emulator = Emulator::new();
    let stored = emulator.seed(REGION).await;
    emulator
        .store()
        .lock()
        .restrict_principal("AKIARESTRICTEDEXAMPLE");

    // Rows: the same operation on a target that EXISTS and on one that does not, plus a
    // whole-Region operation on a Region that is enabled and one that is not.
    let rows: [(&str, Call); 4] = [
        (
            "GetSubscriber on a stored subscriber",
            Call::new("GetSubscriber", "GET", format!("/v1/subscribers/{stored}")),
        ),
        (
            "GetSubscriber on an unknown subscriber",
            Call::new(
                "GetSubscriber",
                "GET",
                format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
            ),
        ),
        (
            "DeleteDataLake on an enabled Region",
            Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
                .with_body(json!({ "regions": [REGION] })),
        ),
        (
            "DeleteDataLake on a Region that is not enabled",
            Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
                .with_body(json!({ "regions": ["ap-south-1"] })),
        ),
    ];

    let mut answers = Vec::new();
    for (case, call) in &rows {
        let reply = emulator
            .send(Some("AKIARESTRICTEDEXAMPLE"), REGION, call)
            .await;
        assert!(
            reply.is_unauthorized(),
            "case {case:?}: expected the front door's 401, got {reply:?}"
        );
        answers.push((*case, reply));
    }
    let (first_case, first) = &answers[0];
    for (case, reply) in &answers[1..] {
        assert_eq!(
            reply, first,
            "case {case:?}: the refusal must be identical to {first_case:?}, got {reply:?} \
             against {first:?}"
        );
    }
}

/// RULE: a refused request runs no handler, so it changes nothing a later admitted caller
/// sees — not the subscriber a refused delete named, not the data lake a refused
/// DeleteDataLake named, and no new subscriber for a refused create.
#[tokio::test]
async fn a_refused_request_mutates_nothing() {
    const REGION: &str = "ap-southeast-1";

    let emulator = Emulator::new();
    let stored = emulator.seed(REGION).await;
    emulator.store().lock().restrict_principal("ASIARESTRICTED");

    // Rows: one refused mutation per kind — delete a resource, disable the Region, create
    // a resource.
    let mutations: [(&str, Call); 3] = [
        (
            "DeleteSubscriber",
            Call::new(
                "DeleteSubscriber",
                "DELETE",
                format!("/v1/subscribers/{stored}"),
            ),
        ),
        (
            "DeleteDataLake",
            Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
                .with_body(json!({ "regions": [REGION] })),
        ),
        (
            "CreateSubscriber",
            Call::new("CreateSubscriber", "POST", "/v1/subscribers").with_body(json!({
                "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-2" },
                "subscriberName": "sub-refused",
                "sources": [{ "awsLogSource": { "sourceName": "WAF", "sourceVersion": "latest" } }],
            })),
        ),
    ];

    for (case, call) in &mutations {
        let refused = emulator.send(Some("ASIARESTRICTED"), REGION, call).await;
        assert!(
            refused.is_unauthorized(),
            "case {case:?}: expected the front door's 401, got {refused:?}"
        );
    }

    // What an admitted caller sees afterwards: the seeded subscriber, exactly one
    // subscriber in the Region, and the Region still enabled.
    let fetched = emulator
        .send(
            Some(ADMITTED_PRINCIPAL),
            REGION,
            &Call::new("GetSubscriber", "GET", format!("/v1/subscribers/{stored}")),
        )
        .await;
    assert_eq!(
        fetched.status, 200,
        "case \"after the refusals\": the seeded subscriber must still be readable, got {fetched:?}"
    );
    assert_eq!(
        fetched.body["subscriber"]["subscriberId"].as_str(),
        Some(stored.as_str()),
        "case \"after the refusals\": wrong subscriber reported, got {:?}",
        fetched.body
    );

    let listed = emulator
        .send(
            Some(ADMITTED_PRINCIPAL),
            REGION,
            &Call::new("ListSubscribers", "GET", "/v1/subscribers"),
        )
        .await;
    let subscribers = listed.body["subscribers"].as_array().cloned();
    assert_eq!(
        subscribers.as_ref().map(Vec::len),
        Some(1),
        "case \"after the refusals\": a refused create must store nothing, got {:?}",
        listed.body
    );

    let lakes = emulator
        .send(
            Some(ADMITTED_PRINCIPAL),
            REGION,
            &Call::new("ListDataLakes", "GET", "/v1/datalakes"),
        )
        .await;
    let regions: Vec<String> = lakes.body["dataLakes"]
        .as_array()
        .map(|lakes| {
            lakes
                .iter()
                .filter_map(|lake| lake["region"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        regions.iter().any(|region| region == REGION),
        "case \"after the refusals\": a refused DeleteDataLake must leave the Region enabled, \
         got {regions:?}"
    );
}

// ---------------------------------------------------------------------------
// What an admitted principal still gets
// ---------------------------------------------------------------------------

/// RULE: admission decides only whether the caller may ask. An ADMITTED principal still
/// gets this service's own modelled answers — including the 404
/// `ResourceNotFoundException` the model declares for a target that is not there, which
/// the front door must not have replaced.
#[tokio::test]
async fn an_admitted_principal_still_gets_the_modelled_resource_errors() {
    const REGION: &str = "us-west-2";

    // Rows: the three operations whose modelled 404 the refusal must not displace, on an
    // emulator holding nothing at all.
    let rows: [(&str, Call); 3] = [
        (
            "GetSubscriber on an unknown subscriber",
            Call::new(
                "GetSubscriber",
                "GET",
                format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
            ),
        ),
        (
            "DeleteSubscriber on an unknown subscriber",
            Call::new(
                "DeleteSubscriber",
                "DELETE",
                format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
            ),
        ),
        (
            "DeleteDataLake where Security Lake is not enabled",
            Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
                .with_body(json!({ "regions": [REGION] })),
        ),
    ];

    let emulator = Emulator::new();
    for (case, call) in &rows {
        let reply = emulator.send(Some(ADMITTED_PRINCIPAL), REGION, call).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: an admitted caller must still get the modelled 404, got {reply:?}"
        );
    }
}

/// RULE: an authorization record is keyed by the signing principal, so restricting one
/// principal leaves every other caller alone, and the records report exactly what was
/// restricted.
#[tokio::test]
async fn the_record_is_keyed_by_the_signing_principal() {
    const REGION: &str = "us-east-2";

    let emulator = Emulator::new();
    let stored = emulator.seed(REGION).await;
    let read = Call::new("GetSubscriber", "GET", format!("/v1/subscribers/{stored}"));

    // Rows: three principals, the middle one restricted.
    let principals = ["AKIDCALLERONE", "AKIDCALLERTWO", "AKIDCALLERTHREE"];
    assert!(
        emulator.store().lock().restrict_principal(principals[1]),
        "restricting a fresh principal must record it"
    );
    assert_eq!(
        emulator
            .store()
            .lock()
            .unauthorized_principals()
            .collect::<Vec<_>>(),
        vec![principals[1]],
        "the store must hold exactly the one restricted principal"
    );
    assert!(
        !emulator.store().lock().restrict_principal(principals[1]),
        "restricting the same principal twice must not change the records"
    );

    for principal in principals {
        let reply = emulator.send(Some(principal), REGION, &read).await;
        let expected_refusal = principal == principals[1];
        assert_eq!(
            reply.is_unauthorized(),
            expected_refusal,
            "case {principal:?}: expected refused={expected_refusal}, got {reply:?}"
        );
        if !expected_refusal {
            assert_eq!(
                reply.status, 200,
                "case {principal:?}: an admitted caller must read the subscriber, got {reply:?}"
            );
        }
    }

    assert!(
        !emulator.store().lock().permit_principal(principals[0]),
        "permitting a principal that was never restricted must report no change"
    );
}
