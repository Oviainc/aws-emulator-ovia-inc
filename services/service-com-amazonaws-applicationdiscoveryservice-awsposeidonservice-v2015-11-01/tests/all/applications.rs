// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the application-configuration lifecycle: `CreateApplication`,
//! `ListConfigurations`, `DescribeConfigurations`, `UpdateApplication`,
//! `DeleteApplications` and `GetDiscoverySummary`, driven over the wire
//! (`awsJson1_1`, `POST /` + `X-Amz-Target`) through the same router the emulator
//! serves from.
//!
//! Each test states a RULE and asserts it over a set of inputs; the happy path from
//! the bootstrap issue is one row of several.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_applicationdiscoveryservice_awsposeidonservice_v2015_11_01::state::APPLICATION_QUOTA;
use service_com_amazonaws_applicationdiscoveryservice_awsposeidonservice_v2015_11_01::EmulatedService;

/// The `X-Amz-Target` prefix from the model's service shape (`AWSPoseidonService_V2015_11_01`).
const TARGET_PREFIX: &str = "AWSPoseidonService_V2015_11_01";

/// One emulator: a shared engine, the service mounted on the real router.
struct Discovery {
    router: MultiServiceRouter,
}

impl Discovery {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return its HTTP status and JSON body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let payload = serde_json::to_vec(body).expect("serialize the request body");
        let request = http::Request::builder()
            .method("POST")
            .uri("https://discovery.us-east-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            // The router keys on the SigV4 credential scope (`discovery`). Request
            // authentication is off above, so the signature itself is never read.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEMULATOR/20260101/us-east-1/discovery/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=00",
            )
            .body(bytes::Bytes::from(payload))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let value: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, value)
    }

    /// `CreateApplication`, returning the minted configuration id.
    async fn create(&self, request: &Value) -> String {
        let (status, body) = self.call("CreateApplication", request).await;
        assert_eq!(
            status, 200,
            "case {request:?}: CreateApplication must succeed, got {status} {body:?}"
        );
        body["configurationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: no configurationId in {body:?}"))
            .to_string()
    }

    /// Every `APPLICATION` configuration, in one page.
    async fn list_applications(&self) -> Vec<BTreeMap<String, String>> {
        let (status, body) = self
            .call(
                "ListConfigurations",
                &json!({"configurationType": "APPLICATION"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "ListConfigurations(APPLICATION) must succeed, got {status} {body:?}"
        );
        configurations(&body["configurations"])
    }

    /// `DescribeConfigurations` over `ids`.
    async fn describe(&self, ids: &[&str]) -> Vec<BTreeMap<String, String>> {
        let (status, body) = self
            .call(
                "DescribeConfigurations",
                &json!({"configurationIds": ids}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {ids:?}: DescribeConfigurations must succeed, got {status} {body:?}"
        );
        configurations(&body["configurations"])
    }

    /// The `applications` count `GetDiscoverySummary` reports.
    async fn summary(&self) -> Value {
        let (status, body) = self.call("GetDiscoverySummary", &json!({})).await;
        assert_eq!(
            status, 200,
            "GetDiscoverySummary must succeed, got {status} {body:?}"
        );
        body
    }
}

/// One `CreateApplication` case: a label, the request, and the flat attributes the
/// stored record must report beyond its own id.
type CreateCase = (&'static str, Value, Vec<(&'static str, String)>);

/// What a record must read as after an update: name, description, wave.
type UpdatedRecord = (&'static str, Option<&'static str>, Option<&'static str>);

/// One `UpdateApplication` case: a label, the patch it sends, and the record that must
/// follow.
type UpdateCase = (&'static str, Value, UpdatedRecord);

/// A `configurations` list as comparable maps.
fn configurations(value: &Value) -> Vec<BTreeMap<String, String>> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("configurations must be a list, got {value:?}"))
        .iter()
        .map(|entry| {
            entry
                .as_object()
                .unwrap_or_else(|| panic!("a configuration must be a map, got {entry:?}"))
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .unwrap_or_else(|| panic!("attribute {k} must be a string: {v:?}"))
                            .to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// The `application.configurationId` of each row, in order.
fn ids_of(rows: &[BTreeMap<String, String>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.get("application.configurationId")
                .unwrap_or_else(|| panic!("row without application.configurationId: {row:?}"))
                .clone()
        })
        .collect()
}

/// The wire error code (`__type`) of an error body, without its namespace.
fn error_code(body: &Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("error body without __type: {body:?}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

#[tokio::test]
async fn application_lifecycle_round_trips_through_persisted_state() {
    let ads = Discovery::new();

    let app1 = ads
        .create(&json!({"name": "migration-wave-1", "description": "first wave"}))
        .await;
    let app2 = ads.create(&json!({"name": "migration-wave-2"})).await;
    assert_ne!(
        app1, app2,
        "two creates must mint distinct ids, got {app1} twice"
    );

    let listed = ids_of(&ads.list_applications().await);
    assert_eq!(
        listed,
        vec![app1.clone(), app2.clone()],
        "both created applications must be listed, got {listed:?}"
    );

    let described = ads.describe(&[&app1]).await;
    assert_eq!(described.len(), 1, "one id, one row: {described:?}");
    assert_eq!(
        described[0].get("application.name").map(String::as_str),
        Some("migration-wave-1"),
        "case {app1}: stored name must be reported, got {:?}",
        described[0]
    );
    assert_eq!(
        described[0]
            .get("application.description")
            .map(String::as_str),
        Some("first wave"),
        "case {app1}: stored description must be reported, got {:?}",
        described[0]
    );

    let (status, body) = ads
        .call(
            "UpdateApplication",
            &json!({"configurationId": app1, "name": "migration-wave-1b"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateApplication must succeed, got {status} {body:?}"
    );

    let described = ads.describe(&[&app1]).await;
    assert_eq!(
        described[0].get("application.name").map(String::as_str),
        Some("migration-wave-1b"),
        "case {app1}: the update must be observable, got {:?}",
        described[0]
    );
    assert_eq!(
        described[0]
            .get("application.description")
            .map(String::as_str),
        Some("first wave"),
        "case {app1}: an absent member must keep its stored value, got {:?}",
        described[0]
    );

    assert_eq!(
        ads.summary().await["applications"],
        json!(2),
        "two persisted applications must be counted as 2"
    );

    let (status, body) = ads
        .call("DeleteApplications", &json!({"configurationIds": [app1]}))
        .await;
    assert_eq!(
        status, 200,
        "DeleteApplications must succeed, got {status} {body:?}"
    );
    assert!(
        ads.describe(&[&app1]).await.is_empty(),
        "case {app1}: a deleted application must not be described"
    );
    assert_eq!(
        ids_of(&ads.list_applications().await),
        vec![app2.clone()],
        "case {app1}: a deleted application must not be listed"
    );
    assert_eq!(
        ads.summary().await["applications"],
        json!(1),
        "one remaining application must be counted as 1"
    );
}

#[tokio::test]
async fn a_created_application_reports_exactly_the_members_it_was_created_with() {
    // The class the rule generalises over: which optional members are supplied, and
    // the size/shape of their values (the model's own @length bounds for
    // ApplicationName 0..127, ApplicationDescription 0..1000, ApplicationWave 0..256).
    let long_name = "n".repeat(127);
    let long_description = "d".repeat(1000);
    let long_wave = "w".repeat(256);
    let cases: Vec<CreateCase> = vec![
        (
            "name only",
            json!({"name": "alpha"}),
            vec![("application.name", "alpha".to_string())],
        ),
        (
            "name and description",
            json!({"name": "beta", "description": "the beta wave"}),
            vec![
                ("application.name", "beta".to_string()),
                ("application.description", "the beta wave".to_string()),
            ],
        ),
        (
            "name and wave",
            json!({"name": "gamma", "wave": "wave 3"}),
            vec![
                ("application.name", "gamma".to_string()),
                ("application.wave", "wave 3".to_string()),
            ],
        ),
        (
            "every member",
            json!({"name": "delta", "description": "all three", "wave": "wave-4"}),
            vec![
                ("application.name", "delta".to_string()),
                ("application.description", "all three".to_string()),
                ("application.wave", "wave-4".to_string()),
            ],
        ),
        (
            "non-ascii name",
            json!({"name": "wäve-ünö 🚚", "description": "ünïcödé"}),
            vec![
                ("application.name", "wäve-ünö 🚚".to_string()),
                ("application.description", "ünïcödé".to_string()),
            ],
        ),
        (
            "members at their modeled maximum length",
            json!({"name": long_name, "description": long_description, "wave": long_wave}),
            vec![
                ("application.name", long_name.clone()),
                ("application.description", long_description.clone()),
                ("application.wave", long_wave.clone()),
            ],
        ),
        (
            "name carrying inner whitespace",
            json!({"name": "two   words", "description": "a  b  c"}),
            vec![
                ("application.name", "two   words".to_string()),
                ("application.description", "a  b  c".to_string()),
            ],
        ),
    ];

    let ads = Discovery::new();
    for (case, request, expected_members) in cases {
        let id = ads.create(&request).await;
        assert!(
            id.starts_with("d-application-"),
            "case {case}: a minted id must be an ADS application id, got {id}"
        );

        let mut expected: BTreeMap<String, String> = expected_members
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        expected.insert("application.configurationId".to_string(), id.clone());

        let described = ads.describe(&[&id]).await;
        assert_eq!(
            described.len(),
            1,
            "case {case}: exactly one row for one id, got {described:?}"
        );
        assert_eq!(
            described[0], expected,
            "case {case}: DescribeConfigurations must report the stored members, got {:?}",
            described[0]
        );

        // Both readers flatten through the same helper, so the same record reports
        // the same keys and values whichever operation asks.
        let listed = ads.list_applications().await;
        let from_list = listed
            .iter()
            .find(|row| row.get("application.configurationId") == Some(&id))
            .unwrap_or_else(|| panic!("case {case}: {id} missing from the listing: {listed:?}"));
        assert_eq!(
            from_list, &expected,
            "case {case}: ListConfigurations must report the same map as DescribeConfigurations, got {from_list:?}"
        );
    }
}

#[tokio::test]
async fn minted_ids_are_unique_and_never_echo_the_request() {
    let ads = Discovery::new();
    let mut ids = Vec::new();
    // Same name every time: the id cannot be derived from the request.
    for index in 0..25 {
        let id = ads.create(&json!({"name": "identical-name"})).await;
        assert!(
            id.starts_with("d-application-") && id.len() > "d-application-".len(),
            "case {index}: id must be d-application-<generated>, got {id}"
        );
        assert!(
            !id.contains("identical-name"),
            "case {index}: id must not echo the request, got {id}"
        );
        ids.push(id);
    }
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every minted id must be unique, got {ids:?}"
    );
}

#[tokio::test]
async fn paging_partitions_the_listing_exactly_once() {
    // (applications created, maxResults asked for). 0 is the member's modeled
    // default: unset, so the whole listing comes back in one page.
    let cases = [(1_usize, 1_i32), (3, 1), (3, 2), (5, 5), (5, 100), (4, 0), (7, 3)];

    for (count, max_results) in cases {
        let ads = Discovery::new();
        let mut created = Vec::new();
        for index in 0..count {
            created.push(ads.create(&json!({"name": format!("app-{index}")})).await);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({"configurationType": "APPLICATION"});
            if max_results != 0 {
                request["maxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = ads.call("ListConfigurations", &request).await;
            assert_eq!(
                status, 200,
                "case ({count}, {max_results}): page {pages} must succeed, got {status} {body:?}"
            );
            let page = ids_of(&configurations(&body["configurations"]));
            if max_results != 0 {
                assert!(
                    page.len() <= max_results as usize,
                    "case ({count}, {max_results}): a page must not exceed maxResults, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            next_token = body["nextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= count + 1,
                "case ({count}, {max_results}): paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, created,
            "case ({count}, {max_results}): the pages must cover every application exactly once, in order"
        );
        let page_size = if max_results == 0 {
            count
        } else {
            max_results as usize
        };
        let expected_pages = count.div_ceil(page_size.max(1)).max(1);
        assert_eq!(
            pages, expected_pages,
            "case ({count}, {max_results}): expected {expected_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn max_results_is_honoured_up_to_its_documented_bound_and_refused_past_it() {
    // ListConfigurationsRequest$maxResults: "The total number of items to return. The
    // maximum value is 100." Both published bounds, and one past the top.
    let cases = [(0_i32, Some(3_usize)), (1, Some(1)), (2, Some(2)), (100, Some(3)), (101, None), (-1, None)];

    let ads = Discovery::new();
    for index in 0..3 {
        ads.create(&json!({"name": format!("app-{index}")})).await;
    }

    for (max_results, expected_len) in cases {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({"configurationType": "APPLICATION", "maxResults": max_results}),
            )
            .await;
        match expected_len {
            Some(len) => {
                assert_eq!(
                    status, 200,
                    "case maxResults={max_results}: must be accepted, got {status} {body:?}"
                );
                assert_eq!(
                    configurations(&body["configurations"]).len(),
                    len,
                    "case maxResults={max_results}: expected {len} rows, got {body:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case maxResults={max_results}: must be refused, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValueException",
                    "case maxResults={max_results}: wrong error code in {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn filters_select_rows_by_their_condition() {
    let ads = Discovery::new();
    let web = ads.create(&json!({"name": "web-tier", "wave": "wave-1"})).await;
    let app = ads.create(&json!({"name": "app-tier", "wave": "wave-1"})).await;
    let db = ads.create(&json!({"name": "db-tier", "wave": "wave-2"})).await;

    // Every condition `Filter$condition` documents, over two different attributes
    // and both one-value and multi-value filters.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "EQUALS on one name",
            json!([{"name": "application.name", "values": ["web-tier"], "condition": "EQUALS"}]),
            vec![web.clone()],
        ),
        (
            "EQUALS on two names",
            json!([{"name": "application.name", "values": ["web-tier", "db-tier"], "condition": "EQUALS"}]),
            vec![web.clone(), db.clone()],
        ),
        (
            "NOT_EQUALS on one name",
            json!([{"name": "application.name", "values": ["web-tier"], "condition": "NOT_EQUALS"}]),
            vec![app.clone(), db.clone()],
        ),
        (
            "CONTAINS on a substring",
            json!([{"name": "application.name", "values": ["-tier"], "condition": "CONTAINS"}]),
            vec![web.clone(), app.clone(), db.clone()],
        ),
        (
            "NOT_CONTAINS on a substring",
            json!([{"name": "application.name", "values": ["db"], "condition": "NOT_CONTAINS"}]),
            vec![web.clone(), app.clone()],
        ),
        (
            "EQUALS on another attribute",
            json!([{"name": "application.wave", "values": ["wave-1"], "condition": "EQUALS"}]),
            vec![web.clone(), app.clone()],
        ),
        (
            "two filters are ANDed",
            json!([
                {"name": "application.wave", "values": ["wave-1"], "condition": "EQUALS"},
                {"name": "application.name", "values": ["app"], "condition": "CONTAINS"},
            ]),
            vec![app.clone()],
        ),
        (
            "an attribute no row carries",
            json!([{"name": "application.description", "values": ["anything"], "condition": "EQUALS"}]),
            vec![],
        ),
        (
            "the negation of an attribute no row carries",
            json!([{"name": "application.description", "values": ["anything"], "condition": "NOT_EQUALS"}]),
            vec![web.clone(), app.clone(), db.clone()],
        ),
    ];

    for (case, filters, expected) in cases {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({"configurationType": "APPLICATION", "filters": filters}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: must succeed, got {status} {body:?}"
        );
        let got = ids_of(&configurations(&body["configurations"]));
        assert_eq!(got, expected, "case {case}: wrong rows selected, got {got:?}");
    }

    // A condition outside the documented set is a bad parameter value, not a stub.
    // The documented operators are upper case, and are matched exactly.
    for condition in ["STARTS_WITH", "equals", ""] {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({
                    "configurationType": "APPLICATION",
                    "filters": [{"name": "application.name", "values": ["web-tier"], "condition": condition}],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case condition {condition:?}: must be refused, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert!(
            code == "InvalidParameterValueException" || code == "ValidationException",
            "case condition {condition:?}: expected a client error code, got {code} in {body:?}"
        );
    }
}

#[tokio::test]
async fn order_by_sorts_on_the_named_attribute_in_the_named_direction() {
    let ads = Discovery::new();
    // `alpha` deliberately carries NO description, so the absent-attribute rule is
    // exercised beside the present ones.
    let beta = ads
        .create(&json!({"name": "beta", "wave": "wave-3", "description": "b"}))
        .await;
    let alpha = ads.create(&json!({"name": "alpha", "wave": "wave-2"})).await;
    let gamma = ads
        .create(&json!({"name": "gamma", "wave": "wave-1", "description": "a"}))
        .await;

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "name ascending",
            json!([{"fieldName": "application.name", "sortOrder": "ASC"}]),
            vec![alpha.clone(), beta.clone(), gamma.clone()],
        ),
        (
            "name descending",
            json!([{"fieldName": "application.name", "sortOrder": "DESC"}]),
            vec![gamma.clone(), beta.clone(), alpha.clone()],
        ),
        (
            "wave ascending",
            json!([{"fieldName": "application.wave", "sortOrder": "ASC"}]),
            vec![gamma.clone(), alpha.clone(), beta.clone()],
        ),
        (
            "sortOrder omitted defaults to ascending",
            json!([{"fieldName": "application.name"}]),
            vec![alpha.clone(), beta.clone(), gamma.clone()],
        ),
        (
            "a row without the attribute sorts last, ascending",
            json!([{"fieldName": "application.description", "sortOrder": "ASC"}]),
            vec![gamma.clone(), beta.clone(), alpha.clone()],
        ),
        (
            "a row without the attribute sorts last, descending",
            json!([{"fieldName": "application.description", "sortOrder": "DESC"}]),
            vec![beta.clone(), gamma.clone(), alpha.clone()],
        ),
        (
            "an attribute no row carries falls back to configuration-id order",
            json!([{"fieldName": "application.serverCount", "sortOrder": "DESC"}]),
            vec![beta.clone(), alpha.clone(), gamma.clone()],
        ),
    ];

    for (case, order_by, expected) in cases {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({"configurationType": "APPLICATION", "orderBy": order_by}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: must succeed, got {status} {body:?}"
        );
        let got = ids_of(&configurations(&body["configurations"]));
        assert_eq!(got, expected, "case {case}: wrong order, got {got:?}");
    }

    // Ordering also decides the page boundaries, so a paged ordered walk matches.
    let (status, body) = ads
        .call(
            "ListConfigurations",
            &json!({
                "configurationType": "APPLICATION",
                "orderBy": [{"fieldName": "application.name", "sortOrder": "DESC"}],
                "maxResults": 2,
            }),
        )
        .await;
    assert_eq!(status, 200, "ordered first page must succeed, got {body:?}");
    assert_eq!(
        ids_of(&configurations(&body["configurations"])),
        vec![gamma.clone(), beta.clone()],
        "case ordered page 1: wrong rows, got {body:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let ads = Discovery::new();
    for index in 0..3 {
        ads.create(&json!({"name": format!("app-{index}")})).await;
    }

    // A token minted for one configurationType, then replayed against another, plus
    // tokens the service never minted at all.
    let (status, body) = ads
        .call(
            "ListConfigurations",
            &json!({"configurationType": "APPLICATION", "maxResults": 1}),
        )
        .await;
    assert_eq!(status, 200, "first page must succeed, got {body:?}");
    let issued = body["nextToken"]
        .as_str()
        .expect("a truncated page must carry a nextToken")
        .to_string();

    let cases: Vec<(&str, &str, &str)> = vec![
        ("issued for APPLICATION, used for SERVER", &issued, "SERVER"),
        ("never issued", "0000000000000000000000000000beef", "APPLICATION"),
        ("an offset dressed as a token", "2", "APPLICATION"),
        ("empty", "", "APPLICATION"),
    ];
    for (case, token, configuration_type) in cases {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({"configurationType": configuration_type, "nextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case}: wrong error code in {body:?}"
        );
    }

    // And the token the service DID issue resumes where the page ended.
    let (status, body) = ads
        .call(
            "ListConfigurations",
            &json!({"configurationType": "APPLICATION", "nextToken": issued}),
        )
        .await;
    assert_eq!(status, 200, "case resume: must succeed, got {body:?}");
    assert_eq!(
        configurations(&body["configurations"]).len(),
        2,
        "case resume: the rest of the listing must follow, got {body:?}"
    );
}

#[tokio::test]
async fn every_configuration_type_answers_from_its_store_and_an_unknown_one_is_refused() {
    let ads = Discovery::new();
    let app = ads.create(&json!({"name": "only-application"})).await;

    // ConfigurationItemType's own value set (from the model's enum), including the
    // types nothing in this emulator discovers yet: those list empty, not 501.
    let cases = [
        ("APPLICATION", vec![app.clone()]),
        ("SERVER", vec![]),
        ("PROCESS", vec![]),
        ("CONNECTION", vec![]),
    ];
    for (configuration_type, expected) in cases {
        let (status, body) = ads
            .call(
                "ListConfigurations",
                &json!({"configurationType": configuration_type}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {configuration_type}: must succeed, got {status} {body:?}"
        );
        let rows = configurations(&body["configurations"]);
        let got = if configuration_type == "APPLICATION" {
            ids_of(&rows)
        } else {
            assert!(
                rows.is_empty(),
                "case {configuration_type}: nothing is discovered yet, got {rows:?}"
            );
            vec![]
        };
        assert_eq!(got, expected, "case {configuration_type}: got {got:?}");
    }

    // A value outside the enum, and the required member absent: both 400, neither 501.
    let refusals: Vec<(&str, Value)> = vec![
        ("off-set value", json!({"configurationType": "APPLICATIONS"})),
        ("lower case value", json!({"configurationType": "application"})),
        ("configurationType omitted", json!({})),
    ];
    for (case, request) in refusals {
        let (status, body) = ads.call("ListConfigurations", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: must be a client refusal, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert!(
            code == "ValidationException" || code == "InvalidParameterValueException",
            "case {case}: expected a client error code, got {code} in {body:?}"
        );
    }
}

#[tokio::test]
async fn update_writes_only_the_supplied_members_and_refuses_an_unknown_id() {
    // Each row: which members the update supplies, and what the record must read as
    // afterwards (name, description, wave).
    let cases: Vec<UpdateCase> = vec![
        (
            "name only",
            json!({"name": "renamed"}),
            ("renamed", Some("original description"), Some("wave-0")),
        ),
        (
            "description only",
            json!({"description": "new description"}),
            ("original", Some("new description"), Some("wave-0")),
        ),
        (
            "wave only",
            json!({"wave": "wave-9"}),
            ("original", Some("original description"), Some("wave-9")),
        ),
        (
            "every member",
            json!({"name": "all", "description": "all", "wave": "all"}),
            ("all", Some("all"), Some("all")),
        ),
        (
            "nothing supplied",
            json!({}),
            ("original", Some("original description"), Some("wave-0")),
        ),
        (
            "description emptied",
            json!({"description": ""}),
            ("original", Some(""), Some("wave-0")),
        ),
    ];

    for (case, patch, (name, description, wave)) in cases {
        let ads = Discovery::new();
        let id = ads
            .create(&json!({
                "name": "original",
                "description": "original description",
                "wave": "wave-0",
            }))
            .await;

        let mut request = patch.clone();
        request["configurationId"] = json!(id);
        let (status, body) = ads.call("UpdateApplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateApplication must succeed, got {status} {body:?}"
        );

        let described = ads.describe(&[&id]).await;
        let row = &described[0];
        assert_eq!(
            row.get("application.name").map(String::as_str),
            Some(name),
            "case {case}: wrong name, got {row:?}"
        );
        assert_eq!(
            row.get("application.description").map(String::as_str),
            description,
            "case {case}: wrong description, got {row:?}"
        );
        assert_eq!(
            row.get("application.wave").map(String::as_str),
            wave,
            "case {case}: wrong wave, got {row:?}"
        );
    }

    // An id no application holds: refused, never a silent success.
    let ads = Discovery::new();
    let live = ads.create(&json!({"name": "live"})).await;
    let unknown = ["d-application-doesnotexist", "d-application-0000009999", "not-an-id"];
    for id in unknown {
        let (status, body) = ads
            .call(
                "UpdateApplication",
                &json!({"configurationId": id, "name": "ignored"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {id}: an unknown configurationId must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {id}: wrong error code in {body:?}"
        );
    }
    assert_eq!(
        ads.describe(&[&live]).await[0]
            .get("application.name")
            .map(String::as_str),
        Some("live"),
        "a refused update must not touch another record"
    );
}

#[tokio::test]
async fn describe_reports_known_ids_once_and_skips_unknown_ones() {
    let ads = Discovery::new();
    let first = ads.create(&json!({"name": "first"})).await;
    let second = ads.create(&json!({"name": "second"})).await;

    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one known id", vec![first.clone()], vec![first.clone()]),
        (
            "both known ids",
            vec![first.clone(), second.clone()],
            vec![first.clone(), second.clone()],
        ),
        (
            "request order is kept",
            vec![second.clone(), first.clone()],
            vec![second.clone(), first.clone()],
        ),
        (
            "a repeated id is reported once",
            vec![first.clone(), first.clone()],
            vec![first.clone()],
        ),
        (
            "an unknown id is absent",
            vec![first.clone(), "d-application-ffffffffff".to_string()],
            vec![first.clone()],
        ),
        (
            "only unknown ids",
            vec!["d-application-ffffffffff".to_string()],
            vec![],
        ),
    ];

    for (case, requested, expected) in cases {
        let borrowed: Vec<&str> = requested.iter().map(String::as_str).collect();
        let got = ids_of(&ads.describe(&borrowed).await);
        assert_eq!(got, expected, "case {case}: got {got:?}");
    }
}

#[tokio::test]
async fn delete_removes_the_named_applications_and_repeats_without_error() {
    // Each row: how many applications exist, which of them the delete names (by
    // index), and whether the call is repeated.
    let cases: Vec<(&str, usize, Vec<usize>, bool)> = vec![
        ("one of one", 1, vec![0], false),
        ("one of three", 3, vec![1], false),
        ("two of three", 3, vec![0, 2], false),
        ("all of three", 3, vec![0, 1, 2], false),
        ("none named", 3, vec![], false),
        ("repeated delete is idempotent", 3, vec![0, 1], true),
    ];

    for (case, count, delete_indexes, repeat) in cases {
        let ads = Discovery::new();
        let mut created = Vec::new();
        for index in 0..count {
            created.push(ads.create(&json!({"name": format!("app-{index}")})).await);
        }
        let doomed: Vec<String> = delete_indexes
            .iter()
            .map(|index| created[*index].clone())
            .collect();

        for attempt in 0..=u8::from(repeat) {
            let (status, body) = ads
                .call(
                    "DeleteApplications",
                    &json!({"configurationIds": doomed}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: delete attempt {attempt} must succeed, got {status} {body:?}"
            );
        }

        let survivors: Vec<String> = created
            .iter()
            .filter(|id| !doomed.contains(id))
            .cloned()
            .collect();
        assert_eq!(
            ids_of(&ads.list_applications().await),
            survivors,
            "case {case}: the listing must hold exactly the survivors"
        );
        for gone in &doomed {
            assert!(
                ads.describe(&[gone]).await.is_empty(),
                "case {case}: {gone} must not be described after the delete"
            );
        }
        assert_eq!(
            ads.summary().await["applications"],
            json!(survivors.len()),
            "case {case}: the summary must count the survivors"
        );
    }
}

#[tokio::test]
async fn the_discovery_summary_counts_what_the_store_holds() {
    // Each row: creates then deletes, and the applications count that must follow.
    let cases: Vec<(&str, usize, usize, i64)> = vec![
        ("empty store", 0, 0, 0),
        ("one application", 1, 0, 1),
        ("two applications", 2, 0, 2),
        ("two created, one deleted", 2, 1, 1),
        ("five created, five deleted", 5, 5, 0),
        ("ten created, three deleted", 10, 3, 7),
    ];

    for (case, create_count, delete_count, expected) in cases {
        let ads = Discovery::new();
        let mut created = Vec::new();
        for index in 0..create_count {
            created.push(ads.create(&json!({"name": format!("app-{index}")})).await);
        }
        if delete_count > 0 {
            let doomed: Vec<String> = created.iter().take(delete_count).cloned().collect();
            let (status, body) = ads
                .call("DeleteApplications", &json!({"configurationIds": doomed}))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: delete must succeed, got {status} {body:?}"
            );
        }

        let summary = ads.summary().await;
        assert_eq!(
            summary["applications"],
            json!(expected),
            "case {case}: wrong applications count in {summary:?}"
        );
        // Nothing in this emulator discovers a server, tags one, or associates one
        // with an application yet, so those counts read from empty stores.
        for member in [
            "servers",
            "serversMappedToApplications",
            "serversMappedtoTags",
        ] {
            assert_eq!(
                summary[member],
                json!(0),
                "case {case}: {member} must count its own store, got {summary:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_application_store_is_bounded_and_a_delete_frees_a_slot() {
    // The store is the emulator's own memory: once it holds APPLICATION_QUOTA
    // applications a further create is refused, and deleting one makes room again.
    let ads = Discovery::new();
    for index in 0..APPLICATION_QUOTA {
        let (status, body) = ads
            .call("CreateApplication", &json!({"name": format!("app-{index}")}))
            .await;
        assert_eq!(
            status, 200,
            "case create {index} of {APPLICATION_QUOTA}: must be accepted, got {status} {body:?}"
        );
    }

    let (status, body) = ads
        .call("CreateApplication", &json!({"name": "one-too-many"}))
        .await;
    assert_eq!(
        status, 400,
        "case over quota: a create past {APPLICATION_QUOTA} must be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case over quota: wrong error code in {body:?}"
    );

    // The refusal added nothing, and freeing a slot lets the next create through.
    assert_eq!(
        ads.summary().await["applications"],
        json!(APPLICATION_QUOTA),
        "case over quota: the refused create must not have been stored"
    );
    let first = ads.describe(&["d-application-0000000000"]).await;
    assert_eq!(
        first.len(),
        1,
        "the first application must still be stored: {first:?}"
    );
    let (status, body) = ads
        .call(
            "DeleteApplications",
            &json!({"configurationIds": ["d-application-0000000000"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case free a slot: delete must succeed, got {status} {body:?}"
    );
    let (status, body) = ads
        .call("CreateApplication", &json!({"name": "after-a-delete"}))
        .await;
    assert_eq!(
        status, 200,
        "case free a slot: a create must be accepted again, got {status} {body:?}"
    );
}

#[tokio::test]
async fn reading_the_store_does_not_change_what_a_later_caller_sees() {
    let ads = Discovery::new();
    let id = ads
        .create(&json!({"name": "stable", "description": "unchanged", "wave": "wave-1"}))
        .await;

    let first_list = ads.list_applications().await;
    let first_describe = ads.describe(&[&id]).await;
    let first_summary = ads.summary().await;

    // Poll each reader repeatedly: a read must be idempotent in every respect.
    for round in 0..5 {
        assert_eq!(
            ads.list_applications().await,
            first_list,
            "round {round}: ListConfigurations changed between reads"
        );
        assert_eq!(
            ads.describe(&[&id]).await,
            first_describe,
            "round {round}: DescribeConfigurations changed between reads"
        );
        assert_eq!(
            ads.summary().await,
            first_summary,
            "round {round}: GetDiscoverySummary changed between reads"
        );
    }
}
