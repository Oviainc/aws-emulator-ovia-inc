// SPDX-License-Identifier: CC0-1.0

//! Every response this service sends names the request it answered.
//!
//! The rule: a response carries the id of the request it answers in the response header
//! this protocol reports that id in (`x-amzn-RequestId` for `awsJson1_1`) — on a SUCCESS
//! exactly as on a failure — and that id is per RESPONSE, so two calls are told apart by
//! it. It is a property of the response rather than of any modelled shape: no output and
//! no error shape of this model declares it, which is why it is asserted on the header
//! here and not on a body member. An SDK surfaces it as `ResponseMetadata.RequestId`, the
//! join key correlation, retry bookkeeping and audit workflows read.
//!
//! The tables below span the LAYERS that can answer a request, since the rule holds over
//! all of them rather than over one operation: a reporting operation's 200, a mutating
//! operation's 200, a listing's 200, a 200 reporting an empty collection, a response to an
//! UNSIGNED call (this service's credential exchange is called with no credentials at
//! all), a handler-returned modelled error, a constraint refusal raised before any handler
//! ran, a request naming no modelled operation, and an authentication refusal raised
//! before any operation was reached.
//!
//! The harness is this suite's own, because the rule is about response HEADERS and
//! `identity_pool_flow::Emulator` reports a status and a parsed body; it drives the same
//! `MultiServiceRouter` over the same single shared engine.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_pool_flow::ABSENT_POOL_ID;

/// The header AWS reports a response's request id in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The region the harness signs for, and so the region every pool it creates is minted in.
const REGION: &str = "us-east-1";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-identity.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so it names no service and an unsigned call
/// addressed to it is routed by its `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// How a call is credentialed. This service's credential exchange (`GetId`,
/// `GetOpenIdToken`, `GetCredentialsForIdentity`) is modelled anonymous and is called
/// before the client holds any credential, so both forms have to be covered.
///
/// `pub(crate)` so a sibling suite in this test target that asserts on response HEADERS
/// drives the service through this harness rather than a second copy of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caller {
    /// A SigV4-signed call, as the SDK sends every credentialed operation.
    Signed,
    /// An unsigned call, as the SDK sends the credential exchange.
    Anonymous,
}

/// One emulator holding this service, reached exactly as the production router reaches it.
/// `pub(crate)` for the same reason as [`Caller`].
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router with this service mounted, with request authentication off (the default
    /// for a test) or on (how the emulator serves traffic for real).
    pub(crate) fn new(authenticating: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        router.set_request_authentication(authenticating);
        Self { router }
    }

    /// Call `operation` with `body` and answer the WHOLE response, headers included.
    pub(crate) async fn dispatch(
        &self,
        caller: Caller,
        operation: &str,
        body: &Value,
    ) -> http::Response<bytes::Bytes> {
        let host = match caller {
            Caller::Signed => ENDPOINT_HOST,
            Caller::Anonymous => EMULATOR_HOST,
        };
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            );
        if caller == Caller::Signed {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=EMULATORTESTKEY12345/20260101/{REGION}/cognito-identity/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            );
        }
        self.router
            .dispatch(
                request
                    .body(bytes::Bytes::from(
                        serde_json::to_vec(body).expect("the request body serializes"),
                    ))
                    .expect("the request builds"),
            )
            .await
    }

    /// Call a signed operation that must succeed, answering its parsed output.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let response = self.dispatch(Caller::Signed, operation, body).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        assert_eq!(
            status,
            200,
            "operation {operation} must succeed, got {status} with {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "operation {operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }
}

/// The id a response states, which must be there.
fn request_id(response: &http::Response<bytes::Bytes>, case: &str) -> String {
    response
        .headers()
        .get(REQUEST_ID_HEADER)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {REQUEST_ID_HEADER} header, got headers {:?}",
                response.headers()
            )
        })
        .to_str()
        .unwrap_or_else(|error| panic!("case {case:?}: expected an ASCII id, got {error}"))
        .to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let well_formed = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !well_formed {
            return false;
        }
    }
    groups.next().is_none()
}

/// An identity pool's ARN, as AWS documents it for a Cognito Identity pool:
/// `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`.
fn pool_arn(identity_pool_id: &str) -> String {
    format!(
        "arn:aws:cognito-identity:{REGION}:{}:identitypool/{identity_pool_id}",
        engine::emulator_account_id()
    )
}

/// Every response names the request it answers, whatever the layer that answered it and
/// whatever the status — and no two responses name the same request.
#[tokio::test]
async fn every_response_names_the_request_it_answers() {
    let emulator = Emulator::new(false);
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "named-responses",
                "AllowUnauthenticatedIdentities": true,
                // A principal-tag mapping is stored only for a provider the pool declares,
                // and one row below writes one for this provider.
                "SupportedLoginProviders": {"graph.facebook.com": "named-app-id"},
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();
    let identity_id = emulator
        .ok("GetId", &json!({ "IdentityPoolId": pool_id }))
        .await["IdentityId"]
        .as_str()
        .expect("GetId returns an IdentityId")
        .to_string();

    // The rows differ along the LAYER that answers and the status it answers with, which
    // is what the rule generalises over: a report, a mutation, a listing, a report of an
    // empty collection, an unsigned call, a modelled error from a handler, a refusal
    // raised before any handler ran, and a failure no operation produced at all.
    //
    // The status a row states is the one this rule is indifferent to: the id is owed
    // whatever it is. The row for a request naming no modelled operation states `None`,
    // because the status a routing failure carries is the routing layer's own concern,
    // not this one's.
    let cases: Vec<(&str, Caller, &str, Value, Option<u16>)> = vec![
        (
            "a reporting operation's success",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": pool_id }),
            Some(200),
        ),
        (
            "a mutating operation's success",
            Caller::Signed,
            "CreateIdentityPool",
            json!({
                "IdentityPoolName": "a-second-pool",
                "AllowUnauthenticatedIdentities": false,
            }),
            Some(200),
        ),
        (
            "a listing's success",
            Caller::Signed,
            "ListIdentityPools",
            json!({ "MaxResults": 10 }),
            Some(200),
        ),
        (
            "a success reporting an empty collection",
            Caller::Signed,
            "ListTagsForResource",
            json!({ "ResourceArn": pool_arn(&pool_id) }),
            Some(200),
        ),
        (
            "a mutating operation answering an empty body",
            Caller::Signed,
            "SetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "UseDefaults": true,
            }),
            Some(200),
        ),
        (
            "an UNSIGNED call's success: the credential exchange this service models \
             anonymous",
            Caller::Anonymous,
            "GetId",
            json!({ "IdentityPoolId": pool_id }),
            Some(200),
        ),
        (
            "an unsigned call's success on an identity that already exists",
            Caller::Anonymous,
            "GetCredentialsForIdentity",
            json!({ "IdentityId": identity_id }),
            // The pool holds no roles, so this is a modelled refusal rather than a vend:
            // which one it is does not matter here, only that it names its request.
            None,
        ),
        (
            "a handler-returned modelled error",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
            Some(400),
        ),
        (
            "a handler-returned modelled error on an unsigned call",
            Caller::Anonymous,
            "GetId",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
            Some(400),
        ),
        (
            "a constraint refusal raised before any handler ran: a required member absent",
            Caller::Signed,
            "CreateIdentityPool",
            json!({ "IdentityPoolName": "no-unauthenticated-flag" }),
            Some(400),
        ),
        (
            "a constraint refusal raised before any handler ran: a value outside its pattern",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": "not-an-identity-pool-id" }),
            Some(400),
        ),
        (
            "a request naming no modelled operation",
            Caller::Signed,
            "NoSuchOperationInThisModel",
            json!({}),
            None,
        ),
    ];

    let mut seen: HashSet<String> = HashSet::new();
    for (case, caller, operation, body, expected_status) in cases {
        let response = emulator.dispatch(caller, operation, &body).await;
        let status = response.status().as_u16();
        if let Some(expected) = expected_status {
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected}, got {status} with body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
        let id = request_id(&response, case);
        assert!(
            is_request_id(&id),
            "case {case:?}: expected an 8-4-4-4-12 lowercase-hex request id, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: expected a request id of this response alone, got the already \
             reported {id:?}"
        );
    }
}

/// The SAME call, repeated, names each of its responses separately: the id belongs to the
/// response, not to the resource it reports or to the operation that answered.
#[tokio::test]
async fn repeating_one_call_names_each_response_separately() {
    let emulator = Emulator::new(false);
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "repeated-describe",
                "AllowUnauthenticatedIdentities": true,
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();
    let body = json!({ "IdentityPoolId": pool_id });

    let mut seen: HashSet<String> = HashSet::new();
    for attempt in 1..=4 {
        let case = format!("DescribeIdentityPool attempt {attempt}");
        let response = emulator
            .dispatch(Caller::Signed, "DescribeIdentityPool", &body)
            .await;
        let id = request_id(&response, &case);
        assert!(
            is_request_id(&id),
            "case {case:?}: expected an 8-4-4-4-12 lowercase-hex request id, got {id:?}"
        );
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: expected an id of this response alone, got the already reported \
             {id:?}"
        );
    }
}

/// One response states ONE id: the header is not repeated, so a client reading it sees a
/// single value whichever layer wrote it.
#[tokio::test]
async fn one_response_states_one_id() {
    let emulator = Emulator::new(false);
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "single-valued",
                "AllowUnauthenticatedIdentities": true,
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    let cases: Vec<(&str, Caller, &str, Value)> = vec![
        (
            "a success",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": pool_id }),
        ),
        (
            "an unsigned success",
            Caller::Anonymous,
            "GetId",
            json!({ "IdentityPoolId": pool_id }),
        ),
        (
            "a modelled error",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
        ),
        (
            "a refusal",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": "not-an-identity-pool-id" }),
        ),
        (
            "a request naming no modelled operation",
            Caller::Signed,
            "NoSuchOperationInThisModel",
            json!({}),
        ),
    ];

    for (case, caller, operation, body) in cases {
        let response = emulator.dispatch(caller, operation, &body).await;
        let stated: Vec<&str> = response
            .headers()
            .get_all(REQUEST_ID_HEADER)
            .iter()
            .map(|value| value.to_str().expect("an ASCII id"))
            .collect();
        assert_eq!(
            stated.len(),
            1,
            "case {case:?}: expected exactly one {REQUEST_ID_HEADER}, got {stated:?}"
        );
    }
}

/// A refusal raised BEFORE any operation was reached names its request too: with request
/// authentication on, a signed call whose signature does not verify is refused by the
/// router, and the response still carries the header.
#[tokio::test]
async fn an_authentication_refusal_names_the_request_it_answers() {
    let emulator = Emulator::new(true);
    let case = "a signed call whose signature does not verify";
    let response = emulator
        .dispatch(
            Caller::Signed,
            "DescribeIdentityPool",
            &json!({ "IdentityPoolId": ABSENT_POOL_ID }),
        )
        .await;
    let status = response.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "case {case:?}: expected a 4xx refusal, got {status} with body {:?}",
        String::from_utf8_lossy(response.body())
    );
    let id = request_id(&response, case);
    assert!(
        is_request_id(&id),
        "case {case:?}: expected an 8-4-4-4-12 lowercase-hex request id, got {id:?}"
    );
}
