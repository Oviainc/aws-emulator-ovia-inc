// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCESS LOG FORMAT a stage may carry: which
//! `accessLogSettings.format` an `UpdateStage` patch stores, and what the refusal of one it
//! does not states.
//!
//! THE RULE UNDER TEST: "The format must include at least `$context.requestId`"
//! (`AccessLogSettings$format`), and API Gateway takes EITHER identifier of the request in
//! its place — `$context.extendedRequestId` names the same request under the member API
//! Gateway reports it as on the response — so a format naming either one is stored and a
//! format naming NEITHER is `BadRequestException` whose message names BOTH. Naming only one
//! of the two would tell the caller to add a variable it does not have to add, so the fix it
//! applies could still be refused.
//!
//! The rows span what the rule generalises over: WHICH identifier the format names, WHERE in
//! the format it appears (alone, inside a CLF line, inside a JSON document, both of them),
//! and the forms that name neither (another context variable, the bare member name without
//! the `$context.` prefix, a format with no reference at all, the empty format).

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// The stage every row patches.
const STAGE: &str = "prod";

/// The destination the settings name, which is a well-formed log group ARN; nothing about
/// the format rule depends on it.
const DESTINATION: &str =
    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/apigateway/orders-access";

/// The error shape a refused `UpdateStage` carries.
const BAD_REQUEST: &str = "BadRequestException";

/// The two variables that satisfy the rule, which a refusal has to name.
const REQUEST_ID: &str = "$context.requestId";

/// See [`REQUEST_ID`].
const EXTENDED_REQUEST_ID: &str = "$context.extendedRequestId";

/// `UpdateStage` replacing the access log destination and format.
async fn set_access_log(gateway: &Gateway, api: &str, format: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({
                "patchOperations": [
                    {
                        "op": "replace",
                        "path": "/accessLogSettings/destinationArn",
                        "value": DESTINATION,
                    },
                    { "op": "replace", "path": "/accessLogSettings/format", "value": format },
                ],
            })),
        )
        .await
}

/// An api with one deployment released to [`STAGE`].
async fn deployed_api(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    create_deployment(gateway, &api, name, json!({ "stageName": STAGE })).await;
    api
}

/// RULE: a format naming EITHER request identifier is stored and read back; a format naming
/// neither is refused with a message naming both.
#[tokio::test]
async fn a_format_naming_either_request_identifier_is_stored() {
    // (case, the format, whether it is stored)
    let rows: &[(&str, &str, bool)] = &[
        ("the required variable alone", REQUEST_ID, true),
        (
            "the extended identifier alone, which names the same request",
            EXTENDED_REQUEST_ID,
            true,
        ),
        (
            "a CLF line carrying the required variable",
            "$context.identity.sourceIp - - [$context.requestTime] \
             \"$context.httpMethod $context.resourcePath $context.protocol\" \
             $context.status $context.responseLength $context.requestId",
            true,
        ),
        (
            "a JSON document carrying the extended identifier",
            "{\"requestId\":\"$context.extendedRequestId\",\"status\":\"$context.status\"}",
            true,
        ),
        (
            "a format carrying both identifiers",
            "{\"id\":\"$context.requestId\",\"extended\":\"$context.extendedRequestId\"}",
            true,
        ),
        (
            "another context variable and neither identifier",
            "$context.status",
            false,
        ),
        (
            "the member name without the $context. prefix",
            "{\"requestId\":\"requestId\"}",
            false,
        ),
        (
            "a format carrying no reference at all",
            "a fixed line",
            false,
        ),
        (
            "a longer member whose name merely starts with the required one",
            "{\"who\":\"$context.requestIdentity\"}",
            false,
        ),
        (
            "the required variable at the very end of the format, where nothing follows it",
            "request=$context.requestId",
            true,
        ),
        ("the empty format", "", false),
    ];
    for (index, (case, format, stored)) in rows.iter().enumerate() {
        // One emulator per row, so a format a row stored cannot decide the next row's answer.
        let gateway = Gateway::new();
        let api = deployed_api(&gateway, &format!("access-log-{index}")).await;
        let answer = set_access_log(&gateway, &api, format).await;
        if *stored {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the format to be stored, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["accessLogSettings"]["format"],
                json!(format),
                "case {case:?}: expected the stored format to be reported, got {:?}",
                answer.body["accessLogSettings"]
            );
            // The round trip: what the patch stored is what the stage reads back.
            let read = get_stage(&gateway, &api, STAGE).await;
            assert_eq!(
                read.body["accessLogSettings"]["format"],
                json!(format),
                "case {case:?}: expected GetStage to read the format back, got {:?}",
                read.body["accessLogSettings"]
            );
            continue;
        }
        assert_failure(&answer, 400, BAD_REQUEST, case);
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(REQUEST_ID) && message.contains(EXTENDED_REQUEST_ID),
            "case {case:?}: expected the refusal to name both {REQUEST_ID} and \
             {EXTENDED_REQUEST_ID}, got {message:?}"
        );
        // A refused patch leaves the stage exactly as it was: nothing of the settings is
        // stored, not even the destination the same list named.
        let read = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            read.body["accessLogSettings"],
            Value::Null,
            "case {case:?}: a refused patch must store no access log settings, got {:?}",
            read.body["accessLogSettings"]
        );
    }
}
