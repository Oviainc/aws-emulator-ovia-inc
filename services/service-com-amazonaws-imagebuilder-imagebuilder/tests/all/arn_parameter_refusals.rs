// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service refuses a value its modelled ARN patterns do
//! not admit.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization and the modelled-constraint walk — so what is asserted is what a signed
//! AWS SDK request observes.
//!
//! The rule under test: a value a modelled ARN pattern does not admit is refused as the
//! parameter value it is — HTTP 400 `InvalidParameterValueException`, naming the parameter
//! and the pattern its value had to match, with the partition and Region fields of that
//! pattern spelled in the form of the layer that READ the value: the Region-code form for a
//! value read out of the request BODY, the character-class form for one read out of the
//! REQUEST LINE. A constraint class this service states no requirement for keeps the
//! framework's own wording.
//!
//! The rows span what the rule generalises over: which operation carries the member, where
//! the member rides (a JSON body member, a query parameter), which resource kind the ARN
//! names, and how the submitted value misses the pattern (no ARN at all, another service's
//! ARN, another resource kind's ARN, a field missing).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The pattern `ListImagePackages` states for `imageBuildVersionArn`: the modelled
/// `ImageBuildVersionArn` pattern
/// (`smithy_query(query="shape",shape="ImageBuildVersionArn")`) with its partition and
/// Region fields spelled as the layer that reads a BODY member states them.
const IMAGE_BUILD_VERSION_ARN_PATTERN: &str = "^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+$";

/// The pattern `GetImageRecipe` states for `imageRecipeArn`, from the modelled
/// `ImageRecipeArn`, with the two fields spelled as the layer that reads a REQUEST-LINE
/// parameter states them. See [`IMAGE_BUILD_VERSION_ARN_PATTERN`].
const IMAGE_RECIPE_ARN_PATTERN: &str = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$";

/// The pattern `GetComponent` states for `componentBuildVersionArn`, from the modelled
/// `ComponentVersionArnOrBuildVersionArn`. See [`IMAGE_RECIPE_ARN_PATTERN`].
const COMPONENT_ARN_PATTERN: &str = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws(?:-[a-z-]+)?|third-party):component/[a-z0-9-_]+/(?:(?:([0-9]+|x)\\.([0-9]+|x)\\.([0-9]+|x))|(?:[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+))$";

/// The pattern `GetInfrastructureConfiguration` states for
/// `infrastructureConfigurationArn`, from the modelled `InfrastructureConfigurationArn`.
/// See [`IMAGE_RECIPE_ARN_PATTERN`].
const INFRASTRUCTURE_CONFIGURATION_ARN_PATTERN: &str =
    "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$";

/// One row of the refusal table: the case, the member whose value is refused, the pattern
/// the refusal states, and the request that carries the refused value (method, URI, body).
type RefusalRow<'a> = (&'a str, &'a str, &'a str, &'a str, String, Option<Value>);

/// One row of the control table: the case and the request (method, URI, body).
type ControlRow<'a> = (&'a str, &'a str, String, Option<Value>);

/// One emulator with an empty store, mounted exactly as the router mounts it. Nothing is
/// stored: every case below is refused before any handler runs.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status, error code and message of one call.
async fn call(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, String) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()));
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; read either.
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(error_type)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {parsed}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// The refusal this service states for `member`, whose value had to match `pattern`.
fn expected_message(member: &str, pattern: &str) -> String {
    format!(
        "The value supplied for parameter '{member}' is not valid. {member} must match \
         pattern {pattern}"
    )
}

#[tokio::test]
async fn a_value_a_modelled_arn_pattern_refuses_is_an_invalid_parameter_value() {
    let router = emulator();

    // Each row: the member, the pattern it states, and one request carrying a value that
    // pattern does not admit.
    let rows: Vec<RefusalRow> = vec![
        (
            "no ARN at all, in a body member",
            "imageBuildVersionArn",
            IMAGE_BUILD_VERSION_ARN_PATTERN,
            "POST",
            "/ListImagePackages".to_string(),
            Some(json!({"imageBuildVersionArn": "my-image"})),
        ),
        (
            "another service's ARN, in a body member",
            "imageBuildVersionArn",
            IMAGE_BUILD_VERSION_ARN_PATTERN,
            "POST",
            "/ListImagePackages".to_string(),
            Some(json!({
                "imageBuildVersionArn":
                    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:image/ami-00000000000000000"),
            })),
        ),
        (
            "an image build version ARN missing its build number",
            "imageBuildVersionArn",
            IMAGE_BUILD_VERSION_ARN_PATTERN,
            "POST",
            "/ListImagePackages".to_string(),
            Some(json!({
                "imageBuildVersionArn":
                    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/my-image/1.0.0"),
            })),
        ),
        (
            "another resource kind's ARN, in a query parameter",
            "imageRecipeArn",
            IMAGE_RECIPE_ARN_PATTERN,
            "GET",
            format!(
                "/GetImageRecipe?imageRecipeArn=arn:aws:imagebuilder:{REGION}:{ACCOUNT}:\
                 component/my-component/1.0.0/1"
            ),
            None,
        ),
        (
            "a component ARN whose version is not three nodes",
            "componentBuildVersionArn",
            COMPONENT_ARN_PATTERN,
            "GET",
            format!(
                "/GetComponent?componentBuildVersionArn=arn:aws:imagebuilder:{REGION}:\
                 {ACCOUNT}:component/my-component/1.0"
            ),
            None,
        ),
        (
            "an infrastructure configuration ARN carrying an uppercase name",
            "infrastructureConfigurationArn",
            INFRASTRUCTURE_CONFIGURATION_ARN_PATTERN,
            "GET",
            format!(
                "/GetInfrastructureConfiguration?infrastructureConfigurationArn=arn:aws:\
                 imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/My-Config"
            ),
            None,
        ),
    ];

    for (case, member, pattern, method, uri, body) in &rows {
        let (status, code, message) = call(&router, case, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {code}: {message}"
        );
        assert_eq!(
            code, "InvalidParameterValueException",
            "case {case:?}: expected the error code \"InvalidParameterValueException\", got \
             {code:?}"
        );
        let expected = expected_message(member, pattern);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_constraint_class_this_service_states_no_requirement_for_keeps_the_framework_wording() {
    let router = emulator();

    // The control: this service states a requirement for the constraint classes its
    // recorded refusals establish one for (a pattern, a numeric bound, an enumerated value
    // set). A class they do not — a string past a `@length` maximum, a member the shape
    // requires and the request omits — is left to the framework's own refusal rather than
    // being given a clause this service has never been seen to state.
    let rows: Vec<ControlRow> = vec![
        (
            "a description past its modelled length maximum",
            "PUT",
            "/CreateComponent".to_string(),
            Some(json!({
                "name": "control-component",
                "semanticVersion": "1.0.0",
                "platform": "Linux",
                "clientToken": "ctrl-0001",
                "description": "d".repeat(1025),
                "data": "name: Control\nschemaVersion: 1.0\n",
            })),
        ),
        (
            "a member the shape requires and the request omits",
            "PUT",
            "/CreateComponent".to_string(),
            Some(json!({
                "name": "control-component",
                "platform": "Linux",
                "clientToken": "ctrl-0002",
                "data": "name: Control\nschemaVersion: 1.0\n",
            })),
        ),
    ];

    for (case, method, uri, body) in &rows {
        let (status, code, message) = call(&router, case, method, uri, body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {code}: {message}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the framework's own refusal, got {code:?}: {message}"
        );
    }
}
