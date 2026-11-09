// SPDX-License-Identifier: CC0-1.0

//! `CreatePolicyEngine` end to end over HTTP: the engine an accepted request mints, and
//! the request this control plane refuses.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint walk and the handler are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="CreatePolicyEngine,GetPolicyEngine")`): an
//! accepted `CreatePolicyEngine` mints exactly one engine and persists it, under a
//! `policyEngineId` that is the requested name plus a generated 10-character suffix and an
//! ARN derived from that id, the Region the request was signed for and the account; it
//! reports the engine `CREATING` with the transition to `ACTIVE` owed by the clock and not
//! by the call ("This is an asynchronous operation. Use the GetPolicyEngine operation to
//! poll the status field to track completion"); every optional member the request carried
//! — and only those — is reported by the create envelope and by `GetPolicyEngine` alike;
//! a retry carrying the same `clientToken` answers with the engine that token first
//! produced rather than a second engine ("If you retry a request with the same client
//! token, the service returns the same response without creating a duplicate policy
//! engine"); and a request outside the modelled input constraints is refused HTTP 400
//! `ValidationException` and mints nothing.
//!
//! The rows differ along what the rule generalises over: the length and shape of the name
//! (the shortest and the longest `PolicyEngineName` admits, mixed case, underscores and
//! digits — which carry the `policyEngineId` to both ends of `ResourceId`'s 12..59 length),
//! which optional members the request carries, the KMS partition the key ARN names, and
//! for the refusals the member, the constraint class and whether the value travelled in the
//! body or was absent altogether.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The Region the rows sign for.
const REGION: &str = "us-east-1";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The settle span the policy plane gives its clock-owed transitions, plus slack, so a row
/// can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_200);

/// `PolicyEngineName`'s `@length` maximum, which `policyEngineId` inherits as
/// `ResourceId`'s 59-character maximum (48 + `-` + 10).
const MAX_NAME_LENGTH: usize = 48;

/// A `KmsKeyArn` in the `aws` partition: 36 characters of `[a-zA-Z0-9-]` after `key/`, per
/// the member's `@pattern`.
const AWS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// A `KmsKeyArn` in the `aws-us-gov` partition — the other alternation the same `@pattern`
/// admits, so the rows span the partition and not just one spelling of it.
const GOV_KEY_ARN: &str =
    "arn:aws-us-gov:kms:us-gov-west-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";

/// A `ClientToken` inside its 33..256 `@length` and its
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$` `@pattern`.
const CLIENT_TOKEN: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A second, different `ClientToken` inside the same constraints.
const OTHER_CLIENT_TOKEN: &str = "ffffffff-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The members `CreatePolicyEngineResponse` and `GetPolicyEngineResponse` both declare, so
/// a round trip compares the whole envelope rather than the members one row happens to set.
const ENGINE_MEMBERS: [&str; 9] = [
    "policyEngineId",
    "name",
    "policyEngineArn",
    "status",
    "statusReasons",
    "description",
    "encryptionKeyArn",
    "createdAt",
    "updatedAt",
];

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

async fn get_policy_engine(router: &MultiServiceRouter, policy_engine_id: &str) -> Answer {
    send(router, "GET", &format!("/policy-engines/{policy_engine_id}"), None).await
}

/// The ARN an engine of this account carries: `PolicyEngineArn`'s
/// `…:bedrock-agentcore:{region}:{account}:policy-engine/{policyEngineId}`.
fn expected_arn(policy_engine_id: &str) -> String {
    format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:policy-engine/{policy_engine_id}")
}

/// An id is a well-formed `ResourceId` for `name`: the name, `-`, and 10 characters of
/// `[a-z0-9_]`, 12..59 characters in all
/// (`^[A-Za-z][A-Za-z0-9_]*-[a-z0-9_]{10}$`).
fn well_formed_resource_id(resource_id: &str, name: &str) -> bool {
    let Some(suffix) = resource_id.strip_prefix(&format!("{name}-")) else {
        return false;
    };
    (12..=59).contains(&resource_id.chars().count())
        && suffix.chars().count() == 10
        && suffix.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// A `DateTimestamp` is rendered in RFC 3339 form (`@timestampFormat("date-time")`), which
/// is what a caller parses: a date, a `T`, a time and a `Z` offset.
fn is_date_time(value: &Value) -> bool {
    value.as_str().is_some_and(|rendered| {
        rendered.len() >= "2026-01-01T00:00:00Z".len()
            && rendered.contains('T')
            && rendered.ends_with('Z')
    })
}

/// One row of the accept table.
struct Row {
    case: &'static str,
    /// `PolicyEngineName` is `^[A-Za-z][A-Za-z0-9_]*$`, 1..48.
    name: String,
    description: Option<&'static str>,
    encryption_key_arn: Option<&'static str>,
    tags: Option<Value>,
    client_token: Option<&'static str>,
}

impl Row {
    /// The `CreatePolicyEngine` body this row sends: the required name plus exactly the
    /// optional members the row carries.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        if let Some(encryption_key_arn) = self.encryption_key_arn {
            body["encryptionKeyArn"] = json!(encryption_key_arn);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        if let Some(client_token) = self.client_token {
            body["clientToken"] = json!(client_token);
        }
        body
    }
}

fn accept_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, the shortest name the pattern admits",
            name: "A".to_string(),
            description: None,
            encryption_key_arn: None,
            tags: None,
            client_token: None,
        },
        Row {
            case: "a description and a customer managed key",
            name: "Orders_gateway_engine".to_string(),
            description: Some("Governs the tool calls of the orders gateway"),
            encryption_key_arn: Some(AWS_KEY_ARN),
            tags: None,
            client_token: None,
        },
        Row {
            case: "the longest name the pattern admits, with tags and a client token",
            // 48 characters: `PolicyEngineName`'s maximum, which makes the minted id
            // `ResourceId`'s 59-character maximum.
            name: format!("Engine_{}", "a".repeat(MAX_NAME_LENGTH - "Engine_".len())),
            description: Some("x"),
            encryption_key_arn: Some(GOV_KEY_ARN),
            tags: Some(json!({"team": "governance", "environment": "staging"})),
            client_token: Some(CLIENT_TOKEN),
        },
        Row {
            case: "mixed case and digits in the name, and the empty tag map the length \
                   minimum admits",
            name: "Engine9_Mixed_Case".to_string(),
            description: None,
            encryption_key_arn: Some(AWS_KEY_ARN),
            tags: Some(json!({})),
            client_token: Some(OTHER_CLIENT_TOKEN),
        },
    ]
}

/// An accepted create mints one engine, reports it `CREATING` with the members the request
/// carried, and `GetPolicyEngine` reads back exactly what was stored.
#[tokio::test]
async fn an_accepted_create_mints_an_engine_that_reads_back() {
    for row in accept_rows() {
        assert!(
            (1..=MAX_NAME_LENGTH).contains(&row.name.chars().count()),
            "case {:?}: the row's own name must be inside PolicyEngineName's 1..48 length, \
             got {} characters",
            row.case,
            row.name.chars().count()
        );
        let router = router();
        let created = create_policy_engine(&router, &row.body()).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: expected HTTP 202 from CreatePolicyEngine, got {} ({})",
            row.case, created.status, created.body
        );

        let policy_engine_id = text(&created.body, "policyEngineId");
        assert!(
            well_formed_resource_id(&policy_engine_id, &row.name),
            "case {:?}: the id is the requested name plus a generated 10-character suffix, \
             got {policy_engine_id:?}",
            row.case
        );
        assert_eq!(
            text(&created.body, "name"),
            row.name,
            "case {:?}: the engine keeps the name the request carried, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            text(&created.body, "policyEngineArn"),
            expected_arn(&policy_engine_id),
            "case {:?}: the ARN is derived from the id, the Region and the account, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            (
                text(&created.body, "status"),
                created.body.get("statusReasons").cloned(),
            ),
            ("CREATING".to_string(), Some(json!([]))),
            "case {:?}: a new engine is CREATING and nothing failed, so it carries no \
             status reason, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            created.body.get("createdAt"),
            created.body.get("updatedAt"),
            "case {:?}: for a newly created engine updatedAt matches createdAt, got {}",
            row.case,
            created.body
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                is_date_time(&created.body[member]),
                "case {:?}: {member} is an RFC 3339 date-time, got {}",
                row.case,
                created.body[member]
            );
        }

        // Exactly the optional members the request carried are reported, and the response
        // declares no `tags` member, so a request that carried tags must not grow one.
        assert_eq!(
            created.body.get("description").and_then(Value::as_str),
            row.description,
            "case {:?}: the description is reported iff the request carried one, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            created.body.get("encryptionKeyArn").and_then(Value::as_str),
            row.encryption_key_arn,
            "case {:?}: the key ARN is reported iff the request carried one, got {}",
            row.case,
            created.body
        );
        assert!(
            created.body.get("tags").is_none(),
            "case {:?}: CreatePolicyEngineResponse declares no tags member, got {}",
            row.case,
            created.body
        );

        // The round trip: the read reports the record the create wrote, member for member.
        let read = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the engine must read back, got {} ({})",
            row.case, read.status, read.body
        );
        for member in ENGINE_MEMBERS {
            assert_eq!(
                read.body.get(member),
                created.body.get(member),
                "case {:?}: the read reports the {member} the create did, got {}",
                row.case,
                read.body
            );
        }

        // A read reports; it does not advance anything a later caller sees.
        let read_again = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            (read_again.status, &read_again.body),
            (200, &read.body),
            "case {:?}: a second read answers exactly what the first did, got {} ({})",
            row.case,
            read_again.status,
            read_again.body
        );
    }
}

/// Two creates are two engines, each addressed by its own id — and a retry carrying a
/// `clientToken` already seen answers the engine that token produced instead of a second
/// one.
#[tokio::test]
async fn a_client_token_replay_answers_the_engine_it_first_produced() {
    // (case, the body sent twice, whether the second call is a replay of the first)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the same client token and the same members",
            json!({"name": "Replayed_engine", "clientToken": CLIENT_TOKEN}),
            true,
        ),
        (
            "a client token beside every optional member",
            json!({
                "name": "Replayed_with_options",
                "clientToken": OTHER_CLIENT_TOKEN,
                "description": "replayed with a key and tags",
                "encryptionKeyArn": AWS_KEY_ARN,
                "tags": {"team": "governance"},
            }),
            true,
        ),
        (
            "no client token at all, so nothing identifies the retry",
            json!({"name": "Untokened_engine"}),
            false,
        ),
    ];

    for (case, body, replays) in rows {
        let router = router();
        let first = create_policy_engine(&router, &body).await;
        let second = create_policy_engine(&router, &body).await;
        assert_eq!(
            (first.status, second.status),
            (202, 202),
            "case {case:?}: both creates are accepted, got {} and {}",
            first.status,
            second.status
        );
        let first_id = text(&first.body, "policyEngineId");
        let second_id = text(&second.body, "policyEngineId");
        if replays {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: the retry answers the engine the token first produced, got \
                 {second_id:?} after {first_id:?}"
            );
            for member in ENGINE_MEMBERS {
                // `status` is read at the instant of the answer, so it is compared through
                // the read below rather than between the two envelopes.
                if member == "status" {
                    continue;
                }
                assert_eq!(
                    second.body.get(member),
                    first.body.get(member),
                    "case {case:?}: the retry reports the {member} the first call did, got {}",
                    second.body
                );
            }
        } else {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: with no token the second call mints a second engine, got \
                 {second_id:?} twice"
            );
            let other = get_policy_engine(&router, &second_id).await;
            assert_eq!(
                (other.status, text(&other.body, "policyEngineArn")),
                (200, expected_arn(&second_id)),
                "case {case:?}: the second engine exists in its own right, got {} ({})",
                other.status,
                other.body
            );
        }
        // Whichever branch ran, the engine the first call reported is stored and readable.
        let read = get_policy_engine(&router, &first_id).await;
        assert_eq!(
            (read.status, text(&read.body, "policyEngineId")),
            (200, first_id.clone()),
            "case {case:?}: the first engine reads back, got {} ({})",
            read.status,
            read.body
        );
    }

    // A different token for the same members is a different request, so it mints its own
    // engine: the idempotency is the token's, not the name's.
    let router = router();
    let first = create_policy_engine(
        &router,
        &json!({"name": "Two_tokens_engine", "clientToken": CLIENT_TOKEN}),
    )
    .await;
    let second = create_policy_engine(
        &router,
        &json!({"name": "Two_tokens_engine", "clientToken": OTHER_CLIENT_TOKEN}),
    )
    .await;
    assert_ne!(
        text(&second.body, "policyEngineId"),
        text(&first.body, "policyEngineId"),
        "case \"two different tokens\": each token mints its own engine, got {} twice",
        text(&first.body, "policyEngineId")
    );
}

/// `CREATING` is the status the create reports, and the transition to `ACTIVE` is owed by
/// the clock: a caller polling straight after the call still sees `CREATING`, and the
/// engine is `ACTIVE` once the span has passed — with the timestamps the create wrote
/// untouched.
#[tokio::test]
async fn the_clock_and_not_the_call_moves_a_new_engine_to_active() {
    let router = router();
    let names = [
        "A",
        "Settling_engine",
        "Engine9_Mixed_Case",
        "Engine_with_underscores_in_its_name",
    ];
    let mut settling = Vec::new();
    for name in names {
        let created = create_policy_engine(&router, &json!({"name": name})).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: expected HTTP 202 from CreatePolicyEngine, got {} ({})",
            created.status, created.body
        );
        let policy_engine_id = text(&created.body, "policyEngineId");
        let polled = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            (polled.status, text(&polled.body, "status").as_str()),
            (200, "CREATING"),
            "case {name:?}: a caller polling straight after the create still sees CREATING, \
             got {} {} ({})",
            polled.status,
            text(&polled.body, "status"),
            polled.body
        );
        settling.push((name, policy_engine_id, created.body));
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (name, policy_engine_id, created) in settling {
        let settled = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            (settled.status, text(&settled.body, "status").as_str()),
            (200, "ACTIVE"),
            "case {name:?}: once the span has passed the engine is ACTIVE, got {} {} ({})",
            settled.status,
            text(&settled.body, "status"),
            settled.body
        );
        for member in ["policyEngineId", "name", "policyEngineArn", "createdAt"] {
            assert_eq!(
                settled.body.get(member),
                created.get(member),
                "case {name:?}: the transition left {member} as the create wrote it, got {}",
                settled.body
            );
        }
    }
}

/// One row of the refusal table: the body, and the clause the refusal states for it.
struct Refusal {
    case: &'static str,
    body: Value,
    /// The whole `message` the refusal states for this row.
    message: String,
}

/// The one-clause refusal this family words a broken constraint with: `/policy-engines` is
/// not one of the value-less families, so the clause quotes the value the member held.
fn one_clause(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A request outside a modelled input constraint is refused HTTP 400 `ValidationException`
/// and mints nothing.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    // Every requirement below is the one the named member's own constraint states, with a
    // `@pattern` quoted unanchored, the way this service publishes it. Sources:
    // `smithy_query(query="operation",operation="CreatePolicyEngine")` for `name`
    // (`^[A-Za-z][A-Za-z0-9_]*$`, 1..48), `clientToken` (33..256) and `encryptionKeyArn`.
    let name_pattern = "Member must satisfy regular expression pattern: [A-Za-z][A-Za-z0-9_]*";
    let key_pattern = "Member must satisfy regular expression pattern: \
                       arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}";
    let too_long_name = format!("E{}", "x".repeat(MAX_NAME_LENGTH));
    let short_token = "tooshort";
    let malformed_key = "arn:aws:kms:us-east-1:000000000000:key/not-a-key";

    let rows = vec![
        Refusal {
            case: "a name that does not start with a letter",
            body: json!({"name": "9Engine"}),
            message: one_clause("9Engine", "name", name_pattern),
        },
        Refusal {
            case: "a name carrying a character the pattern excludes",
            body: json!({"name": "Engine-with-hyphens"}),
            message: one_clause("Engine-with-hyphens", "name", name_pattern),
        },
        Refusal {
            case: "a name one character past the length maximum",
            body: json!({"name": too_long_name.clone()}),
            message: one_clause(
                &too_long_name,
                "name",
                &format!("Member must have length less than or equal to {MAX_NAME_LENGTH}"),
            ),
        },
        Refusal {
            case: "a client token below the length minimum",
            body: json!({"name": "Valid_engine", "clientToken": short_token}),
            message: one_clause(
                short_token,
                "clientToken",
                "Member must have length greater than or equal to 33",
            ),
        },
        Refusal {
            case: "a key ARN outside the KMS key pattern",
            body: json!({"name": "Valid_engine", "encryptionKeyArn": malformed_key}),
            message: one_clause(malformed_key, "encryptionKeyArn", key_pattern),
        },
        Refusal {
            case: "no name at all",
            body: json!({}),
            message: "1 validation error detected: Value null at 'name' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
    ];

    let router = router();
    for row in rows {
        let refused = create_policy_engine(&router, &row.body).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected HTTP 400 ValidationException, got {} {} ({})",
            row.case,
            refused.status,
            error_code(&refused.body),
            refused.body
        );
        assert_eq!(
            text(&refused.body, "message"),
            row.message,
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            row.message,
            text(&refused.body, "message")
        );
        assert!(
            refused.body.get("policyEngineId").is_none(),
            "case {:?}: a refused create answers no success envelope, got {}",
            row.case,
            refused.body
        );
    }

    // None of those refusals minted anything: the name every one of them used is free, and
    // a request inside every constraint is still served.
    let accepted = create_policy_engine(
        &router,
        &json!({"name": "Valid_engine", "clientToken": CLIENT_TOKEN}),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"a request inside every constraint, after the refusals\": expected HTTP 202, \
         got {} ({})",
        accepted.status, accepted.body
    );
    let policy_engine_id = text(&accepted.body, "policyEngineId");
    assert!(
        well_formed_resource_id(&policy_engine_id, "Valid_engine"),
        "case \"a request inside every constraint, after the refusals\": the id is the name \
         plus a generated suffix, got {policy_engine_id:?}"
    );
    let read = get_policy_engine(&router, &policy_engine_id).await;
    assert_eq!(
        (read.status, text(&read.body, "name").as_str()),
        (200, "Valid_engine"),
        "case \"a request inside every constraint, after the refusals\": the engine reads \
         back, got {} ({})",
        read.status,
        read.body
    );
}
