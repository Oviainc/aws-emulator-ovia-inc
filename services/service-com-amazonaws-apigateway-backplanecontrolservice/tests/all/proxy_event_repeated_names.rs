// SPDX-License-Identifier: CC0-1.0

//! What an `AWS_PROXY` integration's event carries about the request it was built from: a
//! name the request REPEATS, the spelling each header name is reported under, and the
//! address the call came from.
//!
//! RULE: a proxy integration's event reports a request's query string and headers TWICE —
//! `multiValueQueryStringParameters` and `multiValueHeaders` carry every value a name
//! carries, in the order sent, while `queryStringParameters` and `headers` carry one value
//! per name, the LAST of them — and every header name is reported in the capitalisation HTTP
//! spells a field name in (`X-Multi`, `User-Agent`), which is the spelling the wire layer
//! normalises away and the one API Gateway hands the function.
//!
//! RULE: `requestContext.identity.sourceIp` is the address the call came from: the first hop
//! a forwarder recorded where the request reached the emulator through one, and the loopback
//! interface this emulator is reached over otherwise.
//!
//! The rows differ along what the rules generalise over: a name sent once, twice and three
//! times, in the query string and in a header, a header name whose canonical spelling has
//! one token and several, and a request that carries a forwarded address and one that carries
//! none. The handler reports the members back verbatim, so what is asserted is the event the
//! function was handed.
//!
//! The wire harness is the sibling `proxy_binary_media_types` suite's, reused rather than
//! stood up a second way.

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};

use crate::proxy_binary_media_types::{apigw, authorization, dispatch, emulator, lambda, member,
                                      package, wait_until_active, FUNCTION_TIMEOUT, HANDLER,
                                      REGION, ROLE, STAGE};

/// The function the integration names.
const FUNCTION: &str = "apigw-event-repeated-names";

/// The handler: it reports the four members of the event this suite is about.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps(
            {
                "query": event.get("queryStringParameters"),
                "multiQuery": event.get("multiValueQueryStringParameters"),
                "headers": event.get("headers"),
                "multiHeaders": event.get("multiValueHeaders"),
                "identity": (event.get("requestContext") or {}).get("identity"),
            }
        ),
    }
"#;

#[tokio::test]
async fn a_proxy_event_carries_every_value_a_repeated_name_sent_and_the_last_of_them() {
    let router = emulator();

    let created = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup CreateFunction: expected HTTP 201 from Lambda, got {} with {:?}",
        created.status,
        created.text()
    );
    let function_arn = member("CreateFunction", &created.json(), "FunctionArn");
    wait_until_active(&router, FUNCTION).await;
    let granted = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigw-any-api",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
        })),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "setup AddPermission: expected HTTP 201 from Lambda, got {} with {:?}",
        granted.status,
        granted.text()
    );

    let api_record = apigw(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "proxy-event-repeated-names" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api_record, "id");
    let root = member("CreateRestApi", &api_record, "rootResourceId");
    apigw(
        &router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{root}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        &router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{root}/methods/GET/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function_arn}/invocations"
            ),
        })),
        201,
    )
    .await;
    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    let answered = dispatch(
        &router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/?single=x&twice=1&twice=2&thrice=a&thrice=b&thrice=c"),
        &[
            ("x-multi", "one"),
            ("x-multi", "two"),
            ("x-single-header", "only"),
            ("accept", "application/json"),
            ("authorization", &authorization("execute-api")),
        ],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case \"the event's repeated names\": expected HTTP 200 from the stage, got {} with \
         {:?}",
        answered.status,
        answered.text()
    );
    let event = answered.json();

    // (the member, the name, the value the member must carry for it)
    let single: &[(&str, &str, Value)] = &[
        ("query", "single", json!("x")),
        // The LAST value of a repeated name, in both single-valued maps.
        ("query", "twice", json!("2")),
        ("query", "thrice", json!("c")),
        ("headers", "X-Multi", json!("two")),
        ("headers", "X-Single-Header", json!("only")),
        ("headers", "Accept", json!("application/json")),
    ];
    for (member_name, name, expected) in single {
        let read = event
            .get(*member_name)
            .and_then(|carried| carried.get(*name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            read, *expected,
            "case {member_name:?}.{name:?}: expected {expected}, got {read} in {event}"
        );
    }

    let every: &[(&str, &str, Value)] = &[
        ("multiQuery", "single", json!(["x"])),
        ("multiQuery", "twice", json!(["1", "2"])),
        ("multiQuery", "thrice", json!(["a", "b", "c"])),
        ("multiHeaders", "X-Multi", json!(["one", "two"])),
        ("multiHeaders", "X-Single-Header", json!(["only"])),
    ];
    for (member_name, name, expected) in every {
        let read = event
            .get(*member_name)
            .and_then(|carried| carried.get(*name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            read, *expected,
            "case {member_name:?}.{name:?}: expected {expected}, got {read} in {event}"
        );
    }

    // The credential the event withholds is withheld from both header maps.
    for member_name in ["headers", "multiHeaders"] {
        let carried = event.get(member_name).cloned().unwrap_or(Value::Null);
        assert!(
            carried.get("Authorization").is_none(),
            "case {member_name:?}: expected the caller's credential to be withheld, got \
             {carried}"
        );
    }

    // The address the call came from, over the two cases a request off the wire takes: one
    // a forwarder recorded an address for, and one that reached the emulator directly.
    // (the case, the forwarded header the request carries, the address the event reports)
    let addresses: &[(&str, Option<&str>, &str)] = &[
        ("a request no forwarder recorded", None, "127.0.0.1"),
        (
            "a request a forwarder recorded, whose first hop is the client",
            Some("203.0.113.7, 198.51.100.2"),
            "203.0.113.7",
        ),
    ];
    for (case, forwarded, expected) in addresses {
        let mut headers: Vec<(&str, &str)> = vec![("accept", "application/json")];
        if let Some(forwarded) = forwarded {
            headers.push(("x-forwarded-for", forwarded));
        }
        let answered = dispatch(
            &router,
            "GET",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            &format!("/{STAGE}/"),
            &headers,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from the stage, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let reported = answered
            .json()
            .get("identity")
            .and_then(|identity| identity.get("sourceIp").cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            reported,
            json!(expected),
            "case {case:?}: expected sourceIp {expected:?}, got {reported}"
        );
    }
}
