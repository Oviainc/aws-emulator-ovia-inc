// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request that addresses one of its routes but leaves a
//! `@required` `@httpQuery` parameter out, end to end over HTTP.
//!
//! The rule under test: a request whose method and path address a modelled route is a
//! request for that route's operation, so leaving a `@required` query parameter out is
//! refused with that operation's modelled 400 naming the absent member — never answered
//! as an operation nobody implemented (501), which is what the REST router's
//! required-binding discrimination (it exists to pick BETWEEN operations overloaded on
//! one method + URI template) would otherwise make of it. A request that addresses no
//! route of this service keeps the protocol's own routing answer.
//!
//! The cases are tables spanning what the rule generalises over: every route this model
//! binds a `@required` query parameter on — `GET /revocations`,
//! `DELETE /signing-profiles/{profileName}/permissions/{statementId}` and
//! `DELETE /tags/{resourceArn}`
//! (`smithy_query(query="operation",operation="GetRevocationStatus,\
//! RemoveProfilePermission,UntagResource",model_path="services/\
//! service-com-amazonaws-signer-wallabyservice/smithy-model/\
//! com-amazonaws-signer-wallabyservice.json")`) — which parameter is left out, how many
//! are, whether the resource the path names exists, and the form and length of the
//! values its path labels carry. The counterpart rows supply the parameter, so each
//! refusal differs from a served request only by the parameter it omits.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is one
/// `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The error `code` a refusal of a lost optimistic-concurrency check carries:
/// the condition a writer branches on, not the modelled shape's name.
const ERROR_CODE_CONFLICT: &str = "PolicyRevisionIdMismatch";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the body as JSON, and the wire error shape.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when there is none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The `revisionId` this answer reports, or the empty string when it reports none.
    fn revision_id(&self) -> &str {
        self.body["revisionId"].as_str().unwrap_or_default()
    }
}

/// `value` percent-encoded for ONE URI segment or query value: a `statementId` and a
/// `resourceArn` are `@httpLabel`s and a `revisionId` and a `tagKeys` entry are
/// `@httpQuery` values, all of them unconstrained or punctuation-bearing strings, so an
/// SDK encodes everything outside the unreserved set. Spelled out here the way this
/// crate's other suites spell it, so a suite takes no dependency of its own.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A profile of `name`, and the ARN this service holds for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutSigningProfile must report an arn: {:?}", answer.body))
        .to_string()
}

/// One cross-account permission statement on `profile`, and the revision the policy
/// moved to.
async fn seed_permission(
    router: &MultiServiceRouter,
    profile: &str,
    statement_id: &str,
) -> String {
    let answer = call(
        router,
        "POST",
        &format!("/signing-profiles/{profile}/permissions"),
        Some(json!({
            "statementId": statement_id,
            "action": "signer:StartSigningJob",
            "principal": "123456789012",
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding permission {statement_id} on {profile}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.revision_id().to_string()
}

/// One tag on the resource `arn` names.
async fn seed_tag(router: &MultiServiceRouter, arn: &str, key: &str, value: &str) {
    let mut tags = Map::new();
    tags.insert(key.to_string(), Value::String(value.to_string()));
    let answer = call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({ "tags": Value::Object(tags) })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding tag {key} on {arn}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// The tags the resource `arn` names holds now.
async fn tags_of(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let answer = call(router, "GET", &format!("/tags/{}", encode(arn)), None).await;
    assert_eq!(
        answer.status, 200,
        "reading the tags of {arn}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The statement ids the permission policy of `profile` holds now.
async fn statement_ids(router: &MultiServiceRouter, profile: &str) -> Vec<String> {
    let answer = call(
        router,
        "GET",
        &format!("/signing-profiles/{profile}/permissions"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the permissions of {profile}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["permissions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a permissions list: {:?}", answer.body))
        .iter()
        .map(|permission| {
            permission["statementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A request that addresses a route and leaves a required query parameter out: what to
/// send, and the member the refusal has to name.
struct Omitted {
    case: &'static str,
    method: &'static str,
    uri: String,
    member: &'static str,
}

/// Every required query parameter of `GET /revocations`, in the order the model declares
/// them, each carrying a value this endpoint serves (nothing named is stored, which the
/// operation reports rather than refuses).
fn revocation_parameters() -> Vec<(&'static str, String)> {
    vec![
        ("signatureTimestamp", "2023-11-14T22:13:20Z".to_string()),
        ("platformId", PLATFORM_ID.to_string()),
        (
            "profileVersionArn",
            format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Absent_Param/abcde12345"
            ),
        ),
        (
            "jobArn",
            format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/absent-param-job"),
        ),
        ("certificateHashes", "a".repeat(192)),
    ]
}

/// `GET /revocations` carrying exactly `parameters`.
fn revocations_uri(parameters: &[(&'static str, String)]) -> String {
    if parameters.is_empty() {
        return "/revocations".to_string();
    }
    let query = parameters
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("/revocations?{query}")
}

/// A request addressing a modelled route with a `@required` query parameter left out is
/// refused with the modelled 400 naming the absent member — on every route of this
/// service that binds one, whatever the path labels carry and whether the resource they
/// name exists.
#[tokio::test]
async fn an_absent_required_query_parameter_is_refused_by_name() {
    let router = emulator();

    // A seeded profile, a permission on it and a tag on it: the rows that name these
    // differ from a SERVED request only by the parameter they omit.
    let seeded_profile = "Absent_Param_Profile";
    let seeded_arn = seed_profile(&router, seeded_profile).await;
    seed_permission(&router, seeded_profile, "s1").await;
    seed_tag(&router, &seeded_arn, "team", "signing").await;

    // A profile name at the model's minimum length (2) and one at its maximum (64),
    // neither of them stored: the refusal is about the absent parameter, which the
    // boundary states before any handler looks for the resource.
    let shortest_profile = "Ab";
    let longest_profile = format!("P{}", "_".repeat(63));

    let mut rows: Vec<Omitted> = vec![
        Omitted {
            case: "the permission removal of a stored statement, no revisionId",
            method: "DELETE",
            uri: format!("/signing-profiles/{seeded_profile}/permissions/s1"),
            member: "revisionId",
        },
        Omitted {
            case: "the same removal with the parameter name misspelled",
            method: "DELETE",
            uri: format!(
                "/signing-profiles/{seeded_profile}/permissions/s1?revision_id=abcdefghij"
            ),
            member: "revisionId",
        },
        Omitted {
            case: "a permission removal on a profile that does not exist",
            method: "DELETE",
            uri: format!("/signing-profiles/{shortest_profile}/permissions/s1"),
            member: "revisionId",
        },
        Omitted {
            case: "a permission removal naming the longest profile name and a long statement id",
            method: "DELETE",
            uri: format!(
                "/signing-profiles/{longest_profile}/permissions/{}",
                encode("Statement.Id-With~Punctuation_0123456789")
            ),
            member: "revisionId",
        },
        Omitted {
            case: "a permission removal carrying another operation's query parameter",
            method: "DELETE",
            uri: format!("/signing-profiles/{seeded_profile}/permissions/s1?nextToken=whatever"),
            member: "revisionId",
        },
        Omitted {
            case: "the tag removal of a stored tag, no tagKeys",
            method: "DELETE",
            uri: format!("/tags/{}", encode(&seeded_arn)),
            member: "tagKeys",
        },
        Omitted {
            case: "a tag removal naming a resource that does not exist",
            method: "DELETE",
            uri: format!(
                "/tags/{}",
                encode(&format!(
                    "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile/abcde12345"
                ))
            ),
            member: "tagKeys",
        },
        Omitted {
            case: "the revocation status with no parameters at all",
            method: "GET",
            uri: revocations_uri(&[]),
            member: "signatureTimestamp",
        },
    ];
    // One row per required parameter of the revocation route omitted on its own, so the
    // rule is exercised where a route binds FIVE of them rather than one.
    let complete = revocation_parameters();
    for (index, (omitted, _)) in complete.iter().enumerate() {
        let kept: Vec<(&'static str, String)> = complete
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != index)
            .map(|(_, (name, value))| (*name, value.clone()))
            .collect();
        rows.push(Omitted {
            case: "the revocation status with one parameter omitted",
            method: "GET",
            uri: revocations_uri(&kept),
            member: omitted,
        });
    }

    for Omitted {
        case,
        method,
        uri,
        member,
    } in &rows
    {
        let answer = call(&router, method, uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({member} absent): expected HTTP 400 — a 501 would say nobody \
             implemented the operation — got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {case:?} ({member} absent): expected a ValidationException, got {:?}",
            answer.body
        );
        assert!(
            answer.message().contains(member),
            "case {case:?}: expected the refusal to name {member:?}, got {:?}",
            answer.message()
        );
    }
}

/// The same routes with the parameter SUPPLIED are served by their operation, which
/// applies its own rules to the value: the removal takes effect and is observable
/// through a later read, and a value that loses the optimistic-concurrency check is the
/// handler's 409 rather than the boundary's 400.
#[tokio::test]
async fn the_same_route_with_the_parameter_supplied_is_served() {
    let router = emulator();
    let profile = "Supplied_Param_Profile";
    let arn = seed_profile(&router, profile).await;

    // The parameter present but carrying a revision the policy is not at — empty, a
    // revision the service never issued, and a superseded one — reaches the handler's
    // precondition, which is a different refusal from the absent parameter above.
    let revision = seed_permission(&router, profile, "s1").await;
    let superseded = revision.clone();
    let current = seed_permission(&router, profile, "s2").await;
    assert_ne!(
        superseded, current,
        "a second grant must move the policy to a fresh revision, got {current:?} twice"
    );

    let stale_rows: Vec<(&str, String)> = vec![
        ("an empty revisionId", String::new()),
        ("a revisionId never issued", "0123456789".to_string()),
        ("a superseded revisionId", superseded),
    ];
    for (case, revision_id) in &stale_rows {
        let answer = call(
            &router,
            "DELETE",
            &format!(
                "/signing-profiles/{profile}/permissions/s1?revisionId={}",
                encode(revision_id)
            ),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected the handler's HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            ERROR_CODE_CONFLICT,
            "case {case:?}: expected {ERROR_CODE_CONFLICT}, got {:?}",
            answer.body
        );
    }
    assert_eq!(
        statement_ids(&router, profile).await,
        vec!["s1".to_string(), "s2".to_string()],
        "a refused removal must leave both statements in place"
    );

    // The current revision: the removal is applied, answers the revision the policy
    // moved to, and the statement is gone from a later read.
    let answer = call(
        &router,
        "DELETE",
        &format!(
            "/signing-profiles/{profile}/permissions/s1?revisionId={}",
            encode(&current)
        ),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"the current revisionId\": expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.revision_id(),
        "",
        "case \"the current revisionId\": a removal hands back no revision (a writer re-reads \
         the policy for it), got {:?}",
        answer.body
    );
    assert_eq!(
        statement_ids(&router, profile).await,
        vec!["s2".to_string()],
        "the removed statement must be gone from a later read"
    );

    // The tag route, the same way: the key supplied is removed and the read shows it.
    seed_tag(&router, &arn, "team", "signing").await;
    seed_tag(&router, &arn, "env", "prod").await;
    let answer = call(
        &router,
        "DELETE",
        &format!("/tags/{}?tagKeys={}", encode(&arn), encode("team")),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"tagKeys supplied\": expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    let remaining = tags_of(&router, &arn).await;
    assert_eq!(
        remaining.keys().cloned().collect::<Vec<_>>(),
        vec!["env".to_string()],
        "case \"tagKeys supplied\": expected only the untouched key left, got {remaining:?}"
    );
}

/// A request that addresses NO route of this service still gets the protocol's own
/// routing answer: naming the operation a request addressed must not turn every
/// unroutable request into a refusal of some operation's input.
#[tokio::test]
async fn a_request_addressing_no_route_keeps_the_routing_answer() {
    let router = emulator();
    let profile = "No_Route_Profile";
    let arn = seed_profile(&router, profile).await;

    // Rows: a path no pattern matches, one segment too few for the permission route,
    // one segment too many, a method no route binds on an existing path, and the tag
    // path with no label at all.
    let rows: Vec<(&str, &str, String)> = vec![
        ("a path no route matches", "GET", "/not-a-route".to_string()),
        (
            "the permission route with no statement id",
            "DELETE",
            format!("/signing-profiles/{profile}/permissions"),
        ),
        (
            "the permission route with a segment too many",
            "DELETE",
            format!("/signing-profiles/{profile}/permissions/s1/extra"),
        ),
        (
            "the revocation route on a method it does not bind",
            "POST",
            "/revocations".to_string(),
        ),
        (
            "the tag route with no resource",
            "DELETE",
            "/tags".to_string(),
        ),
        (
            "the tag route with a segment too many",
            "DELETE",
            format!("/tags/{}/extra", encode(&arn)),
        ),
    ];

    for (case, method, uri) in &rows {
        let answer = call(&router, method, uri, None).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: expected the protocol's routing answer (HTTP 501), got {} {:?}",
            answer.status, answer.body
        );
    }
}
