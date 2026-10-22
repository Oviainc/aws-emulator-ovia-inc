// SPDX-License-Identifier: CC0-1.0

//! The SignIn OAuth 2.0 token, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones this service states for its tokens:
//!
//! * `CreateOAuth2TokenWithIAM` mints an access token and persists the claims it was
//!   minted with, so `IntrospectOAuth2TokenWithIAM` reports exactly those claims;
//! * introspection of a token that is unknown, expired, revoked or owned by another
//!   account answers the RFC 7662 §2.2 body and nothing else — `{"active": false}`;
//! * `RevokeOAuth2TokenWithIAM` invalidates the refresh token it names together with
//!   the whole chain minted from it, and answers 200 with an empty body however many
//!   times it is called with however invalid a token.
//!
//! A refresh token enters the store only through `CreateOAuth2Token` (the Human OAuth
//! flow), which is still a scaffold stub, so the refresh-token rows below seed the store
//! through the same `state::ComAmazonawsSigninSigninState::issue_token` the handlers
//! mint with, over the one shared engine the router serves from.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signin_signin::state::{
    ComAmazonawsSigninSigninState, NewToken, TOKEN_TYPE_ACCESS, TOKEN_TYPE_REFRESH,
};
use service_com_amazonaws_signin_signin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service — what routes a request to signin, and
/// the region every ARN minted while serving it carries.
const SIGNED_FOR_SIGNIN: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/signin/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region in [`SIGNED_FOR_SIGNIN`]'s credential scope.
const REGION: &str = "us-east-1";

/// The account id the emulator answers for.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// The `iss` claim the model fixes: "Always "signin.amazonaws.com" for AWS Sign-In"
/// (`IntrospectOAuth2TokenWithIAMResponse$iss`). Spelled out here rather than imported,
/// so a wrong value in `src/state.rs` fails this test.
const ISSUER: &str = "signin.amazonaws.com";

/// The IAM principal ARN and the AWS unique id this emulator attributes a request to:
/// the ambient identity `sts:GetCallerIdentity` reports for a request whose access key
/// names no session, which `$sub`/`$userId` say the claims must match.
const SUBJECT: &str = "arn:aws:iam::000000000000:user/emulator";

/// `CreateOAuth2TokenWithIAM`'s only grant type: `ClientCredentialsGrantType` is
/// modelled `^client_credentials$`.
const CLIENT_CREDENTIALS: &str = "client_credentials";

/// Every claim `IntrospectOAuth2TokenWithIAMResponse` declares beside `active`, by its
/// wire name — the members RFC 7662 §2.2 says are "omitted" from an inactive answer.
/// Written out from the model's `jsonName`s rather than imported.
const CLAIM_MEMBERS: &[&str] = &[
    "client_id",
    "user_id",
    "token_type",
    "exp",
    "iat",
    "nbf",
    "sub",
    "aud",
    "iss",
    "jti",
    "account_id",
    "signin_session",
    "resource",
];

/// One emulator: a router carrying only this service, over one shared engine the test
/// also holds, so a case can seed a token the way a handler mints one.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

impl Emulator {
    /// Mints and persists a token through the service's own store, and hands back the
    /// opaque value a caller would present. `issued_at` is when the token was issued,
    /// so a case can place a token's lifetime in the past.
    fn seed(&self, new: NewToken, issued_at: SystemTime) -> String {
        self.engine
            .state::<ComAmazonawsSigninSigninState>()
            .lock()
            .issue_token(new, issued_at)
            .token
    }

    /// The chain and session of the stored token `value` names, so a case can mint a
    /// second token into the same chain.
    fn chain_of(&self, value: &str) -> (String, String) {
        let store = self.engine.state::<ComAmazonawsSigninSigninState>();
        let store = store.lock();
        let token = store
            .token(value)
            .expect("the seeded token is in the store");
        (token.chain.clone(), token.session_arn.clone())
    }

    /// Re-attributes the stored token `value` names to `account_id`, which is the one
    /// claim no request of this service can set: it is the account of the token's
    /// subject principal.
    fn reassign_account(&self, value: &str, account_id: &str) {
        let store = self.engine.state::<ComAmazonawsSigninSigninState>();
        let mut store = store.lock();
        store
            .oauth_tokens
            .get_mut(value)
            .expect("the seeded token is in the store")
            .account_id = account_id.to_string();
    }
}

/// A token to seed of `token_type`, scoped to `resource`, living `expires_in` seconds.
fn new_token(token_type: &'static str, resource: &str, expires_in: i64) -> NewToken {
    NewToken {
        token_type,
        resource: resource.to_string(),
        client_id: None,
        region: REGION.to_string(),
        expires_in,
        chain: None,
        session_arn: None,
    }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or_default()
    }

    /// A string member of the body, or the empty string when it carries none.
    fn string(&self, member: &str) -> &str {
        self.body[member].as_str().unwrap_or_default()
    }

    /// A numeric member of the body, or `None` when it carries none.
    fn number(&self, member: &str) -> Option<i64> {
        self.body[member].as_i64()
    }

    /// Whether the body carries `member` at all.
    fn has(&self, member: &str) -> bool {
        self.body.get(member).is_some_and(|value| !value.is_null())
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "signin.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_SIGNIN)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create_token(em: &Emulator, resource: &str) -> Answer {
    call(
        &em.router,
        "/v1/token?x-amz-client-auth-method=iam",
        json!({"grant_type": CLIENT_CREDENTIALS, "resource": resource}),
    )
    .await
}

async fn introspect(em: &Emulator, request: Value) -> Answer {
    call(
        &em.router,
        "/v1/introspect?x-amz-client-auth-method=iam",
        request,
    )
    .await
}

async fn revoke(em: &Emulator, token: &str) -> Answer {
    call(
        &em.router,
        "/v1/revoke?x-amz-client-auth-method=iam",
        json!({"token": token}),
    )
    .await
}

/// `IntrospectionToken`'s `^ASO[AR][A-Za-z0-9+/=_\-]+$` with length 1..4096, checked
/// here rather than through the crate so a wrong token value cannot make this test pass.
fn matches_introspection_token(value: &str) -> bool {
    let body = value
        .strip_prefix("ASOA")
        .or_else(|| value.strip_prefix("ASOR"));
    body.is_some_and(|body| {
        !body.is_empty()
            && value.len() <= 4096
            && body.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
            })
    })
}

/// The canonical hyphenated form of a UUID: 8-4-4-4-12 lowercase hex digits.
fn matches_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

/// Now, in whole seconds since the Unix epoch — the NumericDate the `exp`, `iat` and
/// `nbf` claims are modelled in.
fn now_epoch() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the epoch second fits in an i64")
}

/// A minted access token introspects to the claims it was minted with.
///
/// The rows differ in the resource the token is scoped to — its value, its length at
/// each published bound of `resource`'s `@length(min: 1, max: 2048)`, and its form —
/// which is what the claim projection generalises over.
#[tokio::test]
async fn a_minted_access_token_introspects_to_the_claims_it_carries() {
    // Rows: the model's own example for `resource` ("aws-mcp.amazonaws.com"), both
    // published length bounds, and other forms an OAuth resource identifier takes.
    let cases: Vec<(&str, String)> = vec![
        ("the model's example resource", "aws-mcp.amazonaws.com".to_string()),
        ("a resource at the 1-character length floor", "a".to_string()),
        ("a resource at the 2048-character ceiling", "r".repeat(2048)),
        ("a URL-form resource", "https://example.amazonaws.com/mcp".to_string()),
        ("a dotted host resource", "bedrock-runtime.us-west-2.amazonaws.com".to_string()),
        ("a resource with a port and path", "https://example.aws:8443/a/b".to_string()),
    ];

    let em = emulator();
    let mut seen_tokens = Vec::new();
    let mut seen_jtis = Vec::new();
    for (case, resource) in &cases {
        let before = now_epoch();
        let created = create_token(&em, resource).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a mint must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let token = created.string("access_token").to_string();
        assert!(
            matches_introspection_token(&token),
            "case {case:?}: access_token must satisfy IntrospectionToken's pattern, got a \
             {}-character value starting {:?}",
            token.len(),
            token.chars().take(8).collect::<String>()
        );
        assert_eq!(
            created.string("token_type"),
            "Bearer",
            "case {case:?}: tokenType is modelled ^Bearer$, got {:?}",
            created.body["token_type"]
        );
        let expires_in = created
            .number("expires_in")
            .unwrap_or_else(|| panic!("case {case:?}: expiresIn is required, got {:?}", created.body));
        assert!(
            (1..=3600).contains(&expires_in),
            "case {case:?}: expiresIn is modelled @range(1..3600), got {expires_in}"
        );

        let answer = introspect(&em, json!({"token": token})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: introspection must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["active"],
            json!(true),
            "case {case:?}: a token just minted is active, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("token_type"),
            "access_token",
            "case {case:?}: the client-credentials grant issues an access_token, got {:?}",
            answer.body["token_type"]
        );
        assert_eq!(
            answer.string("aud"),
            resource.as_str(),
            "case {case:?}: aud is the resource the token is scoped to, got {:?}",
            answer.body["aud"]
        );
        assert!(
            !answer.has("resource"),
            "case {case:?}: the resource claim is only present for refresh tokens, got {:?}",
            answer.body["resource"]
        );
        assert!(
            !answer.has("client_id"),
            "case {case:?}: the IAM flow has no OAuth client to report, got {:?}",
            answer.body["client_id"]
        );
        assert_eq!(
            answer.string("iss"),
            ISSUER,
            "case {case:?}: expected iss {ISSUER}, got {:?}",
            answer.body["iss"]
        );
        assert_eq!(
            answer.string("account_id"),
            EMULATOR_ACCOUNT,
            "case {case:?}: expected accountId {EMULATOR_ACCOUNT}, got {:?}",
            answer.body["account_id"]
        );
        assert_eq!(
            answer.string("sub"),
            SUBJECT,
            "case {case:?}: expected sub {SUBJECT}, got {:?}",
            answer.body["sub"]
        );
        let user_id = answer.string("user_id");
        assert!(
            user_id.len() == 21
                && user_id.starts_with("AIDA")
                && user_id
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "case {case:?}: userId must be an AWS unique id of an IAM user, got {user_id:?}"
        );
        let session = answer.string("signin_session");
        let expected_session_prefix = format!("arn:aws:signin:{REGION}:{EMULATOR_ACCOUNT}:session/");
        assert!(
            session
                .strip_prefix(&expected_session_prefix)
                .is_some_and(matches_uuid),
            "case {case:?}: signinSession must be {expected_session_prefix}{{uuid}}, got \
             {session:?}"
        );

        let iat = answer
            .number("iat")
            .unwrap_or_else(|| panic!("case {case:?}: iat must be reported, got {:?}", answer.body));
        let exp = answer
            .number("exp")
            .unwrap_or_else(|| panic!("case {case:?}: exp must be reported, got {:?}", answer.body));
        let nbf = answer
            .number("nbf")
            .unwrap_or_else(|| panic!("case {case:?}: nbf must be reported, got {:?}", answer.body));
        assert!(
            (before..=now_epoch()).contains(&iat),
            "case {case:?}: iat must be the issuance second, got {iat} outside \
             {before}..={}",
            now_epoch()
        );
        assert_eq!(
            exp,
            iat + expires_in,
            "case {case:?}: exp must be iat plus the promised lifetime ({iat} + \
             {expires_in}), got {exp}"
        );
        assert_eq!(
            nbf, iat,
            "case {case:?}: a token is usable from issuance, so nbf must equal iat {iat}, \
             got {nbf}"
        );

        let jti = answer.string("jti").to_string();
        assert!(
            matches_uuid(&jti),
            "case {case:?}: jti must identify the token, got {jti:?}"
        );
        assert!(
            !seen_jtis.contains(&jti),
            "case {case:?}: jti {jti:?} was already reported for another token"
        );
        assert!(
            !seen_tokens.contains(&token),
            "case {case:?}: the mint handed out a token value twice"
        );

        // A read changes nothing: the same call answers the same claims.
        let again = introspect(&em, json!({"token": token})).await;
        assert_eq!(
            again.body, answer.body,
            "case {case:?}: introspecting twice must answer the same claims, got {:?} then \
             {:?}",
            answer.body, again.body
        );

        seen_tokens.push(token);
        seen_jtis.push(jti);
    }
    assert_eq!(
        seen_tokens.len(),
        cases.len(),
        "every case must have minted its own token"
    );
}

/// `tokenTypeHint` only optimizes the lookup: a hint that names the other kind, the
/// right kind, or no hint at all answers the same claims for the same token.
#[tokio::test]
async fn the_token_type_hint_does_not_change_the_claims() {
    let em = emulator();
    let access = create_token(&em, "aws-mcp.amazonaws.com")
        .await
        .string("access_token")
        .to_string();
    let refresh = em.seed(
        new_token(TOKEN_TYPE_REFRESH, "aws-console.amazonaws.com", 3600),
        SystemTime::now(),
    );

    // Rows: each stored token crossed with every hint the model admits
    // (`TokenTypeHint` is `^(access_token|refresh_token)$`) and with no hint at all.
    for (kind, token) in [("an access token", &access), ("a refresh token", &refresh)] {
        let unhinted = introspect(&em, json!({"token": token})).await;
        assert_eq!(
            unhinted.body["active"],
            json!(true),
            "case {kind:?}: the stored token must be active, got {:?}",
            unhinted.body
        );
        for hint in [TOKEN_TYPE_ACCESS, TOKEN_TYPE_REFRESH] {
            let hinted = introspect(&em, json!({"token": token, "token_type_hint": hint})).await;
            assert_eq!(
                hinted.body, unhinted.body,
                "case {kind:?} with hint {hint:?}: the hint must not change the claims, got \
                 {:?} against {:?}",
                hinted.body, unhinted.body
            );
        }
    }

    // And the two kinds report their own projection of the stored resource: `aud` for an
    // access token ("Omitted for refresh tokens"), `resource` for a refresh token
    // ("Only present for refresh token introspection").
    let refresh_claims = introspect(&em, json!({"token": &refresh})).await;
    assert_eq!(
        refresh_claims.string("token_type"),
        "refresh_token",
        "a refresh token reports its own tokenType, got {:?}",
        refresh_claims.body["token_type"]
    );
    assert_eq!(
        refresh_claims.string("resource"),
        "aws-console.amazonaws.com",
        "a refresh token reports the resource it is scoped to, got {:?}",
        refresh_claims.body["resource"]
    );
    assert!(
        !refresh_claims.has("aud"),
        "aud is omitted for refresh tokens, got {:?}",
        refresh_claims.body["aud"]
    );
}

/// RFC 7662 §2.2, as this operation quotes it: a token that is unknown, expired, revoked
/// or owned by another account answers a body that is EXACTLY `{"active": false}`.
///
/// The rows span every reason the model names, plus the published length bounds of a
/// token value nothing was ever issued for.
#[tokio::test]
async fn an_inactive_token_answers_exactly_active_false() {
    let em = emulator();
    let now = SystemTime::now();

    // Expired: issued far enough in the past that its own lifetime has run out.
    let expired = em.seed(
        new_token(TOKEN_TYPE_ACCESS, "aws-mcp.amazonaws.com", 3600),
        now - Duration::from_secs(7200),
    );
    // Revoked: a refresh token this test revokes through the API below.
    let revoked = em.seed(
        new_token(TOKEN_TYPE_REFRESH, "aws-console.amazonaws.com", 86_400),
        now,
    );
    // Minted from that refresh token, so revoking the refresh token revokes it too.
    let (chain, session_arn) = em.chain_of(&revoked);
    let chained = em.seed(
        NewToken {
            chain: Some(chain),
            session_arn: Some(session_arn),
            ..new_token(TOKEN_TYPE_ACCESS, "aws-console.amazonaws.com", 3600)
        },
        now,
    );
    assert_eq!(
        revoke(&em, &revoked).await.status,
        200,
        "revoking the seeded refresh token must be served"
    );
    // Another account's token: the one claim no request can set.
    let other_account = em.seed(
        new_token(TOKEN_TYPE_ACCESS, "aws-mcp.amazonaws.com", 3600),
        now,
    );
    em.reassign_account(&other_account, "123456789012");

    let cases: Vec<(&str, String)> = vec![
        (
            "an access-token value nothing was issued for",
            format!("ASOA{}", "a".repeat(64)),
        ),
        (
            "a refresh-token value nothing was issued for",
            format!("ASOR{}", "0".repeat(64)),
        ),
        (
            "an unissued value at the 5-character length floor",
            "ASOAx".to_string(),
        ),
        (
            "an unissued value at the 4096-character ceiling",
            format!("ASOR{}", "z".repeat(4092)),
        ),
        (
            "an unissued value of punctuation the pattern admits",
            "ASOA+/=_-".to_string(),
        ),
        ("a token whose lifetime has run out", expired),
        ("a revoked refresh token", revoked),
        ("an access token in a revoked chain", chained),
        ("a token belonging to another account", other_account),
    ];

    for (case, token) in &cases {
        let answer = introspect(&em, json!({"token": token})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: introspection must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body,
            json!({"active": false}),
            "case {case:?}: an inactive token answers exactly {{\"active\": false}}, got {:?}",
            answer.body
        );
        for member in CLAIM_MEMBERS {
            assert!(
                !answer.has(member),
                "case {case:?}: the {member} claim must be omitted, got {:?}",
                answer.body[*member]
            );
        }
    }
}

/// Revoking a refresh token invalidates the whole chain minted from it, leaves every
/// other chain alone, and answers 200 with an empty body however often it is repeated
/// and however invalid the token it names.
#[tokio::test]
async fn revoking_a_refresh_token_is_idempotent_and_revokes_its_chain() {
    let em = emulator();
    let now = SystemTime::now();

    let refresh = em.seed(
        new_token(TOKEN_TYPE_REFRESH, "aws-console.amazonaws.com", 86_400),
        now,
    );
    let (chain, session_arn) = em.chain_of(&refresh);
    // Two access tokens minted from that refresh token: the chain the revocation walks.
    let chained: Vec<String> = (0..2)
        .map(|_| {
            em.seed(
                NewToken {
                    chain: Some(chain.clone()),
                    session_arn: Some(session_arn.clone()),
                    ..new_token(TOKEN_TYPE_ACCESS, "aws-console.amazonaws.com", 3600)
                },
                now,
            )
        })
        .collect();
    // And one minted through the IAM flow, which heads its own chain.
    let unrelated = create_token(&em, "aws-mcp.amazonaws.com")
        .await
        .string("access_token")
        .to_string();

    // Before the revocation every one of them is active.
    for (case, token) in [("the refresh token", &refresh), ("an unrelated token", &unrelated)]
        .into_iter()
        .chain(chained.iter().map(|token| ("a chained access token", token)))
    {
        let answer = introspect(&em, json!({"token": token})).await;
        assert_eq!(
            answer.body["active"],
            json!(true),
            "case {case:?}: must be active before any revocation, got {:?}",
            answer.body
        );
    }

    // Rows: the same revocation repeated. Each answers the same success, which is what
    // `@idempotent` and "revoking an already-revoked [...] token still returns 200 OK
    // with an empty body" require.
    for attempt in 1..=3 {
        let answer = revoke(&em, &refresh).await;
        assert_eq!(
            answer.status, 200,
            "attempt {attempt}: a revocation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "attempt {attempt}: a revocation answers an empty body, got {:?}",
            answer.body
        );

        for (case, token) in [("the revoked refresh token", &refresh)]
            .into_iter()
            .chain(chained.iter().map(|token| ("a token in its chain", token)))
        {
            let claims = introspect(&em, json!({"token": token})).await;
            assert_eq!(
                claims.body,
                json!({"active": false}),
                "attempt {attempt}, case {case:?}: a revoked token is inactive, got {:?}",
                claims.body
            );
        }

        let untouched = introspect(&em, json!({"token": &unrelated})).await;
        assert_eq!(
            untouched.body["active"],
            json!(true),
            "attempt {attempt}: a token of another chain must stay active, got {:?}",
            untouched.body
        );
    }

    // A refresh-token value nothing was ever issued for is the same success: rows differ
    // in the shape of the value the pattern admits.
    for (case, token) in [
        ("an unissued hex value", format!("ASOR{}", "1".repeat(64))),
        ("an unissued value at the length floor", "ASORx".to_string()),
        (
            "an unissued value at the length ceiling",
            format!("ASOR{}", "y".repeat(4092)),
        ),
    ] {
        let answer = revoke(&em, &token).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: revoking an unknown token must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// A token value the model's patterns do not admit is refused at the boundary — 400
/// `ValidationException` — rather than answered: `IntrospectionToken` takes either token
/// family (`^ASO[AR]…`) while `RevocationToken` takes only the refresh one (`^ASOR…`),
/// since "access_tokens are not accepted for revocation".
#[tokio::test]
async fn a_token_outside_the_modelled_pattern_is_refused() {
    let em = emulator();
    let minted = create_token(&em, "aws-mcp.amazonaws.com")
        .await
        .string("access_token")
        .to_string();

    // (case, token, refused by introspect, refused by revoke)
    let cases: Vec<(&str, String, bool, bool)> = vec![
        ("not a sign-in token at all", "nope".to_string(), true, true),
        ("empty", String::new(), true, true),
        (
            "an access key id, not a token",
            "ASIAEXAMPLE1234567890".to_string(),
            true,
            true,
        ),
        ("the access prefix alone", "ASOA".to_string(), true, true),
        ("the refresh prefix alone", "ASOR".to_string(), true, true),
        (
            "a third token family the pattern does not admit",
            format!("ASOB{}", "a".repeat(64)),
            true,
            true,
        ),
        (
            "a character outside the alphabet",
            format!("ASOR{}!", "a".repeat(63)),
            true,
            true,
        ),
        (
            "one character past the 4096 ceiling",
            format!("ASOR{}", "a".repeat(4093)),
            true,
            true,
        ),
        // An access token is a valid `IntrospectionToken` and not a valid
        // `RevocationToken`: introspection serves it, revocation refuses it.
        ("a minted access token", minted, false, true),
    ];

    for (case, token, introspect_refused, revoke_refused) in &cases {
        let answer = introspect(&em, json!({"token": token})).await;
        let expected = if *introspect_refused { 400 } else { 200 };
        assert_eq!(
            answer.status, expected,
            "case {case:?}: introspection must answer HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        if *introspect_refused {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException from introspection, got {:?}",
                answer.body
            );
        }

        let answer = revoke(&em, token).await;
        let expected = if *revoke_refused { 400 } else { 200 };
        assert_eq!(
            answer.status, expected,
            "case {case:?}: revocation must answer HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        if *revoke_refused {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException from revocation, got {:?}",
                answer.body
            );
        }
    }
}

/// The token store is bounded: once it holds [`OAUTH_TOKEN_QUOTA`] live tokens a mint is
/// refused with the modelled `TooManyRequestsError`, and a token whose lifetime has run
/// out is reclaimed first, so expired records never consume the bound.
#[tokio::test]
async fn the_token_store_is_bounded_and_reclaims_expired_tokens() {
    use service_com_amazonaws_signin_signin::state::OAUTH_TOKEN_QUOTA;

    let em = emulator();
    let expired_at = SystemTime::now() - Duration::from_secs(7200);
    // Fill the store with tokens that have ALREADY expired. They are reclaimed by the
    // next mint, so the bound is not reached.
    for _ in 0..OAUTH_TOKEN_QUOTA {
        em.seed(
            new_token(TOKEN_TYPE_ACCESS, "aws-mcp.amazonaws.com", 3600),
            expired_at,
        );
    }
    let served = create_token(&em, "aws-mcp.amazonaws.com").await;
    assert_eq!(
        served.status, 200,
        "expired tokens must be reclaimed rather than consume the bound, got {} with {:?}",
        served.status, served.body
    );

    // Now fill it with LIVE tokens, which do consume it.
    for index in 1..OAUTH_TOKEN_QUOTA {
        let answer = create_token(&em, "aws-mcp.amazonaws.com").await;
        assert_eq!(
            answer.status, 200,
            "mint {index} inside the bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
    }
    let refused = create_token(&em, "aws-mcp.amazonaws.com").await;
    assert_eq!(
        refused.status, 429,
        "a mint past the bound must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyRequestsError",
        "expected TooManyRequestsError, got {:?}",
        refused.body
    );
}
