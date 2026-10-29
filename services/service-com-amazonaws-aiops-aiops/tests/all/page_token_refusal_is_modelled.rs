// SPDX-License-Identifier: CC0-1.0

//! Which ERROR SHAPE this service answers a handler-enforced refusal with.
//!
//! The rule under test: when a handler of this service refuses bad client input, the
//! error value it returns is the shape THIS service's model declares for a client-input
//! refusal — `com.amazonaws.aiops#ValidationException` (`@error client`,
//! `@httpError 400`), which the model binds to every operation as a service-common error
//! — and never a shape from outside the model that merely renders the same status and
//! code. `ListInvestigationGroups` is the one operation left answering from such an
//! out-of-model shape, so its `nextToken` refusal is where the rule is pinned.
//!
//! Each row is asserted on the value the HANDLER returns, called directly with a typed
//! input: the wire form of a restJson1 error carries only the status, the `__type` code
//! and `message`, so the shape identity — the thing the rule is about — is observable
//! only on the typed error. The identity is read off the SCHEMA the error channel
//! renders with (`OperationError::variant_schema`), which is what would carry a member
//! the modelled shape grew. Every row is then driven over the wire too, so the status
//! and code a signed SDK request observes are pinned alongside.
//!
//! The rows differ along what the rule generalises over: the FORM a token no page ever
//! minted can take (text that is not hex, the empty string, odd-length hex, non-hex
//! letters, hex of bytes that are not UTF-8, hex of an id one character either side of
//! the id length, hex of sixteen characters outside the id class, an id-shaped value
//! that was never hex-encoded, and hex far longer than a minted token). The positive
//! controls are the two tokens the operation serves: none at all, and the hex form it
//! mints for a group id.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_aiops_aiops as aiops;

use aiops::ops::list_investigation_groups::{
    ListInvestigationGroups, ListInvestigationGroupsError, ListInvestigationGroupsErrorModeled,
};
use aiops::ops::types::{ListInvestigationGroupsInput, ListInvestigationGroupsOutput};
use aiops::scaffolded::Scaffolded;
use aiops::state;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The shape id this SERVICE declares for a client-input refusal, which a refusal has to
/// carry — `smithy_query(query="shape",shape="ValidationException")`.
const MODELLED_VALIDATION_SHAPE: &str = "com.amazonaws.aiops#ValidationException";

/// The wire error code that shape renders with on this service's restJson1 protocol.
const VALIDATION_CODE: &str = "ValidationException";

/// The requirement clause a refused `nextToken` is worded with: the token is opaque, so
/// the only requirement it can state is where a valid one comes from.
const NEXT_TOKEN_REQUIREMENT: &str =
    "Member must be a pagination token returned by a previous ListInvestigationGroups call";

/// One emulator: the shared engine, the service mounted on a router (for the callback a
/// handler is given, and for the wire leg of each row) and the request metadata a
/// handler reads its Region from.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    service: aiops::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(aiops::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: aiops::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// The listing as the HANDLER answers it, so the refusal's own type is observable.
    async fn list(
        &self,
        next_token: Option<&str>,
    ) -> Result<ListInvestigationGroupsOutput, ListInvestigationGroupsError> {
        ListInvestigationGroups::new(Arc::clone(&self.engine))
            .handle(
                ListInvestigationGroupsInput {
                    next_token: next_token.map(str::to_string),
                    max_results: None,
                },
                &self.ctx(),
            )
            .await
    }

    /// Create a group over the wire, so the listing has state, and answer its ARN.
    async fn create(&self, name: &str) -> String {
        let (status, created) = self
            .call(
                "POST",
                "/investigationGroups",
                Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
            )
            .await;
        assert_eq!(
            status, 201,
            "create {name:?}: expected 201, got {status} / {created:?}"
        );
        created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("create {name:?}: no arn in {created:?}"))
            .to_string()
    }

    /// The listing as a signed SDK request observes it, for `token` carried as the
    /// `@httpQuery`-bound `nextToken`.
    async fn list_over_the_wire(&self, token: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/investigationGroups?nextToken={}", encode(token)),
            None,
        )
        .await
    }

    /// Status and parsed body of one wire call.
    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("aiops.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            );
        let payload = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = builder.body(payload).expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let mut parsed: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
        };
        // restJson1 carries the error shape in `__type`, and the framework also sets the
        // `x-amzn-errortype` header; fold the header in so one accessor reads either.
        if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
            map.entry("__type")
                .or_insert_with(|| Value::String(error_type));
        }
        (status, parsed)
    }
}

/// Request metadata naming this service's signing name and a Region, which is how a
/// request names the Region a handler reads.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("GET")
        .uri("/investigationGroups")
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// Percent-encode everything a URI query value may not carry literally — what an SDK
/// does for an `@httpQuery`-bound member.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The wire error code a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// `nextToken` values no page of this operation ever handed out, as
/// `(case, the token)`. The rows differ along the FORM alone: a group id is
/// `state::GROUP_ID_LEN` characters from `[A-Za-z0-9]`, hex-encoded into a token, so a
/// row breaks the hex step, the UTF-8 step, the length or the character class.
fn forged_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("text that is not hex at all", "not-a-token".to_string()),
        ("the empty token", String::new()),
        ("hex digits of odd length", "abc".to_string()),
        ("letters outside the hex alphabet", "zzzz".to_string()),
        (
            "hex of bytes that are not UTF-8",
            hex::encode([0xff_u8, 0xfe_u8]),
        ),
        (
            "hex of an id one character short",
            hex::encode("A".repeat(state::GROUP_ID_LEN - 1)),
        ),
        (
            "hex of an id one character long",
            hex::encode("A".repeat(state::GROUP_ID_LEN + 1)),
        ),
        (
            "hex of id-length text outside the id character class",
            hex::encode("AAAA-AAAA.AAAA/A"),
        ),
        (
            "an id-shaped value that was never hex-encoded",
            "0123456789abcdef".to_string(),
        ),
        (
            "hex far longer than any token this operation mints",
            hex::encode("A".repeat(600)),
        ),
    ]
}

/// The `message` of a refusal that MUST be this service's own modelled
/// `ValidationException`, or a panic naming the case.
fn modelled_refusal_message(case: &str, error: &ListInvestigationGroupsError) -> String {
    match error {
        Scaffolded::Modeled(ListInvestigationGroupsErrorModeled::ValidationException(refusal)) => {
            refusal.message.clone().unwrap_or_default()
        }
        other => panic!(
            "case {case:?}: the refusal must be the ValidationException this service's model \
             declares, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn a_forged_page_token_is_refused_with_the_shape_this_service_models() {
    let emulator = Emulator::new();
    let arn = emulator.create("token-refusal-group").await;
    let stored_id = arn
        .rsplit('/')
        .next()
        .expect("an investigation-group ARN names its id")
        .to_string();
    let expected_message = state::validation_message("nextToken", NEXT_TOKEN_REQUIREMENT);

    for (case, token) in forged_tokens() {
        let refused = emulator.list(Some(&token)).await.expect_err(&format!(
            "case {case:?}: a token this operation never issued must be refused, token \
             {token:?}"
        ));

        // The rule: the refusal carries the shape the MODEL declares for a bad input,
        // read off the schema the error channel renders it with.
        let shape = refused.variant_schema().shape_id().to_string();
        assert_eq!(
            shape, MODELLED_VALIDATION_SHAPE,
            "case {case:?}: expected the modelled shape {MODELLED_VALIDATION_SHAPE}, got \
             {shape:?}"
        );
        assert_eq!(
            refused.http_status(),
            400,
            "case {case:?}: expected 400, got {}",
            refused.http_status()
        );
        assert_eq!(
            refused.shape_name(),
            VALIDATION_CODE,
            "case {case:?}: expected the wire code {VALIDATION_CODE}, got {:?}",
            refused.shape_name()
        );

        // The refusal still reads like every other refusal this service makes, and a
        // `@sensitive` token is never echoed.
        let message = modelled_refusal_message(case, &refused);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
        assert!(
            token.is_empty() || !message.contains(&token),
            "case {case:?}: the refusal must not echo the token, got {message:?}"
        );

        // The same row over the wire: the status and code a signed request observes. An
        // empty query value is not carried as a token at all, so it has no wire leg.
        if !token.is_empty() {
            let (status, body) = emulator.list_over_the_wire(&token).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 over the wire, got {status} / {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, VALIDATION_CODE,
                "case {case:?}: expected {VALIDATION_CODE} over the wire, got {body:?}"
            );
        }
    }

    // The boundary of the rule: the two tokens the operation SERVES are not refused —
    // none at all, and the hex form a page mints for a group id.
    for (case, token) in [
        ("no token at all", None),
        (
            "the token form this operation mints",
            Some(hex::encode(&stored_id)),
        ),
    ] {
        let listed = emulator
            .list(token.as_deref())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: must be served, got {error:?}"));
        let arns: Vec<String> = listed
            .investigation_groups
            .unwrap_or_default()
            .into_iter()
            .filter_map(|group| group.arn)
            .collect();
        assert_eq!(
            arns,
            vec![arn.clone()],
            "case {case:?}: the Region's one group must be reported, got {arns:?}"
        );
    }
}
