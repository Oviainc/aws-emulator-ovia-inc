// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListComponents` and `ListComponentBuildVersions`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a component listing reports one entry per stored component
//! VERSION, under the version ARN of that version's three assigned nodes and read from the
//! latest build version stored under it; a listing that asks for components by name
//! instead reports one entry per NAME, under the name ARN whose version nodes are
//! wildcards and read from the latest version of that name. Either listing reports only
//! the components of the Region whose endpoint served it, selects on the filter names the
//! operation documents, and pages through what it selected under a token of its own.
//!
//! The rows below therefore differ along what that rule generalises over: how many build
//! versions a version holds, how many versions a name holds, the Region, which member a
//! filter names, how many values it carries, whether the version it names is exact or
//! carries wildcard nodes, and the page size the request asks for.
//!
//! `ListComponentBuildVersions` is the listing BELOW that one: it reports one entry per
//! stored BUILD version of the component version its ARN names — of every component the
//! account owns in the calling Region when it names none — so the two listings are tested
//! against the same stored components and differ only in what one entry stands for.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, so a listing is shown to report only the components of the Region
/// whose endpoint served it.
const OTHER_REGION: &str = "us-east-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The version nodes a NAME ARN carries, from the note on `ComponentVersion`'s `arn`:
/// "Versionless ARNs and Name ARNs do not include specific values in any of the nodes. The
/// nodes are either left off entirely, or they are specified as wildcards, for example:
/// x.x.x."
const WILDCARD_VERSION: &str = "x.x.x";

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// A test component document: a single phase named `test`, so the derived type is `TEST`.
const TEST_DOCUMENT: &str = "name: CheckWorld\nschemaVersion: 1.0\nphases:\n  - name: test\n    steps:\n      - name: Check\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo ok\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on
/// and the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `CreateComponent` with `body`, in `region`.
async fn create_component_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateComponent", Some(body)).await
}

/// `GetComponent` for `arn`, in [`REGION`].
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `ListComponents` with `body`, in `region`.
async fn list_components_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "POST", "/ListComponents", Some(body)).await
}

/// `ListComponents` with `body`, in [`REGION`].
async fn list_components(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    list_components_in(router, REGION, body).await
}

/// A `CreateComponent` body: the required members, `clientToken`, and whatever else
/// `extra` names.
fn component_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

/// The ARN a listing entry carries for the component `normalized` at `version` in
/// `region`: no build number, since an entry stands for a version or for a name.
fn listed_arn(region: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:component/{normalized}/{version}")
}

/// The ARN one BUILD version of the component `normalized` carries in `region`.
fn build_version_arn(region: &str, normalized: &str, version: &str, build: u32) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:component/{normalized}/{version}/{build}")
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// The entries of one listing page.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    body.get("componentVersionList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a componentVersionList in {body}"))
        .clone()
}

/// The ARNs one listing page reports, in the order it reported them.
fn page_arns(case: &str, body: &Value) -> Vec<String> {
    entries(case, body)
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {entry}"))
                .to_string()
        })
        .collect()
}

/// The entry of a listing whose ARN is `arn`, or a panic naming the case.
fn entry_at(case: &str, body: &Value, arn: &str) -> Value {
    entries(case, body)
        .into_iter()
        .find(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {arn} in {body}"))
}

/// Assert one member of an entry.
fn assert_member(case: &str, entry: &Value, name: &str, expected: &Value) {
    let actual = entry.get(name);
    assert_eq!(
        actual,
        Some(expected),
        "case {case:?}: expected {name} to be {expected}, got {actual:?}"
    );
}

/// Assert an entry carries an instant in the form this model's `DateTime` members use.
fn assert_instant(case: &str, entry: &Value, name: &str) {
    let instant = entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {entry}"))
        .to_string();
    assert!(
        instant.contains('T') && instant.ends_with('Z') && instant.len() >= 20,
        "case {case:?}: expected {name} to be an ISO 8601 instant, got {instant:?}"
    );
}

/// The components every case lists: three versions of one name — the first of them stored
/// twice, so one version holds two build versions — a second name of another platform,
/// type and supported OS version, and one component of the other Region.
async fn stored_components(router: &MultiServiceRouter) {
    let created: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "alpha-component",
            "1.0.0",
            "token-alpha-100-build-1",
            REGION,
            json!({"description": "the first build"}),
        ),
        (
            "alpha-component",
            "1.0.0",
            "token-alpha-100-build-2",
            REGION,
            json!({"description": "the second build"}),
        ),
        (
            "alpha-component",
            "1.1.0",
            "token-alpha-110",
            REGION,
            json!({"description": "the minor line"}),
        ),
        (
            "alpha-component",
            "2.0.0",
            "token-alpha-200",
            REGION,
            json!({
                "description": "the next major",
                "supportedOsVersions": ["Amazon Linux 2"],
            }),
        ),
        (
            "Beta Component",
            "1.0.0",
            "token-beta-100",
            REGION,
            json!({
                "description": "checks the image",
                "platform": "Windows",
                "data": TEST_DOCUMENT,
                "supportedOsVersions": ["Microsoft Windows Server 2019", "Microsoft Windows Server 2022"],
            }),
        ),
        (
            "gamma-component",
            "1.0.0",
            "token-gamma-100",
            OTHER_REGION,
            json!({"description": "another Region's own"}),
        ),
    ];
    for (name, version, token, region, extra) in created {
        let body = component_body(name, version, token, extra);
        let (status, created) = create_component_in(router, region, &body).await;
        assert_eq!(
            status, 200,
            "case creating {name} {version} in {region}: expected HTTP 200, got {status} with \
             {created}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_one_entry_per_version_under_its_version_arn() {
    let router = emulator();
    stored_components(&router).await;

    // One entry per stored VERSION, in the one order the listing has, and no token: with
    // no page size asked for, nothing remains.
    let case = "the whole listing";
    let (status, listed) = list_components(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {listed}"
    );
    let expected = vec![
        listed_arn(REGION, "alpha-component", "1.0.0"),
        listed_arn(REGION, "alpha-component", "1.1.0"),
        listed_arn(REGION, "alpha-component", "2.0.0"),
        listed_arn(REGION, "beta-component", "1.0.0"),
    ];
    let arns = page_arns(case, &listed);
    assert_eq!(
        arns, expected,
        "case {case:?}: expected {expected:?}, got {arns:?}"
    );
    assert_eq!(
        listed.get("nextToken"),
        None,
        "case {case:?}: expected no nextToken, got {listed}"
    );
    assert!(
        listed
            .get("requestId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "case {case:?}: expected a requestId, got {listed}"
    );

    // The entry for the version two build versions are stored under reports the LATEST of
    // them, and reports the version itself rather than a build number.
    let alpha = entry_at(case, &listed, &expected[0]);
    assert_member(case, &alpha, "name", &json!("alpha-component"));
    assert_member(case, &alpha, "version", &json!("1.0.0"));
    assert_member(case, &alpha, "description", &json!("the second build"));
    assert_member(case, &alpha, "platform", &json!("Linux"));
    assert_member(case, &alpha, "type", &json!("BUILD"));
    assert_member(case, &alpha, "status", &json!("ACTIVE"));
    assert_member(case, &alpha, "owner", &json!(ACCOUNT));
    assert_instant(case, &alpha, "dateCreated");

    // The second name's own members, which differ from the first's along every one the
    // create named.
    let beta = entry_at(case, &listed, &expected[3]);
    assert_member(case, &beta, "name", &json!("Beta Component"));
    assert_member(case, &beta, "version", &json!("1.0.0"));
    assert_member(case, &beta, "platform", &json!("Windows"));
    assert_member(case, &beta, "type", &json!("TEST"));
    assert_member(
        case,
        &beta,
        "supportedOsVersions",
        &json!(["Microsoft Windows Server 2019", "Microsoft Windows Server 2022"]),
    );

    // The ARN an entry reports is one this service RESOLVES: a read of it answers the
    // latest build version stored under that version.
    let (status, read) = get_component(&router, &expected[0]).await;
    assert_eq!(
        status, 200,
        "case reading back the version ARN: expected HTTP 200, got {status} with {read}"
    );
    let reported = read
        .get("component")
        .cloned()
        .unwrap_or_else(|| panic!("case reading back the version ARN: expected a component in {read}"));
    assert_member(
        "reading back the version ARN",
        &reported,
        "arn",
        &json!(build_version_arn(REGION, "alpha-component", "1.0.0", 2)),
    );

    // A listing is a READ: the entries it reports do not change what a later caller sees.
    let (_, again) = list_components(&router, &json!({})).await;
    let repeated = page_arns("the repeated listing", &again);
    assert_eq!(
        repeated, expected,
        "case the repeated listing: expected {expected:?}, got {repeated:?}"
    );
}

#[tokio::test]
async fn a_listing_by_name_reports_one_wildcard_entry_per_name() {
    let router = emulator();
    stored_components(&router).await;

    let case = "the listing by name";
    let (status, listed) = list_components(&router, &json!({"byName": true})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {listed}"
    );
    let expected = vec![
        listed_arn(REGION, "alpha-component", WILDCARD_VERSION),
        listed_arn(REGION, "beta-component", WILDCARD_VERSION),
    ];
    let arns = page_arns(case, &listed);
    assert_eq!(
        arns, expected,
        "case {case:?}: expected {expected:?}, got {arns:?}"
    );

    // One entry per NAME, with all versions of each component aggregated: the entry
    // carries the wildcard nodes and reports the latest version's own members.
    let alpha = entry_at(case, &listed, &expected[0]);
    assert_member(case, &alpha, "name", &json!("alpha-component"));
    assert_member(case, &alpha, "version", &json!(WILDCARD_VERSION));
    assert_member(case, &alpha, "description", &json!("the next major"));
    assert_member(case, &alpha, "supportedOsVersions", &json!(["Amazon Linux 2"]));
    assert_member(case, &alpha, "status", &json!("ACTIVE"));

    // That wildcard ARN resolves too: a read of it answers the latest build version of
    // the latest version of the name.
    let (status, read) = get_component(&router, &expected[0]).await;
    assert_eq!(
        status, 200,
        "case reading back the name ARN: expected HTTP 200, got {status} with {read}"
    );
    let reported = read
        .get("component")
        .cloned()
        .unwrap_or_else(|| panic!("case reading back the name ARN: expected a component in {read}"));
    assert_member(
        "reading back the name ARN",
        &reported,
        "arn",
        &json!(build_version_arn(REGION, "alpha-component", "2.0.0", 1)),
    );

    // "You can't combine this option with the version filter."
    let (status, refused) = list_components(
        &router,
        &json!({"byName": true, "filters": [{"name": "version", "values": ["1.0.0"]}]}),
    )
    .await;
    assert_refused(
        "by name beside the version filter",
        status,
        &refused,
        400,
        "InvalidRequestException",
    );
}

#[tokio::test]
async fn a_listing_reports_only_the_components_of_its_own_region() {
    let router = emulator();
    stored_components(&router).await;

    let regions: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the Region the components were created in",
            REGION,
            vec![
                listed_arn(REGION, "alpha-component", "1.0.0"),
                listed_arn(REGION, "alpha-component", "1.1.0"),
                listed_arn(REGION, "alpha-component", "2.0.0"),
                listed_arn(REGION, "beta-component", "1.0.0"),
            ],
        ),
        (
            "the other Region, which holds one of its own",
            OTHER_REGION,
            vec![listed_arn(OTHER_REGION, "gamma-component", "1.0.0")],
        ),
    ];
    for (case, region, want) in regions {
        let (status, listed) = list_components_in(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let arns = page_arns(case, &listed);
        assert_eq!(
            arns, want,
            "case {case:?}: expected {want:?}, got {arns:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_selects_on_each_documented_filter_and_owner() {
    let router = emulator();
    stored_components(&router).await;

    let alpha_100 = listed_arn(REGION, "alpha-component", "1.0.0");
    let alpha_110 = listed_arn(REGION, "alpha-component", "1.1.0");
    let alpha_200 = listed_arn(REGION, "alpha-component", "2.0.0");
    let beta_100 = listed_arn(REGION, "beta-component", "1.0.0");
    let everything = vec![
        alpha_100.clone(),
        alpha_110.clone(),
        alpha_200.clone(),
        beta_100.clone(),
    ];

    // Every filter name the `filters` member documents — description, name, platform,
    // productCodes, status, supportedOsVersion, type and version — including the ones the
    // issue did not cite, and the owners the `Ownership` enum names.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the description filter",
            json!({"filters": [{"name": "description", "values": ["the next major"]}]}),
            vec![alpha_200.clone()],
        ),
        (
            "the name filter",
            json!({"filters": [{"name": "name", "values": ["alpha-component"]}]}),
            vec![alpha_100.clone(), alpha_110.clone(), alpha_200.clone()],
        ),
        (
            "the name filter, on the name as the create submitted it",
            json!({"filters": [{"name": "name", "values": ["Beta Component"]}]}),
            vec![beta_100.clone()],
        ),
        (
            "the name filter carrying two values",
            json!({"filters": [{"name": "name", "values": ["Beta Component", "alpha-component"]}]}),
            everything.clone(),
        ),
        (
            "the platform filter",
            json!({"filters": [{"name": "platform", "values": ["Windows"]}]}),
            vec![beta_100.clone()],
        ),
        (
            "the productCodes filter, which no component a caller created carries",
            json!({"filters": [{"name": "productCodes", "values": ["abcd1234"]}]}),
            Vec::new(),
        ),
        (
            "the status filter naming the status a stored component reports",
            json!({"filters": [{"name": "status", "values": ["ACTIVE"]}]}),
            everything.clone(),
        ),
        (
            "the status filter naming DEPRECATED, which no stored component is in",
            json!({"filters": [{"name": "status", "values": ["DEPRECATED"]}]}),
            Vec::new(),
        ),
        (
            "the supportedOsVersion filter",
            json!({"filters": [{"name": "supportedOsVersion", "values": ["Amazon Linux 2"]}]}),
            vec![alpha_200.clone()],
        ),
        (
            "the supportedOsVersion filter naming a later entry of the list",
            json!({"filters": [
                {"name": "supportedOsVersion", "values": ["Microsoft Windows Server 2022"]},
            ]}),
            vec![beta_100.clone()],
        ),
        (
            "the type filter",
            json!({"filters": [{"name": "type", "values": ["TEST"]}]}),
            vec![beta_100.clone()],
        ),
        (
            "the version filter, exact",
            json!({"filters": [{"name": "version", "values": ["1.0.0"]}]}),
            vec![alpha_100.clone(), beta_100.clone()],
        ),
        (
            "the version filter, a wildcard patch node",
            json!({"filters": [{"name": "version", "values": ["1.1.x"]}]}),
            vec![alpha_110.clone()],
        ),
        (
            "the version filter, wildcard minor and patch nodes",
            json!({"filters": [{"name": "version", "values": ["1.x.x"]}]}),
            vec![alpha_100.clone(), alpha_110.clone(), beta_100.clone()],
        ),
        (
            "the version filter, every node a wildcard",
            json!({"filters": [{"name": "version", "values": ["x.x.x"]}]}),
            everything.clone(),
        ),
        (
            "two filters, which select what satisfies both",
            json!({"filters": [
                {"name": "platform", "values": ["Linux"]},
                {"name": "version", "values": ["1.x.x"]},
            ]}),
            vec![alpha_100.clone(), alpha_110.clone()],
        ),
        (
            "the caller's own components",
            json!({"owner": "Self"}),
            everything.clone(),
        ),
        (
            "components Amazon owns, of which this account holds none",
            json!({"owner": "Amazon"}),
            Vec::new(),
        ),
        (
            "components shared with this account, of which it holds none",
            json!({"owner": "Shared"}),
            Vec::new(),
        ),
        (
            "a filter beside byName, which reports the names it selected",
            json!({"byName": true, "filters": [{"name": "platform", "values": ["Linux"]}]}),
            vec![listed_arn(REGION, "alpha-component", WILDCARD_VERSION)],
        ),
    ];
    for (case, body, want) in cases {
        let (status, listed) = list_components(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let arns = page_arns(case, &listed);
        assert_eq!(
            arns, want,
            "case {case:?}: expected {want:?}, got {arns:?}"
        );
    }

    let refusals: Vec<(&str, Value)> = vec![
        (
            "a filter name this operation does not filter on",
            json!({"filters": [{"name": "owner", "values": ["Self"]}]}),
        ),
        (
            "a filter with no values",
            json!({"filters": [{"name": "name"}]}),
        ),
        (
            "a filter with no name",
            json!({"filters": [{"values": ["alpha-component"]}]}),
        ),
    ];
    for (case, body) in refusals {
        let (status, refused) = list_components(&router, &body).await;
        assert_refused(case, status, &refused, 400, "InvalidRequestException");
    }
}

#[tokio::test]
async fn a_listing_pages_through_what_it_selected_under_a_token_of_its_own() {
    let router = emulator();
    stored_components(&router).await;

    let by_version = vec![
        listed_arn(REGION, "alpha-component", "1.0.0"),
        listed_arn(REGION, "alpha-component", "1.1.0"),
        listed_arn(REGION, "alpha-component", "2.0.0"),
        listed_arn(REGION, "beta-component", "1.0.0"),
    ];
    let by_name = vec![
        listed_arn(REGION, "alpha-component", WILDCARD_VERSION),
        listed_arn(REGION, "beta-component", WILDCARD_VERSION),
    ];

    // Each page carries at most the size asked for, and a token while an entry remains;
    // the pages walked in order are the whole listing, for either kind of entry and
    // whatever the page size.
    let walks: Vec<(&str, bool, usize, Vec<String>)> = vec![
        ("one entry per page", false, 1, by_version.clone()),
        ("two entries per page", false, 2, by_version.clone()),
        ("three entries per page", false, 3, by_version.clone()),
        ("one name per page", true, 1, by_name.clone()),
        (
            "a page larger than the listing",
            false,
            25,
            by_version.clone(),
        ),
    ];
    for (case, by_name_option, page_size, want) in walks {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = Map::new();
            body.insert("maxResults".to_string(), json!(page_size));
            body.insert("byName".to_string(), json!(by_name_option));
            if let Some(token) = &token {
                body.insert("nextToken".to_string(), json!(token));
            }
            let (status, page) = list_components(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page}"
            );
            let arns = page_arns(case, &page);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {arns:?}"
            );
            walked.extend(arns);
            pages += 1;
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let expects_token = walked.len() < want.len();
            assert_eq!(
                token.is_some(),
                expects_token,
                "case {case:?}: expected a nextToken={expects_token} after {} entries, got {page}",
                walked.len()
            );
            if token.is_none() {
                break;
            }
            assert!(
                pages <= want.len(),
                "case {case:?}: expected at most {} pages, got {pages}",
                want.len()
            );
        }
        assert_eq!(
            walked, want,
            "case {case:?}: expected {want:?}, got {walked:?}"
        );
    }

    // A token this service did not issue for this listing is refused, however it is
    // wrong.
    let (_, first_page) = list_components(&router, &json!({"maxResults": 1})).await;
    let issued = first_page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tokens: Vec<(&str, Value)> = vec![
        (
            "a fabricated token",
            json!({"nextToken": "not-a-token", "maxResults": 1}),
        ),
        (
            "a token issued for the listing by version, spent on the listing by name",
            json!({"nextToken": issued.clone(), "maxResults": 1, "byName": true}),
        ),
        (
            "a token issued for the unfiltered listing, spent on a filtered one",
            json!({
                "nextToken": issued.clone(),
                "maxResults": 1,
                "filters": [{"name": "platform", "values": ["Linux"]}],
            }),
        ),
        (
            "a token issued in one Region, spent in another",
            json!({"nextToken": issued, "maxResults": 1, "owner": "Self"}),
        ),
    ];
    for (case, body) in tokens {
        let (status, refused) = if case.contains("Region") {
            list_components_in(&router, OTHER_REGION, &body).await
        } else {
            list_components(&router, &body).await
        };
        assert_refused(case, status, &refused, 400, "InvalidPaginationTokenException");
    }
}

/// `ListComponentBuildVersions` with `body`, in `region`.
async fn list_build_versions_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/ListComponentBuildVersions",
        Some(body),
    )
    .await
}

/// `ListComponentBuildVersions` with `body`, in [`REGION`].
async fn list_build_versions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    list_build_versions_in(router, REGION, body).await
}

/// The ARNs one build-version listing page reports, in the order it reported them.
fn summary_arns(case: &str, body: &Value) -> Vec<String> {
    body.get("componentSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a componentSummaryList in {body}"))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {entry}"))
                .to_string()
        })
        .collect()
}

/// RULE: a build-version listing reports one entry per stored BUILD version of the
/// component version its ARN names — of every component of the calling Region when it
/// names none — and nothing of another Region.
#[tokio::test]
async fn a_build_version_listing_reports_one_entry_per_stored_build_version() {
    let router = emulator();
    stored_components(&router).await;

    let every_build = vec![
        build_version_arn(REGION, "alpha-component", "1.0.0", 1),
        build_version_arn(REGION, "alpha-component", "1.0.0", 2),
        build_version_arn(REGION, "alpha-component", "1.1.0", 1),
        build_version_arn(REGION, "alpha-component", "2.0.0", 1),
        build_version_arn(REGION, "beta-component", "1.0.0", 1),
    ];

    // The rows differ along how many build versions the named version holds, whether the
    // request names a version at all, and which Region's endpoint served it.
    let cases: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "a version holding two build versions",
            REGION,
            json!({"componentVersionArn": listed_arn(REGION, "alpha-component", "1.0.0")}),
            vec![
                build_version_arn(REGION, "alpha-component", "1.0.0", 1),
                build_version_arn(REGION, "alpha-component", "1.0.0", 2),
            ],
        ),
        (
            "a version holding one build version",
            REGION,
            json!({"componentVersionArn": listed_arn(REGION, "alpha-component", "1.1.0")}),
            vec![build_version_arn(REGION, "alpha-component", "1.1.0", 1)],
        ),
        (
            "a version of the other name",
            REGION,
            json!({"componentVersionArn": listed_arn(REGION, "beta-component", "1.0.0")}),
            vec![build_version_arn(REGION, "beta-component", "1.0.0", 1)],
        ),
        (
            "a version nothing was stored under",
            REGION,
            json!({"componentVersionArn": listed_arn(REGION, "alpha-component", "9.9.9")}),
            Vec::new(),
        ),
        (
            "a name nothing was stored under",
            REGION,
            json!({"componentVersionArn": listed_arn(REGION, "absent-component", "1.0.0")}),
            Vec::new(),
        ),
        (
            "no ARN at all, which asks about every component this account owns",
            REGION,
            json!({}),
            every_build.clone(),
        ),
        (
            "a version of this Region, through the other Region's endpoint",
            OTHER_REGION,
            json!({"componentVersionArn": listed_arn(REGION, "alpha-component", "1.0.0")}),
            Vec::new(),
        ),
        (
            "no ARN at all, through the other Region's endpoint",
            OTHER_REGION,
            json!({}),
            vec![build_version_arn(OTHER_REGION, "gamma-component", "1.0.0", 1)],
        ),
    ];

    for (case, region, body, want) in cases {
        let (status, listed) = list_build_versions_in(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let arns = summary_arns(case, &listed);
        assert_eq!(arns, want, "case {case:?}: expected {want:?}, got {arns:?}");
    }

    // One entry, member for member: a summary of the stored build version it stands for.
    let case = "the second build version of alpha-component 1.0.0";
    let (_, listed) = list_build_versions(
        &router,
        &json!({"componentVersionArn": listed_arn(REGION, "alpha-component", "1.0.0")}),
    )
    .await;
    let entry = listed
        .get("componentSummaryList")
        .and_then(Value::as_array)
        .and_then(|entries| entries.get(1))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a second entry in {listed}"));
    for (name, expected) in [
        ("name", json!("alpha-component")),
        ("version", json!("1.0.0")),
        ("platform", json!("Linux")),
        ("type", json!("BUILD")),
        ("owner", json!(ACCOUNT)),
        ("description", json!("the second build")),
        ("state", json!({"status": "ACTIVE"})),
        ("obfuscate", json!(false)),
    ] {
        assert_member(case, &entry, name, &expected);
    }
    assert_instant(case, &entry, "dateCreated");
}

/// RULE: a build-version listing pages through what it selected under a token issued for
/// that listing and that query, and nothing else.
#[tokio::test]
async fn a_build_version_listing_pages_under_a_token_of_its_own() {
    let router = emulator();
    stored_components(&router).await;

    let every_build = vec![
        build_version_arn(REGION, "alpha-component", "1.0.0", 1),
        build_version_arn(REGION, "alpha-component", "1.0.0", 2),
        build_version_arn(REGION, "alpha-component", "1.1.0", 1),
        build_version_arn(REGION, "alpha-component", "2.0.0", 1),
        build_version_arn(REGION, "beta-component", "1.0.0", 1),
    ];

    // The walks differ along the page size, including one larger than the listing itself.
    for page_size in [1_usize, 2, 4, 25] {
        let case = format!("{page_size} entries per page");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = Map::new();
            body.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                body.insert("nextToken".to_string(), json!(token));
            }
            let (status, page) = list_build_versions(&router, &Value::Object(body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page}"
            );
            let arns = summary_arns(&case, &page);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {arns:?}"
            );
            walked.extend(arns);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let expects_token = walked.len() < every_build.len();
            assert_eq!(
                token.is_some(),
                expects_token,
                "case {case:?}: expected a nextToken={expects_token} after {} entries, got {page}",
                walked.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every_build,
            "case {case:?}: expected {every_build:?}, got {walked:?}"
        );
    }

    // A token this service did not issue for THIS listing and query is refused.
    let (_, first_page) = list_build_versions(&router, &json!({"maxResults": 1})).await;
    let issued = first_page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (_, components_page) = list_components(&router, &json!({"maxResults": 1})).await;
    let from_components = components_page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tokens: Vec<(&str, &str, Value)> = vec![
        (
            "a fabricated token",
            REGION,
            json!({"nextToken": "not-a-token", "maxResults": 1}),
        ),
        (
            "a token issued for the unfiltered listing, spent on one naming a version",
            REGION,
            json!({
                "nextToken": issued.clone(),
                "maxResults": 1,
                "componentVersionArn": listed_arn(REGION, "alpha-component", "1.0.0"),
            }),
        ),
        (
            "a token issued in one Region, spent in another",
            OTHER_REGION,
            json!({"nextToken": issued, "maxResults": 1}),
        ),
        (
            "a token issued by the component listing, spent on the build-version listing",
            REGION,
            json!({"nextToken": from_components, "maxResults": 1}),
        ),
    ];
    for (case, region, body) in tokens {
        let (status, refused) = list_build_versions_in(&router, region, &body).await;
        assert_refused(case, status, &refused, 400, "InvalidPaginationTokenException");
    }
}
