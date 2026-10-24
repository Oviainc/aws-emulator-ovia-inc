// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListLayerVersions`: the versions one layer reports, the order
//! they come back in, the two compatibility filters, the pagination walk and what a
//! deleted version (or a layer of another Region) leaves behind.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than any handler's internals. Everything a
//! listing reports is checked against what `PublishLayerVersion` persisted and what
//! `GetLayerVersion` reads back, so no suite trusts the listing alone.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region a suite that is not about Regions signs every request for.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The largest `MaxItems` the model admits (`MaxLayerListItems`, `@range(min: 1, max:
/// 50)`).
const MAX_ITEMS_MAX: usize = 50;

/// A SigV4 credential scope naming `lambda` in `region` — the router picks a service by
/// its service field and the handler reads the Region from it. Neither the key id nor
/// the signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one JSON request signed for `region`.
async fn json_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let host = format!("lambda.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Layer archive bytes of a given length. The content is irrelevant to the API — only
/// its length and its digest are reported — but it differs per length so two archives
/// never share a digest.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Content.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// A `Content` member holding a small inline archive, for a version published only to
/// exist.
fn inline_content(length: usize) -> Value {
    json!({ "ZipFile": zip_file(&archive_bytes(length)) })
}

/// `PublishLayerVersion` in `region`, answering the version it stored. Panics unless the
/// publish succeeded, because every assertion below is about what it stored.
async fn publish(router: &MultiServiceRouter, region: &str, name: &str, body: &Value) -> Value {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    let answer = json_call(router, region, "POST", &uri, body).await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: expected 201 from PublishLayerVersion, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// `DeleteLayerVersion` in `region`, which answers 204.
async fn delete_version(router: &MultiServiceRouter, region: &str, name: &str, version: i64) {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    let answer = json_call(router, region, "DELETE", &uri, &Value::Null).await;
    assert_eq!(
        answer.status,
        204,
        "case {name:?}:{version}: expected 204 from DeleteLayerVersion, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `GetLayerVersion` in `region` (`smithy_query`: `GET
/// /2018-10-31/layers/{LayerName}/versions/{VersionNumber}`).
async fn get_layer_version(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: i64,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, region, "GET", &uri, &Value::Null).await
}

/// `ListLayerVersions` in `region` for `reference` (a layer name or a layer ARN) with a
/// query string (`""` for none) — `smithy_query`: `GET
/// /2018-10-31/layers/{LayerName}/versions`.
async fn list_versions(
    router: &MultiServiceRouter,
    region: &str,
    reference: &str,
    query: &str,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{reference}/versions{query}");
    json_call(router, region, "GET", &uri, &Value::Null).await
}

/// The `LayerVersions` a successful `ListLayerVersions` reported, in the order it
/// reported them. Panics unless the call answered 200 with the member present.
async fn listed_versions(
    router: &MultiServiceRouter,
    region: &str,
    reference: &str,
    query: &str,
) -> Vec<Value> {
    let answer = list_versions(router, region, reference, query).await;
    assert_eq!(
        answer.status,
        200,
        "case {reference:?}{query:?} in {region}: expected 200 from ListLayerVersions, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    body.get("LayerVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {reference:?}{query:?}: expected a LayerVersions array, got {}",
                answer.text()
            )
        })
}

/// The `Version` numbers a successful `ListLayerVersions` reported, in the order it
/// reported them.
async fn listed_numbers(
    router: &MultiServiceRouter,
    region: &str,
    reference: &str,
    query: &str,
) -> Vec<i64> {
    listed_versions(router, region, reference, query)
        .await
        .iter()
        .map(|item| {
            item.get("Version").and_then(Value::as_i64).unwrap_or_else(|| {
                panic!("case {reference:?}{query:?}: a listed version must carry a Version, got {item:?}")
            })
        })
        .collect()
}

/// The ARN a layer of `name` published in `region` must carry.
fn layer_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:{name}")
}

/// Assert one member of a listed entry: `None` means the member must be absent from the
/// answer altogether, not present and empty.
fn assert_member(case: &str, body: &Value, member: &str, expected: Option<Value>) {
    let actual = body.get(member);
    match expected {
        None => assert!(
            actual.is_none(),
            "case {case:?}: expected no {member} member, got {actual:?}"
        ),
        Some(value) => assert_eq!(
            actual,
            Some(&value),
            "case {case:?}: expected {member} {value:?}, got {actual:?}"
        ),
    }
}

/// One row of the listing table: a layer and the `PublishLayerVersion` bodies to store
/// under it, oldest first.
struct ListCase {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// One `PublishLayerVersion` body per version, oldest first.
    versions: Vec<Value>,
}

/// `ListLayerVersions` reports one entry per version the layer holds, newest first, and
/// every member of an entry is what the publish of that version persisted.
///
/// The rows differ along what the rule generalises over: how many versions the layer
/// holds (one, two, four, and more than one page's worth is covered by the pagination
/// suite below), and which optional members each version carries — none at all, each of
/// them, the published maximum of 15 compatible runtimes and both architectures, and
/// versions whose metadata differs from their neighbours' so an entry is never another
/// version's metadata. The runtime and architecture values are entries of the model's own
/// `Runtime` and `Architecture` enums
/// (`smithy_query(query="shape",shape="Runtime,Architecture")`).
#[tokio::test]
async fn list_layer_versions_reports_every_version_newest_first() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Fifteen entries: the `CompatibleRuntimes` length maximum the model publishes
    // (`@length(min: 0, max: 15)`), spelled with values from the `Runtime` enum.
    let fifteen_runtimes = json!([
        "nodejs18.x",
        "nodejs20.x",
        "nodejs22.x",
        "python3.9",
        "python3.10",
        "python3.11",
        "python3.12",
        "python3.13",
        "java11",
        "java17",
        "java21",
        "ruby3.2",
        "ruby3.3",
        "dotnet8",
        "provided.al2023",
    ]);

    let cases = vec![
        // The plain case: one version carrying no optional member at all.
        ListCase {
            name: "single-bare",
            versions: vec![json!({ "Content": inline_content(32) })],
        },
        // Two versions whose metadata differs: the older one described, the newer bare.
        ListCase {
            name: "two-mixed",
            versions: vec![
                json!({
                    "Content": inline_content(64),
                    "Description": "the version below",
                    "LicenseInfo": "Apache-2.0",
                    "CompatibleRuntimes": ["nodejs20.x"],
                    "CompatibleArchitectures": ["x86_64"],
                }),
                json!({ "Content": inline_content(96) }),
            ],
        },
        // Four versions, each carrying its own metadata, including the list bounds and
        // the license given as the URL form the member's documentation names.
        ListCase {
            name: "four-described",
            versions: vec![
                json!({
                    "Content": inline_content(128),
                    "Description": "first",
                    "CompatibleRuntimes": ["python3.12"],
                }),
                json!({
                    "Content": inline_content(160),
                    "Description": "",
                    "LicenseInfo": "MIT",
                    "CompatibleArchitectures": ["arm64"],
                }),
                json!({
                    "Content": inline_content(192),
                    "Description": "third",
                    "LicenseInfo": "https://opensource.org/license/mit",
                    "CompatibleRuntimes": fifteen_runtimes.clone(),
                    "CompatibleArchitectures": ["arm64", "x86_64"],
                }),
                json!({
                    "Content": inline_content(224),
                    "Description": "newest",
                    "LicenseInfo": "BSD-3-Clause",
                    "CompatibleRuntimes": ["java21"],
                    "CompatibleArchitectures": ["x86_64"],
                }),
            ],
        },
    ];

    // What each publish persisted, per layer: the version number it minted and the whole
    // response it answered — so the listing below is checked against stored state rather
    // than against values this test made up.
    let mut published: Vec<(&str, Vec<Value>)> = Vec::new();
    for case in &cases {
        let mut stored = Vec::new();
        for body in &case.versions {
            stored.push(publish(&router, REGION, case.name, body).await);
        }
        published.push((case.name, stored));
    }

    for (case, (name, stored)) in cases.iter().zip(published.iter()) {
        assert_eq!(case.name, *name, "the case order must match the publish order");
        let listed = listed_versions(&router, REGION, case.name, "").await;
        assert_eq!(
            listed.len(),
            stored.len(),
            "case {name:?}: expected one entry per published version ({}), got {listed:?}",
            stored.len()
        );

        // Newest first: the numbers come back descending, which is the reverse of the
        // order the versions were published in.
        let numbers = listed_numbers(&router, REGION, case.name, "").await;
        let expected_numbers: Vec<i64> = (1..=i64::try_from(stored.len()).unwrap_or_default())
            .rev()
            .collect();
        assert_eq!(
            numbers, expected_numbers,
            "case {name:?}: expected the versions newest first, got {numbers:?}"
        );

        // Each entry carries exactly what the publish of THAT version answered, and the
        // same values `GetLayerVersion` reads back.
        for (item, publish_response) in listed.iter().zip(stored.iter().rev()) {
            let version = publish_response
                .get("Version")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("case {name:?}: the publish must report a Version"));
            let case_name = format!("{name}:{version}");
            let read = get_layer_version(&router, REGION, case.name, version).await;
            assert_eq!(
                read.status,
                200,
                "case {case_name:?}: expected 200 from GetLayerVersion, got {} {}",
                read.status,
                read.text()
            );
            let read = read.json();

            assert_member(&case_name, item, "Version", Some(json!(version)));
            assert_member(
                &case_name,
                item,
                "LayerVersionArn",
                Some(json!(format!("{}:{version}", layer_arn(REGION, case.name)))),
            );
            for member in [
                "LayerVersionArn",
                "Description",
                "CreatedDate",
                "LicenseInfo",
                "CompatibleRuntimes",
                "CompatibleArchitectures",
            ] {
                assert_member(&case_name, item, member, publish_response.get(member).cloned());
                assert_member(&case_name, item, member, read.get(member).cloned());
            }
            // A listing entry is a summary: it carries no `Content`, which only the
            // single-version reads report (`LayerVersionsListItem` has no such member).
            assert_member(&case_name, item, "Content", None);
        }

        // One page holds every version here, so no pagination token is handed out.
        let body = list_versions(&router, REGION, case.name, "").await.json();
        assert_member(name, &body, "NextMarker", None);
    }
}

/// One row of the filter table: the query string and the version numbers it must report.
struct FilterCase {
    /// The case name.
    case: &'static str,
    /// The query string the listing is called with.
    query: &'static str,
    /// The version numbers the listing must report, newest first.
    expected: Vec<i64>,
}

/// The two compatibility filters select versions by what each version itself declares:
/// "Specify a runtime identifier to list only versions that indicate that they're
/// compatible with that runtime. Specify a compatible architecture to include only layer
/// versions that are compatible with that architecture."
///
/// The rows differ along which filter is applied and what it asks for: neither, each one
/// alone, both together, a runtime no version declares, a runtime only the oldest version
/// declares, and a value from the enum the other rows do not cite.
#[tokio::test]
async fn list_layer_versions_filters_on_each_versions_own_compatibility() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let name = "filtered";
    // Version 1 declares a runtime nothing else does; 2 declares an architecture only;
    // 3 declares both; 4 declares neither, so no filtered listing can report it.
    for body in [
        json!({
            "Content": inline_content(32),
            "CompatibleRuntimes": ["ruby3.3"],
            "CompatibleArchitectures": ["x86_64"],
        }),
        json!({
            "Content": inline_content(64),
            "CompatibleArchitectures": ["arm64"],
        }),
        json!({
            "Content": inline_content(96),
            "CompatibleRuntimes": ["python3.12", "python3.13"],
            "CompatibleArchitectures": ["arm64", "x86_64"],
        }),
        json!({ "Content": inline_content(128) }),
    ] {
        publish(&router, REGION, name, &body).await;
    }

    let cases = vec![
        FilterCase {
            case: "no filter",
            query: "",
            expected: vec![4, 3, 2, 1],
        },
        FilterCase {
            case: "runtime declared by the newest matching version",
            query: "?CompatibleRuntime=python3.12",
            expected: vec![3],
        },
        FilterCase {
            case: "runtime declared only by the oldest version",
            query: "?CompatibleRuntime=ruby3.3",
            expected: vec![1],
        },
        FilterCase {
            // `nodejs22.x` is an entry of the model's `Runtime` enum that no version
            // here declares, so it selects nothing.
            case: "runtime no version declares",
            query: "?CompatibleRuntime=nodejs22.x",
            expected: vec![],
        },
        FilterCase {
            case: "architecture arm64",
            query: "?CompatibleArchitecture=arm64",
            expected: vec![3, 2],
        },
        FilterCase {
            case: "architecture x86_64",
            query: "?CompatibleArchitecture=x86_64",
            expected: vec![3, 1],
        },
        FilterCase {
            case: "both filters, satisfied together",
            query: "?CompatibleRuntime=python3.13&CompatibleArchitecture=arm64",
            expected: vec![3],
        },
        FilterCase {
            // Version 1 declares the runtime and version 2 the architecture, but no
            // version declares both, so the pair selects nothing.
            case: "both filters, satisfied by different versions",
            query: "?CompatibleRuntime=ruby3.3&CompatibleArchitecture=arm64",
            expected: vec![],
        },
    ];

    for case in &cases {
        let numbers = listed_numbers(&router, REGION, name, case.query).await;
        assert_eq!(
            numbers, case.expected,
            "case {:?}: expected versions {:?}, got {numbers:?}",
            case.case, case.expected
        );
    }
}

/// A paged walk reports every version of the layer exactly once, newest to oldest, and
/// the last page carries no marker.
///
/// The rows differ along the page size: one version at a time, sizes that do not divide
/// the total, each published `MaxItems` bound (1 and 50) and a size larger than the
/// number of versions. A size outside the published range is refused rather than clamped,
/// and so is a marker that is no version number.
#[tokio::test]
async fn list_layer_versions_pages_newest_to_oldest() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let name = "paged";
    let total: i64 = 7;
    for index in 0..total {
        publish(
            &router,
            REGION,
            name,
            &json!({
                "Content": inline_content(32 + usize::try_from(index).unwrap_or(0)),
                "Description": format!("version {}", index + 1),
            }),
        )
        .await;
    }
    let expected: Vec<i64> = (1..=total).rev().collect();

    for page_size in [1_usize, 2, 3, 5, MAX_ITEMS_MAX] {
        let mut seen: Vec<i64> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_versions(&router, REGION, name, &query).await;
            assert_eq!(
                answer.status,
                200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
            let body = answer.json();
            let page = body
                .get("LayerVersions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case page size {page_size}: expected a LayerVersions array, got {}",
                        answer.text()
                    )
                });
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most {page_size} versions, got {}",
                page.len()
            );
            for item in &page {
                let version = item.get("Version").and_then(Value::as_i64).unwrap_or_else(|| {
                    panic!("case page size {page_size}: a listed version must carry a Version, got {item:?}")
                });
                seen.push(version);
            }
            match body.get("NextMarker").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case page size {page_size}: a marker was returned for an empty page"
                    );
                    marker = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every version once, newest first, got {seen:?}"
        );
    }

    // `MaxItems` is modelled `@range(min: 1, max: 50)`: a page size outside it is
    // refused rather than paged over.
    for (case, max_items) in [("zero", "0"), ("negative", "-1"), ("past the maximum", "51")] {
        let answer = list_versions(&router, REGION, name, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {case}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // A marker names the place in the order the previous page stopped at, so a value
    // that is no version number names no place at all.
    for (case, marker) in [
        ("not a number", "not-a-version"),
        ("empty", ""),
        ("fractional", "2.5"),
    ] {
        let answer = list_versions(&router, REGION, name, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status,
            400,
            "case Marker {case}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // A marker past the newest version reports every version; one at the oldest reports
    // nothing after it, and neither hands out a further token.
    for (case, marker, expected_page) in [
        ("above the newest", total + 1, expected.clone()),
        ("at the oldest", 1, Vec::new()),
    ] {
        let answer = list_versions(&router, REGION, name, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status,
            200,
            "case Marker {case}: expected 200, got {} {}",
            answer.status,
            answer.text()
        );
        let numbers = listed_numbers(&router, REGION, name, &format!("?Marker={marker}")).await;
        assert_eq!(
            numbers, expected_page,
            "case Marker {case}: expected versions {expected_page:?}, got {numbers:?}"
        );
        assert_member(case, &answer.json(), "NextMarker", None);
    }
}

/// One row of the stored-state table: what a layer holds and what the listing therefore
/// reports for it.
struct StoredCase {
    /// The case name, which is also the layer name.
    case: &'static str,
    /// How many versions are published under the name.
    published: i64,
    /// The versions deleted afterwards, in order.
    deleted: Vec<i64>,
    /// The version numbers the listing must report, newest first.
    expected: Vec<i64>,
}

/// The listing reports the versions the store still holds: "Versions that have been
/// deleted aren't listed", and a name nothing was ever published under holds none.
///
/// The rows differ along which versions a layer holds when it is listed: every one of
/// them, the newest deleted, one in the middle deleted, all of them deleted, and a name
/// nobody published under at all. A deleted number is not reused, so the numbers the
/// listing reports stay the ones the publishes minted.
#[tokio::test]
async fn list_layer_versions_reports_only_the_versions_the_store_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases = vec![
        StoredCase {
            case: "nothing-deleted",
            published: 3,
            deleted: Vec::new(),
            expected: vec![3, 2, 1],
        },
        StoredCase {
            case: "newest-deleted",
            published: 3,
            deleted: vec![3],
            expected: vec![2, 1],
        },
        StoredCase {
            case: "middle-deleted",
            published: 4,
            deleted: vec![2],
            expected: vec![4, 3, 1],
        },
        StoredCase {
            case: "every-version-deleted",
            published: 2,
            deleted: vec![1, 2],
            expected: Vec::new(),
        },
        StoredCase {
            case: "never-published",
            published: 0,
            deleted: Vec::new(),
            expected: Vec::new(),
        },
    ];

    for case in &cases {
        for index in 0..case.published {
            publish(
                &router,
                REGION,
                case.case,
                &json!({ "Content": inline_content(48 + usize::try_from(index).unwrap_or(0)) }),
            )
            .await;
        }
        for version in &case.deleted {
            delete_version(&router, REGION, case.case, *version).await;
        }

        let numbers = listed_numbers(&router, REGION, case.case, "").await;
        assert_eq!(
            numbers, case.expected,
            "case {:?}: expected versions {:?}, got {numbers:?}",
            case.case, case.expected
        );

        // A version the listing reports is one `GetLayerVersion` still reads, and a
        // version it leaves out is one that read answers 404 for — the listing and the
        // single-version read answer from the same stored record.
        for version in 1..=case.published {
            let read = get_layer_version(&router, REGION, case.case, version).await;
            let expected_status = if case.expected.contains(&version) { 200 } else { 404 };
            assert_eq!(
                read.status,
                expected_status,
                "case {:?}:{version}: expected {expected_status} from GetLayerVersion, got {} {}",
                case.case,
                read.status,
                read.text()
            );
        }

        // A publish after the deletes keeps counting from the highest number ever
        // published, so the listing's newest entry is that number rather than the count
        // of what the layer holds.
        let next = publish(
            &router,
            REGION,
            case.case,
            &json!({ "Content": inline_content(300) }),
        )
        .await;
        let minted = next
            .get("Version")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: the publish must report a Version", case.case));
        assert_eq!(
            minted,
            case.published + 1,
            "case {:?}: expected the next publish to mint version {}, got {minted}",
            case.case,
            case.published + 1
        );
        let numbers = listed_numbers(&router, REGION, case.case, "").await;
        assert_eq!(
            numbers.first().copied(),
            Some(minted),
            "case {:?}: expected the newly published version first, got {numbers:?}",
            case.case
        );
    }

    // A `LayerName` that is well formed for the member's pattern but names a Lambda
    // resource of another kind names no layer, so the call is refused rather than
    // reported as a listing of nothing.
    let refused = list_versions(
        &router,
        REGION,
        "arn:aws:lambda:us-east-1:000000000000:function:not-a-layer",
        "",
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "case \"not a layer name\": expected 400 from ListLayerVersions, got {} {}",
        refused.status,
        refused.text()
    );
}

/// A layer belongs to the Region it was published in, and both the bare name and the
/// layer ARN name the same record.
///
/// The rows differ along the Region the layer was published in — four of them, across
/// three partitions' worth of name shapes — and each listing is checked from its own
/// Region (by name and by ARN) and from every other Region.
#[tokio::test]
async fn a_layers_versions_are_invisible_from_another_region() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // (the Region the layer is published in, the layer's name)
    let cases = [
        ("us-east-1", "versions-use1"),
        ("us-west-2", "versions-usw2"),
        ("eu-central-1", "versions-euc1"),
        ("ap-southeast-2", "versions-apse2"),
    ];
    for (region, name) in cases {
        for length in [32_usize, 64] {
            publish(
                &router,
                region,
                name,
                &json!({
                    "Content": inline_content(length + name.len()),
                    "CompatibleRuntimes": ["python3.12"],
                }),
            )
            .await;
        }
    }

    for (region, name) in cases {
        let numbers = listed_numbers(&router, region, name, "").await;
        assert_eq!(
            numbers,
            vec![2, 1],
            "case {region}: expected both versions of {name:?} newest first, got {numbers:?}"
        );

        // The ARN form of the `LayerName` names the same record as the bare name.
        let by_arn = listed_versions(&router, region, &layer_arn(region, name), "").await;
        let by_name = listed_versions(&router, region, name, "").await;
        assert_eq!(
            by_arn, by_name,
            "case {region}: expected the layer ARN to list the same versions as {name:?}"
        );

        for (other, _) in cases.iter().filter(|(other, _)| *other != region) {
            let elsewhere = listed_numbers(&router, other, name, "").await;
            assert!(
                elsewhere.is_empty(),
                "case {region} from {other}: expected no versions of {name:?}, got {elsewhere:?}"
            );
            // A filter every version satisfies does not make another Region's layer
            // visible either.
            let filtered =
                listed_numbers(&router, other, name, "?CompatibleRuntime=python3.12").await;
            assert!(
                filtered.is_empty(),
                "case {region} from {other}: expected no versions of {name:?} under a runtime \
                 filter, got {filtered:?}"
            );
        }
    }
}
