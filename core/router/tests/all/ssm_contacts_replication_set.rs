// SPDX-License-Identifier: CC0-1.0

//! Amazon SSM Contacts is fronted by an account-level Incident Manager replication set,
//! and Incident Manager's own `CreateReplicationSet` — the one call that mints one — is
//! deprecated. The two services live in DIFFERENT crates, so this suite needs the WHOLE
//! router: it is the only place both are mounted, which is what makes "an emulated account
//! is normally not onboarded, so the refusal is the reachable behaviour" assertable.
//!
//! The rule: a contact-creating call is refused with `ValidationException` (HTTP 400)
//! while the account has no ACTIVE replication set, and serviced once it holds one. The
//! table sends the SAME requests before and after the set is installed, over the contact
//! types and plan shapes the model admits, so what changes the answer is the account's
//! state and nothing in the request.
//!
//! Onboarding cannot be done over the wire — `ssm-incidents` `CreateReplicationSet`
//! refuses every call as deprecated (Amazon Web Services stopped accepting new Incident
//! Manager customers on November 7, 2025), which this suite asserts through the router too
//! — so the set is written into the store the gate reads.
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

/// Give the account an ACTIVE replication set, in the store the contact data plane's gate
/// reads, answering the set's ARN.
///
/// This is how an account is onboarded here: `CreateReplicationSet` refuses every call as
/// deprecated, so nothing over the wire can mint one.
fn install_replication_set(engine: &InMemoryEngine) -> String {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
        AccountReplicationSet, ComAmazonawsSsmcontactsSsmcontactsState,
    };

    let arn = format!(
        "arn:aws:ssm-incidents::{}:replication-set/router-suite",
        engine::EMULATOR_ACCOUNT_ID
    );
    engine
        .state::<ComAmazonawsSsmcontactsSsmcontactsState>()
        .lock()
        .replication_set = Some(AccountReplicationSet {
        arn: arn.clone(),
        active: true,
    });
    arn
}

#[test]
fn ssm_contacts_writes_are_refused_until_the_account_has_a_replication_set() {
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

            // Before onboarding every row is refused, and the refusal is the modelled
            // ValidationException rather than a 501, a 404 or a success.
            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 400,
                    "case {case:?}: CreateContact must be refused while the account has no \
                     replication set, got {status} with {rendered}"
                );
                assert!(
                    rendered.contains("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {status} with {rendered}"
                );
            }

            // Onboarding over the wire is impossible: the owning service refuses every
            // CreateReplicationSet as deprecated, which is why a client sees the refusal
            // above at all.
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

            // So the account is onboarded by writing the set into the store the gate
            // reads, and the same rows are serviced — each created contact readable back.
            let arn = install_replication_set(&engine);
            assert!(
                arn.contains(":ssm-incidents:"),
                "the installed set must carry an Incident Manager ARN, got {arn}"
            );
            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateContact must be serviced once the account has a \
                     replication set, got {status} with {rendered}"
                );
                let created: Value =
                    serde_json::from_str(&rendered).expect("CreateContact answers JSON");
                let arn = created["ContactArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ContactArn in {rendered}"))
                    .to_string();

                let response = runtime.block_on(router.dispatch(contacts_request(
                    "GetContact",
                    &json!({"ContactId": arn}),
                )));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created contact must be readable back, got {status} \
                     with {rendered}"
                );
            }
        })
        .expect("spawn the replication-set thread")
        .join()
        .expect("SSM Contacts must refuse its writes until the account is onboarded");
}
