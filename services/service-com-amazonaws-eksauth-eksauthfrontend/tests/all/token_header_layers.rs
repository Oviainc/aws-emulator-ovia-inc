// SPDX-License-Identifier: CC0-1.0

//! The ORDERED layers `AssumeRoleForPodIdentity` reads a presented token's JOSE header
//! in, and the one message each layer answers with.
//!
//! Every rejection here is HTTP 400 `InvalidTokenException` — the status and the shape are
//! the same for all of them — so what a caller can act on is the MESSAGE, and the message
//! names the SHALLOWEST layer the header fails at:
//!
//! 1. the header segment must read as a JOSE header at all: a JSON object whose `alg` is
//!    there and is a string, and whose `kid`, if it carries one, is a string or `null`.
//!    Anything else did not parse, exactly like a segment that is not base64url or not a
//!    JSON object -> "Failed to parse the token included in the request.";
//! 2. `alg` must name the algorithm a cluster's Kubernetes API server signs projected
//!    service account tokens with -> "... has invalid header - alg.";
//! 3. `kid` must name something — absent, `null`, empty and whitespace-only all name
//!    nothing -> "... has missing header - kid.";
//! 4. the `kid` must resolve to a key the named cluster's OIDC provider publishes; a
//!    value outside the form every key id takes, one no cluster publishes, and one
//!    published by a DIFFERENT cluster all resolve to nothing -> "... has invalid header
//!    - kid.".
//!
//! A header wrong at several layers is reported only on the shallowest, so the four
//! layers stay four distinct diagnoses: that is what lets an operator tell "the projected
//! token is not a JWT" from "the cluster's signing key rotated".
//!
//! The rows span the class each layer generalises over (the JSON type and the value of
//! each member, the ways a value can name nothing, the lengths and alphabets a key id can
//! take, and which cluster publishes it) and are driven over the real request path
//! through the harness `tests/all/assume_role_for_pod_identity.rs` owns.

use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    encode_jwt_segment, TOKEN_SIGNING_ALGORITHM,
};

use crate::assume_role_for_pod_identity::{Harness, SEEDS};

/// The seeded cluster whose OIDC provider publishes a key, and a second seeded cluster
/// whose key is a key id the first one does not publish.
const CLUSTER: &str = SEEDS[0].cluster;
const OTHER_CLUSTER: &str = SEEDS[3].cluster;

/// A cluster name inside the modelled `ClusterName` pattern and length that is NOT
/// registered, so its OIDC provider publishes nothing at all.
const UNREGISTERED_CLUSTER: &str = "sim2realbehavabsentcluster";

/// Which layer of the header read a row must be reported at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// 1: the header segment does not read as a JOSE header.
    Parse,
    /// 2: `alg` names an algorithm no cluster signs with.
    Algorithm,
    /// 3: `kid` names nothing.
    MissingKeyId,
    /// 4: `kid` names no key the cluster's OIDC provider publishes.
    UnresolvableKeyId,
}

impl Layer {
    /// The message this layer answers with. Fixed text throughout; the only variable
    /// part is the name of the header member the caller must change.
    fn message(self) -> &'static str {
        match self {
            Layer::Parse => "Failed to parse the token included in the request.",
            Layer::Algorithm => "The token included in the request has invalid header - alg.",
            Layer::MissingKeyId => "The token included in the request has missing header - kid.",
            Layer::UnresolvableKeyId => {
                "The token included in the request has invalid header - kid."
            }
        }
    }
}

/// Every layer, shallowest first.
const LAYERS: &[Layer] = &[
    Layer::Parse,
    Layer::Algorithm,
    Layer::MissingKeyId,
    Layer::UnresolvableKeyId,
];

/// A key id in the form a Kubernetes API server derives for every key it signs with: the
/// unpadded base64url of a 32-byte digest, which is 43 characters. `seed` makes two of
/// them different values, and no cluster in the harness publishes either.
fn key_id(seed: u8) -> String {
    encode_jwt_segment([seed; 32])
}

/// A token whose JOSE header is `header`, with a well-formed claim set and signature, so
/// the only thing a rejection can be about is the header.
fn token_with_header(header: &serde_json::Value) -> String {
    format!(
        "{}.{}.{}",
        encode_jwt_segment(header.to_string()),
        encode_jwt_segment(
            r#"{"aud":["pods.eks.amazonaws.com"],"sub":"system:serviceaccount:default:my-sa"}"#
        ),
        encode_jwt_segment([7_u8; 32]),
    )
}

/// What one call answered: the status, the shape `x-amzn-errortype` names, the `__type`
/// the body names, the message, and the raw body.
#[derive(Debug)]
struct Reply {
    status: u16,
    errortype: Option<String>,
    type_name: Option<String>,
    message: String,
    body: String,
}

impl Reply {
    /// `<status> <shape>` from the errortype header, for comparing a whole verdict in one
    /// assertion. A real service appends a coral suffix to the header, which names the
    /// same shape, so the shape name is what is compared.
    fn verdict(&self) -> String {
        let shape = self.errortype.as_deref().unwrap_or("<no errortype header>");
        format!("{} {}", self.status, shape.split(':').next().unwrap_or(shape))
    }
}

/// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`.
async fn assume(harness: &Harness, cluster: &str, token: &str) -> Reply {
    let (status, errortype, body) = harness
        .send(cluster, serde_json::json!({ "token": token }))
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    Reply {
        status,
        errortype,
        type_name: parsed["__type"].as_str().map(str::to_string),
        message: parsed["message"].as_str().unwrap_or_default().to_string(),
        body,
    }
}

/// Assert one header is answered at `layer`, against `cluster`.
async fn assert_layer(
    harness: &Harness,
    cluster: &str,
    case: &str,
    token: &str,
    layer: Layer,
) {
    let reply = assume(harness, cluster, token).await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidTokenException",
        "case {case:?} against cluster {cluster:?}: expected the modeled token refusal, \
         got {reply:?}"
    );
    assert_eq!(
        reply.type_name.as_deref(),
        Some("InvalidTokenException"),
        "case {case:?} against cluster {cluster:?}: the body must name the same shape as \
         the header, got {reply:?}"
    );
    assert_eq!(
        reply.message,
        layer.message(),
        "case {case:?} against cluster {cluster:?}: expected the {layer:?} layer's \
         message, got {:?}",
        reply.message
    );
    // `token` is @sensitive: no part of the presented value may come back.
    assert!(
        !reply.body.contains(token),
        "case {case:?} against cluster {cluster:?}: the token value reached the response \
         {}",
        reply.body
    );
}

/// Each layer's class of defective header, over the registered cluster and over one that
/// is not registered: the layer a header is reported at is decided the same way for both,
/// because every row names a key neither cluster publishes.
#[tokio::test]
async fn every_header_defect_is_reported_at_its_own_layer() {
    let harness = Harness::seeded();
    let foreign_key_id = harness.signing_key_id(OTHER_CLUSTER);

    // (case, header, the layer it must be reported at)
    let rows: &[(&str, serde_json::Value, Layer)] = &[
        // LAYER 1: the header object is not a JOSE header. An absent `alg` is the
        // recorded case from the issue, with a blank `kid` beneath it that is never
        // reached.
        (
            "no alg and a blank kid",
            serde_json::json!({ "kid": "" }),
            Layer::Parse,
        ),
        (
            "no alg at all",
            serde_json::json!({ "typ": "JWT" }),
            Layer::Parse,
        ),
        (
            "no alg beside a well-formed kid",
            serde_json::json!({ "kid": key_id(1), "typ": "JWT" }),
            Layer::Parse,
        ),
        (
            "alg holding a number",
            serde_json::json!({ "alg": 256, "kid": key_id(1) }),
            Layer::Parse,
        ),
        (
            "alg holding null",
            serde_json::json!({ "alg": null, "kid": key_id(1) }),
            Layer::Parse,
        ),
        (
            "alg holding an array",
            serde_json::json!({ "alg": ["RS256"], "kid": key_id(1) }),
            Layer::Parse,
        ),
        (
            "alg holding an object",
            serde_json::json!({ "alg": { "name": "RS256" }, "kid": key_id(1) }),
            Layer::Parse,
        ),
        (
            "kid holding a number",
            serde_json::json!({ "alg": "RS256", "kid": 42 }),
            Layer::Parse,
        ),
        (
            "kid holding an object",
            serde_json::json!({ "alg": "RS256", "kid": { "id": key_id(1) } }),
            Layer::Parse,
        ),
        (
            "kid holding a boolean",
            serde_json::json!({ "alg": "RS256", "kid": true }),
            Layer::Parse,
        ),
        // LAYER 2: `alg` reads, and names an algorithm no cluster signs with. Each row
        // is also wrong deeper down, which is never what is reported.
        (
            "another algorithm over a blank kid",
            serde_json::json!({ "alg": "HS256", "kid": "" }),
            Layer::Algorithm,
        ),
        (
            "no algorithm at all, named",
            serde_json::json!({ "alg": "none", "kid": key_id(1) }),
            Layer::Algorithm,
        ),
        (
            // The value is matched exactly, so a different case is a different value.
            "the algorithm in lower case",
            serde_json::json!({ "alg": "rs256", "kid": key_id(1) }),
            Layer::Algorithm,
        ),
        (
            "the algorithm with surrounding space",
            serde_json::json!({ "alg": " RS256 ", "kid": key_id(1) }),
            Layer::Algorithm,
        ),
        (
            "a longer algorithm of the same family",
            serde_json::json!({ "alg": "RS512", "kid": "my-signing-key" }),
            Layer::Algorithm,
        ),
        (
            "an empty algorithm",
            serde_json::json!({ "alg": "", "kid": key_id(1) }),
            Layer::Algorithm,
        ),
        (
            "another algorithm beside no kid at all",
            serde_json::json!({ "alg": "ES256", "typ": "JWT" }),
            Layer::Algorithm,
        ),
        // LAYER 3: the algorithm is right and the header names no key. Absent, null,
        // empty and whitespace-only all name nothing.
        (
            "no kid member",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "typ": "JWT" }),
            Layer::MissingKeyId,
        ),
        (
            "an empty kid",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "" }),
            Layer::MissingKeyId,
        ),
        (
            "a kid of spaces",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "   " }),
            Layer::MissingKeyId,
        ),
        (
            "a kid of other whitespace",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "\t\n " }),
            Layer::MissingKeyId,
        ),
        (
            "a kid holding null",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": null }),
            Layer::MissingKeyId,
        ),
        // LAYER 4: the header reads and names a key, and the key resolves to nothing.
        (
            "a key id of the right form that nothing published",
            serde_json::json!({
                "alg": TOKEN_SIGNING_ALGORITHM,
                "kid": key_id(2),
                "typ": "JWT",
            }),
            Layer::UnresolvableKeyId,
        ),
        (
            "a plain name rather than a key id",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "my-signing-key" }),
            Layer::UnresolvableKeyId,
        ),
        (
            "a key id one character short",
            serde_json::json!({
                "alg": TOKEN_SIGNING_ALGORITHM,
                "kid": key_id(3)[1..].to_string(),
            }),
            Layer::UnresolvableKeyId,
        ),
        (
            "a key id one character long",
            serde_json::json!({
                "alg": TOKEN_SIGNING_ALGORITHM,
                "kid": format!("{}A", key_id(3)),
            }),
            Layer::UnresolvableKeyId,
        ),
        (
            "a key id of the right length outside the base64url alphabet",
            serde_json::json!({
                "alg": TOKEN_SIGNING_ALGORITHM,
                "kid": format!("{}.", &key_id(3)[1..]),
            }),
            Layer::UnresolvableKeyId,
        ),
        (
            // A key id another cluster really publishes: resolution is against the
            // provider of the cluster the request NAMES, so this one resolves nowhere
            // here.
            "a key id another cluster publishes",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": foreign_key_id }),
            Layer::UnresolvableKeyId,
        ),
    ];

    for cluster in [CLUSTER, UNREGISTERED_CLUSTER, "a"] {
        for (case, header, layer) in rows {
            let token = token_with_header(header);
            assert_layer(&harness, cluster, case, &token, *layer).await;
        }
    }
}

/// The parse layer is ONE answer for every way a value fails to read as a token, whatever
/// part of it failed: the caller's fix is the same, so a header that is not base64url, one
/// that is not a JSON object and one that is a JSON object but not a JOSE header are all
/// the same message. Only the HEADER segment can put a value in this class — the segments
/// past it are not read (see `token_claims_are_never_read.rs`).
#[tokio::test]
async fn the_whole_parse_class_is_one_message() {
    let harness = Harness::seeded();
    let claims = encode_jwt_segment(r#"{"sub":"system:serviceaccount:default:my-sa"}"#);
    let signature = encode_jwt_segment([0_u8; 32]);

    let rows: &[(&str, String)] = &[
        (
            "a header that is not base64url",
            format!("A.{claims}.{signature}"),
        ),
        (
            "a header that is not JSON",
            format!(
                "{}.{claims}.{signature}",
                encode_jwt_segment("not json at all")
            ),
        ),
        (
            "a header that is a JSON array",
            format!(
                "{}.{claims}.{signature}",
                encode_jwt_segment(r#"["alg","kid"]"#)
            ),
        ),
        (
            // The header segment is base64url of a JSON object, but the object is not a
            // JOSE header, so the value did not read either.
            "a header that is a JSON object but not a JOSE header",
            token_with_header(&serde_json::json!({ "typ": "JWT" })),
        ),
    ];

    for (case, token) in rows {
        assert_layer(&harness, UNREGISTERED_CLUSTER, case, token, Layer::Parse).await;
    }
}

/// The four layers are four DISTINCT diagnoses, and fixing one layer moves the answer
/// exactly one layer deeper: the shallowest failing layer is the one reported.
#[tokio::test]
async fn fixing_one_layer_moves_the_answer_to_the_next() {
    let harness = Harness::seeded();

    // One chain of headers, each repairing exactly the layer the previous one failed at
    // and leaving the deeper defects in place.
    let chain: &[(&str, serde_json::Value)] = &[
        ("no alg, blank kid", serde_json::json!({ "kid": "" })),
        (
            "alg present but another algorithm, blank kid",
            serde_json::json!({ "alg": "HS256", "kid": "" }),
        ),
        (
            "the right algorithm, blank kid",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "" }),
        ),
        (
            "the right algorithm, a key id nothing published",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": key_id(5) }),
        ),
    ];
    assert_eq!(
        chain.len(),
        LAYERS.len(),
        "the chain must repair every layer in turn, got {} steps for {} layers",
        chain.len(),
        LAYERS.len()
    );

    let mut answered = Vec::new();
    for (step, ((case, header), layer)) in chain.iter().zip(LAYERS).enumerate() {
        let token = token_with_header(header);
        let reply = assume(&harness, UNREGISTERED_CLUSTER, &token).await;
        assert_eq!(
            reply.message,
            layer.message(),
            "step {step} {case:?}: expected the {layer:?} layer's message, got {reply:?}"
        );
        answered.push(reply.message);
    }

    // Four layers, four messages a client can tell apart.
    let mut distinct = answered.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "the layers must answer distinct messages, got {answered:?}"
    );
}

/// The deepest layer is answered from STATE: the key id the named cluster's OIDC provider
/// publishes is the one that resolves, so a token naming it is past the header layers and
/// is judged on the cluster's record of what it issued, while the same token against any
/// other cluster name resolves to no key.
#[tokio::test]
async fn the_key_id_resolves_only_against_the_cluster_that_publishes_it() {
    let harness = Harness::seeded();

    for cluster in [CLUSTER, OTHER_CLUSTER] {
        let published = harness.signing_key_id(cluster);
        let token = token_with_header(&serde_json::json!({
            "alg": TOKEN_SIGNING_ALGORITHM,
            "kid": published,
            "typ": "JWT",
        }));

        // Against the cluster that publishes it the header is past: nothing issued this
        // token, so the refusal comes from the record rather than from a header.
        let reply = assume(&harness, cluster, &token).await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidTokenException",
            "case {cluster:?}: a token nothing issued is invalid, got {reply:?}"
        );
        for layer in LAYERS {
            assert_ne!(
                reply.message,
                layer.message(),
                "case {cluster:?}: a header that reads and resolves must not be refused \
                 at the {layer:?} layer, got {reply:?}"
            );
        }

        // Against every other cluster name the same key id resolves to nothing.
        let elsewhere = if cluster == CLUSTER {
            OTHER_CLUSTER
        } else {
            CLUSTER
        };
        for other in [elsewhere, UNREGISTERED_CLUSTER] {
            assert_layer(
                &harness,
                other,
                &format!("the key id of {cluster} presented to {other}"),
                &token,
                Layer::UnresolvableKeyId,
            )
            .await;
        }
    }

    // And the token that cluster really issued is exchanged, so the resolution layer
    // refuses nothing it should admit.
    let (status, body) = harness.assume(CLUSTER, &harness.token(&SEEDS[0])).await;
    assert_eq!(
        status, 200,
        "the issued token must still be exchanged, got {status} {body}"
    );
}
