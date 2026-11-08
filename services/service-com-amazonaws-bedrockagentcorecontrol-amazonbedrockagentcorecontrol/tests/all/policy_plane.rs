// SPDX-License-Identifier: CC0-1.0

//! The policy plane — `CreatePolicyEngine`, `GetPolicyEngine`, `CreatePolicy` and
//! `GetPolicy` — end to end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="CreatePolicy,CreatePolicyEngine,\
//! GetPolicy,GetPolicyEngine")`): a policy is created IN a policy engine, so the engine
//! the request path
//! names has to be one this account holds — an id naming no engine is refused the modelled
//! `ResourceNotFoundException` (HTTP 404) and creates nothing, whatever the definition
//! would have been — and a policy created in an engine that does exist is reported
//! `CREATING` with the transition to `ACTIVE` owed by the clock ("This is an asynchronous
//! operation. Use the GetPolicy operation to poll the status field"), reads back through
//! `GetPolicy` as it was stored, and is reachable only through the engine that holds it.
//!
//! The rows differ along what the rule generalises over: the shape of the `policyEngineId`
//! the path carries (its length, its underscores, the shortest the pattern admits), the
//! member of the `definition` union, the enforcement mode and whether a description is
//! supplied.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state/policies.rs` gives the policy plane's clock-owed
/// transitions, plus slack, so a row can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_200);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn create_policy_engine(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(router, "POST", "/policy-engines", Some(body)).await
}

async fn create_policy(router: &MultiServiceRouter, policy_engine: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/policy-engines/{policy_engine}/policies"),
        Some(body),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, policy_engine: &str, policy: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/policy-engines/{policy_engine}/policies/{policy}"),
        None,
    )
    .await
}

async fn get_policy_engine(router: &MultiServiceRouter, policy_engine: &str) -> Answer {
    send(router, "GET", &format!("/policy-engines/{policy_engine}"), None).await
}

/// A Cedar definition: `CedarPolicy$statement` is 35..10000 characters
/// (`smithy_query(query="shape",shape="CedarPolicy")`).
fn cedar_definition(action: &str) -> Value {
    json!({"cedar": {
        "statement": format!("permit(principal, action == Action::\"{action}\", resource);"),
    }})
}

/// A Dogwood definition, the other statement-carrying member of the union
/// (`smithy_query(query="shape",shape="PolicyDefinition")`).
fn dogwood_definition(tool: &str) -> Value {
    json!({"policy": {
        "statement": format!("forbid(principal, action, resource) when {{ tool == \"{tool}\" }};"),
    }})
}

/// The sentence this service refuses a policy create with when the path names no policy
/// engine of this account. The fixed text is the service's; the only variable part is the
/// id the request's path carried.
fn engine_not_found_message(policy_engine_id: &str) -> String {
    format!("Policy engine with ID {policy_engine_id} not found")
}

/// One row of the create table.
struct Row {
    case: &'static str,
    /// The engine's name (`PolicyEngineName` is `^[A-Za-z][A-Za-z0-9_]*$`, 1..48).
    engine_name: &'static str,
    /// The policy's name (`PolicyName` carries the same pattern).
    policy_name: &'static str,
    definition: Value,
    enforcement_mode: Option<&'static str>,
    description: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, a Cedar definition",
            engine_name: "Gateway_engine",
            policy_name: "AllowOrders",
            definition: cedar_definition("invokeTool"),
            enforcement_mode: None,
            description: None,
        },
        Row {
            case: "a Dogwood definition, LOG_ONLY, with a description",
            engine_name: "A",
            policy_name: "LogOnlySearch",
            definition: dogwood_definition("search"),
            enforcement_mode: Some("LOG_ONLY"),
            description: Some("collect data before enforcing"),
        },
        Row {
            case: "the longest engine and policy names this suite uses, ACTIVE named \
                   explicitly",
            engine_name: "Engine_with_a_long_but_valid_name_for_account",
            policy_name: "Policy_with_a_long_but_valid_name_for_account",
            definition: cedar_definition("listTools"),
            enforcement_mode: Some("ACTIVE"),
            description: Some("x"),
        },
    ]
}

impl Row {
    /// The `CreatePolicy` body this row sends.
    fn policy_body(&self) -> Value {
        let mut body = json!({"name": self.policy_name, "definition": self.definition});
        if let Some(enforcement_mode) = self.enforcement_mode {
            body["enforcementMode"] = json!(enforcement_mode);
        }
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        body
    }
}

/// An id is a well-formed `ResourceId`: a name, `-`, and 10 characters of
/// `[a-z0-9_]`, 12..59 characters in all.
fn well_formed_resource_id(resource_id: &str, name: &str) -> bool {
    let Some(suffix) = resource_id.strip_prefix(&format!("{name}-")) else {
        return false;
    };
    (12..=59).contains(&resource_id.chars().count())
        && suffix.chars().count() == 10
        && suffix
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_')
}

/// A policy engine the path names and this account does not hold refuses the create with
/// the modelled 404 and the plane's own sentence — and creates nothing, so the engine does
/// not come into being by being written to.
#[tokio::test]
async fn a_create_against_an_unknown_policy_engine_is_refused() {
    let router = router();
    // Every id below satisfies `ResourceId`'s pattern
    // `^[A-Za-z][A-Za-z0-9_]*-[a-z0-9_]{10}$`, so the refusal is the handler's lookup and
    // not the boundary constraint walk: the shortest id the pattern admits, one carrying
    // underscores, one at the top of the length range, and one whose suffix is all
    // underscores.
    let unknown = [
        "A-0123456789",
        "Missing_engine-abcdefghij",
        "Engine_with_a_long_but_valid_name_for_account-zz00zz00zz",
        "Underscores-__________",
    ];
    for policy_engine_id in unknown {
        for (shape, definition) in [
            ("cedar", cedar_definition("invokeTool")),
            ("policy", dogwood_definition("search")),
        ] {
            let refused = create_policy(
                &router,
                policy_engine_id,
                &json!({"name": "AnyPolicy", "definition": definition}),
            )
            .await;
            assert_eq!(
                (refused.status, error_code(&refused.body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {policy_engine_id:?}/{shape}: expected HTTP 404 \
                 ResourceNotFoundException, got {} {} ({})",
                refused.status,
                error_code(&refused.body),
                refused.body
            );
            assert_eq!(
                text(&refused.body, "message"),
                engine_not_found_message(policy_engine_id),
                "case {policy_engine_id:?}/{shape}: the refusal names the engine the path \
                 carried, got {}",
                text(&refused.body, "message")
            );
            assert!(
                refused.body.get("policyId").is_none(),
                "case {policy_engine_id:?}/{shape}: a refused create answers no success \
                 envelope, got {}",
                refused.body
            );
        }
        // Nothing was created by the refused call: the engine still does not exist, so a
        // read of it, and a read through it, are refused in the same words.
        let read_engine = get_policy_engine(&router, policy_engine_id).await;
        assert_eq!(
            (read_engine.status, text(&read_engine.body, "message")),
            (404, engine_not_found_message(policy_engine_id)),
            "case {policy_engine_id:?}: the refused create left no engine behind, got {} ({})",
            read_engine.status,
            read_engine.body
        );
        let read = get_policy(&router, policy_engine_id, "AnyPolicy-0123456789").await;
        assert_eq!(
            (read.status, text(&read.body, "message")),
            (404, engine_not_found_message(policy_engine_id)),
            "case {policy_engine_id:?}: a policy read through the absent engine is refused in \
             the same words, got {} ({})",
            read.status,
            read.body
        );
    }
}

/// A policy created in an engine that exists is stored, reported `CREATING`, reads back
/// through `GetPolicy`, and reaches `ACTIVE` on the clock.
#[tokio::test]
async fn a_policy_created_in_an_engine_reads_back_and_settles() {
    for row in rows() {
        let router = router();
        let engine = create_policy_engine(&router, &json!({"name": row.engine_name})).await;
        assert_eq!(
            engine.status, 202,
            "case {:?}: expected HTTP 202 from CreatePolicyEngine, got {} ({})",
            row.case, engine.status, engine.body
        );
        let policy_engine_id = text(&engine.body, "policyEngineId");
        assert!(
            well_formed_resource_id(&policy_engine_id, row.engine_name),
            "case {:?}: the engine id is the name plus a generated suffix, got \
             {policy_engine_id:?}",
            row.case
        );
        assert_eq!(
            text(&engine.body, "policyEngineArn"),
            format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:policy-engine/{policy_engine_id}"
            ),
            "case {:?}: the engine ARN is derived from its id, the Region and the account, \
             got {}",
            row.case,
            engine.body
        );
        assert_eq!(
            text(&engine.body, "status"),
            "CREATING",
            "case {:?}: a new engine is CREATING, got {}",
            row.case,
            engine.body
        );

        // The engine round trip: the read reports the record the create wrote.
        let read_engine = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            read_engine.status, 200,
            "case {:?}: the engine must read back, got {} ({})",
            row.case, read_engine.status, read_engine.body
        );
        for member in [
            "policyEngineId",
            "name",
            "policyEngineArn",
            "description",
            "encryptionKeyArn",
            "status",
            "createdAt",
            "updatedAt",
            "statusReasons",
        ] {
            assert_eq!(
                read_engine.body.get(member),
                engine.body.get(member),
                "case {:?}: the engine read reports the {member} the create did, got {}",
                row.case,
                read_engine.body
            );
        }

        let created = create_policy(&router, &policy_engine_id, &row.policy_body()).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: expected HTTP 202 from CreatePolicy, got {} ({})",
            row.case, created.status, created.body
        );
        let policy_id = text(&created.body, "policyId");
        assert!(
            well_formed_resource_id(&policy_id, row.policy_name),
            "case {:?}: the policy id is the name plus a generated suffix, got {policy_id:?}",
            row.case
        );
        assert_eq!(
            text(&created.body, "policyArn"),
            format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:policy-engine/\
                 {policy_engine_id}/policy/{policy_id}"
            ),
            "case {:?}: the policy ARN nests the policy under the engine that evaluates it, \
             got {}",
            row.case,
            created.body
        );
        assert_eq!(
            (
                text(&created.body, "status"),
                text(&created.body, "policyEngineId"),
                created.body.get("statusReasons").cloned(),
            ),
            (
                "CREATING".to_string(),
                policy_engine_id.clone(),
                Some(json!([])),
            ),
            "case {:?}: a new policy is CREATING in the engine the path named, with no \
             status reason, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            text(&created.body, "enforcementMode"),
            row.enforcement_mode.unwrap_or("ACTIVE"),
            "case {:?}: the enforcement mode is the request's, or the model's ACTIVE \
             default, got {}",
            row.case,
            created.body
        );

        // The round trip: every member the create reported is what the read reports.
        let read = get_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the policy must read back, got {} ({})",
            row.case, read.status, read.body
        );
        for member in [
            "policyId",
            "name",
            "policyEngineId",
            "policyArn",
            "definition",
            "description",
            "enforcementMode",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                read.body.get(member),
                created.body.get(member),
                "case {:?}: the read reports the {member} the create did, got {}",
                row.case,
                read.body
            );
        }
        assert_eq!(
            read.body.get("definition"),
            Some(&row.definition),
            "case {:?}: the stored definition is the one submitted, got {}",
            row.case,
            read.body
        );

        // The clock owes the transition, and only the clock performs it.
        tokio::time::sleep(PAST_SETTLE).await;
        let settled = get_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            (settled.status, text(&settled.body, "status").as_str()),
            (200, "ACTIVE"),
            "case {:?}: once the span passes the policy is ACTIVE, got {} {} ({})",
            row.case,
            settled.status,
            text(&settled.body, "status"),
            settled.body
        );
        let settled_engine = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            (
                settled_engine.status,
                text(&settled_engine.body, "status").as_str()
            ),
            (200, "ACTIVE"),
            "case {:?}: and so is the engine that holds it, got {} {} ({})",
            row.case,
            settled_engine.status,
            text(&settled_engine.body, "status"),
            settled_engine.body
        );
    }
}

/// A policy is reachable only through the engine that holds it, a name is held once per
/// account, and an id no policy carries is the modelled 404.
#[tokio::test]
async fn a_policy_is_addressed_through_its_own_engine() {
    let router = router();
    let first = create_policy_engine(&router, &json!({"name": "First_engine"})).await;
    let second = create_policy_engine(&router, &json!({"name": "Second_engine"})).await;
    let first_id = text(&first.body, "policyEngineId");
    let second_id = text(&second.body, "policyEngineId");
    assert!(
        !first_id.is_empty() && !second_id.is_empty() && first_id != second_id,
        "setup: two engines are two resources, got {first_id:?} and {second_id:?}"
    );

    let created = create_policy(
        &router,
        &first_id,
        &json!({"name": "OnlyInFirst", "definition": cedar_definition("invokeTool")}),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the policy must be created, got {} ({})",
        created.status, created.body
    );
    let policy_id = text(&created.body, "policyId");

    let through_other = get_policy(&router, &second_id, &policy_id).await;
    assert_eq!(
        (through_other.status, error_code(&through_other.body).as_str()),
        (404, "ResourceNotFoundException"),
        "a policy of one engine is not readable through another, got {} {} ({})",
        through_other.status,
        error_code(&through_other.body),
        through_other.body
    );
    assert_eq!(
        text(&through_other.body, "message"),
        format!("Policy with ID {policy_id} not found"),
        "the refusal names the policy the path carried, got {}",
        text(&through_other.body, "message")
    );

    // "Must be unique within the account", so the second engine cannot hold a policy of
    // the same name either.
    for (case, engine_id) in [("the same engine", &first_id), ("another engine", &second_id)] {
        let repeated = create_policy(
            &router,
            engine_id,
            &json!({"name": "OnlyInFirst", "definition": dogwood_definition("search")}),
        )
        .await;
        assert_eq!(
            (repeated.status, error_code(&repeated.body).as_str()),
            (409, "ConflictException"),
            "case {case:?}: a repeated policy name is the modelled conflict, got {} {} ({})",
            repeated.status,
            error_code(&repeated.body),
            repeated.body
        );
    }

    let missing = get_policy(&router, &first_id, "NoSuchPolicy-0123456789").await;
    assert_eq!(
        (
            missing.status,
            error_code(&missing.body),
            text(&missing.body, "message")
        ),
        (
            404,
            "ResourceNotFoundException".to_string(),
            "Policy with ID NoSuchPolicy-0123456789 not found".to_string()
        ),
        "an id no policy of the engine carries is the modelled 404, got {} ({})",
        missing.status,
        missing.body
    );
}
