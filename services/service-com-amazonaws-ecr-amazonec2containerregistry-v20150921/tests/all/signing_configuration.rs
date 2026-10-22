// SPDX-License-Identifier: CC0-1.0

//! The registry's signing configuration, end to end over the wire.
//!
//! The rule under test: `GetSigningConfiguration` reports the signing configuration the
//! registry currently holds — the one the last accepted `PutSigningConfiguration` stored,
//! member for member — beside the registry's own account id, and refuses with
//! `SigningConfigurationNotFoundException` when the registry holds none. A refused put
//! stores nothing, and a get changes nothing.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! boundary validator, the handler, the persisted state and serialization — the same path
//! a client takes. The harness here is deliberately local to this suite (the sibling
//! `repository_image_lifecycle` suite keeps its own) so neither suite's cases can be
//! changed by editing the other.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to ECR.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator vends, spelled out here rather than imported
/// so a wrong registry id in the implementation fails this test.
const REGISTRY_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The only value `SigningRepositoryFilterType` carries
/// (`smithy_query(query="shape",shape="SigningRepositoryFilterType")` → values
/// `["WILDCARD_MATCH"]`).
const WILDCARD_MATCH: &str = "WILDCARD_MATCH";

/// `SigningRuleList`'s modelled maximum (`length(min: 0, max: 10)`).
const MAX_RULES: usize = 10;

/// `SigningRepositoryFilterList`'s modelled maximum (`length(min: 1, max: 100)`).
const MAX_FILTERS: usize = 100;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
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
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A signing rule as the wire spells it: a signer signing-profile ARN, and the repository
/// filters when the rule carries any.
fn rule(signing_profile_arn: &str, filters: Option<Vec<Value>>) -> Value {
    match filters {
        Some(filters) => json!({
            "signingProfileArn": signing_profile_arn,
            "repositoryFilters": filters,
        }),
        None => json!({"signingProfileArn": signing_profile_arn}),
    }
}

/// One `WILDCARD_MATCH` repository filter.
fn filter(value: &str) -> Value {
    json!({"filter": value, "filterType": WILDCARD_MATCH})
}

/// `count` distinct repository filters, for the cases that exercise the list's bounds.
fn filters(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| filter(&format!("team-{index}/app")))
        .collect()
}

/// `count` distinct signing rules, for the cases that exercise the rule list's bounds.
fn rules(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Profile_{index}"),
                Some(vec![filter(&format!("app-{index}"))]),
            )
        })
        .collect()
}

/// The configurations this emulator must round-trip, spanning what the rule generalises
/// over: how many rules a configuration carries (from none to the modelled maximum),
/// whether a rule filters repositories at all, how many filters it carries (from the
/// modelled minimum to the maximum) and what those filters match, and the shape of the
/// signer profile ARN (partition, region, account and profile name, all within
/// `SigningProfileArn`'s modelled pattern
/// `^arn:aws(-[a-z]+)*:signer:[a-z0-9-]+:[0-9]{12}:\/signing-profiles\/[a-zA-Z0-9_]{2,}$`).
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            // `SigningRuleList` is `length(min: 0, …)`: a configuration with no rules at
            // all is a configuration, not an absent one.
            "no rules",
            json!({"rules": []}),
        ),
        (
            "one rule, filtering no repositories",
            json!({"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/ProdProfile"),
                None,
            )]}),
        ),
        (
            "one rule, one wildcard filter",
            json!({"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/ProdProfile"),
                Some(vec![filter("prod-*")]),
            )]}),
        ),
        (
            // The filter alphabet the model allows: lowercase, digits, `*`, the `._-`
            // separators and `/` namespaces.
            "one rule, filters spanning the filter alphabet",
            json!({"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Mixed_1"),
                Some(vec![
                    filter("*"),
                    filter("team/app"),
                    filter("a.b_c-d"),
                    filter("nested/path/*"),
                ]),
            )]}),
        ),
        (
            // A profile ARN in another partition and region, with the shortest profile
            // name the pattern allows (`[a-zA-Z0-9_]{2,}`).
            "a rule naming another partition, region and account",
            json!({"rules": [rule(
                "arn:aws-us-gov:signer:us-gov-west-1:123456789012:/signing-profiles/Ab",
                Some(vec![filter("gov-*")]),
            )]}),
        ),
        (
            "a rule naming the China partition",
            json!({"rules": [rule(
                "arn:aws-cn:signer:cn-north-1:210987654321:/signing-profiles/Cn_Profile_9",
                None,
            )]}),
        ),
        (
            "one rule at the filter list's modelled minimum of 1",
            json!({"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Min_1"),
                Some(filters(1)),
            )]}),
        ),
        (
            "one rule at the filter list's modelled maximum of 100",
            json!({"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Max_100"),
                Some(filters(MAX_FILTERS)),
            )]}),
        ),
        (
            "two rules, one of them unfiltered",
            json!({"rules": [
                rule(
                    &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/First"),
                    Some(vec![filter("first/*")]),
                ),
                rule(
                    &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Second"),
                    None,
                ),
            ]}),
        ),
        (
            "rules at the modelled maximum of 10",
            json!({"rules": rules(MAX_RULES)}),
        ),
    ]
}

/// A registry that was never given a signing configuration has none to report, and
/// reading it neither creates one nor changes the answer a later read gets.
#[tokio::test]
async fn get_signing_configuration_is_not_found_until_one_is_put() {
    let router = emulator();

    for attempt in ["first read", "second read"] {
        let answer = call(&router, "GetSigningConfiguration", &json!({})).await;
        assert_eq!(
            answer.status, 400,
            "case {attempt:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "SigningConfigurationNotFoundException",
            "case {attempt:?}: expected SigningConfigurationNotFoundException, got {:?}",
            answer.body
        );
    }

    // And the first put is what makes it exist: the same read now answers from state.
    let configuration = json!({"rules": [rule(
        &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/AfterPut"),
        None,
    )]});
    ok(
        &router,
        "after the first put",
        "PutSigningConfiguration",
        &json!({"signingConfiguration": configuration}),
    )
    .await;
    let got = ok(
        &router,
        "after the first put",
        "GetSigningConfiguration",
        &json!({}),
    )
    .await;
    assert_eq!(
        got["signingConfiguration"], configuration,
        "case \"after the first put\": expected the stored configuration {configuration:?}, \
         got {:?}",
        got["signingConfiguration"]
    );
}

/// Whatever shape of configuration a put stores, the get reports it back unchanged, with
/// this registry's account id beside it — and the configuration reported is the one the
/// LAST put stored, because a put replaces the registry's configuration whole.
#[tokio::test]
async fn a_put_configuration_is_reported_back_by_get_whatever_its_shape() {
    let router = emulator();

    for (case, configuration) in round_trip_cases() {
        let put = ok(
            &router,
            case,
            "PutSigningConfiguration",
            &json!({"signingConfiguration": configuration}),
        )
        .await;
        assert_eq!(
            put["signingConfiguration"], configuration,
            "case {case:?}: PutSigningConfiguration must report the configuration it \
             stored {configuration:?}, got {:?}",
            put["signingConfiguration"]
        );

        let got = ok(&router, case, "GetSigningConfiguration", &json!({})).await;
        assert_eq!(
            got["registryId"],
            json!(REGISTRY_ID),
            "case {case:?}: expected registryId {REGISTRY_ID}, got {:?}",
            got["registryId"]
        );
        assert_eq!(
            got["signingConfiguration"], configuration,
            "case {case:?}: expected the configuration the last put stored \
             {configuration:?}, got {:?}",
            got["signingConfiguration"]
        );

        // A read reports; it does not change what the next caller sees.
        let again = ok(&router, case, "GetSigningConfiguration", &json!({})).await;
        assert_eq!(
            again.get("signingConfiguration"),
            got.get("signingConfiguration"),
            "case {case:?}: a second read must report the same configuration, got {:?}",
            again["signingConfiguration"]
        );
    }
}

/// A put the request cannot satisfy is refused, and the registry keeps the configuration
/// it already held: the refused request stores nothing.
#[tokio::test]
async fn a_refused_put_leaves_the_stored_configuration_untouched() {
    let router = emulator();

    // The configuration the registry holds throughout: every refusal below must leave
    // exactly this one in place.
    let held = json!({"rules": [rule(
        &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/Held"),
        Some(vec![filter("held/*")]),
    )]});
    ok(
        &router,
        "the configuration under test",
        "PutSigningConfiguration",
        &json!({"signingConfiguration": held}),
    )
    .await;

    let long_profile_name = "A".repeat(200);
    // Each row is outside a constraint the model states for this input, so the boundary
    // validator refuses it with `ValidationException` before the handler runs.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no signingConfiguration at all, which the input requires",
            json!({}),
        ),
        (
            "a configuration with no rules member, which SigningConfiguration requires",
            json!({"signingConfiguration": {}}),
        ),
        (
            "eleven rules, past SigningRuleList's modelled maximum of 10",
            json!({"signingConfiguration": {"rules": rules(MAX_RULES + 1)}}),
        ),
        (
            "a signingProfileArn that is not a signer signing-profile ARN",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:iam::{REGISTRY_ID}:role/NotASigningProfile"),
                None,
            )]}}),
        ),
        (
            "a signing profile name shorter than the modelled two characters",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/A"),
                None,
            )]}}),
        ),
        (
            "a signingProfileArn past SigningProfileArn's modelled length of 200",
            json!({"signingConfiguration": {"rules": [rule(
                &format!(
                    "arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/{long_profile_name}"
                ),
                None,
            )]}}),
        ),
        (
            "no repository filters, below SigningRepositoryFilterList's minimum of 1",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/NoFilters"),
                Some(vec![]),
            )]}}),
        ),
        (
            "more repository filters than the modelled maximum of 100",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/TooMany"),
                Some(filters(MAX_FILTERS + 1)),
            )]}}),
        ),
        (
            "a filterType outside SigningRepositoryFilterType's one value",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/BadType"),
                Some(vec![json!({"filter": "team/app", "filterType": "EXACT_MATCH"})]),
            )]}}),
        ),
        (
            "a filter value outside SigningRepositoryFilterValue's pattern",
            json!({"signingConfiguration": {"rules": [rule(
                &format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/BadFilter"),
                Some(vec![filter("Team/App")]),
            )]}}),
        ),
    ];

    for (case, request) in cases {
        let answer = call(&router, "PutSigningConfiguration", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );

        let got = ok(&router, case, "GetSigningConfiguration", &json!({})).await;
        assert_eq!(
            got["signingConfiguration"], held,
            "case {case:?}: a refused put must leave {held:?} in place, got {:?}",
            got["signingConfiguration"]
        );
    }
}
