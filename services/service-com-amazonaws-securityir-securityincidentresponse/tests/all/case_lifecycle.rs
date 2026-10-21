// SPDX-License-Identifier: CC0-1.0

//! The case lifecycle of the Security IR emulator, end to end over `restJson1`.
//!
//! Every suite here drives the service the way a client does — through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`], with a real SigV4-scoped
//! `Authorization` header — so what it asserts is the wire behaviour, not a handler's
//! internals.
//!
//! The rule under test: `CreateCase` mints and STORES a case, and every later call
//! computes its answer from that stored record. So the suites below are tables whose
//! rows vary along what the rule generalises over — `resolverType` and `engagementType`
//! (both whole modelled value sets), the sizes of `title`/`description`/`body` at their
//! modelled bounds, the lengths of each list member (empty through several), presence of
//! `tags`, the signing region, and the number of comments — and each row asserts the
//! submitted values come back unchanged and the mutations are visible to every reader.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_securityir_securityincidentresponse::ops::types::{
    CASE_STATUS_VALUES, ENGAGEMENT_TYPE_VALUES, PENDING_ACTION_VALUES, RESOLVER_TYPE_VALUES,
};
use service_com_amazonaws_securityir_securityincidentresponse::state::{
    idempotency_key, page_keys, page_size, ComAmazonawsSecurityirSecurityincidentresponseState,
    CASE_STATUS_CLOSED, CASE_STATUS_SUBMITTED, DEFAULT_MAX_RESULTS, MAX_COMMENTS_PER_CASE,
    MAX_IDEMPOTENCY_ENTRIES, MAX_PAGE_SIZE, MAX_STORED_CASES, PENDING_ACTION_NONE,
};
use service_com_amazonaws_securityir_securityincidentresponse::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name, which is also the routing key the emulator
/// resolves a request by.
const SIGNING_NAME: &str = "security-ir";

/// The canonical account id every generated emulator presents, and therefore the
/// account segment of every `CaseArn` it mints.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator: a router with only this service mounted on one shared engine, exactly
/// as the production router mounts it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; these
        // requests carry a scope (which is how they route) but no real signature.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one request signed into `region`, and return its status and JSON body.
    async fn call(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{SIGNING_NAME}.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    async fn create_case(&self, region: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/v1/create-case", region, Some(body)).await
    }

    async fn get_case(&self, region: &str, case_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/cases/{case_id}/get-case"), region, None)
            .await
    }

    async fn list_cases(&self, region: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/v1/list-cases", region, Some(body)).await
    }

    async fn create_comment(&self, region: &str, case_id: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/v1/cases/{case_id}/create-comment"),
            region,
            Some(body),
        )
        .await
    }

    async fn list_comments(&self, region: &str, case_id: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/v1/cases/{case_id}/list-comments"),
            region,
            Some(body),
        )
        .await
    }

    async fn close_case(&self, region: &str, case_id: &str) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/v1/cases/{case_id}/close-case"),
            region,
            Some(json!({})),
        )
        .await
    }
}

/// One row of the lifecycle table: a whole `CreateCase` request plus the comments to
/// post on the case it mints.
struct CaseRow {
    label: &'static str,
    /// The region the request is signed into — the region segment of the minted
    /// `CaseArn`.
    region: &'static str,
    request: Value,
    comments: Vec<String>,
}

/// `caseId` satisfies `CaseId`'s `^\d{10,32}`, and the ten digits `CaseArn`'s
/// `case/[0-9]{10}` tail requires.
fn is_case_id(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|b| b.is_ascii_digit())
}

/// `commentId` satisfies `CommentId`'s `^\d{6}$`.
fn is_comment_id(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|b| b.is_ascii_digit())
}

fn string_of(len: usize, fill: char) -> String {
    std::iter::repeat_n(fill, len).collect()
}

/// The lifecycle table. Rows differ along every axis the rule generalises over; between
/// them they cover both `ResolverType` values and both `EngagementType` values (the
/// whole modelled sets, asserted in `modelled_value_sets_back_the_stored_constants`),
/// the `title` / `description` length bounds, empty and multi-element list members,
/// `tags` present and absent, and two signing regions.
fn lifecycle_table() -> Vec<CaseRow> {
    vec![
        CaseRow {
            label: "self-resolved investigation, every optional list empty, title/description at their minimum length",
            region: "us-east-1",
            request: json!({
                "clientToken": "tok-minimal",
                "resolverType": "Self",
                "engagementType": "Investigation",
                "title": string_of(1, 'a'),
                "description": string_of(1, 'b'),
                "reportedIncidentStartDate": 1_700_000_000,
                "impactedAccounts": [],
                "watchers": [],
                "threatActorIpAddresses": [],
                "impactedServices": [],
                "impactedAwsRegions": [],
            }),
            comments: vec![string_of(1, 'c')],
        },
        CaseRow {
            label: "AWS-resolved security incident, full lists and tags, title/description at their maximum length",
            region: "us-east-1",
            request: json!({
                "clientToken": "tok-full",
                "resolverType": "AWS",
                "engagementType": "Security Incident",
                "title": string_of(300, 'T'),
                "description": string_of(8000, 'D'),
                "reportedIncidentStartDate": 1_600_000_123,
                "impactedAccounts": ["000123123123", "000000000001", "999999999999"],
                "watchers": [
                    {"email": "sec@example.com"},
                    {"email": "ir-lead@example.com", "name": "Ira Lead", "jobTitle": "IR Lead"},
                ],
                "threatActorIpAddresses": [
                    {"ipAddress": "203.0.113.7"},
                    {"ipAddress": "198.51.100.42", "userAgent": "curl/8.4.0"},
                ],
                "impactedServices": ["s3", "iam", "ec2"],
                // `AwsRegion` values, none of them the region this case is signed into.
                "impactedAwsRegions": [{"region": "eu-west-1"}, {"region": "ap-southeast-2"}],
                "tags": {"team": "ir", "severity": "high"},
            }),
            comments: vec![
                "Containment started".to_string(),
                "Æ — unicode body with emoji 🔒".to_string(),
                string_of(12_000, 'x'),
            ],
        },
        CaseRow {
            label: "signed into another region, no clientToken, no comments",
            region: "us-west-2",
            request: json!({
                "resolverType": "Self",
                "engagementType": "Security Incident",
                "title": "Suspicious IAM activity",
                "description": "Unexpected AssumeRole calls",
                "reportedIncidentStartDate": 1_712_345_678,
                "impactedAccounts": ["000123123123"],
                "watchers": [{"email": "sec@example.com", "jobTitle": "Analyst"}],
                "threatActorIpAddresses": [{"ipAddress": "2001:0DB8:0000:0000:0000:0000:0000:0001"}],
                "impactedServices": ["lambda"],
                "impactedAwsRegions": [{"region": "us-east-2"}],
            }),
            comments: vec![],
        },
    ]
}

/// The whole slice, per row: create, replay, read back, list, comment, list comments,
/// close, and read back again.
#[tokio::test]
async fn a_created_case_is_stored_and_every_later_call_reads_it_back() {
    for row in lifecycle_table() {
        let case = row.label;
        let emulator = Emulator::new();

        // --- CreateCase mints and stores -----------------------------------------
        let (status, created) = emulator.create_case(row.region, row.request.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateCase must answer 201, got {status} with {created:?}"
        );
        let case_id = created["caseId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_case_id(&case_id),
            "case {case:?}: caseId must be ten digits (CaseId ^\\d{{10,32}}), got {:?}",
            created["caseId"]
        );

        // --- the same clientToken is a replay, not a second case ------------------
        let (status, total_before) = emulator.list_cases(row.region, json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListCases must answer 200");
        if row.request.get("clientToken").is_some() {
            let (status, replayed) = emulator.create_case(row.region, row.request.clone()).await;
            assert_eq!(
                status, 201,
                "case {case:?}: replaying CreateCase must answer 201, got {status}"
            );
            assert_eq!(
                replayed["caseId"].as_str(),
                Some(case_id.as_str()),
                "case {case:?}: replaying the clientToken must return the first caseId, got {:?}",
                replayed["caseId"]
            );
            let (_, total_after) = emulator.list_cases(row.region, json!({})).await;
            assert_eq!(
                total_after["total"], total_before["total"],
                "case {case:?}: replaying the clientToken must not store a second case, total went from {:?} to {:?}",
                total_before["total"], total_after["total"]
            );
        }

        // --- GetCase projects the stored record -----------------------------------
        let (status, fetched) = emulator.get_case(row.region, &case_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCase must answer 200, got {status} with {fetched:?}"
        );
        let submitted = row.request.as_object().expect("the row is a JSON object");
        for member in [
            "title",
            "description",
            "engagementType",
            "resolverType",
            "reportedIncidentStartDate",
            "impactedAccounts",
            "watchers",
            "threatActorIpAddresses",
            "impactedServices",
            "impactedAwsRegions",
        ] {
            assert_eq!(
                &fetched[member], &submitted[member],
                "case {case:?}: GetCase must return the submitted {member}, got {:?}",
                fetched[member]
            );
        }
        assert_eq!(
            fetched["caseStatus"].as_str(),
            Some(CASE_STATUS_SUBMITTED),
            "case {case:?}: a new case must be {CASE_STATUS_SUBMITTED}, got {:?}",
            fetched["caseStatus"]
        );
        assert_eq!(
            fetched["pendingAction"].as_str(),
            Some(PENDING_ACTION_NONE),
            "case {case:?}: a new case waits on nobody, got pendingAction {:?}",
            fetched["pendingAction"]
        );
        assert_eq!(
            fetched["caseArn"].as_str(),
            Some(
                format!(
                    "arn:aws:{SIGNING_NAME}:{}:{ACCOUNT_ID}:case/{case_id}",
                    row.region
                )
                .as_str()
            ),
            "case {case:?}: caseArn must name this service, the request's region and the minted id, got {:?}",
            fetched["caseArn"]
        );
        let created_date = fetched["createdDate"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: createdDate must be a timestamp, got {:?}",
                fetched["createdDate"]
            )
        });
        assert_eq!(
            fetched["lastUpdatedDate"].as_f64(),
            Some(created_date),
            "case {case:?}: an untouched case was last updated when it was created, got {:?}",
            fetched["lastUpdatedDate"]
        );
        assert!(
            fetched["closedDate"].is_null(),
            "case {case:?}: an open case has no closedDate, got {:?}",
            fetched["closedDate"]
        );

        // --- reading does not change what the next caller sees --------------------
        let (_, fetched_again) = emulator.get_case(row.region, &case_id).await;
        assert_eq!(
            fetched_again, fetched,
            "case {case:?}: GetCase must not change the case it reports"
        );

        // --- ListCases enumerates what is stored ----------------------------------
        let (status, listed) = emulator
            .list_cases(row.region, json!({"maxResults": 25}))
            .await;
        assert_eq!(status, 200, "case {case:?}: ListCases must answer 200");
        let item = find_item(&listed, "caseId", &case_id).unwrap_or_else(|| {
            panic!("case {case:?}: ListCases must include the stored case, got {listed:?}")
        });
        assert_eq!(
            item["title"], fetched["title"],
            "case {case:?}: the ListCases item must project the stored title, got {:?}",
            item["title"]
        );
        assert_eq!(
            item["caseArn"], fetched["caseArn"],
            "case {case:?}: the ListCases item must project the stored caseArn, got {:?}",
            item["caseArn"]
        );
        assert_eq!(
            item["caseStatus"].as_str(),
            Some(CASE_STATUS_SUBMITTED),
            "case {case:?}: the ListCases item must project the stored status, got {:?}",
            item["caseStatus"]
        );
        assert_eq!(
            listed["total"].as_i64(),
            Some(1),
            "case {case:?}: total must be the stored case count, got {:?}",
            listed["total"]
        );

        // --- comments append, replay and read back byte-identical ------------------
        let mut comment_ids: Vec<String> = Vec::new();
        for (index, body) in row.comments.iter().enumerate() {
            let token = format!("comment-tok-{index}");
            let request = json!({"body": body, "clientToken": token});
            let (status, posted) = emulator
                .create_comment(row.region, &case_id, request.clone())
                .await;
            assert_eq!(
                status, 201,
                "case {case:?} comment {index}: CreateCaseComment must answer 201, got {status} with {posted:?}"
            );
            let comment_id = posted["commentId"].as_str().unwrap_or_default().to_string();
            assert!(
                is_comment_id(&comment_id),
                "case {case:?} comment {index}: commentId must be six digits (^\\d{{6}}$), got {:?}",
                posted["commentId"]
            );
            let (_, replayed) = emulator.create_comment(row.region, &case_id, request).await;
            assert_eq!(
                replayed["commentId"].as_str(),
                Some(comment_id.as_str()),
                "case {case:?} comment {index}: replaying the clientToken must return the first commentId, got {:?}",
                replayed["commentId"]
            );
            comment_ids.push(comment_id);
        }

        let (status, comments) = emulator
            .list_comments(row.region, &case_id, json!({"maxResults": 25}))
            .await;
        assert_eq!(status, 200, "case {case:?}: ListComments must answer 200");
        assert_eq!(
            comments["total"].as_i64(),
            Some(row.comments.len() as i64),
            "case {case:?}: ListComments total must be the stored comment count, got {:?}",
            comments["total"]
        );
        let items = comments["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            row.comments.len(),
            "case {case:?}: ListComments must return every stored comment, got {items:?}"
        );
        for (index, body) in row.comments.iter().enumerate() {
            assert_eq!(
                items[index]["commentId"].as_str(),
                Some(comment_ids[index].as_str()),
                "case {case:?} comment {index}: ListComments must report the allocated commentId, got {:?}",
                items[index]["commentId"]
            );
            assert_eq!(
                items[index]["body"].as_str(),
                Some(body.as_str()),
                "case {case:?} comment {index}: the stored body must come back byte-identical, got {:?}",
                items[index]["body"]
            );
            assert_eq!(
                items[index]["creator"].as_str(),
                Some(ACCOUNT_ID),
                "case {case:?} comment {index}: creator must be the calling account, got {:?}",
                items[index]["creator"]
            );
            assert_eq!(
                items[index]["lastUpdatedBy"], items[index]["creator"],
                "case {case:?} comment {index}: an unedited comment was last updated by its creator, got {:?}",
                items[index]["lastUpdatedBy"]
            );
            assert!(
                items[index]["createdDate"].as_f64().is_some(),
                "case {case:?} comment {index}: createdDate must be a timestamp, got {:?}",
                items[index]["createdDate"]
            );
        }

        // --- CloseCase mutates the stored case ------------------------------------
        let (status, closed) = emulator.close_case(row.region, &case_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CloseCase must answer 200, got {status} with {closed:?}"
        );
        assert_eq!(
            closed["caseStatus"].as_str(),
            Some(CASE_STATUS_CLOSED),
            "case {case:?}: CloseCase must report {CASE_STATUS_CLOSED}, got {:?}",
            closed["caseStatus"]
        );
        let closed_date = closed["closedDate"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: CloseCase must report a closedDate, got {:?}",
                closed["closedDate"]
            )
        });

        let (_, after_close) = emulator.get_case(row.region, &case_id).await;
        assert_eq!(
            after_close["caseStatus"].as_str(),
            Some(CASE_STATUS_CLOSED),
            "case {case:?}: GetCase must see the close, got {:?}",
            after_close["caseStatus"]
        );
        assert_eq!(
            after_close["closedDate"].as_f64(),
            Some(closed_date),
            "case {case:?}: GetCase must report the closedDate CloseCase returned, got {:?}",
            after_close["closedDate"]
        );
        assert!(
            after_close["lastUpdatedDate"].as_f64().unwrap_or(0.0) >= created_date,
            "case {case:?}: closing must not move lastUpdatedDate backwards, got {:?} after {created_date}",
            after_close["lastUpdatedDate"]
        );

        let (_, listed_after) = emulator.list_cases(row.region, json!({})).await;
        let item = find_item(&listed_after, "caseId", &case_id)
            .unwrap_or_else(|| panic!("case {case:?}: ListCases must still include the case"));
        assert_eq!(
            item["caseStatus"].as_str(),
            Some(CASE_STATUS_CLOSED),
            "case {case:?}: ListCases must report the case as closed, got {:?}",
            item["caseStatus"]
        );
        assert_eq!(
            item["closedDate"].as_f64(),
            Some(closed_date),
            "case {case:?}: ListCases must report the same closedDate, got {:?}",
            item["closedDate"]
        );

        // Closing again records nothing new: closedDate says when the case WAS closed.
        let (status, reclosed) = emulator.close_case(row.region, &case_id).await;
        assert_eq!(status, 200, "case {case:?}: re-closing must answer 200");
        assert_eq!(
            reclosed["closedDate"].as_f64(),
            Some(closed_date),
            "case {case:?}: re-closing must not move closedDate, got {:?}",
            reclosed["closedDate"]
        );
    }
}

/// Every case is minted with its own id, and the ids a run hands out are distinct.
#[tokio::test]
async fn each_created_case_gets_its_own_id_and_its_own_record() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for index in 0..5u32 {
        let title = format!("case number {index}");
        let (status, created) = emulator
            .create_case(region, create_request(&title, index))
            .await;
        assert_eq!(status, 201, "case {index}: CreateCase must answer 201");
        let case_id = created["caseId"].as_str().unwrap_or_default().to_string();
        assert!(
            ids.insert(case_id.clone()),
            "case {index}: caseId {case_id} was already handed out"
        );

        let (_, fetched) = emulator.get_case(region, &case_id).await;
        assert_eq!(
            fetched["title"].as_str(),
            Some(title.as_str()),
            "case {index}: each case must keep its own title, got {:?}",
            fetched["title"]
        );
    }
    assert_eq!(ids.len(), 5, "five creates must store five cases, got {ids:?}");
}

/// A `caseId` nothing is stored under is refused with the service's modelled
/// `ResourceNotFoundException` (HTTP 404) — never a zeroed success — by every operation
/// that resolves a case.
#[tokio::test]
async fn an_unknown_case_id_is_refused_by_every_case_operation() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    // One real case exists, so a refusal cannot be "the store is empty".
    let (_, created) = emulator
        .create_case(region, create_request("a real case", 0))
        .await;
    let real = created["caseId"].as_str().expect("a minted caseId").to_string();

    // Unknown ids spanning `CaseId`'s whole ^\d{10,32} shape: its 10-digit minimum
    // (including the all-zero id no counter mints) through its 32-digit maximum.
    let unknown = [
        "9999999999",
        "0000000000",
        "12345678901234567890",
        "99999999999999999999999999999999",
    ];
    for case_id in unknown {
        assert_ne!(case_id, real.as_str(), "the fixture must be unknown");
        let probes: Vec<(&str, (u16, Value))> = vec![
            ("GetCase", emulator.get_case(region, case_id).await),
            (
                "ListComments",
                emulator.list_comments(region, case_id, json!({})).await,
            ),
            (
                "CreateCaseComment",
                emulator
                    .create_comment(region, case_id, json!({"body": "x"}))
                    .await,
            ),
            ("CloseCase", emulator.close_case(region, case_id).await),
        ];
        for (operation, (status, body)) in probes {
            assert_eq!(
                status, 404,
                "case {operation} on unknown {case_id}: must be refused 404, got {status} with {body:?}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {operation} on unknown {case_id}: must be a ResourceNotFoundException, got {:?}",
                body["__type"]
            );
        }
    }

    // And the real case is still readable: a refusal stored nothing and removed nothing.
    let (status, fetched) = emulator.get_case(region, &real).await;
    assert_eq!(
        status, 200,
        "case the real case after four refusals: GetCase must still answer 200, got {status} with {fetched:?}"
    );
}

/// `ListCases` honours the `@paginated` contract at every page size, and `total` always
/// reports the full stored count.
#[tokio::test]
async fn list_cases_pages_at_every_page_size() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let stored = 7usize;
    let mut all: Vec<String> = Vec::new();
    for index in 0..stored {
        let (_, created) = emulator
            .create_case(region, create_request(&format!("case {index}"), index as u32))
            .await;
        all.push(created["caseId"].as_str().expect("a minted caseId").to_string());
    }
    all.sort();

    // 1 and 25 are `maxResults`'s published range bounds; 3 and 7 sit inside it, 7
    // exactly covering the stored count.
    for max_results in [1usize, 3, 7, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": max_results});
            if let Some(ref token) = token {
                request["nextToken"] = json!(token);
            }
            let (status, page) = emulator.list_cases(region, request).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results} page {pages}: must answer 200, got {page:?}"
            );
            assert_eq!(
                page["total"].as_i64(),
                Some(stored as i64),
                "case maxResults={max_results} page {pages}: total must be the full stored count, got {:?}",
                page["total"]
            );
            let items = page["items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= max_results,
                "case maxResults={max_results} page {pages}: a page must hold at most maxResults items, got {}",
                items.len()
            );
            for item in &items {
                seen.push(item["caseId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            match page["nextToken"].as_str() {
                Some(next) => {
                    assert_eq!(
                        items.len(),
                        max_results,
                        "case maxResults={max_results} page {pages}: a page that is not the last must be full, got {}",
                        items.len()
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= stored + 1,
                "case maxResults={max_results}: pagination did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, all,
            "case maxResults={max_results}: paging must enumerate every stored case exactly once, got {seen:?}"
        );
    }
}

/// With no `maxResults`, the member's `@default` of 25 is the page size.
#[tokio::test]
async fn list_cases_defaults_to_the_modelled_page_size() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let stored = (DEFAULT_MAX_RESULTS + 1) as usize;
    for index in 0..stored {
        emulator
            .create_case(region, create_request(&format!("case {index}"), index as u32))
            .await;
    }

    let (status, page) = emulator.list_cases(region, json!({})).await;
    assert_eq!(status, 200, "case no maxResults: ListCases must answer 200");
    let items = page["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        DEFAULT_MAX_RESULTS as usize,
        "case no maxResults: the page must hold the modelled default of {DEFAULT_MAX_RESULTS}, got {}",
        items.len()
    );
    assert!(
        page["nextToken"].as_str().is_some(),
        "case no maxResults: {stored} stored cases must leave a nextToken, got {:?}",
        page["nextToken"]
    );
    assert_eq!(
        page["total"].as_i64(),
        Some(stored as i64),
        "case no maxResults: total must be the full stored count, got {:?}",
        page["total"]
    );
}

/// A `maxResults` one past either published bound of its 1..25 range is refused before
/// any handler runs.
#[tokio::test]
async fn a_max_results_outside_its_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    emulator.create_case(region, create_request("a case", 0)).await;

    for max_results in [0, 26] {
        let (status, body) = emulator
            .list_cases(region, json!({"maxResults": max_results}))
            .await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: a value outside 1..25 must be refused 400, got {status} with {body:?}"
        );
    }
}

/// `ListComments` pages the comments of one case, and `nextToken` resumes where the page
/// ended.
#[tokio::test]
async fn list_comments_pages_one_case_at_a_time() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (_, created) = emulator
        .create_case(region, create_request("a commented case", 0))
        .await;
    let case_id = created["caseId"].as_str().expect("a minted caseId").to_string();
    let (_, other) = emulator
        .create_case(region, create_request("an uncommented case", 1))
        .await;
    let other_id = other["caseId"].as_str().expect("a minted caseId").to_string();

    let bodies: Vec<String> = (0..5).map(|index| format!("comment body {index}")).collect();
    for body in &bodies {
        emulator
            .create_comment(region, &case_id, json!({"body": body}))
            .await;
    }

    for max_results in [1usize, 2, 5, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": max_results});
            if let Some(ref token) = token {
                request["nextToken"] = json!(token);
            }
            let (status, page) = emulator.list_comments(region, &case_id, request).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: ListComments must answer 200, got {page:?}"
            );
            assert_eq!(
                page["total"].as_i64(),
                Some(bodies.len() as i64),
                "case maxResults={max_results}: total must be the stored comment count, got {:?}",
                page["total"]
            );
            for item in page["items"].as_array().cloned().unwrap_or_default() {
                seen.push(item["body"].as_str().unwrap_or_default().to_string());
            }
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, bodies,
            "case maxResults={max_results}: paging must enumerate every comment of the case exactly once, got {seen:?}"
        );
    }

    // Comments belong to the case they were posted on, not to the service.
    let (status, empty) = emulator.list_comments(region, &other_id, json!({})).await;
    assert_eq!(status, 200, "case the uncommented case: must answer 200");
    assert_eq!(
        empty["total"].as_i64(),
        Some(0),
        "case the uncommented case: must hold no comments, got {:?}",
        empty
    );
}

/// A `nextToken` this service never handed out is refused with the modelled
/// `ValidationException` (HTTP 400), on both paginated operations.
#[tokio::test]
async fn a_next_token_the_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (_, created) = emulator.create_case(region, create_request("a case", 0)).await;
    let case_id = created["caseId"].as_str().expect("a minted caseId").to_string();

    for token in ["", "not-a-token", "abc", "12345", "-000000001"] {
        let (status, body) = emulator
            .list_cases(region, json!({"nextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case ListCases nextToken={token:?}: must be refused 400, got {status} with {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case ListCases nextToken={token:?}: must be a ValidationException, got {:?}",
            body["__type"]
        );

        let (status, body) = emulator
            .list_comments(region, &case_id, json!({"nextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case ListComments nextToken={token:?}: must be refused 400, got {status} with {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case ListComments nextToken={token:?}: must be a ValidationException, got {:?}",
            body["__type"]
        );
    }
}

/// The status and pending-action values this service stores are values the model
/// declares — the catalogue comes from the generated `*_VALUES` constants, which are the
/// Smithy enums' own members.
#[test]
fn modelled_value_sets_back_the_stored_constants() {
    for (label, value, catalogue) in [
        ("CASE_STATUS_SUBMITTED", CASE_STATUS_SUBMITTED, CASE_STATUS_VALUES),
        ("CASE_STATUS_CLOSED", CASE_STATUS_CLOSED, CASE_STATUS_VALUES),
        ("PENDING_ACTION_NONE", PENDING_ACTION_NONE, PENDING_ACTION_VALUES),
    ] {
        assert!(
            catalogue.contains(&value),
            "case {label}: {value:?} must be one of the modelled values {catalogue:?}"
        );
    }

    // The lifecycle table above submits every value of both of these enums, so the rule
    // is asserted over each one and not just over the pair the issue cited.
    assert_eq!(
        RESOLVER_TYPE_VALUES.len(),
        2,
        "ResolverType gained a value the lifecycle table does not cover: {RESOLVER_TYPE_VALUES:?}"
    );
    assert_eq!(
        ENGAGEMENT_TYPE_VALUES.len(),
        2,
        "EngagementType gained a value the lifecycle table does not cover: {ENGAGEMENT_TYPE_VALUES:?}"
    );
    let submitted: Vec<String> = lifecycle_table()
        .iter()
        .map(|row| row.request["resolverType"].as_str().unwrap_or_default().to_string())
        .collect();
    for value in RESOLVER_TYPE_VALUES {
        assert!(
            submitted.iter().any(|seen| seen == value),
            "case ResolverType {value:?}: the lifecycle table must submit it, submitted {submitted:?}"
        );
    }
    let submitted: Vec<String> = lifecycle_table()
        .iter()
        .map(|row| row.request["engagementType"].as_str().unwrap_or_default().to_string())
        .collect();
    for value in ENGAGEMENT_TYPE_VALUES {
        assert!(
            submitted.iter().any(|seen| seen == value),
            "case EngagementType {value:?}: the lifecycle table must submit it, submitted {submitted:?}"
        );
    }
}

/// The page builder never hands back more than the server-side maximum, whatever page
/// size reaches it — the bound does not depend on the request being refused upstream.
#[test]
fn the_page_builder_never_exceeds_the_server_side_maximum() {
    // The shared ceiling, over the whole `i32` a request could carry: absent (0 and the
    // negatives the wire can still deliver) takes the default, anything above the
    // ceiling is cut down to it, anything inside it is honoured.
    for (requested, expected) in [
        (i32::MIN, DEFAULT_MAX_RESULTS as usize),
        (-1, DEFAULT_MAX_RESULTS as usize),
        (0, DEFAULT_MAX_RESULTS as usize),
        (1, 1),
        (MAX_PAGE_SIZE - 1, MAX_PAGE_SIZE as usize - 1),
        (MAX_PAGE_SIZE, MAX_PAGE_SIZE as usize),
        (MAX_PAGE_SIZE + 1, MAX_PAGE_SIZE as usize),
        (1_000, MAX_PAGE_SIZE as usize),
        (i32::MAX, MAX_PAGE_SIZE as usize),
    ] {
        assert_eq!(
            page_size(requested),
            expected,
            "case maxResults={requested}: the page size must be {expected}, got {}",
            page_size(requested)
        );
    }

    let stored: std::collections::BTreeMap<String, u8> = (0..(MAX_PAGE_SIZE as usize + 10))
        .map(|index| (format!("{index:010}"), 0u8))
        .collect();

    for requested in [
        MAX_PAGE_SIZE as usize,
        MAX_PAGE_SIZE as usize + 1,
        1_000,
        usize::from(u16::MAX),
        i32::MAX as usize,
    ] {
        let (page, next) = page_keys(&stored, None, requested);
        assert!(
            page.len() <= MAX_PAGE_SIZE as usize,
            "case requested={requested}: a page must hold at most {MAX_PAGE_SIZE} keys, got {}",
            page.len()
        );
        assert!(
            next.is_some(),
            "case requested={requested}: {} stored keys must leave a next token behind a capped page, got {next:?}",
            stored.len()
        );
    }
}

/// The idempotency map is bounded, and the oldest remembered token is the one evicted —
/// so a caller sending a fresh `clientToken` every request cannot grow it without limit.
#[test]
fn the_idempotency_map_is_bounded_and_evicts_the_oldest() {
    let mut state = ComAmazonawsSecurityirSecurityincidentresponseState::default();
    let overflow = 25usize;
    for index in 0..(MAX_IDEMPOTENCY_ENTRIES + overflow) {
        let key = idempotency_key("CreateCase", "", &format!("token-{index}"));
        state.record_idempotency(key, &format!("{index:010}"));
    }

    assert_eq!(
        state.idempotency.len(),
        MAX_IDEMPOTENCY_ENTRIES,
        "case overflow by {overflow}: the map must stay at its bound of {MAX_IDEMPOTENCY_ENTRIES}, got {}",
        state.idempotency.len()
    );
    for index in 0..overflow {
        let key = idempotency_key("CreateCase", "", &format!("token-{index}"));
        assert!(
            !state.idempotency.contains_key(&key),
            "case overflow by {overflow}: token-{index} is among the oldest and must have been evicted"
        );
    }
    for index in overflow..(MAX_IDEMPOTENCY_ENTRIES + overflow) {
        let key = idempotency_key("CreateCase", "", &format!("token-{index}"));
        assert_eq!(
            state.idempotency.get(&key).map(String::as_str),
            Some(format!("{index:010}").as_str()),
            "case overflow by {overflow}: token-{index} must still replay its minted id"
        );
    }
    // Re-recording a key already held is not a new entry, so it cannot push the map past
    // its bound either.
    let held = idempotency_key("CreateCase", "", &format!("token-{overflow}"));
    state.record_idempotency(held, "0000000099");
    assert_eq!(
        state.idempotency.len(),
        MAX_IDEMPOTENCY_ENTRIES,
        "case re-recording a held token: the map must stay at its bound, got {}",
        state.idempotency.len()
    );
}

/// One case stops accepting comments at the per-case bound, with the modelled
/// `ServiceQuotaExceededException` — never by minting an id the `^\d{6}$` pattern cannot
/// describe or by overwriting a stored comment.
#[tokio::test]
async fn a_case_stops_accepting_comments_at_the_per_case_bound() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (_, created) = emulator.create_case(region, create_request("a busy case", 0)).await;
    let case_id = created["caseId"].as_str().expect("a minted caseId").to_string();

    for index in 0..MAX_COMMENTS_PER_CASE {
        let (status, body) = emulator
            .create_comment(region, &case_id, json!({"body": format!("comment {index}")}))
            .await;
        assert_eq!(
            status, 201,
            "case comment {index} of {MAX_COMMENTS_PER_CASE}: must be accepted, got {status} with {body:?}"
        );
        assert!(
            is_comment_id(body["commentId"].as_str().unwrap_or_default()),
            "case comment {index}: commentId must stay six digits, got {:?}",
            body["commentId"]
        );
    }

    let (status, refused) = emulator
        .create_comment(region, &case_id, json!({"body": "one too many"}))
        .await;
    assert_eq!(
        status, 402,
        "case one comment past the bound: must be refused 402, got {status} with {refused:?}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case one comment past the bound: must be a ServiceQuotaExceededException, got {:?}",
        refused["__type"]
    );

    let (_, comments) = emulator.list_comments(region, &case_id, json!({})).await;
    assert_eq!(
        comments["total"].as_i64(),
        Some(MAX_COMMENTS_PER_CASE as i64),
        "case after the refusal: the case must still hold exactly {MAX_COMMENTS_PER_CASE} comments, got {:?}",
        comments["total"]
    );
}

/// The case store stops accepting cases at its bound, with the modelled
/// `ServiceQuotaExceededException` — it does not grow without limit.
#[tokio::test]
async fn the_case_store_stops_accepting_cases_at_its_bound() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    for index in 0..MAX_STORED_CASES {
        let (status, body) = emulator
            .create_case(region, create_request(&format!("case {index}"), index as u32))
            .await;
        assert_eq!(
            status, 201,
            "case {index} of {MAX_STORED_CASES}: must be accepted, got {status} with {body:?}"
        );
    }

    let (status, refused) = emulator
        .create_case(region, create_request("one too many", 0))
        .await;
    assert_eq!(
        status, 402,
        "case one past the bound: must be refused 402, got {status} with {refused:?}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "case one past the bound: must be a ServiceQuotaExceededException, got {:?}",
        refused["__type"]
    );

    let (_, listed) = emulator.list_cases(region, json!({})).await;
    assert_eq!(
        listed["total"].as_i64(),
        Some(MAX_STORED_CASES as i64),
        "case after the refusal: the store must still hold exactly {MAX_STORED_CASES} cases, got {:?}",
        listed["total"]
    );
}

/// A minimal, valid `CreateCase` body whose `title` and `reportedIncidentStartDate` vary
/// with `index`, for suites that care about identity rather than about member fidelity.
fn create_request(title: &str, index: u32) -> Value {
    json!({
        "resolverType": "Self",
        "engagementType": "Investigation",
        "title": title,
        "description": format!("description for {title}"),
        "reportedIncidentStartDate": 1_700_000_000 + i64::from(index),
        "impactedAccounts": ["000123123123"],
        "watchers": [{"email": "sec@example.com"}],
        "threatActorIpAddresses": [],
        "impactedServices": [],
        "impactedAwsRegions": [],
    })
}

/// The first item of a list response whose `member` equals `value`.
fn find_item(response: &Value, member: &str, value: &str) -> Option<Value> {
    response["items"]
        .as_array()?
        .iter()
        .find(|item| item[member].as_str() == Some(value))
        .cloned()
}
