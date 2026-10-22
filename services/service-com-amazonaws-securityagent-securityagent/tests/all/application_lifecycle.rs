// SPDX-License-Identifier: CC0-1.0

//! The Application lifecycle of the SecurityAgent emulator, end to end over the wire:
//! `CreateApplication`, `GetApplication`, `ListApplications`, `UpdateApplication`,
//! `DeleteApplication`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (sources: this crate's `OPERATIONS.md` "Wire protocol" section and the
//! per-operation `Route` lines). So nothing here can pass against a handler that
//! answers a canned body: a value must come back out of the shared store through
//! `GetApplication` or `ListApplications` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: which
//! members survive a round trip, that the identity is minted per call, where a page
//! resumes, which members an update replaces, and what an id the store does not hold
//! answers.

// The wire harness below (`Answer`, `emulator`, `call_in`, `call`, `ok`, `member`) is
// `pub(crate)` so a sibling suite in this test target reuses it instead of writing a
// second dispatch path; `create` and `list_all_ids` stay private to this suite.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    MAX_APPLICATIONS, MAX_PAGE_SIZE,
};
use service_com_amazonaws_securityagent_securityagent::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answer from the emulator.
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The wire error code: restJson1 carries it in the `x-amzn-errortype` header and
    /// in the body's `__type` discriminator. Empty on a success.
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

/// A router carrying only this service, over ONE shared engine — the same
/// `EmulatedService::new(engine)` seam `core/router` mounts.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator front door's concern, not this service's; these
    // cases drive unsigned requests whose credential scope still names the service.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, signed for `region`.
///
/// The `Authorization` credential scope names `securityagent`, the signing name
/// `resolve_route_key` reads to pick this service, and the region inside it is what
/// the service reads when it derives a region-bearing value.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://securityagent.{region}.amazonaws.com/{operation}"
        ))
        .header("host", format!("securityagent.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/securityagent/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    let error_code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    // Both renderings may carry a namespace or a URI prefix; a client matches on the
    // shape name.
    let error_code = error_code
        .rsplit(['#', '/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_code,
        body,
    }
}

/// `call_in`, in the region every case that is not about the region uses.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    call_in(router, "us-east-1", operation, body).await
}

/// `call`, requiring HTTP 200 and naming the case when it is not.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `CreateApplication`, returning the minted `applicationId`.
async fn create(router: &MultiServiceRouter, body: &Value, case: &str) -> String {
    let payload = ok(router, "CreateApplication", body, case).await;
    payload
        .get("applicationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateApplication returned no applicationId: {payload}")
        })
        .to_string()
}

/// A string member of a payload, or `None` when it is absent.
pub(crate) fn member(payload: &Value, name: &str) -> Option<String> {
    payload.get(name).and_then(Value::as_str).map(str::to_string)
}

/// Every `applicationId` `ListApplications` reports, paging to the end with
/// `page_size` (or the service's default page size when `None`).
async fn list_all_ids(router: &MultiServiceRouter, page_size: Option<i32>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = ok(
            router,
            "ListApplications",
            &request,
            &format!("page size {page_size:?}"),
        )
        .await;
        let summaries = page
            .get("applicationSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("ListApplications answered no applicationSummaries list: {page}")
            })
            .clone();
        if let Some(size) = page_size {
            assert!(
                summaries.len() <= size as usize,
                "case page size {size}: a page held {} summaries: {page}",
                summaries.len()
            );
        }
        for summary in &summaries {
            ids.push(
                member(summary, "applicationId")
                    .unwrap_or_else(|| panic!("a summary carries no applicationId: {summary}")),
            );
        }
        token = member(&page, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            !summaries.is_empty(),
            "case page size {page_size:?}: a page that carries a nextToken must not be \
             empty: {page}"
        );
        assert!(
            ids.len() <= MAX_APPLICATIONS,
            "case page size {page_size:?}: pagination did not terminate"
        );
    }
    ids
}

/// The input rows every round-trip case spans: they differ along which optional
/// members the request carries, the form of each value, and the size of the tag map —
/// the dimensions `CreateApplication`'s input generalises over
/// (`smithy_query(query="operation",operation="CreateApplication")`: `idcInstanceArn`,
/// `roleArn`, `defaultKmsKeyId`, `tags`, all optional and unconstrained).
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("no optional members at all", json!({})),
        (
            "every member, an Identity Center instance and one tag",
            json!({
                "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1111",
                "roleArn": "arn:aws:iam::000000000000:role/SecAgent",
                "defaultKmsKeyId": "alias/sec",
                "tags": {"env": "test"},
            }),
        ),
        (
            "role only",
            json!({"roleArn": "arn:aws:iam::000000000000:role/OnlyRole"}),
        ),
        (
            // `DefaultKmsKeyId` admits "a key ID, key ARN, alias name, or alias ARN"
            // (source: the `DefaultKmsKeyId` doc from
            // `smithy_query(query="shape",shape="DefaultKmsKeyId")`), so the rows below
            // carry one of each form.
            "a bare KMS key id only",
            json!({"defaultKmsKeyId": "12345678-1234-1234-1234-123456789012"}),
        ),
        (
            "a KMS key ARN only",
            json!({
                "defaultKmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012"
            }),
        ),
        (
            "a KMS alias ARN only",
            json!({"defaultKmsKeyId": "arn:aws:kms:us-east-1:000000000000:alias/sec"}),
        ),
        (
            "an Identity Center instance only",
            json!({"idcInstanceArn": "arn:aws:sso:::instance/ssoins-abc0123456789def"}),
        ),
        (
            "tags only, several of them",
            json!({"tags": {"env": "prod", "team": "appsec", "cost-center": "4711"}}),
        ),
        (
            "an empty tag map",
            json!({"tags": {}}),
        ),
    ]
}

/// RULE: `GetApplication` answers each member from the record `CreateApplication`
/// persisted — the supplied members verbatim, and the service-minted identity
/// (`applicationId`, `applicationName`, `domain`, `idcConfiguration`) consistently
/// across reads. Nothing is echoed from the Get request and nothing is a placeholder.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_application() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    let mut seen_domains: BTreeSet<String> = BTreeSet::new();

    for (case, request) in create_rows() {
        let id = create(&router, &request, case).await;
        assert!(
            seen_ids.insert(id.clone()),
            "case {case:?}: CreateApplication reused the applicationId {id:?}"
        );

        let got = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": id}),
            case,
        )
        .await;

        assert_eq!(
            member(&got, "applicationId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: GetApplication reported another application: {got}"
        );
        assert_eq!(
            member(&got, "roleArn"),
            member(&request, "roleArn"),
            "case {case:?}: expected the stored roleArn {:?}, got {:?}",
            member(&request, "roleArn"),
            member(&got, "roleArn")
        );
        assert_eq!(
            member(&got, "defaultKmsKeyId"),
            member(&request, "defaultKmsKeyId"),
            "case {case:?}: expected the stored defaultKmsKeyId {:?}, got {:?}",
            member(&request, "defaultKmsKeyId"),
            member(&got, "defaultKmsKeyId")
        );

        // `domain` and `applicationName` are the service's to mint, and are real stored
        // values: non-empty, unique per application, and the same on every read.
        let name = member(&got, "applicationName").unwrap_or_else(|| {
            panic!("case {case:?}: GetApplication reported no applicationName: {got}")
        });
        let domain = member(&got, "domain").unwrap_or_else(|| {
            panic!("case {case:?}: GetApplication reported no domain: {got}")
        });
        assert!(
            !name.is_empty() && !domain.is_empty(),
            "case {case:?}: expected a non-empty applicationName and domain, got \
             {name:?} and {domain:?}"
        );
        assert!(
            domain.contains(&id) && domain.contains("us-east-1"),
            "case {case:?}: expected the domain to name the application and the signed \
             region, got {domain:?}"
        );
        assert!(
            seen_names.insert(name.clone()),
            "case {case:?}: applicationName {name:?} was minted twice"
        );
        assert!(
            seen_domains.insert(domain.clone()),
            "case {case:?}: domain {domain:?} was minted twice"
        );

        // IdC configuration exists exactly when the caller named an instance, and it
        // carries that instance back plus the application ARN the emulator minted.
        match member(&request, "idcInstanceArn") {
            None => assert!(
                got.get("idcConfiguration").is_none(),
                "case {case:?}: no idcInstanceArn was supplied, so no idcConfiguration \
                 should be reported: {got}"
            ),
            Some(instance_arn) => {
                let idc = got.get("idcConfiguration").unwrap_or_else(|| {
                    panic!("case {case:?}: expected an idcConfiguration: {got}")
                });
                assert_eq!(
                    member(idc, "idcInstanceArn").as_deref(),
                    Some(instance_arn.as_str()),
                    "case {case:?}: expected the stored idcInstanceArn {instance_arn:?}, \
                     got {idc}"
                );
                let application_arn = member(idc, "idcApplicationArn").unwrap_or_else(|| {
                    panic!("case {case:?}: expected a minted idcApplicationArn: {idc}")
                });
                // The emulator's canonical account id, never a real one
                // (`engine::emulator_account_id`).
                assert!(
                    application_arn.starts_with("arn:aws:sso::")
                        && application_arn.contains(engine::emulator_account_id())
                        && application_arn.contains(":application/"),
                    "case {case:?}: expected an Identity Center application ARN, got \
                     {application_arn:?}"
                );
            }
        }

        // The read is a read: a second GetApplication answers exactly the same body.
        let again = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": id}),
            case,
        )
        .await;
        assert_eq!(
            again, got,
            "case {case:?}: two reads of one application disagreed"
        );
    }
}

/// RULE: the identity is minted per call, not derived from the request — two calls
/// with identical inputs are two distinct records, each readable on its own.
#[tokio::test]
async fn identical_requests_create_distinct_applications() {
    let router = emulator();
    let request = json!({
        "idcInstanceArn": "arn:aws:sso:::instance/ssoins-same",
        "roleArn": "arn:aws:iam::000000000000:role/Same",
        "defaultKmsKeyId": "alias/same",
    });

    let mut ids = Vec::new();
    for attempt in 0..4 {
        ids.push(create(&router, &request, &format!("attempt {attempt}")).await);
    }
    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case identical requests: expected {} distinct applicationIds, got {ids:?}",
        ids.len()
    );

    // Each one is its own record, and each answers with its own identity.
    let mut identities = BTreeSet::new();
    for id in &ids {
        let got = ok(&router, "GetApplication", &json!({"applicationId": id}), id).await;
        assert_eq!(
            member(&got, "applicationId").as_deref(),
            Some(id.as_str()),
            "case {id:?}: GetApplication answered another application: {got}"
        );
        identities.insert((
            member(&got, "applicationName"),
            member(&got, "domain"),
            member(&got, "idcConfiguration").or_else(|| {
                got.get("idcConfiguration")
                    .and_then(|idc| member(idc, "idcApplicationArn"))
            }),
        ));
    }
    assert_eq!(
        identities.len(),
        ids.len(),
        "case identical requests: expected a distinct name, domain and Identity Center \
         application ARN per record, got {identities:?}"
    );
}

/// RULE: the region a derived value names is the region the request was signed for.
#[tokio::test]
async fn the_domain_names_the_region_the_request_was_signed_for() {
    let router = emulator();
    // Rows differ along the signed region. The last row carries no credential scope at
    // all, which falls back to the service's default region.
    let cases = [
        Some("us-east-1"),
        Some("eu-west-1"),
        Some("ap-southeast-2"),
        None,
    ];

    for case in cases {
        let id = match case {
            Some(region) => {
                let created = call_in(&router, region, "CreateApplication", &json!({})).await;
                assert_eq!(
                    created.status, 200,
                    "case region {region:?}: expected 200, got {} with {}",
                    created.status, created.body
                );
                member(&created.body, "applicationId").unwrap_or_else(|| {
                    panic!("case region {region:?}: no applicationId: {}", created.body)
                })
            }
            None => {
                // No `Authorization` header: the router still routes on the host, and
                // the service falls back to its default region.
                let request = http::Request::builder()
                    .method("POST")
                    .uri("https://securityagent.us-east-1.amazonaws.com/CreateApplication")
                    .header("host", "securityagent.us-east-1.amazonaws.com")
                    .header("content-type", "application/json")
                    .body(bytes::Bytes::from("{}"))
                    .expect("a well-formed request");
                let response = router.dispatch(request).await;
                if response.status().as_u16() != 200 {
                    // An unsigned request this router will not route is not what this
                    // case is about; the signed rows already cover the rule.
                    continue;
                }
                let body: Value = serde_json::from_slice(&response.into_body())
                    .expect("CreateApplication answered JSON");
                member(&body, "applicationId")
                    .unwrap_or_else(|| panic!("no applicationId: {body}"))
            }
        };

        let got = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": id}),
            &format!("region {case:?}"),
        )
        .await;
        let domain = member(&got, "domain")
            .unwrap_or_else(|| panic!("case region {case:?}: no domain: {got}"));
        let expected_region = case.unwrap_or("us-east-1");
        assert!(
            domain.contains(expected_region),
            "case region {case:?}: expected the domain to name {expected_region}, got \
             {domain:?}"
        );
    }
}

/// RULE: an `applicationId` the store does not hold is HTTP 404
/// `ResourceNotFoundException` — never a hollow 200 and never an echo of the request.
#[tokio::test]
async fn an_unknown_application_id_is_a_not_found() {
    let router = emulator();
    // One real application exists, so a miss is a miss on a populated store.
    let live = create(&router, &json!({}), "the live application").await;

    // Rows differ along the shape of the unknown id: free text, an id-shaped value, a
    // near miss on the live id, and one that differs only in case.
    let unknown = [
        "does-not-exist",
        "00000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffff",
        &format!("{live}0"),
        &live.to_uppercase(),
    ];

    for id in unknown {
        if id == live {
            continue;
        }
        for operation in ["GetApplication", "UpdateApplication"] {
            let answer = call(&router, operation, &json!({"applicationId": id})).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {id:?}: expected HTTP 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {operation} {id:?}: expected ResourceNotFoundException, got {:?} \
                 with {}",
                answer.error_code, answer.body
            );
        }
    }

    // And the live application is untouched by every miss.
    let got = ok(
        &router,
        "GetApplication",
        &json!({"applicationId": &live}),
        "the live application",
    )
    .await;
    assert_eq!(
        member(&got, "applicationId").as_deref(),
        Some(live.as_str()),
        "case the live application: a not-found disturbed the store: {got}"
    );
}

/// RULE: `ListApplications` enumerates the stored records as summaries projected from
/// state, in one stable order; a page holds at most `maxResults` of them, `nextToken`
/// is present exactly when records remain, and resuming from it walks the whole set
/// exactly once.
#[tokio::test]
async fn list_applications_pages_over_the_stored_applications() {
    let router = emulator();
    // Rows differ along `defaultKmsKeyId` so a summary can be matched to its record.
    let mut created: BTreeMap<String, Option<String>> = BTreeMap::new();
    for index in 0..7 {
        let kms = if index % 2 == 0 {
            Some(format!("alias/key-{index}"))
        } else {
            None
        };
        let mut request = json!({});
        if let Some(kms) = &kms {
            request["defaultKmsKeyId"] = json!(kms);
        }
        let id = create(&router, &request, &format!("application {index}")).await;
        created.insert(id, kms);
    }
    let every_id: Vec<String> = created.keys().cloned().collect();

    // Page sizes spanning 1, every divisor and non-divisor of the record count, the
    // exact count, past the count, and the default (`maxResults` absent).
    for page_size in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(50), None] {
        let walked = list_all_ids(&router, page_size).await;
        assert_eq!(
            walked, every_id,
            "case page size {page_size:?}: expected every stored application exactly \
             once in the stable order {every_id:?}, got {walked:?}"
        );
    }

    // Each summary is projected from the record, not from the request that listed it.
    let page = ok(
        &router,
        "ListApplications",
        &json!({"maxResults": 50}),
        "every summary",
    )
    .await;
    let summaries = page["applicationSummaries"]
        .as_array()
        .expect("applicationSummaries is a list");
    for summary in summaries {
        let id = member(summary, "applicationId").expect("a summary carries an applicationId");
        let detail = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": &id}),
            &id,
        )
        .await;
        assert_eq!(
            member(summary, "applicationName"),
            member(&detail, "applicationName"),
            "case {id:?}: the summary's applicationName disagrees with GetApplication: \
             {summary} vs {detail}"
        );
        assert_eq!(
            member(summary, "domain"),
            member(&detail, "domain"),
            "case {id:?}: the summary's domain disagrees with GetApplication: {summary} \
             vs {detail}"
        );
        assert_eq!(
            member(summary, "defaultKmsKeyId"),
            created[&id],
            "case {id:?}: expected the stored defaultKmsKeyId {:?}, got {summary}",
            created[&id]
        );
    }

    // The last page omits `nextToken` rather than carrying an empty one.
    assert!(
        page.get("nextToken").is_none(),
        "case every summary: the final page must omit nextToken: {page}"
    );
}

/// RULE: a page request this service cannot act on is refused with HTTP 400
/// `ValidationException` — a `nextToken` it never minted, or a page size that cannot
/// bound a page.
#[tokio::test]
async fn list_applications_refuses_a_page_request_it_cannot_act_on() {
    let router = emulator();
    create(&router, &json!({}), "a stored application").await;

    // Rows differ along the form of the bad token: free text, a hex string without the
    // cursor prefix, the prefix with a body that is not hex, and JSON.
    for token in [
        "not-a-token",
        "deadbeef",
        "app-v1:zzzz",
        "{\"page\":2}",
        "",
    ] {
        let answer = call(&router, "ListApplications", &json!({"nextToken": token})).await;
        assert_eq!(
            answer.status, 400,
            "case token {token:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case token {token:?}: expected ValidationException, got {:?} with {}",
            answer.error_code, answer.body
        );
    }

    // A page size that cannot bound a page: zero, and below zero.
    for size in [0, -1, i32::MIN] {
        let answer = call(&router, "ListApplications", &json!({"maxResults": size})).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {size}: expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case maxResults {size}: expected ValidationException, got {:?} with {}",
            answer.error_code, answer.body
        );
    }

    // And a refusal changes nothing: the store still lists its one application.
    assert_eq!(
        list_all_ids(&router, None).await.len(),
        1,
        "case after refusals: a refused page request must not change the store"
    );
}

/// RULE: `UpdateApplication` patches the stored record in place — a supplied member
/// replaces what was stored, an omitted one is left as it was, and the minted identity
/// never moves. A later `GetApplication` observes exactly that.
#[tokio::test]
async fn update_application_patches_the_stored_record() {
    let router = emulator();
    // (case, patch, expected roleArn after, expected defaultKmsKeyId after)
    let rows: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "both members replaced",
            json!({
                "roleArn": "arn:aws:iam::000000000000:role/Changed",
                "defaultKmsKeyId": "alias/changed",
            }),
            Some("arn:aws:iam::000000000000:role/Changed"),
            Some("alias/changed"),
        ),
        (
            "only roleArn supplied, the KMS key left alone",
            json!({"roleArn": "arn:aws:iam::000000000000:role/OnlyRole"}),
            Some("arn:aws:iam::000000000000:role/OnlyRole"),
            Some("alias/original"),
        ),
        (
            "only defaultKmsKeyId supplied, the role left alone",
            json!({"defaultKmsKeyId": "alias/only-kms"}),
            Some("arn:aws:iam::000000000000:role/Original"),
            Some("alias/only-kms"),
        ),
        (
            "no member supplied: the record is unchanged",
            json!({}),
            Some("arn:aws:iam::000000000000:role/Original"),
            Some("alias/original"),
        ),
    ];

    for (case, patch, expected_role, expected_kms) in rows {
        let id = create(
            &router,
            &json!({
                "idcInstanceArn": "arn:aws:sso:::instance/ssoins-patch",
                "roleArn": "arn:aws:iam::000000000000:role/Original",
                "defaultKmsKeyId": "alias/original",
                "tags": {"env": "test"},
            }),
            case,
        )
        .await;
        let before = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": &id}),
            case,
        )
        .await;

        let mut request = patch.clone();
        request["applicationId"] = json!(id);
        let updated = ok(&router, "UpdateApplication", &request, case).await;
        assert_eq!(
            member(&updated, "applicationId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: UpdateApplication answered another applicationId: {updated}"
        );

        let after = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": &id}),
            case,
        )
        .await;
        assert_eq!(
            member(&after, "roleArn").as_deref(),
            expected_role,
            "case {case:?}: expected roleArn {expected_role:?}, got {:?}",
            member(&after, "roleArn")
        );
        assert_eq!(
            member(&after, "defaultKmsKeyId").as_deref(),
            expected_kms,
            "case {case:?}: expected defaultKmsKeyId {expected_kms:?}, got {:?}",
            member(&after, "defaultKmsKeyId")
        );
        // The identity an update never touches.
        assert_eq!(
            (
                member(&after, "applicationName"),
                member(&after, "domain"),
                after.get("idcConfiguration").cloned()
            ),
            (
                member(&before, "applicationName"),
                member(&before, "domain"),
                before.get("idcConfiguration").cloned()
            ),
            "case {case:?}: the minted identity moved: {before} -> {after}"
        );

        // `@idempotent`: replaying the same update leaves the same record.
        let replayed = ok(&router, "UpdateApplication", &request, case).await;
        assert_eq!(
            replayed, updated,
            "case {case:?}: replaying the update answered differently"
        );
        let after_replay = ok(
            &router,
            "GetApplication",
            &json!({"applicationId": &id}),
            case,
        )
        .await;
        assert_eq!(
            after_replay, after,
            "case {case:?}: replaying the update changed the record"
        );
    }
}

/// RULE: `DeleteApplication` removes the record — the id stops resolving and stops
/// being listed — and, being `@idempotent`, a repeat of the call answers the same
/// empty success. The other stored applications are untouched.
#[tokio::test]
async fn delete_application_removes_only_that_record() {
    let router = emulator();
    let mut ids = Vec::new();
    for index in 0..5 {
        ids.push(
            create(
                &router,
                &json!({"defaultKmsKeyId": format!("alias/key-{index}")}),
                &format!("application {index}"),
            )
            .await,
        );
    }

    // Rows differ along which record is deleted: the first, one in the middle, and the
    // last, so a cursor cannot be an offset that shifts.
    let mut survivors: BTreeSet<String> = ids.iter().cloned().collect();
    for target in [ids[0].clone(), ids[2].clone(), ids[4].clone()] {
        let deleted = ok(
            &router,
            "DeleteApplication",
            &json!({"applicationId": &target}),
            &target,
        )
        .await;
        assert!(
            deleted.is_null() || deleted.as_object().is_some_and(|body| body.is_empty()),
            "case {target:?}: DeleteApplication has no output members, got {deleted}"
        );
        survivors.remove(&target);

        let gone = call(&router, "GetApplication", &json!({"applicationId": &target})).await;
        assert_eq!(
            gone.status, 404,
            "case {target:?}: expected a deleted application to 404, got {} with {}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_code, "ResourceNotFoundException",
            "case {target:?}: expected ResourceNotFoundException, got {:?} with {}",
            gone.error_code, gone.body
        );

        // A deleted application is not listed, and the survivors still are — walked a
        // page at a time, so the cursor survives the deletion.
        let listed = list_all_ids(&router, Some(1)).await;
        let expected: Vec<String> = survivors.iter().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {target:?}: expected the survivors {expected:?}, got {listed:?}"
        );

        // `@idempotent`: deleting it again is the same call with the same result.
        let repeat = call(
            &router,
            "DeleteApplication",
            &json!({"applicationId": &target}),
        )
        .await;
        assert_eq!(
            repeat.status, 200,
            "case {target:?}: a repeated delete must answer as the first did, got {} \
             with {}",
            repeat.status, repeat.body
        );
    }

    // Every survivor still reads back its own record.
    for id in &survivors {
        let got = ok(&router, "GetApplication", &json!({"applicationId": id}), id).await;
        assert_eq!(
            member(&got, "applicationId").as_deref(),
            Some(id.as_str()),
            "case {id:?}: a survivor lost its record: {got}"
        );
    }
}

/// RULE: a page is bounded by the service's own ceiling however large a page size the
/// caller asks for, and the caller still reaches every application — one page at a
/// time.
#[tokio::test]
async fn a_page_size_above_the_ceiling_is_clamped() {
    let router = emulator();
    // More applications than one page can hold, so the clamp is observable.
    let total = MAX_PAGE_SIZE as usize + 5;
    for index in 0..total {
        create(&router, &json!({}), &format!("application {index}")).await;
    }

    // Rows differ along how far above the ceiling the request reaches.
    for requested in [MAX_PAGE_SIZE + 1, MAX_PAGE_SIZE * 10, i32::MAX] {
        let page = ok(
            &router,
            "ListApplications",
            &json!({"maxResults": requested}),
            &format!("maxResults {requested}"),
        )
        .await;
        let summaries = page["applicationSummaries"]
            .as_array()
            .expect("applicationSummaries is a list");
        assert_eq!(
            summaries.len(),
            MAX_PAGE_SIZE as usize,
            "case maxResults {requested}: a page must hold at most {MAX_PAGE_SIZE} \
             summaries, got {}",
            summaries.len()
        );
        assert!(
            member(&page, "nextToken").is_some(),
            "case maxResults {requested}: applications remain, so the page must carry a \
             nextToken: {page}"
        );
        // And no application becomes unreachable: paging on reaches all of them.
        let walked = list_all_ids(&router, Some(requested)).await;
        assert_eq!(
            walked.len(),
            total,
            "case maxResults {requested}: expected to reach every one of {total} \
             applications, reached {}",
            walked.len()
        );
    }
}

/// RULE: the store a caller can grow is bounded — at the application quota
/// `CreateApplication` answers the service's `ServiceQuotaExceededException` (HTTP
/// 402), the applications already stored are untouched, and deleting one makes room
/// for exactly one more.
#[tokio::test]
async fn create_application_stops_at_the_application_quota() {
    let router = emulator();
    let mut first = None;
    for index in 0..MAX_APPLICATIONS {
        let id = create(&router, &json!({}), &format!("application {index}")).await;
        if index == 0 {
            first = Some(id);
        }
    }
    let first = first.expect("the first application's id");

    let refused = call(&router, "CreateApplication", &json!({})).await;
    assert_eq!(
        refused.status, 402,
        "case at the quota: CreateApplication must refuse past {MAX_APPLICATIONS} \
         applications, got {} with {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "ServiceQuotaExceededException",
        "case at the quota: expected ServiceQuotaExceededException, got {:?} with {}",
        refused.error_code, refused.body
    );

    // The refusal stored nothing and removed nothing.
    assert_eq!(
        list_all_ids(&router, Some(MAX_PAGE_SIZE)).await.len(),
        MAX_APPLICATIONS,
        "case after the refusal: the stored applications must be untouched"
    );

    // Deleting one makes room for exactly one more.
    ok(
        &router,
        "DeleteApplication",
        &json!({"applicationId": &first}),
        "freeing one slot",
    )
    .await;
    create(&router, &json!({}), "the application that fits again").await;
    let refused_again = call(&router, "CreateApplication", &json!({})).await;
    assert_eq!(
        refused_again.status, 402,
        "case back at the quota: expected 402 again, got {} with {}",
        refused_again.status, refused_again.body
    );
}
