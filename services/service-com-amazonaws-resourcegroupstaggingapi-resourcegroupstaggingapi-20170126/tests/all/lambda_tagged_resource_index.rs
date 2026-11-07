// SPDX-License-Identifier: CC0-1.0

//! AWS Lambda as an OWNING service of this API: the tag set a Lambda resource
//! holds is the ONE tag set this API reads and writes, so a function created with
//! `Tags` is discoverable through `GetResources` with no tagging call in between,
//! a tag `TagResources` writes is the tag Lambda's own `ListTags` reads back, and
//! an ARN Lambda holds no taggable resource for is a per-resource failure
//! carrying Lambda's own refusal.
//!
//! Every test drives the real wire path — the multi-service router, this API's
//! `POST /` with an `X-Amz-Target` and an awsJson body, and Lambda's own
//! `restJson1` routes — over ONE engine carrying both services, which is how the
//! emulator runs.
//!
//! The tables span what each rule generalises over: which taggable Lambda
//! resource type holds the tags (and with it the ARN form and the resource-type
//! key), how many tags a create carried, a resource carrying none at all, the key
//! and value a filter selects by, how a match set is paged, which Region a
//! resource lives in, and each form of ARN that names no tag set. The function
//! carrying one colon-bearing key is one row of each.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_lambda_awsgirapiservice as owner_lambda;
use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most requests below are signed for, and therefore the one both
/// endpoints serve for them.
const REGION: &str = "us-east-1";

/// A SECOND Region, for the rule that this API's answers are Region-scoped.
const OTHER_REGION: &str = "us-west-2";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The SigV4 signing names the router routes on.
const TAGGING: &str = "tagging";
const LAMBDA: &str = "lambda";

/// An execution role ARN inside the modelled `Role` pattern. Lambda SERVES a
/// create naming a role no IAM record is held for, so no role is created here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/LambdaExecutionRole";

/// An empty .zip archive, base64-encoded: the smallest deployment package
/// `CreateFunction`'s `Code.ZipFile` accepts.
const ZIP_FILE: &str = "UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==";

/// The tag key the recorded Lambda console writes, and the one key every filter
/// table below selects by. The colon in it is DATA: it is one key, not a
/// namespace separator.
const BLUEPRINT_KEY: &str = "lambda-console:blueprint";

/// A router carrying this API and AWS Lambda over ONE engine — the arrangement
/// `core/router` builds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization` header
    // below is present because it is what routes a request to a signing name, not
    // to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(owner_lambda::EmulatedService::new(engine));
    router
}

/// The `Authorization` header that routes a request to `signing_name` in `region`.
fn authorization(signing_name: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// Call one awsJson operation of this API in `region`, and return its HTTP status
/// and parsed body.
async fn tagging_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    tagging_raw_in(router, region, operation, body.to_string()).await
}

/// [`tagging_in`] over a body written by hand, for a request whose member ORDER
/// matters (a `serde_json` object key-sorts its members).
async fn tagging_raw(
    router: &MultiServiceRouter,
    operation: &str,
    body: String,
) -> (u16, Value) {
    tagging_raw_in(router, REGION, operation, body).await
}

/// [`tagging_raw`] in a stated Region.
async fn tagging_raw_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: String,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header("authorization", authorization(TAGGING, region))
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, parsed_body(operation, response.body()))
}

/// [`tagging_in`] for the Region most tests use.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    tagging_in(router, REGION, operation, body).await
}

/// One operation of this API that must answer 200.
async fn tagging_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    tagging_ok_in(router, REGION, operation, body).await
}

/// [`tagging_ok`] in a stated Region.
async fn tagging_ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = tagging_in(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// Call one of Lambda's own `restJson1` routes in `region`. A `Null` body is sent
/// as no body at all, which is what a `GET` or a `DELETE` carries.
async fn lambda_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("lambda.{region}.amazonaws.com");
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization(LAMBDA, region))
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, parsed_body(path, response.body()))
}

/// One Lambda route that must answer `expected` status.
async fn lambda_ok(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
    expected: u16,
) -> Value {
    let (status, parsed) = lambda_in(router, region, method, path, body).await;
    assert_eq!(
        status, expected,
        "{method} {path} must answer {expected}, got {status} with body {parsed}"
    );
    parsed
}

/// A response body as JSON, or `Value::Null` when it is empty.
fn parsed_body(what: &str, body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(body).unwrap_or_else(|error| {
        panic!(
            "{what} answered a body that is not JSON ({error}): {}",
            String::from_utf8_lossy(body)
        )
    })
}

/// The unqualified ARN a function of `name` carries in `region`.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT_ID}:function:{name}")
}

/// `CreateFunction` in `region`, with the tags of `tags`, and the ARN it answers.
async fn create_function(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut request = json!({
        "FunctionName": name,
        "Role": ROLE_ARN,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": ZIP_FILE },
    });
    if !tags.is_empty() {
        request["Tags"] = json!(tag_object(tags));
    }
    let created = lambda_ok(
        router,
        region,
        "POST",
        "/2015-03-31/functions",
        &request,
        201,
    )
    .await;
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction answered no FunctionArn: {created}"))
        .to_string();
    assert_eq!(
        arn,
        function_arn(region, name),
        "CreateFunction must mint the unqualified ARN of the Region it was called in"
    );
    arn
}

/// The tag map Lambda's OWN `ListTags` reports for one ARN.
async fn lambda_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = lambda_ok(
        router,
        REGION,
        "GET",
        &format!("/2017-03-31/tags/{arn}"),
        &Value::Null,
        200,
    )
    .await;
    body["Tags"]
        .as_object()
        .unwrap_or_else(|| panic!("a ListTags body with no Tags: {body}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A `(key, value)` table as the JSON object a `Tags` map is sent as.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// A `(key, value)` table as the expected `BTreeMap`.
fn expected(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
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

/// The ARNs `GetResources` selects in `region` for one request, sorted.
async fn selected_arns_in(router: &MultiServiceRouter, region: &str, request: Value) -> Vec<String> {
    let body = tagging_ok_in(router, region, "GetResources", request).await;
    let mut arns = page_arns(&body);
    arns.sort();
    arns
}

/// [`selected_arns_in`] in the Region most tests use.
async fn selected_arns(router: &MultiServiceRouter, request: Value) -> Vec<String> {
    selected_arns_in(router, REGION, request).await
}

/// Every page of a `GetResources` walk, following `PaginationToken` until the
/// service stops issuing one: the ARNs of each page, in page order.
///
/// Bounded by a page count no correct walk can reach, so a service that never
/// stops issuing a token fails the assertion instead of looping forever.
async fn walked_pages(router: &MultiServiceRouter, mut request: Value) -> Vec<Vec<String>> {
    const MAX_PAGES: usize = 16;
    let mut pages: Vec<Vec<String>> = Vec::new();
    loop {
        let body = tagging_ok(router, "GetResources", request.clone()).await;
        pages.push(page_arns(&body));
        let token = body["PaginationToken"].as_str().unwrap_or_default();
        if token.is_empty() {
            return pages;
        }
        assert!(
            pages.len() < MAX_PAGES,
            "a walk of at most {MAX_PAGES} pages must end, got {pages:?}"
        );
        request["PaginationToken"] = json!(token);
    }
}

/// Every tag key `GetTagKeys` reports, over every page of the walk.
async fn all_tag_keys(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let mut request = json!({});
    loop {
        let body = tagging_ok(router, "GetTagKeys", request.clone()).await;
        for key in body["TagKeys"].as_array().into_iter().flatten() {
            keys.insert(key.as_str().expect("a TagKeys entry").to_string());
        }
        let token = body["PaginationToken"].as_str().unwrap_or_default();
        if token.is_empty() {
            return keys;
        }
        request["PaginationToken"] = json!(token);
    }
}

/// Every value `GetTagValues` reports under `key`, over every page of the walk.
/// A `Vec`, not a set, so a value reported twice fails the assertion.
async fn all_tag_values(router: &MultiServiceRouter, key: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut request = json!({ "Key": key });
    loop {
        let body = tagging_ok(router, "GetTagValues", request.clone()).await;
        for value in body["TagValues"].as_array().into_iter().flatten() {
            values.push(value.as_str().expect("a TagValues entry").to_string());
        }
        let token = body["PaginationToken"].as_str().unwrap_or_default();
        if token.is_empty() {
            values.sort();
            return values;
        }
        request["PaginationToken"] = json!(token);
    }
}

/// The `FailedResourcesMap` of a `TagResources` answer, as
/// `ARN -> (StatusCode, ErrorCode, ErrorMessage)`.
fn failures(body: &Value) -> BTreeMap<String, (i64, String, String)> {
    body["FailedResourcesMap"]
        .as_object()
        .unwrap_or_else(|| panic!("a TagResources body with no FailedResourcesMap: {body}"))
        .iter()
        .map(|(arn, failure)| {
            (
                arn.clone(),
                (
                    failure["StatusCode"].as_i64().unwrap_or_default(),
                    failure["ErrorCode"].as_str().unwrap_or_default().to_string(),
                    failure["ErrorMessage"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
            )
        })
        .collect()
}

/// One taggable Lambda resource: the create that makes it, the ARN it answers to,
/// the tags that create recorded and the resource-type key this API names its
/// kind by.
struct Row {
    case: &'static str,
    /// The create's route, as Lambda's model binds it.
    method: &'static str,
    path: &'static str,
    request: Value,
    /// The member of the create's answer that carries the minted ARN, as a path
    /// of object keys.
    arn_member: &'static [&'static str],
    /// The `service:resourceType` key a qualified `ResourceTypeFilters` entry
    /// selects this resource by.
    resource_type: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The taggable kinds, spanning the tag counts a create can carry: a FUNCTION,
/// whose tags "CreateFunction" records, and a CODE-SIGNING CONFIGURATION, the
/// other resource this emulator's Lambda holds a `Tags` map for at creation —
/// each created with tags and one created with none at all.
///
/// Source for the kinds and their `Tags` members:
/// `smithy_query(query="operation", operation="CreateFunction,CreateCodeSigningConfig")`
/// on the Lambda model.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function with one colon-bearing key",
            method: "POST",
            path: "/2015-03-31/functions",
            request: json!({
                "FunctionName": "blueprint-one",
                "Role": ROLE_ARN,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
                "Tags": { BLUEPRINT_KEY: "true" },
            }),
            arn_member: &["FunctionArn"],
            resource_type: "lambda:function",
            tags: &[(BLUEPRINT_KEY, "true")],
        },
        Row {
            case: "a function with three tags",
            method: "POST",
            path: "/2015-03-31/functions",
            request: json!({
                "FunctionName": "Billing_Pipeline",
                "Role": ROLE_ARN,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
                "Tags": {
                    "Environment": "prod",
                    "Project": "project1",
                    "owner": "billing",
                },
            }),
            arn_member: &["FunctionArn"],
            resource_type: "lambda:function",
            tags: &[
                ("Environment", "prod"),
                ("Project", "project1"),
                ("owner", "billing"),
            ],
        },
        Row {
            case: "a function created with no tags",
            method: "POST",
            path: "/2015-03-31/functions",
            request: json!({
                "FunctionName": "untagged-function",
                "Role": ROLE_ARN,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
            }),
            arn_member: &["FunctionArn"],
            resource_type: "lambda:function",
            tags: &[],
        },
        Row {
            case: "a code-signing configuration with two tags",
            method: "POST",
            path: "/2020-04-22/code-signing-configs",
            request: json!({
                "AllowedPublishers": {
                    "SigningProfileVersionArns": [
                        "arn:aws:signer:us-east-1:000000000000:/signing-profiles/prod/aBcDeF",
                    ],
                },
                "Tags": { BLUEPRINT_KEY: "true", "owner": "platform" },
            }),
            arn_member: &["CodeSigningConfig", "CodeSigningConfigArn"],
            resource_type: "lambda:code-signing-config",
            tags: &[(BLUEPRINT_KEY, "true"), ("owner", "platform")],
        },
    ]
}

/// One resource a row's create really made: the row it came from, and the ARN the
/// owning service minted for it.
struct Created {
    row: Row,
    arn: String,
}

/// RULE: a resource the OWNING service holds a tag set for is a row of this API's
/// projection as soon as that service records those tags, under the
/// `service:resourceType` key its own ARN carries — and a resource carrying no
/// tag at all is no row ("GetResources does not return untagged resources").
#[tokio::test]
async fn every_tagged_lambda_resource_is_a_row_of_the_projection() {
    let router = emulator();
    let mut created: Vec<Created> = Vec::new();

    for row in rows() {
        // Every create in the table answers 201, as its own route declares
        // (`POST /2015-03-31/functions -> 201`,
        // `POST /2020-04-22/code-signing-configs -> 201`).
        let answer = lambda_ok(&router, REGION, row.method, row.path, &row.request, 201).await;
        let mut carried = &answer;
        for member in row.arn_member {
            carried = &carried[*member];
        }
        let arn = carried
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the create answered no ARN at {:?}: {answer}",
                    row.case, row.arn_member
                )
            })
            .to_string();
        created.push(Created { row, arn });
    }

    for created in &created {
        let (case, tags, resource_type) =
            (created.row.case, created.row.tags, created.row.resource_type);
        let arn = &created.arn;
        let selected = selected_arns(&router, json!({ "ResourceARNList": [arn] })).await;
        if tags.is_empty() {
            assert!(
                selected.is_empty(),
                "case {case:?}: a resource carrying no tag is no row of the projection, \
                 got {selected:?}"
            );
            continue;
        }
        assert_eq!(
            selected,
            vec![arn.clone()],
            "case {case:?}: the tagged resource must be the one row its own ARN selects"
        );

        // The tags the projection reports are the ones the OWNING service's create
        // recorded, pair for pair.
        let body = tagging_ok(&router, "GetResources", json!({ "ResourceARNList": [arn] })).await;
        let reported = page_tags(&body, arn);
        assert_eq!(
            reported,
            expected(tags),
            "case {case:?}: expected the tags the create recorded, got {reported:?}"
        );

        // A BARE service token selects every resource of that service and the
        // QUALIFIED token only that one type.
        for filter in [LAMBDA, resource_type] {
            let selected = selected_arns(
                &router,
                json!({ "ResourceTypeFilters": [filter] }),
            )
            .await;
            assert!(
                selected.contains(arn),
                "case {case:?}: ResourceTypeFilters [{filter:?}] must select {arn}, got \
                 {selected:?}"
            );
        }

        // And every key and value of it is in the account/Region summaries.
        let keys = all_tag_keys(&router).await;
        for (key, value) in tags {
            assert!(
                keys.contains(*key),
                "case {case:?}: GetTagKeys must list {key:?}, got {keys:?}"
            );
            // The whole key is ONE key: a colon in it is data, so no prefix of it
            // is ever a key of its own.
            if let Some((prefix, _)) = key.split_once(':') {
                assert!(
                    !keys.contains(prefix),
                    "case {case:?}: {prefix:?} is no key of its own, got {keys:?}"
                );
            }
            let values = all_tag_values(&router, key).await;
            assert!(
                values.contains(&(*value).to_string()),
                "case {case:?}: GetTagValues({key:?}) must list {value:?}, got {values:?}"
            );
            assert_eq!(
                values.iter().filter(|held| *held == value).count(),
                1,
                "case {case:?}: GetTagValues({key:?}) lists a value once however many \
                 resources carry it, got {values:?}"
            );
        }
    }
}

/// RULE: a `TagFilters` entry selects the resources carrying that EXACT key, and
/// where it states `Values` those carrying one of those exact values — both
/// compared whole and case-sensitively, with several values OR-ed.
#[tokio::test]
async fn tag_filters_select_by_exact_key_and_exact_value() {
    let router = emulator();
    // The key the filters select by, a longer key it is a PREFIX of, a resource
    // whose VALUE spells the key, and two values of the same key differing only in
    // case.
    let hit = create_function(&router, REGION, "filter-hit", &[(BLUEPRINT_KEY, "true")]).await;
    let other_case =
        create_function(&router, REGION, "filter-other-case", &[(BLUEPRINT_KEY, "True")]).await;
    let longer_key = create_function(
        &router,
        REGION,
        "filter-longer-key",
        &[(&format!("{BLUEPRINT_KEY}-extra"), "true")],
    )
    .await;
    let key_as_value =
        create_function(&router, REGION, "filter-key-as-value", &[("owner", BLUEPRINT_KEY)]).await;

    // Columns: the case, the `TagFilters` entry, and the ARNs it must select.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the key alone selects every resource carrying that exact key",
            json!({ "Key": BLUEPRINT_KEY }),
            vec![hit.clone(), other_case.clone()],
        ),
        (
            "a key the resources' key is a prefix of selects none of them",
            json!({ "Key": format!("{BLUEPRINT_KEY}-extra") }),
            vec![longer_key.clone()],
        ),
        (
            "one exact value selects only the resource carrying it",
            json!({ "Key": BLUEPRINT_KEY, "Values": ["true"] }),
            vec![hit.clone()],
        ),
        (
            "value matching is case-sensitive",
            json!({ "Key": BLUEPRINT_KEY, "Values": ["True"] }),
            vec![other_case.clone()],
        ),
        (
            "several values are a union of exact strings",
            json!({ "Key": BLUEPRINT_KEY, "Values": ["false", "true"] }),
            vec![hit.clone()],
        ),
        (
            "a value no resource carries selects none",
            json!({ "Key": BLUEPRINT_KEY, "Values": ["TRUE"] }),
            Vec::new(),
        ),
    ];

    for (case, filter, wanted) in cases {
        let mut wanted = wanted;
        wanted.sort();
        let selected = selected_arns(
            &router,
            json!({
                "TagFilters": [filter],
                "ResourceTypeFilters": ["lambda:function"],
            }),
        )
        .await;
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected {wanted:?}, got {selected:?}"
        );
    }

    // The resource whose tag VALUE spells the key is never selected by a filter on
    // that key: a filter matches keys, not values.
    let selected = selected_arns(
        &router,
        json!({
            "TagFilters": [{ "Key": BLUEPRINT_KEY }],
            "ResourceTypeFilters": ["lambda:function"],
        }),
    )
    .await;
    assert!(
        !selected.contains(&key_as_value),
        "a key appearing as a VALUE is no match for a filter on that key, got {selected:?}"
    );
}

/// RULE: `ResourcesPerPage` caps each page by count and following the
/// `PaginationToken` enumerates every match exactly once.
#[tokio::test]
async fn following_the_token_enumerates_every_match_once() {
    let router = emulator();
    let mut wanted: Vec<String> = Vec::new();
    for name in ["page-a", "page-b", "page-c"] {
        wanted.push(create_function(&router, REGION, name, &[(BLUEPRINT_KEY, "true")]).await);
    }
    wanted.sort();

    // Columns: the page size, and how many resources each page may hold.
    for per_page in [1_usize, 2, 3] {
        let pages = walked_pages(
            &router,
            json!({
                "TagFilters": [{ "Key": BLUEPRINT_KEY }],
                "ResourceTypeFilters": ["lambda:function"],
                "ResourcesPerPage": per_page,
            }),
        )
        .await;
        for page in &pages {
            assert!(
                page.len() <= per_page,
                "case ResourcesPerPage={per_page}: a page holds at most the page size, \
                 got {page:?}"
            );
        }
        let mut walked: Vec<String> = pages.concat();
        let enumerated = walked.len();
        walked.sort();
        walked.dedup();
        assert_eq!(
            walked.len(),
            enumerated,
            "case ResourcesPerPage={per_page}: a walk reports no resource twice, got \
             {pages:?}"
        );
        assert_eq!(
            walked, wanted,
            "case ResourcesPerPage={per_page}: a walk reports every match, got {pages:?}"
        );
    }
}

/// RULE: this API answers for the Region the request is scoped to, so a resource
/// of another Region is no row of it; and a resource the owning service DELETES
/// leaves the projection while an identically tagged sibling stays.
#[tokio::test]
async fn the_projection_is_region_scoped_and_follows_the_owner() {
    let router = emulator();
    let here = create_function(&router, REGION, "scoped-here", &[(BLUEPRINT_KEY, "true")]).await;
    let sibling =
        create_function(&router, REGION, "scoped-sibling", &[(BLUEPRINT_KEY, "true")]).await;
    let elsewhere =
        create_function(&router, OTHER_REGION, "scoped-there", &[(BLUEPRINT_KEY, "true")]).await;

    // Columns: the Region the query is made in, and the ARNs it must select.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the serving Region reports its own resources",
            REGION,
            vec![here.clone(), sibling.clone()],
        ),
        (
            "another Region reports its own and none of theirs",
            OTHER_REGION,
            vec![elsewhere.clone()],
        ),
    ];
    for (case, region, wanted) in cases {
        let mut wanted = wanted;
        wanted.sort();
        let selected = selected_arns_in(
            &router,
            region,
            json!({
                "TagFilters": [{ "Key": BLUEPRINT_KEY }],
                "ResourceTypeFilters": ["lambda:function"],
            }),
        )
        .await;
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected {wanted:?} in {region}, got {selected:?}"
        );
    }

    // `DeleteFunction` withdraws the function from the query, with no untag call.
    lambda_ok(
        &router,
        REGION,
        "DELETE",
        "/2015-03-31/functions/scoped-here",
        &Value::Null,
        204,
    )
    .await;
    let selected = selected_arns(
        &router,
        json!({
            "TagFilters": [{ "Key": BLUEPRINT_KEY }],
            "ResourceTypeFilters": ["lambda:function"],
        }),
    )
    .await;
    assert_eq!(
        selected,
        vec![sibling.clone()],
        "a deleted function leaves the query while its identically tagged sibling stays, \
         got {selected:?} (deleted {here})"
    );
}

/// RULE: Lambda tags belong to the FUNCTION, never to one of its versions or
/// aliases, so only the unqualified function ARN is ever a row of this API.
#[tokio::test]
async fn neither_a_version_nor_an_alias_is_a_tagged_resource() {
    let router = emulator();
    let arn = create_function(&router, REGION, "versioned", &[(BLUEPRINT_KEY, "true")]).await;
    lambda_ok(
        &router,
        REGION,
        "POST",
        "/2015-03-31/functions/versioned/versions",
        &json!({}),
        201,
    )
    .await;
    lambda_ok(
        &router,
        REGION,
        "POST",
        "/2015-03-31/functions/versioned/aliases",
        &json!({ "Name": "live", "FunctionVersion": "1" }),
        201,
    )
    .await;

    let selected = selected_arns(
        &router,
        json!({
            "TagFilters": [{ "Key": BLUEPRINT_KEY }],
            "ResourceTypeFilters": ["lambda:function"],
        }),
    )
    .await;
    assert_eq!(
        selected,
        vec![arn.clone()],
        "only the unqualified function ARN is a tagged resource, got {selected:?}"
    );
}

/// RULE: `TagResources` and `UntagResources` write the OWNING service's own tag
/// set, so what they wrote is what that service's own read reports back — and
/// what this API's projection reports.
#[tokio::test]
async fn a_tag_written_here_is_the_tag_the_owner_reads_back() {
    let router = emulator();
    let arn = create_function(&router, REGION, "write-through", &[]).await;

    /// Columns: the case, the tags the write carries, and the tag set the function
    /// holds afterwards — so a second write onto a key already held overwrites it
    /// and leaves the rest alone.
    type Write = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static [(&'static str, &'static str)],
    );
    let cases: Vec<Write> = vec![
        (
            "the first write onto a function with no tags",
            &[(BLUEPRINT_KEY, "true")],
            &[(BLUEPRINT_KEY, "true")],
        ),
        (
            "a write adding two more keys",
            &[("Environment", "prod"), ("owner", "billing")],
            &[
                ("Environment", "prod"),
                (BLUEPRINT_KEY, "true"),
                ("owner", "billing"),
            ],
        ),
        (
            "a write onto a key already held replaces only that key",
            &[(BLUEPRINT_KEY, "false")],
            &[
                ("Environment", "prod"),
                (BLUEPRINT_KEY, "false"),
                ("owner", "billing"),
            ],
        ),
    ];

    for (case, written, held) in cases {
        let answer = tagging_ok(
            &router,
            "TagResources",
            json!({ "ResourceARNList": [&arn], "Tags": tag_object(written) }),
        )
        .await;
        assert_eq!(
            failures(&answer),
            BTreeMap::new(),
            "case {case:?}: a write to a resource the owner holds fails nothing"
        );
        let reported = lambda_tags(&router, &arn).await;
        assert_eq!(
            reported,
            expected(held),
            "case {case:?}: Lambda's own ListTags must report what this API wrote, got \
             {reported:?}"
        );
        let body = tagging_ok(&router, "GetResources", json!({ "ResourceARNList": [&arn] })).await;
        let projected = page_tags(&body, &arn);
        assert_eq!(
            projected, reported,
            "case {case:?}: the projection and the owner's own read are one tag set"
        );
    }

    // And the other direction: `UntagResources` removes exactly the named keys
    // from the owning service's own tag set.
    tagging_ok(
        &router,
        "UntagResources",
        json!({ "ResourceARNList": [&arn], "TagKeys": [BLUEPRINT_KEY, "owner"] }),
    )
    .await;
    let reported = lambda_tags(&router, &arn).await;
    assert_eq!(
        reported,
        expected(&[("Environment", "prod")]),
        "UntagResources removes exactly the named keys from the owner's tag set, got \
         {reported:?}"
    );
}

/// RULE: `TagResources` is PER RESOURCE — it answers 200 and places every ARN it
/// could not tag in `FailedResourcesMap` carrying the OWNING service's own
/// refusal, while the resolvable ARNs of the same request are tagged.
#[tokio::test]
async fn an_arn_the_owner_will_not_tag_fails_on_its_own() {
    let router = emulator();
    let live = create_function(&router, REGION, "per-resource-live", &[]).await;
    lambda_ok(
        &router,
        REGION,
        "POST",
        "/2015-03-31/functions/per-resource-live/versions",
        &json!({}),
        201,
    )
    .await;
    let ghost = function_arn(REGION, "per-resource-absent");

    // The wording of each refusal is the OWNING service's own: "Function not
    // found: <arn>" for a function it does not hold (`ResourceNotFoundException`,
    // httpError 404) and the fixed sentence for an ARN that names a version or an
    // alias (`InvalidParameterValueException`, httpError 400), from
    // `smithy_query(query="operation", operation="TagResource")` on the Lambda
    // model and the `Resource` member's own documentation.
    //
    // Columns: the case, the ARN, and the failure it earns.
    let cases: Vec<(&str, String, (i64, String, String))> = vec![
        (
            "a function ARN the owner holds no function for",
            ghost.clone(),
            (
                404,
                "ResourceNotFoundException".to_string(),
                format!("Function not found: {ghost}"),
            ),
        ),
        (
            "a version-qualified function ARN",
            format!("{live}:1"),
            (
                400,
                "InvalidParameterValueException".to_string(),
                "Tags on function aliases and versions are not supported. Please specify a \
                 function ARN."
                    .to_string(),
            ),
        ),
        (
            "an alias-qualified function ARN",
            format!("{live}:live"),
            (
                400,
                "InvalidParameterValueException".to_string(),
                "Tags on function aliases and versions are not supported. Please specify a \
                 function ARN."
                    .to_string(),
            ),
        ),
    ];

    for (case, arn, failure) in cases {
        let (status, answer) = tagging(
            &router,
            "TagResources",
            json!({
                "ResourceARNList": [&live, &arn],
                "Tags": { BLUEPRINT_KEY: "true" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a per-resource failure is still HTTP 200, got {status} with \
             body {answer}"
        );
        let failed = failures(&answer);
        assert_eq!(
            failed,
            [(arn.clone(), failure)].into_iter().collect(),
            "case {case:?}: exactly that ARN must fail, with the owning service's own \
             refusal"
        );
        let reported = lambda_tags(&router, &live).await;
        assert_eq!(
            reported,
            expected(&[(BLUEPRINT_KEY, "true")]),
            "case {case:?}: the resolvable ARN of the same request is tagged, got \
             {reported:?}"
        );
        tagging_ok(
            &router,
            "UntagResources",
            json!({ "ResourceARNList": [&live], "TagKeys": [BLUEPRINT_KEY] }),
        )
        .await;
    }
}

/// RULE: a constraint clause about a COLLECTION member quotes the collection the
/// request carried — a map as `{<key>=<value>, …}` over its entries in the order
/// the request wrote them — not a summary of its size.
///
/// The limit and the member name come from the model
/// (`smithy_query(query="operation", operation="TagResources")`: `Tags` targets
/// `TagMap`, `@length(1, 50)`). The rows span how many entries the map holds and
/// the ORDER the request carried them in, which is what tells a quoted map from
/// one rebuilt in some order of the server's own.
#[tokio::test]
async fn the_clause_about_the_tags_map_quotes_the_map_the_request_carried() {
    let router = emulator();
    let arn = create_function(&router, REGION, "constraint", &[]).await;

    /// `count` tag pairs, each key distinct, in ascending index order.
    fn tags_of(count: usize) -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("k{index}"), format!("v{index}")))
            .collect()
    }

    /// The pairs as the map the clause quotes: every entry `<key>=<value>`,
    /// comma-separated inside braces, in the order given.
    fn rendered(pairs: &[(String, String)]) -> String {
        let entries: Vec<String> = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{{{}}}", entries.join(", "))
    }

    /// The `TagResources` body carrying `pairs` as `Tags`, written by hand so the
    /// ENTRY ORDER is the one the row states (a `serde_json` object would key-sort
    /// them).
    fn request_body(arn: &str, pairs: &[(String, String)]) -> String {
        let entries: Vec<String> = pairs
            .iter()
            .map(|(key, value)| format!("{}:{}", json!(key), json!(value)))
            .collect();
        format!(
            "{{\"ResourceARNList\":[{}],\"Tags\":{{{}}}}}",
            json!(arn),
            entries.join(",")
        )
    }

    /// Columns: the case, the tags the request carries in the order it carries
    /// them, and whether the model admits that many.
    type Case = (&'static str, Vec<(String, String)>, bool);
    let mut reversed = tags_of(51);
    reversed.reverse();
    let cases: Vec<Case> = vec![
        ("one tag past the maximum", tags_of(51), false),
        (
            "one tag past the maximum, written in the opposite order",
            reversed,
            false,
        ),
        ("ten tags past the maximum", tags_of(60), false),
        ("exactly the maximum", tags_of(50), true),
    ];

    for (case, pairs, admitted) in cases {
        let (status, answer) =
            tagging_raw(&router, "TagResources", request_body(&arn, &pairs)).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: a map the model admits is served, got {status} with body \
                 {answer}"
            );
            assert_eq!(
                lambda_tags(&router, &arn).await.len(),
                pairs.len(),
                "case {case:?}: every admitted tag is written"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: a map past the maximum is refused, got {status} with body \
             {answer}"
        );
        let code = answer["__type"].as_str().unwrap_or_default();
        let message = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            (code, message),
            (
                "ValidationException",
                format!(
                    "1 validation error detected: Value '{}' at 'tags' failed to satisfy \
                     constraint: Member must have length less than or equal to 50",
                    rendered(&pairs)
                )
                .as_str()
            ),
            "case {case:?}: expected the clause to quote the rejected map in the order the \
             request carried it, got {answer}"
        );
        // A refused call writes nothing: the whole request is rejected.
        let held = lambda_tags(&router, &arn).await;
        assert!(
            held.is_empty(),
            "case {case:?}: a refused request writes no tag, got {held:?}"
        );
    }
}
