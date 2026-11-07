// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for VERB SELECTION on one resource of a deployed stage: which of the
//! methods a resource carries serves a request, when the resource carries both an explicit
//! verb and the `ANY` catch-all.
//!
//! The RULE under test, one level above the recorded case:
//!
//! > A request to a deployed stage is served by the method the DEPLOYMENT holds for the
//! > request's own verb, and by the resource's `ANY` method only for a verb the deployment
//! > holds no method of its own for — so an explicit verb takes precedence over `ANY` for
//! > that verb alone and every other verb is served by `ANY`. Which methods the resource
//! > HAS is a separate question the control plane answers: `GetMethod` for a verb the
//! > resource carries no method for is `NotFoundException` (404) however the stage serves
//! > that verb, and a deployment made after the explicit method is deleted serves its verb
//! > from `ANY`.
//!
//! The rows span what that rule generalises over: the verb (two explicit ones, and every
//! other verb `PutMethod` admits, served by `ANY`), and the resource before and after the
//! explicit method is deleted and the deletion deployed.
//!
//! Every case drives the real wire path through the multi-service router on the api's own
//! invoke host, with the control plane driven through the same emulator ([`Gateway`]).

use serde_json::json;

use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};

/// The region the invoke host is addressed in; any region is served, so one is enough.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "beta";

/// The two segments the methods' resource is built from: a literal and a path parameter, so
/// the request path is matched by a template rather than by a literal — the shape the
/// recorded case uses.
const COLLECTION_SEGMENT: &str = "data-agreements";
const ITEM_SEGMENT: &str = "{agreementId}";

/// The verbs `PutMethod` admits besides the `ANY` catch-all
/// (`crate::rest_api_lifecycle`'s own list: `DELETE`, `GET`, `HEAD`, `OPTIONS`, `PATCH`,
/// `POST`, `PUT`).
const VERBS: &[&str] = &["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];

/// The two verbs this api carries an explicit method for; every other verb of [`VERBS`] is
/// served by `ANY`.
const EXPLICIT: &[&str] = &["GET", "DELETE"];

/// The body the `ANY` method's integration answers with.
const ANY_BODY: &str = r#"{"served":"any"}"#;

/// The body an explicit method's integration answers with, naming its own verb — which is
/// what tells the method that served a request from the catch-all that did not.
fn explicit_body(verb: &str) -> String {
    format!("{{\"served\":\"{verb}\"}}")
}

/// Issue one unsigned request of `verb` to `api`'s own invoke endpoint at `path`, answering
/// its status and its body verbatim.
async fn invoke(gateway: &Gateway, api: &str, verb: &str, path: &str) -> (u16, String) {
    invoke_with(gateway, api, verb, path, &[]).await
}

/// [`invoke`] carrying `headers` beside the `Host` every request needs.
async fn invoke_with(
    gateway: &Gateway,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", &host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    (status, body)
}

/// Put one `MOCK` method of `verb` on `resource`, answering `body` with status 200: the
/// method, its integration, its 200 method response and the integration response whose
/// template is that body.
async fn mock_method(gateway: &Gateway, api: &str, resource: &str, verb: &str, body: &str) {
    let base = format!("/restapis/{api}/resources/{resource}/methods/{verb}");
    let calls: Vec<(String, serde_json::Value)> = vec![
        (base.clone(), json!({ "authorizationType": "NONE" })),
        (
            format!("{base}/integration"),
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
        ),
        (format!("{base}/responses/200"), json!({})),
        (
            format!("{base}/integration/responses/200"),
            json!({ "responseTemplates": { "application/json": body } }),
        ),
    ];
    for (uri, request) in calls {
        let answer = gateway.call("PUT", &uri, Some(request)).await;
        assert_eq!(
            answer.status, 201,
            "setup {uri}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// `CreateDeployment` into [`STAGE`], asserting it was accepted.
async fn deploy(gateway: &Gateway, api: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "setup CreateDeployment({STAGE}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn an_explicit_verb_serves_its_own_requests_and_any_serves_every_other_verb() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "verb-precedence").await;
    let collection = resource_under(&gateway, &api, &root, COLLECTION_SEGMENT).await;
    let item = resource_under(&gateway, &api, &collection, ITEM_SEGMENT).await;

    for verb in EXPLICIT {
        mock_method(&gateway, &api, &item, verb, &explicit_body(verb)).await;
    }
    mock_method(&gateway, &api, &item, "ANY", ANY_BODY).await;
    deploy(&gateway, &api).await;

    // A path the resource's template matches; the value of the path parameter is the
    // caller's and changes nothing about which method serves the request.
    let path = format!("/{STAGE}/{COLLECTION_SEGMENT}/a-1");

    // Rows: every verb the resource could be reached by. An explicit one is served by its
    // own method, every other by `ANY`.
    for verb in VERBS {
        let expected = if EXPLICIT.contains(verb) {
            explicit_body(verb)
        } else {
            ANY_BODY.to_string()
        };
        let (status, body) = invoke(&gateway, &api, verb, &path).await;
        assert_eq!(
            (status, body.as_str()),
            (200, expected.as_str()),
            "case {verb:?}: expected the serving method's own body, got ({status}, {body:?})"
        );
    }

    // Which methods the RESOURCE has is the control plane's own answer, and it does not
    // follow from which verbs the stage serves: a verb served by `ANY` is a verb the
    // resource carries no method of its own for.
    for verb in VERBS {
        let answer = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources/{item}/methods/{verb}"),
                None,
            )
            .await;
        let expected = if EXPLICIT.contains(verb) {
            (200, None)
        } else {
            (404, Some("NotFoundException"))
        };
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            expected,
            "case GetMethod({verb}): expected {expected:?}, got {} {:?} with {:?}",
            answer.status,
            answer.error,
            answer.body
        );
    }

    // The headers an ordinary HTTP client sends of its own accord change none of that: the
    // method that serves a request follows its verb and the api's records, not what the
    // client declares it accepts.
    let client_headers: &[(&str, &str)] = &[
        ("accept", "*/*"),
        ("accept-encoding", "gzip, deflate"),
        ("connection", "keep-alive"),
        ("user-agent", "emulator-test/1.0"),
    ];
    for (verb, expected) in [("GET", explicit_body("GET")), ("POST", ANY_BODY.to_string())] {
        let (status, body) = invoke_with(&gateway, &api, verb, &path, client_headers).await;
        assert_eq!(
            (status, body.as_str()),
            (200, expected.as_str()),
            "case \"{verb} with an ordinary client's own headers\": expected the serving \
             method's own body, got ({status}, {body:?})"
        );
    }

    // The explicit method is deleted, and the deletion is DEPLOYED: what the stage serves
    // follows the deployment, so the verb that had its own method is served by `ANY` from
    // the new one, while the other explicit method keeps serving its own.
    let deleted = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{item}/methods/GET"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "setup DeleteMethod(GET): expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    deploy(&gateway, &api).await;
    let rows: &[(&str, &str, &str)] = &[
        ("GET once the deletion is deployed", "GET", ANY_BODY),
        (
            "the other explicit verb, untouched",
            "DELETE",
            r#"{"served":"DELETE"}"#,
        ),
        ("a verb that was always ANY's", "POST", ANY_BODY),
    ];
    for (case, verb, expected) in rows {
        let (status, body) = invoke(&gateway, &api, verb, &path).await;
        assert_eq!(
            (status, body.as_str()),
            (200, *expected),
            "case {case:?}: expected the serving method's own body, got ({status}, {body:?})"
        );
    }

    // And `GetMethod` for the deleted verb is the same `NotFoundException` every other verb
    // `ANY` serves earns: which methods the resource HAS does not follow from what the stage
    // serves.
    let after_delete = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{item}/methods/GET"),
            None,
        )
        .await;
    assert_eq!(
        (after_delete.status, after_delete.error.as_deref()),
        (404, Some("NotFoundException")),
        "case \"GetMethod(GET) once the method is deleted\": expected 404 NotFoundException, \
         got {} {:?}",
        after_delete.status,
        after_delete.error
    );
}
