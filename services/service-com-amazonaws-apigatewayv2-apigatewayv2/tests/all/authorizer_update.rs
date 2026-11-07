// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Authorizer` WRITERS and READERS that
//! complete the resource: `UpdateAuthorizer` and `GetAuthorizers`.
//!
//! The rules these cases assert:
//!
//! * **`UpdateAuthorizer` is a PATCH of the ONE stored record**
//!   (`PATCH /v2/apis/{ApiId}/authorizers/{AuthorizerId}`, success 200): every member the
//!   request carries replaces the stored one, every member it omits keeps the value it
//!   had, and the `AuthorizerId` is unchanged — so the body it answers is the body a later
//!   `GetAuthorizer` and `GetAuthorizers` report.
//! * **The MERGED configuration has to hold together**, under the same requirements the
//!   create applies: `AuthorizerType` `JWT` is "supported only for HTTP APIs" and needs a
//!   `JwtConfiguration` whose `Issuer` is the identity provider, `REQUEST` needs the
//!   `AuthorizerUri` of the Lambda it invokes and (off WebSocket) an
//!   `AuthorizerPayloadFormatVersion` of "1.0 and 2.0"; a patch that would leave the
//!   authorizer in a configuration none of those admit is refused 400
//!   `BadRequestException` and changes nothing a later reader sees.
//! * **An authorizer is addressed by the api that owns it AND its own id**: an api no
//!   record holds, an id no authorizer of that api carries, and an id belonging to
//!   ANOTHER api are each 404 `NotFoundException`, and nothing is written.
//! * **`GetAuthorizers` reports that same record for every authorizer of the addressed
//!   api**, in the store's stable id order, one bounded page at a time: `NextToken` is
//!   present only while more remain, resuming from it continues exactly where the page
//!   stopped, and no authorizer of another api appears.
//! * **The `$connect` authorization step reads the UPDATED record**: patching an
//!   authorizer's `IdentitySource` changes which opening handshakes are refused 401, which
//!   is what "one record, every reader" means for the data plane.
//!
//! The rows vary what each rule generalises over: the owning api's protocol (`HTTP`,
//! `WEBSOCKET`), the authorizer type (`REQUEST`, `JWT`, and each switch between them),
//! which members a patch carries (none, one, several, a list, a structure), the number of
//! identity sources (none, one, two), the payload format version, the TTL at each
//! published bound and past it, and the page size.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};
use crate::websocket_handshake_data_plane::{message, send, Answer, CLIENT_KEY};

/// Every member of the modelled `Authorizer` shape a patch may carry, from
/// `smithy_query(query="operation", operation="UpdateAuthorizer")`'s input: these are the
/// members whose "replaced if sent, kept if omitted" behaviour the first table asserts.
/// `AuthorizerId` and `ApiId` are the addressing, not configuration, so they are not here.
const PATCHABLE_MEMBERS: &[&str] = &[
    "name",
    "authorizerType",
    "authorizerUri",
    "authorizerCredentialsArn",
    "authorizerPayloadFormatVersion",
    "authorizerResultTtlInSeconds",
    "enableSimpleResponses",
    "identitySource",
    "identityValidationExpression",
    "jwtConfiguration",
];

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:eu-west-1:123456789012:function:{function}/invocations"
    )
}

/// Create one api of `protocol` and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, protocol: &str) -> String {
    let mut request = json!({ "name": "authorizer-host", "protocolType": protocol });
    if protocol == "WEBSOCKET" {
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one authorizer under `api_id` and answer the body it was created with.
async fn create_authorizer(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> Value {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
    );
    created
}

/// `PATCH` one authorizer and answer `(status, body)`.
async fn patch(
    emulator: &MultiServiceRouter,
    api_id: &str,
    authorizer_id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
        Some(body.clone()),
    )
    .await
}

/// Read one authorizer back and answer `(status, body)`.
async fn get_authorizer(
    emulator: &MultiServiceRouter,
    api_id: &str,
    authorizer_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
        None,
    )
    .await
}

/// Read one page of an api's authorizers and answer `(status, body)`. `query` is appended
/// to the collection URI as sent (so a malformed `maxResults` reaches the handler).
async fn get_authorizers(
    emulator: &MultiServiceRouter,
    api_id: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/v2/apis/{api_id}/authorizers")
    } else {
        format!("/v2/apis/{api_id}/authorizers?{query}")
    };
    call(emulator, "GET", &uri, None).await
}

/// The `items` of a collection answer.
fn items(body: &Value) -> Vec<Value> {
    body.get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One accepted-patch row: the owning api's protocol, the create, and the patch.
struct Row {
    case: &'static str,
    protocol: &'static str,
    create: Value,
    patch: Value,
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a WebSocket REQUEST authorizer renamed, nothing else touched",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({ "name": "connect-auth-renamed" }),
        },
        Row {
            case: "the identity source list replaced by two entries",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({
                "identitySource": [
                    "route.request.header.Auth",
                    "route.request.querystring.token"
                ]
            }),
        },
        Row {
            case: "the identity source list replaced by an empty one, which a REQUEST \
                   authorizer may carry",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({ "identitySource": [] }),
        },
        Row {
            case: "the Lambda the authorizer invokes, and the role it is invoked with",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("first-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({
                "authorizerUri": lambda_uri("second-auth"),
                "authorizerCredentialsArn": "arn:aws:iam::123456789012:role/apigw-invoke"
            }),
        },
        Row {
            case: "an empty patch, which replaces nothing",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({}),
        },
        Row {
            case: "an HTTP REQUEST authorizer moved to payload format 2.0 with simple \
                   responses on",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "1.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({
                "authorizerPayloadFormatVersion": "2.0",
                "enableSimpleResponses": true
            }),
        },
        Row {
            case: "the TTL patched to the published lower bound, which disables caching",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": 300,
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerResultTtlInSeconds": 0 }),
        },
        Row {
            case: "the TTL patched to the published upper bound",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerResultTtlInSeconds": 3600 }),
        },
        Row {
            case: "a JWT authorizer's configuration replaced wholesale, two audiences for \
                   one",
            protocol: "HTTP",
            create: json!({
                "name": "jwt-auth",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": {
                    "issuer": "https://first-issuer.test/oauth2",
                    "audience": ["first-audience"]
                }
            }),
            patch: json!({
                "jwtConfiguration": {
                    "issuer": "https://second-issuer.test/",
                    "audience": ["second-audience", "third-audience"]
                }
            }),
        },
        Row {
            case: "a JWT authorizer's name and validation expression",
            protocol: "HTTP",
            create: json!({
                "name": "jwt-auth",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": {
                    "issuer": "https://first-issuer.test/oauth2",
                    "audience": ["first-audience"]
                }
            }),
            patch: json!({
                "name": "jwt-auth-renamed",
                "identityValidationExpression": "^Bearer .+$"
            }),
        },
        Row {
            case: "an HTTP REQUEST authorizer switched to JWT, with the configuration that \
                   type needs in the same patch",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({
                "authorizerType": "JWT",
                "jwtConfiguration": {
                    "issuer": "https://issuer.test/oauth2",
                    "audience": ["an-audience"]
                }
            }),
        },
        Row {
            case: "a JWT authorizer switched to REQUEST, with the Lambda and payload \
                   format that type needs",
            protocol: "HTTP",
            create: json!({
                "name": "jwt-auth",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": {
                    "issuer": "https://issuer.test/oauth2",
                    "audience": ["an-audience"]
                }
            }),
            patch: json!({
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "1.0"
            }),
        },
    ]
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_keeps_the_rest() {
    for row in accepted_rows() {
        let emulator = emulator();
        let api_id = create_api(&emulator, row.case, row.protocol).await;
        let created = create_authorizer(&emulator, row.case, &api_id, &row.create).await;
        let authorizer_id = as_str(&created, "authorizerId");

        let (status, updated) = patch(&emulator, &api_id, &authorizer_id, &row.patch).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateAuthorizer for patch {}, got {status} \
             {updated}",
            row.case, row.patch
        );
        assert_eq!(
            as_str(&updated, "authorizerId"),
            authorizer_id,
            "case {:?}: a patch updates the authorizer it addresses rather than minting \
             another, got {updated}",
            row.case
        );

        let sent = row
            .patch
            .as_object()
            .cloned()
            .unwrap_or_else(Map::new);
        for member in PATCHABLE_MEMBERS {
            let expected = sent.get(*member).or_else(|| created.get(*member));
            let actual = updated.get(*member);
            assert_eq!(
                actual,
                expected,
                "case {:?}: member {member:?} must be {} the patch carries, \
                 expected {expected:?}, got {actual:?} in {updated}",
                row.case,
                if sent.contains_key(*member) {
                    "the value"
                } else {
                    "unchanged by a patch"
                }
            );
        }

        // What a later caller reads is the record the patch wrote — through the single
        // reader and through the collection, which must not disagree about it.
        let (status, fetched) = get_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetAuthorizer, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, updated,
            "case {:?}: GetAuthorizer must report the record the patch wrote",
            row.case
        );
        let (status, collection) = get_authorizers(&emulator, &api_id, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetAuthorizers, got {status} {collection}",
            row.case
        );
        assert_eq!(
            items(&collection),
            vec![updated.clone()],
            "case {:?}: GetAuthorizers must report the api's one authorizer as the patch \
             left it, got {collection}",
            row.case
        );
    }
}

/// One refused-patch row: the create, the patch, and the status and error code it owes.
struct Refusal {
    case: &'static str,
    protocol: &'static str,
    create: Value,
    patch: Value,
    status: u16,
    /// `Some` where the refusal is the operation's own modelled error; `None` where the
    /// boundary validator refuses the request before the handler runs, whose code is the
    /// framework's rather than this operation's.
    code: Option<&'static str>,
}

fn refusal_rows() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a WebSocket authorizer switched to JWT, which is supported only for \
                   HTTP APIs",
            protocol: "WEBSOCKET",
            create: json!({
                "name": "connect-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.querystring.token"]
            }),
            patch: json!({
                "authorizerType": "JWT",
                "jwtConfiguration": { "issuer": "https://issuer.test/oauth2" }
            }),
            status: 400,
            code: Some("BadRequestException"),
        },
        Refusal {
            case: "an HTTP authorizer switched to JWT without the configuration that type \
                   requires",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerType": "JWT" }),
            status: 400,
            code: Some("BadRequestException"),
        },
        Refusal {
            case: "a JWT authorizer switched to REQUEST without the Lambda URI that type \
                   requires",
            protocol: "HTTP",
            create: json!({
                "name": "jwt-auth",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": {
                    "issuer": "https://issuer.test/oauth2",
                    "audience": ["an-audience"]
                }
            }),
            patch: json!({
                "authorizerType": "REQUEST",
                "authorizerPayloadFormatVersion": "2.0"
            }),
            status: 400,
            code: Some("BadRequestException"),
        },
        Refusal {
            case: "a payload format version outside the supported values",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerPayloadFormatVersion": "3.0" }),
            status: 400,
            code: Some("BadRequestException"),
        },
        Refusal {
            case: "a TTL one past the published upper bound",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerResultTtlInSeconds": 3601 }),
            status: 400,
            code: None,
        },
        Refusal {
            case: "a TTL one below the published lower bound",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerResultTtlInSeconds": -1 }),
            status: 400,
            code: None,
        },
        Refusal {
            case: "an authorizer type outside the modelled enum",
            protocol: "HTTP",
            create: json!({
                "name": "http-auth",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("http-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
            patch: json!({ "authorizerType": "TOKEN" }),
            status: 400,
            code: None,
        },
    ]
}

#[tokio::test]
async fn a_refused_patch_changes_nothing_a_later_reader_sees() {
    for row in refusal_rows() {
        let emulator = emulator();
        let api_id = create_api(&emulator, row.case, row.protocol).await;
        let created = create_authorizer(&emulator, row.case, &api_id, &row.create).await;
        let authorizer_id = as_str(&created, "authorizerId");

        let (status, refused) = patch(&emulator, &api_id, &authorizer_id, &row.patch).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected {} for patch {}, got {status} {refused}",
            row.case, row.status, row.patch
        );
        if let Some(code) = row.code {
            assert_eq!(
                error_code(&refused),
                code,
                "case {:?}: expected the modelled {code}, got {refused}",
                row.case
            );
        }

        let (status, fetched) = get_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetAuthorizer, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, created,
            "case {:?}: the refused patch changed the record a later caller reads",
            row.case
        );
    }
}

#[tokio::test]
async fn an_authorizer_is_addressed_by_the_api_that_owns_it_and_its_own_id() {
    let emulator = emulator();
    let case = "addressing";
    let first_api = create_api(&emulator, case, "WEBSOCKET").await;
    let second_api = create_api(&emulator, case, "WEBSOCKET").await;
    let created = create_authorizer(
        &emulator,
        case,
        &first_api,
        &json!({
            "name": "connect-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("connect-auth"),
            "identitySource": ["route.request.querystring.token"]
        }),
    )
    .await;
    let authorizer_id = as_str(&created, "authorizerId");
    let rename = json!({ "name": "renamed" });

    // Lowercase alphanumerics, the alphabet this service mints an id from, that no record
    // carries.
    const ABSENT_ID: &str = "zzzz999zzz";
    for (case, api, id) in [
        ("an api no record holds", ABSENT_ID, authorizer_id.as_str()),
        ("an authorizer id no record holds", first_api.as_str(), ABSENT_ID),
        (
            "an authorizer of ANOTHER api, which this api does not hold",
            second_api.as_str(),
            authorizer_id.as_str(),
        ),
    ] {
        let (status, refused) = patch(&emulator, api, id, &rename).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateAuthorizer, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {refused}"
        );
    }

    let (status, collection) = get_authorizers(&emulator, ABSENT_ID, "").await;
    assert_eq!(
        status, 404,
        "an api no record holds has no collection to report, got {status} {collection}"
    );
    assert_eq!(
        error_code(&collection),
        "NotFoundException",
        "expected the modelled NotFoundException, got {collection}"
    );
    let (status, second) = get_authorizers(&emulator, &second_api, "").await;
    assert_eq!(status, 200, "expected 200 from GetAuthorizers, got {status} {second}");
    assert_eq!(
        items(&second),
        Vec::<Value>::new(),
        "an api holding no authorizer reports an empty collection, got {second}"
    );
    let (status, unchanged) = get_authorizer(&emulator, &first_api, &authorizer_id).await;
    assert_eq!(status, 200, "expected 200 from GetAuthorizer, got {status} {unchanged}");
    assert_eq!(
        unchanged, created,
        "a refused patch of another api's authorizer changed this one"
    );
}

#[tokio::test]
async fn the_collection_reports_every_authorizer_of_the_api_one_page_at_a_time() {
    let emulator = emulator();
    let case = "pagination";
    let api_id = create_api(&emulator, case, "HTTP").await;
    let other_api = create_api(&emulator, case, "HTTP").await;
    let mut created: Vec<Value> = Vec::new();
    for index in 0..5 {
        created.push(
            create_authorizer(
                &emulator,
                case,
                &api_id,
                &json!({
                    "name": format!("http-auth-{index}"),
                    "authorizerType": "REQUEST",
                    "authorizerUri": lambda_uri(&format!("http-auth-{index}")),
                    "authorizerPayloadFormatVersion": "2.0",
                    "identitySource": ["$request.header.Authorization"]
                }),
            )
            .await,
        );
    }
    create_authorizer(
        &emulator,
        case,
        &other_api,
        &json!({
            "name": "other-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("other-auth"),
            "authorizerPayloadFormatVersion": "2.0",
            "identitySource": ["$request.header.Authorization"]
        }),
    )
    .await;

    // The collection's order is the store's own stable id order, so the expected whole is
    // the created set sorted by `AuthorizerId` rather than by creation order.
    let mut expected = created.clone();
    expected.sort_by_key(|authorizer| as_str(authorizer, "authorizerId"));

    for page_size in [1usize, 2, 4, 5, 6] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = get_authorizers(&emulator, &api_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetAuthorizers, got {status} {body}"
            );
            let page = items(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} items, got {} in {body}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the walk did not terminate after {pages} pages"
            );
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages walked must be exactly the api's own authorizers, in \
             the collection's order"
        );
    }

    let (status, body) = get_authorizers(&emulator, &api_id, "").await;
    assert_eq!(status, 200, "expected 200 from GetAuthorizers, got {status} {body}");
    assert_eq!(
        items(&body),
        expected,
        "a request naming no page size reports the whole collection, got {body}"
    );
    assert_eq!(
        body.get("nextToken"),
        None,
        "nothing remains after the last page, so no NextToken is reported, got {body}"
    );

    for raw in ["0", "-1", "abc", ""] {
        let (status, refused) = get_authorizers(&emulator, &api_id, &format!("maxResults={raw}")).await;
        assert_eq!(
            status, 400,
            "case {raw:?}: a MaxResults that is not a positive integer is refused, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {raw:?}: expected the modelled BadRequestException, got {refused}"
        );
    }

    // A token this collection did not issue cannot be served as a page of its own: one
    // the service never minted, and one minted for ANOTHER api's authorizers, are both
    // refused rather than silently restarting the walk.
    let (status, first_page) = get_authorizers(&emulator, &api_id, "maxResults=1").await;
    assert_eq!(status, 200, "expected 200 from GetAuthorizers, got {status} {first_page}");
    let foreign_token = as_str(&first_page, "nextToken");
    assert!(
        !foreign_token.is_empty(),
        "a page of one over five authorizers must carry a NextToken, got {first_page}"
    );
    for (case, token) in [
        ("a token this service never issued", "not-a-token"),
        ("another api's token", foreign_token.as_str()),
    ] {
        let (status, refused) =
            get_authorizers(&emulator, &other_api, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a token this collection did not issue, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected the modelled BadRequestException, got {refused}"
        );
    }
}

/// Send the opening handshake to `host`'s `stage`, with `query` appended to the path and
/// `headers` added to the three fields of RFC 6455 §4.1.
async fn handshake(
    emulator: &MultiServiceRouter,
    host: &str,
    stage: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut fields: Vec<(&str, &str)> = vec![
        ("connection", "Upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-key", CLIENT_KEY),
        ("sec-websocket-version", "13"),
    ];
    fields.extend_from_slice(headers);
    let path = if query.is_empty() {
        format!("/{stage}")
    } else {
        format!("/{stage}?{query}")
    };
    send(emulator, host, "GET", &path, &fields).await
}

#[tokio::test]
async fn the_connect_authorization_step_reads_the_patched_identity_sources() {
    // The authorization of the `$connect` route is applied before anything that route
    // fronts, from the authorizer's CURRENT record: "verifying all of the
    // identity-related request parameters are present in the request, not null, and
    // non-empty … Otherwise, it returns a 401 Unauthorized response without calling the
    // Lambda function" (`CreateAuthorizerRequest$IdentitySource`). A handshake the step lets
    // through is then answered by the CONNECTION REQUEST's own integration, and the `MOCK`
    // integration this route fronts reaches no backend, so the gateway answers 500
    // (`websocket_transport_data_plane` owns that rule) — so here a 500 means "let through".
    const UNAUTHORIZED: u16 = 401;
    const LET_THROUGH: u16 = 500;
    let case = "the data plane reads the patched record";
    let emulator = emulator();

    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: CreateApi must answer 201, got {created}");
    let api_id = as_str(&created, "apiId");
    let endpoint = as_str(&created, "apiEndpoint");
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let authorizer = create_authorizer(
        &emulator,
        case,
        &api_id,
        &json!({
            "name": "connect-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("connect-auth"),
            "identitySource": ["route.request.querystring.token"]
        }),
    )
    .await;
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let (status, route) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "$connect",
            "authorizationType": "CUSTOM",
            "authorizerId": authorizer_id
        })),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: CreateRoute must answer 201, got {route}");

    let stage = "prod";
    let (status, created_stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {created_stage}"
    );
    let (status, deployment) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDeployment must answer 201, got {deployment}"
    );

    // Before the patch: the query-string source the authorizer declares decides.
    let answer = handshake(&emulator, &host, stage, "token=t0ken", &[]).await;
    assert_eq!(
        answer.status,
        LET_THROUGH,
        "case {case:?}: the declared query-string source is presented, so the step lets \
         the handshake through, got {}",
        answer.summary()
    );
    let answer = handshake(&emulator, &host, stage, "", &[("auth", "t0ken")]).await;
    assert_eq!(
        answer.status,
        UNAUTHORIZED,
        "case {case:?}: a header the authorizer does not declare does not satisfy it, got {}",
        answer.summary()
    );

    // Patch the identity source onto that header, and the two answers swap.
    let (status, updated) = patch(
        &emulator,
        &api_id,
        &authorizer_id,
        &json!({ "identitySource": ["route.request.header.Auth"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateAuthorizer must answer 200, got {status} {updated}"
    );
    assert_eq!(
        updated.get("identitySource"),
        Some(&json!(["route.request.header.Auth"])),
        "case {case:?}: the patched authorizer must report its new identity source, got \
         {updated}"
    );

    let answer = handshake(&emulator, &host, stage, "", &[("auth", "t0ken")]).await;
    assert_eq!(
        answer.status,
        LET_THROUGH,
        "case {case:?}: the patched header source is presented, so the step lets the \
         handshake through, got {}",
        answer.summary()
    );
    let answer = handshake(&emulator, &host, stage, "token=t0ken", &[]).await;
    assert_eq!(
        answer.status,
        UNAUTHORIZED,
        "case {case:?}: the source the patch replaced no longer satisfies the authorizer, \
         got {}",
        answer.summary()
    );
    assert_eq!(
        message(&answer),
        "Unauthorized",
        "case {case:?}: the refusal carries the Unauthorized body, got {}",
        answer.summary()
    );
}
