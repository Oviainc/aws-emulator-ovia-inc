// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListThings`: the filtered, paginated reader over the
//! AWS IoT thing registry.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `GET /things` request signed for the `iot` signing name — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `ListThings` reports ONE PAGE of the things the registry holds,
//! in `thingName` order, each entry built from the record `CreateThing` persisted
//! (`thingName`, `thingTypeName`, `thingArn`, `attributes`, `version`); the request's
//! `thingTypeName`, `attributeName` and `attributeValue` members NARROW that set
//! (`usePrefixAttributeValue` deciding whether the value matches in full or as a
//! prefix); `maxResults` bounds the page and `nextToken` is returned only when things
//! remain and resumes the SAME listing exactly once.
//!
//! Each table spans the class the rule generalises over — which filter members a
//! request carries and of what form, how many things match, and every page size from
//! one to past the number of matches — rather than one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::describe_endpoint::percent_encode;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service. The signature itself is
/// never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `things` member as the list it is, or an empty list when it is absent.
    fn things(&self) -> Vec<Value> {
        self.body
            .get("things")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `thingName` of every entry, in the order the response carries them.
    fn names(&self) -> Vec<String> {
        self.things()
            .iter()
            .map(|thing| {
                thing
                    .get("thingName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A `ThingAttribute` entry, reduced to the members the registry persists, so an entry
/// in a listing and the `DescribeThing` read of the same name compare as one value.
#[derive(Debug, PartialEq, Eq)]
struct Attributes {
    thing_name: Option<String>,
    thing_type_name: Option<String>,
    thing_arn: Option<String>,
    attributes: BTreeMap<String, String>,
    version: i64,
}

impl Attributes {
    fn of(document: &Value) -> Self {
        let text = |name: &str| {
            document
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Self {
            thing_name: text("thingName"),
            thing_type_name: text("thingTypeName"),
            thing_arn: text("thingArn"),
            attributes: document
                .get("attributes")
                .and_then(Value::as_object)
                .map(|members: &Map<String, Value>| {
                    members
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.clone(),
                                value.as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            // `ThingAttribute$version` and `DescribeThing$version` are both the
            // registry version of the record, defaulting to 0 when absent.
            version: document.get("version").and_then(Value::as_i64).unwrap_or(0),
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Create the thing types the fixture's things are created with.
    ///
    /// `CreateThing` refuses a `thingTypeName` the registry does not hold, so the
    /// types are created first — through the wire, like everything else here, so the
    /// fixture seeds no state directly.
    async fn register_thing_types(&self, names: &[&str]) {
        for name in names {
            let created = self
                .call("POST", &format!("/thing-types/{name}"), Some(&json!({})))
                .await;
            assert_eq!(
                created.status, 200,
                "thing type {name}: CreateThingType must succeed, got {created:?}"
            );
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
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
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str, body: &Value) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(body))
            .await
    }

    async fn describe_thing(&self, thing_name: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}"), None).await
    }

    /// `GET /things` with the query string `query` (already encoded).
    async fn list_things(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/things".to_string()
        } else {
            format!("/things?{query}")
        };
        self.call("GET", &path, None).await
    }
}

/// The ARN a thing registered in this harness's region must carry.
fn expected_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// One thing the fixture registers.
struct Fixture {
    thing_name: String,
    thing_type_name: Option<&'static str>,
    attributes: Vec<(&'static str, &'static str)>,
}

impl Fixture {
    fn body(&self) -> Value {
        let attributes: Map<String, Value> = self
            .attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect();
        let mut body = Map::new();
        if !attributes.is_empty() {
            body.insert(
                "attributePayload".to_string(),
                json!({ "attributes": Value::Object(attributes) }),
            );
        }
        if let Some(thing_type_name) = self.thing_type_name {
            body.insert("thingTypeName".to_string(), json!(thing_type_name));
        }
        Value::Object(body)
    }
}

/// The thing types the fixture uses. Both are arbitrary `ThingTypeName`s (modelled
/// `len 1..128, pattern ^[a-zA-Z0-9:_-]+$`).
const SENSOR_TYPE: &str = "sensor";
const GATEWAY_TYPE: &str = "gateway";

/// An attribute name and value using every character class the model allows:
/// `AttributeName` is `len 0..128, pattern ^[a-zA-Z0-9_.,@/:#-]+$` and `AttributeValue`
/// is `len 0..800, pattern ^[a-zA-Z0-9_.,@/:#=\[\]-]*$`
/// (`smithy_query(query="operation",operation="ListThings")`).
const PUNCTUATED_NAME: &str = "build.no,v@1/2:3#4-5";
const PUNCTUATED_VALUE: &str = "a=b[c]-d.e,f@g/h:i#j";

/// The registry every case below reads. It spans what the rule generalises over:
///
/// * the form and length of `thingName` — the shortest legal name, the longest
///   (`ThingName` is `len 1..128`), and one using every character class its
///   `^[a-zA-Z0-9:_-]+$` allows;
/// * whether a thing is associated with a thing type, and with which one;
/// * how many attributes a thing carries and of what form — none at all, one, several,
///   an empty value (which `AttributeValue`'s `*` admits), a value that another thing's
///   value is a prefix of, and a value differing from another's only in case.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            thing_name: "a".to_string(),
            thing_type_name: None,
            attributes: vec![],
        },
        Fixture {
            thing_name: "Fleet:01_sensor-A".to_string(),
            thing_type_name: Some(SENSOR_TYPE),
            attributes: vec![("color", "red"), ("size", "large")],
        },
        Fixture {
            thing_name: "blank-value".to_string(),
            thing_type_name: None,
            attributes: vec![("color", "")],
        },
        Fixture {
            thing_name: "punct_thing".to_string(),
            thing_type_name: Some(SENSOR_TYPE),
            attributes: vec![(PUNCTUATED_NAME, PUNCTUATED_VALUE)],
        },
        Fixture {
            thing_name: "sensor1".to_string(),
            thing_type_name: Some(GATEWAY_TYPE),
            attributes: vec![("color", "redwood")],
        },
        Fixture {
            thing_name: "n".repeat(128),
            thing_type_name: None,
            attributes: vec![("color", "Red")],
        },
        Fixture {
            thing_name: "zulu".to_string(),
            thing_type_name: None,
            attributes: vec![("color", "red"), ("size", "small")],
        },
    ]
}

/// Register every fixture through the wire and answer their names in the order a
/// listing must report them: `thingName` order.
async fn registry() -> (Emulator, Vec<String>) {
    let emulator = Emulator::new();
    emulator.register_thing_types(&[SENSOR_TYPE, GATEWAY_TYPE]).await;
    let mut names = Vec::new();
    for fixture in fixtures() {
        let created = emulator
            .create_thing(&fixture.thing_name, &fixture.body())
            .await;
        assert_eq!(
            created.status,
            200,
            "fixture {:?}: CreateThing must succeed, got {created:?}",
            fixture.thing_name
        );
        names.push(fixture.thing_name.clone());
    }
    names.sort();
    (emulator, names)
}

#[tokio::test]
async fn every_registered_thing_is_listed_in_name_order_with_the_record_that_was_persisted() {
    let (emulator, ordered_names) = registry().await;

    let listed = emulator.list_things("").await;
    assert_eq!(
        listed.status, 200,
        "an unfiltered ListThings must succeed, got {listed:?}"
    );
    assert_eq!(
        listed.names(),
        ordered_names,
        "ListThings must report every registered thing in thingName order, got {:?}",
        listed.names()
    );
    assert_eq!(
        listed.next_token(),
        None,
        "a listing that returned every thing must carry no nextToken, got {:?}",
        listed.next_token()
    );

    // Every entry is the record the writer persisted: the reader of the same name
    // answers member-for-member, and the ARN is the one minted for this region.
    for entry in listed.things() {
        let listed_thing = Attributes::of(&entry);
        let thing_name = listed_thing
            .thing_name
            .clone()
            .unwrap_or_else(|| panic!("case {entry:?}: a listed thing must carry its name"));
        let described = emulator.describe_thing(&thing_name).await;
        assert_eq!(
            described.status, 200,
            "case {thing_name:?}: DescribeThing must succeed, got {described:?}"
        );
        assert_eq!(
            listed_thing,
            Attributes::of(&described.body),
            "case {thing_name:?}: the listed entry must be the persisted record, got {entry:?}"
        );
        assert_eq!(
            listed_thing.thing_arn,
            Some(expected_arn(&thing_name)),
            "case {thing_name:?}: the listed entry must carry the thing's ARN, got {entry:?}"
        );
        assert_eq!(
            listed_thing.version, 1,
            "case {thing_name:?}: a thing written once is at registry version 1, got {entry:?}"
        );
    }
}

/// One filter case: the query string a client sends, and the names it must select.
struct FilterCase {
    label: &'static str,
    query: String,
    expected: Vec<&'static str>,
}

/// A `thingName` the registry holds, for the rows that name the longest legal one.
fn longest_name() -> String {
    "n".repeat(128)
}

/// The filter table. Each row differs along what the filter rule generalises over:
/// which of the three filter members the request carries, whether the value matches in
/// full or as a prefix, and how many things the combination selects (none, one, many).
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "no filter at all",
            query: String::new(),
            expected: vec![
                "Fleet:01_sensor-A",
                "a",
                "blank-value",
                "LONGEST",
                "punct_thing",
                "sensor1",
                "zulu",
            ],
        },
        FilterCase {
            label: "an attributeName with no attributeValue selects the things that carry it",
            query: "attributeName=color".to_string(),
            expected: vec![
                "Fleet:01_sensor-A",
                "blank-value",
                "LONGEST",
                "sensor1",
                "zulu",
            ],
        },
        FilterCase {
            label: "an exact attributeValue match",
            query: "attributeName=color&attributeValue=red".to_string(),
            expected: vec!["Fleet:01_sensor-A", "zulu"],
        },
        FilterCase {
            label: "the same value as a PREFIX also selects the value it is a prefix of",
            query: "attributeName=color&attributeValue=red&usePrefixAttributeValue=true"
                .to_string(),
            expected: vec!["Fleet:01_sensor-A", "sensor1", "zulu"],
        },
        FilterCase {
            label: "an attribute value is matched case-sensitively",
            query: "attributeName=color&attributeValue=Red".to_string(),
            expected: vec!["LONGEST"],
        },
        FilterCase {
            label: "the empty attributeValue matches the thing whose value is empty",
            query: "attributeName=color&attributeValue=".to_string(),
            expected: vec!["blank-value"],
        },
        FilterCase {
            label: "the empty attributeValue as a prefix matches every value of that attribute",
            query: "attributeName=color&attributeValue=&usePrefixAttributeValue=true".to_string(),
            expected: vec![
                "Fleet:01_sensor-A",
                "blank-value",
                "LONGEST",
                "sensor1",
                "zulu",
            ],
        },
        FilterCase {
            label: "an attribute name and value using the whole modelled alphabet",
            query: format!(
                "attributeName={}&attributeValue={}",
                percent_encode(PUNCTUATED_NAME),
                percent_encode(PUNCTUATED_VALUE)
            ),
            expected: vec!["punct_thing"],
        },
        FilterCase {
            label: "the same punctuated value truncated, as a prefix",
            query: format!(
                "attributeName={}&attributeValue={}&usePrefixAttributeValue=true",
                percent_encode(PUNCTUATED_NAME),
                percent_encode(&PUNCTUATED_VALUE[..6])
            ),
            expected: vec!["punct_thing"],
        },
        FilterCase {
            label: "a thingTypeName selects the things associated with that type",
            query: format!("thingTypeName={SENSOR_TYPE}"),
            expected: vec!["Fleet:01_sensor-A", "punct_thing"],
        },
        FilterCase {
            label: "a thingTypeName and an attribute filter both apply",
            query: format!("thingTypeName={GATEWAY_TYPE}&attributeName=color&attributeValue=redwood"),
            expected: vec!["sensor1"],
        },
        FilterCase {
            label: "a thing type and an attribute value only one thing of that type carries",
            query: format!("thingTypeName={SENSOR_TYPE}&attributeName=color&attributeValue=red"),
            expected: vec!["Fleet:01_sensor-A"],
        },
        FilterCase {
            label: "an attribute value no thing carries",
            query: "attributeName=color&attributeValue=blue".to_string(),
            expected: vec![],
        },
        FilterCase {
            label: "an attribute name no thing carries",
            query: "attributeName=altitude".to_string(),
            expected: vec![],
        },
        FilterCase {
            label: "a thing type no thing is associated with",
            query: "thingTypeName=no-such-type".to_string(),
            expected: vec![],
        },
    ]
}

/// The expected names of a case, with the `LONGEST` placeholder resolved and sorted
/// into `thingName` order — the order the rule says a listing reports.
fn expected_names(expected: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = expected
        .iter()
        .map(|name| {
            if *name == "LONGEST" {
                longest_name()
            } else {
                (*name).to_string()
            }
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_filter_members_select_exactly_the_things_that_match_them() {
    let (emulator, _) = registry().await;

    for case in filter_cases() {
        let label = case.label;
        let listed = emulator.list_things(&case.query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListThings must succeed, got {listed:?}"
        );
        assert_eq!(
            listed.names(),
            expected_names(&case.expected),
            "case {label:?}: wrong things selected, got {:?}",
            listed.names()
        );
        assert!(
            listed.body.get("things").is_some(),
            "case {label:?}: `things` must be reported even when nothing matches, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_attribute_value_without_an_attribute_name_is_refused() {
    let (emulator, _) = registry().await;

    // `attributeValue` names no attribute to compare against on its own, so there is
    // no set of things it could select: the modelled `InvalidRequestException` (HTTP
    // 400) is the answer, not a listing of everything.
    for value in ["red", "", "a=b[c]"] {
        let label = format!("attributeValue={value:?} with no attributeName");
        let listed = emulator
            .list_things(&format!("attributeValue={}", percent_encode(value)))
            .await;
        assert_eq!(
            listed.status, 400,
            "case {label:?}: must be refused 400, got {listed:?}"
        );
        assert_eq!(
            listed.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: must be refused InvalidRequestException, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn every_page_size_walks_the_listing_once_through_its_next_token() {
    let (emulator, ordered_names) = registry().await;
    let total = ordered_names.len();

    // Exhaustive over the page size rather than sampled: every size from the modelled
    // minimum (`RegistryMaxResults` is `range 1..250`) to one past the number of
    // things, plus the modelled maximum. Each walk must visit the listing exactly
    // once, in order, whatever the boundary falls between.
    let mut page_sizes: Vec<usize> = (1..=total + 1).collect();
    page_sizes.push(250);

    for page_size in page_sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={}", percent_encode(token)));
            }
            let page = emulator.list_things(&query).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}, page {pages}: ListThings must succeed, got {page:?}"
            );
            let names = page.names();
            assert!(
                names.len() <= page_size,
                "page size {page_size}, page {pages}: a page must not exceed maxResults, got {names:?}"
            );
            walked.extend(names);
            assert_eq!(
                page.next_token().is_some(),
                walked.len() < total,
                "page size {page_size}, page {pages}: nextToken must appear exactly while things \
                 remain, got {page:?}"
            );
            token = page.next_token();
            pages += 1;
            assert!(
                pages <= total + 2,
                "page size {page_size}: the walk must terminate, got {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ordered_names,
            "page size {page_size}: the pages must concatenate to the whole listing once, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_page_of_a_filtered_listing_resumes_that_same_listing() {
    let (emulator, _) = registry().await;
    let whole = emulator.list_things("attributeName=color").await;
    let expected = whole.names();
    assert!(
        expected.len() > 1,
        "the fixture must hold more than one thing with that attribute, got {expected:?}"
    );

    let first = emulator
        .list_things("attributeName=color&maxResults=1")
        .await;
    assert_eq!(
        first.names(),
        expected[..1].to_vec(),
        "the first page must be the first match, got {first:?}"
    );
    let token = first
        .next_token()
        .unwrap_or_else(|| panic!("a truncated filtered listing must carry a nextToken: {first:?}"));

    let rest = emulator
        .list_things(&format!(
            "attributeName=color&nextToken={}",
            percent_encode(&token)
        ))
        .await;
    assert_eq!(
        rest.status, 200,
        "the continuation of a filtered listing must succeed, got {rest:?}"
    );
    assert_eq!(
        rest.names(),
        expected[1..].to_vec(),
        "the continuation must report the rest of the SAME filtered listing, got {rest:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let (emulator, _) = registry().await;

    // A token issued by the attribute-filtered listing: replaying it against another
    // listing resumes a cut that listing never made, so it is refused rather than
    // silently restarting a different one from the middle.
    let filtered = emulator
        .list_things("attributeName=color&maxResults=1")
        .await;
    let filtered_token = filtered
        .next_token()
        .unwrap_or_else(|| panic!("a truncated listing must carry a nextToken: {filtered:?}"));
    let unfiltered = emulator.list_things("maxResults=1").await;
    let unfiltered_token = unfiltered
        .next_token()
        .unwrap_or_else(|| panic!("a truncated listing must carry a nextToken: {unfiltered:?}"));

    let cases: Vec<(&str, String)> = vec![
        ("the empty token", String::new()),
        ("a token that is not hex at all", "not-a-real-token".to_string()),
        // Hex that decodes to text no listing ever issued.
        ("hex of arbitrary text", ::hex::encode("zulu")),
        (
            "a token of the filtered listing, replayed unfiltered",
            filtered_token.clone(),
        ),
        (
            "a token of the unfiltered listing, replayed with a filter",
            unfiltered_token.clone(),
        ),
    ];

    for (label, token) in cases {
        // The first three rows and the fourth are sent to the UNFILTERED listing; the
        // last is sent to the filtered one, which is the listing that did not issue it.
        let query = if label.ends_with("replayed with a filter") {
            format!("attributeName=color&nextToken={}", percent_encode(&token))
        } else {
            format!("nextToken={}", percent_encode(&token))
        };
        let refused = emulator.list_things(&query).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: must be refused InvalidRequestException, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn max_results_is_honoured_at_its_modelled_bounds_and_refused_outside_them() {
    let (emulator, ordered_names) = registry().await;

    // `RegistryMaxResults` is `range 1..250`
    // (`smithy_query(query="operation",operation="ListThings")`): both bounds are
    // served, and one past either bound is refused.
    let smallest = emulator.list_things("maxResults=1").await;
    assert_eq!(
        smallest.names(),
        ordered_names[..1].to_vec(),
        "maxResults=1 must report exactly one thing, got {smallest:?}"
    );
    let largest = emulator.list_things("maxResults=250").await;
    assert_eq!(
        largest.names(),
        ordered_names,
        "maxResults=250 must report every thing the registry holds, got {largest:?}"
    );
    assert_eq!(
        largest.next_token(),
        None,
        "a page that held every thing must carry no nextToken, got {largest:?}"
    );

    for out_of_range in ["0", "251"] {
        let refused = emulator
            .list_things(&format!("maxResults={out_of_range}"))
            .await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={out_of_range}: must be refused 400, got {refused:?}"
        );
    }
}
