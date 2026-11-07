// SPDX-License-Identifier: CC0-1.0

//! A deployed `OPTIONS` method behind a `MOCK` integration: the CORS preflight a browser
//! sends is answered from the api's own records.

use serde_json::json;

use crate::execute_api_data_plane::{integration_response, method_response, put_integration,
                                    put_method};
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

const REGION: &str = "us-east-1";
const STAGE: &str = "live";

#[tokio::test]
async fn a_mock_options_method_answers_the_preflight() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "cors-preflight").await;
    let resource = create_resource(&gateway, &api, &root, "items").await;
    let resource = string_member(&resource, "id", "CreateResource(items)");

    put_method(
        &gateway,
        &api,
        &resource,
        "OPTIONS",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        &gateway,
        &api,
        &resource,
        "OPTIONS",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
        }),
    )
    .await;
    method_response(
        &gateway,
        &api,
        &resource,
        "OPTIONS",
        "200",
        json!({
            "responseParameters": {
                "method.response.header.Access-Control-Allow-Origin": false,
                "method.response.header.Access-Control-Allow-Methods": false,
                "method.response.header.Access-Control-Allow-Headers": false,
            },
        }),
    )
    .await;
    integration_response(
        &gateway,
        &api,
        &resource,
        "OPTIONS",
        "200",
        json!({
            "responseTemplates": { "application/json": "" },
            "responseParameters": {
                "method.response.header.Access-Control-Allow-Origin": "'*'",
                "method.response.header.Access-Control-Allow-Methods": "'OPTIONS,GET'",
                "method.response.header.Access-Control-Allow-Headers": "'Content-Type'",
            },
        }),
    )
    .await;
    let deployed = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        )
        .await;
    assert_eq!(
        deployed.status, 201,
        "case CreateDeployment: expected 201, got {} with {:?}",
        deployed.status, deployed.body
    );

    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("OPTIONS")
        .uri(format!("https://{host}/{STAGE}/items"))
        .header("host", &host)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "content-type")
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    assert_eq!(
        status, 200,
        "case \"the preflight\": expected 200, got {status} with {body:?} and {headers:?}"
    );
    assert_eq!(
        header("access-control-allow-origin").as_deref(),
        Some("*"),
        "case \"the preflight\": expected the mapped origin header, got {headers:?}"
    );
}
