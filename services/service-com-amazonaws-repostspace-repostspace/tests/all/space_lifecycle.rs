// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the private re:Post *space* lifecycle: CreateSpace,
//! GetSpace, ListSpaces, UpdateSpace, DeleteSpace, plus the CreateChannel / GetChannel
//! pair the same store holds.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! `restJson1` request — so routing, deserialization, constraint validation,
//! the handler and serialization are all exercised, and state is read back through a
//! SEPARATE HTTP call.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_repostspace_repostspace::state::{
    BASIC_STORAGE_LIMIT, MAX_CHANNELS_PER_SPACE, MAX_SPACES_PER_ACCOUNT, PROVISIONING_SPAN,
    SPACE_DOMAIN_SUFFIX, SPACE_STATUS_CREATED, SPACE_STATUS_CREATING, STANDARD_STORAGE_LIMIT,
};
use service_com_amazonaws_repostspace_repostspace::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header naming this service (the routing key the emulator
/// reads) and the region every ARN below is expected to carry.
const REGION: &str = "us-west-2";
const AUTH: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/repostspace/aws4_request, SignedHeaders=host, Signature=0123456789abcdef";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator, one shared store — exactly what the production router builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these cases; the routing key still comes from
    // the `Authorization` header above.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Drive one request and return its status plus its parsed JSON body (`Null` when the
/// body is empty, as it is for the operations modelled with a `Unit` output).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{uri}"))
        .header("authorization", AUTH)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error body.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Create a space and return its generated id, asserting the create succeeded.
async fn create_space(router: &MultiServiceRouter, request_body: &str) -> String {
    let (status, body) = call(router, "POST", "/spaces", request_body).await;
    assert_eq!(
        status, 200,
        "case {request_body}: expected CreateSpace 200, got {status} {body}"
    );
    body.get("spaceId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {request_body}: CreateSpace returned no spaceId: {body}"))
        .to_string()
}

fn str_member(body: &serde_json::Value, member: &str) -> String {
    body.get(member)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("member {member} is absent or not a string in {body}"))
        .to_string()
}

/// A modelled timestamp member, as the wire carries it, asserted to be a real clock
/// reading.
///
/// The model declares `@timestampFormat("date-time")` on this service's `Timestamp`
/// shape, but the vendored schema codegen (`vendor/smithy-macros`, which a service crate
/// cannot change) emits no timestamp format, so the restJson1 default — epoch seconds —
/// is what reaches the wire. Both renderings are accepted here; what this issue owns is
/// that the value is a real creation instant, stored once and stable across reads.
fn timestamp_member(body: &serde_json::Value, member: &str) -> String {
    let value = body
        .get(member)
        .unwrap_or_else(|| panic!("member {member} is absent in {body}"));
    match value {
        serde_json::Value::Number(seconds) => {
            let seconds = seconds.as_f64().expect("a JSON number is an f64");
            // 2023-11-14T22:13:20Z — any emulator run is later than this.
            assert!(
                seconds > 1_700_000_000.0,
                "member {member} must be a real clock reading, got {seconds}"
            );
            value.to_string()
        }
        serde_json::Value::String(rendered) => {
            assert!(
                rendered.contains('T') && rendered.len() >= 20,
                "member {member} must be a real date-time, got {rendered:?}"
            );
            rendered.clone()
        }
        other => panic!("member {member} must be a timestamp, got {other}"),
    }
}

/// One row of the create/read table: the inputs that distinguish it, and what the
/// tier implies.
struct CreateCase {
    label: &'static str,
    name: &'static str,
    subdomain: &'static str,
    tier: &'static str,
    description: Option<&'static str>,
    request: String,
    expected_storage_limit: i64,
}

/// RULE: CreateSpace mints an id, derives and PERSISTS the whole record, and GetSpace
/// answers from that record — over the whole class of accepted inputs, not one of them.
#[tokio::test]
async fn create_space_persists_a_record_get_space_reports_verbatim() {
    // The rows differ along everything the rule generalises over: tier (both members of
    // `TierLevel`), presence of the optional members, name and subdomain length and
    // shape (1-char through the 30/63-char modelled maxima), tag count.
    let long_name = "n".repeat(30);
    let long_subdomain = "s".repeat(63);
    let long_description = "d".repeat(255);
    let long_request = format!(
        r#"{{"name":"{long_name}","subdomain":"{long_subdomain}","tier":"STANDARD","description":"{long_description}"}}"#
    );
    let cases: Vec<CreateCase> = vec![
        CreateCase {
            label: "basic tier, every optional member supplied",
            name: "demo",
            subdomain: "demo-co",
            tier: "BASIC",
            description: Some("bootstrap demo"),
            request: r#"{"name":"demo","subdomain":"demo-co","tier":"BASIC","description":"bootstrap demo","userKMSKey":"arn:aws:kms:us-west-2:000000000000:key/abcd","roleArn":"arn:aws:iam::000000000000:role/support","tags":{"team":"platform","env":"test"},"supportedEmailDomains":{"enabled":"ENABLED","allowedDomains":["example.com"]}}"#
                .to_string(),
            expected_storage_limit: BASIC_STORAGE_LIMIT,
        },
        CreateCase {
            label: "standard tier, only the required members",
            name: "s",
            subdomain: "s",
            tier: "STANDARD",
            description: None,
            request: r#"{"name":"s","subdomain":"s","tier":"STANDARD"}"#.to_string(),
            expected_storage_limit: STANDARD_STORAGE_LIMIT,
        },
        CreateCase {
            label: "members at the modelled length maxima",
            name: "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnn",
            subdomain: "sssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss",
            tier: "STANDARD",
            description: Some(
                "ddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            ),
            request: long_request,
            expected_storage_limit: STANDARD_STORAGE_LIMIT,
        },
    ];

    let router = emulator();
    for case in &cases {
        let space_id = create_space(&router, &case.request).await;
        assert!(
            !space_id.is_empty(),
            "case {}: the minted spaceId must not be empty",
            case.label
        );

        // A SEPARATE request reads the record back.
        let (status, body) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
        assert_eq!(
            status, 200,
            "case {}: expected GetSpace 200, got {status} {body}",
            case.label
        );

        assert_eq!(
            str_member(&body, "spaceId"),
            space_id,
            "case {}: GetSpace must report the id CreateSpace minted",
            case.label
        );
        assert_eq!(
            str_member(&body, "name"),
            case.name,
            "case {}: name must round-trip",
            case.label
        );
        assert_eq!(
            body.get("description").and_then(serde_json::Value::as_str),
            case.description,
            "case {}: description must round-trip (absent when not supplied)",
            case.label
        );
        assert_eq!(
            str_member(&body, "tier"),
            case.tier,
            "case {}: tier must round-trip",
            case.label
        );
        assert_eq!(
            body.get("storageLimit").and_then(serde_json::Value::as_i64),
            Some(case.expected_storage_limit),
            "case {}: storageLimit must follow the tier, got {body}",
            case.label
        );
        assert_eq!(
            str_member(&body, "arn"),
            format!("arn:aws:repostspace:{REGION}:{ACCOUNT_ID}:space/{space_id}"),
            "case {}: the ARN must be derived from the region, account and space id",
            case.label
        );
        assert_eq!(
            str_member(&body, "vanityDomain"),
            format!("{}.{SPACE_DOMAIN_SUFFIX}", case.subdomain),
            "case {}: vanityDomain must come from the requested subdomain",
            case.label
        );
        let random_domain = str_member(&body, "randomDomain");
        assert!(
            random_domain.ends_with(&format!(".{SPACE_DOMAIN_SUFFIX}"))
                && random_domain != str_member(&body, "vanityDomain"),
            "case {}: randomDomain must be an AWS-generated subdomain, got {random_domain:?}",
            case.label
        );
        assert_eq!(
            str_member(&body, "vanityDomainStatus"),
            "PENDING",
            "case {}: a custom subdomain starts out awaiting AWS approval",
            case.label
        );
        assert_eq!(
            str_member(&body, "configurationStatus"),
            "UNCONFIGURED",
            "case {}: a space with no administrators yet is UNCONFIGURED",
            case.label
        );
        assert!(
            !str_member(&body, "clientId").is_empty(),
            "case {}: clientId must be a real generated value, got {body}",
            case.label
        );
        let create_date_time = timestamp_member(&body, "createDateTime");
        assert!(
            !create_date_time.is_empty(),
            "case {}: createDateTime must be populated, got {body}",
            case.label
        );
        let status_value = str_member(&body, "status");
        assert!(
            status_value == SPACE_STATUS_CREATING || status_value == SPACE_STATUS_CREATED,
            "case {}: status must be a provisioning status, got {status_value:?}",
            case.label
        );
    }

    // Each row minted a DISTINCT id and the store holds all of them.
    let (status, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
    assert_eq!(status, 200, "ListSpaces must succeed, got {status} {body}");
    let ids: BTreeSet<String> = body["spaces"]
        .as_array()
        .expect("spaces is a list")
        .iter()
        .map(|space| str_member(space, "spaceId"))
        .collect();
    assert_eq!(
        ids.len(),
        cases.len(),
        "every created space must be stored exactly once, got {body}"
    );
}

/// RULE: a reporting call never changes what a later caller sees, except to materialise
/// the provisioning transition the clock already owed.
#[tokio::test]
async fn provisioning_status_settles_on_the_clock_not_on_the_read() {
    let router = emulator();
    // Started BEFORE the create, so `started.elapsed()` is never shorter than the time
    // since the space's own due instant was set: when it is still inside the span the
    // transition is definitely not due yet, whatever the machine's load.
    let started = std::time::Instant::now();
    let space_id = create_space(
        &router,
        r#"{"name":"settling","subdomain":"settling","tier":"BASIC"}"#,
    )
    .await;

    // Inside the span the transition is not due, so repeated reads all report the
    // transient status — a read does not bring it forward.
    for attempt in 0..2 {
        let (_, body) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
        let status = str_member(&body, "status");
        if started.elapsed() < PROVISIONING_SPAN {
            assert_eq!(
                status, SPACE_STATUS_CREATING,
                "case read #{attempt} inside the provisioning span: expected {SPACE_STATUS_CREATING}, got {body}"
            );
        } else {
            assert_eq!(
                status, SPACE_STATUS_CREATED,
                "case read #{attempt} after the provisioning span: expected {SPACE_STATUS_CREATED}, got {body}"
            );
        }
    }

    // Once the span has elapsed, the status the clock owed is materialised.
    tokio::time::sleep(PROVISIONING_SPAN + std::time::Duration::from_millis(200)).await;
    let (_, body) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
    assert_eq!(
        str_member(&body, "status"),
        SPACE_STATUS_CREATED,
        "case read after the span elapsed: expected {SPACE_STATUS_CREATED}, got {body}"
    );
}

/// RULE: UpdateSpace mutates the stored record in place — a supplied member replaces the
/// stored value (and `storageLimit` follows a changed `tier`), an absent one leaves it
/// alone, and the members the operation does not accept never move.
#[tokio::test]
async fn update_space_replaces_only_the_members_it_is_given() {
    struct UpdateCase {
        label: &'static str,
        request: &'static str,
        expected_description: Option<&'static str>,
        expected_tier: &'static str,
        expected_storage_limit: i64,
        expected_role_arn: Option<&'static str>,
    }

    // The rows differ along which members are present: none, each one alone, and all
    // together — the dimension the partial-update rule generalises over.
    let cases = [
        UpdateCase {
            label: "no member supplied leaves the record untouched",
            request: "{}",
            expected_description: Some("original"),
            expected_tier: "BASIC",
            expected_storage_limit: BASIC_STORAGE_LIMIT,
            expected_role_arn: Some("arn:aws:iam::000000000000:role/original"),
        },
        UpdateCase {
            label: "description alone",
            request: r#"{"description":"updated"}"#,
            expected_description: Some("updated"),
            expected_tier: "BASIC",
            expected_storage_limit: BASIC_STORAGE_LIMIT,
            expected_role_arn: Some("arn:aws:iam::000000000000:role/original"),
        },
        UpdateCase {
            label: "tier alone, upgrading",
            request: r#"{"tier":"STANDARD"}"#,
            expected_description: Some("original"),
            expected_tier: "STANDARD",
            expected_storage_limit: STANDARD_STORAGE_LIMIT,
            expected_role_arn: Some("arn:aws:iam::000000000000:role/original"),
        },
        UpdateCase {
            label: "roleArn alone",
            request: r#"{"roleArn":"arn:aws:iam::000000000000:role/replacement"}"#,
            expected_description: Some("original"),
            expected_tier: "BASIC",
            expected_storage_limit: BASIC_STORAGE_LIMIT,
            expected_role_arn: Some("arn:aws:iam::000000000000:role/replacement"),
        },
        UpdateCase {
            label: "every member at once",
            request: r#"{"description":"everything","tier":"STANDARD","roleArn":"arn:aws:iam::000000000000:role/everything","supportedEmailDomains":{"enabled":"DISABLED"}}"#,
            expected_description: Some("everything"),
            expected_tier: "STANDARD",
            expected_storage_limit: STANDARD_STORAGE_LIMIT,
            expected_role_arn: Some("arn:aws:iam::000000000000:role/everything"),
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        // A fresh emulator per row, so each row starts from the same stored record.
        let router = emulator();
        let create = format!(
            r#"{{"name":"space-{row}","subdomain":"space-{row}","tier":"BASIC","description":"original","roleArn":"arn:aws:iam::000000000000:role/original"}}"#
        );
        let space_id = create_space(&router, &create).await;
        let (_, before) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/spaces/{space_id}"),
            case.request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected UpdateSpace 200, got {status} {body}",
            case.label
        );
        assert_eq!(
            body,
            serde_json::Value::Null,
            "case {}: UpdateSpace has an empty output body, got {body}",
            case.label
        );

        let (_, after) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
        assert_eq!(
            after.get("description").and_then(serde_json::Value::as_str),
            case.expected_description,
            "case {}: description after the update, got {after}",
            case.label
        );
        assert_eq!(
            str_member(&after, "tier"),
            case.expected_tier,
            "case {}: tier after the update, got {after}",
            case.label
        );
        assert_eq!(
            after.get("storageLimit").and_then(serde_json::Value::as_i64),
            Some(case.expected_storage_limit),
            "case {}: storageLimit must follow the tier, got {after}",
            case.label
        );
        assert_eq!(
            after
                .get("customerRoleArn")
                .and_then(serde_json::Value::as_str),
            case.expected_role_arn,
            "case {}: customerRoleArn after the update, got {after}",
            case.label
        );
        // The identity of the space never moves on an update.
        for member in ["spaceId", "name", "arn", "vanityDomain"] {
            assert_eq!(
                str_member(&after, member),
                str_member(&before, member),
                "case {}: {member} must be unchanged by UpdateSpace",
                case.label
            );
        }
        assert_eq!(
            timestamp_member(&after, "createDateTime"),
            timestamp_member(&before, "createDateTime"),
            "case {}: createDateTime must be unchanged by UpdateSpace",
            case.label
        );
    }
}

/// RULE: ListSpaces reports one entry per stored space, ordered stably, capped at
/// `maxResults`, resumable through the opaque `nextToken`, with no token on the last
/// page — whatever the space count and page size are.
#[tokio::test]
async fn list_spaces_pages_over_every_stored_space() {
    // Rows differ along the two dimensions the pagination rule generalises over: how
    // many spaces are stored and how large a page is asked for (1 — the modelled
    // minimum, a size that divides the count, one that does not, and one larger than
    // the count).
    let cases = [(1usize, 1i32), (3, 1), (4, 2), (5, 3), (2, 100)];

    for (total, page_size) in cases {
        let label = format!("{total} spaces, maxResults={page_size}");
        let router = emulator();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            created.insert(
                create_space(
                    &router,
                    &format!(
                        r#"{{"name":"space-{index}","subdomain":"sub-{index}","tier":"BASIC"}}"#
                    ),
                )
                .await,
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {label}: pagination did not terminate after {pages} pages"
            );
            let uri = match token.as_deref() {
                Some(next) => format!("/spaces?maxResults={page_size}&nextToken={next}"),
                None => format!("/spaces?maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &uri, "").await;
            assert_eq!(
                status, 200,
                "case {label}: expected ListSpaces 200, got {status} {body}"
            );
            let page = body["spaces"].as_array().expect("spaces is a list").clone();
            assert!(
                page.len() <= page_size as usize,
                "case {label}: a page must hold at most maxResults entries, got {}",
                page.len()
            );
            for space in &page {
                // Every entry agrees with GetSpace for the same space.
                let space_id = str_member(space, "spaceId");
                let (_, one) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
                for member in [
                    "arn",
                    "name",
                    "status",
                    "configurationStatus",
                    "vanityDomainStatus",
                    "vanityDomain",
                    "randomDomain",
                    "tier",
                ] {
                    assert_eq!(
                        str_member(space, member),
                        str_member(&one, member),
                        "case {label}: {member} must agree between ListSpaces and GetSpace"
                    );
                }
                assert_eq!(
                    timestamp_member(space, "createDateTime"),
                    timestamp_member(&one, "createDateTime"),
                    "case {label}: createDateTime must agree between ListSpaces and GetSpace"
                );
                assert_eq!(
                    space.get("storageLimit"),
                    one.get("storageLimit"),
                    "case {label}: storageLimit must agree between ListSpaces and GetSpace"
                );
                seen.push(space_id);
            }
            match body.get("nextToken").and_then(serde_json::Value::as_str) {
                Some(next) => {
                    assert_eq!(
                        page.len(),
                        page_size as usize,
                        "case {label}: a page carrying a token must be full, got {body}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {label}: no space may be reported twice, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {label}: paging must report every stored space exactly once"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {label}: spaces must be reported in a stable id order"
        );
    }
}

/// RULE: a token this service did not issue is refused as a `ValidationException` (400),
/// not served as a page.
#[tokio::test]
async fn an_unissued_pagination_token_is_refused() {
    let router = emulator();
    create_space(
        &router,
        r#"{"name":"token","subdomain":"token","tier":"BASIC"}"#,
    )
    .await;
    // Rows: not hex at all, hex of a payload without the issued prefix, hex of the
    // prefix with an empty position.
    for token in ["not-a-token", "6162636465", "7370616365732f76313a"] {
        let (status, body) = call(&router, "GET", &format!("/spaces?nextToken={token}"), "").await;
        assert_eq!(
            status, 400,
            "case token={token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case token={token:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE: a space's name and its subdomain each identify it, so a create that reuses
/// either one of a stored space is a `ConflictException` (409).
#[tokio::test]
async fn a_create_reusing_a_name_or_a_subdomain_conflicts() {
    let router = emulator();
    create_space(
        &router,
        r#"{"name":"taken","subdomain":"taken-co","tier":"BASIC"}"#,
    )
    .await;

    let conflicting = [
        (
            "same subdomain, different name",
            r#"{"name":"other","subdomain":"taken-co","tier":"BASIC"}"#,
        ),
        (
            "same name, different subdomain",
            r#"{"name":"taken","subdomain":"other-co","tier":"STANDARD"}"#,
        ),
        (
            "same name and subdomain",
            r#"{"name":"taken","subdomain":"taken-co","tier":"STANDARD"}"#,
        ),
    ];
    for (label, request) in conflicting {
        let (status, body) = call(&router, "POST", "/spaces", request).await;
        assert_eq!(
            status, 409,
            "case {label}: expected 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label}: expected ConflictException, got {body}"
        );
    }

    // A create that reuses neither still succeeds, and the conflicting attempts stored
    // nothing.
    create_space(
        &router,
        r#"{"name":"fresh","subdomain":"fresh-co","tier":"BASIC"}"#,
    )
    .await;
    let (_, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
    assert_eq!(
        body["spaces"].as_array().expect("spaces is a list").len(),
        2,
        "a refused create must store nothing, got {body}"
    );
}

/// RULE: an operation addressing a space (or a channel) that the store does not hold
/// answers `ResourceNotFoundException` (404) — never a 501 and never an empty 200.
#[tokio::test]
async fn an_unknown_id_is_a_404_on_every_implemented_operation() {
    let router = emulator();
    let live = create_space(
        &router,
        r#"{"name":"live","subdomain":"live-co","tier":"BASIC"}"#,
    )
    .await;

    let cases: Vec<(&str, &str, String, &str)> = vec![
        ("GetSpace", "GET", "/spaces/does-not-exist".to_string(), ""),
        ("UpdateSpace", "PUT", "/spaces/does-not-exist".to_string(), r#"{"description":"x"}"#),
        ("DeleteSpace", "DELETE", "/spaces/does-not-exist".to_string(), ""),
        (
            "CreateChannel on an unknown space",
            "POST",
            "/spaces/does-not-exist/channels".to_string(),
            r#"{"channelName":"general"}"#,
        ),
        (
            "GetChannel on an unknown space",
            "GET",
            "/spaces/does-not-exist/channels/aaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "",
        ),
        (
            "GetChannel on an unknown channel of a live space",
            "GET",
            format!("/spaces/{live}/channels/aaaaaaaaaaaaaaaaaaaaaaaa"),
            "",
        ),
    ];

    for (label, method, uri, body) in cases {
        let (status, response) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {label}: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException, got {response}"
        );
    }
}

/// RULE: a request outside a modelled constraint is refused with a 400
/// `ValidationException` and stores nothing.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = emulator();

    // The catalogue of constraints comes from the model (`smithy_query` on CreateSpace /
    // ListSpaces): name len 1..30, subdomain len 1..63, description len 1..255,
    // tier one of TierLevel, maxResults range 1..100. Each is tested one past a bound.
    let too_long_name = "n".repeat(31);
    let too_long_subdomain = "s".repeat(64);
    let too_long_description = "d".repeat(256);
    let create_cases = [
        (
            "name below the minimum length",
            r#"{"name":"","subdomain":"ok-co","tier":"BASIC"}"#.to_string(),
        ),
        (
            "name one past the maximum length",
            format!(r#"{{"name":"{too_long_name}","subdomain":"ok-co","tier":"BASIC"}}"#),
        ),
        (
            "subdomain below the minimum length",
            r#"{"name":"ok","subdomain":"","tier":"BASIC"}"#.to_string(),
        ),
        (
            "subdomain one past the maximum length",
            format!(r#"{{"name":"ok","subdomain":"{too_long_subdomain}","tier":"BASIC"}}"#),
        ),
        (
            "description one past the maximum length",
            format!(
                r#"{{"name":"ok","subdomain":"ok-co","tier":"BASIC","description":"{too_long_description}"}}"#
            ),
        ),
        (
            "tier outside TierLevel",
            r#"{"name":"ok","subdomain":"ok-co","tier":"GOLD"}"#.to_string(),
        ),
    ];
    for (label, request) in &create_cases {
        let (status, body) = call(&router, "POST", "/spaces", request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected ValidationException, got {body}"
        );
    }

    // `maxResults` range 1..100: one below and one above, plus both published bounds,
    // which are accepted.
    for (label, max_results, expected) in [
        ("one below the minimum", "0", 400),
        ("the published minimum", "1", 200),
        ("the published maximum", "100", 200),
        ("one above the maximum", "101", 400),
    ] {
        let (status, body) =
            call(&router, "GET", &format!("/spaces?maxResults={max_results}"), "").await;
        assert_eq!(
            status, expected,
            "case maxResults {label}: expected {expected}, got {status} {body}"
        );
    }

    // An update outside a constraint is refused too, and leaves the record alone.
    let space_id = create_space(
        &router,
        r#"{"name":"keeper","subdomain":"keeper-co","tier":"BASIC"}"#,
    )
    .await;
    for (label, request) in [
        (
            "update tier outside TierLevel",
            r#"{"tier":"PLATINUM"}"#.to_string(),
        ),
        (
            "update description one past the maximum length",
            format!(r#"{{"description":"{too_long_description}"}}"#),
        ),
    ] {
        let (status, body) =
            call(&router, "PUT", &format!("/spaces/{space_id}"), &request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected ValidationException, got {body}"
        );
    }
    let (_, after) = call(&router, "GET", &format!("/spaces/{space_id}"), "").await;
    assert_eq!(
        str_member(&after, "tier"),
        "BASIC",
        "a refused update must leave the stored record alone, got {after}"
    );
    assert_eq!(
        after.get("description"),
        None,
        "a refused update must leave the stored record alone, got {after}"
    );

    // Nothing the refused creates asked for was stored.
    let (_, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
    assert_eq!(
        body["spaces"].as_array().expect("spaces is a list").len(),
        1,
        "a refused create must store nothing, got {body}"
    );
}

/// RULE: DeleteSpace forgets the record, so every later read of it is a 404 and it is
/// gone from the listing — while the spaces it did not name stay exactly as they were.
#[tokio::test]
async fn delete_space_removes_only_the_space_it_names() {
    // Rows: delete the first, the middle and the last of three spaces, so the rule is
    // checked wherever the deleted space sits in the id order.
    for victim in 0..3usize {
        let label = format!("delete space #{victim} of 3");
        let router = emulator();
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(
                create_space(
                    &router,
                    &format!(
                        r#"{{"name":"space-{index}","subdomain":"sub-{index}","tier":"BASIC"}}"#
                    ),
                )
                .await,
            );
        }
        ids.sort();
        let deleted = ids[victim].clone();

        let (status, body) = call(&router, "DELETE", &format!("/spaces/{deleted}"), "").await;
        assert_eq!(
            status, 200,
            "case {label}: expected DeleteSpace 200, got {status} {body}"
        );
        assert_eq!(
            body,
            serde_json::Value::Null,
            "case {label}: DeleteSpace has an empty output body, got {body}"
        );

        let (status, body) = call(&router, "GET", &format!("/spaces/{deleted}"), "").await;
        assert_eq!(
            status, 404,
            "case {label}: a deleted space must be gone, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException, got {body}"
        );

        // Deleting it twice is a 404 as well: the record is gone, not merely hidden.
        let (status, _) = call(&router, "DELETE", &format!("/spaces/{deleted}"), "").await;
        assert_eq!(
            status, 404,
            "case {label}: a second delete must be a 404, got {status}"
        );

        let (_, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
        let remaining: Vec<String> = body["spaces"]
            .as_array()
            .expect("spaces is a list")
            .iter()
            .map(|space| str_member(space, "spaceId"))
            .collect();
        let expected: Vec<String> = ids
            .iter()
            .filter(|id| **id != deleted)
            .cloned()
            .collect();
        assert_eq!(
            remaining, expected,
            "case {label}: only the named space may disappear from the listing"
        );
        for id in &expected {
            let (status, _) = call(&router, "GET", &format!("/spaces/{id}"), "").await;
            assert_eq!(
                status, 200,
                "case {label}: space {id} must still be readable"
            );
        }
    }
}

/// RULE: CreateChannel mints a 24-character id under its parent space and PERSISTS the
/// channel there, so GetChannel answers from the stored record.
#[tokio::test]
async fn create_channel_persists_a_record_get_channel_reports_verbatim() {
    let router = emulator();
    let space_id = create_space(
        &router,
        r#"{"name":"channels","subdomain":"channels-co","tier":"STANDARD"}"#,
    )
    .await;

    // Rows differ along the optional description and the name's length (1 char through
    // the modelled 64-char maximum).
    let long_name = "c".repeat(64);
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "name and description",
            r#"{"channelName":"general","channelDescription":"everything else"}"#.to_string(),
            Some("everything else"),
        ),
        (
            "name only",
            r#"{"channelName":"q"}"#.to_string(),
            None,
        ),
        (
            "name at the modelled maximum length",
            format!(r#"{{"channelName":"{long_name}"}}"#),
            None,
        ),
    ];

    let mut channel_ids = BTreeSet::new();
    for (label, request, expected_description) in &cases {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/spaces/{space_id}/channels"),
            request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected CreateChannel 200, got {status} {body}"
        );
        let channel_id = str_member(&body, "channelId");
        assert_eq!(
            channel_id.len(),
            24,
            "case {label}: channelId must be exactly 24 characters, got {channel_id:?}"
        );
        assert!(
            channel_ids.insert(channel_id.clone()),
            "case {label}: channelId {channel_id} was minted twice"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/spaces/{space_id}/channels/{channel_id}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected GetChannel 200, got {status} {body}"
        );
        assert_eq!(
            str_member(&body, "spaceId"),
            space_id,
            "case {label}: the channel must report its parent space"
        );
        assert_eq!(
            str_member(&body, "channelId"),
            channel_id,
            "case {label}: GetChannel must report the id CreateChannel minted"
        );
        let expected_name: String = serde_json::from_str::<serde_json::Value>(request)
            .expect("the request body is JSON")["channelName"]
            .as_str()
            .expect("channelName is a string")
            .to_string();
        assert_eq!(
            str_member(&body, "channelName"),
            expected_name,
            "case {label}: channelName must round-trip"
        );
        assert_eq!(
            body.get("channelDescription")
                .and_then(serde_json::Value::as_str),
            *expected_description,
            "case {label}: channelDescription must round-trip (absent when not supplied), got {body}"
        );
        let channel_status = str_member(&body, "channelStatus");
        assert!(
            channel_status == "CREATING" || channel_status == "CREATED",
            "case {label}: channelStatus must be a ChannelStatus, got {channel_status:?}"
        );
        let created = timestamp_member(&body, "createDateTime");
        assert!(
            !created.is_empty(),
            "case {label}: createDateTime must be populated, got {body}"
        );
    }

    // A second channel reusing a stored channel's name conflicts.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/spaces/{space_id}/channels"),
        r#"{"channelName":"general"}"#,
    )
    .await;
    assert_eq!(
        status, 409,
        "a channel name already used in the space must conflict, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body}"
    );

    // Deleting the space takes its channels with it.
    let any_channel = channel_ids.iter().next().expect("a channel was created");
    call(&router, "DELETE", &format!("/spaces/{space_id}"), "").await;
    let (status, body) = call(
        &router,
        "GET",
        &format!("/spaces/{space_id}/channels/{any_channel}"),
        "",
    )
    .await;
    assert_eq!(
        status, 404,
        "a channel of a deleted space must be gone, got {status} {body}"
    );
}

/// RULE: a create that would take a store past its quota is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) and stores nothing, so the record count is
/// bounded however many creates a caller makes.
#[tokio::test]
async fn a_create_past_the_quota_is_refused() {
    let router = emulator();
    for index in 0..MAX_SPACES_PER_ACCOUNT {
        create_space(
            &router,
            &format!(r#"{{"name":"space-{index}","subdomain":"sub-{index}","tier":"BASIC"}}"#),
        )
        .await;
    }

    // Rows: the first create past the quota, and a second one — the refusal is the
    // steady state, not a one-off.
    for attempt in 0..2 {
        let (status, body) = call(
            &router,
            "POST",
            "/spaces",
            &format!(
                r#"{{"name":"over-{attempt}","subdomain":"over-{attempt}","tier":"BASIC"}}"#
            ),
        )
        .await;
        assert_eq!(
            status, 402,
            "case create past the quota, attempt {attempt}: expected 402, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case create past the quota, attempt {attempt}: expected ServiceQuotaExceededException, got {body}"
        );
    }

    // The store still holds exactly the quota, and deleting one makes room for exactly
    // one more.
    let (_, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
    let stored = body["spaces"].as_array().expect("spaces is a list");
    assert_eq!(
        stored.len(),
        MAX_SPACES_PER_ACCOUNT,
        "a refused create must store nothing"
    );
    let victim = str_member(&stored[0], "spaceId");
    let (status, _) = call(&router, "DELETE", &format!("/spaces/{victim}"), "").await;
    assert_eq!(status, 200, "case delete to make room: expected 200, got {status}");
    create_space(
        &router,
        r#"{"name":"room","subdomain":"room-co","tier":"BASIC"}"#,
    )
    .await;

    // The same rule holds for a space's channels.
    let (_, body) = call(&router, "GET", "/spaces?maxResults=100", "").await;
    let space_id = str_member(
        &body["spaces"].as_array().expect("spaces is a list")[0],
        "spaceId",
    );
    for index in 0..MAX_CHANNELS_PER_SPACE {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/spaces/{space_id}/channels"),
            &format!(r#"{{"channelName":"channel-{index}"}}"#),
        )
        .await;
        assert_eq!(
            status, 200,
            "case channel {index} within the quota: expected 200, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "POST",
        &format!("/spaces/{space_id}/channels"),
        r#"{"channelName":"one-too-many"}"#,
    )
    .await;
    assert_eq!(
        status, 402,
        "case channel past the quota: expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case channel past the quota: expected ServiceQuotaExceededException, got {body}"
    );
}

/// RULE: the operations this slice does not implement still answer 501, not an empty
/// 200 — the 501 is the honest "not written yet" answer.
#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_501() {
    let router = emulator();
    let space_id = create_space(
        &router,
        r#"{"name":"stubs","subdomain":"stubs-co","tier":"BASIC"}"#,
    )
    .await;

    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            "ListChannels",
            "GET",
            format!("/spaces/{space_id}/channels"),
            "",
        ),
        (
            "RegisterAdmin",
            "POST",
            format!("/spaces/{space_id}/admins/admin-1"),
            "",
        ),
        (
            "BatchAddRole",
            "POST",
            format!("/spaces/{space_id}/roles"),
            r#"{"accessorIds":["a"],"role":"EXPERT"}"#,
        ),
    ];
    for (label, method, uri, body) in cases {
        let (status, response) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 501,
            "case {label}: an unimplemented operation must answer 501, got {status} {response}"
        );
    }
}
