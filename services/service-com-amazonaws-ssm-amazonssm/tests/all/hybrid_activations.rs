// SPDX-License-Identifier: CC0-1.0

//! Hybrid activations end to end: `CreateActivation` mints one, `DescribeActivations`
//! reports it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.
//! Nothing reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass: every read follows a create.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — which optional members the create carries, the registration limit,
//! the expiry it asks for, the tag count, the filter key, the page size, the Region —
//! rather than the single activation a recorded unit happened to create.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, REGION};

/// How long an activation lasts when the request names no expiry: "If you don't provide
/// an expiration date, the activation code expires in 24 hours"
/// (`smithy_query(query="doc", shape="CreateActivationRequest", member="ExpirationDate")`).
const DEFAULT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// How many nodes an activation registers when the request names no limit: "The default
/// value is 1"
/// (`smithy_query(query="doc", shape="CreateActivationRequest", member="RegistrationLimit")`).
const DEFAULT_REGISTRATION_LIMIT: i64 = 1;

/// The bounds the modelled `RegistrationLimit` carries
/// (`smithy_query(query="shape", shape="CreateActivationRequest")`: range 1..1000).
const MIN_REGISTRATION_LIMIT: i64 = 1;

/// See [`MIN_REGISTRATION_LIMIT`].
const MAX_REGISTRATION_LIMIT: i64 = 1000;

/// The bounds the modelled `ActivationCode` length carries
/// (`smithy_query(query="shape", shape="ActivationCode")`: length 20..250), and the form
/// Systems Manager mints one in: 20 characters of the base64 alphabet.
const CODE_LEN: usize = 20;

/// The alphabet an activation code is drawn from (base64, which needs no padding at this
/// length).
fn is_activation_code(candidate: &str) -> bool {
    candidate.len() == CODE_LEN
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// The bounds the modelled `MaxResults` of the activation report carries
/// (`smithy_query(query="operation", operation="DescribeActivations")`: range 1..50).
const MIN_PAGE_SIZE: i64 = 1;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 50;

/// The filter keys the modelled `DescribeActivationsFilterKeys` enum declares
/// (`smithy_query(query="shape", shape="DescribeActivationsFilterKeys")`).
const FILTER_KEYS: &[&str] = &["ActivationIds", "DefaultInstanceName", "IamRole"];

/// How far apart two instants may be for a test to call them the same moment: the
/// emulator stamps an activation from its own clock, which runs on between the call and
/// the assertion.
const CLOCK_SLACK: f64 = 60.0;

/// Does `candidate` satisfy the modelled `ActivationId` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`?
fn is_activation_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let lengths = [8usize, 4, 4, 4, 12];
    groups.len() == lengths.len()
        && groups.iter().zip(lengths).all(|(group, length)| {
            group.len() == length
                && group
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// Now, as the epoch seconds an awsJson1_1 timestamp travels as.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// `CreateActivation` in [`REGION`], returning `(status, body)`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "CreateActivation", body).await
}

/// `CreateActivation` that must succeed, returning `(ActivationId, ActivationCode)`.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> (String, String) {
    create_ok_in(router, REGION, case, body).await
}

/// [`create_ok`] at `region`'s endpoint.
async fn create_ok_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> (String, String) {
    let (status, response) = call_in_region(router, region, "CreateActivation", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateActivation must succeed, got {status} {response}"
    );
    let id = response["ActivationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateActivation must report an ActivationId, got {response}"))
        .to_string();
    let code = response["ActivationCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateActivation must report an ActivationCode, got {response}"))
        .to_string();
    assert!(
        is_activation_id(&id),
        "case {case:?}: the ActivationId must satisfy the modelled pattern, got {id:?}"
    );
    assert!(
        is_activation_code(&code),
        "case {case:?}: the ActivationCode must be {CODE_LEN} characters of the base64 \
         alphabet, got {} in {code:?}",
        code.len()
    );
    (id, code)
}

/// `DescribeActivations` in `region`, returning `(status, body)`.
async fn describe_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribeActivations", body).await
}

/// `DescribeActivations` that must succeed, returning its `ActivationList`.
async fn describe_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    describe_ok_in(router, REGION, case, body).await
}

/// [`describe_ok`] at `region`'s endpoint.
async fn describe_ok_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> Vec<Value> {
    let (status, response) = describe_in(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeActivations must succeed, got {status} {response}"
    );
    response["ActivationList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeActivations must report an ActivationList, got {response}"))
        .clone()
}

/// A request filtering the report by one key.
fn filter_body(key: &str, values: &[&str]) -> Value {
    json!({ "Filters": [{ "FilterKey": key, "FilterValues": values }] })
}

/// The one activation `id` names, read back through the report's own id filter.
async fn read_back(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    read_back_in(router, REGION, case, id).await
}

/// [`read_back`] at `region`'s endpoint.
async fn read_back_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    id: &str,
) -> Value {
    let reported = describe_ok_in(router, region, case, &filter_body("ActivationIds", &[id])).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the report must hold exactly the activation {id:?}, got {reported:?}"
    );
    reported[0].clone()
}

/// The `ActivationId` of every activation a report listed, in the order it listed them.
fn reported_ids(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|activation| {
            activation["ActivationId"]
                .as_str()
                .unwrap_or("<no ActivationId>")
                .to_string()
        })
        .collect()
}

/// The `Tags` a report carries, as a map.
fn reported_tags(activation: &Value) -> BTreeMap<String, String> {
    activation["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the round-trip table: a create, and what a read of it must report.
struct CreateCase {
    case: &'static str,
    description: Option<&'static str>,
    default_instance_name: Option<&'static str>,
    iam_role: &'static str,
    registration_limit: Option<i64>,
    /// Seconds from now the create asks the activation to expire at, or `None` to let
    /// the service apply the documented default.
    expires_in: Option<i64>,
    tags: &'static [(&'static str, &'static str)],
    /// Whether the create carries `RegistrationMetadata`, which is "reserved for
    /// internal use" and reported by no member of any response.
    registration_metadata: bool,
}

impl CreateCase {
    /// The request this row sends.
    fn body(&self, now: f64) -> Value {
        let mut body = Map::new();
        body.insert("IamRole".to_string(), json!(self.iam_role));
        if let Some(description) = self.description {
            body.insert("Description".to_string(), json!(description));
        }
        if let Some(name) = self.default_instance_name {
            body.insert("DefaultInstanceName".to_string(), json!(name));
        }
        if let Some(limit) = self.registration_limit {
            body.insert("RegistrationLimit".to_string(), json!(limit));
        }
        if let Some(seconds) = self.expires_in {
            body.insert(
                "ExpirationDate".to_string(),
                json!(now + seconds as f64),
            );
        }
        if !self.tags.is_empty() {
            let tags: Vec<Value> = self
                .tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect();
            body.insert("Tags".to_string(), Value::Array(tags));
        }
        if self.registration_metadata {
            body.insert(
                "RegistrationMetadata".to_string(),
                json!([{ "Key": "ManagedInstanceGroup", "Value": "reserved" }]),
            );
        }
        Value::Object(body)
    }
}

/// A 256-character `Description`, the longest the modelled length admits.
fn longest_description() -> String {
    "d".repeat(256)
}

/// A 64-character `IamRole` name, the longest the modelled length admits.
fn longest_role() -> String {
    "r".repeat(64)
}

/// The rows of the round-trip table: the optional members present and absent, the
/// registration limit at both published bounds and absent, the expiry asked for and
/// defaulted, and zero, one and three tags.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "only the required role",
            description: None,
            default_instance_name: None,
            iam_role: "SSMServiceRole",
            registration_limit: None,
            expires_in: None,
            tags: &[],
            registration_metadata: false,
        },
        CreateCase {
            case: "every optional member, one tag",
            description: Some("the edge devices in the lab"),
            default_instance_name: Some("lab-edge"),
            iam_role: "HybridRole",
            registration_limit: Some(7),
            expires_in: Some(60 * 60),
            tags: &[("Environment", "Production")],
            registration_metadata: true,
        },
        CreateCase {
            case: "the registration limit at its published minimum",
            description: Some("one node only"),
            default_instance_name: Some("single"),
            iam_role: "MinRole",
            registration_limit: Some(MIN_REGISTRATION_LIMIT),
            expires_in: Some(24 * 60 * 60),
            tags: &[],
            registration_metadata: false,
        },
        CreateCase {
            case: "the registration limit at its published maximum",
            description: Some("a fleet"),
            default_instance_name: Some("fleet"),
            iam_role: "MaxRole",
            registration_limit: Some(MAX_REGISTRATION_LIMIT),
            expires_in: Some(30 * 24 * 60 * 60),
            tags: &[
                ("OS", "Windows"),
                ("Environment", "Production"),
                ("Owner", "platform"),
            ],
            registration_metadata: false,
        },
        CreateCase {
            case: "a default instance name using every character class the pattern admits",
            description: Some(""),
            default_instance_name: Some("naïve node_1.2:3/4=5+6-7@8"),
            iam_role: "PatternRole",
            registration_limit: Some(2),
            expires_in: Some(15 * 60),
            tags: &[("tag-key-only", "")],
            registration_metadata: false,
        },
    ]
}

/// What a create persists is what a read reports: every member of the request comes back
/// through `DescribeActivations`, the service's own defaults fill the members the request
/// omitted, and the code the create minted is reported by no read.
#[tokio::test]
async fn a_created_activation_is_reported_as_it_was_created() {
    let router = router();

    for row in create_cases() {
        let case = row.case;
        let now = epoch_now();
        let (id, code) = create_ok(&router, case, &row.body(now)).await;
        let reported = read_back(&router, case, &id).await;

        assert_eq!(
            reported["ActivationId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the report must name the created activation, got {reported}"
        );
        assert_eq!(
            reported["IamRole"].as_str(),
            Some(row.iam_role),
            "case {case:?}: expected the role the create named, got {reported}"
        );
        assert_eq!(
            reported["Description"].as_str(),
            row.description,
            "case {case:?}: expected the description the create named, got {reported}"
        );
        assert_eq!(
            reported["DefaultInstanceName"].as_str(),
            row.default_instance_name,
            "case {case:?}: expected the default instance name the create named, got {reported}"
        );
        assert_eq!(
            reported["RegistrationLimit"].as_i64(),
            Some(row.registration_limit.unwrap_or(DEFAULT_REGISTRATION_LIMIT)),
            "case {case:?}: expected the registration limit the create named, or the \
             documented default, got {reported}"
        );
        assert_eq!(
            reported["RegistrationsCount"].as_i64(),
            Some(0),
            "case {case:?}: no node has registered with a new activation, got {reported}"
        );
        assert_eq!(
            reported["Expired"].as_bool(),
            Some(false),
            "case {case:?}: an activation that expires in the future is not expired, got \
             {reported}"
        );

        let created = reported["CreatedDate"].as_f64().unwrap_or_default();
        assert!(
            (created - now).abs() < CLOCK_SLACK,
            "case {case:?}: expected a created date around {now}, got {created} in {reported}"
        );
        let expires = reported["ExpirationDate"].as_f64().unwrap_or_default();
        let wanted = match row.expires_in {
            Some(seconds) => now + seconds as f64,
            None => created + DEFAULT_LIFETIME.as_secs_f64(),
        };
        assert!(
            (expires - wanted).abs() < CLOCK_SLACK,
            "case {case:?}: expected an expiration date around {wanted}, got {expires} in \
             {reported}"
        );

        let wanted_tags: BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported_tags(&reported),
            wanted_tags,
            "case {case:?}: expected the tags the create assigned, got {reported}"
        );

        let rendered = reported.to_string();
        assert!(
            !rendered.contains(code.as_str()),
            "case {case:?}: the activation code is the activation's password and must be \
             reported by no read, found it in {rendered}"
        );
    }
}

/// The longest values the modelled lengths admit round-trip unchanged, so the echo above
/// is not an artefact of short values.
#[tokio::test]
async fn the_longest_modelled_values_round_trip() {
    let router = router();
    let description = longest_description();
    let role = longest_role();

    let case = "a 256-character description and a 64-character role name";
    let (id, _) = create_ok(
        &router,
        case,
        &json!({ "IamRole": role, "Description": description }),
    )
    .await;
    let reported = read_back(&router, case, &id).await;
    assert_eq!(
        reported["Description"].as_str(),
        Some(description.as_str()),
        "case {case:?}: expected the description the create named, got {reported}"
    );
    assert_eq!(
        reported["IamRole"].as_str(),
        Some(role.as_str()),
        "case {case:?}: expected the role the create named, got {reported}"
    );
}

/// Every create mints its OWN id and its own code: neither is derived from the request,
/// so two creates carrying identical inputs are two activations.
#[tokio::test]
async fn every_create_mints_a_fresh_id_and_code() {
    let router = router();
    let request = json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": "same-input" });

    let mut ids = BTreeSet::new();
    let mut codes = BTreeSet::new();
    for attempt in 1..=8 {
        let case = format!("create {attempt} of 8 with identical input");
        let (id, code) = create_ok(&router, &case, &request).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: the activation id {id:?} was already minted for another activation"
        );
        assert!(
            codes.insert(code.clone()),
            "case {case:?}: the activation code was already minted for another activation"
        );
        let reported = read_back(&router, &case, &id).await;
        assert_eq!(
            reported["DefaultInstanceName"].as_str(),
            Some("same-input"),
            "case {case:?}: expected the stored activation, got {reported}"
        );
    }

    let case = "an unfiltered report after 8 creates";
    let reported = describe_ok(&router, case, &json!({})).await;
    assert_eq!(
        reported_ids(&reported).into_iter().collect::<BTreeSet<_>>(),
        ids,
        "case {case:?}: the report must hold every activation that was created, got \
         {reported:?}"
    );
}

/// Three activations whose filterable properties overlap, for the filter table below:
/// `(id, iam role, default instance name)`.
async fn seeded(router: &MultiServiceRouter) -> Vec<(String, &'static str, &'static str)> {
    let rows: &[(&'static str, &'static str)] = &[
        ("SharedRole", "alpha"),
        ("SharedRole", "beta"),
        ("OtherRole", "alpha"),
    ];
    let mut seeded = Vec::new();
    for (role, name) in rows {
        let case = format!("seed {role}/{name}");
        let (id, _) = create_ok(
            router,
            &case,
            &json!({ "IamRole": role, "DefaultInstanceName": name }),
        )
        .await;
        seeded.push((id, *role, *name));
    }
    seeded
}

/// A filter selects the activations whose property equals one of its values: the values
/// of one filter are alternatives, and two filters narrow together.
#[tokio::test]
async fn filters_select_by_the_property_they_name() {
    let router = router();
    let seeded = seeded(&router).await;
    let id = |index: usize| seeded[index].0.clone();

    // (case, filters, the ids the report must hold)
    let cases: Vec<(String, Value, Vec<String>)> = vec![
        (
            "no filter at all".to_string(),
            json!({}),
            vec![id(0), id(1), id(2)],
        ),
        (
            "one id".to_string(),
            filter_body("ActivationIds", &[&id(1)]),
            vec![id(1)],
        ),
        (
            "two ids, which are alternatives".to_string(),
            filter_body("ActivationIds", &[&id(0), &id(2)]),
            vec![id(0), id(2)],
        ),
        (
            "a role two activations share".to_string(),
            filter_body("IamRole", &["SharedRole"]),
            vec![id(0), id(1)],
        ),
        (
            "a role one activation has".to_string(),
            filter_body("IamRole", &["OtherRole"]),
            vec![id(2)],
        ),
        (
            "a default instance name two activations share".to_string(),
            filter_body("DefaultInstanceName", &["alpha"]),
            vec![id(0), id(2)],
        ),
        (
            "two filters, which narrow together".to_string(),
            json!({ "Filters": [
                { "FilterKey": "IamRole", "FilterValues": ["SharedRole"] },
                { "FilterKey": "DefaultInstanceName", "FilterValues": ["alpha"] },
            ] }),
            vec![id(0)],
        ),
        (
            "two filters that no activation satisfies together".to_string(),
            json!({ "Filters": [
                { "FilterKey": "IamRole", "FilterValues": ["OtherRole"] },
                { "FilterKey": "DefaultInstanceName", "FilterValues": ["beta"] },
            ] }),
            Vec::new(),
        ),
    ];

    for (case, body, wanted) in cases {
        let reported = describe_ok(&router, &case, &body).await;
        let mut got = reported_ids(&reported);
        got.sort();
        let mut wanted = wanted;
        wanted.sort();
        assert_eq!(
            got, wanted,
            "case {case:?}: expected exactly those activations, got {reported:?}"
        );
    }

    // A value no activation holds, and a filter carrying no value at all, select nothing
    // — on every key the model declares, not only the ones cited above.
    for key in FILTER_KEYS {
        for (what, values) in [
            ("a value no activation holds", &["00000000-0000-0000-0000-000000000000"][..]),
            ("no values at all", &[][..]),
        ] {
            let case = format!("{key} with {what}");
            let reported = describe_ok(&router, &case, &filter_body(key, values)).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: expected no activation, got {reported:?}"
            );
        }
    }
}

/// A filter the operation cannot serve is refused, and so is a page cursor it never
/// issued and a page size outside the modelled range — each with its own status and code,
/// rather than an empty report that looks like "no activations".
#[tokio::test]
async fn an_unservable_request_is_refused() {
    let router = router();
    let (id, _) = create_ok(
        &router,
        "the activation the refusals are asked about",
        &json!({ "IamRole": "SSMServiceRole" }),
    )
    .await;

    // (case, request, the wire error code the refusal must carry)
    let cases: Vec<(String, Value, &str)> = vec![
        (
            "a filter naming no key".to_string(),
            json!({ "Filters": [{ "FilterValues": [id.clone()] }] }),
            "InvalidFilter",
        ),
        (
            "a filter key outside the modelled enum".to_string(),
            filter_body("Tags", &["Environment"]),
            "ValidationException",
        ),
        (
            "a filter key differing only in case".to_string(),
            filter_body("iamRole", &["SSMServiceRole"]),
            "ValidationException",
        ),
        (
            "a filter key naming the id member rather than the filter".to_string(),
            filter_body("ActivationId", &[&id]),
            "ValidationException",
        ),
        (
            "a next token this service never issued".to_string(),
            json!({ "NextToken": "not-an-activation-id" }),
            "InvalidNextToken",
        ),
        (
            "a next token shaped like an id but uppercase".to_string(),
            json!({ "NextToken": "AAAAAAAA-0000-0000-0000-000000000000" }),
            "InvalidNextToken",
        ),
        (
            format!("a page size one below the published minimum of {MIN_PAGE_SIZE}"),
            json!({ "MaxResults": MIN_PAGE_SIZE - 1 }),
            "ValidationException",
        ),
        (
            format!("a page size one above the published maximum of {MAX_PAGE_SIZE}"),
            json!({ "MaxResults": MAX_PAGE_SIZE + 1 }),
            "ValidationException",
        ),
    ];

    for (case, body, wanted) in cases {
        let (status, response) = describe_in(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            wanted,
            "case {case:?}: expected {wanted}, got {response}"
        );
    }

    // Both published page-size bounds are SERVED, so the refusals above report the range
    // the report actually enforces.
    for size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
        let case = format!("a page size of {size}, a published bound");
        let reported = describe_ok(&router, &case, &json!({ "MaxResults": size })).await;
        assert_eq!(
            reported_ids(&reported),
            vec![id.clone()],
            "case {case:?}: expected the one activation, got {reported:?}"
        );
    }
}

/// Paging walks every activation exactly once, in one total order, whatever page size the
/// caller asks for: the token is where the next page resumes, and the last page carries
/// none.
#[tokio::test]
async fn paging_walks_every_activation_exactly_once() {
    let router = router();
    let mut all = BTreeSet::new();
    for index in 0..7 {
        let case = format!("activation {index} of 7");
        let (id, _) = create_ok(
            &router,
            &case,
            &json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": format!("node-{index}") }),
        )
        .await;
        all.insert(id);
    }
    let all: Vec<String> = all.into_iter().collect();

    for size in [1i64, 2, 3, 7, MAX_PAGE_SIZE] {
        let case = format!("paging {} activations {size} at a time", all.len());
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=all.len() {
            let mut body = Map::new();
            body.insert("MaxResults".to_string(), json!(size));
            if let Some(token) = token.clone() {
                body.insert("NextToken".to_string(), json!(token));
            }
            let (status, response) =
                describe_in(&router, REGION, &Value::Object(body.clone())).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {page} must succeed, got {status} {response}"
            );
            let reported = response["ActivationList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                reported.len() as i64 <= size,
                "case {case:?}: page {page} must hold at most {size} activations, got {reported:?}"
            );
            walked.extend(reported_ids(&reported));
            token = response["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end without a token, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: paging must report every activation once, in ascending id order"
        );
    }
}

/// An activation belongs to the Region it was created at: a report at another Region's
/// endpoint does not hold it, even when it is asked for by id.
#[tokio::test]
async fn an_activation_is_reported_only_at_the_region_it_was_created_at() {
    let router = router();
    let regions = [REGION, "eu-central-1", "ap-southeast-2"];

    let mut created = Vec::new();
    for region in regions {
        let case = format!("the activation created at {region}");
        let (id, _) = create_ok_in(
            &router,
            region,
            &case,
            &json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": region }),
        )
        .await;
        created.push((region, id));
    }

    for (region, id) in &created {
        let case = format!("the report at {region}");
        let reported = describe_ok_in(&router, region, &case, &json!({})).await;
        assert_eq!(
            reported_ids(&reported),
            vec![id.clone()],
            "case {case:?}: expected only that Region's activation, got {reported:?}"
        );
        let reported = read_back_in(&router, region, &case, id).await;
        assert_eq!(
            reported["DefaultInstanceName"].as_str(),
            Some(*region),
            "case {case:?}: expected the activation created there, got {reported}"
        );

        for (other, _) in &created {
            if other == region {
                continue;
            }
            let case = format!("{region}'s activation asked for at {other}");
            let reported =
                describe_ok_in(&router, other, &case, &filter_body("ActivationIds", &[id])).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: another Region's activation must not be reported, got \
                 {reported:?}"
            );
        }
    }
}

/// `Expired` follows the clock against the stored expiration date: an activation whose
/// date has passed reports as expired without any call having moved it, and everything
/// else about it is unchanged.
#[tokio::test]
async fn an_activation_expires_when_its_date_passes() {
    let router = router();
    let now = epoch_now();

    // One row per side of the stored date: a date the clock reaches during this test, and
    // one it does not.
    let short = "an activation expiring in a second";
    let (short_id, _) = create_ok(
        &router,
        short,
        &json!({ "IamRole": "ShortRole", "ExpirationDate": now + 1.0 }),
    )
    .await;
    let long = "an activation expiring in a day";
    let (long_id, _) = create_ok(
        &router,
        long,
        &json!({ "IamRole": "LongRole", "ExpirationDate": now + DEFAULT_LIFETIME.as_secs_f64() }),
    )
    .await;

    let before = read_back(&router, short, &short_id).await;
    assert_eq!(
        before["Expired"].as_bool(),
        Some(false),
        "case {short:?}: an activation whose date is still ahead is not expired, got {before}"
    );

    tokio::time::sleep(Duration::from_millis(1_400)).await;

    let after = read_back(&router, short, &short_id).await;
    assert_eq!(
        after["Expired"].as_bool(),
        Some(true),
        "case {short:?}: an activation whose date has passed is expired, got {after}"
    );
    assert_eq!(
        after["ExpirationDate"], before["ExpirationDate"],
        "case {short:?}: expiring moves no date, got {after}"
    );
    assert_eq!(
        after["CreatedDate"], before["CreatedDate"],
        "case {short:?}: expiring moves no date, got {after}"
    );
    assert_eq!(
        after["RegistrationsCount"].as_i64(),
        Some(0),
        "case {short:?}: expiring registers nothing, got {after}"
    );

    let reported = read_back(&router, long, &long_id).await;
    assert_eq!(
        reported["Expired"].as_bool(),
        Some(false),
        "case {long:?}: another activation's expiry does not expire this one, got {reported}"
    );
}

/// A create outside a modelled constraint is refused rather than persisted, so the
/// echo above is of values the operation actually admits.
///
/// Each row violates one constraint the model carries
/// (`smithy_query(query="operation", operation="CreateActivation")`): the required role,
/// the two lengths, the pattern and the registration range.
#[tokio::test]
async fn a_create_outside_the_modelled_constraints_is_refused() {
    let router = router();

    let cases: Vec<(String, Value)> = vec![
        (
            "no IamRole at all, which the model requires".to_string(),
            json!({ "DefaultInstanceName": "roleless" }),
        ),
        (
            "an IamRole one character past its length of 64".to_string(),
            json!({ "IamRole": "r".repeat(65) }),
        ),
        (
            "a Description one character past its length of 256".to_string(),
            json!({ "IamRole": "SSMServiceRole", "Description": "d".repeat(257) }),
        ),
        (
            "a DefaultInstanceName one character past its length of 256".to_string(),
            json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": "n".repeat(257) }),
        ),
        (
            "a DefaultInstanceName the modelled pattern excludes".to_string(),
            json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": "no!" }),
        ),
        (
            format!("a registration limit one below the published minimum of {MIN_REGISTRATION_LIMIT}"),
            json!({ "IamRole": "SSMServiceRole", "RegistrationLimit": MIN_REGISTRATION_LIMIT - 1 }),
        ),
        (
            format!("a registration limit one above the published maximum of {MAX_REGISTRATION_LIMIT}"),
            json!({ "IamRole": "SSMServiceRole", "RegistrationLimit": MAX_REGISTRATION_LIMIT + 1 }),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }

    let case = "the report after every create was refused";
    let reported = describe_ok(&router, case, &json!({})).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: a refused create must persist nothing, got {reported:?}"
    );
}

/// A read changes nothing a later caller sees: two reports of the same store report the
/// same activation, however many times it is read.
#[tokio::test]
async fn reading_the_report_changes_nothing() {
    let router = router();
    let case = "the activation two reports read";
    let (id, _) = create_ok(&router, case, &json!({ "IamRole": "SSMServiceRole" })).await;

    let first = read_back(&router, case, &id).await;
    for repeat in 1..=3 {
        let case = format!("read {repeat} of 3");
        let again = read_back(&router, &case, &id).await;
        assert_eq!(
            again, first,
            "case {case:?}: a read must report what the previous read did"
        );
    }
}

/// A delete removes exactly the activation its id names, at the Region it was created
/// at, and leaves every other activation reportable; a second delete of the same id, and
/// a delete of an id nothing was created under, are refused as an invalid activation.
#[tokio::test]
async fn deleting_an_activation_removes_exactly_that_one() {
    let router = router();
    let regions = [REGION, "eu-central-1"];

    let mut created: Vec<(&str, String)> = Vec::new();
    for region in regions {
        for index in 0..3 {
            let case = format!("activation {index} at {region}");
            let (id, _) = create_ok_in(
                &router,
                region,
                &case,
                &json!({ "IamRole": "SSMServiceRole", "DefaultInstanceName": format!("node-{index}") }),
            )
            .await;
            created.push((region, id));
        }
    }

    // Deleting walks the rows one at a time: after each delete the store must hold
    // exactly the activations that were not deleted yet.
    for position in 0..created.len() {
        let (region, id) = created[position].clone();
        let case = format!("deleting {id} at {region}");
        let (status, response) =
            call_in_region(&router, region, "DeleteActivation", &json!({ "ActivationId": id }))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteActivation must succeed, got {status} {response}"
        );

        for (other_region, other_id) in &created[position + 1..] {
            let case = format!("{other_id} at {other_region} after {case}");
            let reported = read_back_in(&router, other_region, &case, other_id).await;
            assert_eq!(
                reported["ActivationId"].as_str(),
                Some(other_id.as_str()),
                "case {case:?}: an activation nobody deleted must still be reported, got \
                 {reported}"
            );
        }
        let case = format!("the report of {id} after {case}");
        let reported =
            describe_ok_in(&router, region, &case, &filter_body("ActivationIds", &[&id])).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a deleted activation must be reported by nothing, got {reported:?}"
        );
    }

    // (case, the id the delete names, the Region it is sent to)
    let (live_region, live_id) = {
        let case = "the activation the refusals are asked about";
        let (id, _) = create_ok(&router, case, &json!({ "IamRole": "SSMServiceRole" })).await;
        (REGION, id)
    };
    let refused: Vec<(String, &str, String)> = vec![
        (
            "an id nothing was ever created under".to_string(),
            REGION,
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an id that was already deleted".to_string(),
            created[0].0,
            created[0].1.clone(),
        ),
        (
            "a live activation of another Region".to_string(),
            "eu-central-1",
            live_id.clone(),
        ),
    ];
    for (case, region, id) in refused {
        let (status, response) =
            call_in_region(&router, region, "DeleteActivation", &json!({ "ActivationId": id }))
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidActivation",
            "case {case:?}: expected InvalidActivation, got {response}"
        );
        // Systems Manager states this failure with the code alone, carrying no message
        // of its own.
        let message = response["message"]
            .as_str()
            .or_else(|| response["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.is_empty(),
            "case {case:?}: expected no message beside the code, got {message:?}"
        );
    }

    // The refusal above is about the activation, not the request: the live activation is
    // still there to be read, and deleted, at its own Region.
    let case = "the live activation after another Region refused to delete it";
    let reported = read_back_in(&router, live_region, case, &live_id).await;
    assert_eq!(
        reported["ActivationId"].as_str(),
        Some(live_id.as_str()),
        "case {case:?}: expected the activation to survive, got {reported}"
    );

    // A malformed id violates the modelled `ActivationId` pattern, so it is refused as a
    // bad request rather than as a missing activation.
    for malformed in [
        "not-a-uuid",
        "0000000000000000000000000000000000000",
        "00000000-0000-0000-0000-00000000000",
        "0000000G-0000-0000-0000-000000000000",
        "00000000-0000-0000-0000-000000000000-0000",
    ] {
        let case = format!("the malformed activation id {malformed:?}");
        let (status, response) = call(
            &router,
            "DeleteActivation",
            &json!({ "ActivationId": malformed }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}
