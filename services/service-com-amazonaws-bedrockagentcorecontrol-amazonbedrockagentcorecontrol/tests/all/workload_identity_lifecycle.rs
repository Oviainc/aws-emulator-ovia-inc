// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workload-identity operations
//! (`CreateWorkloadIdentity`, `GetWorkloadIdentity`, `ListWorkloadIdentities`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what a create writes is what the read and the listing report.
//!
//! The rule under test: a workload identity is persisted under the `name` the caller
//! chose — unique within the account, and the identifier every later read addresses it by
//! — and every operation that reports one reads that stored record, so the listing cannot
//! disagree with the read; the listing reports every stored identity in one deterministic
//! order, cut into pages of at most `maxResults` entries that resume exactly where the
//! previous page ended, and it writes nothing a later caller can see.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The modelled bounds of `ListWorkloadIdentities`'s `maxResults` (range 1..20) and the
/// default it carries, from `smithy_query operation=ListWorkloadIdentities`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;
const MAX_RESULTS_DEFAULT: usize = 10;

/// The modelled bounds of `WorkloadIdentityNameType` (len 3..255, pattern
/// `^[A-Za-z0-9_.-]+$`), from `smithy_query operation=CreateWorkloadIdentity`.
const NAME_MIN_LENGTH: usize = 3;
const NAME_MAX_LENGTH: usize = 255;

/// The members `WorkloadIdentityType` carries (`smithy_query shape=WorkloadIdentityType`),
/// which a listing must report exactly as `GetWorkloadIdentity` reports them.
const SHARED_MEMBERS: [&str; 2] = ["name", "workloadIdentityArn"];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on and
/// what the handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_identity(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    post(router, "/identities/CreateWorkloadIdentity", body).await
}

async fn get_identity(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    post(
        router,
        "/identities/GetWorkloadIdentity",
        &json!({"name": name}),
    )
    .await
}

async fn list_identities(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(max) = max_results {
        body["maxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["nextToken"] = json!(token);
    }
    post(router, "/identities/ListWorkloadIdentities", &body).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `workloadIdentities` of a listing response.
fn listed(body: &Value) -> Vec<Value> {
    body.get("workloadIdentities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `name` of every entry of a listing response, in the order reported.
fn listed_names(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|entry| text(entry, "name"))
        .collect()
}

/// The ARN an identity of this account carries: the token vault's `default`
/// workload-identity directory, ending in the identity's own name.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:workload-identity-directory/default/\
         workload-identity/{name}"
    )
}

/// A create persists what the caller configured and the read reports exactly that record.
///
/// Rows differ along what the stored identity is built from: the name's shape and length
/// (each character class `WorkloadIdentityNameType`'s pattern admits, at the minimum
/// length, in between, and at the maximum) and whether the request carried allowed OAuth2
/// return URLs (none, one, several) and tags.
#[tokio::test]
async fn a_created_workload_identity_is_read_back_as_it_was_stored() {
    let router = router();

    // (case, name, allowedResourceOauth2ReturnUrls, tags)
    let rows: [(&str, String, Option<Vec<&str>>, bool); 6] = [
        (
            "the shortest name the model admits, no URLs",
            "a".repeat(NAME_MIN_LENGTH),
            None,
            false,
        ),
        (
            "dots, dashes and underscores, one URL",
            "payments.service_v1-east".to_string(),
            Some(vec!["https://example.com/oauth2/callback"]),
            false,
        ),
        (
            "digits only, several URLs",
            "20261029".to_string(),
            Some(vec![
                "https://example.com/oauth2/callback",
                "myapp://auth/return",
                "http://localhost:8080/return",
            ]),
            false,
        ),
        (
            "mixed case with tags",
            "MixedCaseIdentity".to_string(),
            None,
            true,
        ),
        (
            "an empty URL list",
            "empty-url-list".to_string(),
            Some(vec![]),
            false,
        ),
        (
            "the longest name the model admits",
            "z".repeat(NAME_MAX_LENGTH),
            None,
            false,
        ),
    ];

    for (case, name, urls, tagged) in &rows {
        let mut body = json!({"name": name});
        if let Some(urls) = urls {
            body["allowedResourceOauth2ReturnUrls"] = json!(urls);
        }
        if *tagged {
            body["tags"] = json!({"owner": "platform", "env": "test"});
        }

        let (status, created) = create_identity(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} ({created})"
        );
        assert_eq!(
            text(&created, "name"),
            *name,
            "case {case:?}: expected the name the request carried, got {}",
            text(&created, "name")
        );
        assert_eq!(
            text(&created, "workloadIdentityArn"),
            expected_arn(name),
            "case {case:?}: expected the identity ARN of this account and Region, got {}",
            text(&created, "workloadIdentityArn")
        );

        let (status, fetched) = get_identity(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetWorkloadIdentity, got {status} ({fetched})"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: the read must report the {member} the create reported, got {} \
                 for {}",
                fetched[member], created[member]
            );
        }
        assert_eq!(
            fetched["allowedResourceOauth2ReturnUrls"],
            urls.as_ref().map_or(Value::Null, |urls| json!(urls)),
            "case {case:?}: expected the return URLs the request carried, got {}",
            fetched["allowedResourceOauth2ReturnUrls"]
        );
        // `createdTime` / `lastUpdatedTime` target the prelude `Timestamp` with no
        // `@timestampFormat`, so on this service's restJson1 wire each instant is
        // epoch-seconds as a NUMBER, and a freshly created identity has never been
        // updated since.
        let instants: Vec<f64> = ["createdTime", "lastUpdatedTime"]
            .iter()
            .map(|member| {
                fetched[*member].as_f64().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected {member} as epoch-seconds, got {}",
                        fetched[*member]
                    )
                })
            })
            .collect();
        assert!(
            instants[0] > 0.0,
            "case {case:?}: expected a createdTime after the epoch, got {}",
            instants[0]
        );
        assert_eq!(
            instants[0], instants[1],
            "case {case:?}: a never-updated identity reports lastUpdatedTime as its \
             createdTime, got {} for {}",
            instants[1], instants[0]
        );
    }

    // Every identity created above is listed, each entry exactly what the read reports.
    let mut expected_names: Vec<String> = rows.iter().map(|(_, name, ..)| name.clone()).collect();
    expected_names.sort();

    let (status, page) = list_identities(&router, Some(MAX_RESULTS_MAX), None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListWorkloadIdentities, got {status} ({page})"
    );
    assert_eq!(
        listed_names(&page),
        expected_names,
        "expected every stored identity in ascending name order, got {:?} for {expected_names:?}",
        listed_names(&page)
    );
    for entry in listed(&page) {
        let name = text(&entry, "name");
        let (_, fetched) = get_identity(&router, &name).await;
        for member in SHARED_MEMBERS {
            assert_eq!(
                entry[member], fetched[member],
                "case {name:?}: the listing must report the {member} GetWorkloadIdentity \
                 reports, got {} for {}",
                entry[member], fetched[member]
            );
        }
    }
}

/// An empty store lists nothing — an empty page, not a refusal — and the listing is a
/// read: it reports what the store holds and changes nothing a later caller sees.
#[tokio::test]
async fn a_workload_identity_listing_is_a_read() {
    let router = router();

    let (status, empty) = list_identities(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"an empty store\": expected HTTP 200, got {status} ({empty})"
    );
    assert!(
        listed(&empty).is_empty(),
        "case \"an empty store\": expected no entries, got {empty}"
    );
    assert!(
        empty.get("nextToken").is_none_or(Value::is_null),
        "case \"an empty store\": expected no nextToken, got {}",
        empty["nextToken"]
    );

    for name in ["alpha-identity", "beta-identity"] {
        let (status, created) = create_identity(&router, &json!({"name": name})).await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} \
             ({created})"
        );
    }

    let (_, first) = list_identities(&router, None, None).await;
    let (_, again) = list_identities(&router, None, None).await;
    assert_eq!(
        again, first,
        "case \"a second listing\": expected the same answer, got {again} for {first}"
    );
    assert_eq!(
        listed_names(&first),
        vec!["alpha-identity".to_string(), "beta-identity".to_string()],
        "case \"both identities stored\": expected both in name order, got {:?}",
        listed_names(&first)
    );

    // The read left the identities readable, unchanged, through the single-identity read.
    for name in ["alpha-identity", "beta-identity"] {
        let (status, fetched) = get_identity(&router, name).await;
        assert_eq!(
            (status, text(&fetched, "workloadIdentityArn")),
            (200, expected_arn(name)),
            "case {name:?}: expected the stored identity after two listings, got {status} \
             ({fetched})"
        );
    }
}

/// The listing pages over every stored identity: each page holds at most `maxResults`
/// entries, resumes where the previous one ended, the last carries no token, and the whole
/// walk reports every identity exactly once in the one order — for page sizes below, at
/// and above the number stored, including each modelled bound. A request carrying no
/// `maxResults` pages at the modelled default.
#[tokio::test]
async fn a_workload_identity_listing_pages_over_every_stored_identity() {
    let router = router();

    // More identities than the modelled default page size, so the default is observed
    // cutting a page rather than reporting everything.
    let mut expected_names = Vec::new();
    for index in 0..(MAX_RESULTS_DEFAULT + 3) {
        let name = format!("identity-{index:02}");
        let (status, created) = create_identity(&router, &json!({"name": name})).await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} \
             ({created})"
        );
        expected_names.push(name);
    }
    expected_names.sort();

    // (case, maxResults)
    let rows: [(&str, Option<i64>); 5] = [
        ("no maxResults, so the modelled default", None),
        ("one at a time (the modelled minimum)", Some(MAX_RESULTS_MIN)),
        ("five at a time", Some(5)),
        (
            "exactly the number stored",
            Some(expected_names.len() as i64),
        ),
        ("the modelled maximum", Some(MAX_RESULTS_MAX)),
    ];

    for (case, max_results) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (status, page) = list_identities(&router, max_results, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListWorkloadIdentities, got {status} \
                 ({page})"
            );
            let page_names = listed_names(&page);
            let ceiling = max_results.unwrap_or(MAX_RESULTS_DEFAULT as i64);
            assert!(
                page_names.len() as i64 <= ceiling,
                "case {case:?}: a page holds at most {ceiling} entries, got {} ({page})",
                page_names.len()
            );
            walked.extend(page_names.iter().cloned());
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !page_names.is_empty(),
                        "case {case:?}: a page that hands back a token is not empty ({page})"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, expected_names,
            "case {case:?}: the whole walk reports every stored identity exactly once, in \
             order; got {walked:?} for {expected_names:?}"
        );
    }
}

/// What the workload-identity operations refuse, and with which modelled error: a name
/// outside the modelled bounds, a duplicate name, a read of an identity this account does
/// not hold, a `maxResults` past each published bound and a `nextToken` this service never
/// issued.
#[tokio::test]
async fn the_workload_identity_operations_refuse_what_the_model_states_they_must() {
    let router = router();
    let held = "held-identity";
    let (status, created) = create_identity(&router, &json!({"name": held})).await;
    assert_eq!(
        status, 201,
        "case \"the identity the refusals are measured against\": expected HTTP 201, got \
         {status} ({created})"
    );

    // (case, name, expected status, expected error code)
    let name_rows: [(&str, String, u16, &str); 5] = [
        (
            "a name at the modelled minimum length",
            "abc".to_string(),
            201,
            "",
        ),
        (
            "a name one character below the minimum",
            "a".repeat(NAME_MIN_LENGTH - 1),
            400,
            "ValidationException",
        ),
        (
            "a name one character above the maximum",
            "a".repeat(NAME_MAX_LENGTH + 1),
            400,
            "ValidationException",
        ),
        (
            "a name carrying a character the pattern forbids",
            "not a name".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a name already held by this account",
            held.to_string(),
            400,
            "ValidationException",
        ),
    ];
    for (case, name, expected_status, expected_code) in &name_rows {
        let (status, answer) = create_identity(&router, &json!({"name": name})).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answer})"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&answer),
                *expected_code,
                "case {case:?}: expected the modelled {expected_code}, got {} ({answer})",
                error_code(&answer)
            );
        }
    }

    // The refused creates stored nothing: only the two identities that were accepted are
    // held, and the duplicate did not overwrite the first.
    let (_, page) = list_identities(&router, Some(MAX_RESULTS_MAX), None).await;
    assert_eq!(
        listed_names(&page),
        vec!["abc".to_string(), held.to_string()],
        "case \"after the refused creates\": expected only the accepted identities, got {:?}",
        listed_names(&page)
    );

    // A read of a name this account does not hold is the modelled 404, not an empty 200.
    let (status, missing) = get_identity(&router, "no-such-identity").await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a read of an identity this account does not hold\": expected HTTP 404 \
         ResourceNotFoundException, got {status} {} ({missing})",
        error_code(&missing)
    );

    // (case, maxResults, expected status, expected error code)
    let page_rows: [(&str, i64, u16, &str); 4] = [
        ("maxResults at the modelled minimum", MAX_RESULTS_MIN, 200, ""),
        ("maxResults at the modelled maximum", MAX_RESULTS_MAX, 200, ""),
        (
            "maxResults one below the minimum",
            MAX_RESULTS_MIN - 1,
            400,
            "ValidationException",
        ),
        (
            "maxResults one above the maximum",
            MAX_RESULTS_MAX + 1,
            400,
            "ValidationException",
        ),
    ];
    for (case, max_results, expected_status, expected_code) in page_rows {
        let (status, answer) = list_identities(&router, Some(max_results), None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answer})"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&answer),
                expected_code,
                "case {case:?}: expected the modelled {expected_code}, got {} ({answer})",
                error_code(&answer)
            );
        }
    }

    // A token this service never issued is refused rather than silently answered with the
    // first page.
    let bogus: [(&str, &str); 3] = [
        (
            "a token that is not the form this service issues",
            "not-a-token",
        ),
        ("a token with an odd number of hex digits", "abc"),
        ("a token whose bytes are not text", "ff"),
    ];
    for (case, token) in bogus {
        let (status, answer) = list_identities(&router, None, Some(token)).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({token}): expected HTTP 400 ValidationException, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }

    // A token this service DID issue still resumes the walk after the refusals above.
    let (_, first) = list_identities(&router, Some(1), None).await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page of one over two identities hands back a token")
        .to_string();
    let (status, second) = list_identities(&router, Some(1), Some(&token)).await;
    assert_eq!(
        (status, listed_names(&second)),
        (200, vec![held.to_string()]),
        "case \"the page after the one the token was issued for\": expected the second \
         identity, got {status} ({second})"
    );
}
