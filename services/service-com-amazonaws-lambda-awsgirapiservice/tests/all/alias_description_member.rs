// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Description` member of an `AliasConfiguration`.
//!
//! The rule under test: every alias body this service answers carries the
//! `AliasConfiguration$Description` member, whatever the caller configured — an alias
//! created or updated without a description reports it as the empty string rather than
//! omitting it, so a client can read the member unconditionally. The member's own bound
//! (`@length(min: 0, max: 256)`, from
//! `smithy_query(query="shape", shape="AliasConfiguration")`) admits the empty string, so
//! "no description" and "the empty description" are one value on the wire.
//!
//! The rule is about the SHAPE, not about one operation, so each row is asserted on every
//! operation that answers an alias configuration: `CreateAlias`, `GetAlias`,
//! `UpdateAlias` and each `ListAliases` entry. The rows differ along what the rule
//! generalises over — the description the request states (absent, the empty string, one
//! character, a sentence, the modelled maximum length, and one past the alias's own
//! name), the version the alias invokes and whether the alias shifts traffic — and each
//! row is read back through `GetAlias`, so a value that was answered but never stored
//! fails here.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here. Nothing in it is verified: the
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, from the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`).
const VERSION_LATEST: &str = "$LATEST";

/// The upper bound the model puts on `Description` (`@length(min: 0, max: 256)`).
const DESCRIPTION_MAX_LEN: usize = 256;

/// What a dispatched request answered.
struct Answer {
    status: u16,
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
        body,
    }
}

/// Create a function and publish `versions` versions of it, numbered from 1.
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
            "Publish": true,
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
    }
}

/// The `Description` member of an alias body, distinguishing "absent" (`None`) from "the
/// empty string" (`Some(String::new())`) — which is the whole point of the rule.
fn described(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("Description")
        .map(|member| match member {
            Value::String(text) => text.clone(),
            other => panic!("expected Description to be a string, got {other}"),
        })
}

/// One row: the description the create request states, and the one every alias body must
/// then report.
struct DescriptionCase {
    name: &'static str,
    function: &'static str,
    /// How many versions the function publishes, so a row can invoke a middle version.
    versions: u32,
    alias: &'static str,
    /// The version the alias invokes.
    function_version: &'static str,
    /// The routing configuration the alias is created with, or `Value::Null` for none:
    /// the rule holds whatever else the alias carries.
    routing: Value,
    /// The `Description` the create request states, or `None` for a request that states
    /// the member not at all.
    stated: Option<String>,
    /// What every alias body must report.
    expected: String,
}

/// The rows. Each differs in the description it states — the dimension the rule
/// generalises over — and in the version invoked and the routing configuration, so the
/// member is a property of the alias body rather than of one kind of alias.
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        // The cause the issue recorded: no description stated at all.
        DescriptionCase {
            name: "no description stated",
            function: "ad-absent",
            versions: 1,
            alias: "absent",
            function_version: "1",
            routing: Value::Null,
            stated: None,
            expected: String::new(),
        },
        // The same value, stated explicitly: the two are one value on the wire.
        DescriptionCase {
            name: "the empty description stated explicitly",
            function: "ad-empty",
            versions: 1,
            alias: "empty",
            function_version: VERSION_LATEST,
            routing: Value::Null,
            stated: Some(String::new()),
            expected: String::new(),
        },
        // One character: the shortest description that is not the empty string.
        DescriptionCase {
            name: "a one-character description",
            function: "ad-one-char",
            versions: 1,
            alias: "one-char",
            function_version: "1",
            routing: Value::Null,
            stated: Some("d".to_string()),
            expected: "d".to_string(),
        },
        // A sentence, on an alias that also shifts traffic to a second version.
        DescriptionCase {
            name: "a sentence, on an alias that shifts traffic",
            function: "ad-sentence",
            versions: 2,
            alias: "canary",
            function_version: "1",
            routing: json!({ "AdditionalVersionWeights": { "2": 0.25 } }),
            stated: Some("the version clients invoke".to_string()),
            expected: "the version clients invoke".to_string(),
        },
        // The published maximum, on a middle version of a function with three.
        DescriptionCase {
            name: "a description at the modelled maximum length",
            function: "ad-max",
            versions: 3,
            alias: "max",
            function_version: "2",
            routing: Value::Null,
            stated: Some("d".repeat(DESCRIPTION_MAX_LEN)),
            expected: "d".repeat(DESCRIPTION_MAX_LEN),
        },
        // Whitespace alone is a description in its own right, and is not collapsed to
        // the value an unset one reports.
        DescriptionCase {
            name: "a description of whitespace alone",
            function: "ad-blank",
            versions: 1,
            alias: "blank",
            function_version: VERSION_LATEST,
            routing: Value::Null,
            stated: Some(" ".to_string()),
            expected: " ".to_string(),
        },
    ]
}

#[tokio::test]
async fn every_alias_body_reports_the_description_member() {
    for case in description_cases() {
        let label = case.name;
        assert!(
            case.expected.len() <= DESCRIPTION_MAX_LEN,
            "case {label:?}: the row's description must satisfy the modelled \
             0..{DESCRIPTION_MAX_LEN} length, got {}",
            case.expected.len()
        );

        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function_with_versions(&router, case.function, case.versions).await;

        let mut request = json!({
            "Name": case.alias,
            "FunctionVersion": case.function_version,
        });
        if let Some(stated) = case.stated.clone() {
            request["Description"] = json!(stated);
        }
        if !case.routing.is_null() {
            request["RoutingConfig"] = case.routing.clone();
        }

        // CreateAlias answers the configuration it stored.
        let created = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{}/aliases", case.function),
            &request,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateAlias, got {} {}",
            created.status,
            created.text()
        );
        let actual = described(&created);
        assert_eq!(
            actual.as_deref(),
            Some(case.expected.as_str()),
            "case {label:?}: expected CreateAlias to report Description {:?}, got {actual:?}",
            case.expected
        );

        // GetAlias reports the same value out of the store.
        let fetched = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{}/aliases/{}", case.function, case.alias),
            &Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: expected 200 from GetAlias, got {} {}",
            fetched.status,
            fetched.text()
        );
        let actual = described(&fetched);
        assert_eq!(
            actual.as_deref(),
            Some(case.expected.as_str()),
            "case {label:?}: expected GetAlias to report Description {:?}, got {actual:?}",
            case.expected
        );

        // Each ListAliases entry is the same configuration.
        let listed = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{}/aliases", case.function),
            &Value::Null,
        )
        .await;
        assert_eq!(
            listed.status,
            200,
            "case {label:?}: expected 200 from ListAliases, got {} {}",
            listed.status,
            listed.text()
        );
        let body = listed.json();
        let entries = body["Aliases"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {label:?}: expected the one alias created, got {}",
            listed.text()
        );
        let entry = entries[0].get("Description").cloned();
        assert_eq!(
            entry,
            Some(json!(case.expected)),
            "case {label:?}: expected the listed entry to report Description {:?}, got {entry:?}",
            case.expected
        );

        // UpdateAlias states no description, so it keeps the one the alias holds — and
        // reports the member either way.
        let updated = call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{}/aliases/{}", case.function, case.alias),
            &json!({ "FunctionVersion": case.function_version }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {label:?}: expected 200 from UpdateAlias, got {} {}",
            updated.status,
            updated.text()
        );
        let actual = described(&updated);
        assert_eq!(
            actual.as_deref(),
            Some(case.expected.as_str()),
            "case {label:?}: expected UpdateAlias to report Description {:?}, got {actual:?}",
            case.expected
        );
    }
}

/// An alias whose description is CLEARED by an update reports the member as the empty
/// string, the same as one that never had a description: the member is reported from what
/// is stored, so the two are indistinguishable on the wire.
#[tokio::test]
async fn an_alias_whose_description_is_cleared_reports_the_empty_string() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "ad-cleared";
    create_function_with_versions(&router, function, 1).await;

    let created = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
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

    for (case, stated, expected) in [
        ("cleared to the empty string", json!(""), ""),
        ("restated", json!("second"), "second"),
        ("cleared again", json!(""), ""),
    ] {
        let updated = call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{function}/aliases/live"),
            &json!({ "Description": stated }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200 from UpdateAlias, got {} {}",
            updated.status,
            updated.text()
        );
        let actual = described(&updated);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected UpdateAlias to report Description {expected:?}, got \
             {actual:?}"
        );

        let fetched = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{function}/aliases/live"),
            &Value::Null,
        )
        .await;
        let actual = described(&fetched);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected GetAlias to report Description {expected:?}, got {actual:?}"
        );
    }
}
