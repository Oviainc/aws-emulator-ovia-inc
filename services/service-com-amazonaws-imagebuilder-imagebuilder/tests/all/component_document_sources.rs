// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHERE a component document comes from, what makes a document
//! acceptable, and how this service words a create that collides and a delete that is
//! blocked.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — with the emulator's own
//! Amazon S3 mounted beside imagebuilder on one engine, exactly the way the assembled
//! emulator runs them. A document staged with `PutObject` is therefore the document the
//! `uri` paths resolve.
//!
//! The rules under test:
//!
//! * `CreateComponent` takes the document "using exactly one of the following methods:
//!   Inline, using the data property in the request body. A URL that points to a YAML
//!   document file stored in Amazon S3, using the uri property", and FETCHES the object a
//!   `uri` names at create time: the bytes it was served become the component's own
//!   document, a one-time snapshot a later overwrite of the object cannot change, and an
//!   address the object store serves nothing at is refused as that parameter's value with
//!   nothing minted.
//! * A document this service cannot accept — one declaring no phases among them — is an
//!   invalid value of the member that CARRIED it, refused as this service refuses every
//!   parameter whose value it cannot accept, and nothing is minted.
//! * A create whose resource already exists is refused as the create-collision it is,
//!   naming the kind of resource it would have minted and the ARN of the one that already
//!   carries it, read from this service's own state; the existing resource is untouched and
//!   another version of the same name is still accepted.
//! * A delete of a resource another stored resource still references is refused naming the
//!   ARN the request asked to delete, and a sibling nothing references deletes and then
//!   reads as a missing resource.
//!
//! The tables span what the rules generalise over: which member carried the document, the
//! shape of the document (build phases, a lone test phase, declared parameters), the shape
//! of the key (a bare name, a nested prefix), which part of the address resolves to nothing
//! (the bucket, the key, a key that only prefixes or only extends a stored one), the way a
//! document declares no phases, and which kind of resource a create collides on.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
///
/// Amazon S3 places a bucket created with no `LocationConstraint` in us-east-1 ("If you
/// don't specify a Region, the bucket is created in the US East (N. Virginia) Region
/// (us-east-1) by default"), which is also the Region imagebuilder serves a request in by
/// default, so one Region label serves the object store and the control plane alike.
const REGION: &str = "us-east-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bucket every staged document below lives in.
const BUCKET: &str = "imagebuilder-components";

/// A bucket name no `CreateBucket` here ever creates.
const ABSENT_BUCKET: &str = "imagebuilder-components-missing";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str = concat!(
    "name: HelloWorld\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: Greet\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hello\n",
);

/// One emulator: imagebuilder and Amazon S3 on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// Status and parsed body of one imagebuilder call, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
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

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request rather
/// than a modelled call.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged documents below have a bucket to live
/// in.
async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "case \"the document bucket\": expected it to be created, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `document` as the object's own body.
async fn given_object(router: &MultiServiceRouter, key: &str, document: &str) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), document.as_bytes()).await;
    assert_eq!(
        status, 200,
        "case {key:?}: expected the document object to be stored, got {status}"
    );
}

/// The `uri` naming the object `key` in [`BUCKET`].
fn uri_of(key: &str) -> String {
    format!("s3://{BUCKET}/{key}")
}

/// A `CreateComponent` body: the required members, `clientToken`, and whatever else `extra`
/// names. `extra` carrying `data` or `uri` replaces the inline document.
fn component_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(client_token));
    if let Value::Object(extra) = &extra {
        if !extra.contains_key("data") && !extra.contains_key("uri") {
            body.insert("data".to_string(), json!(BUILD_DOCUMENT));
        }
        for (name, value) in extra {
            body.insert(name.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// `CreateComponent` with `body`.
async fn create_component(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateComponent", Some(body)).await
}

/// `GetComponent` for `arn`.
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `DeleteComponent` for `arn`.
async fn delete_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/DeleteComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `CreateImageRecipe` with `body`.
async fn create_recipe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateImageRecipe", Some(body)).await
}

/// `GetImageRecipe` for `arn`.
async fn get_recipe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={arn}"),
        None,
    )
    .await
}

/// A `CreateImageRecipe` body naming `components`.
fn recipe_body(name: &str, version: &str, client_token: &str, components: Value) -> Value {
    json!({
        "name": name,
        "semanticVersion": version,
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": client_token,
        "components": components,
    })
}

/// The build version ARN of the `build`th build of one component name+version pair.
fn component_arn(name: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/{name}/{version}/{build}")
}

/// The ARN of one image recipe name+version pair.
fn recipe_arn(name: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/{name}/{version}")
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the status, the error code and the message of a refusal.
fn assert_refused(
    case: &str,
    status: u16,
    body: &Value,
    expect_status: u16,
    expect_code: &str,
    expect_message: &str,
) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
    let message = text(case, body, "message");
    assert_eq!(
        message, expect_message,
        "case {case:?}: expected the message {expect_message:?}, got {message:?}"
    );
}

/// The message a value of `member` this service cannot accept is refused with.
fn parameter_value_message(member: &str, requirement: &str) -> String {
    format!("The value supplied for parameter '{member}' is not valid. {requirement}")
}

/// The component the read of `arn` reports, asserted to be a successful read.
async fn reported_component(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = get_component(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read, got {status} with {body}"
    );
    body.get("component")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a component in {body}"))
}

/// One document staged in the object store and the component it creates.
struct StagedRow {
    /// What makes this row different from the others.
    case: &'static str,
    /// The key the document is staged under.
    key: &'static str,
    /// The document itself.
    document: String,
    /// The component TYPE the document declares.
    component_type: &'static str,
    /// The parameter names the document declares, in document order.
    parameters: Vec<&'static str>,
}

/// The documents a `uri` names, spanning the document shape and the shape of the key.
fn staged_rows() -> Vec<StagedRow> {
    vec![
        StagedRow {
            case: "a build document under a bare key",
            key: "build.yaml",
            document: BUILD_DOCUMENT.to_string(),
            component_type: "BUILD",
            parameters: Vec::new(),
        },
        StagedRow {
            case: "a lone test phase, which makes the component a TEST one",
            key: "team/qa/check.yaml",
            document: concat!(
                "name: CheckWorld\n",
                "schemaVersion: 1.0\n",
                "phases:\n",
                "  - name: test\n",
                "    steps:\n",
                "      - name: Check\n",
                "        action: ExecuteBash\n",
                "        inputs:\n",
                "          commands:\n",
                "            - echo ok\n",
            )
            .to_string(),
            component_type: "TEST",
            parameters: Vec::new(),
        },
        StagedRow {
            case: "a document declaring parameters",
            key: "nested/prefix/parameterised.yaml",
            document: concat!(
                "name: Parameterised\n",
                "schemaVersion: 1.0\n",
                "parameters:\n",
                "  - Size:\n",
                "      type: string\n",
                "      default: small\n",
                "  - Hosts:\n",
                "      type: stringList\n",
                "phases:\n",
                "  - name: build\n",
                "    steps:\n",
                "      - name: Greet\n",
                "        action: ExecuteBash\n",
                "        inputs:\n",
                "          commands:\n",
                "            - echo hello\n",
            )
            .to_string(),
            component_type: "BUILD",
            parameters: vec!["Size", "Hosts"],
        },
        StagedRow {
            case: "a document carrying both phases and non-ASCII text",
            key: "accentué.yaml",
            document: concat!(
                "name: Accentué\n",
                "schemaVersion: 1.0\n",
                "phases:\n",
                "  - name: build\n",
                "    steps:\n",
                "      - name: Café\n",
                "        action: ExecuteBash\n",
                "        inputs:\n",
                "          commands:\n",
                "            - echo café ☕\n",
            )
            .to_string(),
            component_type: "BUILD",
            parameters: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_uri_named_document_is_fetched_and_stored_as_the_components_own() {
    let router = emulator();
    given_bucket(&router).await;

    for (index, row) in staged_rows().into_iter().enumerate() {
        given_object(&router, row.key, &row.document).await;

        let name = format!("staged-component-{index}");
        let body = component_body(
            &name,
            "1.0.0",
            &format!("utok-{index:04}"),
            json!({"uri": uri_of(row.key)}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the create, got {status} with {created}",
            row.case
        );
        let arn = text(row.case, &created, "componentBuildVersionArn");
        let expected_arn = component_arn(&name, "1.0.0", 1);
        assert_eq!(
            arn, expected_arn,
            "case {:?}: expected the ARN {expected_arn:?}, got {arn:?}",
            row.case
        );

        // The fetched bytes ARE the component's document, byte for byte, and everything
        // the document declares is read out of them.
        let component = reported_component(&router, row.case, &arn).await;
        let data = component.get("data");
        assert_eq!(
            data,
            Some(&json!(row.document)),
            "case {:?}: expected the fetched document stored verbatim, got {data:?}",
            row.case
        );
        let kind = component.get("type");
        assert_eq!(
            kind,
            Some(&json!(row.component_type)),
            "case {:?}: expected the type {:?} derived from the fetched document, got {kind:?}",
            row.case,
            row.component_type
        );
        let declared: Vec<String> = component
            .get("parameters")
            .and_then(Value::as_array)
            .map(|parameters| {
                parameters
                    .iter()
                    .filter_map(|parameter| parameter.get("name"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            declared, row.parameters,
            "case {:?}: expected the declared parameters {:?}, got {declared:?}",
            row.case, row.parameters
        );

        // The fetch is a ONE-TIME SNAPSHOT: overwriting the object cannot change what the
        // build version that was created from it reports.
        given_object(&router, row.key, "name: Replaced\nphases:\n  - name: build\n    steps: []\n")
            .await;
        let after = reported_component(&router, row.case, &arn).await;
        let data = after.get("data");
        assert_eq!(
            data,
            Some(&json!(row.document)),
            "case {:?}: expected the snapshot to survive an overwrite of the object, got \
             {data:?}",
            row.case
        );
    }
}

/// RULE: "if the content is also identical to the latest build version, the request fails
/// because the component already exists" — the content is the resolved DOCUMENT and the other
/// creation members, so which member named the document does not change what "identical"
/// means, and a `uri` whose object was overwritten names new content.
#[tokio::test]
async fn identical_content_collides_whichever_member_named_the_document() {
    let router = emulator();
    given_bucket(&router).await;
    let key = "content/build.yaml";
    given_object(&router, key, BUILD_DOCUMENT).await;

    let name = "content-component";
    let body = component_body(name, "1.0.0", "ktok-0001", json!({"uri": uri_of(key)}));
    let (status, created) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case \"the first build from the uri\": expected HTTP 200, got {status} with {created}"
    );
    let first = text("the first build from the uri", &created, "componentBuildVersionArn");

    // The rows differ along WHICH member carries the same content a second time.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the same uri again, the object unchanged",
            component_body(name, "1.0.0", "ktok-0002", json!({"uri": uri_of(key)})),
        ),
        (
            "the very bytes the object holds, carried inline",
            component_body(name, "1.0.0", "ktok-0003", json!({"data": BUILD_DOCUMENT})),
        ),
    ];
    for (case, body) in rows {
        let (status, refused) = create_component(&router, &body).await;
        assert_refused(
            case,
            status,
            &refused,
            400,
            "ResourceAlreadyExistsException",
            &format!("The following resource 'Component' already exists: '{first}'."),
        );
    }

    // Overwriting the object makes the same uri name DIFFERENT content, which is the next
    // build version rather than a collision.
    let replaced = concat!(
        "name: Replaced\n",
        "schemaVersion: 1.0\n",
        "phases:\n",
        "  - name: build\n",
        "    steps:\n",
        "      - name: Greet\n",
        "        action: ExecuteBash\n",
        "        inputs:\n",
        "          commands:\n",
        "            - echo goodbye\n",
    );
    given_object(&router, key, replaced).await;
    let case = "the same uri after the object was overwritten";
    let body = component_body(name, "1.0.0", "ktok-0004", json!({"uri": uri_of(key)}));
    let (status, created) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {created}"
    );
    let second = text(case, &created, "componentBuildVersionArn");
    let expected = component_arn(name, "1.0.0", 2);
    assert_eq!(
        second, expected,
        "case {case:?}: expected the next build version {expected:?}, got {second:?}"
    );
    let component = reported_component(&router, case, &second).await;
    let data = component.get("data");
    assert_eq!(
        data,
        Some(&json!(replaced)),
        "case {case:?}: expected the new build to hold the overwritten document, got {data:?}"
    );
    // And the first build still holds what it was created from.
    let first_read = reported_component(&router, "the first build", &first).await;
    let data = first_read.get("data");
    assert_eq!(
        data,
        Some(&json!(BUILD_DOCUMENT)),
        "case \"the first build\": expected its own snapshot, got {data:?}"
    );

    // And a RETRY of that create, carrying the same clientToken, answers the original
    // response without reading the object again — even once the object it was created from is
    // gone, since a retry runs nothing.
    let case = "the same token retried after the object was removed";
    let removed = s3(&router, "DELETE", &format!("/{BUCKET}/{key}"), &[]).await;
    assert!(
        (200..300).contains(&removed),
        "case {case:?}: expected the object to be removed, got {removed}"
    );
    let body = component_body(name, "1.0.0", "ktok-0004", json!({"uri": uri_of(key)}));
    let (status, retried) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the retry, got {status} with {retried}"
    );
    let answered = text(case, &retried, "componentBuildVersionArn");
    assert_eq!(
        answered, second,
        "case {case:?}: expected the original response naming {second:?}, got {answered:?}"
    );
}

#[tokio::test]
async fn a_uri_the_object_store_serves_nothing_at_is_refused_and_mints_nothing() {
    let router = emulator();
    given_bucket(&router).await;
    given_object(&router, "staged/present.yaml", BUILD_DOCUMENT).await;

    // The rows differ along WHICH part of the address resolves to nothing.
    let rows: Vec<(&str, String)> = vec![
        (
            "a bucket nothing created",
            format!("s3://{ABSENT_BUCKET}/staged/present.yaml"),
        ),
        ("a key nothing was staged under", uri_of("staged/absent.yaml")),
        (
            "a key that only prefixes a stored one",
            uri_of("staged/present"),
        ),
        (
            "a key that only extends a stored one",
            uri_of("staged/present.yaml.bak"),
        ),
        ("a key at the root of the bucket", uri_of("present.yaml")),
    ];

    for (index, (case, uri)) in rows.into_iter().enumerate() {
        let name = format!("absent-object-{index}");
        let body = component_body(
            &name,
            "1.0.0",
            &format!("atok-{index:04}"),
            json!({"uri": uri}),
        );
        let (status, refused) = create_component(&router, &body).await;
        assert_refused(
            case,
            status,
            &refused,
            400,
            "InvalidParameterValueException",
            &parameter_value_message("uri", "Not Found"),
        );

        // Nothing was minted: the ARN the create would have taken names nothing, and the
        // name is still free for the FIRST build version.
        let (status, missing) = get_component(&router, &component_arn(&name, "1.0.0", 1)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to mint nothing, got {status} with \
             {missing}"
        );
        let body = component_body(
            &name,
            "1.0.0",
            &format!("atok-recover-{index:04}"),
            json!({"uri": uri_of("staged/present.yaml")}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the name to still be free, got {status} with {created}"
        );
        let arn = text(case, &created, "componentBuildVersionArn");
        let expected = component_arn(&name, "1.0.0", 1);
        assert_eq!(
            arn, expected,
            "case {case:?}: expected the first build version {expected:?}, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn a_document_declaring_no_phases_is_refused_naming_the_member_that_carried_it() {
    let router = emulator();
    given_bucket(&router).await;

    // The rows differ along the member that carried the document and the way the document
    // declares no phases.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an inline document with no phases key", "data", "name: x\nschemaVersion: 1.0\n"),
        ("an inline document that is nothing but a name", "data", "name: x\n"),
        ("an inline document with an empty phases list", "data", "name: x\nphases: []\n"),
        ("an inline document with a valueless phases key", "data", "name: x\nphases:\n"),
        (
            "an inline document whose phases is a mapping",
            "data",
            "name: x\nphases:\n  build:\n    steps: []\n",
        ),
        (
            "a staged document with no phases key",
            "uri",
            "name: staged\nschemaVersion: 1.0\n",
        ),
        (
            "a staged document with an empty phases list",
            "uri",
            "name: staged\nphases: []\n",
        ),
    ];

    for (index, (case, member, document)) in rows.into_iter().enumerate() {
        let name = format!("no-phases-{index}");
        let extra = if member == "data" {
            json!({"data": document})
        } else {
            let key = format!("no-phases-{index}.yaml");
            given_object(&router, &key, document).await;
            json!({"uri": uri_of(&key)})
        };
        let body = component_body(&name, "1.0.0", &format!("ptok-{index:04}"), extra);
        let (status, refused) = create_component(&router, &body).await;
        assert_refused(
            case,
            status,
            &refused,
            400,
            "InvalidParameterValueException",
            &parameter_value_message(member, "Missing phases."),
        );

        // Nothing was minted, so the same name with a document that DOES declare phases
        // receives the first build version.
        let body = component_body(
            &name,
            "1.0.0",
            &format!("ptok-recover-{index:04}"),
            json!({"data": BUILD_DOCUMENT}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the name to still be free, got {status} with {created}"
        );
        let arn = text(case, &created, "componentBuildVersionArn");
        let expected = component_arn(&name, "1.0.0", 1);
        assert_eq!(
            arn, expected,
            "case {case:?}: expected the first build version {expected:?}, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_resource_already_exists_names_the_kind_and_the_existing_arn() {
    let router = emulator();

    // One component to collide on, and one recipe over it.
    let body = component_body("collision-component", "1.0.0", "ctok-0001", json!({}));
    let (status, created) = create_component(&router, &body).await;
    assert_eq!(
        status, 200,
        "case \"the component to collide on\": expected HTTP 200, got {status} with {created}"
    );
    let component = text("the component to collide on", &created, "componentBuildVersionArn");

    let recipe = recipe_body(
        "collision-recipe",
        "1.0.0",
        "rtok-0001",
        json!([{"componentArn": component.clone()}]),
    );
    let (status, answered) = create_recipe(&router, &recipe).await;
    assert_eq!(
        status, 200,
        "case \"the recipe to collide on\": expected HTTP 200, got {status} with {answered}"
    );

    // The rows differ along the KIND of resource the create collides on: a component build
    // version whose content repeats the latest one, and a recipe name+version pair that is
    // already taken.
    let component_repeat =
        component_body("collision-component", "1.0.0", "ctok-0002", json!({}));
    let recipe_repeat = recipe_body(
        "collision-recipe",
        "1.0.0",
        "rtok-0002",
        json!([{"componentArn": component.clone()}]),
    );
    let rows: Vec<(&str, bool, Value, String)> = vec![
        (
            "a component whose content repeats the latest build version",
            true,
            component_repeat,
            component_arn("collision-component", "1.0.0", 1),
        ),
        (
            "an image recipe name and version that is already taken",
            false,
            recipe_repeat,
            recipe_arn("collision-recipe", "1.0.0"),
        ),
    ];

    for (case, is_component, body, existing) in rows {
        let (status, refused) = if is_component {
            create_component(&router, &body).await
        } else {
            create_recipe(&router, &body).await
        };
        let kind = if is_component {
            "Component"
        } else {
            "ImageRecipe"
        };
        assert_refused(
            case,
            status,
            &refused,
            400,
            "ResourceAlreadyExistsException",
            &format!("The following resource '{kind}' already exists: '{existing}'."),
        );
    }

    // The refused creates left the existing resources exactly as they were, and another
    // version of each name is still accepted.
    let component_read = reported_component(&router, "the existing component", &component).await;
    let data = component_read.get("data");
    assert_eq!(
        data,
        Some(&json!(BUILD_DOCUMENT)),
        "case \"the existing component\": expected its document untouched, got {data:?}"
    );
    let (status, read) = get_recipe(&router, &recipe_arn("collision-recipe", "1.0.0")).await;
    assert_eq!(
        status, 200,
        "case \"the existing recipe\": expected HTTP 200, got {status} with {read}"
    );
    let components = read
        .get("imageRecipe")
        .and_then(|recipe| recipe.get("components"))
        .cloned();
    assert_eq!(
        components,
        Some(json!([{"componentArn": component.clone()}])),
        "case \"the existing recipe\": expected its components untouched, got {components:?}"
    );

    let next = recipe_body(
        "collision-recipe",
        "1.0.1",
        "rtok-0003",
        json!([{"componentArn": component.clone()}]),
    );
    let (status, answered) = create_recipe(&router, &next).await;
    assert_eq!(
        status, 200,
        "case \"another version of the taken name\": expected HTTP 200, got {status} with \
         {answered}"
    );
    let arn = text("another version of the taken name", &answered, "imageRecipeArn");
    let expected = recipe_arn("collision-recipe", "1.0.1");
    assert_eq!(
        arn, expected,
        "case \"another version of the taken name\": expected {expected:?}, got {arn:?}"
    );
}

#[tokio::test]
async fn a_delete_a_stored_reference_blocks_states_the_resource_arn() {
    let router = emulator();

    // Two build versions of one component name, differing in content, and a recipe over
    // the first: the second is a sibling nothing references.
    let mut builds = Vec::new();
    for (index, change) in ["first", "second"].into_iter().enumerate() {
        let body = component_body(
            "referenced-component",
            "1.0.0",
            &format!("dtok-{index:04}"),
            json!({"changeDescription": change}),
        );
        let (status, created) = create_component(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {change:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        builds.push(text(change, &created, "componentBuildVersionArn"));
    }
    let referenced = builds.first().cloned().expect("the referenced build");
    let sibling = builds.get(1).cloned().expect("the sibling build");

    let recipe = recipe_body(
        "dependent-recipe",
        "1.0.0",
        "dtok-recipe",
        json!([{"componentArn": referenced.clone()}]),
    );
    let (status, answered) = create_recipe(&router, &recipe).await;
    assert_eq!(
        status, 200,
        "case \"the dependent recipe\": expected HTTP 200, got {status} with {answered}"
    );

    // The build a recipe includes is not removed, and the refusal names the ARN the
    // request asked to delete.
    let case = "a build version a stored recipe includes";
    let (status, refused) = delete_component(&router, &referenced).await;
    assert_refused(
        case,
        status,
        &refused,
        400,
        "ResourceDependencyException",
        &format!(
            "Resource dependency error: The resource ARN '{referenced}' has other resources \
             depended on it."
        ),
    );
    let still_there = reported_component(&router, case, &referenced).await;
    let arn = still_there.get("arn");
    assert_eq!(
        arn,
        Some(&json!(referenced)),
        "case {case:?}: expected the refusal to leave the build version stored, got {arn:?}"
    );

    // The sibling nothing references deletes, and the read that follows reports it as a
    // missing required resource.
    let case = "an unreferenced sibling build version";
    let (status, deleted) = delete_component(&router, &sibling).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the delete, got {status} with {deleted}"
    );
    let removed = text(case, &deleted, "componentBuildVersionArn");
    assert_eq!(
        removed, sibling,
        "case {case:?}: expected the delete to name {sibling:?}, got {removed:?}"
    );
    let (status, missing) = get_component(&router, &sibling).await;
    assert_refused(
        case,
        status,
        &missing,
        404,
        "ResourceNotFoundException",
        &format!("The following required resource 'Component' cannot be found: '{sibling}'."),
    );
}
