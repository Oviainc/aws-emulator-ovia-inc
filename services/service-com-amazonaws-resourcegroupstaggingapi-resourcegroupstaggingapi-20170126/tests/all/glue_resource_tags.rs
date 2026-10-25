// SPDX-License-Identifier: CC0-1.0

//! AWS Glue as an OWNING service of this API: a Glue resource's tags live in Glue's own
//! record, so the tags a Glue create recorded are reported by `GetResources` for that
//! resource's ARN with no tagging call in between, and a tag this API writes is the same
//! tag Glue's `GetTags` reads back.
//!
//! Every test drives the real wire path — the multi-service router, `POST /` with an
//! `X-Amz-Target` and an awsJson body — over ONE engine carrying both services, which is
//! how the emulator runs.
//!
//! The tables span the class the rule generalises over: which KIND of Glue resource holds
//! the tags (and with it the ARN form), how many tags the create carried, the casing of
//! the name the catalog folds, and resources carrying no tag at all. The database the
//! issue cited is one row.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_glue_awsglue as owner_glue;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the one both endpoints
/// serve.
const REGION: &str = "us-east-1";

/// This emulator's account id, which is also Glue's default `CatalogId`.
const ACCOUNT_ID: &str = "000000000000";

/// The SigV4 signing names the router routes on.
const TAGGING: &str = "tagging";
const GLUE: &str = "glue";

/// A router carrying the tagging API and Glue over ONE engine — the arrangement
/// `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header below
    // is present because it is what routes a request to a signing name, not to be
    // verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_glue::EmulatedService::new(engine));
    router
}

/// Call one operation of one service and return its HTTP status and parsed body.
///
/// Every call is signed for [`REGION`], which is the Region both endpoints serve.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let region = REGION;
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{target} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// One tagging-API operation.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call(
        router,
        TAGGING,
        &format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        body,
    )
    .await
}

/// One Glue operation.
async fn glue(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call(router, GLUE, &format!("AWSGlue.{operation}"), body).await
}

/// One Glue operation that must succeed.
async fn glue_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = glue(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The error code an awsJson error body carries (`__type` minus its namespace).
fn error_code(body: &Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body with no __type: {body}"));
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The ARNs of a `GetResources` page, in the order the service returned them.
fn page_arns(body: &Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// `GetResources` restricted to one ARN.
async fn resources_for(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = tagging(
        router,
        "GetResources",
        json!({ "ResourceARNList": [arn] }),
    )
    .await;
    assert_eq!(status, 200, "case {arn}: GetResources body {body}");
    body
}

/// The tag map Glue's own `GetTags` reports for one ARN.
async fn glue_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = glue_ok(router, "GetTags", json!({ "ResourceArn": arn })).await;
    body["Tags"]
        .as_object()
        .unwrap_or_else(|| panic!("a GetTags body with no Tags map: {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().expect("a tag value").to_string(),
            )
        })
        .collect()
}

/// A tag map as the expected `BTreeMap`.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A Glue ARN of the form this emulator mints, in the serving Region and account.
fn glue_arn(resource: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// One taggable Glue resource: the create that makes it, the ARN it answers to and the
/// tags that create recorded.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The ARN's resource portion, which carries the resource type the tagging API names
    /// this kind by.
    resource: String,
    tags: &'static [(&'static str, &'static str)],
}

/// The taggable kinds, one row each: the four Glue resources whose create carries a
/// `Tags` map. The first row is the database the issue cited, tags and all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a database with two tags",
            operation: "CreateDatabase",
            request: json!({
                "DatabaseInput": { "Name": "tagged-db" },
                "Tags": { "Environment": "dev", "Project": "project1" },
            }),
            resource: "database/tagged-db".to_string(),
            tags: &[("Environment", "dev"), ("Project", "project1")],
        },
        Row {
            // The catalog folds a database name to lowercase, so the ARN the tagging API
            // reports is the one built from the STORED name.
            case: "a database created under a mixed-case name",
            operation: "CreateDatabase",
            request: json!({
                "DatabaseInput": { "Name": "Analytics_EU" },
                "Tags": { "owner": "analytics" },
            }),
            resource: "database/analytics_eu".to_string(),
            tags: &[("owner", "analytics")],
        },
        Row {
            case: "a crawler with two tags",
            operation: "CreateCrawler",
            request: json!({
                "Name": "tagged-crawler",
                "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
                "Targets": { "S3Targets": [{ "Path": "s3://lake/raw/" }] },
                "Tags": { "Environment": "dev", "team": "data" },
            }),
            resource: "crawler/tagged-crawler".to_string(),
            tags: &[("Environment", "dev"), ("team", "data")],
        },
        Row {
            case: "a job definition with one tag",
            operation: "CreateJob",
            request: json!({
                "Name": "tagged-job",
                "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://scripts/etl.py" },
                "Tags": { "Project": "project1" },
            }),
            resource: "job/tagged-job".to_string(),
            tags: &[("Project", "project1")],
        },
        Row {
            case: "a connection whose name keeps its casing",
            operation: "CreateConnection",
            request: json!({
                "ConnectionInput": {
                    "Name": "Warehouse",
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": {
                        "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                        "USERNAME": "etl",
                    },
                },
                "Tags": { "Environment": "prod" },
            }),
            resource: "connection/Warehouse".to_string(),
            tags: &[("Environment", "prod")],
        },
    ]
}

/// The tags a Glue create recorded are the resource's own, so `GetResources` queried by
/// that resource's ARN reports exactly one mapping carrying every pair the create
/// carried — with no tagging call in between — and Glue's own `GetTags` reports the same
/// pairs, because there is ONE tag set and both operations read it.
#[tokio::test]
async fn create_time_glue_tags_are_reported_by_get_resources() {
    let router = emulator();
    for row in rows() {
        glue_ok(&router, row.operation, row.request.clone()).await;
        let arn = glue_arn(&row.resource);
        let want = expected(row.tags);

        let page = resources_for(&router, &arn).await;
        let arns = page_arns(&page);
        assert_eq!(
            arns,
            vec![arn.clone()],
            "case {}: expected exactly one mapping for {arn}, got {arns:?}",
            row.case
        );
        let got = page_tags(&page, &arn);
        assert_eq!(
            got, want,
            "case {}: expected GetResources to report {want:?} for {arn}, got {got:?}",
            row.case
        );

        let echoed = glue_tags(&router, &arn).await;
        assert_eq!(
            echoed, want,
            "case {}: expected Glue GetTags to report {want:?} for {arn}, got {echoed:?}",
            row.case
        );
    }
}

/// `ResourceTypeFilters` select Glue resources by the `service[:resourceType]` key their
/// ARN carries: the bare service token selects every tagged Glue resource and a qualified
/// token only that kind.
///
/// The same pass pins the two boundaries of the projection: a Glue resource carrying NO
/// tag is not reported at all ("GetResources does not return untagged resources"), and an
/// ARN naming a Glue resource that was never created is not an error, just absent.
#[tokio::test]
async fn resource_type_filters_select_glue_resources_by_kind() {
    let router = emulator();
    for row in rows() {
        glue_ok(&router, row.operation, row.request.clone()).await;
    }
    // An untagged database: created the same way, with no `Tags`.
    glue_ok(
        &router,
        "CreateDatabase",
        json!({ "DatabaseInput": { "Name": "untagged-db" } }),
    )
    .await;
    let untagged = glue_arn("database/untagged-db");

    let mut tagged: Vec<String> = rows()
        .iter()
        .map(|row| glue_arn(&row.resource))
        .collect();
    tagged.sort();

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        (
            "the bare service token",
            vec!["glue"],
            tagged.clone(),
        ),
        (
            "one qualified kind",
            vec!["glue:database"],
            vec![
                glue_arn("database/analytics_eu"),
                glue_arn("database/tagged-db"),
            ],
        ),
        (
            "two qualified kinds",
            vec!["glue:crawler", "glue:job"],
            vec![glue_arn("crawler/tagged-crawler"), glue_arn("job/tagged-job")],
        ),
        (
            "a kind nothing tagged",
            vec!["glue:table"],
            Vec::new(),
        ),
    ];
    for (case, filters, want) in cases {
        let (status, body) = tagging(
            &router,
            "GetResources",
            json!({ "ResourceTypeFilters": filters }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetResources body {body}");
        let mut got = page_arns(&body);
        got.sort();
        assert_eq!(
            got, want,
            "case {case}: expected ResourceTypeFilters={filters:?} to select {want:?}, got {got:?}"
        );
        assert!(
            !got.contains(&untagged),
            "case {case}: an untagged Glue resource must not be reported, got {got:?}"
        );
    }

    for (case, arn) in [
        ("an untagged resource", untagged.as_str()),
        ("a resource that was never created", &glue_arn("database/ghost")),
    ] {
        let page = resources_for(&router, arn).await;
        let got = page_arns(&page);
        assert!(
            got.is_empty(),
            "case {case}: expected {arn} to be absent from the answer, got {got:?}"
        );
    }
}

/// Glue holds the tag set of every resource this API can tag, so each one has ONE tag
/// home: that service's own record. A tag written through this API is the resource's own
/// tag, which `GetTags` reports; the keys the create recorded and the keys this API did
/// not name are left exactly as they were; and an untag removes the key from that same
/// record, which `GetResources` then reports without it.
///
/// The rows differ along the resource kind, so the rule is shown to be per-resource and
/// not per-service.
#[tokio::test]
async fn glue_tags_live_in_glues_own_records() {
    let router = emulator();
    for row in rows() {
        glue_ok(&router, row.operation, row.request.clone()).await;
        let arn = glue_arn(&row.resource);

        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [&arn], "Tags": { "CostCentre": "cc-42" } }),
        )
        .await;
        assert_eq!(status, 200, "case {}: TagResources body {body}", row.case);
        assert_eq!(
            body["FailedResourcesMap"],
            json!({}),
            "case {}: expected no failed resource, got {}",
            row.case,
            body["FailedResourcesMap"]
        );

        let mut want = expected(row.tags);
        want.insert("CostCentre".to_string(), "cc-42".to_string());
        let got = glue_tags(&router, &arn).await;
        assert_eq!(
            got, want,
            "case {}: expected Glue GetTags to report {want:?} for {arn}, got {got:?}",
            row.case
        );

        let (status, body) = tagging(
            &router,
            "UntagResources",
            json!({ "ResourceARNList": [&arn], "TagKeys": ["CostCentre"] }),
        )
        .await;
        assert_eq!(status, 200, "case {}: UntagResources body {body}", row.case);
        let want = expected(row.tags);
        let got = glue_tags(&router, &arn).await;
        assert_eq!(
            got, want,
            "case {}: expected the untag to leave {want:?} on {arn}, got {got:?}",
            row.case
        );
        let page = resources_for(&router, &arn).await;
        let got = page_tags(&page, &arn);
        assert_eq!(
            got, want,
            "case {}: expected GetResources to report {want:?} for {arn}, got {got:?}",
            row.case
        );
    }
}

/// A Glue ARN this API cannot resolve is answered per RESOURCE, with Glue's own refusal:
/// a well-formed ARN of a taggable kind that names nothing gets that service's
/// `EntityNotFoundException`, and an ARN naming no taggable Glue resource at all gets its
/// `InvalidInputException` — both with `StatusCode` 400, and the other ARNs of the same
/// batch are tagged.
///
/// The rows differ along the kind named and along WHY the ARN resolves to nothing: a
/// missing resource of each kind, a Glue resource type that carries no tag set, and a
/// Glue ARN with no resource type at all.
#[tokio::test]
async fn an_unresolvable_glue_arn_fails_with_glues_own_refusal() {
    let router = emulator();
    glue_ok(
        &router,
        "CreateDatabase",
        json!({ "DatabaseInput": { "Name": "present-db" }, "Tags": { "Environment": "dev" } }),
    )
    .await;
    let present = glue_arn("database/present-db");

    let cases: &[(&str, String, &str)] = &[
        (
            "a database that was never created",
            glue_arn("database/ghost"),
            "EntityNotFoundException",
        ),
        (
            "a crawler that was never created",
            glue_arn("crawler/ghost"),
            "EntityNotFoundException",
        ),
        (
            "a job that was never created",
            glue_arn("job/ghost"),
            "EntityNotFoundException",
        ),
        (
            "a connection that was never created",
            glue_arn("connection/ghost"),
            "EntityNotFoundException",
        ),
        (
            "a database of another account's catalog",
            format!("arn:aws:glue:{REGION}:111122223333:database/present-db"),
            "EntityNotFoundException",
        ),
        (
            // A Glue table carries no `Tags` member on its create, so no tag set.
            "a Glue resource type that holds no tags",
            glue_arn("table/present-db/orders"),
            "InvalidInputException",
        ),
        (
            "a Glue ARN with no resource type",
            glue_arn("catalog"),
            "InvalidInputException",
        ),
    ];
    for (case, arn, want) in cases {
        let (status, body) = tagging(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [arn, &present], "Tags": { "Project": "project1" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: TagResources body {body}");
        let failure = &body["FailedResourcesMap"][arn.as_str()];
        assert_eq!(
            failure["ErrorCode"],
            json!(want),
            "case {case}: expected {want} for {arn}, got {}",
            body["FailedResourcesMap"]
        );
        assert_eq!(
            failure["StatusCode"],
            json!(400),
            "case {case}: expected StatusCode 400 for {arn}, got {failure}"
        );
        assert_eq!(
            body["FailedResourcesMap"]
                .as_object()
                .map(serde_json::Map::len),
            Some(1),
            "case {case}: the other ARN of the batch must be tagged, got {}",
            body["FailedResourcesMap"]
        );
    }

    // The same refusals, from Glue's own read of the same ARNs.
    for (case, arn, want) in cases {
        let (status, body) = glue(&router, "GetTags", json!({ "ResourceArn": arn })).await;
        assert_eq!(status, 400, "case {case}: GetTags({arn}) body {body}");
        let got = error_code(&body);
        assert_eq!(
            got, *want,
            "case {case}: expected GetTags({arn}) to answer {want}, got {got}"
        );
    }
}

/// A Glue endpoint serves one Region, so a `ResourceArn` naming another Region's estate
/// names a resource this endpoint holds nothing for — not the same-named local one.
#[tokio::test]
async fn glue_get_tags_answers_only_for_the_region_it_serves() {
    let router = emulator();
    glue_ok(
        &router,
        "CreateDatabase",
        json!({ "DatabaseInput": { "Name": "regional-db" }, "Tags": { "Environment": "dev" } }),
    )
    .await;

    // The ARN of the Region this endpoint serves resolves, which is what makes the
    // refusal below about the Region and not about the ARN.
    let served = glue_arn("database/regional-db");
    let got = glue_tags(&router, &served).await;
    assert_eq!(
        got,
        expected(&[("Environment", "dev")]),
        "case the serving Region: expected the stored tags for {served}, got {got:?}"
    );

    let foreign = format!("arn:aws:glue:eu-west-1:{ACCOUNT_ID}:database/regional-db");
    let (status, body) = glue(&router, "GetTags", json!({ "ResourceArn": &foreign })).await;
    assert_eq!(status, 400, "case another Region: GetTags body {body}");
    let got = error_code(&body);
    assert_eq!(
        got, "EntityNotFoundException",
        "case another Region: expected EntityNotFoundException for {foreign}, got {got}"
    );
}
