// SPDX-License-Identifier: CC0-1.0

//! The application (project) + endpoint lifecycle, end to end over HTTP.
//!
//! Every case drives the real restJson1 wire: the request is dispatched through the
//! `MultiServiceRouter` the production front door uses, so routing, deserialization,
//! the handler, serialization and the HTTP status are all exercised.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a created resource is readable back, byte-identical, through its Get, and is gone
//!   from both the Get and the List after its Delete;
//! * an identifier the service mints is unique per call and formatted the way the
//!   service's ids are, and the ARN is built from the request's own signing region;
//! * a page of a paginated list is a prefix of one deterministic order, and the
//!   `NextToken` resumes exactly where the page stopped — absent on the last page;
//! * an endpoint write persists every member it carries and keeps the members it
//!   omits, and only a `ChannelType` the model lists is accepted.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A mounted emulator holding one shared engine, exactly as the router builds it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: it asserts service behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_pinpoint_pinpoint::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed (nominally) for `region`, and return the status
    /// plus the parsed JSON body (`Value::Null` for an empty body).
    async fn call(
        &self,
        method: &str,
        uri: &str,
        region: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |v| {
            Bytes::from(serde_json::to_vec(&v).expect("a test body always serializes"))
        });
        let request = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The routing key: Pinpoint's sigv4 signing name is `mobiletargeting`.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/mobiletargeting/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn create_app(&self, region: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/v1/apps", region, Some(body)).await
    }

    async fn get_app(&self, region: &str, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/apps/{id}"), region, None).await
    }

    async fn delete_app(&self, region: &str, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/v1/apps/{id}"), region, None)
            .await
    }

    async fn get_apps(&self, region: &str, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/v1/apps".to_string()
        } else {
            format!("/v1/apps?{query}")
        };
        self.call("GET", &uri, region, None).await
    }

    async fn update_endpoint(
        &self,
        region: &str,
        app: &str,
        endpoint: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/v1/apps/{app}/endpoints/{endpoint}"),
            region,
            Some(body),
        )
        .await
    }

    async fn get_endpoint(&self, region: &str, app: &str, endpoint: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/v1/apps/{app}/endpoints/{endpoint}"),
            region,
            None,
        )
        .await
    }
}

/// The modelled error code carried by a restJson1 error body (`__type`).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The 32 lowercase hex characters a Pinpoint `ApplicationId` is made of.
///
/// Deliberately an INDEPENDENT restatement of `crate::state::is_application_id`: the
/// test states the format it expects on the wire rather than reusing the code under
/// test, so a change to the minting rule has to be made here too.
fn is_application_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One row of the application table: what a client sends, and what it must read back.
struct AppCase {
    label: &'static str,
    region: &'static str,
    name: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// The application table — the rows differ along everything the rules generalise
/// over: the display name's content and length, the number of tags (none, one,
/// several), and the signing region the ARN is built from.
fn app_cases() -> Vec<AppCase> {
    vec![
        AppCase {
            label: "plain name, one tag",
            region: "us-east-1",
            name: "demo-project",
            tags: vec![("env", "test")],
        },
        AppCase {
            label: "no tags at all",
            region: "us-west-2",
            name: "no-tags",
            tags: Vec::new(),
        },
        AppCase {
            label: "several tags, non-us region",
            region: "eu-west-1",
            name: "Marketing Project 2026",
            tags: vec![("env", "prod"), ("team", "growth"), ("cost-centre", "42")],
        },
        AppCase {
            label: "non-ascii display name",
            region: "ap-northeast-1",
            name: "キャンペーン",
            tags: vec![("owner", "日本")],
        },
        AppCase {
            label: "long display name",
            region: "us-east-2",
            name: "a-very-long-application-display-name-that-a-client-may-well-choose",
            tags: vec![("k", "")],
        },
    ]
}

fn tags_value(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect(),
    )
}

#[tokio::test]
async fn an_application_is_created_read_back_listed_and_deleted() {
    let emulator = Emulator::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created: Vec<(String, Value, &'static str)> = Vec::new();

    for case in app_cases() {
        let request = json!({ "Name": case.name, "tags": tags_value(&case.tags) });
        let (status, created_body) = emulator.create_app(case.region, request).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected CreateApp 201, got {status} with {created_body:?}",
            case.label
        );

        let id = created_body["Id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_application_id(&id),
            "case {:?}: expected a 32-char lowercase-hex Id, got {:?}",
            case.label,
            created_body["Id"]
        );
        assert!(
            minted.insert(id.clone()),
            "case {:?}: CreateApp minted an Id it had already handed out: {id:?}",
            case.label
        );
        assert_eq!(
            created_body["Name"].as_str(),
            Some(case.name),
            "case {:?}: expected Name {:?}, got {:?}",
            case.label,
            case.name,
            created_body["Name"]
        );
        assert_eq!(
            created_body["Arn"].as_str(),
            Some(
                format!(
                    "arn:aws:mobiletargeting:{}:{}:apps/{id}",
                    case.region,
                    engine::EMULATOR_ACCOUNT_ID
                )
                .as_str()
            ),
            "case {:?}: expected the ARN to carry the signing region and the minted Id, got {:?}",
            case.label,
            created_body["Arn"]
        );
        assert_eq!(
            created_body["tags"],
            tags_value(&case.tags),
            "case {:?}: expected the stored tags back, got {:?}",
            case.label,
            created_body["tags"]
        );
        let creation_date = created_body["CreationDate"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            creation_date.starts_with("20") && creation_date.ends_with('Z'),
            "case {:?}: expected an ISO 8601 CreationDate, got {:?}",
            case.label,
            created_body["CreationDate"]
        );

        // The round trip: GetApp answers with the very same application.
        let (status, fetched) = emulator.get_app(case.region, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetApp 200, got {status} with {fetched:?}",
            case.label
        );
        assert_eq!(
            fetched, created_body,
            "case {:?}: GetApp must answer with the application CreateApp stored",
            case.label
        );

        created.push((id, created_body, case.label));
    }

    // Every created application is listed, exactly once.
    let (status, listed) = emulator.get_apps("us-east-1", "").await;
    assert_eq!(status, 200, "expected GetApps 200, got {status} with {listed:?}");
    let items = listed["Item"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        created.len(),
        "expected GetApps to list every stored application, got {items:?}"
    );
    assert!(
        listed.get("NextToken").is_none_or(Value::is_null),
        "a single-page listing must carry no NextToken, got {:?}",
        listed["NextToken"]
    );
    for (id, body, label) in &created {
        assert!(
            items.contains(body),
            "case {label:?}: expected GetApps to list application {id} as CreateApp described it, got {items:?}"
        );
    }

    // Deleting one answers with the deleted application and removes it everywhere.
    let (id, body, label) = created.remove(0);
    let (status, deleted) = emulator.delete_app("us-east-1", &id).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected DeleteApp 200, got {status} with {deleted:?}"
    );
    assert_eq!(
        deleted, body,
        "case {label:?}: DeleteApp must answer with the application it removed"
    );

    let (status, missing) = emulator.get_app("us-east-1", &id).await;
    assert_eq!(
        status, 404,
        "case {label:?}: expected GetApp 404 after DeleteApp, got {status} with {missing:?}"
    );
    assert_eq!(
        error_code(&missing),
        "NotFoundException",
        "case {label:?}: expected NotFoundException after DeleteApp, got {missing:?}"
    );

    let (_, listed) = emulator.get_apps("us-east-1", "").await;
    let items = listed["Item"].as_array().cloned().unwrap_or_default();
    assert!(
        !items.contains(&body),
        "case {label:?}: GetApps must not list a deleted application, got {items:?}"
    );

    // And deleting it twice is a 404, not a second success.
    let (status, again) = emulator.delete_app("us-east-1", &id).await;
    assert_eq!(
        status, 404,
        "case {label:?}: expected DeleteApp 404 for an unknown id, got {status} with {again:?}"
    );
}

#[tokio::test]
async fn create_app_refuses_a_name_that_displays_nothing() {
    let emulator = Emulator::new();
    // Rows: every way a name can carry no displayable text.
    for name in ["", " ", "\t", "\n", "   \t "] {
        let (status, body) = emulator
            .create_app("us-east-1", json!({ "Name": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected CreateApp 400 for a blank Name, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {name:?}: expected BadRequestException, got {body:?}"
        );
    }

    // ... and a name that merely *contains* whitespace is perfectly valid.
    let (status, body) = emulator
        .create_app("us-east-1", json!({ "Name": " padded name " }))
        .await;
    assert_eq!(
        status, 201,
        "case \" padded name \": expected CreateApp 201, got {status} with {body:?}"
    );
    assert_eq!(
        body["Name"].as_str(),
        Some(" padded name "),
        "case \" padded name \": the stored name must be the one sent, got {:?}",
        body["Name"]
    );
}

#[tokio::test]
async fn get_apps_pages_through_every_application_exactly_once() {
    let emulator = Emulator::new();
    const TOTAL: usize = 7;

    let mut all_ids: BTreeSet<String> = BTreeSet::new();
    for i in 0..TOTAL {
        let (status, body) = emulator
            .create_app("us-east-1", json!({ "Name": format!("app-{i}") }))
            .await;
        assert_eq!(status, 201, "setup: CreateApp {i} failed with {body:?}");
        all_ids.insert(body["Id"].as_str().unwrap_or_default().to_string());
    }

    // Rows: a page size below, at and above the number of stored applications.
    for page_size in [1_usize, 2, 3, TOTAL, TOTAL + 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let query = match &token {
                Some(t) => format!("page-size={page_size}&token={t}"),
                None => format!("page-size={page_size}"),
            };
            let (status, body) = emulator.get_apps("us-east-1", &query).await;
            assert_eq!(
                status, 200,
                "case page-size={page_size}: expected GetApps 200, got {status} with {body:?}"
            );
            let items = body["Item"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case page-size={page_size}: a page must hold at most page-size items, got {}",
                items.len()
            );
            for item in items {
                seen.push(item["Id"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= TOTAL + 2,
                "case page-size={page_size}: pagination did not terminate after {pages} pages"
            );

            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "case page-size={page_size}: paging returned a duplicate application: {seen:?}"
        );
        assert_eq!(
            unique, all_ids,
            "case page-size={page_size}: paging must yield every application exactly once, got {seen:?}"
        );
        let expected_pages = TOTAL.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page-size={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn get_apps_refuses_a_page_size_that_is_not_a_count_and_a_foreign_token() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .create_app("us-east-1", json!({ "Name": "paged" }))
        .await;
    assert_eq!(status, 201, "setup: CreateApp failed with {body:?}");

    // Rows: every non-count a client can put in `page-size`.
    for page_size in ["0", "-1", "abc", "1.5", "", "99999999999999999999999"] {
        let (status, body) = emulator
            .get_apps("us-east-1", &format!("page-size={page_size}"))
            .await;
        assert_eq!(
            status, 400,
            "case page-size={page_size:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case page-size={page_size:?}: expected BadRequestException, got {body:?}"
        );
    }

    // A token this service never issued is refused, not silently ignored — including
    // one that is well-formed hex but does not decode to an id this service mints.
    for token in [
        "not-a-token",
        "zz",
        "0",
        // hex("aa") and hex("not-an-id-but-valid-hex-0123456"): decodable, but not an
        // application id.
        "6161",
        "6e6f742d616e2d69642d6275742d76616c69642d6865782d30313233343536",
    ] {
        let (status, body) = emulator
            .get_apps("us-east-1", &format!("token={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case token={token:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case token={token:?}: expected BadRequestException, got {body:?}"
        );
    }
}

/// The largest page GetApps serves, and the page it serves when the caller names no
/// size — `crate::state::APP_PAGE_LIMIT`, which this crate does not export to a test,
/// so it is restated here and asserted from the outside.
const APP_PAGE_LIMIT: usize = 100;

#[tokio::test]
async fn a_page_is_bounded_by_the_services_own_page_limit() {
    let emulator = Emulator::new();
    let total = APP_PAGE_LIMIT + 7;
    for i in 0..total {
        let (status, body) = emulator
            .create_app("us-east-1", json!({ "Name": format!("bounded-{i}") }))
            .await;
        assert_eq!(status, 201, "setup: CreateApp {i} failed with {body:?}");
    }

    // Rows: no page-size at all, and page sizes at, just over and far over the limit.
    // None of them may answer with more than the limit, and each must hand back a
    // NextToken because applications remain.
    for query in [
        String::new(),
        format!("page-size={APP_PAGE_LIMIT}"),
        format!("page-size={}", APP_PAGE_LIMIT + 1),
        format!("page-size={}", APP_PAGE_LIMIT * 1000),
    ] {
        let (status, body) = emulator.get_apps("us-east-1", &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected GetApps 200, got {status} with {body:?}"
        );
        let items = body["Item"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            APP_PAGE_LIMIT,
            "case {query:?}: a page must hold at most {APP_PAGE_LIMIT} applications"
        );
        let token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            token.is_some(),
            "case {query:?}: expected a NextToken while applications remain, got {body:?}"
        );

        // ... and the rest arrives on the next page, which then ends.
        let (status, rest) = emulator
            .get_apps("us-east-1", &format!("token={}", token.unwrap()))
            .await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected the resumed page to be 200, got {status} with {rest:?}"
        );
        let remaining = rest["Item"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            remaining.len(),
            total - APP_PAGE_LIMIT,
            "case {query:?}: expected the remaining applications on the resumed page"
        );
        assert!(
            rest.get("NextToken").is_none_or(Value::is_null),
            "case {query:?}: the last page must carry no NextToken, got {:?}",
            rest["NextToken"]
        );
    }
}

/// The `ChannelType` catalog, from the Smithy model's `ChannelType` enum
/// (`smithy_query(query="shape", shape="ChannelType")` — the same 13 values the
/// generated `CHANNEL_TYPE_VALUES` carries).
const CHANNEL_TYPES: &[&str] = &[
    "ADM",
    "APNS",
    "APNS_SANDBOX",
    "APNS_VOIP",
    "APNS_VOIP_SANDBOX",
    "BAIDU",
    "CUSTOM",
    "EMAIL",
    "GCM",
    "IN_APP",
    "PUSH",
    "SMS",
    "VOICE",
];

#[tokio::test]
async fn an_endpoint_write_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let (status, app) = emulator
        .create_app("us-east-1", json!({ "Name": "endpoints" }))
        .await;
    assert_eq!(status, 201, "setup: CreateApp failed with {app:?}");
    let app_id = app["Id"].as_str().expect("a minted application id");

    // One row per modelled channel, each with its own addressing and payload, so no
    // assertion can pass by matching the shape of one recorded request.
    for (index, channel) in CHANNEL_TYPES.iter().enumerate() {
        let endpoint_id = format!("endpoint-{}", channel.to_lowercase());
        let request = json!({
            "Address": format!("address-{index}@example.com"),
            "ChannelType": channel,
            "Attributes": { "Interests": ["Music", "Sport"], "Tier": ["gold"] },
            "Metrics": { "Logins": 3.0, "Spend": 12.5 },
            "User": { "UserId": format!("user-{index}"), "UserAttributes": { "Role": ["admin"] } },
            "Location": { "City": "Seattle", "Country": "US", "Latitude": 47.6, "Longitude": -122.3, "PostalCode": "98101", "Region": "WA" },
            "Demographic": { "AppVersion": "1.2.3", "Make": "Apple", "Model": "iPhone", "Platform": "ios", "Timezone": "America/Los_Angeles" },
            "OptOut": "NONE",
        });

        let (status, accepted) = emulator
            .update_endpoint("us-east-1", app_id, &endpoint_id, request.clone())
            .await;
        assert_eq!(
            status, 202,
            "case {channel:?}: expected UpdateEndpoint 202, got {status} with {accepted:?}"
        );
        let request_id = accepted["RequestID"].as_str().unwrap_or_default().to_string();
        assert!(
            !request_id.is_empty(),
            "case {channel:?}: expected a RequestID in the MessageBody, got {accepted:?}"
        );

        let (status, stored) = emulator
            .get_endpoint("us-east-1", app_id, &endpoint_id)
            .await;
        assert_eq!(
            status, 200,
            "case {channel:?}: expected GetEndpoint 200, got {status} with {stored:?}"
        );

        assert_eq!(
            stored["Id"].as_str(),
            Some(endpoint_id.as_str()),
            "case {channel:?}: expected Id {endpoint_id:?}, got {:?}",
            stored["Id"]
        );
        assert_eq!(
            stored["ApplicationId"].as_str(),
            Some(app_id),
            "case {channel:?}: expected ApplicationId {app_id:?}, got {:?}",
            stored["ApplicationId"]
        );
        for member in [
            "Address",
            "ChannelType",
            "Attributes",
            "Metrics",
            "User",
            "Location",
            "Demographic",
            "OptOut",
        ] {
            assert_eq!(
                stored[member], request[member],
                "case {channel:?}: member {member} must be read back exactly as written, got {:?}",
                stored[member]
            );
        }
        assert_eq!(
            stored["RequestId"].as_str(),
            Some(request_id.as_str()),
            "case {channel:?}: expected the endpoint to record the request that last wrote it, got {:?}",
            stored["RequestId"]
        );
        // `EndpointRequest$EndpointStatus`: Pinpoint sets ACTIVE on a create or update.
        assert_eq!(
            stored["EndpointStatus"].as_str(),
            Some("ACTIVE"),
            "case {channel:?}: expected a written endpoint to be ACTIVE, got {:?}",
            stored["EndpointStatus"]
        );
        for member in ["CreationDate", "EffectiveDate"] {
            let value = stored[member].as_str().unwrap_or_default();
            assert!(
                value.starts_with("20") && value.ends_with('Z'),
                "case {channel:?}: expected an ISO 8601 {member}, got {:?}",
                stored[member]
            );
        }
        // `EndpointResponse$CohortId`: a number from 0-99.
        let cohort: i64 = stored["CohortId"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .unwrap_or(-1);
        assert!(
            (0..=99).contains(&cohort),
            "case {channel:?}: expected a CohortId in 0-99, got {:?}",
            stored["CohortId"]
        );

        // A read does not change what the next reader sees.
        let (_, again) = emulator
            .get_endpoint("us-east-1", app_id, &endpoint_id)
            .await;
        assert_eq!(
            again, stored,
            "case {channel:?}: GetEndpoint must be a pure read"
        );
    }

    // A channel the model does not list is refused as a client error (the boundary
    // validator answers it, before the handler, as a 400 `ValidationException`).
    for channel in ["TELEGRAM", "email", "", "SMS "] {
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                app_id,
                "endpoint-bad-channel",
                json!({ "ChannelType": channel }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {channel:?}: expected UpdateEndpoint 400 for an unmodelled ChannelType, got {status} with {body:?}"
        );
        let code = error_code(&body);
        assert!(
            code == "ValidationException" || code == "BadRequestException",
            "case {channel:?}: expected the refusal to be a client-input error, got {body:?}"
        );
        let (status, missing) = emulator
            .get_endpoint("us-east-1", app_id, "endpoint-bad-channel")
            .await;
        assert_eq!(
            status, 404,
            "case {channel:?}: a refused write must store nothing, got {status} with {missing:?}"
        );
    }

    // An EndpointId that identifies nothing is refused too.
    for endpoint_id in ["%20", "%09"] {
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                app_id,
                endpoint_id,
                json!({ "Address": "blank@example.com", "ChannelType": "EMAIL" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {endpoint_id:?}: expected UpdateEndpoint 400 for a blank EndpointId, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {endpoint_id:?}: expected BadRequestException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_write_merges_with_what_is_already_stored() {
    let emulator = Emulator::new();
    let (_, app) = emulator
        .create_app("us-east-1", json!({ "Name": "merging" }))
        .await;
    let app_id = app["Id"].as_str().expect("a minted application id");

    let (status, _) = emulator
        .update_endpoint(
            "us-east-1",
            app_id,
            "merge-1",
            json!({
                "Address": "first@example.com",
                "ChannelType": "EMAIL",
                "Attributes": { "Interests": ["Music"] },
                "Metrics": { "Logins": 1.0 },
                "User": { "UserId": "user-1" },
            }),
        )
        .await;
    assert_eq!(status, 202, "setup: the first write must be accepted");
    let (_, first) = emulator.get_endpoint("us-east-1", app_id, "merge-1").await;
    let created_at = first["CreationDate"].as_str().unwrap_or_default().to_string();

    // Rows: each partial write names SOME members and omits others. After it, the
    // named members carry the new value and every omitted one keeps the old.
    let writes: Vec<(&str, Value, HashMap<&str, Value>)> = vec![
        (
            "a metric only",
            json!({ "Metrics": { "Spend": 9.5 } }),
            HashMap::from([
                ("Address", json!("first@example.com")),
                ("ChannelType", json!("EMAIL")),
                ("Attributes", json!({ "Interests": ["Music"] })),
                ("Metrics", json!({ "Logins": 1.0, "Spend": 9.5 })),
                ("User", json!({ "UserId": "user-1" })),
            ]),
        ),
        (
            "a new attribute key",
            json!({ "Attributes": { "Tier": ["gold"] } }),
            HashMap::from([
                ("Address", json!("first@example.com")),
                ("Attributes", json!({ "Interests": ["Music"], "Tier": ["gold"] })),
                ("Metrics", json!({ "Logins": 1.0, "Spend": 9.5 })),
            ]),
        ),
        (
            "replacing an attribute's values",
            json!({ "Attributes": { "Interests": ["Sport", "Travel"] } }),
            HashMap::from([
                (
                    "Attributes",
                    json!({ "Interests": ["Sport", "Travel"], "Tier": ["gold"] }),
                ),
                ("Metrics", json!({ "Logins": 1.0, "Spend": 9.5 })),
            ]),
        ),
        (
            "a new address and channel",
            json!({ "Address": "+15550100", "ChannelType": "SMS" }),
            HashMap::from([
                ("Address", json!("+15550100")),
                ("ChannelType", json!("SMS")),
                ("User", json!({ "UserId": "user-1" })),
            ]),
        ),
        (
            "an explicit INACTIVE status",
            json!({ "EndpointStatus": "INACTIVE" }),
            HashMap::from([
                ("EndpointStatus", json!("INACTIVE")),
                ("Address", json!("+15550100")),
            ]),
        ),
    ];

    for (label, request, expected) in writes {
        let (status, accepted) = emulator
            .update_endpoint("us-east-1", app_id, "merge-1", request)
            .await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected UpdateEndpoint 202, got {status} with {accepted:?}"
        );
        let (_, stored) = emulator.get_endpoint("us-east-1", app_id, "merge-1").await;
        for (member, value) in expected {
            assert_eq!(
                stored[member], value,
                "case {label:?}: expected {member} to be {value:?}, got {:?}",
                stored[member]
            );
        }
        assert_eq!(
            stored["CreationDate"].as_str(),
            Some(created_at.as_str()),
            "case {label:?}: CreationDate is set once, at creation, got {:?}",
            stored["CreationDate"]
        );
    }
}

#[tokio::test]
async fn writing_an_address_onto_another_endpoint_deactivates_the_first() {
    // `EndpointRequest$EndpointStatus`: "Amazon Pinpoint automatically sets this value
    // to INACTIVE if you update another endpoint that has the same address specified by
    // the Address property."
    let emulator = Emulator::new();
    let (_, app) = emulator
        .create_app("us-east-1", json!({ "Name": "addresses" }))
        .await;
    let app_id = app["Id"].as_str().expect("a minted application id");

    // Rows: one address per channel family, each claimed by two endpoints in turn.
    for (first, second, address, channel) in [
        ("a-1", "a-2", "shared@example.com", "EMAIL"),
        ("b-1", "b-2", "+15550111", "SMS"),
        ("c-1", "c-2", "device-token-xyz", "GCM"),
    ] {
        let body = json!({ "Address": address, "ChannelType": channel });
        let (status, _) = emulator
            .update_endpoint("us-east-1", app_id, first, body.clone())
            .await;
        assert_eq!(status, 202, "case {address:?}: the first write must be accepted");
        let (status, _) = emulator
            .update_endpoint("us-east-1", app_id, second, body)
            .await;
        assert_eq!(status, 202, "case {address:?}: the second write must be accepted");

        let (_, older) = emulator.get_endpoint("us-east-1", app_id, first).await;
        let (_, newer) = emulator.get_endpoint("us-east-1", app_id, second).await;
        assert_eq!(
            newer["EndpointStatus"].as_str(),
            Some("ACTIVE"),
            "case {address:?}: the endpoint written last holds the address, got {:?}",
            newer["EndpointStatus"]
        );
        assert_eq!(
            older["EndpointStatus"].as_str(),
            Some("INACTIVE"),
            "case {address:?}: the endpoint that lost the address must go INACTIVE, got {:?}",
            older["EndpointStatus"]
        );
    }
}

#[tokio::test]
async fn an_unknown_application_or_endpoint_is_not_found() {
    let emulator = Emulator::new();
    let (_, app) = emulator
        .create_app("us-east-1", json!({ "Name": "lookups" }))
        .await;
    let app_id = app["Id"].as_str().expect("a minted application id").to_string();
    let (status, _) = emulator
        .update_endpoint(
            "us-east-1",
            &app_id,
            "known",
            json!({ "Address": "known@example.com", "ChannelType": "EMAIL" }),
        )
        .await;
    assert_eq!(status, 202, "setup: the endpoint write must be accepted");

    // Rows: every way a caller can name something that is not stored.
    let unknown = "00000000000000000000000000000000";
    let cases: Vec<(&str, (u16, Value))> = vec![
        ("GetApp of an unknown application", emulator.get_app("us-east-1", unknown).await),
        (
            "DeleteApp of an unknown application",
            emulator.delete_app("us-east-1", unknown).await,
        ),
        (
            "GetEndpoint of an unknown application",
            emulator.get_endpoint("us-east-1", unknown, "known").await,
        ),
        (
            "GetEndpoint of an unknown endpoint",
            emulator.get_endpoint("us-east-1", &app_id, "no-such-endpoint").await,
        ),
        (
            "UpdateEndpoint under an unknown application",
            emulator
                .update_endpoint(
                    "us-east-1",
                    unknown,
                    "known",
                    json!({ "Address": "x@example.com", "ChannelType": "EMAIL" }),
                )
                .await,
        ),
    ];

    for (label, (status, body)) in cases {
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body:?}"
        );
    }

    // Deleting the application takes its endpoints with it.
    let (status, _) = emulator.delete_app("us-east-1", &app_id).await;
    assert_eq!(status, 200, "setup: DeleteApp must succeed");
    let (status, body) = emulator.get_endpoint("us-east-1", &app_id, "known").await;
    assert_eq!(
        status, 404,
        "an endpoint of a deleted application must be gone, got {status} with {body:?}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_own_ceilings() {
    // The emulator's own ceilings (crate::state::MAX_APPS / MAX_ENDPOINTS_PER_APP /
    // MAX_ENDPOINT_ATTRIBUTES / MAX_ENDPOINT_METRICS), restated here and asserted
    // from the outside: a create past one is refused, never served.
    const MAX_APPS: usize = 200;
    const MAX_ENDPOINTS_PER_APP: usize = 200;
    const MAX_ENDPOINT_ATTRIBUTES: usize = 250;
    const MAX_ENDPOINT_METRICS: usize = 250;

    let emulator = Emulator::new();
    let mut first_id = String::new();
    for i in 0..MAX_APPS {
        let (status, body) = emulator
            .create_app("us-east-1", json!({ "Name": format!("app-{i}") }))
            .await;
        assert_eq!(
            status, 201,
            "case app {i}: a create below the ceiling must succeed, got {status} with {body:?}"
        );
        if i == 0 {
            first_id = body["Id"].as_str().unwrap_or_default().to_string();
        }
    }

    let (status, body) = emulator
        .create_app("us-east-1", json!({ "Name": "one-too-many" }))
        .await;
    assert_eq!(
        status, 400,
        "case app {MAX_APPS}: a create past the ceiling must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case app {MAX_APPS}: expected BadRequestException, got {body:?}"
    );

    // Deleting one frees its slot, so the bound is on live applications.
    let (status, _) = emulator.delete_app("us-east-1", &first_id).await;
    assert_eq!(status, 200, "setup: DeleteApp must succeed");
    let (status, body) = emulator
        .create_app("us-east-1", json!({ "Name": "after-a-delete" }))
        .await;
    assert_eq!(
        status, 201,
        "a create must succeed again once a slot is free, got {status} with {body:?}"
    );
    let app_id = body["Id"]
        .as_str()
        .expect("a minted application id")
        .to_string();

    // Endpoints per application: the same bound, the same refusal.
    for i in 0..MAX_ENDPOINTS_PER_APP {
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                &app_id,
                &format!("endpoint-{i}"),
                json!({ "Address": format!("user-{i}@example.com"), "ChannelType": "EMAIL" }),
            )
            .await;
        assert_eq!(
            status, 202,
            "case endpoint {i}: a write below the ceiling must be accepted, got {status} with {body:?}"
        );
    }
    let (status, body) = emulator
        .update_endpoint(
            "us-east-1",
            &app_id,
            "endpoint-one-too-many",
            json!({ "Address": "spill@example.com", "ChannelType": "EMAIL" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case endpoint {MAX_ENDPOINTS_PER_APP}: a write past the ceiling must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case endpoint {MAX_ENDPOINTS_PER_APP}: expected BadRequestException, got {body:?}"
    );
    // ... and an update of an endpoint that already exists still works at the ceiling.
    let (status, body) = emulator
        .update_endpoint(
            "us-east-1",
            &app_id,
            "endpoint-0",
            json!({ "OptOut": "ALL" }),
        )
        .await;
    assert_eq!(
        status, 202,
        "a write to an EXISTING endpoint adds nothing and must stay accepted, got {status} with {body:?}"
    );

    // The merging maps are bounded too. Rows: one oversized request, and an
    // accumulation that fills the ceiling and then asks for one key more. A fresh
    // emulator, since the one above is at its application ceiling.
    let emulator = Emulator::new();
    let (status, app) = emulator
        .create_app("us-east-1", json!({ "Name": "bounded-maps" }))
        .await;
    assert_eq!(status, 201, "setup: CreateApp failed with {app:?}");
    let maps_app = app["Id"]
        .as_str()
        .expect("a minted application id")
        .to_string();

    for (member, ceiling) in [
        ("Attributes", MAX_ENDPOINT_ATTRIBUTES),
        ("Metrics", MAX_ENDPOINT_METRICS),
    ] {
        let value = |i: usize| -> Value {
            if member == "Attributes" {
                json!([format!("v{i}")])
            } else {
                json!(f64::from(u32::try_from(i).expect("a small test index")))
            }
        };
        let map = |range: std::ops::Range<usize>| -> Value {
            Value::Object(range.map(|i| (format!("k{i}"), value(i))).collect())
        };
        let endpoint_id = format!("maps-{}", member.to_lowercase());

        // One request already past the ceiling.
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                &maps_app,
                &endpoint_id,
                json!({ member: map(0..ceiling + 1) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {member}: {} keys in one write must be refused, got {status} with {body:?}",
            ceiling + 1
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {member}: expected BadRequestException, got {body:?}"
        );
        let (status, _) = emulator
            .get_endpoint("us-east-1", &maps_app, &endpoint_id)
            .await;
        assert_eq!(
            status, 404,
            "case {member}: a refused write must store nothing"
        );

        // Exactly the ceiling is accepted, and then one further key is refused —
        // the bound is on what is STORED, across writes.
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                &maps_app,
                &endpoint_id,
                json!({ member: map(0..ceiling) }),
            )
            .await;
        assert_eq!(
            status, 202,
            "case {member}: exactly {ceiling} keys must be accepted, got {status} with {body:?}"
        );
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                &maps_app,
                &endpoint_id,
                json!({ member: map(ceiling..ceiling + 1) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {member}: one key past the ceiling must be refused, got {status} with {body:?}"
        );
        // Replacing a key the endpoint already holds grows nothing, so it is fine.
        let (status, body) = emulator
            .update_endpoint(
                "us-east-1",
                &maps_app,
                &endpoint_id,
                json!({ member: map(0..1) }),
            )
            .await;
        assert_eq!(
            status, 202,
            "case {member}: replacing a stored key must stay accepted, got {status} with {body:?}"
        );
        let (_, stored) = emulator
            .get_endpoint("us-east-1", &maps_app, &endpoint_id)
            .await;
        assert_eq!(
            stored[member].as_object().map(serde_json::Map::len),
            Some(ceiling),
            "case {member}: the stored map must still hold exactly {ceiling} keys"
        );
    }
}

#[tokio::test]
async fn state_is_shared_across_requests_not_per_request() {
    // The whole point of the shared engine: what one request writes, the next reads.
    // Two emulators over two engines must NOT see each other's applications.
    let one = Emulator::new();
    let other = Emulator::new();

    let (_, app) = one.create_app("us-east-1", json!({ "Name": "only-in-one" })).await;
    let id = app["Id"].as_str().expect("a minted application id");

    let (status, body) = one.get_app("us-east-1", id).await;
    assert_eq!(
        status, 200,
        "the same emulator must read back its own application, got {status} with {body:?}"
    );
    let (status, body) = other.get_app("us-east-1", id).await;
    assert_eq!(
        status, 404,
        "a separate engine must not hold another emulator's application, got {status} with {body:?}"
    );
}
