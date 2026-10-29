// SPDX-License-Identifier: CC0-1.0

//! What `ListInvestigationGroups` does with the `nextToken` a caller hands it.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a pagination token is opaque SERVER state, so the listing
//! resumes only at a token that names a resource of the result set the request is for —
//! a group the Region the request is signed for actually holds — and refuses every other
//! token with the modelled 400 `ValidationException`, never a 200 page the store cannot
//! back. A token that merely LOOKS like one (a well-formed id no group carries, another
//! Region's group, a group since deleted) sorts somewhere in the walk's key order and
//! would otherwise answer a short or empty page with no `nextToken`, which a caller
//! cannot tell from a complete final page — the listing would hide persisted state
//! instead of refusing the token.
//!
//! The rows differ along WHAT the token names, which is what the rule generalises over:
//! the Region's own stored position (served), no token at all (served), a well-formed id
//! sorting above every stored id, one sorting below every stored id, the stored id with a
//! single character changed, a position another Region holds, a position this Region held
//! until it was deleted, and text that is no position at all. Each row is run in several
//! Regions — one holding a group, one holding none — and across each published bound of
//! the modelled `maxResults` range (1..50) plus the absent-parameter default, since the
//! page size does not change which tokens are served.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state;
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The requirement clause a refused `nextToken` is worded with, as the handler words it.
const NEXT_TOKEN_REQUIREMENT: &str =
    "Member must be a pagination token returned by a previous ListInvestigationGroups call";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the request's Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
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

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode everything a URI query value may not carry literally — what an SDK
/// does for an `@httpQuery`-bound member.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create a group in `region` that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?} in {region}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {name:?} in {region}: no arn in {created:?}"))
        .to_string()
}

/// The id segment of an investigation-group ARN.
fn id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("an investigation-group ARN carries an id segment: {arn:?}"))
        .to_string()
}

/// `?maxResults=` for a page size, or the empty query for the absent-parameter default.
fn page_size_query(page_size: Option<i32>) -> String {
    page_size.map_or_else(String::new, |size| format!("&maxResults={size}"))
}

/// The same id with ONE character changed, so the token is perfectly well formed and
/// still names no group: the character is moved inside the id alphabet, which keeps the
/// result id-shaped.
fn id_with_one_character_changed(id: &str) -> String {
    let mut changed: Vec<char> = id.chars().collect();
    let first = changed[0];
    changed[0] = if first == 'a' { 'b' } else { 'a' };
    changed.into_iter().collect()
}

/// One `nextToken` a caller can present, and whether the listing serves it.
struct Row {
    /// How a failure names the row.
    case: String,
    /// The token as the query carries it, or `None` for a request with no token.
    token: Option<String>,
    /// `true` when the token names a position of the listing it is presented to.
    served: bool,
}

/// A listing the token is served for reports exactly the Region's own groups from the
/// named position on; a refused one is a 400 `ValidationException` that echoes nothing of
/// the token, and never a 200.
async fn assert_row(router: &MultiServiceRouter, region: &str, expected: &[String], row: &Row) {
    for page_size in [None, Some(1), Some(50)] {
        let case = format!("{} in {region} with maxResults={page_size:?}", row.case);
        let query = match &row.token {
            Some(token) => format!("?nextToken={}{}", encode(token), page_size_query(page_size)),
            None => page_size
                .map_or_else(String::new, |size| format!("?maxResults={size}")),
        };
        let (status, body) =
            call_in(router, region, "GET", &format!("/investigationGroups{query}"), None).await;

        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: a token naming a position of this listing is served, \
                 got {status} / {body:?}"
            );
            let arns: Vec<String> = body
                .get("investigationGroups")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no investigationGroups in {body:?}"))
                .iter()
                .filter_map(|item| item.get("arn").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            assert_eq!(
                arns, expected,
                "case {case:?}: the Region's own groups from that position on, \
                 got {body:?}"
            );
            assert!(
                body.get("nextToken").is_none(),
                "case {case:?}: a page that holds every remaining group names no next \
                 one, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a token this operation never issued is refused, \
                 got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(state::validation_message("nextToken", NEXT_TOKEN_REQUIREMENT).as_str()),
                "case {case:?}: the refusal states the member and the requirement, \
                 got {body:?}"
            );
            let token = row.token.clone().unwrap_or_default();
            assert!(
                token.is_empty() || !format!("{body}").contains(&token),
                "case {case:?}: the refusal of a sensitive token never echoes it, \
                 got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_token_is_served_only_where_it_names_a_stored_position() {
    let router = emulator();

    // An account holds `state::MAX_INVESTIGATION_GROUPS_PER_REGION` group per Region, so
    // each group below needs its own Region. The last Region deliberately holds none.
    let holding = "us-west-2";
    let elsewhere = "eu-central-1";
    let transient = "ap-southeast-4";
    let empty = "ca-central-1";

    let held = create_ok(&router, holding, "the-listed-group").await;
    let held_id = id_of(&held);
    let elsewhere_id = id_of(&create_ok(&router, elsewhere, "another-region-group").await);
    let deleted = create_ok(&router, transient, "since-deleted-group").await;
    let deleted_id = id_of(&deleted);

    // Delete that third group, so its id is a position its Region HELD and no longer
    // does: a token naming it is as forged as one naming an id that never existed.
    let (status, body) = call_in(
        &router,
        transient,
        "DELETE",
        &format!("/investigationGroups/{}", encode(&deleted)),
        None,
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "the delete must succeed, got {status} / {body:?}"
    );

    // Ids no group carries, each well formed by the rule the modelled
    // `InvestigationGroupArn` segment states (`[A-Za-z0-9]{16}`) and each landing in a
    // different place in the store's key order.
    let above_every_id = "z".repeat(state::GROUP_ID_LEN);
    let below_every_id = "0".repeat(state::GROUP_ID_LEN);
    let near_the_stored_id = id_with_one_character_changed(&held_id);

    let forged: Vec<(String, String)> = vec![
        (
            "a well-formed id sorting above every stored id".to_string(),
            above_every_id,
        ),
        (
            "a well-formed id sorting below every stored id".to_string(),
            below_every_id,
        ),
        (
            "the stored id with one character changed".to_string(),
            near_the_stored_id,
        ),
        (
            "a position another Region holds".to_string(),
            elsewhere_id.clone(),
        ),
        (
            "a position this Region held until it was deleted".to_string(),
            deleted_id,
        ),
    ];

    let mut rows: Vec<Row> = vec![
        Row {
            case: "no token at all".to_string(),
            token: None,
            served: true,
        },
        Row {
            case: "text that is not a token at all".to_string(),
            token: Some("not-a-token".to_string()),
            served: false,
        },
        Row {
            case: "hex of bytes that are not UTF-8".to_string(),
            token: Some(hex::encode([0xff_u8, 0xfe_u8])),
            served: false,
        },
    ];
    rows.extend(forged.into_iter().map(|(case, id)| Row {
        case: format!("{case} ({id})"),
        token: Some(hex::encode(&id)),
        served: false,
    }));

    // The Region that holds a group: only its own position is served.
    for row in &rows {
        assert_row(&router, holding, std::slice::from_ref(&held), row).await;
    }
    assert_row(
        &router,
        holding,
        std::slice::from_ref(&held),
        &Row {
            case: "the position this Region's own group holds".to_string(),
            token: Some(hex::encode(&held_id)),
            served: true,
        },
    )
    .await;

    // A Region that holds nothing has no position at all, so every token is refused —
    // including the one the Region next door serves — while a tokenless listing is the
    // empty page it really is.
    for row in &rows {
        let served_in_empty_region = row.token.is_none();
        assert_row(
            &router,
            empty,
            &[],
            &Row {
                case: format!("{} in a Region holding no group", row.case),
                token: row.token.clone(),
                served: served_in_empty_region,
            },
        )
        .await;
    }
    assert_row(
        &router,
        empty,
        &[],
        &Row {
            case: "a position the neighbouring Region holds".to_string(),
            token: Some(hex::encode(&held_id)),
            served: false,
        },
    )
    .await;

    // Refusing a token changes nothing: the Region still reports the group it held, so
    // no refused listing consumed or hid persisted state.
    let (status, body) = call_in(&router, holding, "GET", "/investigationGroups", None).await;
    assert_eq!(
        status, 200,
        "after the refusals the listing still answers, got {status} / {body:?}"
    );
    let arns: Vec<&str> = body
        .get("investigationGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no investigationGroups in {body:?}"))
        .iter()
        .filter_map(|item| item.get("arn").and_then(Value::as_str))
        .collect();
    assert_eq!(
        arns,
        vec![held.as_str()],
        "a refused token leaves the Region's listing exactly as it was, got {body:?}"
    );
}
