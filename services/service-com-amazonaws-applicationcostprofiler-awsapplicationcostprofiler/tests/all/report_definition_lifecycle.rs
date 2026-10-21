// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ApplicationCostProfiler report-definition
//! lifecycle and of `ImportApplicationUsage`.
//!
//! Every assertion here goes over the wire: the suite mounts the service in a
//! [`MultiServiceRouter`] and dispatches real `restJson1` requests, so what it
//! pins is the emulator's HTTP behaviour, not a handler's Rust signature.
//!
//! The rules under test are stated once and asserted over a TABLE of inputs; the
//! enum catalogues the rows span are read from the model-generated
//! `*_VALUES` constants rather than copied by hand, so a model change moves the
//! test with it.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::ops::types::{
    FORMAT_VALUES, REPORT_FREQUENCY_VALUES, S3_BUCKET_REGION_VALUES,
};
use service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service, the
/// routing signal `resolve_route_key` reads first. Signature verification is off
/// on the test router, so the signature itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/application-cost-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One HTTP answer, reduced to what these tests assert on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// `x-amzn-errortype` — how restJson1 names the error shape.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    fn string(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn number(&self, name: &str) -> f64 {
        self.member(name).as_f64().unwrap_or(f64::NAN)
    }
}

/// A router carrying exactly this service over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
fn mounted_service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(engine));
    router
}

/// Percent-escape a query-parameter value (everything outside RFC 3986's
/// unreserved set), which a base64 `nextToken` needs for its `+`, `/` and `=`.
fn query_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(String::new, |value| value.to_string());
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-cost-profiler.us-east-1.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

// ---------------------------------------------------------------------------
// The report-definition table
// ---------------------------------------------------------------------------

/// One row of the report-definition table: a whole definition, so a row differs
/// from its neighbours along the frequency, the format, the id form, the
/// description, and the shape and size of the destination location.
#[derive(Debug, Clone)]
struct DefinitionCase {
    name: String,
    report_id: String,
    description: String,
    frequency: &'static str,
    format: &'static str,
    bucket: String,
    prefix: String,
}

impl DefinitionCase {
    fn put_body(&self) -> Value {
        json!({
            "reportId": self.report_id,
            "reportDescription": self.description,
            "reportFrequency": self.frequency,
            "format": self.format,
            "destinationS3Location": { "bucket": self.bucket, "prefix": self.prefix },
        })
    }

    fn update_body(&self) -> Value {
        json!({
            "reportDescription": self.description,
            "reportFrequency": self.frequency,
            "format": self.format,
            "destinationS3Location": { "bucket": self.bucket, "prefix": self.prefix },
        })
    }
}

/// Legal `reportId` forms, spanning the `ReportId` shape's charset
/// (`^[0-9A-Za-z\.\-_]+$`) and both its published length bounds (1..255).
fn report_id_forms() -> Vec<String> {
    vec![
        "demo-report".to_string(),
        "Report.2024_01".to_string(),
        "a".to_string(),
        "9".to_string(),
        "r".repeat(255),
        "MiXeD-case.and_1".to_string(),
    ]
}

/// Legal `S3Location.bucket` forms, spanning the `S3Bucket` shape's published
/// length bounds (3..63) and the label shapes its pattern allows — including a
/// single all-digit label, which only a DOTTED all-numeric name is excluded by.
fn bucket_forms() -> Vec<String> {
    vec![
        "my-acp-bucket".to_string(),
        "abc".to_string(),
        "b".repeat(63),
        "logs.example.bucket".to_string(),
        "123".to_string(),
        "a-0".to_string(),
    ]
}

/// Legal `S3Prefix` forms, spanning the shape's published length bounds (1..512).
fn prefix_forms() -> Vec<String> {
    vec![
        "reports/".to_string(),
        "p".to_string(),
        "x".repeat(512),
        "nested/path/segment/".to_string(),
        "has spaces/in it".to_string(),
        "reports/2024/01/".to_string(),
    ]
}

/// Legal `ReportDescription` forms, spanning the shape's published length bounds
/// (1..1024) and its `\S` pattern (at least one non-whitespace character).
fn description_forms() -> Vec<String> {
    vec![
        "demo".to_string(),
        "d".to_string(),
        "x".repeat(1024),
        "  padded with whitespace  ".to_string(),
        "unicode: dépôt des rapports".to_string(),
        "multi\nline description".to_string(),
    ]
}

/// The table: the full cross product of the model's `ReportFrequency` and
/// `Format` value sets, each pair carrying a different id / description /
/// bucket / prefix form so one row differs from the next along every dimension
/// the rule generalises over.
fn definition_table() -> Vec<DefinitionCase> {
    let ids = report_id_forms();
    let descriptions = description_forms();
    let buckets = bucket_forms();
    let prefixes = prefix_forms();
    let mut rows = Vec::new();
    let mut index = 0usize;
    for &frequency in REPORT_FREQUENCY_VALUES {
        for &format in FORMAT_VALUES {
            rows.push(DefinitionCase {
                name: format!("{frequency}/{format}/#{index}"),
                report_id: ids[index % ids.len()].clone(),
                description: descriptions[index % descriptions.len()].clone(),
                frequency,
                format,
                bucket: buckets[index % buckets.len()].clone(),
                prefix: prefixes[index % prefixes.len()].clone(),
            });
            index += 1;
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// PutReportDefinition -> GetReportDefinition round trip
// ---------------------------------------------------------------------------

/// RULE: a created report definition is readable afterwards with exactly the
/// values that were stored, plus the two timestamps the service owns.
#[tokio::test]
async fn a_put_definition_reads_back_member_for_member() {
    for case in definition_table() {
        let router = mounted_service();

        let put = call(&router, "POST", "/reportDefinition", Some(case.put_body())).await;
        assert_eq!(
            put.status, 200,
            "case {}: PutReportDefinition must succeed, got {} {:?}",
            case.name, put.status, put.body
        );
        assert_eq!(
            put.string("reportId"),
            case.report_id,
            "case {}: PutReportDefinition must return the stored reportId, got {:?}",
            case.name,
            put.body
        );

        let got = call(
            &router,
            "GET",
            &format!("/reportDefinition/{}", case.report_id),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {}: GetReportDefinition must succeed, got {} {:?}",
            case.name, got.status, got.body
        );
        assert_eq!(
            got.string("reportId"),
            case.report_id,
            "case {}: expected reportId {:?}, got {:?}",
            case.name,
            case.report_id,
            got.member("reportId")
        );
        assert_eq!(
            got.string("reportDescription"),
            case.description,
            "case {}: expected the stored reportDescription, got {:?}",
            case.name,
            got.member("reportDescription")
        );
        assert_eq!(
            got.string("reportFrequency"),
            case.frequency,
            "case {}: expected reportFrequency {:?}, got {:?}",
            case.name,
            case.frequency,
            got.member("reportFrequency")
        );
        assert_eq!(
            got.string("format"),
            case.format,
            "case {}: expected format {:?}, got {:?}",
            case.name,
            case.format,
            got.member("format")
        );
        assert_eq!(
            got.member("destinationS3Location"),
            &json!({ "bucket": case.bucket, "prefix": case.prefix }),
            "case {}: expected the stored destinationS3Location, got {:?}",
            case.name,
            got.member("destinationS3Location")
        );
        let created_at = got.number("createdAt");
        let last_updated = got.number("lastUpdated");
        assert!(
            created_at.is_finite() && created_at > 0.0,
            "case {}: createdAt must be a real timestamp, got {:?}",
            case.name,
            got.member("createdAt")
        );
        assert!(
            last_updated.is_finite() && last_updated >= created_at,
            "case {}: lastUpdated must be a real timestamp at or after createdAt, got {:?}",
            case.name,
            got.member("lastUpdated")
        );

        // A read may not change what a later caller sees.
        let again = call(
            &router,
            "GET",
            &format!("/reportDefinition/{}", case.report_id),
            None,
        )
        .await;
        assert_eq!(
            again.body, got.body,
            "case {}: a second GetReportDefinition must answer identically",
            case.name
        );
    }
}

/// RULE: a second `PutReportDefinition` for an id that already exists refreshes
/// that one definition — it does not add a second — and keeps the `createdAt` of
/// the first creation.
#[tokio::test]
async fn re_putting_one_id_refreshes_the_same_definition() {
    let router = mounted_service();
    let rows = definition_table();
    let first = &rows[0];
    let mut second = rows[1].clone();
    second.report_id = first.report_id.clone();

    let put = call(&router, "POST", "/reportDefinition", Some(first.put_body())).await;
    assert_eq!(put.status, 200, "the first put must succeed: {:?}", put.body);
    let created = call(
        &router,
        "GET",
        &format!("/reportDefinition/{}", first.report_id),
        None,
    )
    .await;
    let created_at = created.number("createdAt");

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let re_put = call(
        &router,
        "POST",
        "/reportDefinition",
        Some(second.put_body()),
    )
    .await;
    assert_eq!(
        re_put.status, 200,
        "re-putting an existing reportId must succeed: {:?}",
        re_put.body
    );

    let after = call(
        &router,
        "GET",
        &format!("/reportDefinition/{}", first.report_id),
        None,
    )
    .await;
    assert_eq!(
        after.string("reportFrequency"),
        second.frequency,
        "case re-put: expected the refreshed reportFrequency {:?}, got {:?}",
        second.frequency,
        after.member("reportFrequency")
    );
    assert_eq!(
        after.number("createdAt"),
        created_at,
        "case re-put: createdAt must survive a re-put, got {:?}",
        after.member("createdAt")
    );
    assert!(
        after.number("lastUpdated") > created_at,
        "case re-put: lastUpdated must advance, got {:?} against createdAt {created_at}",
        after.member("lastUpdated")
    );

    let listed = call(&router, "GET", "/reportDefinition", None).await;
    let definitions = listed
        .member("reportDefinitions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        definitions.len(),
        1,
        "case re-put: one id must hold one definition, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// UpdateReportDefinition
// ---------------------------------------------------------------------------

/// RULE: an update rewrites the definition's mutable members in place, moves
/// `lastUpdated` to the call's instant and leaves `createdAt` alone; the next
/// read shows the new values.
#[tokio::test]
async fn an_update_rewrites_the_definition_and_keeps_created_at() {
    let rows = definition_table();
    for (index, update) in rows.iter().enumerate() {
        let case = format!("update-to/{}", update.name);
        let router = mounted_service();
        // Seed from a DIFFERENT row, so every member the update carries changes.
        let mut seed = rows[(index + 3) % rows.len()].clone();
        seed.report_id = update.report_id.clone();

        let put = call(&router, "POST", "/reportDefinition", Some(seed.put_body())).await;
        assert_eq!(put.status, 200, "case {case}: seeding must succeed: {:?}", put.body);
        let before = call(
            &router,
            "GET",
            &format!("/reportDefinition/{}", seed.report_id),
            None,
        )
        .await;
        let created_at = before.number("createdAt");

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let updated = call(
            &router,
            "PUT",
            &format!("/reportDefinition/{}", update.report_id),
            Some(update.update_body()),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: UpdateReportDefinition must succeed, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("reportId"),
            update.report_id,
            "case {case}: expected the updated reportId, got {:?}",
            updated.body
        );

        let after = call(
            &router,
            "GET",
            &format!("/reportDefinition/{}", update.report_id),
            None,
        )
        .await;
        assert_eq!(
            after.string("reportDescription"),
            update.description,
            "case {case}: expected the updated reportDescription, got {:?}",
            after.member("reportDescription")
        );
        assert_eq!(
            after.string("reportFrequency"),
            update.frequency,
            "case {case}: expected reportFrequency {:?}, got {:?}",
            update.frequency,
            after.member("reportFrequency")
        );
        assert_eq!(
            after.string("format"),
            update.format,
            "case {case}: expected format {:?}, got {:?}",
            update.format,
            after.member("format")
        );
        assert_eq!(
            after.member("destinationS3Location"),
            &json!({ "bucket": update.bucket, "prefix": update.prefix }),
            "case {case}: expected the updated destinationS3Location, got {:?}",
            after.member("destinationS3Location")
        );
        assert_eq!(
            after.number("createdAt"),
            created_at,
            "case {case}: createdAt must not move on update, got {:?}",
            after.member("createdAt")
        );
        assert!(
            after.number("lastUpdated") > created_at,
            "case {case}: lastUpdated must advance past createdAt {created_at}, got {:?}",
            after.member("lastUpdated")
        );
    }
}

// ---------------------------------------------------------------------------
// ListReportDefinitions and its @paginated contract
// ---------------------------------------------------------------------------

/// True when `token` satisfies the `Token` shape's pattern
/// `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`.
fn is_modelled_token(token: &str) -> bool {
    if token.is_empty() || !token.len().is_multiple_of(4) {
        return false;
    }
    let body = token.trim_end_matches('=');
    let padding = token.len() - body.len();
    padding <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// RULE: a listing returns every stored definition in one stable order, a page
/// holds at most `maxResults` of them, and a `nextToken` is present exactly
/// while definitions remain.
#[tokio::test]
async fn listing_pages_over_every_stored_definition() {
    // (how many definitions exist, what maxResults each page asks for)
    let cases: &[(usize, Option<i32>)] = &[
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (7, Some(3)),
        (3, None),
        (4, Some(100)),
        (10, Some(4)),
    ];
    let rows = definition_table();

    for &(count, max_results) in cases {
        let case = format!("count={count}, maxResults={max_results:?}");
        let router = mounted_service();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let mut seed = rows[index % rows.len()].clone();
            // Deliberately NOT in insertion order, so the listing's order is the
            // store's own and not the order the rows arrived in.
            seed.report_id = format!("report-{:02}", (count - index) * 7 % 100);
            expected.push(seed.report_id.clone());
            let put = call(&router, "POST", "/reportDefinition", Some(seed.put_body())).await;
            assert_eq!(
                put.status, 200,
                "case {case}: seeding {} must succeed: {:?}",
                seed.report_id, put.body
            );
        }
        expected.sort();
        expected.dedup();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = Vec::new();
            if let Some(limit) = max_results {
                query.push(format!("maxResults={limit}"));
            }
            if let Some(cursor) = token.as_deref() {
                query.push(format!("nextToken={}", query_escape(cursor)));
            }
            let path = if query.is_empty() {
                "/reportDefinition".to_string()
            } else {
                format!("/reportDefinition?{}", query.join("&"))
            };
            let page = call(&router, "GET", &path, None).await;
            assert_eq!(
                page.status, 200,
                "case {case}: ListReportDefinitions must succeed, got {} {:?}",
                page.status, page.body
            );
            let items = page
                .member("reportDefinitions")
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("case {case}: reportDefinitions must be a list, got {:?}", page.body));
            if let Some(limit) = max_results {
                assert!(
                    items.len() <= limit as usize,
                    "case {case}: a page must hold at most {limit} items, got {}",
                    items.len()
                );
            }
            for item in &items {
                let report_id = item
                    .get("reportId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    item.get("destinationS3Location").is_some()
                        && item.get("createdAt").is_some()
                        && item.get("lastUpdatedAt").is_some(),
                    "case {case}: a listed definition must carry every stored member, got {item:?}"
                );
                seen.push(report_id);
            }
            pages += 1;
            token = page.member("nextToken").as_str().map(ToString::to_string);
            match token.as_deref() {
                Some(cursor) => assert!(
                    is_modelled_token(cursor),
                    "case {case}: nextToken {cursor:?} must satisfy the modelled Token pattern"
                ),
                None => break,
            }
            assert!(
                pages <= count + 2,
                "case {case}: paging must terminate; {pages} pages so far"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case}: paging must yield every definition once, in reportId order"
        );
        let expected_pages = match max_results {
            Some(limit) if count > 0 => count.div_ceil(limit as usize),
            _ => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case}: nextToken must be omitted on the last page, so paging takes \
             {expected_pages} pages, took {pages}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteReportDefinition
// ---------------------------------------------------------------------------

/// RULE: a deleted definition is gone — it can no longer be read and no longer
/// appears in a listing — while its neighbours are untouched.
#[tokio::test]
async fn a_deleted_definition_leaves_the_store() {
    let rows = definition_table();
    for (index, victim) in rows.iter().enumerate() {
        let case = format!("delete/{}", victim.name);
        let router = mounted_service();
        let mut bystander = rows[(index + 1) % rows.len()].clone();
        bystander.report_id = format!("kept-{index}");

        for seed in [victim, &bystander] {
            let put = call(&router, "POST", "/reportDefinition", Some(seed.put_body())).await;
            assert_eq!(
                put.status, 200,
                "case {case}: seeding {} must succeed: {:?}",
                seed.report_id, put.body
            );
        }

        let deleted = call(
            &router,
            "DELETE",
            &format!("/reportDefinition/{}", victim.report_id),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: DeleteReportDefinition must succeed, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.string("reportId"),
            victim.report_id,
            "case {case}: DeleteReportDefinition must return the deleted reportId, got {:?}",
            deleted.body
        );

        let gone = call(
            &router,
            "GET",
            &format!("/reportDefinition/{}", victim.report_id),
            None,
        )
        .await;
        assert_eq!(
            gone.status, 400,
            "case {case}: reading a deleted definition must be refused, got {} {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected a ValidationException, got {:?}",
            gone.error_type
        );

        let listed = call(&router, "GET", "/reportDefinition", None).await;
        let ids: Vec<String> = listed
            .member("reportDefinitions")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item.get("reportId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            vec![bystander.report_id.clone()],
            "case {case}: only the deleted definition may disappear, got {ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ImportApplicationUsage
// ---------------------------------------------------------------------------

/// RULE: every accepted usage import gets its own `importId`, inside the
/// `ImportId` shape's charset and length, and no two calls share one.
#[tokio::test]
async fn every_usage_import_mints_its_own_id() {
    let router = mounted_service();
    let buckets = bucket_forms();
    let keys = prefix_forms();
    // `None` plus each value of the model's `S3BucketRegion` enum.
    let mut regions: Vec<Option<&str>> = vec![None];
    regions.extend(S3_BUCKET_REGION_VALUES.iter().map(|region| Some(*region)));

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut calls = 0usize;
    for (index, bucket) in buckets.iter().enumerate() {
        for key in &keys {
            let region = regions[index % regions.len()];
            let case = format!("{bucket}/{key}/{region:?}");
            let mut location = json!({ "bucket": bucket, "key": key });
            if let Some(region) = region {
                location["region"] = json!(region);
            }
            let reply = call(
                &router,
                "POST",
                "/importApplicationUsage",
                Some(json!({ "sourceS3Location": location })),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: ImportApplicationUsage must succeed, got {} {:?}",
                reply.status, reply.body
            );
            let import_id = reply.string("importId");
            assert!(
                (1..=255).contains(&import_id.len())
                    && import_id.bytes().all(|b| b.is_ascii_alphanumeric()
                        || b == b'.'
                        || b == b'-'
                        || b == b'_'),
                "case {case}: importId {import_id:?} must satisfy the ImportId shape"
            );
            assert!(
                minted.insert(import_id.clone()),
                "case {case}: importId {import_id:?} was already minted"
            );
            calls += 1;
        }
    }
    assert_eq!(
        minted.len(),
        calls,
        "every one of {calls} imports must have its own importId, got {} distinct",
        minted.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request outside the model's own constraints is refused with the
/// service's only client-input error — HTTP 400 `ValidationException` — never
/// served.
#[tokio::test]
async fn a_request_outside_the_model_is_refused_as_a_validation_exception() {
    let legal = &definition_table()[0];
    let body_with = |patch: Value| {
        let mut body = legal.put_body();
        for (key, value) in patch.as_object().expect("a JSON object").clone() {
            body[key] = value;
        }
        body
    };

    // (case name, method, path, body)
    let refused: Vec<(String, &str, String, Option<Value>)> = vec![
        (
            "reportFrequency outside the enum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportFrequency": "HOURLY" }))),
        ),
        (
            "format outside the enum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "format": "JSON" }))),
        ),
        (
            "reportDescription empty".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportDescription": "" }))),
        ),
        (
            "reportDescription all whitespace".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportDescription": "   \t  " }))),
        ),
        (
            "reportDescription one past its maximum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportDescription": "x".repeat(1025) }))),
        ),
        (
            "reportId one past its maximum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportId": "r".repeat(256) }))),
        ),
        (
            "reportId outside its charset".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({ "reportId": "has spaces" }))),
        ),
        (
            "bucket one below its minimum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(
                json!({ "destinationS3Location": { "bucket": "ab", "prefix": "reports/" } }),
            )),
        ),
        (
            "bucket one past its maximum".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(json!({
                "destinationS3Location": { "bucket": "b".repeat(64), "prefix": "reports/" }
            }))),
        ),
        (
            "bucket uppercase".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(
                json!({ "destinationS3Location": { "bucket": "MyBucket", "prefix": "r/" } }),
            )),
        ),
        (
            "bucket IPv4-shaped".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(
                json!({ "destinationS3Location": { "bucket": "192.168.0.1", "prefix": "r/" } }),
            )),
        ),
        (
            "prefix empty".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(body_with(
                json!({ "destinationS3Location": { "bucket": "my-acp-bucket", "prefix": "" } }),
            )),
        ),
        (
            "required member absent".to_string(),
            "POST",
            "/reportDefinition".to_string(),
            Some(json!({ "reportId": "demo-report" })),
        ),
        (
            "maxResults one below its minimum".to_string(),
            "GET",
            "/reportDefinition?maxResults=0".to_string(),
            None,
        ),
        (
            "maxResults one past its maximum".to_string(),
            "GET",
            "/reportDefinition?maxResults=101".to_string(),
            None,
        ),
        (
            "nextToken unparsable".to_string(),
            "GET",
            format!("/reportDefinition?nextToken={}", query_escape("!!not base64!!")),
            None,
        ),
        (
            "nextToken base64 of an illegal reportId".to_string(),
            "GET",
            format!("/reportDefinition?nextToken={}", query_escape("aGFzIHNwYWNlcw==")),
            None,
        ),
        (
            "get an unknown reportId".to_string(),
            "GET",
            "/reportDefinition/never-created".to_string(),
            None,
        ),
        (
            "delete an unknown reportId".to_string(),
            "DELETE",
            "/reportDefinition/never-created".to_string(),
            None,
        ),
        (
            "update an unknown reportId".to_string(),
            "PUT",
            "/reportDefinition/never-created".to_string(),
            Some(legal.update_body()),
        ),
        (
            "import bucket below its minimum".to_string(),
            "POST",
            "/importApplicationUsage".to_string(),
            Some(json!({ "sourceS3Location": { "bucket": "ab", "key": "usage.csv" } })),
        ),
        (
            "import key empty".to_string(),
            "POST",
            "/importApplicationUsage".to_string(),
            Some(json!({ "sourceS3Location": { "bucket": "my-usage-bucket", "key": "" } })),
        ),
        (
            "import region outside the enum".to_string(),
            "POST",
            "/importApplicationUsage".to_string(),
            Some(json!({
                "sourceS3Location": {
                    "bucket": "my-usage-bucket", "key": "usage.csv", "region": "us-east-1"
                }
            })),
        ),
    ];

    for (case, method, path, body) in refused {
        let router = mounted_service();
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected a ValidationException, got {:?} / {:?}",
            reply.error_type,
            reply.body
        );
        let listed = call(&router, "GET", "/reportDefinition", None).await;
        assert_eq!(
            listed.member("reportDefinitions"),
            &json!([]),
            "case {case}: a refused request must store nothing, got {:?}",
            listed.body
        );
    }
}

/// RULE: the number of report definitions one account holds is bounded; a
/// creation past the bound is answered with the operation's own modelled quota
/// error, and deleting one makes room again.
#[tokio::test]
async fn the_report_definition_quota_is_enforced() {
    use service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::state::MAX_REPORT_DEFINITIONS;

    let router = mounted_service();
    let rows = definition_table();
    let id_of = |index: usize| format!("quota-{index:04}");

    for index in 0..MAX_REPORT_DEFINITIONS {
        let mut seed = rows[index % rows.len()].clone();
        seed.report_id = id_of(index);
        let put = call(&router, "POST", "/reportDefinition", Some(seed.put_body())).await;
        assert_eq!(
            put.status, 200,
            "case fill/{index}: a definition within the quota must be created, got {} {:?}",
            put.status, put.body
        );
    }

    // Re-putting an id that already exists does not grow the store, so it stays
    // inside the quota.
    let mut existing = rows[0].clone();
    existing.report_id = id_of(0);
    let re_put = call(
        &router,
        "POST",
        "/reportDefinition",
        Some(existing.put_body()),
    )
    .await;
    assert_eq!(
        re_put.status, 200,
        "case re-put at the quota: refreshing an existing id must stay allowed, got {} {:?}",
        re_put.status, re_put.body
    );

    let mut overflow = rows[1].clone();
    overflow.report_id = id_of(MAX_REPORT_DEFINITIONS);
    let refused = call(
        &router,
        "POST",
        "/reportDefinition",
        Some(overflow.put_body()),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case overflow: expected the modelled ServiceQuotaExceededException status, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case overflow: expected a ServiceQuotaExceededException, got {:?}",
        refused.error_type
    );

    let deleted = call(
        &router,
        "DELETE",
        &format!("/reportDefinition/{}", id_of(0)),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case make-room: DeleteReportDefinition must succeed, got {} {:?}",
        deleted.status, deleted.body
    );
    let accepted = call(
        &router,
        "POST",
        "/reportDefinition",
        Some(overflow.put_body()),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case make-room: a deletion must free a slot, got {} {:?}",
        accepted.status, accepted.body
    );
}

/// RULE: the published bounds themselves are legal — only a value PAST them is
/// refused.
#[tokio::test]
async fn the_published_bounds_are_accepted() {
    for max_results in [1, 2, 99, 100] {
        let router = mounted_service();
        let seed = &definition_table()[0];
        let put = call(&router, "POST", "/reportDefinition", Some(seed.put_body())).await;
        assert_eq!(put.status, 200, "seeding must succeed: {:?}", put.body);
        let listed = call(
            &router,
            "GET",
            &format!("/reportDefinition?maxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case maxResults={max_results}: must be accepted, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed
                .member("reportDefinitions")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case maxResults={max_results}: expected the one stored definition, got {:?}",
            listed.body
        );
    }
}
