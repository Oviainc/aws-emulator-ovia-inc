// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PAGINATION TOKEN an api's collection readers accept.
//!
//! The rule these cases assert: **a `NextToken` is opaque and belongs to the reader that
//! minted it, so a paginated reader of an api's children resumes a walk only for a token it
//! issued for THAT collection and refuses every other value with `BadRequestException`
//! (HTTP 400) and the sentence API Gateway words a cursor it cannot read with — never with
//! an empty page, which a client reads as "the collection is empty".**
//!
//! An empty page for a token nothing issued is the defect this suite exists for: a drift
//! detector paging an api's routes with a carried-over token would be told every route was
//! deleted.
//!
//! The same rule is asserted for two collections elsewhere — `refusal_resource_identity.rs`
//! for an api's route responses and `stage_pagination.rs` for its stages — so what is new
//! here is the collections those do not reach (an api's ROUTES, the operation the report
//! names, and its integrations) and the token forms they do not send (a token the SAME
//! reader issued for ANOTHER api, and another collection of the same api's token).
//!
//! The rows span what the rule generalises over: the COLLECTION being read (an api's routes,
//! its stages, its integrations — the readers that share one pagination), the FORM of the
//! value sent in place of a token (text that is not this service's encoding, an odd number
//! of hex digits, hex that is not UTF-8, a token longer than any this service mints, a
//! token the same reader issued for ANOTHER api, a token another collection of the SAME api
//! issued, and nothing at all), and the page size the request asks for beside it.
//! `GetRoutes` with a token it never issued — the recorded case — is one row.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::INVALID_NEXT_TOKEN_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// How many children of each kind the apis below hold: more than the page size the walk
/// asks for, so every reader has a token to issue.
const CHILDREN: usize = 3;

/// One api with the children every reader under test pages over.
struct Api {
    api_id: String,
}

/// Create an api and `CHILDREN` routes, stages and integrations under it.
async fn create_api(emulator: &MultiServiceRouter, name: &str) -> Api {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup {name:?}: expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    for index in 0..CHILDREN {
        for (collection, body) in [
            (
                "routes",
                json!({ "routeKey": format!("GET /items/{index}") }),
            ),
            ("stages", json!({ "stageName": format!("stage{index}") })),
            (
                "integrations",
                json!({
                    "integrationType": "HTTP_PROXY",
                    "integrationUri": format!("https://backend.example/{index}"),
                    "integrationMethod": "ANY",
                    "payloadFormatVersion": "1.0"
                }),
            ),
        ] {
            let (status, created) = call(
                emulator,
                "POST",
                &format!("/v2/apis/{api_id}/{collection}"),
                Some(body),
            )
            .await;
            assert_eq!(
                status, 201,
                "setup {name:?}: expected 201 creating a child of {collection}, got {status} \
                 {created}"
            );
        }
    }
    Api { api_id }
}

/// Read one page of `collection` of `api`, with the query the case asks for.
async fn page(
    emulator: &MultiServiceRouter,
    api: &Api,
    collection: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{}/{collection}?{query}", api.api_id),
        None,
    )
    .await
}

/// The token a reader of `collection` issues for its FIRST page of one record.
async fn issued_token(emulator: &MultiServiceRouter, api: &Api, collection: &str) -> String {
    let (status, first) = page(emulator, api, collection, "maxResults=1").await;
    assert_eq!(
        status, 200,
        "setup: expected 200 from the first page of {collection}, got {status} {first}"
    );
    let token = as_str(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "setup: {collection} holds more records than the page size, so a token must be \
         issued, got {first}"
    );
    token
}

#[tokio::test]
async fn a_paginated_reader_refuses_a_token_it_did_not_issue_rather_than_serving_an_empty_page() {
    let emulator = emulator();
    let api = create_api(&emulator, "paged-api").await;
    let other = create_api(&emulator, "other-paged-api").await;

    for collection in ["routes", "stages", "integrations"] {
        let own = issued_token(&emulator, &api, collection).await;
        let same_reader_other_api = issued_token(&emulator, &other, collection).await;
        let another_collection = issued_token(
            &emulator,
            &api,
            if collection == "routes" {
                "stages"
            } else {
                "routes"
            },
        )
        .await;

        let rows: Vec<(&str, String)> = vec![
            ("text that is not this service's encoding", "sim2real-not-a-token".to_string()),
            ("an odd number of hex digits", "abc".to_string()),
            ("hex whose bytes are not UTF-8", "ff".to_string()),
            (
                "a token longer than any this service mints",
                "61".repeat(4_096),
            ),
            (
                "a token the same reader issued for another api",
                same_reader_other_api,
            ),
            (
                "a token another collection of this api issued",
                another_collection,
            ),
        ];
        for (case, token) in rows {
            for query in [
                format!("nextToken={token}"),
                format!("maxResults=2&nextToken={token}"),
            ] {
                let (status, body) = page(&emulator, &api, collection, &query).await;
                assert_eq!(
                    status, 400,
                    "case {collection}/{case:?} ({query:?}): expected 400 rather than a page, \
                     got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "BadRequestException",
                    "case {collection}/{case:?}: expected BadRequestException, got {body}"
                );
                assert_eq!(
                    as_str(&body, "message"),
                    INVALID_NEXT_TOKEN_MESSAGE,
                    "case {collection}/{case:?}: expected the refusal \
                     {INVALID_NEXT_TOKEN_MESSAGE:?}, got {body}"
                );
                assert!(
                    body.get("items").is_none(),
                    "case {collection}/{case:?}: a refusal carries no page of records, got \
                     {body}"
                );
            }
        }

        // The reader's OWN token still resumes the walk, so the refusals above are about
        // those values and not about tokens in general — and every record is reported once.
        let (status, resumed) = page(
            &emulator,
            &api,
            collection,
            &format!("maxResults={CHILDREN}&nextToken={own}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {collection}: expected the issuing reader to accept its own token, got \
             {status} {resumed}"
        );
        let reported = resumed["items"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            reported,
            CHILDREN - 1,
            "case {collection}: expected the rest of the collection after its first record, \
             got {resumed}"
        );
    }
}
