// SPDX-License-Identifier: CC0-1.0

//! Amazon SSM Contacts is fronted by an account-level Incident Manager replication set, and
//! Incident Manager's own `CreateReplicationSet` — the one call that mints one — is
//! deprecated. The two services live in DIFFERENT crates, so this suite needs the WHOLE
//! router: it is the only place both are mounted, which is what makes "the two services
//! agree about whether this account is onboarded" assertable over the wire.
//!
//! The rule: an account's onboarding is ONE fact, so Incident Manager's registry of
//! replication sets and the contact data plane's gate on it answer the same way. The account
//! a client finds holds no set — the registry reports none, and `CreateReplicationSet`
//! refuses every call as deprecated, so no client can add one — and until it holds an ACTIVE
//! one every replication-set-scoped contact call is refused with `ValidationException`
//! (HTTP 400) rather than with a 404 about the resource it named or a success. The table
//! sends the SAME requests on the account as found and again once the record is installed,
//! over the contact types and plan shapes the model admits, so what changes the answer is
//! the account's state and nothing in the request.
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

/// The ARN of the replication set the contact data plane records for this account, when it
/// has one — read from the store rather than rebuilt here.
fn installed_replication_set(engine: &InMemoryEngine) -> Option<String> {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::ComAmazonawsSsmcontactsSsmcontactsState;

    engine
        .state::<ComAmazonawsSsmcontactsSsmcontactsState>()
        .lock()
        .active_replication_set()
        .map(|set| set.arn.clone())
}

/// Give the account the onboarding record Incident Manager mints for an onboarded account,
/// which no request can do — this is the only way to open the data plane, and the reason an
/// emulated account cannot start there.
fn install_replication_set(engine: &InMemoryEngine) {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
        AccountReplicationSet, ComAmazonawsSsmcontactsSsmcontactsState,
    };

    engine
        .state::<ComAmazonawsSsmcontactsSsmcontactsState>()
        .lock()
        .replication_set = Some(AccountReplicationSet::onboarded_account());
}

#[test]
fn ssm_contacts_refuses_the_account_incident_manager_holds_no_replication_set_for() {
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
                    "an ONCALL_SCHEDULE whose plan carries the RotationIds shape",
                    json!({
                        "Alias": "weekend-rotation",
                        "Type": "ONCALL_SCHEDULE",
                        // The `RotationIds` SHAPE with no entry (modelled length 0..25): an
                        // entry would have to name a rotation this account holds, and this
                        // suite is about the account gate rather than what a plan may name.
                        "Plan": {"RotationIds": []},
                    }),
                ),
            ];

            // The account a client finds holds no onboarding record, in the contact store
            // and in Incident Manager's own registry alike: one fact, two services.
            assert!(
                installed_replication_set(&engine).is_none(),
                "the account a client finds must hold no replication set, got {:?}",
                installed_replication_set(&engine)
            );
            let response = runtime.block_on(router.dispatch(incidents_request(
                "/listReplicationSets",
                &json!({}),
            )));
            let status = response.status().as_u16();
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            assert_eq!(
                status, 200,
                "ListReplicationSets must be served for an account that holds none, got \
                 {status} with {rendered}"
            );
            let listed: Value =
                serde_json::from_str(&rendered).expect("ListReplicationSets answers JSON");
            assert_eq!(
                listed["replicationSetArns"].as_array().map(Vec::len),
                Some(0),
                "Incident Manager must report no replication set for this account, got \
                 {rendered}"
            );

            // So every row is REFUSED on the first request, with the modelled
            // ValidationException rather than a 404, a 501 or a success.
            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 400,
                    "case {case:?}: CreateContact must be refused while the account holds no \
                     replication set, got {status} with {rendered}"
                );
                assert!(
                    rendered.contains("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {status} with {rendered}"
                );
            }

            // Onboarding over the wire is impossible: the owning service refuses every
            // CreateReplicationSet as deprecated, so nothing a client can call changes the
            // answers above.
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
            assert!(
                installed_replication_set(&engine).is_none(),
                "a refused CreateReplicationSet must onboard nobody, got {:?}",
                installed_replication_set(&engine)
            );

            // And the gate is a gate rather than a wall: an account whose record is present
            // and ACTIVE — which only a reach into the store can arrange — is served the
            // very same requests, and each created contact reads back.
            install_replication_set(&engine);
            let arn = installed_replication_set(&engine)
                .expect("the installed record must be the account's ACTIVE set");
            assert!(
                arn.contains(":ssm-incidents:"),
                "the account's set must carry an Incident Manager ARN, got {arn}"
            );

            for (case, body) in &rows {
                let response = runtime
                    .block_on(router.dispatch(contacts_request("CreateContact", body)));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateContact must be serviced once the account holds an \
                     ACTIVE replication set, got {status} with {rendered}"
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
        })
        .expect("spawn the replication-set thread")
        .join()
        .expect("SSM Contacts and Incident Manager must agree about the account's onboarding");
}
