// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackageV2 origin-endpoint POLICY sub-resource
//! (`PutOriginEndpointPolicy` / `GetOriginEndpointPolicy` /
//! `DeleteOriginEndpointPolicy`), driven over HTTP through the same router the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A put persists the policy document and the CDN authorization settings submitted
//!    with it on the addressed endpoint, so the matching get reports them byte for byte;
//!    a later put REPLACES that record wholesale (an omitted `CdnAuthConfiguration`
//!    leaves none behind).
//! 2. A delete leaves the addressed endpoint carrying NO policy — the get then answers
//!    `ResourceNotFoundException` (404) — and, being `@idempotent` with no 404 modeled,
//!    succeeds whether a policy was attached, was never attached, or the addressed
//!    endpoint, channel or group does not resolve at all.
//! 3. A delete touches only the endpoint it addresses: a sibling endpoint's policy, and
//!    the endpoint resource itself, survive it.
//! 4. A put against an endpoint that does not resolve is `ResourceNotFoundException`
//!    (404) and stores nothing; a policy that is not a JSON object is
//!    `ValidationException` (400) with `ValidationExceptionType` `INVALID_POLICY`; a
//!    policy longer than the modeled `PolicyText` maximum is refused, and one of exactly
//!    that length is accepted.
//! 5. A not-found names the resource type that is MISSING in `ResourceTypeNotFound`,
//!    whose enum is CHANNEL_GROUP | CHANNEL | ORIGIN_ENDPOINT | HARVEST_JOB — one value
//!    per level of the address — and leaves the member unset when the endpoint is present
//!    and only its policy is absent, since no enum value names a policy.
//! 6. The CDN authorization a policy carries is bounded by the MODEL —
//!    `CdnIdentifierSecretArns` is `@length 1..100` — so a list at the bound is stored and
//!    read back whole, while an empty one and one past the bound are refused (400,
//!    naming the member) with nothing stored.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. It travels in the SigV4
/// credential scope, which is also what the service reads its region from.
const REGION: &str = "us-west-2";

/// The maximum length of a policy document, from the model's `PolicyText` shape
/// (`smithy_query(query="shape", shape="PolicyText")` -> `length {max: 6144}`).
const POLICY_TEXT_MAX: usize = 6144;

/// The account every resource this emulator serves belongs to, which is therefore the
/// account a CDN authorization secret has to live in for MediaPackage to read it.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// MediaPackage's FIXED not-found sentences. The service words a not-found by the
/// resource type that is missing and puts none of the caller's identifiers in it, so
/// these are constants a test can compare against exactly.
const CHANNEL_GROUP_NOT_FOUND: &str = "MediaPackage can't process your request because \
    we can't find your channel group. Verify your channel group name or add a channel \
    group and then try again";
const CHANNEL_NOT_FOUND: &str = "MediaPackage can't process your request because we \
    can't find your channel. Verify your channel name or add a channel and then try \
    again";
const ORIGIN_ENDPOINT_NOT_FOUND: &str = "MediaPackage can't process your request because \
    we can't find your origin endpoint. Verify your origin endpoint name or add an origin \
    endpoint and then try again";
const POLICY_NOT_FOUND: &str = "MediaPackage can't process your request because we can't \
    find your policy. Attach a policy to your resource and then try again";

/// A MediaPackageV2 emulator plus a tiny restJson1 client for the policy sub-resource.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing: it exercises handler behaviour, not authentication.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("mediapackagev2.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackagev2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON: {e}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Create the group -> channel -> endpoint chain the policy hangs off.
    async fn create_endpoint(&self, group: &str, channel: &str, endpoint: &str) {
        let (status, body) = self
            .call("POST", "/channelGroup", Some(json!({"ChannelGroupName": group})))
            .await;
        assert!(
            status == 200 || status == 409,
            "setup: creating group {group} answered {status} {body}"
        );
        let (status, body) = self
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel"),
                Some(json!({"ChannelName": channel})),
            )
            .await;
        assert!(
            status == 200 || status == 409,
            "setup: creating channel {channel} answered {status} {body}"
        );
        let (status, body) = self
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel/{channel}/originEndpoint"),
                Some(json!({
                    "OriginEndpointName": endpoint,
                    "ContainerType": "TS",
                    // An origin endpoint needs at least one manifest, which the service
                    // enforces with `ValidationExceptionType` NUM_MANIFESTS_LOW.
                    "HlsManifests": [{"ManifestName": "index"}],
                })),
            )
            .await;
        assert!(
            status == 200 || status == 409,
            "setup: creating endpoint {endpoint} answered {status} {body}"
        );
    }

    fn policy_path(group: &str, channel: &str, endpoint: &str) -> String {
        format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/policy")
    }

    async fn put_policy(
        &self,
        group: &str,
        channel: &str,
        endpoint: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call("POST", &Self::policy_path(group, channel, endpoint), Some(body))
            .await
    }

    async fn get_policy(&self, group: &str, channel: &str, endpoint: &str) -> (u16, Value) {
        self.call("GET", &Self::policy_path(group, channel, endpoint), None)
            .await
    }

    async fn delete_policy(&self, group: &str, channel: &str, endpoint: &str) -> (u16, Value) {
        self.call("DELETE", &Self::policy_path(group, channel, endpoint), None)
            .await
    }

    async fn get_endpoint(&self, group: &str, channel: &str, endpoint: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}"),
            None,
        )
        .await
    }
}

/// The `__type` discriminator of an error envelope.
fn error_type(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The `Message` of an error envelope.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A policy document of EXACTLY `len` characters that is still a valid policy document
/// (a JSON object carrying a `Statement`), so length is the only thing that varies
/// between the bound cases.
fn policy_of_length(len: usize) -> String {
    let prefix = r#"{"Statement":[{"Effect":"Allow","Sid":""#;
    let suffix = r#""}]}"#;
    let pad = len
        .checked_sub(prefix.len() + suffix.len())
        .expect("a length that fits the JSON envelope");
    let policy = format!("{prefix}{}{suffix}", "A".repeat(pad));
    assert_eq!(policy.len(), len, "the padded policy must have length {len}");
    policy
}

/// One row of the put-and-read-back table: a policy document and the CDN authorization
/// settings submitted with it, on its own group/channel/endpoint chain. The rows differ
/// in name shape, document shape and size, and in how many CDN secret ARNs travel with
/// the policy (none at all, through a configuration with two).
struct PolicyCase {
    label: &'static str,
    group: &'static str,
    channel: &'static str,
    endpoint: &'static str,
    /// The `Policy` string exactly as it is submitted and must be read back.
    policy: fn() -> String,
    /// The `CdnAuthConfiguration` member, or `None` when the put omits it.
    cdn_auth: Option<fn() -> Value>,
}

fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            label: "minimal document: an empty statement list, no CDN auth",
            group: "g1",
            channel: "c1",
            endpoint: "e1",
            policy: || r#"{"Statement":[]}"#.to_string(),
            cdn_auth: None,
        },
        PolicyCase {
            label: "one statement, no CDN auth",
            group: "policy-group",
            channel: "policy-channel",
            endpoint: "policy-ep",
            policy: || {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Sid": "AllowAnyone",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "Resource": "*",
                    }],
                })
                .to_string()
            },
            cdn_auth: None,
        },
        PolicyCase {
            label: "two statements, CDN auth with one secret ARN",
            group: "A_group_with_9_and-dashes",
            channel: "A_channel_0",
            endpoint: "An_endpoint_0",
            policy: || {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {"Effect": "Allow", "Principal": {"AWS": "123456789012"},
                         "Action": "mediapackagev2:GetObject", "Resource": "*"},
                        {"Effect": "Deny", "Principal": "*",
                         "Action": "mediapackagev2:GetHeadObject", "Resource": "*"},
                    ],
                })
                .to_string()
            },
            cdn_auth: Some(|| {
                json!({
                    "CdnIdentifierSecretArns": [
                        format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:cdn-one"),
                    ],
                    "SecretsRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaPackageCdnAuth"),
                })
            }),
        },
        PolicyCase {
            label: "a document at the modeled PolicyText maximum, CDN auth with two secret ARNs",
            group: "g-max",
            channel: "c-max",
            endpoint: "e-max",
            policy: || policy_of_length(POLICY_TEXT_MAX),
            cdn_auth: Some(|| {
                json!({
                    "CdnIdentifierSecretArns": [
                        format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:cdn-one"),
                        format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:cdn-two"),
                    ],
                    "SecretsRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaPackageCdnAuth"),
                })
            }),
        },
        PolicyCase {
            label: "a document carrying non-ASCII text, no CDN auth",
            group: "g-unicode",
            channel: "c-unicode",
            endpoint: "e-unicode",
            policy: || {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{"Sid": "dirección-ünicode-策略", "Effect": "Deny",
                                   "Principal": "*", "Action": "mediapackagev2:GetObject",
                                   "Resource": "*"}],
                })
                .to_string()
            },
            cdn_auth: None,
        },
    ]
}

/// The put body for a row.
fn put_body(case: &PolicyCase) -> Value {
    let mut body = json!({"Policy": (case.policy)()});
    if let Some(cdn_auth) = case.cdn_auth {
        body["CdnAuthConfiguration"] = cdn_auth();
    }
    body
}

/// Rule 1: a put persists the document and its CDN settings, and the get reports them.
#[tokio::test]
async fn a_put_policy_is_reported_unchanged_by_the_matching_get() {
    let emulator = Emulator::new();
    for case in policy_cases() {
        let PolicyCase {
            label,
            group,
            channel,
            endpoint,
            ..
        } = case;
        emulator.create_endpoint(group, channel, endpoint).await;

        let (status, body) = emulator
            .put_policy(group, channel, endpoint, put_body(&case))
            .await;
        assert_eq!(status, 200, "case {label:?}: put expected 200, got {status} {body}");
        assert_eq!(
            body,
            json!({}),
            "case {label:?}: put answers an empty structure, got {body}"
        );

        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(status, 200, "case {label:?}: get expected 200, got {status} {body}");
        assert_eq!(
            body.get("Policy").and_then(Value::as_str),
            Some((case.policy)().as_str()),
            "case {label:?}: the get must report the submitted document, got {body}"
        );
        assert_eq!(
            body.get("ChannelGroupName").and_then(Value::as_str),
            Some(group),
            "case {label:?}: the get must echo the addressed group, got {body}"
        );
        assert_eq!(
            body.get("ChannelName").and_then(Value::as_str),
            Some(channel),
            "case {label:?}: the get must echo the addressed channel, got {body}"
        );
        assert_eq!(
            body.get("OriginEndpointName").and_then(Value::as_str),
            Some(endpoint),
            "case {label:?}: the get must echo the addressed endpoint, got {body}"
        );
        assert_eq!(
            body.get("CdnAuthConfiguration"),
            case.cdn_auth.map(|cdn_auth| cdn_auth()).as_ref(),
            "case {label:?}: the CDN settings must round-trip exactly as submitted, got {body}"
        );
    }
}

/// Rule 1, second half: a put REPLACES the stored record rather than merging into it.
#[tokio::test]
async fn a_second_put_replaces_the_whole_policy_record() {
    let emulator = Emulator::new();
    emulator.create_endpoint("grp", "chn", "ep").await;

    let cases = policy_cases();
    let mut previous: Option<&PolicyCase> = None;
    for case in &cases {
        let label = case.label;
        let (status, body) = emulator.put_policy("grp", "chn", "ep", put_body(case)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: replacing put expected 200, got {status} {body}"
        );

        let (status, body) = emulator.get_policy("grp", "chn", "ep").await;
        assert_eq!(status, 200, "case {label:?}: get expected 200, got {status} {body}");
        assert_eq!(
            body.get("Policy").and_then(Value::as_str),
            Some((case.policy)().as_str()),
            "case {label:?}: only the newest document may be reported, got {body}"
        );
        assert_eq!(
            body.get("CdnAuthConfiguration"),
            case.cdn_auth.map(|cdn_auth| cdn_auth()).as_ref(),
            "case {label:?}: the newest put decides the CDN settings — a put that omits \
             them must leave none behind (previous row: {:?}), got {body}",
            previous.map(|p| p.label)
        );
        previous = Some(case);
    }
}

/// Rule 2: a delete leaves the endpoint carrying no policy, and says so again on every
/// repeat — including for addresses that never had a policy or do not resolve.
#[tokio::test]
async fn a_delete_detaches_the_policy_and_repeats_succeed() {
    let emulator = Emulator::new();
    for case in policy_cases() {
        let PolicyCase {
            label,
            group,
            channel,
            endpoint,
            ..
        } = case;
        emulator.create_endpoint(group, channel, endpoint).await;
        assert_eq!(
            emulator
                .put_policy(group, channel, endpoint, put_body(&case))
                .await
                .0,
            200,
            "case {label:?}: the policy must be attached before it is deleted"
        );

        let (status, body) = emulator.delete_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: delete expected 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {label:?}: delete answers an empty structure, got {body}"
        );

        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the deleted policy must be gone, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            POLICY_NOT_FOUND,
            "case {label:?}: the endpoint is still there, so the missing resource is the \
             policy, worded with the service's fixed sentence; got {body}"
        );
        // Idempotent: the endpoint already carries no policy, which is exactly what the
        // operation promises, so the repeat is a success and not a 404.
        let (status, body) = emulator.delete_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a repeat delete expected 200, got {status} {body}"
        );

        // The endpoint itself is untouched by its policy's deletion.
        let (status, body) = emulator.get_endpoint(group, channel, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the endpoint must survive its policy's deletion, got {status} {body}"
        );
    }
}

/// Rule 2, the addresses that resolve to nothing: `DeleteOriginEndpointPolicy` models no
/// `ResourceNotFoundException`, so every one of them is a success.
#[tokio::test]
async fn a_delete_of_a_policy_that_does_not_exist_succeeds() {
    let emulator = Emulator::new();
    emulator.create_endpoint("grp", "chn", "ep").await;
    emulator.create_endpoint("grp", "chn", "sibling").await;

    // Each row addresses something that holds no policy, differing in WHICH part of the
    // address is absent: the policy, the endpoint, the channel, the group.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("an endpoint that never had a policy", "grp", "chn", "ep"),
        ("a sibling endpoint with no policy", "grp", "chn", "sibling"),
        ("an endpoint that does not exist", "grp", "chn", "absent-ep"),
        ("a channel that does not exist", "grp", "absent-chn", "ep"),
        ("a group that does not exist", "absent-grp", "chn", "ep"),
    ];
    for (label, group, channel, endpoint) in cases {
        let (status, body) = emulator.delete_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {label:?}: expected an empty structure, got {body}"
        );
        // And the delete invented nothing: there is still no policy to read.
        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the get must still answer 404, got {status} {body}"
        );
    }
}

/// Rule 3: a delete reaches only the endpoint it addresses.
#[tokio::test]
async fn a_delete_leaves_other_endpoints_policies_alone() {
    let emulator = Emulator::new();
    // Three endpoints across two channels in two groups, so a delete has neighbours at
    // every level of the hierarchy to leave alone.
    let endpoints: &[(&str, &str, &str)] = &[
        ("grp", "chn", "first"),
        ("grp", "chn", "second"),
        ("grp", "other-chn", "first"),
        ("other-grp", "chn", "first"),
    ];
    for (group, channel, endpoint) in endpoints {
        emulator.create_endpoint(group, channel, endpoint).await;
        let policy =
            json!({"Statement": [{"Sid": format!("{group}/{channel}/{endpoint}")}]}).to_string();
        assert_eq!(
            emulator
                .put_policy(group, channel, endpoint, json!({"Policy": policy}))
                .await
                .0,
            200,
            "setup: attaching a policy to {group}/{channel}/{endpoint}"
        );
    }

    for (index, (group, channel, endpoint)) in endpoints.iter().enumerate() {
        let label = format!("deleting {group}/{channel}/{endpoint}");
        assert_eq!(
            emulator.delete_policy(group, channel, endpoint).await.0,
            200,
            "case {label:?}: delete expected 200"
        );
        for (other_index, (other_group, other_channel, other_endpoint)) in
            endpoints.iter().enumerate()
        {
            let (status, body) = emulator
                .get_policy(other_group, other_channel, other_endpoint)
                .await;
            // Everything already deleted answers 404; everything else still reports its
            // own document, unchanged.
            let expected_status = if other_index <= index { 404 } else { 200 };
            assert_eq!(
                status, expected_status,
                "case {label:?}: {other_group}/{other_channel}/{other_endpoint} expected \
                 {expected_status}, got {status} {body}"
            );
            if expected_status == 200 {
                let expected = json!({
                    "Statement": [{"Sid": format!(
                        "{other_group}/{other_channel}/{other_endpoint}"
                    )}],
                })
                .to_string();
                assert_eq!(
                    body.get("Policy").and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {label:?}: an untouched policy must be unchanged, got {body}"
                );
            }
        }
    }
}

/// Rule 4: the refusals — an unresolvable endpoint, a document that is not a policy, and
/// the `PolicyText` length bound.
#[tokio::test]
async fn a_put_is_refused_for_an_absent_endpoint_or_an_invalid_document() {
    let emulator = Emulator::new();
    emulator.create_endpoint("grp", "chn", "ep").await;

    // Each row names a different missing link in the address chain, and each is worded
    // with the fixed sentence of the resource type that is missing.
    let absent: &[(&str, &str, &str, &str, &str)] = &[
        (
            "an endpoint that does not exist",
            "grp",
            "chn",
            "absent-ep",
            ORIGIN_ENDPOINT_NOT_FOUND,
        ),
        (
            "a channel that does not exist",
            "grp",
            "absent-chn",
            "ep",
            CHANNEL_NOT_FOUND,
        ),
        (
            "a group that does not exist",
            "absent-grp",
            "chn",
            "ep",
            CHANNEL_GROUP_NOT_FOUND,
        ),
    ];
    for (label, group, channel, endpoint, expected_message) in absent {
        let (status, body) = emulator
            .put_policy(
                group,
                channel,
                endpoint,
                json!({"Policy": r#"{"Statement":[]}"#}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            *expected_message,
            "case {label:?}: expected the fixed sentence of the missing resource, got {body}"
        );
        for identifier in [*group, *channel, *endpoint] {
            assert!(
                !error_message(&body).contains(identifier),
                "case {label:?}: the message must name no caller identifier, but it \
                 contains {identifier:?}: {body}"
            );
        }
        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the refused put must store nothing, got {status} {body}"
        );
    }

    // A policy document is a JSON object carrying a `Statement`
    // (`crate::state::is_policy_document`); these are not.
    let not_a_document: &[(&str, &str)] = &[
        ("not JSON at all", "definitely not json"),
        ("a JSON string", "\"a policy\""),
        ("a JSON array of statements", "[{\"Effect\":\"Allow\"}]"),
        ("a JSON number", "17"),
        ("a JSON null", "null"),
        ("an empty document", ""),
        ("a JSON object with no Statement", "{\"Version\":\"2012-10-17\"}"),
    ];
    for (label, policy) in not_a_document {
        let (status, body) = emulator
            .put_policy("grp", "chn", "ep", json!({"Policy": policy}))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body.get("ValidationExceptionType").and_then(Value::as_str),
            Some("INVALID_POLICY"),
            "case {label:?}: expected ValidationExceptionType INVALID_POLICY, got {body}"
        );
    }
    assert_eq!(
        emulator.get_policy("grp", "chn", "ep").await.0,
        404,
        "a refused put must leave the endpoint with no policy"
    );

    // The published bound (`PolicyText` @length max 6144): at the maximum a put is
    // accepted, one character past it is refused.
    let (status, body) = emulator
        .put_policy(
            "grp",
            "chn",
            "ep",
            json!({"Policy": policy_of_length(POLICY_TEXT_MAX)}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a document of exactly {POLICY_TEXT_MAX} characters\": expected 200, got \
         {status} {body}"
    );
    let (status, body) = emulator
        .put_policy(
            "grp",
            "chn",
            "ep",
            json!({"Policy": policy_of_length(POLICY_TEXT_MAX + 1)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a document one character past the maximum\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case \"a document one character past the maximum\": expected ValidationException, \
         got {body}"
    );
    // The over-long put changed nothing: the accepted document is still what is stored.
    let (status, body) = emulator.get_policy("grp", "chn", "ep").await;
    assert_eq!(status, 200, "the stored policy must survive a refused put, got {status} {body}");
    assert_eq!(
        body.get("Policy").and_then(Value::as_str),
        Some(policy_of_length(POLICY_TEXT_MAX).as_str()),
        "the refused over-long put must not have replaced the stored document, got {body}"
    );
}

/// A get of an endpoint that carries no policy is a 404, whether the endpoint exists or
/// not: there is nothing to report either way.
#[tokio::test]
async fn a_get_with_no_policy_attached_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_endpoint("grp", "chn", "ep").await;

    // Each row is a different missing resource: the endpoint resolves and only the policy
    // is absent (the policy sentence), or the address itself does not resolve (the
    // origin-endpoint sentence). Either way the message carries no caller identifier.
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "an existing endpoint with no policy",
            "grp",
            "chn",
            "ep",
            POLICY_NOT_FOUND,
        ),
        (
            "an endpoint that does not exist",
            "grp",
            "chn",
            "absent-ep",
            ORIGIN_ENDPOINT_NOT_FOUND,
        ),
        (
            "a channel that does not exist",
            "grp",
            "absent-chn",
            "ep",
            CHANNEL_NOT_FOUND,
        ),
        (
            "a group that does not exist",
            "absent-grp",
            "chn",
            "ep",
            CHANNEL_GROUP_NOT_FOUND,
        ),
    ];
    for (label, group, channel, endpoint, expected_message) in cases {
        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            *expected_message,
            "case {label:?}: expected the fixed sentence for the missing resource, got {body}"
        );
        for identifier in [*group, *channel, *endpoint] {
            assert!(
                !error_message(&body).contains(identifier),
                "case {label:?}: the message must name no caller identifier, but it \
                 contains {identifier:?}: {body}"
            );
        }
    }
}

/// Deleting the ENDPOINT takes its policy with it: a recreated endpoint of the same name
/// carries no policy, so the policy cannot outlive the resource it belongs to.
#[tokio::test]
async fn deleting_the_endpoint_discards_its_policy() {
    let emulator = Emulator::new();
    for (label, policy) in [
        ("a minimal document", r#"{"Statement":[]}"#.to_string()),
        ("a maximum-length document", policy_of_length(POLICY_TEXT_MAX)),
    ] {
        emulator.create_endpoint("grp", "chn", "ep").await;
        assert_eq!(
            emulator
                .put_policy("grp", "chn", "ep", json!({"Policy": policy}))
                .await
                .0,
            200,
            "case {label:?}: the policy must be attached first"
        );
        let (status, body) = emulator
            .call("DELETE", "/channelGroup/grp/channel/chn/originEndpoint/ep", None)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the endpoint expected 200, got {status} {body}"
        );

        emulator.create_endpoint("grp", "chn", "ep").await;
        let (status, body) = emulator.get_policy("grp", "chn", "ep").await;
        assert_eq!(
            status, 404,
            "case {label:?}: a recreated endpoint must carry no policy, got {status} {body}"
        );
    }
}

/// RULE 5 — a 404 says WHICH resource is missing in `ResourceTypeNotFound`, one value per
/// level of the address, and says nothing there when only the policy is absent.
#[tokio::test]
async fn a_not_found_names_the_resource_type_that_is_missing() {
    let emulator = Emulator::new();
    emulator.create_endpoint("rt-grp", "rt-chn", "rt-ep").await;

    // (label, group, channel, endpoint, the `ResourceTypeNotFound` the answer must carry).
    // The enum's values come from the model's `ResourceTypeNotFound` shape; the last row
    // is the one absence no value names, so the member must be left out entirely.
    let cases = [
        (
            "unknown channel group",
            "rt-missing",
            "rt-chn",
            "rt-ep",
            Some("CHANNEL_GROUP"),
        ),
        (
            "unknown channel",
            "rt-grp",
            "rt-missing",
            "rt-ep",
            Some("CHANNEL"),
        ),
        (
            "unknown origin endpoint",
            "rt-grp",
            "rt-chn",
            "rt-missing",
            Some("ORIGIN_ENDPOINT"),
        ),
        (
            "an endpoint that carries no policy",
            "rt-grp",
            "rt-chn",
            "rt-ep",
            None,
        ),
    ];

    for (label, group, channel, endpoint, expected) in cases {
        let (status, body) = emulator.get_policy(group, channel, endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the get expected 404, got {status} {body}"
        );
        assert_eq!(
            body.get("ResourceTypeNotFound").and_then(Value::as_str),
            expected,
            "case {label:?}: expected ResourceTypeNotFound {expected:?}, got {body}"
        );

        // The write answers the same way wherever the address does not resolve. The last
        // row is the one address where a put must SUCCEED, so it is not asserted here.
        if expected.is_none() {
            continue;
        }
        let (status, body) = emulator
            .put_policy(
                group,
                channel,
                endpoint,
                json!({"Policy": r#"{"Statement":[]}"#}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: the put expected 404, got {status} {body}"
        );
        assert_eq!(
            body.get("ResourceTypeNotFound").and_then(Value::as_str),
            expected,
            "case {label:?}: the put must name the same missing resource type, got {body}"
        );
    }
}

/// RULE 6 — the CDN authorization a policy carries is bounded by the MODEL:
/// `CdnIdentifierSecretArns` is `@length 1..100`, so at the bound the put is stored and
/// read back whole, and past it the request is refused (400 `ValidationException`, naming
/// the member and its bound) with the stored policy left as it was.
#[tokio::test]
async fn the_cdn_secret_list_a_policy_carries_is_bounded() {
    let emulator = Emulator::new();
    emulator.create_endpoint("sec-grp", "sec-chn", "sec-ep").await;

    let secrets = |count: usize| -> Value {
        let arns: Vec<String> = (0..count)
            .map(|index| {
                format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:cdn-auth-{index}")
            })
            .collect();
        json!({
            "CdnIdentifierSecretArns": arns,
            // `SecretsRoleArn` is modelled `@length 20..2048`, so the role ARN is a real
            // one rather than a placeholder.
            "SecretsRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaPackageCdnAuth"),
        })
    };
    // The bound on `com.amazonaws.mediapackagev2#CdnIdentifierSecretArns` itself
    // (`@length(min: 1, max: 100)`), not a number this emulator chose.
    let max = 100usize;
    let policy = r#"{"Statement":[]}"#;

    // One secret (the list's modelled minimum), a middling count, and the bound: all
    // stored, and reported back in the order they were submitted.
    for count in [1usize, max / 2, max] {
        let cdn_auth = secrets(count);
        let (status, body) = emulator
            .put_policy(
                "sec-grp",
                "sec-chn",
                "sec-ep",
                json!({"Policy": policy, "CdnAuthConfiguration": cdn_auth.clone()}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {count} secrets: a list within the bound expected 200, got {status} {body}"
        );
        let (status, read) = emulator.get_policy("sec-grp", "sec-chn", "sec-ep").await;
        assert_eq!(
            status, 200,
            "case {count} secrets: the get expected 200, got {status} {read}"
        );
        assert_eq!(
            read["CdnAuthConfiguration"], cdn_auth,
            "case {count} secrets: the whole CDN authorization must be stored, got {read}"
        );
    }

    let at_bound = secrets(max);
    // One past the bound, and an empty list — the list's modelled MINIMUM is one entry.
    for count in [max + 1, 0] {
        let (status, body) = emulator
            .put_policy(
                "sec-grp",
                "sec-chn",
                "sec-ep",
                json!({"Policy": policy, "CdnAuthConfiguration": secrets(count)}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {count} secrets: a list outside the modelled bound expected 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {count} secrets: expected ValidationException, got {body}"
        );
        assert!(
            error_message(&body).contains("cdnIdentifierSecretArns"),
            "case {count} secrets: the refusal must name the member it is about, got {body}"
        );

        let (status, read) = emulator.get_policy("sec-grp", "sec-chn", "sec-ep").await;
        assert_eq!(
            status, 200,
            "case {count} secrets: the get expected 200, got {status} {read}"
        );
        assert_eq!(
            read["CdnAuthConfiguration"], at_bound,
            "case {count} secrets: a refused put must store nothing, got {read}"
        );
    }
}
