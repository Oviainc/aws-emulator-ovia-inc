// SPDX-License-Identifier: CC0-1.0

//! Amazon SSM Contacts is fronted by an account-level Incident Manager replication set,
//! and Incident Manager's own `CreateReplicationSet` — the one call that mints one — is
//! deprecated. The two services live in DIFFERENT crates, so this suite needs the WHOLE
//! router: it is the only place both are mounted, which is what makes "the account a client
//! finds is already onboarded, because no call could onboard it" assertable over the wire.
//!
//! The rule: a prerequisite no API call can create belongs to the emulated account's
//! initial state, so the contact data plane answers a client's FIRST request; the gate on
//! it is still real, and refuses with `ValidationException` (HTTP 400) for an account whose
//! set is gone. The table sends the SAME requests on the account as found and again after
//! the set is removed, over the contact types and plan shapes the model admits, so what
//! changes the answer is the account's state and nothing in the request.
//!
//! `ssm-incidents` `CreateReplicationSet` refusing every call as deprecated (Amazon Web
//! Services stopped accepting new Incident Manager customers on November 7, 2025) is
//! asserted here too, through the router: it is the reason the account cannot start
//! un-onboarded, since no client could ever open the data plane again.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region both services' credential scopes name.
const REGION: &str = "us-east-1";

/// The ARN of the replication set the account is onboarded with, as the contact data plane
/// records it — read from the store rather than rebuilt here.
fn installed_replication_set(engine: &InMemoryEngine) -> Option<String> {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::ComAmazonawsSsmcontactsSsmcontactsState;

    engine
        .state::<ComAmazonawsSsmcontactsSsmcontactsState>()
        .lock()
        .active_replication_set()
        .map(|set| set.arn.clone())
}

/// Take the account's replication set away, which no request can do — this is the only way
/// to reach the refusal, and the reason the account may not START there.
fn remove_replication_set(engine: &InMemoryEngine) {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::ComAmazonawsSsmcontactsSsmcontactsState;

    engine
        .state::<ComAmazonawsSsmcontactsSsmcontactsState>()
        .lock()
        .replication_set = None;
}

#[test]
fn ssm_contacts_serves_the_account_it_finds_and_nothing_can_onboard_another() {
    std::thread::Builder::new()
        .name("ssm-contacts-replication-set".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(Arc::clone(&engine));
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            /// An awsJson1_1 `SSMContacts.<operation>` request.
            fn contacts_request(operation: &str, body: &Value) -> http::Request<bytes::Bytes> {
                http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("content-type", "application/x-amz-json-1.1")
                    .header("x-amz-target", format!("SSMContacts.{operation}"))
                    .header(
                        "authorization",
                        format!(
                            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                             /ssm-contacts/aws4_request, SignedHeaders=host, Signature=0"
                        ),
                    )
                    .body(bytes::Bytes::from(
                        serde_json::to_vec(body).expect("serialize the request body"),
                    ))
                    .expect("a well-formed request")
            }

            /// A restJson1 `ssm-incidents` request to `path`.
            fn incidents_request(path: &str, body: &Value) -> http::Request<bytes::Bytes> {
                http::Request::builder()
                    .method("POST")
                    .uri(format!("http://localhost{path}"))
                    .header("content-type", "application/json")
                    .header(
                        "authorization",
                        format!(
                            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                             /ssm-incidents/aws4_request, SignedHeaders=host, Signature=0"
                        ),
                    )
                    .body(bytes::Bytes::from(
                        serde_json::to_vec(body).expect("serialize the request body"),
                    ))
                    .expect("a well-formed request")
            }

            // Each row: the case, and the `CreateContact` body — spanning the contact
            // types and both shapes the modelled `Plan` admits.
            let rows: [(&str, Value); 3] = [
                (
                    "a PERSONAL contact with an engagement plan",
                    json!({
                        "Alias": "oncall-primary",
                        "Type": "PERSONAL",
                        "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                    }),
                ),
                (
                    "an ESCALATION plan",
                    json!({
                        "Alias": "escalation-plan",
                        "Type": "ESCALATION",
                        "Plan": {"Stages": [{"DurationInMinutes": 0, "Targets": []}]},
                    }),
                ),
                (
                    "an ONCALL_SCHEDULE whose plan carries rotation ids",
                    json!({
                        "Alias": "weekend-rotation",
                        "Type": "ONCALL_SCHEDULE",
                        "Plan": {"RotationIds": [
                            "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/weekend",
                        ]},
                    }),
                ),
            ];

            // The account a client finds carries an ACTIVE set, minted in Incident
            // Manager's namespace, and nothing in this suite put it there.
            let arn = installed_replication_set(&engine)
                .expect("the account a client finds must hold an ACTIVE replication set");
            assert!(
                arn.contains(":ssm-incidents:"),
                "the account's set must carry an Incident Manager ARN, got {arn}"
            );

            // So every row is served on the FIRST request, and each created contact reads
            // back — no onboarding step of any kind in between.
            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateContact must be serviced on the account a client \
                     finds, got {status} with {rendered}"
                );
                let created: Value =
                    serde_json::from_str(&rendered).expect("CreateContact answers JSON");
                let contact_arn = created["ContactArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ContactArn in {rendered}"))
                    .to_string();

                let response = runtime.block_on(router.dispatch(contacts_request(
                    "GetContact",
                    &json!({"ContactId": contact_arn}),
                )));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created contact must be readable back, got {status} \
                     with {rendered}"
                );
            }

            // Onboarding over the wire is impossible: the owning service refuses every
            // CreateReplicationSet as deprecated. That is why the account cannot start
            // without a set — a client could never acquire one.
            let response = runtime.block_on(router.dispatch(incidents_request(
                "/createReplicationSet",
                &json!({"regions": {REGION: {}}}),
            )));
            let status = response.status().as_u16();
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            assert_eq!(
                status, 400,
                "CreateReplicationSet is deprecated and must refuse every call, got \
                 {status} with {rendered}"
            );

            // The gate is still real: an account whose set is gone — which only a reach
            // into the store can arrange — is refused the very same requests, with the
            // modelled ValidationException rather than a 501, a 404 or a success.
            remove_replication_set(&engine);
            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 400,
                    "case {case:?}: CreateContact must be refused once the account has no \
                     replication set, got {status} with {rendered}"
                );
                assert!(
                    rendered.contains("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {status} with {rendered}"
                );
            }
        })
        .expect("spawn the replication-set thread")
        .join()
        .expect("SSM Contacts must serve the account a client finds");
}
