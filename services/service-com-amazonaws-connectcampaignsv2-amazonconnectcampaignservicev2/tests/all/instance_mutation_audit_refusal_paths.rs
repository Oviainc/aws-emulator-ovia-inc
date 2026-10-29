// SPDX-License-Identifier: CC0-1.0

//! The audit record left by the exits of a state-changing Connect-instance call that
//! are decided BEFORE the instance is looked up, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test:
//!
//! * a call asked to change a Connect instance leaves EXACTLY ONE structured record
//!   naming the operation, the access key id the request presented, the instance the
//!   request named and how the call ended, on EVERY exit it returns through — including
//!   the one that reports a store it could not read and the one that refuses the request
//!   body inside the handler — so an attempt is as visible when it was refused as when it
//!   succeeded, whichever exit fired;
//! * the outcome a record names is the modelled error that exit answers with;
//! * no record carries what the request or the instance holds — a key or resource ARN, a
//!   limits or integration config — only wire metadata and the instance id.
//!
//! Sources for the values written by hand here:
//!
//! * `smithy_query(query="operation",operation="StartInstanceOnboardingJob,PutConnectInstanceIntegration,PutInstanceCommunicationLimits,DeleteConnectInstanceConfig,DeleteInstanceOnboardingJob,DeleteConnectInstanceIntegration",model_path="services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json")`
//!   — the method and URI of every state-changing instance operation, and the
//!   `InternalServerException` / `ValidationException` each of them models.
//! * `smithy_query(query="shape",shape="InternalServerException,ValidationException",...)`
//!   — `httpError` 500 and 400, the statuses asserted below.
//! * `smithy_query(query="shape",shape="CommunicationLimits,EncryptionConfig,CustomerProfilesIntegrationConfig,LambdaIntegrationConfig",...)`
//!   — the list `@length` one bad-request row sits past, the encryption rule another
//!   breaks, and the ARN `@length`/`@pattern` every well-formed body below satisfies.
//!
//! The rows differ along what the rule generalises over: which operation is called (all
//! six that change a Connect instance), the HTTP method (PUT, POST, DELETE), whether the
//! request carries a body, how the body is shaped (an encryption config, a limits list,
//! each kind of integration union member), and which exit the call returns through. The
//! server-fault table is driven against two unrelated engine types, so the answer is
//! keyed to "the store cannot be reached" rather than to one engine type.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::EVENT_TYPE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::MAX_COMMUNICATION_LIMITS;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService;

use super::common::{call, error_code, router};
use super::instance_mutation_audit::{
    audit_capture, field, integration_body, integration_identifier, limits_body,
    off_catalog_integration_body, records_for,
};

/// The region every case here signs for; nothing in an audit record is
/// region-dependent, and every exit under test is decided before a handler reads the
/// request's region.
const REGION: &str = "us-east-1";

/// The access key id [`super::common::authorization`] presents, which is what the
/// record names as the caller.
const CALLER_AKID: &str = "AKIDEXAMPLE";

/// The modelled error a server-fault exit answers with, which is the outcome its record
/// names.
const SERVER_FAULT: &str = "InternalServerException";

/// The modelled error a request the handler itself refuses is answered with.
const BAD_REQUEST: &str = "ValidationException";

/// A well-formed Customer Profiles domain ARN, inside the `@length` and `@pattern` of
/// the shape it targets — so the body carrying it reaches its handler, and so a record
/// has an ARN it could leak and must not.
const PROFILE_ARN: &str = "arn:aws:profile:us-east-1:000000000000:domains/audited-domain";

/// One request: the operation it reaches, its method, its URI and the body it carries
/// (`Value::Null` for no body at all).
type Mutation = (&'static str, &'static str, String, Value);

/// An engine this service's store does not live in — the condition the server-fault exit
/// reports, reachable no other way: the store is unreachable exactly when the service is
/// mounted on an engine that is not the emulator's in-memory one.
#[derive(Debug)]
struct ForeignEngine;

/// A second, unrelated engine type, so the table is driven against more than one shape
/// of foreign engine and the answer cannot be keyed to a single unit type. Its label
/// names the engine in each case it produces.
#[derive(Debug)]
struct ForeignEngineWithState {
    label: String,
}

/// This service mounted on `engine`, behind the same router a front door mounts it on.
fn mounted_on<Eng: Send + Sync + 'static>(engine: Eng) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern, not this service's: the header
    // [`super::common::call`] sends is a routing signal and the caller the record names.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine)));
    router
}

/// A Customer Profiles `integrationConfig`, keyed by an event type the model's
/// `EventType` catalog holds — a second union member, so a row carries a different shape
/// of body. The off-catalog counterpart a handler refuses is
/// [`off_catalog_integration_body`], which this suite shares with the audit table.
fn profiles_integration() -> Value {
    json!({"integrationConfig": {"customerProfiles": {
        "domainArn": PROFILE_ARN,
        "objectTypeNames": {EVENT_TYPE_VALUES[0]: "object-type"},
    }}})
}

/// One well-formed request per state-changing Connect-instance operation, addressed to
/// `instance`: every body satisfies the modelled constraints, so each row reaches its
/// handler instead of being refused at the boundary.
fn instance_mutations(instance: &str) -> Vec<Mutation> {
    let root = format!("/v2/connect-instance/{instance}");
    vec![
        (
            "StartInstanceOnboardingJob",
            "PUT",
            format!("{root}/onboarding"),
            json!({"encryptionConfig": {"enabled": false}}),
        ),
        // The same operation with the other shape of encryption config, so the row's
        // outcome cannot be keyed to one body.
        (
            "StartInstanceOnboardingJob",
            "PUT",
            format!("{root}/onboarding"),
            json!({"encryptionConfig": {
                "enabled": true,
                "encryptionType": "KMS",
                "keyArn": "arn:aws:kms:us-east-1:000000000000:key/audited-key",
            }}),
        ),
        (
            "PutInstanceCommunicationLimits",
            "PUT",
            format!("{root}/communication-limits"),
            limits_body(1),
        ),
        (
            "PutInstanceCommunicationLimits",
            "PUT",
            format!("{root}/communication-limits"),
            limits_body(MAX_COMMUNICATION_LIMITS),
        ),
        (
            "PutConnectInstanceIntegration",
            "PUT",
            format!("{root}/integrations"),
            integration_body(),
        ),
        (
            "PutConnectInstanceIntegration",
            "PUT",
            format!("{root}/integrations"),
            profiles_integration(),
        ),
        (
            "DeleteConnectInstanceIntegration",
            "POST",
            format!("{root}/integrations/delete"),
            integration_identifier(),
        ),
        (
            "DeleteConnectInstanceConfig",
            "DELETE",
            format!("{root}/config"),
            Value::Null,
        ),
        // The same operation with the policy its campaigns are deleted under, so a row
        // addresses it through a query string as well as through a bare path.
        (
            "DeleteConnectInstanceConfig",
            "DELETE",
            format!("{root}/config?campaignDeletionPolicy=DELETE_ALL"),
            Value::Null,
        ),
        (
            "DeleteInstanceOnboardingJob",
            "DELETE",
            format!("{root}/onboarding"),
            Value::Null,
        ),
    ]
}

/// One request per exit a handler of this class refuses the request body on ITSELF — a
/// rule the boundary walk cannot weigh: a constraint on a union's own member, a map key,
/// or a rule only the shape's documentation states — addressed to `instance`.
fn handler_refused_requests(instance: &str) -> Vec<Mutation> {
    let root = format!("/v2/connect-instance/{instance}");
    vec![
        // Encryption enabled with no key: the rule `EncryptionConfig`'s own doc states.
        (
            "StartInstanceOnboardingJob",
            "PUT",
            format!("{root}/onboarding"),
            json!({"encryptionConfig": {"enabled": true}}),
        ),
        // One entry past the published list maximum, and three past it, so the row's
        // outcome is not keyed to one length.
        (
            "PutInstanceCommunicationLimits",
            "PUT",
            format!("{root}/communication-limits"),
            limits_body(MAX_COMMUNICATION_LIMITS + 1),
        ),
        (
            "PutInstanceCommunicationLimits",
            "PUT",
            format!("{root}/communication-limits"),
            limits_body(MAX_COMMUNICATION_LIMITS + 3),
        ),
        // An `objectTypeNames` key outside the model's `EventType` catalog: a map's key
        // is never a leaf of the boundary walk, so the handler weighs it.
        (
            "PutConnectInstanceIntegration",
            "PUT",
            format!("{root}/integrations"),
            off_catalog_integration_body(),
        ),
    ]
}

/// Assert the records `instance` gained since `before` are exactly one, naming
/// `operation` and `outcome`, with nothing of what the request carried.
fn assert_one_record(case: &str, instance: &str, before: usize, operation: &str, outcome: &str) {
    let captured = audit_capture();
    let mut after = records_for(captured, instance);
    let added = after.split_off(before);
    let outcomes: Vec<(String, String)> = added
        .iter()
        .map(|record| (field(record, "operation"), field(record, "outcome")))
        .collect();
    let expected = vec![(operation.to_string(), outcome.to_string())];
    assert_eq!(
        outcomes, expected,
        "case {case:?}: expected the audit records {expected:?}, got {outcomes:?} ({added:?})"
    );
    for record in &added {
        assert_eq!(
            field(record, "caller_akid"),
            CALLER_AKID,
            "case {case:?}: expected the record to name the presented access key id, \
             got {record:?}"
        );
        assert!(
            !record.contains("arn:"),
            "case {case:?}: expected the record to carry nothing of what the request \
             carried, got {record:?}"
        );
    }
}

/// Every state-changing instance call records the attempt when it returns through its
/// server-fault exit, naming the modelled error it answers with.
#[tokio::test]
async fn every_instance_mutation_records_a_server_fault_exit() {
    let captured = audit_capture();
    let stateful = ForeignEngineWithState {
        label: "foreign".to_string(),
    };
    let stateful_case = format!("an engine holding state of its own ({})", stateful.label);
    let engines: Vec<(&str, &str, MultiServiceRouter)> = vec![
        ("a unit engine", "unit", mounted_on(ForeignEngine)),
        (&stateful_case, "stateful", mounted_on(stateful)),
    ];

    for (engine_case, label, router) in &engines {
        let instance = format!("audit-fault-{label}");
        for (operation, method, path, body) in instance_mutations(&instance) {
            let case = format!("{engine_case}: {operation} {method} {path}");
            let before = records_for(captured, &instance).len();
            let (status, answered) = call(router, REGION, method, &path, &body).await;
            assert_eq!(
                status, 500,
                "case {case:?}: expected HTTP 500, got {status} {answered}"
            );
            assert!(
                error_code(&answered).ends_with(SERVER_FAULT),
                "case {case:?}: expected {SERVER_FAULT}, got {:?}",
                error_code(&answered)
            );
            assert_one_record(&case, &instance, before, operation, SERVER_FAULT);
        }
    }
}

/// The server-fault record is keyed to the CONDITION, not to the request: the very same
/// requests, against a service whose store IS reachable, are neither answered nor
/// recorded as a server fault.
///
/// What each one is answered with instead belongs to that operation's own suite — these
/// instances are onboarded nowhere, so most are refused as not found. The assertion here
/// is only that none of them reports or records a server fault.
#[tokio::test]
async fn a_reachable_store_is_never_recorded_as_a_server_fault() {
    let captured = audit_capture();
    let router = router();
    let instance = "audit-fault-reachable";

    for (operation, method, path, body) in instance_mutations(instance) {
        let case = format!("{operation} {method} {path}");
        let before = records_for(captured, instance).len();
        let (status, answered) = call(&router, REGION, method, &path, &body).await;
        assert_ne!(
            status, 500,
            "case {case:?}: a reachable store must not be answered as a server fault, \
             got {status} {answered}"
        );
        let mut after = records_for(captured, instance);
        let added = after.split_off(before);
        let outcomes: Vec<String> = added.iter().map(|record| field(record, "outcome")).collect();
        assert!(
            !outcomes.iter().any(|outcome| outcome == SERVER_FAULT),
            "case {case:?}: a reachable store must not be recorded as a server fault, \
             got {outcomes:?} ({added:?})"
        );
    }
}

/// A request a handler of this class refuses itself records the attempt too, naming the
/// bad request it answers with.
#[tokio::test]
async fn a_handler_refused_request_records_the_attempt() {
    let captured = audit_capture();
    let router = router();
    let instance = "audit-bad-request-exit";

    for (operation, method, path, body) in handler_refused_requests(instance) {
        let case = format!("{operation} {method} {path} {body}");
        let before = records_for(captured, instance).len();
        let (status, answered) = call(&router, REGION, method, &path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answered}"
        );
        assert!(
            error_code(&answered).ends_with(BAD_REQUEST),
            "case {case:?}: expected {BAD_REQUEST}, got {:?}",
            error_code(&answered)
        );
        assert_one_record(&case, instance, before, operation, BAD_REQUEST);
    }
}
