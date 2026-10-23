// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's aliases: `CreateAlias` "creates an alias for a
//! Lambda Event Function version", and `GetAlias` reports it.
//!
//! The rule under test: `CreateAlias` persists, on the function the request names, ONE
//! alias of the name it gives — invoking the version `FunctionVersion` names, carrying
//! the description exactly as supplied and the traffic-shifting configuration exactly as
//! supplied, with a revision id minted once and an ARN that is the function's own ARN
//! qualified by the alias name — and answers that stored configuration; it refuses a
//! name the function already has an alias of, a version the function does not hold, and
//! a routing configuration that names more than one other version, the version the alias
//! already invokes, a version the function does not hold, or a second version beside the
//! unpublished one. The alias is then a qualifier of the function: reading the function
//! through it reports the version it invokes.
//!
//! Every row drives real HTTP requests through the router and reads the alias back
//! through `GetAlias`, so an alias that was answered but never stored fails here. The
//! rows differ along what the rule generalises over: the version the alias invokes
//! (unpublished and published), the alias name (its modelled length bounds, hyphens and
//! underscores), the description (absent, empty, at the modelled maximum), the routing
//! configuration (absent, and a weight at each published bound) and the documented
//! `FunctionName` form the request addresses the function by.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, from the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`).
const VERSION_LATEST: &str = "$LATEST";

/// The bounds the model puts on `Alias` (`@length(min: 1, max: 128)`), from
/// `smithy_query(query="shape", shape="AliasConfiguration")`.
const ALIAS_MIN_LEN: usize = 1;
const ALIAS_MAX_LEN: usize = 128;

/// The bounds the model puts on `Description` (`@length(min: 0, max: 256)`).
const DESCRIPTION_MAX_LEN: usize = 256;

/// The bounds the model puts on `Weight`, the value of
/// `AliasRoutingConfiguration$AdditionalVersionWeights` (`@range(min: 0.0, max: 1.0)`).
const WEIGHT_MIN: f64 = 0.0;
const WEIGHT_MAX: f64 = 1.0;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `CreateAlias` on the function `reference` names.
async fn create_alias(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/aliases"),
        body,
    )
    .await
}

/// `GetAlias` on the function `reference` names.
async fn get_alias(router: &MultiServiceRouter, reference: &str, alias: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/aliases/{alias}"),
        &Value::Null,
    )
    .await
}

/// `GetFunctionConfiguration` on the function `reference` names, qualified by
/// `qualifier`.
async fn get_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/configuration?Qualifier={qualifier}"),
        &Value::Null,
    )
    .await
}

/// Create a function and publish `versions` versions of it, numbered from 1.
///
/// The first comes from `CreateFunction`'s own `Publish` parameter; each further one
/// from a `PublishVersion` that overrides the description, because "Lambda doesn't
/// publish a version if the function's configuration and code haven't changed since the
/// last version".
async fn create_function_with_versions(router: &MultiServiceRouter, name: &str, versions: u32) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": versions >= 1,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    for version in 2..=versions {
        let published = call(
            router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({ "Description": format!("snapshot {version}") }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "PublishVersion {version} of {name:?} must succeed, got {} {}",
            published.status,
            published.text()
        );
        assert_eq!(
            published.string("Version").as_deref(),
            Some(version.to_string().as_str()),
            "PublishVersion must publish version {version} of {name:?}, got {:?}",
            published.string("Version")
        );
    }
}

/// Drop the JSON members that are absent, so a row states only the members it
/// configured and a member the emulator invented shows up as a difference.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The `AliasConfiguration` members of an answer, as the rule is about them.
fn reported(answer: &Answer) -> Value {
    let body = answer.json();
    without_nulls(&json!({
        "AliasArn": body["AliasArn"].clone(),
        "Name": body["Name"].clone(),
        "FunctionVersion": body["FunctionVersion"].clone(),
        "Description": body["Description"].clone(),
        "RoutingConfig": body["RoutingConfig"].clone(),
    }))
}

/// One row of the alias table: the function and how many versions it has, the reference
/// the request addresses it by, the alias name, the request body, and the configuration
/// the alias must then hold.
struct AliasCase {
    name: &'static str,
    function: &'static str,
    versions: u32,
    /// The documented `FunctionName` form the request uses; `None` means the bare name.
    reference: Option<String>,
    alias: String,
    request: Value,
    expected_version: &'static str,
    expected_description: Option<String>,
    expected_routing: Value,
}

/// The rows. Each differs from the others in the version the alias invokes, the alias
/// name, the description, the routing configuration or the `FunctionName` form it is
/// addressed by — the dimensions the rule generalises over.
fn alias_cases() -> Vec<AliasCase> {
    vec![
        // The unpublished version, nothing else configured: the answer carries no
        // description and no routing configuration at all.
        AliasCase {
            name: "an alias to the unpublished version with nothing else configured",
            function: "latest-alias",
            versions: 1,
            reference: None,
            alias: "dev".to_string(),
            request: json!({ "Name": "dev", "FunctionVersion": VERSION_LATEST }),
            expected_version: VERSION_LATEST,
            expected_description: None,
            expected_routing: Value::Null,
        },
        // A published version, described, addressed by the function's full ARN.
        AliasCase {
            name: "an alias to a published version, described, addressed by its arn",
            function: "published-alias",
            versions: 1,
            reference: Some(function_arn("published-alias")),
            alias: "prod".to_string(),
            request: json!({
                "Name": "prod",
                "FunctionVersion": "1",
                "Description": "the version clients invoke",
            }),
            expected_version: "1",
            expected_description: Some("the version clients invoke".to_string()),
            expected_routing: Value::Null,
        },
        // Traffic shifting between two published versions, addressed by the partial ARN
        // form, with a weight in the middle of the modelled range.
        AliasCase {
            name: "an alias that splits its invocations between two published versions",
            function: "shifting-alias",
            versions: 2,
            reference: Some(format!("{ACCOUNT}:function:shifting-alias")),
            alias: "canary".to_string(),
            request: json!({
                "Name": "canary",
                "FunctionVersion": "1",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.25 } },
            }),
            expected_version: "1",
            expected_description: None,
            expected_routing: json!({ "AdditionalVersionWeights": { "2": 0.25 } }),
        },
        // A weight at the bottom of the published `Weight` range, an alias name carrying
        // both documented separators, and the bare `function:` prefix form of the name.
        AliasCase {
            name: "a weight at the published minimum, under a name with - and _",
            function: "min-weight-alias",
            versions: 3,
            reference: Some("function:min-weight-alias".to_string()),
            alias: "blue_green-2".to_string(),
            request: json!({
                "Name": "blue_green-2",
                "FunctionVersion": "3",
                "Description": "",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": WEIGHT_MIN } },
            }),
            expected_version: "3",
            // An empty description is a description in its own right (`@length(min: 0)`),
            // and is reported as configured rather than dropped.
            expected_description: Some(String::new()),
            expected_routing: json!({ "AdditionalVersionWeights": { "2": WEIGHT_MIN } }),
        },
        // A weight at the top of the published range, an alias name of the modelled
        // maximum length, and a description of the modelled maximum length.
        AliasCase {
            name: "a weight at the published maximum, longest name and description",
            function: "max-weight-alias",
            versions: 2,
            reference: None,
            alias: "A".repeat(ALIAS_MAX_LEN),
            request: json!({
                "Name": "A".repeat(ALIAS_MAX_LEN),
                "FunctionVersion": "1",
                "Description": "d".repeat(DESCRIPTION_MAX_LEN),
                "RoutingConfig": { "AdditionalVersionWeights": { "2": WEIGHT_MAX } },
            }),
            expected_version: "1",
            expected_description: Some("d".repeat(DESCRIPTION_MAX_LEN)),
            expected_routing: json!({ "AdditionalVersionWeights": { "2": WEIGHT_MAX } }),
        },
        // An alias name of the modelled minimum length, on a version that is neither the
        // first nor the newest the function published.
        AliasCase {
            name: "the shortest alias name, on a middle version",
            function: "short-alias",
            versions: 3,
            reference: None,
            alias: "x".to_string(),
            request: json!({ "Name": "x", "FunctionVersion": "2" }),
            expected_version: "2",
            expected_description: None,
            expected_routing: Value::Null,
        },
    ]
}

#[tokio::test]
async fn an_alias_is_created_from_the_request_and_read_back() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let mut revision_ids: BTreeSet<String> = BTreeSet::new();

    for case in alias_cases() {
        let label = case.name;
        assert!(
            case.alias.len() >= ALIAS_MIN_LEN && case.alias.len() <= ALIAS_MAX_LEN,
            "case {label:?}: the row's alias name must satisfy the modelled \
             {ALIAS_MIN_LEN}..{ALIAS_MAX_LEN} length, got {}",
            case.alias.len()
        );
        create_function_with_versions(&router, case.function, case.versions).await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        let created = create_alias(&router, &reference, &case.request).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateAlias, got {} {}",
            created.status,
            created.text()
        );

        let expected = without_nulls(&json!({
            "AliasArn": format!("{}:{}", function_arn(case.function), case.alias),
            "Name": case.alias.clone(),
            "FunctionVersion": case.expected_version,
            "Description": case.expected_description.clone(),
            "RoutingConfig": case.expected_routing.clone(),
        }));
        assert_eq!(
            reported(&created),
            expected,
            "case {label:?}: expected the configuration the request described, {expected}, \
             got {}",
            created.text()
        );
        let revision_id = created.string("RevisionId").unwrap_or_default();
        assert!(
            !revision_id.is_empty(),
            "case {label:?}: an alias must carry a RevisionId, got {}",
            created.text()
        );
        assert!(
            revision_ids.insert(revision_id.clone()),
            "case {label:?}: every alias gets its own RevisionId, but {revision_id:?} was \
             minted twice"
        );

        // Read the alias back: one that was answered but never persisted fails here.
        let fetched = get_alias(&router, case.function, &case.alias).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: expected 200 from GetAlias, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            reported(&fetched),
            expected,
            "case {label:?}: GetAlias must report the created alias {expected}, got {}",
            fetched.text()
        );
        assert_eq!(
            fetched.string("RevisionId").as_deref(),
            Some(revision_id.as_str()),
            "case {label:?}: expected the stored RevisionId {revision_id:?}, got {:?}",
            fetched.string("RevisionId")
        );

        // A read changes nothing a later caller sees: the same alias comes back again.
        let again = get_alias(&router, case.function, &case.alias).await;
        assert_eq!(
            (again.status, reported(&again), again.string("RevisionId")),
            (200, expected.clone(), Some(revision_id.clone())),
            "case {label:?}: the alias must be stable across reads, got {} {}",
            again.status,
            again.text()
        );

        // The alias is a qualifier of the function from here on: reading the function
        // through it reports the version it invokes, under the ARN the caller addressed.
        let qualified = get_configuration(&router, case.function, &case.alias).await;
        assert_eq!(
            qualified.status,
            200,
            "case {label:?}: expected 200 from GetFunctionConfiguration qualified by the \
             alias, got {} {}",
            qualified.status,
            qualified.text()
        );
        assert_eq!(
            (
                qualified.string("Version"),
                qualified.string("FunctionArn")
            ),
            (
                Some(case.expected_version.to_string()),
                Some(format!(
                    "{}:{}",
                    function_arn(case.function),
                    case.alias
                ))
            ),
            "case {label:?}: expected the version {:?} the alias invokes under the alias \
             ARN, got {}",
            case.expected_version,
            qualified.text()
        );

        // The store itself holds exactly one alias, of the name the wire reported,
        // invoking the version it reported.
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let store = state.lock();
        let record = store
            .functions
            .get(case.function)
            .unwrap_or_else(|| panic!("case {label:?}: the function must be stored"));
        let stored: Vec<(String, String, Option<String>)> = record
            .aliases
            .iter()
            .map(|(name, alias)| {
                (
                    name.clone(),
                    alias.function_version.clone(),
                    alias.description.clone(),
                )
            })
            .collect();
        assert_eq!(
            stored,
            vec![(
                case.alias.clone(),
                case.expected_version.to_string(),
                case.expected_description.clone()
            )],
            "case {label:?}: expected one stored alias {:?} on version {:?}, got {stored:?}",
            case.alias,
            case.expected_version
        );
    }
}

/// One refusal row: the request, and the status and error code it must be answered with.
struct RefusalCase {
    name: &'static str,
    reference: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_request_no_alias_can_satisfy_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function_with_versions(&router, "refusals", 2).await;
    let first = create_alias(
        &router,
        "refusals",
        &json!({ "Name": "taken", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        first.status,
        201,
        "the alias the rows below collide with must be created, got {} {}",
        first.status,
        first.text()
    );

    let cases = [
        RefusalCase {
            name: "a name the function already has an alias of",
            reference: "refusals",
            request: json!({ "Name": "taken", "FunctionVersion": "2" }),
            status: 409,
            code: "ResourceConflictException",
        },
        RefusalCase {
            name: "a function this emulator does not hold",
            reference: "no-such-function",
            request: json!({ "Name": "any", "FunctionVersion": "1" }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a version the function does not hold",
            reference: "refusals",
            request: json!({ "Name": "unpublished", "FunctionVersion": "99" }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a routing configuration naming a version the function does not hold",
            reference: "refusals",
            request: json!({
                "Name": "routes-nowhere",
                "FunctionVersion": "1",
                "RoutingConfig": { "AdditionalVersionWeights": { "99": 0.5 } },
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a routing configuration naming more than one other version",
            reference: "refusals",
            request: json!({
                "Name": "routes-twice",
                "FunctionVersion": "1",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.3, "1": 0.3 } },
            }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a routing configuration naming the version the alias already invokes",
            reference: "refusals",
            request: json!({
                "Name": "routes-to-itself",
                "FunctionVersion": "2",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.5 } },
            }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "traffic shifting away from the unpublished version",
            reference: "refusals",
            request: json!({
                "Name": "routes-from-latest",
                "FunctionVersion": VERSION_LATEST,
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.5 } },
            }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a function name that qualifies a version of its own",
            reference: "refusals:1",
            request: json!({ "Name": "qualified", "FunctionVersion": "1" }),
            status: 400,
            code: "InvalidParameterValueException",
        },
    ];

    for case in cases {
        let label = case.name;
        let answer = create_alias(&router, case.reference, &case.request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (case.status, case.code.to_string()),
            "case {label:?}: expected {} {}, got {} {}",
            case.status,
            case.code,
            answer.status,
            answer.text()
        );
    }

    // None of the refusals above created anything, and the one alias that was created is
    // untouched.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let record = store
        .functions
        .get("refusals")
        .expect("the function must be stored");
    let names: Vec<&String> = record.aliases.keys().collect();
    assert_eq!(
        names,
        vec!["taken"],
        "a refused CreateAlias must store nothing, got {names:?}"
    );
    assert_eq!(
        record.aliases["taken"].function_version, "1",
        "a refused CreateAlias must not change the alias it collided with, got {:?}",
        record.aliases["taken"].function_version
    );
}

#[tokio::test]
async fn an_all_digit_alias_name_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function_with_versions(&router, "digit-names", 1).await;

    // The model's `Alias` pattern is `^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$`: a name that is
    // all digits names a version, never an alias, so it is refused as a bad request
    // whether the wire's own constraint check or the handler catches it.
    for name in ["1", "0", "123"] {
        let answer = create_alias(
            &router,
            "digit-names",
            &json!({ "Name": name, "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {name:?}: an all-digit alias name must be refused with 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let record = store
        .functions
        .get("digit-names")
        .expect("the function must be stored");
    assert!(
        record.aliases.is_empty(),
        "no alias may have been stored, got {:?}",
        record.aliases.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn an_alias_of_a_function_is_not_an_alias_of_another() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function_with_versions(&router, "owner", 1).await;
    create_function_with_versions(&router, "neighbour", 1).await;

    let created = create_alias(
        &router,
        "owner",
        &json!({ "Name": "shared-name", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateAlias must succeed, got {} {}",
        created.status,
        created.text()
    );

    // The alias belongs to the function it was created on: the same name is free on
    // another function, and reading it there is a 404 until it is created.
    let missing = get_alias(&router, "neighbour", "shared-name").await;
    assert_eq!(
        (missing.status, missing.error_code()),
        (404, "ResourceNotFoundException".to_string()),
        "an alias of another function must not be found, got {} {}",
        missing.status,
        missing.text()
    );

    let elsewhere = create_alias(
        &router,
        "neighbour",
        &json!({ "Name": "shared-name", "FunctionVersion": VERSION_LATEST }),
    )
    .await;
    assert_eq!(
        elsewhere.status,
        201,
        "the same alias name must be free on another function, got {} {}",
        elsewhere.status,
        elsewhere.text()
    );
    assert_eq!(
        elsewhere.string("AliasArn"),
        Some(format!("{}:shared-name", function_arn("neighbour"))),
        "the alias ARN must be the ARN of the function it was created on, got {:?}",
        elsewhere.string("AliasArn")
    );
    assert_eq!(
        get_alias(&router, "owner", "shared-name")
            .await
            .string("FunctionVersion"),
        Some("1".to_string()),
        "the first function's alias must be untouched by the second's"
    );
}
