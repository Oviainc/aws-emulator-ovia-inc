// SPDX-License-Identifier: CC0-1.0

//! How Security Lake answers a caller its authorization refuses, end to end over HTTP.
//!
//! RULE under test: a caller Security Lake does not authorize is refused at the front
//! door — HTTP 401, the error code `UnauthorizedException` in `x-amzn-errortype` and the
//! fixed body `{"message":"Unauthorized"}` — on EVERY route, and that verdict is reached
//! from the caller alone, so it precedes the enablement precondition and every resource
//! lookup: the refusal never carries one of this service's modelled resource errors, never
//! reports whether the account has Security Lake enabled, and leaves the account's state
//! exactly as it was. The refusal is the same whichever way the caller came to be refused
//! — a principal the service does not admit, or an account holding no Security Lake role
//! on a route whose authorizer refuses it ahead of the service.
//!
//! Every case drives a real `restJson1` request through the router into this service on
//! the shared harness in `data_lake_and_subscriber_lifecycle`; only the authorization
//! records themselves are seeded through the store, because this service models no
//! operation that writes them.

use serde_json::{json, Value};
use service_com_amazonaws_securitylake_securitylake::state::{
    AccountRole, NOT_ENABLED_IN_ANY_REGION_MESSAGE,
};

use super::data_lake_and_subscriber_lifecycle::{array, text, Emulator, Reply, CALLER_PRINCIPAL};

/// A well-formed `UUID` (the model's
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, which
/// `GetSubscriberRequest$subscriberId` and `DeleteSubscriberRequest$subscriberId` target)
/// naming no stored subscriber.
const UNKNOWN_SUBSCRIBER: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

/// The triple the front door states an authorization refusal as: the status, the error
/// code a client branches on, and the whole body. All three are FIXED — nothing from the
/// request or from the account's state appears in any of them.
const REFUSAL: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// One request a case sends.
struct Call {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

impl Call {
    fn new(case: impl Into<String>, method: &'static str, path: impl Into<String>) -> Self {
        Self {
            case: case.into(),
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

/// A `CreateSubscriber` body, for the ungated mutating route the rows below also cover.
fn subscriber_body(name: &str) -> Value {
    json!({
        "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-refused" },
        "subscriberName": name,
        "sources": [{ "awsLogSource": { "sourceName": "VPC_FLOW", "sourceVersion": "latest" } }],
    })
}

/// The calls one enablement state is exercised with: the two subscriber routes keyed by a
/// path label (on a subscriber that exists where there is one, and on one that does not),
/// the data-lake teardown on an enabled and on an unenabled Region, and the collection
/// routes the entitlement gate does NOT cover — because the refusal is a property of the
/// caller rather than of the route, the method or what the request names.
fn calls(region: &str, other_region: &str, subscriber_id: &str) -> Vec<Call> {
    vec![
        Call::new(
            format!("GetSubscriber {subscriber_id}"),
            "GET",
            format!("/v1/subscribers/{subscriber_id}"),
        ),
        Call::new(
            "GetSubscriber on an unknown subscriber",
            "GET",
            format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
        ),
        Call::new(
            format!("DeleteSubscriber {subscriber_id}"),
            "DELETE",
            format!("/v1/subscribers/{subscriber_id}"),
        ),
        Call::new(
            "DeleteDataLake on the calling Region",
            "POST",
            "/v1/datalake/delete",
        )
        .with_body(json!({ "regions": [region] })),
        Call::new(
            "DeleteDataLake on another Region",
            "POST",
            "/v1/datalake/delete",
        )
        .with_body(json!({ "regions": [other_region] })),
        Call::new("ListSubscribers", "GET", "/v1/subscribers"),
        Call::new("ListDataLakes", "GET", "/v1/datalakes"),
        Call::new("CreateSubscriber", "POST", "/v1/subscribers")
            .with_body(subscriber_body("sub-refused")),
    ]
}

/// Send `call` as the Region `region`.
async fn send(emulator: &Emulator, region: &str, call: &Call) -> Reply {
    emulator
        .send(call.method, &call.path, region, call.body.clone())
        .await
}

/// How a reply reads in an assertion message.
fn rendered(reply: &Reply) -> String {
    format!(
        "{} {:?} {}",
        reply.status, reply.error_type, reply.body
    )
}

/// Assert `reply` is exactly the front door's refusal.
fn assert_refused(case: &str, reply: &Reply) {
    let (status, code, message) = REFUSAL;
    assert_eq!(
        (
            reply.status,
            reply.error_type.as_deref(),
            reply.body.clone()
        ),
        (status, Some(code), json!({ "message": message })),
        "case {case:?}: expected {status} {code} with the body {{\"message\":{message:?}}}, got {}",
        rendered(reply)
    );
}

/// Create a data lake in `region`, as the still-admitted caller.
async fn enable(emulator: &Emulator, region: &str) {
    let created = emulator
        .create_data_lake(region, json!({ "region": region }))
        .await;
    assert_eq!(
        created.status,
        200,
        "setup: enabling Security Lake in {region:?} answered {}",
        rendered(&created)
    );
}

/// Store one subscriber in `region`, as the still-admitted caller, and answer its id.
async fn seed_subscriber(emulator: &Emulator, region: &str) -> String {
    let created = emulator
        .create_subscriber(region, subscriber_body("sub-seeded"))
        .await;
    assert_eq!(
        created.status,
        200,
        "setup: CreateSubscriber in {region:?} answered {}",
        rendered(&created)
    );
    text(&created.body["subscriber"], "subscriberId", "setup").to_string()
}

/// RULE: a caller the service does not admit is refused 401 `UnauthorizedException` /
/// "Unauthorized" on every route, whatever the account's state holds — so the refusal
/// precedes the enablement precondition and the resource lookup behind it, and is
/// byte-identical across every row.
///
/// The rows span the enablement states an account can be in (no Region enabled at all, the
/// calling Region enabled, only another Region enabled), crossed with the routes in
/// `calls` (a labelled read, a labelled delete, a POST teardown naming an enabled and an
/// unenabled Region, the two collection reads and a mutating create) and with the calling
/// Region — none of which may change the answer.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_the_same_way_whatever_the_account_holds() {
    // (case, the Region enabled before the caller is refused)
    let enablement: [(&str, Option<&str>); 3] = [
        ("no Region enabled", None),
        ("the calling Region enabled", Some("calling")),
        ("only another Region enabled", Some("other")),
    ];

    let mut answers: Vec<(String, u16, Option<String>, Value)> = Vec::new();
    for (region, other_region) in [("us-west-2", "eu-west-1"), ("ap-southeast-2", "us-east-1")] {
        for (state_case, enabled) in enablement {
            let emulator = Emulator::new();
            let mut subscriber = UNKNOWN_SUBSCRIBER.to_string();
            match enabled {
                Some("calling") => {
                    enable(&emulator, region).await;
                    subscriber = seed_subscriber(&emulator, region).await;
                }
                Some(_) => enable(&emulator, other_region).await,
                None => {}
            }

            // The positive control: while the caller IS admitted, this service answers
            // from the account's own state — which for an account with no Region enabled
            // is the documented enablement remedy the refusal must displace.
            let probe = Call::new(
                "GetSubscriber",
                "GET",
                format!("/v1/subscribers/{subscriber}"),
            );
            let admitted = send(&emulator, region, &probe).await;
            assert_ne!(
                admitted.status, 401,
                "case {state_case:?} in {region:?}: an admitted caller must not be refused, got {}",
                rendered(&admitted)
            );

            assert!(
                emulator.store().lock().restrict_principal(CALLER_PRINCIPAL),
                "case {state_case:?} in {region:?}: restricting the caller must record it"
            );

            for call in calls(region, other_region, &subscriber) {
                let case = format!("{} / {state_case} in {region}", call.case);
                let reply = send(&emulator, region, &call).await;
                assert_refused(&case, &reply);
                assert_ne!(
                    reply.body["message"].as_str(),
                    Some(NOT_ENABLED_IN_ANY_REGION_MESSAGE),
                    "case {case:?}: a refused caller must learn nothing about the account's \
                     enablement, got {}",
                    rendered(&reply)
                );
                answers.push((
                    case,
                    reply.status,
                    reply.error_type.clone(),
                    reply.body.clone(),
                ));
            }

            // Nothing the refused calls named was applied: the caller is admitted again
            // and reads the account exactly as it was.
            assert!(
                emulator.store().lock().permit_principal(CALLER_PRINCIPAL),
                "case {state_case:?} in {region:?}: permitting the caller must clear its record"
            );
            let listed = emulator.list_data_lakes(region, "").await;
            let regions: Vec<String> = array(&listed, "dataLakes", state_case)
                .iter()
                .map(|lake| text(lake, "region", state_case).to_string())
                .collect();
            let expected: Vec<String> = match enabled {
                Some("calling") => vec![region.to_string()],
                Some(_) => vec![other_region.to_string()],
                _ => Vec::new(),
            };
            assert_eq!(
                regions, expected,
                "case {state_case:?} in {region:?}: the refused teardown must change nothing, \
                 got {}",
                rendered(&listed)
            );
            if enabled == Some("calling") {
                let fetched = emulator.get_subscriber(region, &subscriber).await;
                assert_eq!(
                    fetched.body["subscriber"]["subscriberId"].as_str(),
                    Some(subscriber.as_str()),
                    "case {state_case:?} in {region:?}: the refused delete must leave the \
                     subscriber stored, got {}",
                    rendered(&fetched)
                );
                let subscribers = emulator.list_subscribers(region, "").await;
                assert_eq!(
                    array(&subscribers, "subscribers", state_case).len(),
                    1,
                    "case {state_case:?} in {region:?}: the refused create must store nothing, \
                     got {}",
                    rendered(&subscribers)
                );
            }
        }
    }

    let (first_case, status, code, body) = answers.first().cloned().expect("the table has rows");
    for (case, row_status, row_code, row_body) in &answers[1..] {
        assert_eq!(
            (*row_status, row_code.clone(), row_body.clone()),
            (status, code.clone(), body.clone()),
            "case {case:?}: the refusal must be identical to {first_case:?}, got {row_status} \
             {row_code:?} {row_body}"
        );
    }
}

/// RULE: one refused caller, one verdict. A caller refused because this service does not
/// admit its principal and a caller refused because its account holds no Security Lake
/// role read the SAME answer on a route whose authorizer refuses ahead of the service:
/// same status, same error code, same body.
#[tokio::test]
async fn the_refusal_is_identical_however_the_caller_came_to_be_refused() {
    const REGION: &str = "eu-central-1";

    // The routes the gate reports as the front door's refusal, one row each.
    let routes: [Call; 3] = [
        Call::new(
            "GetSubscriber",
            "GET",
            format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
        ),
        Call::new(
            "DeleteSubscriber",
            "DELETE",
            format!("/v1/subscribers/{UNKNOWN_SUBSCRIBER}"),
        ),
        Call::new("DeleteDataLake", "POST", "/v1/datalake/delete")
            .with_body(json!({ "regions": [REGION] })),
    ];

    for call in &routes {
        // Refused on the principal: the account administers Security Lake (the harness
        // seeds that role), and the authorization records hold this caller.
        let by_principal = Emulator::new();
        by_principal
            .store()
            .lock()
            .restrict_principal(CALLER_PRINCIPAL);
        let refused_principal = send(&by_principal, REGION, call).await;
        assert_refused(&format!("{} / refused principal", call.case), &refused_principal);

        // Refused on the account's role: the principal is admitted, and the account holds
        // no Security Lake role — the state an account starts in.
        let by_role = Emulator::new();
        by_role.set_account_role(AccountRole::OrganizationMember);
        let refused_role = send(&by_role, REGION, call).await;
        assert_refused(&format!("{} / unentitled account", call.case), &refused_role);

        assert_eq!(
            (
                refused_role.status,
                refused_role.error_type.clone(),
                refused_role.body.clone()
            ),
            (
                refused_principal.status,
                refused_principal.error_type.clone(),
                refused_principal.body.clone()
            ),
            "case {:?}: both refusals must read the same, got {} against {}",
            call.case,
            rendered(&refused_role),
            rendered(&refused_principal)
        );
    }
}
