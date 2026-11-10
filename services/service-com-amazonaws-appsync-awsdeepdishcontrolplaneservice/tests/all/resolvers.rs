// SPDX-License-Identifier: CC0-1.0

//! Resolvers, end to end over HTTP: `ListResolvers`, and the `CreateResolver` /
//! `GetResolver` pair whose stored records it pages over.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. a listing reports the resolvers attached to the fields of the type it names,
//!    under the API it names and no other, in one order that does not depend on how the
//!    caller pages, cut into a page of at most `maxResults` entries (the modelled
//!    maximum when the request names none) and resumed by the `nextToken` the previous
//!    page handed back, which is reported exactly while entries remain; each entry is
//!    the record `GetResolver` answers for that field; and the call is a pure read, so
//!    paging through a type leaves every later reader seeing the same set;
//! 2. a listing scoped to a FUNCTION rather than a type follows the same rule: it
//!    reports the resolvers of the API that invoke that function, whatever type each is
//!    attached to, so its cursor is the type/field pair that addresses one;
//! 3. attaching a resolver persists it under its API and answers the stored record —
//!    its ARN named under the API's own type/field path, its `kind` (`UNIT` where the
//!    request named none, "the default resolver type"), its mapping templates, `code`,
//!    `pipelineConfig`, `cachingConfig`, `syncConfig`, `maxBatchSize` and
//!    `metricsConfig` as the request set them — and `GetResolver` answers the same
//!    body;
//! 4. a resolver is addressed by the type and field it is attached to, so a field that
//!    carries none is a 404 `NotFoundException` with AppSync's fixed no-resolver text,
//!    and a field that already carries one is not re-attached;
//! 5. a definition AppSync could not run is a 400 `BadRequestException` that attaches
//!    nothing: a `UNIT` resolver carrying a pipeline, a `PIPELINE` resolver carrying
//!    none, a `UNIT` resolver naming no data source, and `code` and `runtime` without
//!    each other;
//! 6. an `apiId` that cannot name an API is a 400 `BadRequestException`, one that names
//!    no API this account holds a 404 `NotFoundException`, and a `nextToken` that names
//!    no page of the list AppSync's fixed invalid-token text.
//!
//! The rows span the whole definition surface of a resolver: both `ResolverKind` values
//! and a request that names none, VTL mapping templates and `code` with a `runtime`,
//! both published `maxBatchSize` bounds, both `ResolverLevelMetricsConfig` values, a
//! `cachingConfig` at its published TTL bounds, the `SyncConfig` strategies, and a
//! template carrying non-ASCII text.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together. The harness (`call`, `create_api`, `router`,
//! `Answer`) is the error-contract suite's and the data-source and function helpers are
//! the function suites' — reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router, Answer};
use super::functions::{create_data_source, create_function, rows as function_rows, RUNTIME};

/// The name of the one data source the seeded resolvers resolve against.
const DATA_SOURCE: &str = "ds_resolvers";

/// The type whose fields the seeded resolvers are attached to.
const TYPE: &str = "Query";

/// A second type of the same API, so a listing of one type is not a listing of the API.
const OTHER_TYPE: &str = "Mutation";

/// The counts of resolvers a type is listed at: none, one, several, exactly one modelled
/// page, and more than one page.
const COUNTS: &[usize] = &[0, 1, 4, state::DEFAULT_PAGE_SIZE, state::DEFAULT_PAGE_SIZE + 7];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListResolversRequest` (`@range` 0..25, default 0 —
/// i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(25)];

/// The published bounds of `maxBatchSize` (`MaxBatchSize`, `@range` 0..2000) plus a value
/// between them. Source: `smithy_query(query="shape", shape="MaxBatchSize")`.
const MAX_BATCH_SIZES: &[i64] = &[0, 7, 2_000];

/// The published bounds of a resolver cache's TTL: "Valid values are 1–3,600 seconds"
/// (`CachingConfig$ttl`).
const CACHING_TTLS: &[i64] = &[1, 3_600];

/// `ResolverLevelMetricsConfig`'s whole value set. Source:
/// `smithy_query(query="shape", shape="ResolverLevelMetricsConfig")`.
const METRICS_CONFIGS: &[&str] = &["ENABLED", "DISABLED"];

/// One row of the round-trip table: a resolver to attach, minus the field it goes on.
#[derive(Debug, Clone)]
struct Row {
    /// What the row exercises, named in every assertion of it.
    label: &'static str,
    /// The `ResolverKind` the request names, or `None` where it names none — which asks
    /// for a `UNIT` resolver, "the default resolver type" (`Resolver$kind`).
    kind: Option<&'static str>,
    /// Whether the request names the data source the resolver resolves against.
    data_source: bool,
    /// Whether the request carries a `pipelineConfig` naming the seeded function.
    pipeline: bool,
    request_template: Option<&'static str>,
    response_template: Option<&'static str>,
    code: Option<&'static str>,
    caching_config: Option<Value>,
    sync_config: Option<Value>,
    max_batch_size: Option<i64>,
    metrics_config: Option<&'static str>,
}

impl Row {
    /// A row with every optional member unset, which each row below fills in.
    fn bare(label: &'static str) -> Self {
        Self {
            label,
            kind: None,
            data_source: false,
            pipeline: false,
            request_template: None,
            response_template: None,
            code: None,
            caching_config: None,
            sync_config: None,
            max_batch_size: None,
            metrics_config: None,
        }
    }

    /// The `kind` the stored resolver is of: the one the request named, or the default
    /// `UNIT` where it named none.
    fn expected_kind(&self) -> &str {
        self.kind.unwrap_or(state::UNIT_KIND)
    }
}

/// The round-trip table: every definition form a resolver can take.
///
/// Source for the value sets: `smithy_query(query="shape",
/// shape="Resolver,ResolverKind,PipelineConfig,CachingConfig,SyncConfig,
/// ResolverLevelMetricsConfig,MaxBatchSize,AppSyncRuntime")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The smallest resolver, and a request that names no kind at all.
            data_source: true,
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            ..Row::bare("a UNIT resolver whose request names no kind")
        },
        Row {
            kind: Some(state::UNIT_KIND),
            data_source: true,
            request_template: Some("{\"version\": \"2018-05-29\", \"payload\": {}}"),
            response_template: Some("$util.toJson($ctx.result)"),
            max_batch_size: Some(MAX_BATCH_SIZES[0]),
            metrics_config: Some(METRICS_CONFIGS[1]),
            ..Row::bare("a UNIT resolver at the minimum batch size")
        },
        Row {
            kind: Some(state::UNIT_KIND),
            data_source: true,
            code: Some("export function request(ctx) { return {}; }"),
            max_batch_size: Some(MAX_BATCH_SIZES[2]),
            metrics_config: Some(METRICS_CONFIGS[0]),
            caching_config: Some(json!({
                "ttl": CACHING_TTLS[1],
                "cachingKeys": ["$context.arguments.id", "$context.identity.sub"],
            })),
            ..Row::bare("an APPSYNC_JS UNIT resolver, cached at the maximum TTL")
        },
        Row {
            data_source: true,
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            response_template: Some("$util.toJson($ctx.result)"),
            max_batch_size: Some(MAX_BATCH_SIZES[1]),
            caching_config: Some(json!({"ttl": CACHING_TTLS[0]})),
            sync_config: Some(json!({
                "conflictDetection": "VERSION",
                "conflictHandler": "OPTIMISTIC_CONCURRENCY",
            })),
            ..Row::bare("a versioned UNIT resolver, cached at the minimum TTL")
        },
        Row {
            kind: Some(state::PIPELINE_KIND),
            pipeline: true,
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            response_template: Some("$util.toJson($ctx.result)"),
            ..Row::bare("a PIPELINE resolver over one function")
        },
        Row {
            kind: Some(state::PIPELINE_KIND),
            pipeline: true,
            code: Some("export function request(ctx) { return ctx.args; }"),
            metrics_config: Some(METRICS_CONFIGS[0]),
            sync_config: Some(json!({
                "conflictDetection": "NONE",
                "conflictHandler": "NONE",
            })),
            ..Row::bare("an APPSYNC_JS PIPELINE resolver")
        },
        Row {
            // A mapping template is stored verbatim, whatever characters it carries:
            // `MappingTemplate` constrains the length and `^.*$`, no character class.
            data_source: true,
            request_template: Some("## héllo 世界 🎉 \"quoted\" \\backslash\\ Ω≈ç√"),
            response_template: Some("$util.toJson($ctx.result) ## ✅"),
            ..Row::bare("a UNIT resolver whose templates carry non-ASCII text")
        },
    ]
}

/// `row` as a `CreateResolver` request body attaching it to `field`.
fn request_body(row: &Row, field: &str, function_id: &str) -> Value {
    let mut body = json!({"fieldName": field});
    let object = body.as_object_mut().expect("the body is an object");
    if let Some(kind) = row.kind {
        object.insert("kind".to_string(), json!(kind));
    }
    if row.data_source {
        object.insert("dataSourceName".to_string(), json!(DATA_SOURCE));
    }
    if row.pipeline {
        object.insert(
            "pipelineConfig".to_string(),
            json!({"functions": [function_id]}),
        );
    }
    if let Some(template) = row.request_template {
        object.insert("requestMappingTemplate".to_string(), json!(template));
    }
    if let Some(template) = row.response_template {
        object.insert("responseMappingTemplate".to_string(), json!(template));
    }
    if let Some(code) = row.code {
        object.insert("code".to_string(), json!(code));
        // "When code is used, the runtime is required" (`Resolver$code`).
        object.insert(
            "runtime".to_string(),
            json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1}),
        );
    }
    if let Some(caching) = &row.caching_config {
        object.insert("cachingConfig".to_string(), caching.clone());
    }
    if let Some(sync) = &row.sync_config {
        object.insert("syncConfig".to_string(), sync.clone());
    }
    if let Some(max_batch_size) = row.max_batch_size {
        object.insert("maxBatchSize".to_string(), json!(max_batch_size));
    }
    if let Some(metrics) = row.metrics_config {
        object.insert("metricsConfig".to_string(), json!(metrics));
    }
    body
}

/// Send one `CreateResolver` for `body` on the type `type_name`.
async fn create_resolver_body(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(body),
    )
    .await
}

/// Read one resolver back by the type and field it is attached to.
async fn get_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        None,
    )
    .await
}

/// Which listing a walk pages over: the resolvers attached to the fields of one type,
/// or the resolvers of the whole API that invoke one function.
#[derive(Debug, Clone, Copy)]
enum Listing<'a> {
    OfType(&'a str),
    ByFunction(&'a str),
}

impl Listing<'_> {
    /// The route this listing is served on under `api_id`.
    fn path(&self, api_id: &str) -> String {
        match self {
            Self::OfType(type_name) => {
                format!("/v1/apis/{api_id}/types/{type_name}/resolvers")
            }
            Self::ByFunction(function_id) => {
                format!("/v1/apis/{api_id}/functions/{function_id}/resolvers")
            }
        }
    }
}

/// Send one listing request, with the `maxResults` and `nextToken` the caller names.
async fn list_page(
    router: &MultiServiceRouter,
    api_id: &str,
    listing: Listing<'_>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let path = listing.path(api_id);
    let path = if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// One page of a listing: the entries it reported, and the cursor that resumes after
/// them.
#[derive(Debug)]
struct ListedPage {
    fields: Vec<String>,
    next_token: Option<String>,
    resolvers: Vec<Value>,
}

/// One page as the suite reads it, asserting it is a 200 that reports the list member.
fn read_page(case: &str, answer: &Answer) -> ListedPage {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListResolvers 200, got {answer:?}"
    );
    let resolvers: Vec<Value> = answer.body["resolvers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a resolvers list, got {answer:?}"))
        .clone();
    let fields = resolvers
        .iter()
        .map(|resolver| {
            resolver["fieldName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every entry to carry a fieldName, got {answer:?}")
                })
                .to_string()
        })
        .collect();
    ListedPage {
        fields,
        next_token: answer.body["nextToken"].as_str().map(str::to_string),
        resolvers,
    }
}

/// Page through the whole listing at `max_results` and return every entry, in the order
/// it reported them, asserting the page-size bound and the cursor contract on each page.
async fn walk(
    router: &MultiServiceRouter,
    api_id: &str,
    listing: Listing<'_>,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    // An unspecified `maxResults` — absent, or the modelled default of `0` — selects
    // the whole modelled page.
    let bound = match max_results {
        Some(size) if size > 0 => size as usize,
        _ => state::DEFAULT_PAGE_SIZE,
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one entry while entries remain, so the walk cannot take
    // more pages than there are entries, plus the single page of an empty listing.
    for page_index in 0..expected_total + 1 {
        let case = format!(
            "walk {} of {api_id} at maxResults {max_results:?}, page {page_index} from {token:?}",
            listing.path(api_id)
        );
        let answer = list_page(router, api_id, listing, max_results, token.as_deref()).await;
        let page = read_page(&case, &answer);
        let remaining = expected_total - collected.len();
        assert_eq!(
            page.fields.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows (at most \
             {bound}), got {:?}",
            page.fields
        );
        collected.extend(page.resolvers);
        // The cursor is reported exactly while entries remain after this page.
        let more = collected.len() < expected_total;
        assert_eq!(
            page.next_token.is_some(),
            more,
            "case {case}: expected a nextToken {}, got {:?}",
            if more { "while entries remain" } else { "on the last page" },
            page.next_token
        );
        match page.next_token {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!(
        "case walk {} at maxResults {max_results:?}: never reached a last page",
        listing.path(api_id)
    );
}

/// The field names of the entries a walk reported, in the order it reported them.
fn listed_fields(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|resolver| {
            resolver["fieldName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One API with the data source and the function the table's rows need, and the id of
/// that function.
async fn api_with_dependencies(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let api_id = create_api(router, name).await;
    create_data_source(router, &api_id, DATA_SOURCE).await;
    let created = create_function(router, &api_id, DATA_SOURCE, &function_rows()[0]).await;
    assert_eq!(
        created.status, 200,
        "case the pipeline's function under {name}: expected CreateFunction 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the pipeline's function under {name}: expected a minted functionId, got {created:?}"))
        .to_string();
    (api_id, function_id)
}

/// Attach `count` resolvers to the fields of `type_name`, their definitions rotating
/// across the whole round-trip table, and return the field names in creation order.
async fn seed(
    router: &MultiServiceRouter,
    api_id: &str,
    function_id: &str,
    type_name: &str,
    count: usize,
) -> Vec<String> {
    let table = rows();
    let mut fields = Vec::with_capacity(count);
    for index in 0..count {
        let row = &table[index % table.len()];
        let field = format!("{}_field_{index}", type_name.to_lowercase());
        let created = create_resolver_body(
            router,
            api_id,
            type_name,
            request_body(row, &field, function_id),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed {} on {field}: expected CreateResolver 200, got {created:?}",
            row.label
        );
        fields.push(field);
    }
    fields
}

/// Assert `resolver` reports exactly what `row` asked for, on `field` of `type_name`
/// under `api_id`.
fn assert_projects(
    row: &Row,
    api_id: &str,
    type_name: &str,
    field: &str,
    function_id: &str,
    resolver: &Value,
    source: &str,
) {
    let case = format!("{} on {field} ({source})", row.label);

    assert_eq!(
        resolver["typeName"], json!(type_name),
        "case {case}: expected the type it is attached to, got {resolver:?}"
    );
    assert_eq!(
        resolver["fieldName"], json!(field),
        "case {case}: expected the field it is attached to, got {resolver:?}"
    );
    // A resolver is named under its API's own type/field path, in the region the caller
    // addressed.
    let expected_arn = format!(
        "arn:aws:{}:{}:{}:{}",
        state::ARN_SERVICE,
        super::functions::REGION,
        super::functions::ACCOUNT,
        state::resolver_resource(api_id, type_name, field)
    );
    assert_eq!(
        resolver["resolverArn"], json!(expected_arn),
        "case {case}: expected the resolver named under its API, got {resolver:?}"
    );
    assert_eq!(
        resolver["kind"], json!(row.expected_kind()),
        "case {case}: expected kind {:?}, got {resolver:?}",
        row.expected_kind()
    );
    assert_eq!(
        resolver["dataSourceName"],
        if row.data_source {
            json!(DATA_SOURCE)
        } else {
            Value::Null
        },
        "case {case}: expected the data source the request named, got {resolver:?}"
    );
    assert_eq!(
        resolver["pipelineConfig"],
        if row.pipeline {
            json!({"functions": [function_id]})
        } else {
            Value::Null
        },
        "case {case}: expected the pipeline the request named, got {resolver:?}"
    );
    // The templates and the code are stored verbatim, byte for byte.
    assert_eq!(
        resolver["requestMappingTemplate"],
        row.request_template.map_or(Value::Null, |t| json!(t)),
        "case {case}: expected the request template verbatim, got {resolver:?}"
    );
    assert_eq!(
        resolver["responseMappingTemplate"],
        row.response_template.map_or(Value::Null, |t| json!(t)),
        "case {case}: expected the response template verbatim, got {resolver:?}"
    );
    assert_eq!(
        resolver["code"],
        row.code.map_or(Value::Null, |code| json!(code)),
        "case {case}: expected the code verbatim, got {resolver:?}"
    );
    assert_eq!(
        resolver["runtime"],
        if row.code.is_some() {
            json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1})
        } else {
            Value::Null
        },
        "case {case}: expected the runtime the code needs, got {resolver:?}"
    );
    assert_eq!(
        resolver["cachingConfig"],
        row.caching_config.clone().unwrap_or(Value::Null),
        "case {case}: expected the caching configuration the request set, got {resolver:?}"
    );
    assert_eq!(
        resolver["syncConfig"],
        row.sync_config.clone().unwrap_or(Value::Null),
        "case {case}: expected the sync configuration the request set, got {resolver:?}"
    );
    // `maxBatchSize` is modelled with a default of 0, so a request that names none
    // reports that default rather than no member.
    assert_eq!(
        resolver["maxBatchSize"], json!(row.max_batch_size.unwrap_or(0)),
        "case {case}: expected the batch size the request set, got {resolver:?}"
    );
    assert_eq!(
        resolver["metricsConfig"],
        row.metrics_config.map_or(Value::Null, |m| json!(m)),
        "case {case}: expected the metrics configuration the request set, got {resolver:?}"
    );
}

/// Attaching a resolver persists it and answers the stored record, and `GetResolver`
/// answers the same body — over the whole definition table.
#[tokio::test]
async fn attaching_a_resolver_persists_what_the_request_asked_for() {
    let router = router();
    let (api_id, function_id) = api_with_dependencies(&router, "Api_resolver_round_trip").await;

    for (index, row) in rows().iter().enumerate() {
        let field = format!("round_trip_{index}");
        let created = create_resolver_body(
            &router,
            &api_id,
            TYPE,
            request_body(row, &field, &function_id),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateResolver 200, got {created:?}",
            row.label
        );
        assert_projects(
            row,
            &api_id,
            TYPE,
            &field,
            &function_id,
            &created.body["resolver"],
            "the create",
        );

        // The round trip: the stored resolver reads back through the implemented
        // reader, member for member.
        let read = get_resolver(&router, &api_id, TYPE, &field).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetResolver 200, got {read:?}",
            row.label
        );
        assert_projects(
            row,
            &api_id,
            TYPE,
            &field,
            &function_id,
            &read.body["resolver"],
            "the read",
        );
        assert_eq!(
            read.body["resolver"], created.body["resolver"],
            "case {}: expected the read to answer the created body {:?}, got {:?}",
            row.label, created.body["resolver"], read.body["resolver"]
        );
    }
}

/// Every walk of a type reports the same resolvers in the same order, however the caller
/// pages, and every resolver the type holds exactly once — and the walk changes nothing
/// a later caller sees.
#[tokio::test]
async fn every_page_size_walks_the_whole_type_in_one_order() {
    let router = router();

    for count in COUNTS.iter().copied() {
        let (api_id, function_id) =
            api_with_dependencies(&router, &format!("Api_list_resolvers_{count}")).await;
        let mut seeded = seed(&router, &api_id, &function_id, TYPE, count).await;
        seeded.sort();

        let mut reference: Option<Vec<Value>> = None;
        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, &api_id, Listing::OfType(TYPE), max_results, count).await;
            let fields = listed_fields(&listed);
            // Every seeded resolver, exactly once: a set comparison that is blind to
            // the order, which the next assertion pins separately.
            let mut sorted = fields.clone();
            sorted.sort();
            assert_eq!(
                sorted, seeded,
                "case {count} resolvers at maxResults {max_results:?}: expected every seeded \
                 field exactly once, got {fields:?}"
            );
            match &reference {
                None => reference = Some(listed),
                Some(reference) => assert_eq!(
                    &listed, reference,
                    "case {count} resolvers at maxResults {max_results:?}: expected the same \
                     entries in the same order as the first walk, got {fields:?}"
                ),
            }
        }

        // Each listed entry is the record the reader answers for that field, so a
        // listing projects the stored resolver rather than a summary of it.
        for entry in reference.unwrap_or_default() {
            let field = entry["fieldName"].as_str().unwrap_or_default();
            let read = get_resolver(&router, &api_id, TYPE, field).await;
            assert_eq!(
                read.status, 200,
                "case listed {field:?} of {count}: expected GetResolver 200, got {read:?}"
            );
            assert_eq!(
                read.body["resolver"], entry,
                "case listed {field:?} of {count}: expected the listing to answer the stored \
                 record {:?}, got {entry:?}",
                read.body["resolver"]
            );
        }
    }
}

/// A listing reports the resolvers of the type AND the API it names, and no others —
/// even where two types and two APIs hold resolvers created from the same requests.
#[tokio::test]
async fn a_listing_reports_only_the_type_and_api_it_names() {
    let router = router();
    let (first, first_function) =
        api_with_dependencies(&router, "Api_list_resolvers_scope_first").await;
    let (second, second_function) =
        api_with_dependencies(&router, "Api_list_resolvers_scope_second").await;

    // Two types of the first API hold different numbers of resolvers, and the second
    // API holds resolvers created from the same leading rows of the table.
    let query_fields = seed(&router, &first, &first_function, TYPE, 5).await;
    let mutation_fields = seed(&router, &first, &first_function, OTHER_TYPE, 2).await;
    let second_fields = seed(&router, &second, &second_function, TYPE, 3).await;

    for (api_id, type_name, expected) in [
        (&first, TYPE, &query_fields),
        (&first, OTHER_TYPE, &mutation_fields),
        (&second, TYPE, &second_fields),
    ] {
        let listed = walk(
            &router,
            api_id,
            Listing::OfType(type_name),
            Some(1),
            expected.len(),
        )
        .await;
        let mut fields = listed_fields(&listed);
        fields.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            fields, expected_sorted,
            "case the listing of {type_name} under {api_id}: expected exactly its own resolvers \
             {expected_sorted:?}, got {fields:?}"
        );
        // Every entry is named under the API and type that hold it, so the listings
        // report different resources.
        for resolver in &listed {
            let field = resolver["fieldName"].as_str().unwrap_or_default();
            let arn = resolver["resolverArn"].as_str().unwrap_or_default();
            let resource = state::resolver_resource(api_id, type_name, field);
            assert!(
                arn.ends_with(&resource),
                "case the listing of {type_name} under {api_id}: expected {field:?} named under \
                 {resource}, got {arn:?}"
            );
            assert_eq!(
                resolver["typeName"], json!(type_name),
                "case the listing of {type_name} under {api_id}: expected every entry on that \
                 type, got {resolver:?}"
            );
        }
    }

    // A type nothing is attached to carries no resolver, which is the empty listing:
    // this operation reports what is attached to a type rather than resolving the type.
    for type_name in ["Subscription", OTHER_TYPE] {
        let listed = walk(&router, &second, Listing::OfType(type_name), None, 0).await;
        assert!(
            listed.is_empty(),
            "case the listing of {type_name} under the second API: expected no entries, got \
             {listed:?}"
        );
    }
}

/// A listing that addresses nothing is refused: an `apiId` too long to name an API is a
/// 400, one of the right shape this account does not hold a 404, and a token that names
/// no page of the list AppSync's fixed invalid-token text.
#[tokio::test]
async fn a_listing_that_addresses_nothing_is_refused() {
    let router = router();
    let (api_id, function_id) =
        api_with_dependencies(&router, "Api_list_resolvers_refusals").await;
    let seeded = seed(&router, &api_id, &function_id, TYPE, 3).await;

    // An `apiId` longer than an AppSync API id holds more characters than one has, so
    // it names no API at all; an id of the right length this account simply does not
    // hold names nothing.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    let absent = "z".repeat(state::API_ID_LEN);
    for (api, expected_status, expected_error) in [
        (unnameable.as_str(), 400, "BadRequestException"),
        (absent.as_str(), 404, "NotFoundException"),
    ] {
        for max_results in PAGE_SIZES.iter().copied() {
            let answer = list_page(&router, api, Listing::OfType(TYPE), max_results, None).await;
            assert_eq!(
                answer.status, expected_status,
                "case list {TYPE} under {api:?} at maxResults {max_results:?}: expected \
                 {expected_status}, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, expected_error,
                "case list {TYPE} under {api:?} at maxResults {max_results:?}: expected \
                 {expected_error}, got {answer:?}"
            );
        }
        // The reader answers the absent API the same way.
        let read = get_resolver(&router, api, TYPE, &seeded[0]).await;
        assert_eq!(
            read.status, expected_status,
            "case read {TYPE} under {api:?}: expected {expected_status}, got {read:?}"
        );
    }

    // Tokens that name no page of this list: a value that is no field name at all, a
    // field of ANOTHER type of the same API, and a well-formed field name of the right
    // shape this type does not hold.
    let other_type = seed(&router, &api_id, &function_id, OTHER_TYPE, 1).await;
    for token in ["bogus", "0", other_type[0].as_str(), "query_field_42"] {
        let answer = list_page(&router, &api_id, Listing::OfType(TYPE), None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case resuming from {token:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case resuming from {token:?}: expected BadRequestException, got {answer:?}"
        );
        assert_eq!(
            answer.message, "Invalid pagination token given.",
            "case resuming from {token:?}: expected AppSync's fixed invalid-token text, got \
             {answer:?}"
        );
    }

    // A token the listing itself handed back still resumes, so the refusals above are
    // about tokens that name no page rather than about paging at all.
    let first = list_page(&router, &api_id, Listing::OfType(TYPE), Some(1), None).await;
    let page = read_page("the first page of the refusal API", &first);
    let cursor = page
        .next_token
        .expect("the first page of three resolvers carries a nextToken");
    assert!(
        seeded.contains(&cursor),
        "case the cursor of the first page: expected one of the seeded fields {seeded:?}, got \
         {cursor:?}"
    );
    let resumed =
        list_page(&router, &api_id, Listing::OfType(TYPE), Some(1), Some(&cursor)).await;
    let resumed = read_page("resuming from the listing's own token", &resumed);
    assert_eq!(
        resumed.fields,
        vec![cursor.clone()],
        "case resuming from the listing's own token: expected the entry the cursor names, got \
         {:?}",
        resumed.fields
    );
}

/// A field that carries no resolver is a 404 with AppSync's fixed no-resolver text, and
/// a field that already carries one is not re-attached.
#[tokio::test]
async fn a_field_carries_at_most_one_resolver() {
    let router = router();
    let (api_id, function_id) = api_with_dependencies(&router, "Api_resolver_identity").await;
    let table = rows();

    // Fields nothing is attached to: one on a type that holds resolvers, and one on a
    // type that holds none.
    let seeded = seed(&router, &api_id, &function_id, TYPE, 1).await;
    for (type_name, field) in [(TYPE, "never_attached"), ("Subscription", "never_attached")] {
        let read = get_resolver(&router, &api_id, type_name, field).await;
        assert_eq!(
            read.status, 404,
            "case read {field} of {type_name}: expected 404, got {read:?}"
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case read {field} of {type_name}: expected NotFoundException, got {read:?}"
        );
        assert_eq!(
            read.message, "No resolver found.",
            "case read {field} of {type_name}: expected AppSync's fixed no-resolver text, got \
             {read:?}"
        );
    }

    // A second create on the same field is refused, whatever definition it carries, and
    // the resolver already attached is left exactly as it was.
    let attached = get_resolver(&router, &api_id, TYPE, &seeded[0]).await;
    for row in &table {
        let again = create_resolver_body(
            &router,
            &api_id,
            TYPE,
            request_body(row, &seeded[0], &function_id),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case re-attaching {} to {}: expected 400, got {again:?}",
            row.label, seeded[0]
        );
        assert_eq!(
            again.error_type, "BadRequestException",
            "case re-attaching {} to {}: expected BadRequestException, got {again:?}",
            row.label, seeded[0]
        );
        let read = get_resolver(&router, &api_id, TYPE, &seeded[0]).await;
        assert_eq!(
            read.body["resolver"], attached.body["resolver"],
            "case re-attaching {} to {}: expected the attached resolver untouched {:?}, got {:?}",
            row.label, seeded[0], attached.body["resolver"], read.body["resolver"]
        );
    }

    // The same definition on a FREE field of the same type still attaches, so the
    // refusal is about the field being taken rather than about the definition.
    let free = create_resolver_body(
        &router,
        &api_id,
        TYPE,
        request_body(&table[0], "free_field", &function_id),
    )
    .await;
    assert_eq!(
        free.status, 200,
        "case attaching to a free field of the same type: expected 200, got {free:?}"
    );
}

/// A definition AppSync could not run is refused, and the refused create attaches
/// nothing: the field still carries no resolver and the listing is unchanged.
#[tokio::test]
async fn a_definition_appsync_could_not_run_is_refused() {
    let router = router();
    let (api_id, function_id) = api_with_dependencies(&router, "Api_resolver_definitions").await;
    let before = walk(&router, &api_id, Listing::OfType(TYPE), None, 0).await;
    assert!(
        before.is_empty(),
        "case the empty type: expected no resolvers before any create, got {before:?}"
    );

    let pipeline = json!({"functions": [function_id.clone()]});
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a UNIT resolver carrying a pipeline",
            json!({
                "fieldName": "refused_unit_pipeline",
                "kind": state::UNIT_KIND,
                "dataSourceName": DATA_SOURCE,
                "pipelineConfig": pipeline.clone(),
            }),
            "PipelineConfig should not be specified.",
        ),
        (
            "a resolver that names no kind but carries a pipeline",
            json!({
                "fieldName": "refused_default_pipeline",
                "dataSourceName": DATA_SOURCE,
                "pipelineConfig": json!({"functions": []}),
            }),
            "PipelineConfig should not be specified.",
        ),
        (
            "a PIPELINE resolver carrying no pipeline",
            json!({
                "fieldName": "refused_pipeline_bare",
                "kind": state::PIPELINE_KIND,
                "requestMappingTemplate": "{}",
            }),
            "PipelineConfig can't be null.",
        ),
        (
            "a UNIT resolver naming no data source",
            json!({
                "fieldName": "refused_unit_bare",
                "kind": state::UNIT_KIND,
                "requestMappingTemplate": "{}",
            }),
            "Data source name not specified.",
        ),
        (
            "a resolver that names neither kind nor data source",
            json!({"fieldName": "refused_default_bare"}),
            "Data source name not specified.",
        ),
        (
            "a UNIT resolver whose code names no runtime",
            json!({
                "fieldName": "refused_code_only",
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) { return {}; }",
            }),
            "Invalid runtime given. A runtime is required when code is specified.",
        ),
        (
            "a UNIT resolver whose runtime comes with no code",
            json!({
                "fieldName": "refused_runtime_only",
                "dataSourceName": DATA_SOURCE,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            }),
            "Invalid code given. Code is required when a runtime is specified.",
        ),
    ];

    for (label, body, expected_message) in cases {
        let field = body["fieldName"]
            .as_str()
            .expect("every case names the field it would attach to")
            .to_string();
        let refused = create_resolver_body(&router, &api_id, TYPE, body.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type, "BadRequestException",
            "case {label}: expected BadRequestException, got {refused:?}"
        );
        assert_eq!(
            refused.message, expected_message,
            "case {label}: expected AppSync's text for that refusal, got {refused:?}"
        );
        // The refusal left nothing behind: the field carries no resolver ...
        let read = get_resolver(&router, &api_id, TYPE, &field).await;
        assert_eq!(
            read.status, 404,
            "case {label}: expected the field to carry no resolver, got {read:?}"
        );
        assert_eq!(
            read.message, "No resolver found.",
            "case {label}: expected AppSync's fixed no-resolver text, got {read:?}"
        );
        // ... and the listing still reports none.
        let listed = walk(&router, &api_id, Listing::OfType(TYPE), None, 0).await;
        assert!(
            listed.is_empty(),
            "case {label}: expected the listing to report no resolvers, got {listed:?}"
        );
    }
}

/// A `CreateResolver` body for a PIPELINE resolver on `field` invoking `function_id`.
fn pipeline_body(field: &str, function_id: &str) -> Value {
    json!({
        "fieldName": field,
        "kind": state::PIPELINE_KIND,
        "pipelineConfig": {"functions": [function_id]},
        "requestMappingTemplate": "{\"version\": \"2018-05-29\"}",
    })
}

/// The type/field pairs of the entries a walk reported, in the order it reported them —
/// the pair that addresses a resolver within its API, which is what tells two entries of
/// an API-wide listing apart.
fn listed_pairs(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|resolver| {
            format!(
                "{}/{}",
                resolver["typeName"].as_str().unwrap_or_default(),
                resolver["fieldName"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// A listing by function reports the resolvers of the API that invoke that function —
/// across every type they are attached to — and no others, in one order however the
/// caller pages.
#[tokio::test]
async fn a_listing_by_function_reports_the_resolvers_that_invoke_it() {
    let router = router();
    let (api_id, first_function) =
        api_with_dependencies(&router, "Api_resolvers_by_function").await;
    let created = create_function(&router, &api_id, DATA_SOURCE, &function_rows()[1]).await;
    assert_eq!(
        created.status, 200,
        "case the second function: expected CreateFunction 200, got {created:?}"
    );
    let second_function = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .expect("the second function carries a minted functionId")
        .to_string();

    // Resolvers invoking the first function on fields of TWO types, one pair sharing a
    // field name — so an API-wide listing has to tell its entries apart by more than
    // the field — beside a resolver invoking the OTHER function and one invoking none.
    for (type_name, body) in [
        (TYPE, pipeline_body("shared_field", &first_function)),
        (TYPE, pipeline_body("query_only", &first_function)),
        (OTHER_TYPE, pipeline_body("shared_field", &first_function)),
        (TYPE, pipeline_body("other_function", &second_function)),
        (
            TYPE,
            json!({
                "fieldName": "unit_field",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": "{\"version\": \"2018-05-29\"}",
            }),
        ),
    ] {
        let label = body["fieldName"].as_str().unwrap_or_default().to_string();
        let answer = create_resolver_body(&router, &api_id, type_name, body).await;
        assert_eq!(
            answer.status, 200,
            "case seed {label} on {type_name}: expected CreateResolver 200, got {answer:?}"
        );
    }

    // The resolvers that invoke each function, in creation order.
    let expected_first = vec![
        format!("{TYPE}/shared_field"),
        format!("{TYPE}/query_only"),
        format!("{OTHER_TYPE}/shared_field"),
    ];
    let expected_second = vec![format!("{TYPE}/other_function")];
    for (function_id, expected) in [
        (&first_function, &expected_first),
        (&second_function, &expected_second),
    ] {
        let mut reference: Option<Vec<Value>> = None;
        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(
                &router,
                &api_id,
                Listing::ByFunction(function_id),
                max_results,
                expected.len(),
            )
            .await;
            let pairs = listed_pairs(&listed);
            assert_eq!(
                &pairs, expected,
                "case the listing by {function_id} at maxResults {max_results:?}: expected the \
                 resolvers that invoke it in creation order, got {pairs:?}"
            );
            match &reference {
                None => reference = Some(listed),
                Some(reference) => assert_eq!(
                    &listed, reference,
                    "case the listing by {function_id} at maxResults {max_results:?}: expected \
                     the same entries as the first walk, got {pairs:?}"
                ),
            }
        }
        // Each entry is the record the reader answers for that type and field.
        for entry in reference.unwrap_or_default() {
            let type_name = entry["typeName"].as_str().unwrap_or_default();
            let field = entry["fieldName"].as_str().unwrap_or_default();
            let read = get_resolver(&router, &api_id, type_name, field).await;
            assert_eq!(
                read.body["resolver"], entry,
                "case listed {type_name}/{field} by {function_id}: expected the listing to answer \
                 the stored record {:?}, got {entry:?}",
                read.body["resolver"]
            );
        }
    }

    // A function no resolver of this API invokes is reported by none: an id of the right
    // shape nothing holds, and a function created under ANOTHER API.
    let (other_api, foreign_function) =
        api_with_dependencies(&router, "Api_resolvers_by_function_other").await;
    create_resolver_body(
        &router,
        &other_api,
        TYPE,
        pipeline_body("elsewhere", &foreign_function),
    )
    .await;
    for function_id in ["z".repeat(state::API_ID_LEN).as_str(), &foreign_function] {
        let listed = walk(&router, &api_id, Listing::ByFunction(function_id), None, 0).await;
        assert!(
            listed.is_empty(),
            "case the listing by {function_id:?}: expected no entries, got {listed:?}"
        );
    }

    // A listing that addresses no API is refused exactly as the per-type listing is, and
    // a token that names no page of it takes AppSync's fixed invalid-token text.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    let absent = "z".repeat(state::API_ID_LEN);
    for (api, expected_status, expected_error) in [
        (unnameable.as_str(), 400, "BadRequestException"),
        (absent.as_str(), 404, "NotFoundException"),
    ] {
        let answer = list_page(
            &router,
            api,
            Listing::ByFunction(&first_function),
            None,
            None,
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case list by function under {api:?}: expected {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, expected_error,
            "case list by function under {api:?}: expected {expected_error}, got {answer:?}"
        );
    }
    // A field name alone names no page of an API-wide listing: the cursor is the pair.
    for token in ["bogus", "shared_field", "Subscription/shared_field"] {
        let answer = list_page(
            &router,
            &api_id,
            Listing::ByFunction(&first_function),
            None,
            Some(token),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case resuming the listing by function from {token:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.message, "Invalid pagination token given.",
            "case resuming the listing by function from {token:?}: expected AppSync's fixed \
             invalid-token text, got {answer:?}"
        );
    }
}
