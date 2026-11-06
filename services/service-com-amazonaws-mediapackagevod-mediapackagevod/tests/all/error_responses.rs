// SPDX-License-Identifier: CC0-1.0

//! The shape of a MediaPackage VOD client-error RESPONSE.
//!
//! Every case drives real HTTP through the router the production binary builds, so
//! what is asserted is the status, the `x-amzn-errortype` header and the JSON body a
//! `mediapackage-vod` client parses — never a handler called directly. Each test
//! asserts a RULE over a TABLE of operations and values; the ids and the recorded
//! request are one row each.
//!
//! The rules under test:
//!
//! * an identifier carried in a create request's BODY that resolves to nothing is a
//!   violation of that PARAMETER — HTTP 422 `UnprocessableEntityException` — not the
//!   404 `NotFoundException` that belongs to the resource a request ADDRESSES;
//! * a validation refusal attributes itself: the body carries an `errors` map from
//!   the offending input member's wire name — or `base` when the violation is about
//!   the request as a whole — to the constraint phrases violated, and `message`
//!   states the same attribution as `<member_name_in_snake_case> <phrase>` for a
//!   member and `<ResourceType> <phrase>` for `base`;
//! * a message about an identified resource binds the id the REQUEST asked for in
//!   one fixed form, with no trailing period: `<ResourceType> with id=<id> not found`
//!   for a resource this service holds none of, `<ResourceType> with id=<id> already
//!   exists` for one it already holds;
//! * neither of those two is a validation refusal, so neither carries `errors`;
//! * none of this is a blanket answer: the same operations accept the inputs the
//!   rules admit.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

/// The region the test signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The wire error code of a parameter-validation refusal, and its modelled
/// `httpError` (`smithy_query(query="shape",shape="UnprocessableEntityException")`:
/// `errorKind client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// The wire error code of a missing addressed resource, and its modelled `httpError`
/// (`smithy_query(query="shape",shape="NotFoundException")`: `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// What one answer says about itself: the HTTP status, the error shape named by
/// `x-amzn-errortype` (how restJson1 resolves an error shape), and the parsed body.
///
/// The crate's other suite (`packaging_lifecycle`) reads the status and body only;
/// this suite needs the header too, which is why it carries its own harness.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The `message` member the modelled error shapes spell `Message` with
    /// `jsonName: message`, or `"<unset>"` when the body carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The per-attribute `errors` map, or `Value::Null` when the body carries none —
    /// the distinction between a validation refusal and a resource-level one.
    fn errors(&self) -> Value {
        self.body.get("errors").cloned().unwrap_or(Value::Null)
    }

    /// The error shape the body itself names (restJson1's `__type`), which must
    /// agree with the header.
    fn json_type(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }
}

/// A router carrying exactly this service over one shared engine.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and report what it answered.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// A package block of each modelled packaging format, so a case can vary the format
/// without varying anything else (`CreatePackagingConfiguration` takes at most one of
/// `cmafPackage` / `dashPackage` / `hlsPackage` / `mssPackage`).
fn package_blocks() -> Vec<(&'static str, Value)> {
    vec![
        ("hlsPackage", json!({ "hlsPackage": { "hlsManifests": [{}] } })),
        (
            "dashPackage",
            json!({ "dashPackage": { "dashManifests": [{}] } }),
        ),
        (
            "cmafPackage",
            json!({ "cmafPackage": { "hlsManifests": [{}] } }),
        ),
        ("mssPackage", json!({ "mssPackage": { "mssManifests": [{}] } })),
    ]
}

/// A `CreatePackagingConfiguration` body for `id` in `group`, carrying the named
/// package blocks verbatim.
fn config_body(id: &str, group: &str, blocks: &[Value]) -> Value {
    let mut body = json!({ "id": id, "packagingGroupId": group });
    for block in blocks {
        let Value::Object(members) = block else {
            panic!("a package block must be a JSON object");
        };
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    body
}

/// A `CreateAsset` body for `id` in `group`.
fn asset_body(id: &str, group: &str) -> Value {
    json!({
        "id": id,
        "packagingGroupId": group,
        "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
        "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", engine::emulator_account_id()),
    })
}

/// Create a packaging group, asserting the setup succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str) {
    let answer = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreatePackagingGroup {id:?} should succeed, got {} body {}",
        answer.status, answer.body
    );
}

/// Assert the status, the `x-amzn-errortype` header and the body's `__type` of one
/// refusal, which a client resolves the shape from.
fn assert_shape(answer: &Answer, case: &str, (code, status): (&str, u16)) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.errortype, code,
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype
    );
    assert_eq!(
        answer.json_type(),
        code,
        "case {case:?}: the body's __type must name the same shape as the header, got {:?}",
        answer.json_type()
    );
}

/// An id in the request BODY that resolves to nothing is a violation of that
/// parameter — 422, attributed to the member — on every create that takes one.
#[tokio::test]
async fn an_unresolvable_id_in_a_create_body_is_refused_as_an_invalid_parameter() {
    let router = router();
    create_group(&router, "stored-group").await;

    // The group ids below are absent from the store; they differ in form (length,
    // separators, case, digits) because the rule is about the member, not the value.
    let missing_groups = [
        "nope",
        "missing-group-0123456789abcdef",
        "MixedCase_Group",
        "g",
        &"long-".repeat(20),
    ];

    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for group in missing_groups {
        cases.push((
            format!("CreateAsset into {group:?}"),
            "/assets",
            asset_body("new-asset", group),
        ));
        // One row per packaging format, so the refusal cannot depend on the block.
        for (format, block) in package_blocks() {
            cases.push((
                format!("CreatePackagingConfiguration {format} into {group:?}"),
                "/packaging_configurations",
                config_body("new-cfg", group, &[block]),
            ));
        }
    }

    for (case, path, body) in cases {
        let answer = call(&router, "POST", path, Some(body)).await;
        assert_shape(&answer, &case, UNPROCESSABLE);
        assert_eq!(
            answer.errors(),
            json!({ "packagingGroupId": ["does not exist"] }),
            "case {case:?}: the refusal must attribute itself to the offending member, got {}",
            answer.errors()
        );
        assert_eq!(
            answer.message(),
            "packaging_group_id does not exist",
            "case {case:?}: expected the member-and-constraint message, got {:?}",
            answer.message()
        );
    }

    // Not a blanket answer: the same bodies against the group that IS stored succeed.
    let answer = call(
        &router,
        "POST",
        "/assets",
        Some(asset_body("new-asset", "stored-group")),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a resolvable packagingGroupId must not be refused, got {} body {}",
        answer.status, answer.body
    );
}

/// A violation no single member is at fault for is attributed to `base`, and its
/// message names the resource type instead of a member.
#[tokio::test]
async fn a_whole_request_violation_is_attributed_to_base() {
    let router = router();
    create_group(&router, "g").await;

    let blocks = package_blocks();
    let mut cases: Vec<(String, Vec<Value>)> = vec![("no package block".to_string(), Vec::new())];
    // Every pair of distinct formats, then a triple and the whole set: the rule is
    // "one single type", so anything other than exactly one violates it.
    for (i, (left_name, left)) in blocks.iter().enumerate() {
        for (right_name, right) in blocks.iter().skip(i + 1) {
            cases.push((
                format!("{left_name} + {right_name}"),
                vec![left.clone(), right.clone()],
            ));
        }
    }
    cases.push((
        "three package blocks".to_string(),
        blocks.iter().take(3).map(|(_, b)| b.clone()).collect(),
    ));
    cases.push((
        "all four package blocks".to_string(),
        blocks.iter().map(|(_, b)| b.clone()).collect(),
    ));

    for (case, blocks) in cases {
        let answer = call(
            &router,
            "POST",
            "/packaging_configurations",
            Some(config_body("cfg", "g", &blocks)),
        )
        .await;
        assert_shape(&answer, &case, UNPROCESSABLE);
        assert_eq!(
            answer.errors(),
            json!({ "base": ["must include one single type of packaging settings"] }),
            "case {case:?}: a whole-request violation is keyed by `base`, got {}",
            answer.errors()
        );
        assert_eq!(
            answer.message(),
            "PackagingConfiguration must include one single type of packaging settings",
            "case {case:?}: expected the resource-and-constraint message, got {:?}",
            answer.message()
        );
    }

    // Exactly one block of any format is accepted, so the refusal is about the count.
    for (format, block) in package_blocks() {
        let answer = call(
            &router,
            "POST",
            "/packaging_configurations",
            Some(config_body(&format!("cfg-{format}"), "g", &[block])),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "case {format:?}: exactly one package block must be accepted, got {} body {}",
            answer.status, answer.body
        );
    }
}

/// An empty `Id` is a member-level violation on every create that takes one, worded
/// by the same rule as the unresolvable-member case.
#[tokio::test]
async fn an_empty_id_is_attributed_to_the_id_member() {
    let router = router();
    create_group(&router, "g").await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreatePackagingGroup",
            "/packaging_groups",
            json!({ "id": "" }),
        ),
        (
            "CreatePackagingConfiguration",
            "/packaging_configurations",
            config_body("", "g", &[json!({ "hlsPackage": { "hlsManifests": [{}] } })]),
        ),
        ("CreateAsset", "/assets", asset_body("", "g")),
    ];

    for (case, path, body) in cases {
        let answer = call(&router, "POST", path, Some(body)).await;
        assert_shape(&answer, case, UNPROCESSABLE);
        assert_eq!(
            answer.errors(),
            json!({ "id": ["must not be empty"] }),
            "case {case:?}: expected the violation attributed to `id`, got {}",
            answer.errors()
        );
        assert_eq!(
            answer.message(),
            "id must not be empty",
            "case {case:?}: expected the member-and-constraint message, got {:?}",
            answer.message()
        );
    }
}

/// A cursor this service never issued is attributed to the member it arrived in, and
/// worded by the fixed sentence this refusal class carries.
#[tokio::test]
async fn a_forged_pagination_cursor_is_attributed_to_its_member() {
    let router = router();
    // (case, the token as it appears in the query string)
    let forged = [
        ("not hex", "zzzzzzzzzz"),
        ("too short", "abcd"),
        ("checksum does not match the id", "00000000616263"),
        ("empty", ""),
    ];
    for (case, token) in forged {
        let answer = call(
            &router,
            "GET",
            &format!("/assets?nextToken={token}"),
            None,
        )
        .await;
        assert_shape(&answer, case, UNPROCESSABLE);
        assert_eq!(
            answer.errors(),
            json!({ "nextToken": ["is not a cursor this service issued"] }),
            "case {case:?}: expected the violation attributed to `nextToken`, got {}",
            answer.errors()
        );
        // The MESSAGE of this refusal class is a fixed sentence: it names no member
        // and echoes no token, unlike the composed per-member line above.
        assert_eq!(
            answer.message(),
            vod::INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected the fixed invalid-cursor message, got {:?}",
            answer.message()
        );
    }
}

/// Every read that addresses a resource by id words its 404 in the one request-derived
/// form, for any id — and carries no `errors` map, because nothing was invalidated.
#[tokio::test]
async fn a_missing_resource_message_binds_the_requested_id() {
    let router = router();

    // Ids differing in length, separators, case and digits; none is stored.
    let ids = [
        "nope",
        "a",
        "Asset_42",
        "dotted.id-0123456789",
        &"x".repeat(100),
    ];
    // (case, method, path prefix, the resource type the message names)
    let readers = [
        ("DescribeAsset", "GET", "/assets", "Asset"),
        ("DeleteAsset", "DELETE", "/assets", "Asset"),
        (
            "DescribePackagingGroup",
            "GET",
            "/packaging_groups",
            "PackagingGroup",
        ),
    ];

    for id in ids {
        for (op, method, prefix, resource) in readers {
            let case = format!("{op} {id:?}");
            let answer = call(&router, method, &format!("{prefix}/{id}"), None).await;
            assert_shape(&answer, &case, NOT_FOUND);
            assert_eq!(
                answer.message(),
                format!("{resource} with id={id} not found"),
                "case {case:?}: expected the request-derived not-found form, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.errors(),
                Value::Null,
                "case {case:?}: a missing resource is not a member violation, got {}",
                answer.errors()
            );
        }
    }
}

/// A create that re-uses a stored id words its conflict in the one request-derived
/// form, on every create — and leaves the stored resource alone.
#[tokio::test]
async fn a_duplicate_id_conflict_message_binds_the_requested_id() {
    let router = router();

    // Ids differing in length, separators, case and digits.
    let ids = ["g", "group-1", "Mixed_Case-9", &"y".repeat(80)];
    for id in ids {
        // PackagingGroup first: the configuration and the asset need one.
        create_group(&router, id).await;
        let config = config_body(id, id, &[json!({ "hlsPackage": { "hlsManifests": [{}] } })]);
        let created = call(
            &router,
            "POST",
            "/packaging_configurations",
            Some(config.clone()),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "setup: CreatePackagingConfiguration {id:?}: body {}",
            created.body
        );
        let asset = asset_body(id, id);
        let created = call(&router, "POST", "/assets", Some(asset.clone())).await;
        assert_eq!(
            created.status, 200,
            "setup: CreateAsset {id:?}: body {}",
            created.body
        );

        // (case, path, body, the resource type the message names)
        let duplicates: Vec<(&str, &str, Value, &str)> = vec![
            (
                "CreatePackagingGroup",
                "/packaging_groups",
                json!({ "id": id }),
                "PackagingGroup",
            ),
            (
                "CreatePackagingConfiguration",
                "/packaging_configurations",
                config,
                "PackagingConfiguration",
            ),
            ("CreateAsset", "/assets", asset, "Asset"),
        ];
        for (op, path, body, resource) in duplicates {
            let case = format!("{op} {id:?} twice");
            let answer = call(&router, "POST", path, Some(body)).await;
            assert_shape(&answer, &case, UNPROCESSABLE);
            assert_eq!(
                answer.message(),
                format!("{resource} with id={id} already exists"),
                "case {case:?}: expected the request-derived conflict form, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.errors(),
                Value::Null,
                "case {case:?}: a conflict is not a member violation, got {}",
                answer.errors()
            );
        }

        // The refused creates changed nothing: the stored asset still reads back.
        let answer = call(&router, "GET", &format!("/assets/{id}"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {id:?}: the stored asset survived the refused creates, body {}",
            answer.body
        );
    }
}

/// Every message this service builds about an identified resource binds the id the
/// same way: a lower-case `id=` and no trailing period, whichever path answered.
#[tokio::test]
async fn every_resource_message_uses_one_binding_form() {
    let router = router();
    create_group(&router, "g").await;

    let answers = vec![
        ("DescribeAsset, absent", call(&router, "GET", "/assets/absent", None).await),
        (
            "DeleteAsset, absent",
            call(&router, "DELETE", "/assets/absent", None).await,
        ),
        (
            "DescribePackagingGroup, absent",
            call(&router, "GET", "/packaging_groups/absent", None).await,
        ),
        (
            "CreatePackagingGroup, duplicate",
            call(&router, "POST", "/packaging_groups", Some(json!({ "id": "g" }))).await,
        ),
    ];

    for (case, answer) in answers {
        let message = answer.message();
        assert!(
            message.contains(" with id=") && !message.contains(" with Id "),
            "case {case:?}: the id must be bound as ` with id=<id>`, got {message:?}"
        );
        assert!(
            !message.ends_with('.'),
            "case {case:?}: the message carries no trailing period, got {message:?}"
        );
    }
}
