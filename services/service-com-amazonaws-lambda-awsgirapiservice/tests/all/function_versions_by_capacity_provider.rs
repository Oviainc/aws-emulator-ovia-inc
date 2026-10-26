// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFunctionVersionsByCapacityProvider`: "Returns a list of
//! function versions that are configured to use a specific capacity provider."
//!
//! The rule under test: the operation reports the PUBLISHED versions of the account's
//! functions whose configuration names the addressed capacity provider — each as the ARN
//! of that version together with the `State` of the function it belongs to — in a stable
//! order (by function name, then by version number ascending), one page of at most
//! `MaxItems` (never more than the 50 the model's
//! `FunctionVersionsByCapacityProviderList` admits) starting after the `Marker` the
//! previous page answered, with a `NextMarker` only while versions remain, beside the
//! provider's own `CapacityProviderArn`; a provider nothing is configured on reports an
//! empty list; a `CapacityProviderName` naming no provider is refused
//! `ResourceNotFoundException` (HTTP 404) naming the value the request gave; and the
//! listing changes nothing a later caller sees.
//!
//! Every row drives real HTTP requests through the router and cross-checks each listed
//! entry against what `GetFunctionConfiguration` reports for that version, so an entry the
//! listing invented rather than read fails here. The rows differ along what the rule
//! generalises over: how many functions use the provider and how many versions each has
//! (none, one, more than nine so a numeric order cannot pass as a lexicographic one), the
//! functions that use ANOTHER provider or none at all, the lifecycle state the functions
//! are in (`Pending` before the clock has made their activation due, `Active` after),
//! which of the two documented `CapacityProviderName` forms addressed the provider, the
//! page size (each published bound of `MaxItems`, a size between, one past the maximum,
//! and no size at all) and where the page starts (no marker, a marker naming a listed
//! version, a marker naming a version nobody holds, a marker past every version).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCapacityProvider` is bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider")`.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A well-formed capacity-provider operator role ARN.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The `State` values a function moves through here, from
/// `smithy_query(query="shape", shape="State")` (`Pending | Active | …`): a function is
/// `Pending` while it is being provisioned and `Active` once that is due.
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";

/// The bounds the model puts on `MaxItems` (`MaxFiftyListItems`, `@range(min: 1, max:
/// 50)`); the answered `FunctionVersionsByCapacityProviderList` is `@length(min: 0, max:
/// 50)`, the same ceiling. Written as the published numbers rather than as a crate
/// constant, so a wrong bound fails here.
const MAX_ITEMS_MIN: usize = 1;
/// See [`MAX_ITEMS_MIN`].
const MAX_ITEMS_MAX: usize = 50;

/// The two capacity providers every table creates, so a version on one is shown not to be
/// reported for the other.
const FLEET_A: &str = "fleet-a";
/// See [`FLEET_A`].
const FLEET_B: &str = "fleet-b";
/// A third provider nothing is ever configured on.
const FLEET_IDLE: &str = "fleet-idle";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `FunctionVersions` the listing answered. The member is `@required`, so a
    /// successful answer always carries the list, empty or not.
    fn versions(&self) -> Vec<Value> {
        match self.json().get("FunctionVersions") {
            Some(Value::Array(entries)) => entries.clone(),
            other => panic!(
                "expected a FunctionVersions list, got {other:?}: {}",
                self.text()
            ),
        }
    }

    /// The `FunctionArn` of each listed version, in the order they were reported.
    fn arns(&self) -> Vec<String> {
        self.versions()
            .iter()
            .map(|entry| {
                entry
                    .get("FunctionArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("every listed version carries its ARN, got {entry:?}")
                    })
                    .to_string()
            })
            .collect()
    }

    /// The `State` of each listed version, in the order they were reported.
    fn states(&self) -> Vec<String> {
        self.versions()
            .iter()
            .map(|entry| {
                entry
                    .get("State")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("every listed version carries its State, got {entry:?}")
                    })
                    .to_string()
            })
            .collect()
    }

    /// The `NextMarker` the listing answered, when it answered one.
    fn next_marker(&self) -> Option<String> {
        self.string("NextMarker")
    }

    /// The `message` of a wire error, under either casing the JSON envelopes use.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `ListFunctionVersionsByCapacityProvider` on the provider `reference` names, with the
/// query string `query` (`""` for no parameters at all).
async fn list_versions(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}/function-versions{query}"),
        &Value::Null,
    )
    .await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// The unqualified ARN a function of `name` carries — the ARN of its UNPUBLISHED
/// configuration, which is not one of the versions this operation reports.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN of one published version of a function, as the model's `NameSpacedFunctionArn`
/// pattern spells it.
fn version_arn(name: &str, version: u64) -> String {
    format!("{}:{version}", function_arn(name))
}

/// `CreateCapacityProvider` with the smallest request the model admits.
async fn create_provider(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": name,
            "VpcConfig": { "SubnetIds": ["subnet-10000000"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "case provider {name:?}: expected 202 from CreateCapacityProvider, got {} {}",
        answer.status,
        answer.text()
    );
}

/// One function row of a table: the function's name, the capacity provider its
/// configuration names (`None` for a function that runs on no provider) and how many
/// versions are published of it.
struct FunctionRow {
    name: &'static str,
    provider: Option<&'static str>,
    versions: u64,
}

/// The functions of the reporting table, in the order they are CREATED — deliberately not
/// the order their versions must be reported in. They differ in the provider they name,
/// in how many versions they publish (none, one, more than nine) and in whether they name
/// a provider at all.
fn reporting_functions() -> Vec<FunctionRow> {
    vec![
        // More than nine versions, so reporting them in ascending NUMERIC order is
        // distinguishable from reporting them in lexicographic order ("10" < "2").
        FunctionRow {
            name: "fn-order",
            provider: Some(FLEET_A),
            versions: 12,
        },
        FunctionRow {
            name: "fn-a",
            provider: Some(FLEET_A),
            versions: 1,
        },
        // Configured on the provider but never published: it has an unpublished
        // configuration only, and that is not a version this operation reports.
        FunctionRow {
            name: "fn-draft",
            provider: Some(FLEET_A),
            versions: 0,
        },
        FunctionRow {
            name: "fn-other",
            provider: Some(FLEET_B),
            versions: 3,
        },
        FunctionRow {
            name: "fn-plain",
            provider: None,
            versions: 2,
        },
    ]
}

/// Create `row`'s function and publish its versions.
async fn create_function(router: &MultiServiceRouter, row: &FunctionRow) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": row.name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        "Publish": row.versions >= 1,
    });
    if let Some(provider) = row.provider {
        body["CapacityProviderConfig"] = json!({
            "LambdaManagedInstancesCapacityProviderConfig": {
                "CapacityProviderArn": provider_arn(provider),
            },
        });
    }
    let created = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        created.status,
        201,
        "case function {:?}: expected 201 from CreateFunction, got {} {}",
        row.name,
        created.status,
        created.text()
    );
    // Each publish states a description of its own, so it freezes a configuration that
    // has changed since the last version and gets a number of its own.
    for version in 2..=row.versions {
        let published = call(
            router,
            "POST",
            &format!("{FUNCTIONS_URI}/{}/versions", row.name),
            &json!({ "Description": format!("snapshot {version}") }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case function {:?} version {version}: expected 201 from PublishVersion, got {} {}",
            row.name,
            published.status,
            published.text()
        );
    }
}

/// Create every function of `rows` and the three capacity providers they name.
async fn create_all(router: &MultiServiceRouter, rows: &[FunctionRow]) {
    for provider in [FLEET_A, FLEET_B, FLEET_IDLE] {
        create_provider(router, provider).await;
    }
    for row in rows {
        create_function(router, row).await;
    }
}

/// The versions `provider` must be reported for, in the order the rule names: by function
/// name, then by version number ascending.
fn expected_arns(rows: &[FunctionRow], provider: &str) -> Vec<String> {
    let mut names: Vec<&FunctionRow> = rows
        .iter()
        .filter(|row| row.provider == Some(provider))
        .collect();
    names.sort_by_key(|row| row.name);
    names
        .iter()
        .flat_map(|row| (1..=row.versions).map(|version| version_arn(row.name, version)))
        .collect()
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Every function the store holds, as (name, state, published version count) — the
/// snapshot a read must leave untouched.
fn stored_snapshot(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, String, usize)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .values()
        .map(|record| {
            (
                record.config.name.clone(),
                record.state.clone(),
                record.versions.len(),
            )
        })
        .collect()
}

/// The `State` `GetFunctionConfiguration` reports for the version `arn` names — the
/// independent read every listed entry is cross-checked against, so a listing cannot
/// invent a state of its own.
async fn configuration_state(router: &MultiServiceRouter, arn: &str) -> String {
    let answer = call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{arn}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {arn:?}: expected 200 from GetFunctionConfiguration, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .string("State")
        .unwrap_or_else(|| panic!("case {arn:?}: the configuration must report a State"))
}

/// The listing reports every published version configured on the provider, in function and
/// version order, with the state of the function each belongs to — and nothing of the
/// functions that run elsewhere.
#[tokio::test]
async fn listing_reports_every_published_version_configured_on_the_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // A provider nothing is configured on: the `@required` list is answered, empty, with
    // no marker to continue from.
    create_provider(&router, FLEET_IDLE).await;
    let empty = list_versions(&router, FLEET_IDLE, "").await;
    assert_eq!(
        (empty.status, empty.arns(), empty.next_marker()),
        (200, Vec::new(), None),
        "case a provider with no functions: expected an empty page and no marker, got {} {}",
        empty.status,
        empty.text()
    );
    assert_eq!(
        empty.string("CapacityProviderArn"),
        Some(provider_arn(FLEET_IDLE)),
        "case a provider with no functions: expected the provider's own ARN, got {}",
        empty.text()
    );

    let rows = reporting_functions();
    for provider in [FLEET_A, FLEET_B] {
        create_provider(&router, provider).await;
    }
    for row in &rows {
        create_function(&router, row).await;
    }

    // Each row of this table is one lifecycle stage: a freshly created function is
    // `Pending`, and the same versions report `Active` once the clock has made that
    // transition due. The ARNs reported are the same in both stages.
    for (stage, state) in [("freshly created", STATE_PENDING), ("once active", STATE_ACTIVE)] {
        if state == STATE_ACTIVE {
            make_activation_due(&engine);
        }
        for provider in [FLEET_A, FLEET_B] {
            let expected = expected_arns(&rows, provider);
            // Both documented `CapacityProviderName` forms name the same provider, so
            // both report the same page.
            for (form, reference) in [
                ("the bare name", provider.to_string()),
                ("the provider ARN", provider_arn(provider)),
            ] {
                let answer = list_versions(&router, &reference, "?MaxItems=50").await;
                assert_eq!(
                    (answer.status, answer.arns(), answer.next_marker()),
                    (200, expected.clone(), None),
                    "case {stage} {provider:?} by {form}: expected {expected:?} and no \
                     marker, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.string("CapacityProviderArn"),
                    Some(provider_arn(provider)),
                    "case {stage} {provider:?} by {form}: expected the provider's own ARN, \
                     got {}",
                    answer.text()
                );
                assert_eq!(
                    answer.states(),
                    vec![state.to_string(); expected.len()],
                    "case {stage} {provider:?} by {form}: every version must report {state}, \
                     got {:?}",
                    answer.states()
                );
            }

            // The unpublished configuration of a function is not one of the versions
            // reported, whether or not that function has published any.
            let listed = list_versions(&router, provider, "?MaxItems=50").await;
            for row in rows.iter().filter(|row| row.provider == Some(provider)) {
                assert!(
                    !listed.arns().contains(&function_arn(row.name)),
                    "case {stage} {provider:?}: the unpublished configuration of {:?} must \
                     not be reported, got {:?}",
                    row.name,
                    listed.arns()
                );
            }

            // Every entry's state is the one an independent read of that version reports.
            for arn in listed.arns() {
                let reported = configuration_state(&router, &arn).await;
                assert_eq!(
                    reported, state,
                    "case {stage} {arn:?}: the listed state must be the one \
                     GetFunctionConfiguration reports, got {reported:?}"
                );
            }
        }
    }

    // A read changes nothing a later caller sees: the same page comes back, and the store
    // holds exactly what it held before the listing.
    let before = stored_snapshot(&engine);
    let again = list_versions(&router, FLEET_A, "?MaxItems=50").await;
    assert_eq!(
        (again.arns(), again.next_marker()),
        (expected_arns(&rows, FLEET_A), None),
        "case repeated listing: the page must be stable, got {}",
        again.text()
    );
    assert_eq!(
        stored_snapshot(&engine),
        before,
        "case repeated listing: a listing must not change the stored functions"
    );
}

/// One paging row: the page size asked for (`None` asks for none at all) and the most
/// entries one page may then carry.
struct PagingCase {
    name: &'static str,
    max_items: Option<usize>,
    page_limit: usize,
}

/// The listing pages over every version on the provider: each page carries at most the
/// size asked for, the pages concatenate to the whole ordered sequence with nothing
/// repeated or skipped, and a marker is answered exactly while versions remain.
#[tokio::test]
async fn listing_pages_over_every_version_on_the_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let rows = reporting_functions();
    create_all(&router, &rows).await;
    make_activation_due(&engine);
    let expected = expected_arns(&rows, FLEET_A);
    let total = expected.len();

    let cases = [
        PagingCase {
            name: "one at a time, the smallest page the model admits",
            max_items: Some(MAX_ITEMS_MIN),
            page_limit: MAX_ITEMS_MIN,
        },
        PagingCase {
            name: "two at a time",
            max_items: Some(2),
            page_limit: 2,
        },
        PagingCase {
            name: "five at a time, a size that does not divide the total",
            max_items: Some(5),
            page_limit: 5,
        },
        PagingCase {
            name: "at the largest page the model admits",
            max_items: Some(MAX_ITEMS_MAX),
            page_limit: MAX_ITEMS_MAX,
        },
        PagingCase {
            name: "no page size at all",
            max_items: None,
            page_limit: MAX_ITEMS_MAX,
        },
    ];

    for case in cases {
        let label = case.name;
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (case.max_items, marker.as_deref()) {
                (Some(size), Some(from)) => format!("?MaxItems={size}&Marker={from}"),
                (Some(size), None) => format!("?MaxItems={size}"),
                (None, Some(from)) => format!("?Marker={from}"),
                (None, None) => String::new(),
            };
            let answer = list_versions(&router, FLEET_A, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: expected 200 for {query:?}, got {} {}",
                answer.status,
                answer.text()
            );
            let page = answer.arns();
            assert!(
                page.len() <= case.page_limit,
                "case {label:?}: a page carries at most {} entries, got {}",
                case.page_limit,
                page.len()
            );
            seen.extend(page.iter().cloned());
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {label:?}: paging must terminate, got {pages} pages for {total} versions"
            );
            match answer.next_marker() {
                Some(next) => {
                    assert!(
                        seen.len() < total,
                        "case {label:?}: a marker ({next:?}) is answered only while versions \
                         remain, having seen {}",
                        seen.len()
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {label:?}: the pages must concatenate to every version in order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case {label:?}: no version may be reported twice, got {seen:?}"
        );
    }

    // A marker continues AFTER the version it names, whether or not that version is one
    // this provider holds: it is the ordering position that decides the page.
    let marker_cases: [(&str, String, usize); 4] = [
        ("a marker naming the first version", expected[0].clone(), 1),
        (
            "a marker naming a version mid-sequence",
            expected[4].clone(),
            5,
        ),
        (
            "a marker naming a version nobody holds",
            version_arn("fn-a", 7),
            1,
        ),
        (
            "a marker ordered past every version",
            version_arn("zz-past-everything", 1),
            total,
        ),
    ];
    for (label, marker, skipped) in marker_cases {
        let answer = list_versions(&router, FLEET_A, &format!("?MaxItems=50&Marker={marker}")).await;
        let tail: Vec<String> = expected.iter().skip(skipped).cloned().collect();
        assert_eq!(
            (answer.status, answer.arns()),
            (200, tail.clone()),
            "case {label:?}: expected the {} versions after {marker:?}, got {}",
            tail.len(),
            answer.text()
        );
    }
}

/// A page size outside the range the model declares is refused, and each published bound
/// is served.
#[tokio::test]
async fn listing_refuses_a_page_size_outside_the_modelled_range() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let rows = reporting_functions();
    create_all(&router, &rows).await;
    make_activation_due(&engine);
    let expected = expected_arns(&rows, FLEET_A);

    // `MaxItems` is `@range(min: 1, max: 50)`: below the minimum and past the maximum are
    // refused rather than clamped into a page.
    for (label, max_items) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("one past the maximum", "51"),
        ("far past the maximum", "10000"),
    ] {
        let answer = list_versions(&router, FLEET_A, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {label}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // Each published bound is served: the minimum answers one version and a marker, the
    // maximum answers every version this provider holds and none.
    let smallest = list_versions(&router, FLEET_A, &format!("?MaxItems={MAX_ITEMS_MIN}")).await;
    assert_eq!(
        (
            smallest.status,
            smallest.arns(),
            smallest.next_marker().as_deref()
        ),
        (
            200,
            vec![expected[0].clone()],
            Some(expected[0].as_str())
        ),
        "case MaxItems at the minimum: expected one version and a marker, got {}",
        smallest.text()
    );
    let largest = list_versions(&router, FLEET_A, &format!("?MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (largest.status, largest.arns(), largest.next_marker()),
        (200, expected.clone(), None),
        "case MaxItems at the maximum: expected every version and no marker, got {}",
        largest.text()
    );
}

/// One refusal row: how the provider was addressed, and the value the refusal must name.
struct RefusalCase {
    label: &'static str,
    reference: String,
    status: u16,
    code: &'static str,
}

/// A `CapacityProviderName` that names no provider is refused, and the refusal states the
/// value the request gave.
#[tokio::test]
async fn a_capacity_provider_that_does_not_exist_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let rows = reporting_functions();
    create_all(&router, &rows).await;
    make_activation_due(&engine);

    // The rows differ in which documented `CapacityProviderName` form named nothing, and
    // in whether the value names no provider or no capacity provider at all.
    let cases = [
        RefusalCase {
            label: "a bare name no provider is stored under",
            reference: "fleet-never-created".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an ARN no provider is stored under",
            reference: provider_arn("fleet-ghost"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "the name of a function rather than a provider",
            reference: "fn-order".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an ARN of another resource type",
            reference: function_arn("fn-order"),
            status: 400,
            code: "InvalidParameterValueException",
        },
    ];

    for case in &cases {
        let answer = list_versions(&router, &case.reference, "").await;
        assert_eq!(
            (answer.status, answer.error_code().contains(case.code)),
            (case.status, true),
            "case {:?}: expected {} {}, got {} {}",
            case.label,
            case.status,
            case.code,
            answer.status,
            answer.text()
        );
        if case.status == 404 {
            let expected = format!(
                "Capacity provider not found: {}. Check that the ARN is correct and the \
                 capacity provider exists in this Region.",
                case.reference
            );
            let reported = answer.message();
            assert_eq!(
                reported, expected,
                "case {:?}: expected {expected:?}, got {reported:?}",
                case.label
            );
        }
    }

    // The positive control: the same request against a provider that does exist answers
    // its versions, so the refusals above are about the provider and not about the
    // operation.
    let served = list_versions(&router, FLEET_A, "?MaxItems=50").await;
    assert_eq!(
        (served.status, served.arns()),
        (200, expected_arns(&rows, FLEET_A)),
        "case a provider that exists: expected its versions, got {} {}",
        served.status,
        served.text()
    );
}
