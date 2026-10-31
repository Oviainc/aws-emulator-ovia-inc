// SPDX-License-Identifier: CC0-1.0

//! The record the two Connect-instance INTEGRATION mutators leave behind, end to end
//! over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test, over `PutConnectInstanceIntegration` and
//! `DeleteConnectInstanceIntegration`:
//!
//! * a call asked to change what this service holds for a Connect instance leaves
//!   EXACTLY ONE structured record naming the operation, the access key id the request
//!   presented, the instance the request named and how the call ended, on EVERY exit the
//!   handler returns through — the accepted one, each not-found refusal, AND the bad
//!   request the handler weighs itself before any instance is resolved — so an attempt
//!   is as visible when it was refused as when it succeeded;
//! * a request the modelled constraints refuse BEFORE any handler runs is not an
//!   attempt a handler saw, so it leaves none;
//! * each attempt leaves its own record, because what the trail records is the attempt
//!   rather than a change;
//! * no record carries what the request or the instance holds — a resource ARN or an
//!   object type name — only wire metadata and the instance id the request named.
//!
//! Sources for the values written by hand here:
//!
//! * `smithy_query(query="operation",operation="PutConnectInstanceIntegration,DeleteConnectInstanceIntegration",model_path="services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json")`
//!   — each operation's method and URI, and the `ValidationException` (400) and
//!   `ResourceNotFoundException` (404) they model.
//! * `smithy_query(query="shape",shape="IntegrationConfig,IntegrationIdentifier,Arn,LambdaArn,EventType",...)`
//!   — the three union members each of them takes, the `@length`/`@pattern` every
//!   well-formed ARN below satisfies and the two a boundary-refused row sits outside of,
//!   and the `EventType` catalog an `objectTypeNames` key is drawn from, which this crate
//!   restates once as `ops::types::EVENT_TYPE_VALUES` (imported rather than copied).

use serde_json::{json, Map, Value};

use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::EVENT_TYPE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    MAX_INTEGRATIONS_PER_INSTANCE, ONBOARDING_JOB_DURATION,
};

use super::common::{call, error_code, router, Harness};
use super::instance_mutation_audit::{audit_capture, field, records_for};

/// The region every case here signs for; nothing in an audit record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// The access key id [`super::common::authorization`] presents, which is what a record
/// names as the caller.
const CALLER_AKID: &str = "AKIDEXAMPLE";

/// Prefix of every instance id this suite names, so the records it reads out of the
/// process-wide capture are its own and no other suite's.
const PREFIX: &str = "integration-audit";

/// A Customer Profiles domain ARN inside `Arn`'s `@length` and `@pattern`, so a body
/// carrying it reaches its handler — and so a record has an ARN it could leak and must
/// not.
const DOMAIN_ARN: &str = "arn:aws:profile:us-east-1:000000000000:domains/audited-domain";

/// A Q Connect knowledge base ARN inside `Arn`'s bounds.
const KNOWLEDGE_BASE_ARN: &str =
    "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555";

/// A Lambda function ARN inside `LambdaArn`'s bounds.
const FUNCTION_ARN: &str = "arn:aws:lambda:us-east-1:000000000000:function:audited-handler";

/// An ARN one character below `Arn`'s published `@length` minimum of 20, and otherwise
/// well-formed: a constrained LEAF of the request, so the boundary walk refuses it
/// before any handler runs.
const TOO_SHORT_ARN: &str = "arn:a:a:aa-aa-1:a:x";

/// How the instance a row names has to be set up before the row's request runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    /// Nothing at all: neither this service nor the account's Connect instance registry
    /// holds the instance.
    Fresh,
    /// The onboarding job has finished, so the instance is one integrations belong to.
    Onboarded,
}

/// One row: the request to drive against one instance, and the single audit record it
/// is expected to leave (`None` for a request no handler saw, which leaves none).
struct Row {
    case: &'static str,
    /// Suffixed onto [`PREFIX`] to make the instance id the row's request names.
    instance: &'static str,
    setup: Setup,
    /// An `integrationConfig` put on the instance before the row's request runs, for a
    /// row whose request needs an integration to already be there.
    seed: Option<Value>,
    operation: &'static str,
    method: &'static str,
    /// Appended to `/v2/connect-instance/{instance}`.
    path: &'static str,
    body: Value,
    status: u16,
    /// The modelled error code the response is expected to carry, if any.
    error: Option<&'static str>,
    /// The outcome the one record the call leaves must name.
    record: Option<&'static str>,
}

/// An `objectTypeNames` map over `keys`, each mapped to a name `ObjectTypeName` admits.
fn object_type_names(keys: &[&str]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert((*key).to_string(), json!("audited-object-type"));
    }
    Value::Object(map)
}

/// A `customerProfiles` `integrationConfig` whose `objectTypeNames` is keyed by `keys`.
fn profiles_config(keys: &[&str]) -> Value {
    json!({"integrationConfig": {"customerProfiles": {
        "domainArn": DOMAIN_ARN,
        "objectTypeNames": object_type_names(keys),
    }}})
}

/// A `lambda` `integrationConfig`.
fn lambda_config() -> Value {
    json!({"integrationConfig": {"lambda": {"functionArn": FUNCTION_ARN}}})
}

/// The `integrationIdentifier` naming the integration [`lambda_config`] configures.
fn lambda_identifier() -> Value {
    json!({"integrationIdentifier": {"lambda": {"functionArn": FUNCTION_ARN}}})
}

/// The `integrationIdentifier` naming the integration [`profiles_config`] configures.
fn profiles_identifier() -> Value {
    json!({"integrationIdentifier": {"customerProfiles": {"domainArn": DOMAIN_ARN}}})
}

/// The `PutConnectInstanceIntegration` rows: one per exit the handler returns through,
/// crossed with the union member in force, the form an off-catalog `objectTypeNames`
/// key takes, and the state of the instance the request names.
fn put_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a put accepted: a Lambda function",
            instance: "put-ok-lambda",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: lambda_config(),
            status: 200,
            error: None,
            record: Some("accepted"),
        },
        Row {
            case: "a put accepted: a Q Connect knowledge base",
            instance: "put-ok-qconnect",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: json!({"integrationConfig": {"qConnect": {
                "knowledgeBaseArn": KNOWLEDGE_BASE_ARN,
            }}}),
            status: 200,
            error: None,
            record: Some("accepted"),
        },
        // Every key of this map is drawn from the catalog the model declares, so the
        // handler's own check admits it: the accepted outcome is recorded for the same
        // shape of body a refused row below carries.
        Row {
            case: "a put accepted: a Customer Profiles domain keyed by catalog event types",
            instance: "put-ok-profiles",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(EVENT_TYPE_VALUES),
            status: 200,
            error: None,
            record: Some("accepted"),
        },
        // The handler's OWN bad request: a map's key is the one value inside the union
        // no boundary walk reaches, so these rows are refused by the handler and are
        // therefore attempts the trail records.
        Row {
            case: "a put refused by the handler: an event type the catalog does not name",
            instance: "put-bad-key-unknown",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(&["Campaign-Fax"]),
            status: 400,
            error: Some("ValidationException"),
            record: Some("ValidationException"),
        },
        Row {
            case: "a put refused by the handler: a catalog event type in the wrong case",
            instance: "put-bad-key-case",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(&["campaign-email"]),
            status: 400,
            error: Some("ValidationException"),
            record: Some("ValidationException"),
        },
        Row {
            case: "a put refused by the handler: the empty event type",
            instance: "put-bad-key-empty",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(&[""]),
            status: 400,
            error: Some("ValidationException"),
            record: Some("ValidationException"),
        },
        Row {
            case: "a put refused by the handler: an off-catalog key beside catalog ones",
            instance: "put-bad-key-mixed",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(&[EVENT_TYPE_VALUES[0], "Campaign-Pager", EVENT_TYPE_VALUES[1]]),
            status: 400,
            error: Some("ValidationException"),
            record: Some("ValidationException"),
        },
        // The same handler refusal against an instance this service holds nothing for:
        // the map's keys are weighed before the instance is resolved, so the attempt
        // ends in the bad request and is recorded as that outcome and no other.
        Row {
            case: "a put refused by the handler: an off-catalog key, instance not onboarded",
            instance: "put-bad-key-fresh",
            setup: Setup::Fresh,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: profiles_config(&["Campaign-Fax"]),
            status: 400,
            error: Some("ValidationException"),
            record: Some("ValidationException"),
        },
        // An ARN behind the union is a constrained LEAF, which the boundary walk does
        // reach: the request never gets to a handler, so there is no attempt to record.
        Row {
            case: "a put refused at the boundary: a domain ARN under the length minimum",
            instance: "put-bad-arn-profiles",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: json!({"integrationConfig": {"customerProfiles": {
                "domainArn": TOO_SHORT_ARN,
                "objectTypeNames": object_type_names(&[EVENT_TYPE_VALUES[0]]),
            }}}),
            status: 400,
            error: Some("ValidationException"),
            record: None,
        },
        Row {
            case: "a put refused at the boundary: the empty Lambda function ARN",
            instance: "put-bad-arn-lambda",
            setup: Setup::Onboarded,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: json!({"integrationConfig": {"lambda": {"functionArn": ""}}}),
            status: 400,
            error: Some("ValidationException"),
            record: None,
        },
        Row {
            case: "a put refused: the instance is not onboarded",
            instance: "put-not-onboarded",
            setup: Setup::Fresh,
            seed: None,
            operation: "PutConnectInstanceIntegration",
            method: "PUT",
            path: "/integrations",
            body: lambda_config(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some("ResourceNotFoundException"),
        },
    ]
}

/// The `DeleteConnectInstanceIntegration` rows, over the same ground: one per exit, the
/// union member in force, and the state of the instance the request names.
fn delete_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a delete accepted: the Lambda integration the instance holds",
            instance: "delete-ok-lambda",
            setup: Setup::Onboarded,
            seed: Some(lambda_config()),
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: lambda_identifier(),
            status: 200,
            error: None,
            record: Some("accepted"),
        },
        Row {
            case: "a delete accepted: the Customer Profiles integration the instance holds",
            instance: "delete-ok-profiles",
            setup: Setup::Onboarded,
            seed: Some(profiles_config(&[EVENT_TYPE_VALUES[0]])),
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: profiles_identifier(),
            status: 200,
            error: None,
            record: Some("accepted"),
        },
        Row {
            case: "a delete refused: the instance holds nothing under that identifier",
            instance: "delete-no-record",
            setup: Setup::Onboarded,
            seed: Some(lambda_config()),
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: profiles_identifier(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some("ResourceNotFoundException"),
        },
        Row {
            case: "a delete refused: the instance is not onboarded",
            instance: "delete-not-onboarded",
            setup: Setup::Fresh,
            seed: None,
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: lambda_identifier(),
            status: 404,
            error: Some("ResourceNotFoundException"),
            record: Some("ResourceNotFoundException"),
        },
        Row {
            case: "a delete refused at the boundary: the empty Lambda function ARN",
            instance: "delete-bad-arn-lambda",
            setup: Setup::Onboarded,
            seed: None,
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: json!({"integrationIdentifier": {"lambda": {"functionArn": ""}}}),
            status: 400,
            error: Some("ValidationException"),
            record: None,
        },
        Row {
            case: "a delete refused at the boundary: a domain ARN under the length minimum",
            instance: "delete-bad-arn-profiles",
            setup: Setup::Onboarded,
            seed: None,
            operation: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: "/integrations/delete",
            body: json!({"integrationIdentifier": {"customerProfiles": {
                "domainArn": TOO_SHORT_ARN,
            }}}),
            status: 400,
            error: Some("ValidationException"),
            record: None,
        },
    ]
}

/// The instance id a row's request names.
fn instance_id(row: &Row) -> String {
    format!("{PREFIX}-{}", row.instance)
}

/// `StartInstanceOnboardingJob` for `instance`, asserted to have started — on an
/// instance registered in the account's Connect registry first, since onboarding
/// resolves the `connectInstanceId` against it.
async fn start_onboarding(router: &Harness, instance: &str, case: &str) {
    router.register_connect_instance(REGION, instance);
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 starting onboarding for {instance}, got {status} {body}"
    );
}

/// Put `seed` on `instance`, asserted to have been stored, so a row whose request needs
/// an integration to already be there has one.
async fn seed_integration(router: &Harness, instance: &str, seed: &Value, case: &str) {
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/integrations"),
        seed,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 seeding the integration of {instance}, \
         got {status} {body}"
    );
}

/// Assert the records `instance` gained since `before` are exactly the outcomes
/// `expected` names, each naming `operation` and carrying nothing of what the request or
/// the instance holds.
fn assert_records(
    case: &str,
    instance: &str,
    before: usize,
    operation: &str,
    expected: &[&str],
) {
    let captured = audit_capture();
    let mut after = records_for(captured, instance);
    let added = after.split_off(before);
    let outcomes: Vec<(String, String)> = added
        .iter()
        .map(|record| (field(record, "operation"), field(record, "outcome")))
        .collect();
    let wanted: Vec<(String, String)> = expected
        .iter()
        .map(|outcome| (operation.to_string(), (*outcome).to_string()))
        .collect();
    assert_eq!(
        outcomes, wanted,
        "case {case:?}: expected the audit records {wanted:?}, got {outcomes:?} ({added:?})"
    );
    for record in &added {
        assert_eq!(
            field(record, "caller_akid"),
            CALLER_AKID,
            "case {case:?}: expected the record to name the presented access key id, \
             got {record:?}"
        );
        assert!(
            !record.contains("arn:") && !record.contains("audited-object-type"),
            "case {case:?}: expected the record to carry nothing of what the request \
             carried, got {record:?}"
        );
    }
}

/// Every exit of the two integration mutators that a handler returns through leaves one
/// record naming how the call ended, and a request refused before any handler runs
/// leaves none.
#[tokio::test]
async fn every_integration_mutation_records_how_it_ended() {
    let captured = audit_capture();
    let router = router();
    let rows: Vec<Row> = put_rows().into_iter().chain(delete_rows()).collect();

    // The rows needing a finished onboarding job start theirs together, so the span the
    // clock owes is waited out once rather than once per row. Nothing in the emulator
    // sleeps for that transition — the record stores the due instant.
    for row in rows.iter().filter(|row| row.setup == Setup::Onboarded) {
        start_onboarding(&router, &instance_id(row), row.case).await;
    }
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;

    for row in &rows {
        let instance = instance_id(row);
        if let Some(seed) = &row.seed {
            seed_integration(&router, &instance, seed, row.case).await;
        }

        // Whatever the setup wrote is already in the capture, so only the records this
        // row's own request adds are judged.
        let before = records_for(captured, &instance).len();
        let (status, body) = call(
            &router,
            REGION,
            row.method,
            &format!("/v2/connect-instance/{instance}{}", row.path),
            &row.body,
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body}",
            row.case, row.status
        );
        if let Some(expected) = row.error {
            assert!(
                error_code(&body).ends_with(expected),
                "case {:?}: expected {expected}, got {:?}",
                row.case,
                error_code(&body)
            );
        }

        let expected: Vec<&str> = row.record.into_iter().collect();
        assert_records(row.case, &instance, before, row.operation, &expected);
    }
}

/// The last exit the put returns through: the one the store's own ceiling decides,
/// which is a refusal of an attempt like any other and so leaves the same record.
///
/// The ceiling is the crate's `MAX_INTEGRATIONS_PER_INSTANCE`, imported rather than
/// restated, so the case fills the store to whatever that is.
#[tokio::test]
async fn a_put_refused_by_the_store_ceiling_records_the_attempt() {
    let captured = audit_capture();
    let router = router();
    let instance = format!("{PREFIX}-ceiling");
    start_onboarding(&router, &instance, "the store ceiling").await;
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;

    // Fill the instance to the ceiling, each put under an identity of its own so every
    // one of them adds a record rather than updating the one before it.
    for index in 0..MAX_INTEGRATIONS_PER_INSTANCE {
        let case = format!("filling the store: integration {index}");
        let config = json!({"integrationConfig": {"lambda": {
            "functionArn": format!("{FUNCTION_ARN}-{index}"),
        }}});
        seed_integration(&router, &instance, &config, &case).await;
    }

    let before = records_for(captured, &instance).len();
    let (status, body) = call(
        &router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/integrations"),
        &lambda_config(),
    )
    .await;
    let case = "a put past the store ceiling";
    assert_eq!(
        status, 409,
        "case {case:?}: expected HTTP 409, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case {case:?}: expected ConflictException, got {:?}",
        error_code(&body)
    );
    assert_records(
        case,
        &instance,
        before,
        "PutConnectInstanceIntegration",
        &["ConflictException"],
    );
}

/// One request a case sends more than once: the case name, the operation it reaches,
/// its method, the path under the instance, the body it carries, the status it is
/// answered with, and the outcome each attempt records (`None` where no handler saw the
/// attempt).
type Repeated = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Value,
    u16,
    Option<&'static str>,
);

/// A refusal is not a state change, so repeating one is refused the same way — and the
/// trail records one attempt per call where a handler saw the attempt, and none at all
/// where the boundary refused the request first.
///
/// The rows differ along the operation, the union member in force, and which layer
/// refuses the request.
#[tokio::test]
async fn a_repeated_integration_refusal_is_recorded_once_per_attempt() {
    let captured = audit_capture();
    let router = router();
    let instance = format!("{PREFIX}-repeated");
    start_onboarding(&router, &instance, "repeated refusals").await;
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;

    // One row per request, as [`Repeated`] spells it.
    let cases: Vec<Repeated> = vec![
        (
            "a put the handler refuses for an off-catalog event type",
            "PutConnectInstanceIntegration",
            "PUT",
            "/integrations",
            profiles_config(&["Campaign-Fax"]),
            400,
            Some("ValidationException"),
        ),
        (
            "a delete refused for an instance that holds no integration under the identifier",
            "DeleteConnectInstanceIntegration",
            "POST",
            "/integrations/delete",
            lambda_identifier(),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "a put the boundary refuses for a domain ARN under the length minimum",
            // The boundary refuses this body before the handler runs, so no attempt is
            // recorded however many times it is sent.
            "PutConnectInstanceIntegration",
            "PUT",
            "/integrations",
            json!({"integrationConfig": {"customerProfiles": {
                "domainArn": TOO_SHORT_ARN,
                "objectTypeNames": object_type_names(&[EVENT_TYPE_VALUES[0]]),
            }}}),
            400,
            None,
        ),
    ];

    for (case, operation, method, path, body, status, outcome) in cases {
        let attempts = 3;
        let before = records_for(captured, &instance).len();
        for attempt in 1..=attempts {
            let (got, answered) = call(
                &router,
                REGION,
                method,
                &format!("/v2/connect-instance/{instance}{path}"),
                &body,
            )
            .await;
            assert_eq!(
                got, status,
                "case {case:?} attempt {attempt}: expected HTTP {status}, got {got} {answered}"
            );
        }

        let expected: Vec<&str> = match outcome {
            Some(outcome) => (0..attempts).map(|_| outcome).collect(),
            None => Vec::new(),
        };
        assert_records(case, &instance, before, operation, &expected);
    }
}
