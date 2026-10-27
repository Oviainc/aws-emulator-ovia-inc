// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two rules this service decides in ONE place and
//! serves through several operations.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! 1. Which groups belong to the Region a request is signed for. The store is keyed by
//!    group id and read per Region, optionally resumed at an id, by
//!    `state::ComAmazonawsAiopsAiopsState::groups_in_region_from`. Both of its callers
//!    must agree: what `ListInvestigationGroups` reports in a Region, and what the
//!    per-region create quota counts there.
//! 2. What a group id looks like: 16 characters from `[A-Za-z0-9]`, the last segment of
//!    the modelled `InvestigationGroupArn`
//!    (`smithy_query(query="shape",shape="InvestigationGroupArn")`). The same
//!    `state::is_group_id` judgement decides whether an ARN addresses a group
//!    (`GetInvestigationGroup`) and whether a `nextToken` carries a group id
//!    (`ListInvestigationGroups`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{GROUP_ID_LEN, MAX_INVESTIGATION_GROUPS_PER_REGION};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error code an unresolvable identifier is reported with: this service's modelled
/// `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the TAGGING operations report an unresolvable `resourceArn` with:
/// the tagging frontend's own `NotFoundException` — see
/// `state::TAGGING_RESOURCE_NOT_FOUND`.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

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

/// Percent-encode everything a URI component may not carry literally — what an SDK
/// does for an `@httpLabel` path segment.
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
        .unwrap_or_else(|| {
            panic!("create {name:?} in {region}: response carried no arn, got {created:?}")
        })
        .to_string()
}

/// The ARNs `ListInvestigationGroups` reports in `region`, for the given `query`
/// (appended to the URI as-is), with the `nextToken` the page carried.
async fn listing(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
) -> (u16, Vec<String>, Option<String>, Value) {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups{query}"),
        None,
    )
    .await;
    if status != 200 {
        return (status, Vec::new(), None, body);
    }
    let arns = body
        .get("investigationGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("listing {region}{query}: no investigationGroups in {body:?}"))
        .iter()
        .filter_map(|item| item.get("arn").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, arns, token, body)
}

/// The id segment of an investigation-group ARN.
fn id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("an investigation-group ARN carries an id segment: {arn:?}"))
        .to_string()
}

/// An investigation-group ARN in `region` whose id segment is `id`, built like
/// `state::group_arn` does.
fn group_arn(region: &str, id: &str) -> String {
    format!("arn:aws:aiops:{region}:{ACCOUNT}:investigation-group/{id}")
}

/// One text a caller can present as a group id, either inside an ARN or (hex-encoded)
/// as a page token.
struct Candidate {
    /// How a failure names the row.
    case: &'static str,
    /// The text itself.
    text: String,
    /// Whether it has the shape of a group id: 16 characters of `[A-Za-z0-9]`.
    is_id_shaped: bool,
}

/// The texts the id-shape rule is asserted over: every published bound of the modelled
/// `[A-Za-z0-9]{16}` segment and one step past it (15 and 17 characters, the empty
/// text), each sub-alphabet the pattern admits (upper, lower, digits, mixed) and
/// characters it does not (hyphen, underscore, slash, space, a multi-byte character
/// that makes the BYTE length right and the text still not alphanumeric).
fn candidates() -> Vec<Candidate> {
    let shaped = |case: &'static str, text: String| Candidate {
        case,
        text,
        is_id_shaped: true,
    };
    let unshaped = |case: &'static str, text: String| Candidate {
        case,
        text,
        is_id_shaped: false,
    };
    vec![
        shaped("mixed case and digits", "aB3dE5fG7hJ9kL1m".to_string()),
        shaped("all upper case", "ABCDEFGHJKLMNPQR".to_string()),
        shaped("all lower case", "abcdefghjklmnpqr".to_string()),
        shaped("all digits", "0".repeat(GROUP_ID_LEN)),
        shaped("the largest id the alphabet admits", "z".repeat(GROUP_ID_LEN)),
        unshaped("one character short", "a".repeat(GROUP_ID_LEN - 1)),
        unshaped("one character long", "a".repeat(GROUP_ID_LEN + 1)),
        unshaped("empty", String::new()),
        unshaped("a hyphen among 16 characters", "aaaaaaa-aaaaaaaa".to_string()),
        unshaped(
            "an underscore among 16 characters",
            "aaaaaaa_aaaaaaaa".to_string(),
        ),
        unshaped("a slash among 16 characters", "aaaaaaa/aaaaaaaa".to_string()),
        unshaped("a space among 16 characters", "aaaaaaa aaaaaaaa".to_string()),
        // 15 characters, 16 BYTES: the length is right and the text is not alphanumeric.
        unshaped("a multi-byte character", format!("é{}", "a".repeat(14))),
    ]
}

/// Which groups belong to the Region a request is signed for is decided once, so the
/// listing of a Region and the per-region create quota always agree: a Region reports
/// exactly the groups it holds, a create is refused exactly where the listing is
/// already full, and the slot the refusal names is the one the listing shows.
///
/// Rows differ along the Region (geography, naming style and length), along whether the
/// Region already holds a group, and along the page size — each published bound of the
/// modelled `maxResults` range and the absent-parameter default.
#[tokio::test]
async fn the_region_a_listing_reports_is_the_region_the_quota_counts() {
    let router = emulator();

    // Regions spanning geographies and naming styles; those from `holding` on hold no
    // group when the table is first read.
    let regions = [
        "us-west-2",
        "eu-central-1",
        "ap-southeast-4",
        "us-gov-west-1",
        "ca-central-1",
        "ap-southeast-70",
    ];
    let holding = 4;

    let mut owned: Vec<Option<String>> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        owned.push(if index < holding {
            Some(create_ok(&router, region, &format!("group-{index:02}")).await)
        } else {
            None
        });
    }

    for query in ["", "?maxResults=1", "?maxResults=50"] {
        for (region, want) in regions.iter().zip(&owned) {
            let case = format!("{region} with query {query:?}");
            let (status, arns, token, body) = listing(&router, region, query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} / {body:?}"
            );
            let expected: Vec<String> = want.iter().cloned().collect();
            assert_eq!(
                arns, expected,
                "case {case:?}: a Region lists exactly the groups it holds, got {body:?}"
            );
            assert!(
                token.is_none(),
                "case {case:?}: a Region holding at most \
                 {MAX_INVESTIGATION_GROUPS_PER_REGION} group never fills a page, so no \
                 token may be issued, got {token:?}"
            );
        }
    }

    // The quota counts the same per-region set: a create is refused exactly in the
    // Regions whose listing is already full, and succeeds in the others.
    for (region, want) in regions.iter().zip(&owned) {
        let (status, answer) = call_in(
            &router,
            region,
            "POST",
            "/investigationGroups",
            Some(&json!({ "name": "second-in-region", "roleArn": ROLE_ARN })),
        )
        .await;
        match want {
            Some(held) => {
                assert_eq!(
                    status, 402,
                    "case {region:?}: a Region already holding a group refuses a second \
                     create, got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ServiceQuotaExceededException",
                    "case {region:?}: expected ServiceQuotaExceededException, got {answer:?}"
                );
                let (_, arns, _, body) = listing(&router, region, "").await;
                assert_eq!(
                    arns,
                    vec![held.clone()],
                    "case {region:?}: a refused create stores nothing, got {body:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 201,
                    "case {region:?}: a Region holding no group serves the create, \
                     got {status} / {answer:?}"
                );
                let created = answer
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {region:?}: no arn in {answer:?}"))
                    .to_string();
                let (_, arns, _, body) = listing(&router, region, "").await;
                assert_eq!(
                    arns,
                    vec![created],
                    "case {region:?}: the Region now lists the group it just took, \
                     got {body:?}"
                );
            }
        }
    }
}

/// What a group id looks like is one rule, applied wherever a caller presents text that
/// could name a group: the id segment of an ARN the resolver reads (the tagging
/// frontend's unconstrained `resourceArn`, and the resource surface's `identifier`,
/// whose modelled pattern states the same rule), and the `nextToken`
/// `ListInvestigationGroups` resumes a page at. Text that is not id-shaped addresses
/// nothing and is refused as a token; text that is id-shaped is accepted as a token and
/// addresses a group only when a group has that id.
#[tokio::test]
async fn one_rule_decides_what_a_group_id_looks_like() {
    let router = emulator();
    let region = "us-west-2";
    let arn = create_ok(&router, region, "the-only-group").await;
    let stored_id = id_of(&arn);
    assert_eq!(
        stored_id.len(),
        GROUP_ID_LEN,
        "a stored group's id is {GROUP_ID_LEN} characters, got {stored_id:?}"
    );

    for Candidate {
        case,
        text,
        is_id_shaped,
    } in candidates()
    {
        // The ARN resolver, reached through the tagging frontend's `resourceArn` — a
        // plain `String` the model constrains in no way
        // (`smithy_query(query="operation",operation="ListTagsForResource")`), so every
        // row reaches the handler and the id rule is what answers. No group carries any
        // of these ids, so each one addresses nothing.
        let candidate_arn = group_arn(region, &text);
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("/tags/{}", encode(&candidate_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: no group carries the id {text:?}, so the ARN addresses \
             nothing, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            TAGGING_NOT_FOUND,
            "case {case:?}: expected {TAGGING_NOT_FOUND}, got {body:?}"
        );

        // The resource surface's `identifier` carries the SAME rule in its modelled
        // pattern, so text that is not id-shaped is refused at the boundary (400) and
        // id-shaped text reaches the resolver and is reported absent (404).
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("/investigationGroups/{}", encode(&candidate_arn)),
            None,
        )
        .await;
        let (want_status, want_code) = if is_id_shaped {
            (404, RESOURCE_NOT_FOUND)
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            status, want_status,
            "case {case:?}: an ARN whose id segment is {text:?} answers {want_status}, \
             got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            want_code,
            "case {case:?}: expected {want_code}, got {body:?}"
        );

        // The page token: the same judgement decides whether the token carries a group
        // id at all. One that does not is refused; one that does resumes the Region's
        // walk at that id, so the Region's group is reported exactly when its id is not
        // before the token's.
        let token = hex::encode(&text);
        let (status, arns, next, body) =
            listing(&router, region, &format!("?nextToken={token}")).await;
        if is_id_shaped {
            assert_eq!(
                status, 200,
                "case {case:?}: a token carrying a group id is accepted, \
                 got {status} / {body:?}"
            );
            let expected: Vec<String> = if text.as_str() <= stored_id.as_str() {
                vec![arn.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                arns, expected,
                "case {case:?}: a page resumed at {text:?} reports the groups of \
                 {region} from that id on (stored id {stored_id:?}), got {body:?}"
            );
            assert!(
                next.is_none(),
                "case {case:?}: the resumed page is the last one, got {next:?}"
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
            assert!(
                text.is_empty() || !format!("{body}").contains(&text),
                "case {case:?}: the refusal of a sensitive token never echoes it, \
                 got {body:?}"
            );
        }
    }

    // The control: the stored group's own ARN still resolves, and its own id is a token
    // the Region's walk resumes at.
    let (status, body) = call_in(
        &router,
        region,
        "GET",
        &format!("/investigationGroups/{}", encode(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored group's own ARN resolves, got {status} / {body:?}"
    );
    let (status, arns, _, body) = listing(
        &router,
        region,
        &format!("?nextToken={}", hex::encode(&stored_id)),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token naming the stored id is accepted, got {status} / {body:?}"
    );
    assert_eq!(
        arns,
        vec![arn],
        "a page resumed at the stored id still reports that group, got {body:?}"
    );
}
