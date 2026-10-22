// SPDX-License-Identifier: CC0-1.0

//! The ECR Public repository-policy lifecycle, driven end to end over the wire.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"), dispatched by the multi-service router. Nothing
//! reaches into the state struct, so a response that is not computed from persisted
//! state cannot pass.
//!
//! The tables below span the classes the implementation generalises over — every form a
//! JSON policy document can take (whitespace, member order, one statement or many,
//! `Statement` as an object or a list, escape sequences, both published policy language
//! versions, the published size bound and one byte past it), every way the policy grammar
//! can be broken, and every action/principal form by which a `Deny` can lock the
//! repository's policy — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`),
/// which is also this registry's `registryId`.
const REGISTRY_ID: &str = "000000000000";

/// The published maximum of `RepositoryPolicyText`: `@length(0..10240)`. Source: the
/// model, via `smithy_query(query="shape", shape="RepositoryPolicyText")`.
const MAX_POLICY_TEXT_BYTES: usize = 10240;

/// A policy that grants an ordinary pull: the benign document the cases that are not
/// about the document's own form use.
const PULL_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowPull","Effect":"Allow","Principal":"*","Action":["ecr-public:BatchGetImage","ecr-public:GetDownloadUrlForLayer"]}]}"#;

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope
    // below still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDECRPUBLIC/20260101/us-east-1\
             /ecr-public/aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the named modelled error code.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400 {expected_code}, \
         got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {wire_type:?} in {parsed}"
    );
    parsed
}

/// A required string member of a response object.
fn text(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {member:?}, got {body}"))
        .to_string()
}

/// Create `name` so a policy can be attached to it.
async fn repository(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": name}),
        &format!("seed {name}"),
    )
    .await;
}

/// Assert that `SetRepositoryPolicy` stored `document` and that every reader hands back
/// the same bytes, keeping the registry and repository the request named.
async fn assert_round_trip(router: &MultiServiceRouter, name: &str, document: &str, case: &str) {
    let set = ok(
        router,
        "SetRepositoryPolicy",
        json!({"repositoryName": name, "policyText": document}),
        case,
    )
    .await;
    assert_eq!(
        text(&set, "policyText", case),
        document,
        "case {case:?}: expected SetRepositoryPolicy to report the document it stored, \
         got {set}"
    );
    assert_eq!(
        text(&set, "repositoryName", case),
        name,
        "case {case:?}: expected the repository the request named, got {set}"
    );
    assert_eq!(
        text(&set, "registryId", case),
        REGISTRY_ID,
        "case {case:?}: expected the registry id of this account, got {set}"
    );

    let got = ok(
        router,
        "GetRepositoryPolicy",
        json!({"repositoryName": name}),
        case,
    )
    .await;
    assert_eq!(
        got, set,
        "case {case:?}: expected GetRepositoryPolicy to report what SetRepositoryPolicy \
         stored, got {got}"
    );
}

/// Every form a valid JSON policy document can take, as the IAM JSON policy grammar
/// admits them: both published `Version` values and none at all, `Statement` as one
/// object and as a list, member order, insignificant whitespace, escape sequences,
/// multibyte text, a principal map with a condition, and the published
/// `RepositoryPolicyText` size bound. Source for the grammar: the IAM JSON policy
/// reference; for the size bound, `RepositoryPolicyText`'s modelled `@length(0..10240)`.
fn valid_documents() -> Vec<(String, String)> {
    let padded_base = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Pad","Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}]}"#;
    let padding = MAX_POLICY_TEXT_BYTES - padded_base.len();

    vec![
        ("compact, one statement".to_string(), PULL_POLICY.to_string()),
        (
            "pretty printed, newlines and indentation".to_string(),
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n\t\"Effect\": \
             \"Allow\",\n\t\"Principal\": \"*\",\n\t\"Action\": \"ecr-public:BatchGetImage\"\n   \
             }\n  ]\n}"
                .to_string(),
        ),
        (
            "Statement as a single object".to_string(),
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}}"#
                .to_string(),
        ),
        (
            "no Version member".to_string(),
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}]}"#
                .to_string(),
        ),
        (
            "the 2008-10-17 policy language version".to_string(),
            r#"{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}]}"#
                .to_string(),
        ),
        (
            "three statements, Allow and Deny together".to_string(),
            r#"{"Version":"2012-10-17","Id":"three","Statement":[{"Sid":"a","Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"},{"Sid":"b","Effect":"Deny","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ecr-public:BatchGetImage"},{"Sid":"c","Effect":"Allow","Principal":{"Service":"ecs.amazonaws.com"},"NotAction":"ecr-public:DeleteRepository"}]}"#
                .to_string(),
        ),
        (
            "Statement before Version".to_string(),
            r#"{"Statement":[{"Action":"ecr-public:BatchGetImage","Principal":"*","Effect":"Allow"}],"Version":"2012-10-17"}"#
                .to_string(),
        ),
        (
            "principal map with a condition".to_string(),
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::111122223333:root","000000000000"]},"Action":"ecr-public:*","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/team":"equipe"}}}]}"#
                .to_string(),
        ),
        (
            "multibyte text inside the document".to_string(),
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\
             \"*\",\"Action\":\"ecr-public:BatchGetImage\",\"Condition\":{\"StringEquals\":\
             {\"aws:PrincipalTag/équipe\":\"café — ☕\"}}}]}"
                .to_string(),
        ),
        (
            "escape sequences, which a re-serialized document would lose".to_string(),
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage","Condition":{"StringEquals":{"aws:PrincipalTag\/team":"caf\u00e9 \"quoted\" back\\slash"}}}]}"#
                .to_string(),
        ),
        (
            format!("exactly the {MAX_POLICY_TEXT_BYTES}-byte maximum"),
            format!("{padded_base}{}", " ".repeat(padding)),
        ),
    ]
}

#[tokio::test]
async fn every_valid_document_form_round_trips_byte_for_byte() {
    let router = service();

    for (index, (case, document)) in valid_documents().into_iter().enumerate() {
        // One repository per row, so each row is judged on its own stored document.
        let name = format!("policy/form-{index}");
        repository(&router, &name).await;
        assert_round_trip(&router, &name, &document, &case).await;

        // And the document is readable back after a delete returns it, which is the
        // other end of the same round trip.
        let deleted = ok(
            &router,
            "DeleteRepositoryPolicy",
            json!({"repositoryName": name}),
            &case,
        )
        .await;
        assert_eq!(
            text(&deleted, "policyText", &case),
            document,
            "case {case:?}: expected DeleteRepositoryPolicy to report the document it \
             removed, got {deleted}"
        );
    }
}

#[tokio::test]
async fn a_document_one_byte_past_the_published_maximum_is_refused_and_stores_nothing() {
    let router = service();
    repository(&router, "policy/bounds").await;
    assert_round_trip(&router, "policy/bounds", PULL_POLICY, "seeded policy").await;

    let base = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Pad","Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}]}"#;
    let too_long = format!(
        "{base}{}",
        " ".repeat(MAX_POLICY_TEXT_BYTES + 1 - base.len())
    );
    assert_eq!(
        too_long.len(),
        MAX_POLICY_TEXT_BYTES + 1,
        "the case must be exactly one byte past the maximum, got {}",
        too_long.len()
    );
    refused(
        &router,
        "SetRepositoryPolicy",
        json!({"repositoryName": "policy/bounds", "policyText": too_long}),
        "InvalidParameterException",
        "one byte past the maximum",
    )
    .await;

    // The refusal left the seeded document in place.
    let got = ok(
        &router,
        "GetRepositoryPolicy",
        json!({"repositoryName": "policy/bounds"}),
        "after the refusal",
    )
    .await;
    assert_eq!(
        text(&got, "policyText", "after the refusal"),
        PULL_POLICY,
        "expected the refused call to store nothing, got {got}"
    );
}

/// Every way the IAM JSON policy grammar can be broken, one row per rule: the document
/// is not JSON, not an object, carries an unrecognized element, names no statement, or a
/// statement whose `Effect`, `Action` or `Principal` is absent, of the wrong type or
/// outside its closed set of values. Source: the IAM JSON policy grammar.
fn invalid_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an empty document", ""),
        ("not JSON at all", "not json"),
        ("truncated JSON", r#"{"Statement":[{"Effect":"Allow""#),
        (
            "a trailing comma",
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"*"},]}"#,
        ),
        (
            "a JSON array at the top level",
            r#"[{"Effect":"Allow","Principal":"*","Action":"*"}]"#,
        ),
        ("a JSON string at the top level", r#""a policy""#),
        ("a JSON number at the top level", "17"),
        ("no Statement element", r#"{"Version":"2012-10-17"}"#),
        ("an empty Statement list", r#"{"Statement":[]}"#),
        ("a Statement entry that is not an object", r#"{"Statement":["allow it"]}"#),
        (
            "an unpublished policy language version",
            r#"{"Version":"2019-01-01","Statement":{"Effect":"Allow","Principal":"*","Action":"*"}}"#,
        ),
        (
            "a Version that is not a string",
            r#"{"Version":20121017,"Statement":{"Effect":"Allow","Principal":"*","Action":"*"}}"#,
        ),
        (
            "an unrecognized document element",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":"*"},"Policy":true}"#,
        ),
        (
            "an unrecognized statement element",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":"*","Effects":"Deny"}}"#,
        ),
        (
            "a lower-case Effect",
            r#"{"Statement":{"Effect":"allow","Principal":"*","Action":"*"}}"#,
        ),
        (
            "an upper-case Effect",
            r#"{"Statement":{"Effect":"DENY","Principal":"*","Action":"*"}}"#,
        ),
        (
            "an Effect outside the two published values",
            r#"{"Statement":{"Effect":"Permit","Principal":"*","Action":"*"}}"#,
        ),
        (
            "an Effect that is not a string",
            r#"{"Statement":{"Effect":1,"Principal":"*","Action":"*"}}"#,
        ),
        (
            "no Effect",
            r#"{"Statement":{"Principal":"*","Action":"*"}}"#,
        ),
        (
            "no Action and no NotAction",
            r#"{"Statement":{"Effect":"Allow","Principal":"*"}}"#,
        ),
        (
            "both Action and NotAction",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":"*","NotAction":"*"}}"#,
        ),
        (
            "an empty Action list",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":[]}}"#,
        ),
        (
            "an Action list entry that is not a string",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":[7]}}"#,
        ),
        (
            "no Principal and no NotPrincipal",
            r#"{"Statement":{"Effect":"Allow","Action":"*"}}"#,
        ),
        (
            "both Principal and NotPrincipal",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","NotPrincipal":"*","Action":"*"}}"#,
        ),
        (
            "a Principal string other than the wildcard",
            r#"{"Statement":{"Effect":"Allow","Principal":"everyone","Action":"*"}}"#,
        ),
        (
            "an empty Principal map",
            r#"{"Statement":{"Effect":"Allow","Principal":{},"Action":"*"}}"#,
        ),
        (
            "an unrecognized principal type",
            r#"{"Statement":{"Effect":"Allow","Principal":{"Human":"me"},"Action":"*"}}"#,
        ),
        (
            "a Condition that is not an object",
            r#"{"Statement":{"Effect":"Allow","Principal":"*","Action":"*","Condition":"always"}}"#,
        ),
    ]
}

#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_and_stores_nothing() {
    let router = service();
    repository(&router, "policy/grammar").await;
    assert_round_trip(&router, "policy/grammar", PULL_POLICY, "seeded policy").await;

    for (case, document) in invalid_documents() {
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": "policy/grammar", "policyText": document}),
            "InvalidParameterException",
            case,
        )
        .await;
        // `force` is about lockout, never about accepting a document that is not one.
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": "policy/grammar", "policyText": document, "force": true}),
            "InvalidParameterException",
            case,
        )
        .await;

        let got = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": "policy/grammar"}),
            case,
        )
        .await;
        assert_eq!(
            text(&got, "policyText", case),
            PULL_POLICY,
            "case {case:?}: expected the refused call to leave the stored policy alone, \
             got {got}"
        );
    }
}

#[tokio::test]
async fn the_policy_lifecycle_is_set_read_replace_delete() {
    let router = service();
    repository(&router, "policy/lifecycle").await;
    repository(&router, "policy/neighbour").await;

    // Nothing is stored until a policy is set.
    for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
        refused(
            &router,
            operation,
            json!({"repositoryName": "policy/lifecycle"}),
            "RepositoryPolicyNotFoundException",
            "before any policy is set",
        )
        .await;
    }

    assert_round_trip(&router, "policy/lifecycle", PULL_POLICY, "first policy").await;

    // A second set REPLACES the document rather than adding to it.
    let replacement = r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowDescribe","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ecr-public:DescribeImages"}]}"#;
    assert_round_trip(&router, "policy/lifecycle", replacement, "replacement").await;

    // A policy belongs to the repository it was set on, not to the registry.
    refused(
        &router,
        "GetRepositoryPolicy",
        json!({"repositoryName": "policy/neighbour"}),
        "RepositoryPolicyNotFoundException",
        "a neighbouring repository",
    )
    .await;

    // The delete reports what it removed, and removes it.
    let deleted = ok(
        &router,
        "DeleteRepositoryPolicy",
        json!({"repositoryName": "policy/lifecycle"}),
        "delete",
    )
    .await;
    assert_eq!(
        text(&deleted, "policyText", "delete"),
        replacement,
        "expected the delete to report the replacement document, got {deleted}"
    );
    for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
        refused(
            &router,
            operation,
            json!({"repositoryName": "policy/lifecycle"}),
            "RepositoryPolicyNotFoundException",
            "after the delete",
        )
        .await;
    }

    // And the repository takes a policy again afterwards.
    assert_round_trip(&router, "policy/lifecycle", PULL_POLICY, "after the delete").await;
}

#[tokio::test]
async fn a_policy_operation_on_a_repository_that_does_not_exist_is_not_found() {
    let router = service();
    repository(&router, "policy/present").await;

    for name in ["policy/absent", "absent", "other/team/absent"] {
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": name, "policyText": PULL_POLICY}),
            "RepositoryNotFoundException",
            name,
        )
        .await;
        for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
            refused(
                &router,
                operation,
                json!({"repositoryName": name}),
                "RepositoryNotFoundException",
                name,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_registry_id_naming_another_account_is_an_invalid_parameter() {
    let router = service();
    repository(&router, "policy/registry").await;

    // This registry's own id is accepted wherever the member is accepted at all.
    let set = ok(
        &router,
        "SetRepositoryPolicy",
        json!({
            "registryId": REGISTRY_ID,
            "repositoryName": "policy/registry",
            "policyText": PULL_POLICY,
        }),
        "this registry",
    )
    .await;
    assert_eq!(
        text(&set, "policyText", "this registry"),
        PULL_POLICY,
        "expected the document to be stored for this registry, got {set}"
    );

    // Any other account id names a registry this emulator does not serve.
    for other in ["111122223333", "999999999999"] {
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({
                "registryId": other,
                "repositoryName": "policy/registry",
                "policyText": PULL_POLICY,
            }),
            "InvalidParameterException",
            other,
        )
        .await;
        for operation in ["GetRepositoryPolicy", "DeleteRepositoryPolicy"] {
            refused(
                &router,
                operation,
                json!({"registryId": other, "repositoryName": "policy/registry"}),
                "InvalidParameterException",
                other,
            )
            .await;
        }
    }

    // The stored document survived every refusal.
    let got = ok(
        &router,
        "GetRepositoryPolicy",
        json!({"repositoryName": "policy/registry"}),
        "after the refusals",
    )
    .await;
    assert_eq!(
        text(&got, "policyText", "after the refusals"),
        PULL_POLICY,
        "expected the stored document to survive, got {got}"
    );
}

/// Documents that deny `ecr-public:SetRepositoryPolicy` to this registry's own account,
/// one row per action form (an exact action, both IAM wildcards, a whole-service and a
/// whole-partition wildcard, the case-insensitive match, and `NotAction`) and per
/// principal form (the wildcard, the bare account id, that account's ARN).
fn lockout_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "every action, every principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"*"}]}"#,
        ),
        (
            "every action of this service",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:*"}]}"#,
        ),
        (
            "the exact action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":["ecr-public:BatchGetImage","ecr-public:SetRepositoryPolicy"]}]}"#,
        ),
        (
            "a trailing wildcard",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:Set*"}]}"#,
        ),
        (
            "a single-character wildcard",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:SetRepositor?Policy"}]}"#,
        ),
        (
            "an action named in another case",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ECR-PUBLIC:setrepositorypolicy"}]}"#,
        ),
        (
            "NotAction, which denies everything else",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","NotAction":"ecr-public:BatchGetImage"}]}"#,
        ),
        (
            "this account by id",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":"000000000000"},"Action":"ecr-public:SetRepositoryPolicy"}]}"#,
        ),
        (
            "this account by ARN",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":["arn:aws:iam::000000000000:root"]},"Action":"ecr-public:*"}]}"#,
        ),
        (
            "every principal except another account",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","NotPrincipal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"ecr-public:SetRepositoryPolicy"}]}"#,
        ),
        (
            "a second statement that denies it",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"},{"Effect":"Deny","Principal":"*","Action":"ecr-public:SetRepositoryPolicy"}]}"#,
        ),
    ]
}

/// Documents that do NOT deny `ecr-public:SetRepositoryPolicy` to this account, so they
/// need no `force`: a deny of another action, an allow of this one, a deny aimed at
/// another account, a deny whose principal set excludes everyone, and a deny that only
/// applies under a request condition this service does not evaluate.
fn non_lockout_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "a deny of another action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:DeleteRepository"}]}"#,
        ),
        (
            "a deny of a prefix that stops short of this action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"ecr-public:SetRepositoryPolicyAndMore"}]}"#,
        ),
        (
            "an allow of this action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:SetRepositoryPolicy"}]}"#,
        ),
        (
            "a deny aimed at another account",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"*"}]}"#,
        ),
        (
            "a deny aimed at another account whose resource path embeds this account id",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::111122223333:role/path:000000000000:admin"},"Action":"*"}]}"#,
        ),
        (
            "a deny whose principal set excludes everyone",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","NotPrincipal":"*","Action":"*"}]}"#,
        ),
        (
            "a deny that applies only under a condition",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","Action":"*","Condition":{"StringNotEquals":{"aws:PrincipalAccount":"000000000000"}}}]}"#,
        ),
        (
            "NotAction that spares this action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","NotAction":["ecr-public:SetRepositoryPolicy","ecr-public:DeleteRepositoryPolicy"]}]}"#,
        ),
    ]
}

#[tokio::test]
async fn a_document_that_would_lock_the_policy_needs_force_and_then_really_locks_it() {
    let router = service();

    for (index, (case, document)) in lockout_documents().into_iter().enumerate() {
        // One repository per row: the lockout this row installs must not decide the next.
        let name = format!("policy/lockout-{index}");
        repository(&router, &name).await;
        assert_round_trip(&router, &name, PULL_POLICY, case).await;

        // Without `force` the call is refused, and the previous document stands.
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": name, "policyText": document}),
            "InvalidParameterException",
            case,
        )
        .await;
        let got = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_eq!(
            text(&got, "policyText", case),
            PULL_POLICY,
            "case {case:?}: expected the unforced call to store nothing, got {got}"
        );

        // With `force` it is stored...
        assert_round_trip_forced(&router, &name, document, case).await;

        // ...and the lockout is real: the next call is refused however it is made.
        for forced in [false, true] {
            refused(
                &router,
                "SetRepositoryPolicy",
                json!({
                    "repositoryName": name,
                    "policyText": PULL_POLICY,
                    "force": forced,
                }),
                "InvalidParameterException",
                case,
            )
            .await;
        }
        let still = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_eq!(
            text(&still, "policyText", case),
            document,
            "case {case:?}: expected the locking document to still be the stored one, \
             got {still}"
        );

        // Deleting the policy lifts the lockout: the repository takes a policy again.
        ok(
            &router,
            "DeleteRepositoryPolicy",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_round_trip(&router, &name, PULL_POLICY, case).await;
    }
}

/// [`assert_round_trip`] for a document that needs `force` to be accepted.
async fn assert_round_trip_forced(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    case: &str,
) {
    let set = ok(
        router,
        "SetRepositoryPolicy",
        json!({"repositoryName": name, "policyText": document, "force": true}),
        case,
    )
    .await;
    assert_eq!(
        text(&set, "policyText", case),
        document,
        "case {case:?}: expected the forced call to store the document, got {set}"
    );
    let got = ok(
        router,
        "GetRepositoryPolicy",
        json!({"repositoryName": name}),
        case,
    )
    .await;
    assert_eq!(
        got, set,
        "case {case:?}: expected the forced document to be readable back, got {got}"
    );
}

#[tokio::test]
async fn a_document_that_does_not_deny_this_account_needs_no_force() {
    let router = service();

    for (index, (case, document)) in non_lockout_documents().into_iter().enumerate() {
        let name = format!("policy/open-{index}");
        repository(&router, &name).await;

        // No `force`, accepted, and the repository's policy can still be replaced
        // afterwards — nothing was locked.
        assert_round_trip(&router, &name, document, case).await;
        assert_round_trip(&router, &name, PULL_POLICY, case).await;
    }
}
