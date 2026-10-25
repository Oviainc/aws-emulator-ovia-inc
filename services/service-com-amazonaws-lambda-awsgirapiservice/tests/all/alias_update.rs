// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAlias`: "Updates the configuration of a Lambda Event
//! Function alias."
//!
//! The rule under test: an update of an alias the function holds replaces exactly the
//! members the request states, leaves every member it omits at the value the store holds,
//! and mints a new `RevisionId` ("A unique identifier that changes when you update the
//! alias") — and `RevisionId` on the REQUEST is the precondition over that identifier
//! ("Only update the alias if the revision ID matches the ID that's specified. Use this
//! option to avoid modifying an alias that has changed since you last read it"), so of
//! two writes that both carry the revision one read, the first succeeds and the second is
//! refused with the modelled `PreconditionFailedException`. An alias the function does not
//! hold, and a version the function has not got, are refused with the modelled
//! `ResourceNotFoundException` without changing anything.
//!
//! Every row drives real HTTP requests through the router and reads the alias back through
//! `GetAlias`, so an update that answered 200 without persisting the change fails here. The
//! rows differ along what the rule generalises over: WHICH members the update states (the
//! version alone, the description alone, the routing configuration alone, all of them, and
//! none of them), the version it retargets to (the unpublished version and each of two
//! published ones), and the documented `FunctionName` form the request addresses the
//! function by.
//!
//! This module also owns the ONE alias harness this test binary has; the deletion suite
//! ([`crate::alias_deletion`]) drives its requests through it.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
pub(crate) const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here. Nothing in it is verified: the harness turns
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The label the unpublished version is stored under.
pub(crate) const LATEST: &str = "$LATEST";

/// The sentence every operation taking a `RevisionId` precondition on a function or an
/// alias refuses a mismatch with (`state::CONFIGURATION_REVISION_PRECONDITION_MESSAGE`),
/// so a change to that constant fails this suite.
const PRECONDITION_MESSAGE: &str =
    "The Revision Id provided does not match the latest Revision Id. Call the \
     GetFunction/GetAlias API to retrieve the latest Revision Id";

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    pub(crate) fn json(&self) -> Value {
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
    pub(crate) fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    pub(crate) fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The `message` of a wire error, however the protocol spells the member.
    pub(crate) fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    /// The body text, for a failure message.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
pub(crate) fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
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

/// Create a function with `versions` published versions, so an alias has versions to
/// invoke.
pub(crate) async fn create_function(router: &MultiServiceRouter, name: &str, versions: usize) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    for version in 1..=versions {
        // Each publish states its OWN description, so each is a change the function has
        // not published yet: `PublishVersion` re-reports the newest version rather than
        // minting a second one for a configuration identical to it.
        let published = call(
            router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &json!({ "Description": format!("version {version}") }),
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
            published.string("Version"),
            Some(version.to_string()),
            "PublishVersion must mint version {version} of {name:?}, got {}",
            published.text()
        );
    }
}

/// `CreateAlias` on `function`.
pub(crate) async fn create_alias(
    router: &MultiServiceRouter,
    function: &str,
    body: &Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        body,
    )
    .await
}

/// `GetAlias` for the alias `name` of `function`.
pub(crate) async fn get_alias(
    router: &MultiServiceRouter,
    function: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{function}/aliases/{name}"),
        &Value::Null,
    )
    .await
}

/// `UpdateAlias` for the alias `name` of the function `reference` names.
pub(crate) async fn update_alias(
    router: &MultiServiceRouter,
    reference: &str,
    name: &str,
    body: &Value,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{reference}/aliases/{name}"),
        body,
    )
    .await
}

/// `DeleteAlias` for the alias `name` of the function `reference` names.
pub(crate) async fn delete_alias(
    router: &MultiServiceRouter,
    reference: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/{reference}/aliases/{name}"),
        &Value::Null,
    )
    .await
}

/// One update row: which members the request states, and how it addresses the function.
struct UpdateCase {
    name: &'static str,
    function: &'static str,
    /// The alias as it is created, before the update.
    created: Value,
    /// The members the update states, beside the `RevisionId` the suite adds.
    update: Value,
    /// How the update addresses the function, built from the function name.
    reference: fn(&str) -> String,
    /// What the alias must report afterwards: `(FunctionVersion, Description, routing
    /// configuration)`.
    expected: (&'static str, Option<&'static str>, Value),
}

/// The rows. Each states a different subset of the updatable members over a different
/// starting alias, addressed through a different documented `FunctionName` form ("Function
/// name", "Function ARN", "Partial ARN", and the `function:` prefix the modelled
/// `FunctionName` pattern admits).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "the version alone, retargeting to a published version",
            function: "ua-version-only",
            created: json!({ "Name": "live", "FunctionVersion": "1", "Description": "first" }),
            update: json!({ "FunctionVersion": "2" }),
            reference: |name| name.to_string(),
            expected: ("2", Some("first"), Value::Null),
        },
        UpdateCase {
            name: "the version alone, retargeting to the unpublished version",
            function: "ua-version-latest",
            created: json!({ "Name": "live", "FunctionVersion": "2" }),
            update: json!({ "FunctionVersion": LATEST }),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            expected: (LATEST, None, Value::Null),
        },
        UpdateCase {
            name: "the description alone",
            function: "ua-description-only",
            created: json!({ "Name": "live", "FunctionVersion": "1", "Description": "first" }),
            update: json!({ "Description": "second" }),
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            expected: ("1", Some("second"), Value::Null),
        },
        UpdateCase {
            name: "the routing configuration alone, splitting traffic to a second version",
            function: "ua-routing-only",
            created: json!({ "Name": "live", "FunctionVersion": "1" }),
            update: json!({
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.25 } },
            }),
            reference: |name| format!("function:{name}"),
            expected: (
                "1",
                None,
                json!({ "AdditionalVersionWeights": { "2": 0.25 } }),
            ),
        },
        UpdateCase {
            name: "every updatable member at once",
            function: "ua-all-members",
            created: json!({
                "Name": "live",
                "FunctionVersion": "1",
                "Description": "first",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.5 } },
            }),
            update: json!({
                "FunctionVersion": "2",
                "Description": "second",
                "RoutingConfig": { "AdditionalVersionWeights": { "3": 0.75 } },
            }),
            reference: |name| name.to_string(),
            expected: (
                "2",
                Some("second"),
                json!({ "AdditionalVersionWeights": { "3": 0.75 } }),
            ),
        },
        UpdateCase {
            name: "no updatable member at all, which only re-mints the revision",
            function: "ua-no-members",
            created: json!({
                "Name": "live",
                "FunctionVersion": "3",
                "Description": "first",
                "RoutingConfig": { "AdditionalVersionWeights": { "1": 0.1 } },
            }),
            update: json!({}),
            expected: (
                "3",
                Some("first"),
                json!({ "AdditionalVersionWeights": { "1": 0.1 } }),
            ),
            reference: |name| name.to_string(),
        },
    ]
}

/// The alias's `(FunctionVersion, Description, RoutingConfig)` as an answer reports them.
fn reported(answer: &Answer) -> (String, Option<String>, Value) {
    let body = answer.json();
    (
        answer.string("FunctionVersion").unwrap_or_default(),
        answer.string("Description"),
        body.get("RoutingConfig").cloned().unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn an_update_replaces_the_members_it_states_and_keeps_the_rest() {
    for case in update_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function, 3).await;

        let created = create_alias(&router, case.function, &case.created).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: the alias under test must be created, got {} {}",
            created.status,
            created.text()
        );
        let created_revision = created
            .string("RevisionId")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a RevisionId"));

        // A second alias of the same function that no update below names: it must be
        // untouched, revision included.
        let bystander = create_alias(
            &router,
            case.function,
            &json!({ "Name": "bystander", "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            bystander.status,
            201,
            "case {label:?}: the bystander alias must be created, got {} {}",
            bystander.status,
            bystander.text()
        );

        let mut body = case.update.as_object().cloned().unwrap_or_else(Map::new);
        body.insert("RevisionId".to_string(), json!(created_revision.clone()));
        let reference = (case.reference)(case.function);
        let updated = update_alias(&router, &reference, "live", &Value::Object(body)).await;
        assert_eq!(
            updated.status,
            200,
            "case {label:?}: expected the modelled 200 from UpdateAlias, got {} {}",
            updated.status,
            updated.text()
        );

        let expected = (
            case.expected.0.to_string(),
            case.expected.1.map(str::to_string),
            case.expected.2.clone(),
        );
        assert_eq!(
            reported(&updated),
            expected,
            "case {label:?}: expected the alias to report {expected:?}, got {}",
            updated.text()
        );
        assert_eq!(
            updated.string("AliasArn"),
            Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{}:live",
                case.function
            )),
            "case {label:?}: the answer names the alias by its own ARN, got {}",
            updated.text()
        );
        let updated_revision = updated
            .string("RevisionId")
            .unwrap_or_else(|| panic!("case {label:?}: the update answers a RevisionId"));
        assert_ne!(
            updated_revision, created_revision,
            "case {label:?}: a write must change the RevisionId, got {updated_revision:?} \
             for both"
        );

        // Read back through `GetAlias`: the update persisted what it answered.
        let read = get_alias(&router, case.function, "live").await;
        assert_eq!(
            (read.status, reported(&read), read.string("RevisionId")),
            (200, expected.clone(), Some(updated_revision)),
            "case {label:?}: GetAlias must report what the update stored, got {} {}",
            read.status,
            read.text()
        );

        let untouched = get_alias(&router, case.function, "bystander").await;
        assert_eq!(
            (
                untouched.status,
                untouched.string("FunctionVersion"),
                untouched.string("RevisionId")
            ),
            (
                200,
                Some("1".to_string()),
                bystander.string("RevisionId")
            ),
            "case {label:?}: every other alias of the function must be untouched, got {} {}",
            untouched.status,
            untouched.text()
        );
    }
}

#[tokio::test]
async fn of_two_writes_carrying_one_revision_exactly_one_wins() {
    // Each row races two updates that both carry the revision the alias was read at. The
    // rows differ in what the two writers ask for — two different versions, two
    // descriptions, a version against a routing change — so the winner is observable in
    // the stored alias rather than only in the status codes.
    for (label, function, first, second) in [
        (
            "two writers retargeting to different versions",
            "ua-race-versions",
            json!({ "FunctionVersion": "1" }),
            json!({ "FunctionVersion": "2" }),
        ),
        (
            "two writers re-describing the alias",
            "ua-race-descriptions",
            json!({ "Description": "by-first" }),
            json!({ "Description": "by-second" }),
        ),
        (
            "one writer retargeting, one shifting traffic",
            "ua-race-mixed",
            json!({ "FunctionVersion": "3" }),
            json!({ "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.2 } } }),
        ),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, function, 3).await;
        let created = create_alias(
            &router,
            function,
            &json!({ "Name": "live", "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: the alias under test must be created, got {} {}",
            created.status,
            created.text()
        );
        let revision = created
            .string("RevisionId")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a RevisionId"));

        let mut answers = Vec::new();
        for body in [&first, &second] {
            let mut request = body.as_object().cloned().unwrap_or_else(Map::new);
            request.insert("RevisionId".to_string(), json!(revision.clone()));
            answers.push(update_alias(&router, function, "live", &Value::Object(request)).await);
        }

        let winners: Vec<u16> = answers
            .iter()
            .filter(|answer| answer.status == 200)
            .map(|answer| answer.status)
            .collect();
        let statuses: Vec<u16> = answers.iter().map(|answer| answer.status).collect();
        assert_eq!(
            winners.len(),
            1,
            "case {label:?}: expected exactly one winner among the racers, got statuses \
             {statuses:?}"
        );

        let loser = answers
            .iter()
            .find(|answer| answer.status != 200)
            .unwrap_or_else(|| panic!("case {label:?}: one racer must be refused"));
        assert_eq!(
            loser.status,
            412,
            "case {label:?}: the loser is refused with the modelled precondition status, \
             got {} {}",
            loser.status,
            loser.text()
        );
        assert!(
            loser.error_code().contains("PreconditionFailedException"),
            "case {label:?}: expected PreconditionFailedException, got {}",
            loser.error_code()
        );
        assert_eq!(
            loser
                .string("message")
                .or_else(|| loser.string("Message")),
            Some(PRECONDITION_MESSAGE.to_string()),
            "case {label:?}: the refusal states the revision precondition sentence, got {:?}",
            loser.text()
        );

        // The stored alias is the WINNER's, and its revision is the one the winner
        // answered — so the loser changed nothing.
        let winner = answers
            .iter()
            .find(|answer| answer.status == 200)
            .unwrap_or_else(|| panic!("case {label:?}: one racer must succeed"));
        let read = get_alias(&router, function, "live").await;
        assert_eq!(
            (reported(&read), read.string("RevisionId")),
            (reported(winner), winner.string("RevisionId")),
            "case {label:?}: the stored alias must be the winner's, got {}",
            read.text()
        );
    }
}

#[tokio::test]
async fn an_update_of_something_the_function_does_not_hold_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "ua-refusals", 1).await;
    let created = create_alias(
        &router,
        "ua-refusals",
        &json!({ "Name": "live", "FunctionVersion": "1", "Description": "first" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let before = get_alias(&router, "ua-refusals", "live").await;

    // Each row names something that does not exist, or asks for something the rule does
    // not allow, and must change nothing.
    for (label, reference, alias, body, status, code) in [
        (
            "an alias the function does not hold",
            "ua-refusals",
            "absent",
            json!({ "FunctionVersion": "1" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a function that does not exist",
            "ua-absent-function",
            "live",
            json!({ "FunctionVersion": "1" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version the function has not published",
            "ua-refusals",
            "live",
            json!({ "FunctionVersion": "9" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a revision the alias does not carry",
            "ua-refusals",
            "live",
            json!({ "Description": "x", "RevisionId": "11111111-2222-3333-4444-555555555555" }),
            412,
            "PreconditionFailedException",
        ),
        (
            "a second version that is the one the alias already invokes",
            "ua-refusals",
            "live",
            json!({ "RoutingConfig": { "AdditionalVersionWeights": { "1": 0.5 } } }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a second version the function has not published",
            "ua-refusals",
            "live",
            json!({ "RoutingConfig": { "AdditionalVersionWeights": { "7": 0.5 } } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a function name qualified with an alias",
            "ua-refusals:live",
            "live",
            json!({ "FunctionVersion": "1" }),
            400,
            "InvalidParameterValueException",
        ),
    ] {
        let answer = update_alias(&router, reference, alias, &body).await;
        assert_eq!(
            answer.status,
            status,
            "case {label:?}: expected {status} from UpdateAlias, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(code),
            "case {label:?}: expected {code}, got {}",
            answer.error_code()
        );
    }

    let after = get_alias(&router, "ua-refusals", "live").await;
    assert_eq!(
        (reported(&after), after.string("RevisionId")),
        (reported(&before), before.string("RevisionId")),
        "a refused update must leave the alias exactly as it was, got {}",
        after.text()
    );
}
