// SPDX-License-Identifier: CC0-1.0

//! `UpdatePackagingGroup` end to end, as a client sees it over restJson1.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded call:
//!
//! * `PUT /packaging_groups/{Id}` REPLACES the addressed group's CDN `Authorization`
//!   with the one the request body carries — and with none when the body carries
//!   none, because the body is the group's whole mutable configuration — then answers
//!   with the record as it now stands, so a later `DescribePackagingGroup` reports
//!   the identical `Authorization`;
//! * it changes nothing else: `Id`, `Arn`, `CreatedAt` and `DomainName` are
//!   system-generated, `Tags` and `EgressAccessLogs` belong to other operations, and
//!   `ApproximateAssetCount` stays derived from the asset map;
//! * it addresses ONE group: the other stored groups keep their own authorization;
//! * an `Id` this service holds no group for is answered 404 `NotFoundException`
//!   carrying the request-derived not-found message, and stores nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; it appears in every ARN this service mints.
const REGION: &str = "us-west-2";

/// A router carrying exactly this service over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
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

/// Send one request and return its status and parsed JSON body (`Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
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
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// CDN authorization blocks that differ in value, length and form.
///
/// Both members come from the model: `smithy_query(query="shape",
/// shape="Authorization")` gives `CdnIdentifierSecret` and `SecretsRoleArn`, both
/// required plain `__string`s with no length or pattern constraint — so the rule
/// generalises over the string, which is what these rows vary.
fn authorizations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "secrets manager arn + iam role arn",
            json!({
                "cdnIdentifierSecret":
                    "arn:aws:secretsmanager:us-west-2:111122223333:secret:cdn-identifier-1a2b3c",
                "secretsRoleArn": "arn:aws:iam::111122223333:role/MediaPackageVodCdnAuth",
            }),
        ),
        (
            "another account, another region",
            json!({
                "cdnIdentifierSecret":
                    "arn:aws:secretsmanager:eu-central-1:444455556666:secret:other-secret-9z8y7x",
                "secretsRoleArn": "arn:aws:iam::444455556666:role/service-role/VodCdn",
            }),
        ),
        (
            "slashed secret name and a role path",
            json!({
                "cdnIdentifierSecret":
                    "arn:aws:secretsmanager:ap-southeast-2:777788889999:secret:team/cdn/auth-0",
                "secretsRoleArn": "arn:aws:iam::777788889999:role/path/to/Role_Name-1",
            }),
        ),
        (
            "one-character values",
            json!({ "cdnIdentifierSecret": "a", "secretsRoleArn": "b" }),
        ),
        (
            "long values",
            json!({
                "cdnIdentifierSecret": format!("arn:aws:secretsmanager:us-west-2:111122223333:secret:{}", "s".repeat(200)),
                "secretsRoleArn": format!("arn:aws:iam::111122223333:role/{}", "r".repeat(200)),
            }),
        ),
    ]
}

/// Create a packaging group carrying `tags`, asserting the setup succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str, tags: Value) -> Value {
    let (status, group) = call(
        router,
        "POST",
        "/packaging_groups",
        Some(json!({ "id": id, "tags": tags })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreatePackagingGroup {id:?} should succeed, got {status} body {group}"
    );
    group
}

/// Create an asset in `group_id`, asserting the setup succeeded.
async fn create_asset(router: &MultiServiceRouter, id: &str, group_id: &str) {
    let body = json!({
        "id": id,
        "packagingGroupId": group_id,
        "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
        "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVod", engine::emulator_account_id()),
    });
    let (status, asset) = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(
        status, 200,
        "setup: CreateAsset {id:?} should succeed, got {status} body {asset}"
    );
}

/// `UpdatePackagingGroup` for `id`, carrying `authorization` when it is `Some`.
async fn update_group(
    router: &MultiServiceRouter,
    id: &str,
    authorization: Option<&Value>,
) -> (u16, Value) {
    let body = match authorization {
        Some(auth) => json!({ "authorization": auth.clone() }),
        None => json!({}),
    };
    call(
        router,
        "PUT",
        &format!("/packaging_groups/{id}"),
        Some(body),
    )
    .await
}

/// The `authorization` member of a response body, or `Null` when it carries none —
/// which is how a group with no CDN authorization reports itself.
fn authorization_of(body: &Value) -> Value {
    body.get("authorization").cloned().unwrap_or(Value::Null)
}

/// The members no update may change, as one comparable value.
fn immutable_members(body: &Value) -> Value {
    json!({
        "id": body.get("id").cloned().unwrap_or(Value::Null),
        "arn": body.get("arn").cloned().unwrap_or(Value::Null),
        "createdAt": body.get("createdAt").cloned().unwrap_or(Value::Null),
        "domainName": body.get("domainName").cloned().unwrap_or(Value::Null),
        "tags": body.get("tags").cloned().unwrap_or(Value::Null),
        "egressAccessLogs": body.get("egressAccessLogs").cloned().unwrap_or(Value::Null),
    })
}

/// An update replaces the group's authorization, persists it, and leaves every other
/// member of the group exactly as it was.
#[tokio::test]
async fn an_update_replaces_the_authorization_and_the_read_back_group_reports_it() {
    // Group ids differing in length, separators, case and digits: the rule is about
    // the addressed group, not about how its id is spelled.
    let group_ids = ["g", "group-with-dashes-1", "Mixed_Case.Group", &"z".repeat(64)];

    for group_id in group_ids {
        let router = router();
        let created = create_group(&router, group_id, json!({ "team": "video" })).await;
        // Two assets, so `ApproximateAssetCount` is a value an update could plausibly
        // disturb rather than always zero.
        create_asset(&router, &format!("{group_id}-asset-a"), group_id).await;
        create_asset(&router, &format!("{group_id}-asset-b"), group_id).await;
        let baseline = immutable_members(&created);

        // Each authorization is set, then cleared by a body that carries none, so the
        // sequence covers set-from-none, replace-a-different-one and remove. The first
        // row is applied twice in a row to cover re-applying the same value.
        let mut sequence: Vec<(String, Option<Value>)> = Vec::new();
        let rows = authorizations();
        sequence.push((format!("set {} twice (1)", rows[0].0), Some(rows[0].1.clone())));
        sequence.push((format!("set {} twice (2)", rows[0].0), Some(rows[0].1.clone())));
        for (name, auth) in &rows {
            sequence.push((format!("replace with {name}"), Some(auth.clone())));
            sequence.push((format!("clear after {name}"), None));
        }

        for (case, authorization) in sequence {
            let case = format!("{group_id:?}: {case}");
            let expected = authorization.clone().unwrap_or(Value::Null);

            let (status, updated) = update_group(&router, group_id, authorization.as_ref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} body {updated}"
            );
            assert_eq!(
                authorization_of(&updated),
                expected,
                "case {case:?}: expected the update to answer with authorization {expected}, got {}",
                authorization_of(&updated)
            );
            assert_eq!(
                immutable_members(&updated),
                baseline,
                "case {case:?}: expected the update to leave {baseline} unchanged, got {}",
                immutable_members(&updated)
            );
            assert_eq!(
                updated.get("approximateAssetCount").cloned(),
                Some(json!(2)),
                "case {case:?}: expected approximateAssetCount 2 (derived from the asset map), got {:?}",
                updated.get("approximateAssetCount")
            );

            // Persisted, not echoed: the describe path reads the same stored record.
            let (status, described) =
                call(&router, "GET", &format!("/packaging_groups/{group_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribePackagingGroup after the update, got {status} body {described}"
            );
            assert_eq!(
                authorization_of(&described),
                expected,
                "case {case:?}: expected the stored group to report authorization {expected}, got {}",
                authorization_of(&described)
            );
            assert_eq!(
                immutable_members(&described),
                baseline,
                "case {case:?}: expected the stored group to keep {baseline}, got {}",
                immutable_members(&described)
            );
        }
    }
}

/// An update addresses exactly one group: every other stored group keeps its own
/// authorization.
#[tokio::test]
async fn an_update_touches_only_the_addressed_group() {
    let router = router();
    let rows = authorizations();
    // One group per authorization row, each given its own value up front.
    let groups: Vec<String> = (0..rows.len()).map(|i| format!("group-{i}")).collect();
    for (group_id, (name, auth)) in groups.iter().zip(rows.iter()) {
        create_group(&router, group_id, json!({})).await;
        let (status, body) = update_group(&router, group_id, Some(auth)).await;
        assert_eq!(
            status, 200,
            "setup: {group_id:?} set to {name}, got {status} body {body}"
        );
    }

    // Re-point each group in turn at the LAST row's value, and after each single
    // update check every group reports what it should.
    let target = rows.last().expect("the table has rows").1.clone();
    let mut expected: Vec<Value> = rows.iter().map(|(_, auth)| auth.clone()).collect();
    for (index, group_id) in groups.iter().enumerate() {
        let (status, body) = update_group(&router, group_id, Some(&target)).await;
        assert_eq!(
            status, 200,
            "case {group_id:?}: expected HTTP 200, got {status} body {body}"
        );
        expected[index] = target.clone();

        for (other, want) in groups.iter().zip(expected.iter()) {
            let (status, described) =
                call(&router, "GET", &format!("/packaging_groups/{other}"), None).await;
            assert_eq!(
                status, 200,
                "case {group_id:?}: describing {other:?}, got {status} body {described}"
            );
            assert_eq!(
                authorization_of(&described),
                *want,
                "case {group_id:?}: after updating it, {other:?} must report {want}, got {}",
                authorization_of(&described)
            );
        }
    }
}

/// Updating an id this service holds no group for is a missing addressed resource,
/// and creates nothing.
#[tokio::test]
async fn updating_an_unknown_group_is_refused_as_not_found() {
    let router = router();
    create_group(&router, "stored-group", json!({})).await;

    // Ids differing in length, separators, case and digits; none is stored.
    let missing = ["nope", "a", "Group_42", "dotted.id-0123456789", &"x".repeat(100)];
    let rows = authorizations();

    for id in missing {
        // One row with an authorization and one without: the refusal is about the
        // addressed resource, not about the body.
        for (name, authorization) in [
            ("with authorization", Some(rows[0].1.clone())),
            ("without authorization", None),
        ] {
            let case = format!("{id:?} {name}");
            let (status, body) = update_group(&router, id, authorization.as_ref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} body {body}"
            );
            assert_eq!(
                body.get("__type").and_then(Value::as_str),
                Some("NotFoundException"),
                "case {case:?}: expected __type NotFoundException, got {body}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(format!("PackagingGroup with id={id} not found").as_str()),
                "case {case:?}: expected the request-derived not-found message, got {body}"
            );

            // The refusal stored nothing: the id still resolves to no group.
            let (status, described) =
                call(&router, "GET", &format!("/packaging_groups/{id}"), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the refused update must not create the group, got {status} body {described}"
            );
        }
    }

    // Not a blanket answer: the group that IS stored accepts the same body.
    let (status, body) = update_group(&router, "stored-group", Some(&rows[0].1)).await;
    assert_eq!(
        status, 200,
        "a stored group must accept the same update, got {status} body {body}"
    );
}
