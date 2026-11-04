// SPDX-License-Identifier: CC0-1.0

//! `ListFlowVersions`, end to end over HTTP: every version of one flow — its
//! DRAFT version first, then the numbered versions cut from it — reported in
//! version order and in pages.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.
//!
//! The numbered versions are placed in the store directly, through the same
//! `crate::state` snapshot the `CreateFlowVersion` handler stores, because the
//! listing's ordering and paging have to be exercised over version sets that
//! incremental minting cannot produce (gaps, and numbers at the widest the
//! modelled `NumericalVersion` admits). `flow_version_minting.rs` drives the
//! same listing over versions cut by `CreateFlowVersion` itself.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    service_state, FlowStatus, FlowVersionRecord, DRAFT_VERSION, MAX_FLOW_VERSIONS_PER_PAGE,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    /// The `version` member of every reported summary, in the order the
    /// response listed them.
    fn listed_versions(&self) -> Vec<String> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "flowVersionSummaries is not an array in {:?}",
                    self.body
                )
            })
            .iter()
            .map(|summary| {
                summary["version"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary carries no version: {summary:?}"))
                    .to_string()
            })
            .collect()
    }

    fn summaries(&self) -> Vec<Value> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn next_token(&self) -> Option<String> {
        self.member(&["nextToken"]).as_str().map(ToString::to_string)
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `CreateFlow`, the one implemented operation that mints the flow a version
    /// belongs to.
    async fn create_flow(&self, name: &str) -> Answer {
        let created = self
            .call(
                "POST",
                "/flows/",
                Some(json!({ "name": name, "executionRoleArn": EXECUTION_ROLE })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case setup {name}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    /// `ListFlowVersions` on any `FlowIdentifier` form, with or without the
    /// paging members.
    async fn list_flow_versions(
        &self,
        identifier: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> Answer {
        let escaped = identifier.replace(':', "%3A").replace('/', "%2F");
        let mut query: Vec<String> = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.call("GET", &format!("/flows/{escaped}/versions{query}"), None)
            .await
    }

    /// Place numbered versions in the store for `flow_id`, in the order given —
    /// which is deliberately NOT version order, so the listing's ordering is the
    /// handler's work and not an artefact of insertion. Each is the same
    /// snapshot of the flow that `CreateFlowVersion` stores, reported as
    /// `Prepared` so a numbered version is distinguishable from the DRAFT the
    /// flow was created with.
    fn store_versions(&self, flow_id: &str, versions: &[u32]) {
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let mut state = state.lock();
        let base = SystemTime::now();
        for &version in versions {
            let flow = state
                .flows
                .get(flow_id)
                .unwrap_or_else(|| panic!("case setup: the store holds flow {flow_id}"))
                .clone();
            let mut record = FlowVersionRecord::snapshot_of(
                &flow,
                version,
                None,
                base + Duration::from_secs(u64::from(version)),
            );
            record.status = FlowStatus::Prepared;
            state
                .flow_versions
                .insert((flow_id.to_string(), version), record);
        }
        // The flow records the highest version it has been cut at, exactly as a
        // sequence of `CreateFlowVersion` calls would have left it.
        if let Some(highest) = versions.iter().copied().max() {
            let flow = state.flows.get_mut(flow_id).expect("the stored flow");
            flow.versions_cut = flow.versions_cut.max(highest);
        }
    }
}

/// The listing a flow with these numbered versions reports: its DRAFT version
/// first, then each numbered version as the modelled `NumericalVersion` renders
/// it.
fn expected_listing(numbered: &[u32]) -> Vec<String> {
    ::std::iter::once(DRAFT_VERSION.to_string())
        .chain(numbered.iter().map(u32::to_string))
        .collect()
}

/// Every version of the flow is reported, DRAFT first and then the numbered
/// versions in version order, over a table of version sets that differ in COUNT
/// and in numbering: none cut yet, a single version, contiguous runs, a set with
/// gaps, and sets spanning the one-digit to two-digit boundary where lexical
/// order and version order disagree.
#[tokio::test]
async fn list_flow_versions_reports_the_draft_and_every_stored_version_in_version_order() {
    // Rows: (case, the versions stored, in the order they are stored).
    let cases: Vec<(&str, Vec<u32>)> = vec![
        ("no version cut yet", vec![]),
        ("one version", vec![1]),
        ("a contiguous run, stored in order", vec![1, 2, 3]),
        ("a contiguous run, stored in reverse", vec![3, 2, 1]),
        ("a set with gaps", vec![4, 1, 7]),
        (
            "crossing the one-digit boundary: 10 follows 9, though '10' sorts before '9'",
            vec![9, 10, 2],
        ),
        (
            "the widest version the modelled pattern admits",
            vec![99999, 5, 100],
        ),
        ("a full page: nine versions beside the DRAFT", vec![9, 8, 7, 6, 5, 4, 3, 2, 1]),
    ];

    for (case, stored) in cases {
        let emulator = Emulator::new();
        let flow = emulator.create_flow("version-listing-flow").await;
        let flow_id = flow.string(&["id"]);
        let flow_arn = flow.string(&["arn"]);
        emulator.store_versions(&flow_id, &stored);

        let listed = emulator.list_flow_versions(&flow_id, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListFlowVersions -> 200, got {listed:?}"
        );

        let mut expected = stored.clone();
        expected.sort_unstable();
        assert_eq!(
            listed.listed_versions(),
            expected_listing(&expected),
            "case {case}: expected the DRAFT version then the stored versions in version \
             order, got {:?}",
            listed.listed_versions()
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case}: a listing that fits in one page carries no nextToken, got {:?}",
            listed.next_token()
        );

        // Every summary names the flow the version belongs to, and reports that
        // version's own status and creation instant: the flow's own for the
        // DRAFT, the snapshot's for a numbered version.
        for summary in listed.summaries() {
            assert_eq!(
                summary["id"].as_str(),
                Some(flow_id.as_str()),
                "case {case}: a summary must name the flow it belongs to, got {summary:?}"
            );
            assert_eq!(
                summary["arn"].as_str(),
                Some(flow_arn.as_str()),
                "case {case}: a summary carries the ARN of its flow, got {summary:?}"
            );
            let is_draft = summary["version"].as_str() == Some("DRAFT");
            let expected_status = if is_draft { "NotPrepared" } else { "Prepared" };
            assert_eq!(
                summary["status"].as_str(),
                Some(expected_status),
                "case {case}: a summary reports that version's own status, got {summary:?}"
            );
            let created_at = summary["createdAt"].as_str().unwrap_or_else(|| {
                panic!("case {case}: expected createdAt to be a date-time string, got {summary:?}")
            });
            assert!(
                created_at.contains('T') && created_at.ends_with('Z'),
                "case {case}: expected createdAt in the modelled date-time wire form, \
                 got {created_at:?}"
            );
        }
    }
}

/// Both modelled `FlowIdentifier` forms select the same flow's versions, one
/// flow never reports another's, and listing changes nothing a later caller
/// sees.
#[tokio::test]
async fn list_flow_versions_resolves_every_identifier_form_and_isolates_each_flow() {
    let emulator = Emulator::new();

    let first = emulator.create_flow("first-versioned-flow").await;
    let second = emulator.create_flow("second-versioned-flow").await;
    let first_id = first.string(&["id"]);
    let first_arn = first.string(&["arn"]);
    let second_id = second.string(&["id"]);
    let second_arn = second.string(&["arn"]);

    emulator.store_versions(&first_id, &[2, 1]);
    emulator.store_versions(&second_id, &[1, 2, 3]);

    // Rows: each flow x each identifier form.
    for (case, identifier, expected) in [
        ("first flow, bare flowId", first_id.clone(), vec![1, 2]),
        ("first flow, flow ARN", first_arn.clone(), vec![1, 2]),
        ("second flow, bare flowId", second_id.clone(), vec![1, 2, 3]),
        ("second flow, flow ARN", second_arn.clone(), vec![1, 2, 3]),
    ] {
        let listed = emulator.list_flow_versions(&identifier, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListFlowVersions -> 200, got {listed:?}"
        );
        assert_eq!(
            listed.listed_versions(),
            expected_listing(&expected),
            "case {case}: a flow reports its own versions and no other flow's, got {:?}",
            listed.listed_versions()
        );
    }

    // A read changes nothing a later caller sees.
    let again = emulator.list_flow_versions(&first_id, None, None).await;
    assert_eq!(
        again.body,
        emulator
            .list_flow_versions(&first_arn, None, None)
            .await
            .body,
        "case repeated read: listing must not change what a later caller sees"
    );
    assert_eq!(
        again.listed_versions(),
        expected_listing(&[1, 2]),
        "case repeated read: the stored versions are still reported, got {:?}",
        again.listed_versions()
    );
}

/// `maxResults` truncates a page and `nextToken` resumes exactly after it, with
/// the modelled `flowVersionSummaries` length bound (..10) capping a page
/// however large `maxResults` is. Walking the pages yields every version of the
/// flow — the DRAFT and each numbered version — in version order, once.
#[tokio::test]
async fn list_flow_versions_pages_by_max_results_up_to_the_modelled_page_cap() {
    // Twelve numbered versions beside the DRAFT: more than one page at every
    // `maxResults` worth testing, and more than the modelled page cap.
    let stored: Vec<u32> = (1..=12).rev().collect();
    let mut numbered = stored.clone();
    numbered.sort_unstable();
    let all = expected_listing(&numbered);

    // Rows: the published bounds of the modelled `maxResults` range (1..1000),
    // values between them on either side of the page cap, and the member
    // omitted.
    for (case, max_results) in [
        ("maxResults omitted", None),
        ("maxResults at the range's lower bound", Some(1)),
        ("maxResults below the page cap", Some(5)),
        ("maxResults at the page cap", Some(10)),
        ("maxResults above the page cap", Some(11)),
        ("maxResults at the range's upper bound", Some(1000)),
    ] {
        let emulator = Emulator::new();
        let flow = emulator.create_flow("paged-versions-flow").await;
        let flow_id = flow.string(&["id"]);
        emulator.store_versions(&flow_id, &stored);

        let page_cap = max_results
            .map_or(MAX_FLOW_VERSIONS_PER_PAGE, |requested: i64| {
                usize::try_from(requested).unwrap_or(MAX_FLOW_VERSIONS_PER_PAGE)
            })
            .min(MAX_FLOW_VERSIONS_PER_PAGE);

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = emulator
                .list_flow_versions(&flow_id, max_results, token.as_deref())
                .await;
            assert_eq!(
                page.status, 200,
                "case {case}: expected ListFlowVersions -> 200, got {page:?}"
            );
            let versions = page.listed_versions();
            assert!(
                versions.len() <= page_cap,
                "case {case}: a page carries at most {page_cap} versions, got {versions:?}"
            );
            walked.extend(versions.clone());
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            match page.next_token() {
                None => break,
                Some(next) => {
                    assert_eq!(
                        versions.len(),
                        page_cap,
                        "case {case}: a page that carries a nextToken is a full page, \
                         got {versions:?}"
                    );
                    token = Some(next);
                }
            }
        }

        assert_eq!(
            walked, all,
            "case {case}: walking the pages must yield every version of the flow once, \
             in version order, got {walked:?}"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_cap),
            "case {case}: expected {} page(s) of {page_cap}, walked {pages}",
            all.len().div_ceil(page_cap)
        );
    }
}

/// A flow the store does not hold is a missing resource; an identifier of
/// neither modelled form, a `maxResults` outside the modelled range and a
/// `nextToken` this service never issued are bad requests.
#[tokio::test]
async fn list_flow_versions_refuses_an_unknown_flow_a_bad_identifier_and_a_foreign_token() {
    let emulator = Emulator::new();
    let flow = emulator.create_flow("the-only-flow").await;
    let flow_id = flow.string(&["id"]);
    let flow_arn = flow.string(&["arn"]);
    emulator.store_versions(&flow_id, &[1, 2]);

    // Rows: both identifier forms, over well-formed ids the store does not hold.
    let mut nearly = flow_id.clone();
    nearly.pop();
    nearly.push(if flow_id.ends_with('X') { 'Y' } else { 'X' });
    for unknown in ["0123456789".to_string(), "aZ09aZ09aZ".to_string(), nearly] {
        if unknown == flow_id {
            continue;
        }
        for (form, identifier) in [
            ("bare flowId", unknown.clone()),
            (
                "flow ARN",
                flow_arn.replace(flow_id.as_str(), unknown.as_str()),
            ),
        ] {
            let case = format!("{form} {identifier}");
            let listed = emulator.list_flow_versions(&identifier, None, None).await;
            assert_eq!(
                listed.status, 404,
                "case {case}: expected ListFlowVersions -> 404, got {listed:?}"
            );
            assert_eq!(
                listed.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {listed:?}"
            );
            let message = listed.string(&["message"]);
            assert!(
                message.contains("ListFlowVersions") && message.contains("Flow"),
                "case {case}: the refusal names the operation and the resource type, \
                 got {message:?}"
            );
        }
    }

    // Rows differ in HOW the identifier misses the modelled `FlowIdentifier`
    // forms: a flow name, ids of the wrong length or alphabet, and ARNs wrong in
    // the partition, the service, the account and the resource type.
    let malformed = [
        ("a flow name, not an identifier", "the-only-flow".to_string()),
        ("nine characters", "012345678".to_string()),
        ("eleven characters", "0123456789a".to_string()),
        (
            "a character outside the id alphabet",
            "0123_56789".to_string(),
        ),
        (
            "an ARN whose id is too short",
            flow_arn.replace(flow_id.as_str(), "012345678"),
        ),
        (
            "an ARN naming another service",
            flow_arn.replace(":bedrock:", ":s3:"),
        ),
        (
            "an ARN naming another resource type",
            flow_arn.replace(":flow/", ":agent/"),
        ),
        (
            "an ARN in another partition",
            flow_arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
    ];
    for (case, identifier) in malformed {
        let listed = emulator.list_flow_versions(&identifier, None, None).await;
        assert_eq!(
            listed.status, 400,
            "case {case} ({identifier}): expected ListFlowVersions -> 400, got {listed:?}"
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({identifier}): expected ValidationException, got {listed:?}"
        );
    }

    // Rows: tokens no page of this listing handed out — including one that is
    // numeric but wider than the modelled `NumericalVersion`, and numbers no
    // version is ever cut as, since versions are numbered from 1.
    for (case, token) in [
        ("a token from somewhere else", "not-a-pagination-token"),
        ("a token wider than a version number", "999999"),
        ("a signed number", "+1"),
        ("an empty-looking token", "0x1"),
        ("version zero", "0"),
        ("a zero-padded version number", "01"),
    ] {
        let listed = emulator
            .list_flow_versions(&flow_id, None, Some(token))
            .await;
        assert_eq!(
            listed.status, 400,
            "case {case} ({token}): expected ListFlowVersions -> 400, got {listed:?}"
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({token}): expected ValidationException, got {listed:?}"
        );
    }

    // `maxResults` is modelled `range 1..1000`: one past either published bound
    // is refused at the boundary, before the handler runs.
    for (case, max_results) in [
        ("one below the range's lower bound", 0),
        ("one above the range's upper bound", 1001),
    ] {
        let listed = emulator
            .list_flow_versions(&flow_id, Some(max_results), None)
            .await;
        assert_eq!(
            listed.status, 400,
            "case {case} (maxResults={max_results}): expected ListFlowVersions -> 400, \
             got {listed:?}"
        );
    }

    // Every refusal above left the stored versions exactly as they were.
    let listed = emulator.list_flow_versions(&flow_id, None, None).await;
    assert_eq!(
        listed.listed_versions(),
        expected_listing(&[1, 2]),
        "case after the refusals: the flow's versions are still reported, got {:?}",
        listed.listed_versions()
    );
}
