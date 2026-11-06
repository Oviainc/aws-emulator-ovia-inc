// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateView`.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and POSTs restJson1 requests at it, so
//! nothing is proved about the handler that is not also proved about the serialized
//! request and response an SDK sends and receives.
//!
//! Three rules are under test, each over a table of inputs rather than one request:
//!
//! * an update carries the WHOLE new value of the two members a view can change — its
//!   `Filters.FilterString` and its `IncludedProperties` — so what the request omits the
//!   view is left without, and `GetView` reads back exactly what the update answered;
//! * a request the service cannot satisfy (an ARN naming no view, a property name
//!   outside the documented catalog, a value outside a published bound) is refused with
//!   HTTP 400 `ValidationException` and leaves the stored view untouched; and
//! * a search that filters on TAGS is refused unless the view it uses includes the
//!   `tags` property, which is the property `UpdateView` adds and removes.
//!
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header: it is the only place the wire states
//! which Region "the Region in which you call this operation" means.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The published bounds of `SearchFilter$FilterString`
/// (`smithy_query(query="shape",shape="SearchFilter")` reports `length {min: 0, max:
/// 2048}`).
const FILTER_STRING_MIN: usize = 0;
const FILTER_STRING_MAX: usize = 2048;

/// Every value `IncludedProperty$Name` accepts, from that member's own documentation:
/// "You can specify the following property names for this field: `tags`".
const VIEW_PROPERTY_NAMES: &[&str] = &["tags"];

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// POST `/{operation}` as a client signed for `region` would.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// `POST /UpdateView` with whatever members the caller names.
    async fn update_view(&self, region: &str, body: Value) -> Answer {
        self.call(region, "UpdateView", body).await
    }

    /// The `View` structure `GetView` reports for `arn`.
    async fn get_view(&self, region: &str, arn: &str, case: &str) -> Value {
        let answer = self
            .call(region, "GetView", json!({ "ViewArn": arn }))
            .await;
        answer.ok(case)["View"].clone()
    }
}

/// `CreateIndex` in `region`, asserting success: a view can only be created in a Region
/// whose Resource Explorer is turned on.
async fn create_index(emulator: &Emulator, region: &str) {
    emulator
        .call(region, "CreateIndex", json!({ "ClientToken": region }))
        .await
        .ok(region);
}

/// `CreateView` in `region`, asserting success; the `View` it answered with.
async fn create_view(emulator: &Emulator, region: &str, body: Value) -> Value {
    let case = format!("CreateView {body}");
    let answer = emulator.call(region, "CreateView", body).await;
    answer.ok(&case)["View"].clone()
}

/// The `ViewArn` of a `View` structure.
fn view_arn(view: &Value) -> String {
    view["ViewArn"]
        .as_str()
        .expect("a View carries a ViewArn")
        .to_string()
}

/// A view's `Filters.FilterString`, or `None` when it has no filter at all.
fn filter_string(view: &Value) -> Option<String> {
    view["Filters"]["FilterString"]
        .as_str()
        .map(str::to_string)
}

/// The `IncludedProperty$Name`s of a view, in the order it reports them.
fn property_names(view: &Value) -> Vec<String> {
    view["IncludedProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|property| property["Name"].as_str().map(str::to_string))
        .collect()
}

/// A view's `LastUpdatedAt` as a number of seconds, for comparing one read with another.
fn last_updated_at(view: &Value) -> f64 {
    let value = view["LastUpdatedAt"].clone();
    value
        .as_f64()
        .unwrap_or_else(|| panic!("a View carries a numeric LastUpdatedAt, got {value}"))
}

/// The two members of a view an update rewrites, as one comparable value.
fn mutable_members(view: &Value) -> (Option<String>, Vec<String>) {
    (filter_string(view), property_names(view))
}

/// The members of a view that an update can NEVER change.
fn identity(view: &Value) -> Value {
    json!({
        "ViewArn": view["ViewArn"],
        "ViewName": view["ViewName"],
        "Owner": view["Owner"],
        "Scope": view["Scope"],
    })
}

/// One row of the replacement table: what the view was created with, what the update
/// carries, and the filter string and property names the view must hold afterwards.
struct Row {
    case: &'static str,
    region: &'static str,
    create: Value,
    update: Value,
    filter: Option<String>,
    properties: &'static [&'static str],
}

/// An update carries the whole new value of the two members a view can change, so a
/// member the request omits leaves the view without one — and `GetView` reads back
/// exactly the structure the update answered with.
///
/// The rows differ along the members the request carries (each alone, both, neither),
/// the form of the filter string (one term, several terms with a negation, the empty
/// string, a filter at the published maximum length), and the casing of a property name,
/// which Resource Explorer compares case-insensitively.
#[tokio::test]
async fn update_view_replaces_the_members_a_view_can_change() {
    // A filter exactly at the published maximum, built from a filter prefix so the
    // string stays inside the documented syntax.
    let longest_filter = format!("service:{}", "a".repeat(FILTER_STRING_MAX - "service:".len()));
    let rows = vec![
        Row {
            case: "a filter alone leaves no included property",
            region: "us-east-1",
            create: json!({
                "ViewName": "update-filter-only",
                "Filters": { "FilterString": "region:us-east-1" },
                "IncludedProperties": [{ "Name": "tags" }],
            }),
            update: json!({ "Filters": { "FilterString": "service:ec2" } }),
            filter: Some("service:ec2".to_string()),
            properties: &[],
        },
        Row {
            case: "an included property alone leaves no filter",
            region: "us-west-2",
            create: json!({
                "ViewName": "update-properties-only",
                "Filters": { "FilterString": "service:s3" },
            }),
            update: json!({ "IncludedProperties": [{ "Name": "tags" }] }),
            filter: None,
            properties: &["tags"],
        },
        Row {
            case: "both members at once",
            region: "eu-west-1",
            create: json!({ "ViewName": "update-both" }),
            update: json!({
                "Filters": { "FilterString": "region:eu-west-1 -resourcetype:resource-explorer-2:index" },
                "IncludedProperties": [{ "Name": "tags" }],
            }),
            filter: Some("region:eu-west-1 -resourcetype:resource-explorer-2:index".to_string()),
            properties: &["tags"],
        },
        Row {
            case: "neither member clears both",
            region: "ap-southeast-2",
            create: json!({
                "ViewName": "update-neither",
                "Filters": { "FilterString": "service:lambda" },
                "IncludedProperties": [{ "Name": "tags" }],
            }),
            update: json!({}),
            filter: None,
            properties: &[],
        },
        Row {
            case: "the shortest filter string the bound allows",
            region: "us-east-2",
            create: json!({
                "ViewName": "update-empty-filter",
                "Filters": { "FilterString": "service:sqs" },
            }),
            update: json!({ "Filters": { "FilterString": "x".repeat(FILTER_STRING_MIN) } }),
            filter: Some(String::new()),
            properties: &[],
        },
        Row {
            case: "the longest filter string the bound allows",
            region: "ca-central-1",
            create: json!({ "ViewName": "update-longest-filter" }),
            update: json!({ "Filters": { "FilterString": longest_filter.clone() } }),
            filter: Some(longest_filter.clone()),
            properties: &[],
        },
        Row {
            case: "a property name is accepted whatever its casing",
            region: "eu-central-1",
            create: json!({ "ViewName": "update-mixed-case-property" }),
            update: json!({ "IncludedProperties": [{ "Name": "TAGS" }] }),
            filter: None,
            properties: &["TAGS"],
        },
    ];

    for row in rows {
        let Row {
            case,
            region,
            create,
            update,
            filter,
            properties,
        } = row;
        let emulator = Emulator::new();
        create_index(&emulator, region).await;
        let created = create_view(&emulator, region, create).await;
        let arn = view_arn(&created);

        let mut request = update.clone();
        request["ViewArn"] = json!(arn);
        let answer = emulator.update_view(region, request.clone()).await;
        let updated = answer.ok(case)["View"].clone();

        let expected_filter = filter;
        let expected: Vec<String> = properties.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            mutable_members(&updated),
            (expected_filter.clone(), expected.clone()),
            "case {case:?}: the update answers the members it was given, got {updated}"
        );

        // The read-back is the proof the update was PERSISTED rather than echoed.
        let read_back = emulator.get_view(region, &arn, case).await;
        assert_eq!(
            read_back, updated,
            "case {case:?}: GetView must read back the structure UpdateView answered"
        );

        // A view's identity is not a thing an update can change.
        assert_eq!(
            identity(&read_back),
            identity(&created),
            "case {case:?}: an update leaves the view's identity alone, got {read_back}"
        );

        // "The date and time when this view was last modified" moves forward with the
        // write, never backwards.
        let (created_at, updated_at) = (last_updated_at(&created), last_updated_at(&read_back));
        assert!(
            updated_at >= created_at,
            "case {case:?}: LastUpdatedAt must not go backwards, got {created_at:?} then \
             {updated_at:?}"
        );

        // Repeating the identical update is the same end state, so an update is safe to
        // retry.
        let again = emulator.update_view(region, request).await;
        let repeated = again.ok(case)["View"].clone();
        assert_eq!(
            mutable_members(&repeated),
            (expected_filter, expected),
            "case {case:?}: repeating the update changes nothing, got {repeated}"
        );
        assert_eq!(
            identity(&repeated),
            identity(&created),
            "case {case:?}: repeating the update leaves the identity alone, got {repeated}"
        );
    }
}

/// A request the service cannot satisfy is refused with the modelled HTTP 400
/// `ValidationException`, and the stored view is left exactly as it was.
///
/// The rows span the kinds of refusal: an ARN that names no view (in this service's own
/// namespace, in another service's, and syntactically unlike an ARN at all), a property
/// name outside the documented catalog, a filter string one past its published maximum,
/// and a request with no `ViewArn` at all. The same catalog is then shown to bound
/// `CreateView`, which writes the same member of the same stored view.
#[tokio::test]
async fn update_view_refuses_what_it_cannot_satisfy() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region).await;
    let created = create_view(
        &emulator,
        region,
        json!({
            "ViewName": "update-refusals",
            "Filters": { "FilterString": "service:ec2" },
            "IncludedProperties": [{ "Name": "tags" }],
        }),
    )
    .await;
    let arn = view_arn(&created);

    let unknown = engine::emulator_arn(
        "resource-explorer-2",
        region,
        "view/no-such-view/00000000-0000-0000-0000-000000000000",
    );
    let foreign = engine::emulator_arn("sns", region, "a-topic");
    let rows: Vec<(&str, Value)> = vec![
        (
            "an own-namespace ARN that names no view",
            json!({ "ViewArn": unknown }),
        ),
        (
            "another service's resource ARN",
            json!({ "ViewArn": foreign }),
        ),
        (
            "text that is not an ARN at all",
            json!({ "ViewArn": "not-an-arn" }),
        ),
        (
            "a property name outside the catalog",
            json!({ "ViewArn": arn, "IncludedProperties": [{ "Name": "arn" }] }),
        ),
        (
            "a tag filter name is not a property name",
            json!({ "ViewArn": arn, "IncludedProperties": [{ "Name": "tag.key" }] }),
        ),
        (
            "one supported and one unsupported property",
            json!({
                "ViewArn": arn,
                "IncludedProperties": [{ "Name": "tags" }, { "Name": "resourcetype" }],
            }),
        ),
        (
            "a filter string one past the published maximum",
            json!({
                "ViewArn": arn,
                "Filters": { "FilterString": "z".repeat(FILTER_STRING_MAX + 1) },
            }),
        ),
        ("no ViewArn at all", json!({})),
    ];

    for (case, request) in rows {
        emulator
            .update_view(region, request)
            .await
            .failed(case, 400, "ValidationException");
        assert_eq!(
            emulator.get_view(region, &arn, case).await,
            created,
            "case {case:?}: a refused update writes nothing"
        );
    }

    // The refusal names the value at fault and the values the catalog does accept.
    let answer = emulator
        .update_view(
            region,
            json!({ "ViewArn": arn, "IncludedProperties": [{ "Name": "resourcetype" }] }),
        )
        .await;
    answer.failed("an unsupported property name", 400, "ValidationException");
    assert_eq!(
        answer.error_message(),
        format!(
            "resourcetype is not a supported value for IncludedProperties: the supported \
             values are {}.",
            VIEW_PROPERTY_NAMES.join(", ")
        ),
        "case \"an unsupported property name\": the refusal names the value and the catalog"
    );

    // The catalog's own entry is accepted, so the refusals above are about the value and
    // not about the member.
    for name in VIEW_PROPERTY_NAMES {
        let answer = emulator
            .update_view(
                region,
                json!({ "ViewArn": arn, "IncludedProperties": [{ "Name": name }] }),
            )
            .await;
        let view = answer.ok(name)["View"].clone();
        assert_eq!(
            property_names(&view),
            vec![name.to_string()],
            "case {name:?}: a documented property name is accepted, got {view}"
        );
    }

    // The same catalog bounds a view at CREATE time, worded the same way, so a view can
    // never be brought into existence holding a property no reader can report.
    let answer = emulator
        .call(
            region,
            "CreateView",
            json!({
                "ViewName": "create-unsupported-property",
                "IncludedProperties": [{ "Name": "arn" }],
            }),
        )
        .await;
    answer.failed("CreateView with an unsupported property", 400, "ValidationException");
    assert_eq!(
        answer.error_message(),
        format!(
            "arn is not a supported value for IncludedProperties: the supported values \
             are {}.",
            VIEW_PROPERTY_NAMES.join(", ")
        ),
        "case \"CreateView with an unsupported property\": the refusal names the value \
         and the catalog"
    );
}

/// A search that filters on TAGS is refused unless the view it uses includes the `tags`
/// property — and that property is exactly what `UpdateView` adds and removes.
///
/// The rows span the three documented tag filter names, their negated and wildcard
/// forms, and the casing of the filter name; each is paired with a non-tag filter that
/// the same view still answers, so the refusal is shown to be about the tag filter and
/// not about the view.
#[tokio::test]
async fn a_tag_filtered_search_needs_a_view_that_includes_tags() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    create_index(&emulator, region).await;
    let view = create_view(
        &emulator,
        region,
        json!({
            "ViewName": "tag-filtered",
            "Tags": { "env": "prod" },
            "IncludedProperties": [{ "Name": "tags" }],
        }),
    )
    .await;
    let arn = view_arn(&view);

    // While the view includes tags, every tag filter is answered.
    let tag_filters = [
        "tag:env=prod",
        "tag.key:env",
        "tag.value:prod",
        "tag.key:*",
        "-tag:env=dev",
        "TAG.KEY:env",
    ];
    for query in tag_filters {
        let case = format!("{query:?} with the tags property");
        let answer = emulator
            .call(region, "Search", json!({ "QueryString": query, "ViewArn": arn }))
            .await;
        let body = answer.ok(&case).clone();
        let found = body["Resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .any(|resource| resource["Arn"].as_str() == Some(arn.as_str()));
        assert!(
            found,
            "case {case:?}: the tagged view must match its own tag filter, got {body}"
        );
    }

    // Removing the property is an update like any other, and now the same searches are
    // refused — each naming the filter at fault.
    emulator
        .update_view(region, json!({ "ViewArn": arn }))
        .await
        .ok("removing the tags property");
    assert_eq!(
        property_names(&emulator.get_view(region, &arn, "after removal").await),
        Vec::<String>::new(),
        "the update removed the included property"
    );
    for query in tag_filters {
        let case = format!("{query:?} without the tags property");
        let answer = emulator
            .call(region, "Search", json!({ "QueryString": query, "ViewArn": arn }))
            .await;
        answer.failed(&case, 400, "ValidationException");
        let filter = query
            .trim_start_matches('-')
            .split_once(':')
            .map(|(name, _)| name)
            .expect("each row is a filter term");
        assert_eq!(
            answer.error_message(),
            format!(
                "The specified filter {filter} is not valid because it uses properties \
                 that are not included in the view."
            ),
            "case {case:?}: the refusal names the filter at fault"
        );
    }

    // A query that filters on anything else is still answered through the same view.
    for query in ["", "service:resource-explorer-2", "tag-filtered"] {
        let case = format!("{query:?} filters on no property");
        emulator
            .call(region, "Search", json!({ "QueryString": query, "ViewArn": arn }))
            .await
            .ok(&case);
    }

    // The same rule holds for a tag filter the VIEW carries rather than the caller: the
    // view's filter string is ANDed into every search made through it.
    emulator
        .update_view(
            region,
            json!({ "ViewArn": arn, "Filters": { "FilterString": "tag:env=prod" } }),
        )
        .await
        .ok("a tag filter on the view");
    let answer = emulator
        .call(region, "Search", json!({ "QueryString": "", "ViewArn": arn }))
        .await;
    answer.failed("the view's own tag filter", 400, "ValidationException");
    assert_eq!(
        answer.error_message(),
        "The specified filter tag is not valid because it uses properties that are not \
         included in the view."
            .to_string(),
        "case \"the view's own tag filter\": the refusal names the filter at fault"
    );

    // Putting the property back makes the same view searchable again, and the matched
    // resource now carries its tags as the optional property the view asked for.
    emulator
        .update_view(
            region,
            json!({
                "ViewArn": arn,
                "Filters": { "FilterString": "tag:env=prod" },
                "IncludedProperties": [{ "Name": "tags" }],
            }),
        )
        .await
        .ok("restoring the tags property");
    let answer = emulator
        .call(region, "Search", json!({ "QueryString": "", "ViewArn": arn }))
        .await;
    let body = answer.ok("the view's own tag filter, with tags included").clone();
    let resource = body["Resources"]
        .as_array()
        .and_then(|resources| resources.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        resource["Arn"].as_str(),
        Some(arn.as_str()),
        "the view's tag filter matches the tagged view itself, got {body}"
    );
    assert_eq!(
        resource["Properties"][0]["Name"].as_str(),
        Some("tags"),
        "a view that includes tags reports them as a property, got {resource}"
    );
}
