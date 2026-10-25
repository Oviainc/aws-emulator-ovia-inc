// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch dashboard loop: `PutDashboard` creating and replacing
//! a dashboard, `GetDashboard` reading one back, and `ListDashboards` reporting them.
//!
//! Every table row is a distinct input along what the rules under test generalise over (the
//! name's length and characters, the shape and size of the `DashboardBody` document, whether
//! the dashboard already exists, how many dashboards exist and which prefix selects them), so
//! the dashboard a recorded request happened to create is one row among many rather than the
//! specification.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};

/// A string member of a body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A timestamp member, which the awsJson protocols carry as epoch seconds.
fn stamp(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// The ARN a dashboard of this name carries. The Region segment is empty because "All
/// dashboards in your account are global, not region-specific." (`PutDashboard`), and the
/// account is the emulator's canonical one.
fn expected_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch::000000000000:dashboard/{name}")
}

/// A `PutDashboard` request with only the members the model makes required.
fn put_request(name: &str, body: &str) -> Value {
    json!({"DashboardName": name, "DashboardBody": body})
}

/// The dashboard entry `ListDashboards` reported under `name`, if any.
fn entry_named(body: &Value, name: &str) -> Option<Value> {
    body.get("DashboardEntries")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("DashboardName").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// Every dashboard name `ListDashboards` reported, in the order it reported them.
fn reported_names(body: &Value) -> Vec<String> {
    body.get("DashboardEntries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("DashboardName").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the tag table: the case, the `Tags` the create carries, the `Tags` a later
/// update carries, and the (key, value) pairs the dashboard must report afterwards.
type TagCase = (&'static str, Value, Value, Vec<(&'static str, &'static str)>);

/// How far apart the replacement rows below space their writes.
///
/// A time stamp crosses the wire as a double, so two writes in the same instant could report
/// the same `LastModified`. This gap is far larger than any rounding either conversion does,
/// which is what makes "the write advanced it" a definite assertion rather than a race.
const WRITE_GAP: std::time::Duration = std::time::Duration::from_millis(20);

/// A dashboard document holding `count` metric widgets, for the rows that vary the body's
/// size and structure.
fn body_with_widgets(count: usize) -> String {
    let widgets: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "type": "metric",
                "x": 0,
                "y": index * 6,
                "width": 12,
                "height": 6,
                "properties": {
                    "metrics": [["MyApp", format!("Latency{index}")]],
                    "period": 300,
                    "stat": "Average",
                    "region": "us-east-1",
                },
            })
        })
        .collect();
    serde_json::to_string(&json!({"widgets": widgets})).expect("the body serializes")
}

#[tokio::test]
async fn every_dashboard_form_round_trips_through_get_dashboard() {
    // Rows differ along the name's length and which of the documented characters it uses, and
    // along the document's shape and size: an empty object, one widget, many widgets, a text
    // widget carrying non-ASCII markdown, and members the dashboard syntax does not define.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a one-character name and an empty document",
            "d".to_string(),
            "{}".to_string(),
        ),
        (
            "a name using every documented character class",
            "Prod-App_Dashboard-42".to_string(),
            body_with_widgets(1),
        ),
        (
            "a 255-character name, the documented maximum",
            "n".repeat(255),
            body_with_widgets(3),
        ),
        (
            "a document with many widgets",
            "many-widgets".to_string(),
            body_with_widgets(40),
        ),
        (
            "a text widget carrying non-ASCII markdown",
            "text_widget".to_string(),
            serde_json::to_string(&json!({"widgets": [{
                "type": "text",
                "x": 0, "y": 0, "width": 24, "height": 2,
                "properties": {"markdown": "# Übersicht — created by script ✅"},
            }]}))
            .expect("the body serializes"),
        ),
        (
            "a document carrying members the dashboard syntax does not define",
            "0".to_string(),
            serde_json::to_string(&json!({
                "start": "-PT6H",
                "periodOverride": "inherit",
                "widgets": [],
                "somethingElse": {"nested": [1, 2, 3]},
            }))
            .expect("the body serializes"),
        ),
    ];

    for (case, name, body) in cases {
        let router = build_router();
        let put = ok(&router, "PutDashboard", &put_request(&name, &body)).await;
        // "If the input for PutDashboard was correct and the dashboard was successfully
        // created or modified, this result is empty."
        // (`PutDashboardOutput$DashboardValidationMessages`)
        assert_eq!(
            put.get("DashboardValidationMessages"),
            Some(&json!([])),
            "case {case:?}: a stored dashboard reports no validation message, got {put}"
        );

        let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            text(&got, "DashboardBody").as_deref(),
            Some(body.as_str()),
            "case {case:?}: the document must round-trip verbatim, got {got}"
        );
        assert_eq!(
            text(&got, "DashboardName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the name back, got {got}"
        );
        assert_eq!(
            text(&got, "DashboardArn").as_deref(),
            Some(expected_arn(&name).as_str()),
            "case {case:?}: expected the dashboard's global ARN, got {got}"
        );

        // Reading twice changes nothing a later caller sees.
        let again = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            again, got,
            "case {case:?}: a second GetDashboard must report the same dashboard, got {again}"
        );

        let listed = ok(&router, "ListDashboards", &json!({})).await;
        let entry = entry_named(&listed, &name)
            .unwrap_or_else(|| panic!("case {case:?}: expected the dashboard listed, got {listed}"));
        assert_eq!(
            text(&entry, "DashboardArn").as_deref(),
            Some(expected_arn(&name).as_str()),
            "case {case:?}: the listed entry carries the same ARN, got {entry}"
        );
        // "The size of the dashboard, in bytes." (`DashboardEntry$Size`)
        assert_eq!(
            entry.get("Size").and_then(Value::as_i64),
            Some(i64::try_from(body.len()).expect("the body size fits")),
            "case {case:?}: expected the document's size in bytes, got {entry}"
        );
        assert!(
            stamp(&entry, "LastModified").is_some(),
            "case {case:?}: a stored dashboard carries the time it was last written, \
             got {entry}"
        );
    }
}

#[tokio::test]
async fn a_put_replaces_the_dashboard_it_names_and_creates_none() {
    let router = build_router();
    ok(
        &router,
        "PutDashboard",
        &json!({
            "DashboardName": "replaced",
            "DashboardBody": body_with_widgets(1),
            "Tags": [{"Key": "team", "Value": "observability"}],
        }),
    )
    .await;
    ok(
        &router,
        "PutDashboard",
        &put_request("untouched", &body_with_widgets(2)),
    )
    .await;
    let first = ok(&router, "ListDashboards", &json!({})).await;
    let mut previous_modified = stamp(
        &entry_named(&first, "replaced").expect("the dashboard was created"),
        "LastModified",
    )
    .expect("the entry carries LastModified");

    // Rows differ in what the replacement document is: a bigger one, a smaller one, the empty
    // object, and the same document again.
    let replacements: Vec<(&str, String)> = vec![
        ("a larger document", body_with_widgets(5)),
        ("a smaller document", body_with_widgets(1)),
        ("an empty document", "{}".to_string()),
        ("the same document again", "{}".to_string()),
    ];

    for (case, body) in replacements {
        tokio::time::sleep(WRITE_GAP).await;
        ok(&router, "PutDashboard", &put_request("replaced", &body)).await;

        let got = ok(
            &router,
            "GetDashboard",
            &json!({"DashboardName": "replaced"}),
        )
        .await;
        // "If you update a dashboard, the entire contents are replaced with what you specify
        // here." (`PutDashboard`)
        assert_eq!(
            text(&got, "DashboardBody").as_deref(),
            Some(body.as_str()),
            "case {case:?}: the replacement document must be the whole contents, got {got}"
        );

        let listed = ok(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            reported_names(&listed),
            vec!["replaced".to_string(), "untouched".to_string()],
            "case {case:?}: a replacement must not create a second dashboard, got {listed}"
        );
        let entry = entry_named(&listed, "replaced").expect("the dashboard is listed");
        assert_eq!(
            entry.get("Size").and_then(Value::as_i64),
            Some(i64::try_from(body.len()).expect("the body size fits")),
            "case {case:?}: the reported size follows the stored document, got {entry}"
        );
        let modified = stamp(&entry, "LastModified").expect("the entry carries LastModified");
        assert!(
            modified > previous_modified,
            "case {case:?}: expected LastModified to advance past {previous_modified}, \
             got {modified}"
        );
        previous_modified = modified;

        // The dashboard the write did not name is untouched.
        let other = ok(
            &router,
            "GetDashboard",
            &json!({"DashboardName": "untouched"}),
        )
        .await;
        assert_eq!(
            text(&other, "DashboardBody").as_deref(),
            Some(body_with_widgets(2).as_str()),
            "case {case:?}: the dashboard the write did not name must be untouched, got {other}"
        );
    }
}

#[tokio::test]
async fn a_dashboard_name_outside_the_documented_rule_is_refused() {
    // "The maximum length is 255, and valid characters are A-Z, a-z, 0-9, "-", and "_"."
    // (`PutDashboardInput$DashboardName`) — rows span both published bounds, one character
    // past the maximum, and characters inside and outside the documented set.
    let cases: Vec<(&str, String, bool)> = vec![
        ("one character, the shortest name", "d".to_string(), true),
        ("255 characters, the documented maximum", "n".repeat(255), true),
        ("256 characters, one past the maximum", "n".repeat(256), false),
        ("1024 characters", "n".repeat(1024), false),
        ("no characters at all", String::new(), false),
        ("every documented character", "aZ9-_".to_string(), true),
        ("a name holding a space", "my dashboard".to_string(), false),
        ("a name holding a dot", "my.dashboard".to_string(), false),
        ("a name holding a slash", "team/dashboard".to_string(), false),
        ("a name holding a colon", "team:dashboard".to_string(), false),
        ("a name holding non-ASCII letters", "übersicht".to_string(), false),
    ];

    for (case, name, expected_ok) in cases {
        let router = build_router();
        let (status, body) = call(&router, "PutDashboard", &put_request(&name, "{}")).await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a name the documentation allows must be accepted, got {status} \
                 {body}"
            );
            let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
            assert_eq!(
                text(&got, "DashboardName").as_deref(),
                Some(name.as_str()),
                "case {case:?}: the accepted name must round-trip, got {got}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
        );
        if name.chars().count() > 255 {
            // CloudWatch words an over-long field as one fixed sentence naming the field and
            // its bound.
            assert_eq!(
                text(&body, "message").as_deref(),
                Some(
                    "The value for field DashboardName is too long. It must have length less \
                     than or equal to 255.\n"
                ),
                "case {case:?}: expected CloudWatch's over-long-field sentence, got {body}"
            );
        } else if !name.is_empty() {
            // And a field holding a character its rule does not allow as its own sentence,
            // naming the characters that rule does allow.
            assert_eq!(
                text(&body, "message").as_deref(),
                Some(
                    "The value for field DashboardName contains invalid characters. It can \
                     only contain alphanumerics, dash (-) and underscore (_).\n"
                ),
                "case {case:?}: expected CloudWatch's invalid-characters sentence, got {body}"
            );
        }
        // A refused request persists nothing.
        let listed = ok(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            reported_names(&listed),
            Vec::<String>::new(),
            "case {case:?}: a refused PutDashboard must persist nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_dashboard_body_that_is_not_a_json_object_is_refused() {
    // Rows span the JSON forms a body can arrive as: objects the service stores, and the
    // documents that are not a JSON object at all.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("an empty object", "{}", true),
        ("an object holding widgets", r#"{"widgets":[]}"#, true),
        ("an object with leading whitespace", "  {\n\"widgets\": []\n}", true),
        ("a truncated object", "{ this is not valid dashboard json ]", false),
        ("an array", "[]", false),
        ("a bare string", "\"widgets\"", false),
        ("a number", "17", false),
        ("a boolean", "true", false),
        ("JSON null", "null", false),
        ("nothing at all", "", false),
        ("two objects in a row", "{}{}", false),
    ];

    for (case, body, expected_ok) in cases {
        let router = build_router();
        let (status, response) = call(&router, "PutDashboard", &put_request("body-case", body)).await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a JSON object must be accepted, got {status} {response}"
            );
            let got = ok(
                &router,
                "GetDashboard",
                &json!({"DashboardName": "body-case"}),
            )
            .await;
            assert_eq!(
                text(&got, "DashboardBody").as_deref(),
                Some(body),
                "case {case:?}: the accepted document must round-trip verbatim, got {got}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DashboardInvalidInputError"),
            "case {case:?}: expected a 400 DashboardInvalidInputError, got {status} {response}"
        );
        assert_eq!(
            text(&response, "message").as_deref(),
            Some("The field DashboardBody must be a valid JSON object"),
            "case {case:?}: expected CloudWatch's invalid-field sentence, got {response}"
        );
        // A refused request persists nothing.
        let (get_status, get_body) = call(
            &router,
            "GetDashboard",
            &json!({"DashboardName": "body-case"}),
        )
        .await;
        assert_eq!(
            (get_status, error_code(&get_body).as_str()),
            (404, "DashboardNotFoundError"),
            "case {case:?}: a refused PutDashboard must store no dashboard, got {get_status} \
             {get_body}"
        );
    }
}

#[tokio::test]
async fn get_dashboard_answers_not_found_for_a_name_no_dashboard_holds() {
    let router = build_router();
    ok(&router, "PutDashboard", &put_request("present", "{}")).await;

    // Rows differ in how the requested name misses: a name nothing was ever stored under, one
    // differing only in case, and one that is a prefix or an extension of a stored name.
    for name in ["absent", "PRESENT", "pres", "present-2"] {
        let (status, body) = call(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "DashboardNotFoundError"),
            "case {name:?}: expected a 404 DashboardNotFoundError, got {status} {body}"
        );
        // CloudWatch words this not-found as one sentence naming the dashboard the request
        // asked for.
        assert_eq!(
            text(&body, "message").as_deref(),
            Some(format!("Dashboard {name} does not exist").as_str()),
            "case {name:?}: expected the refusal to name the dashboard asked for, got {body}"
        );
    }

    // The dashboard that IS stored is unaffected by the misses.
    let got = ok(&router, "GetDashboard", &json!({"DashboardName": "present"})).await;
    assert_eq!(
        text(&got, "DashboardBody").as_deref(),
        Some("{}"),
        "case \"after the misses\": the stored dashboard must still read back, got {got}"
    );
}

#[tokio::test]
async fn list_dashboards_reports_the_dashboards_a_prefix_selects_in_name_order() {
    let router = build_router();
    // Created out of order, so the reported order is the operation's and not the insertion
    // order.
    let created = [
        "prod-web",
        "dev_api",
        "prod-api",
        "Prod-Legacy",
        "staging-edge",
    ];
    for name in created {
        ok(&router, "PutDashboard", &put_request(name, "{}")).await;
    }

    // Rows differ in what the prefix selects: everything (omitted and empty), a prefix
    // several dashboards share, a whole name, a prefix that is case-sensitive, a prefix
    // holding the "." the member allows, and one nothing starts with.
    let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "no prefix at all",
            None,
            vec!["Prod-Legacy", "dev_api", "prod-api", "prod-web", "staging-edge"],
        ),
        (
            "an empty prefix",
            Some(""),
            vec!["Prod-Legacy", "dev_api", "prod-api", "prod-web", "staging-edge"],
        ),
        ("a shared prefix", Some("prod-"), vec!["prod-api", "prod-web"]),
        ("a whole name", Some("dev_api"), vec!["dev_api"]),
        ("a prefix in another case", Some("Prod"), vec!["Prod-Legacy"]),
        // The prefix member allows the "." no dashboard NAME may hold, so such a prefix is
        // accepted and selects nothing.
        ("a prefix holding a dot", Some("staging."), vec![]),
        ("a prefix nothing starts with", Some("zzz"), vec![]),
    ];

    for (case, prefix, expected) in cases {
        let mut request = json!({});
        if let Some(prefix) = prefix {
            request["DashboardNamePrefix"] = json!(prefix);
        }
        let listed = ok(&router, "ListDashboards", &request).await;
        assert_eq!(
            reported_names(&listed),
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the selected dashboards in name order, got {listed}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {case:?}: one page holds every dashboard here, so no token is owed, \
             got {listed}"
        );
        // Listing twice changes nothing a later caller sees.
        let again = ok(&router, "ListDashboards", &request).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second ListDashboards must report the same entries, got {again}"
        );
    }
}

#[tokio::test]
async fn list_dashboards_refuses_a_prefix_or_token_outside_the_documented_rule() {
    let router = build_router();
    ok(&router, "PutDashboard", &put_request("listed", "{}")).await;

    // "The maximum length is 255, and valid characters are A-Z, a-z, 0-9, ".", "-", and "_"."
    // (`ListDashboardsInput$DashboardNamePrefix`) — rows span the published bound, one past
    // it, and characters inside and outside the set, which is the name's set plus ".".
    let prefixes: Vec<(&str, String, bool)> = vec![
        ("255 characters, the documented maximum", "n".repeat(255), true),
        ("256 characters, one past the maximum", "n".repeat(256), false),
        ("every documented character", "aZ9-_.".to_string(), true),
        ("a prefix holding a space", "my dash".to_string(), false),
        ("a prefix holding a slash", "team/".to_string(), false),
        ("a prefix holding non-ASCII letters", "ü".to_string(), false),
    ];

    for (case, prefix, expected_ok) in prefixes {
        let (status, body) = call(
            &router,
            "ListDashboards",
            &json!({"DashboardNamePrefix": prefix}),
        )
        .await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a prefix the documentation allows must be accepted, got {status} \
                 {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
            );
        }
    }

    // A token this service did not issue names no page. Each row is a token this service
    // could not have minted: one that is not the opaque encoding at all, and one whose
    // decoding is not text.
    for token in ["not-a-token", "zzzz", "fffe"] {
        let (status, body) = call(&router, "ListDashboards", &json!({"NextToken": token})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {token:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_dashboard_is_global_so_every_region_reads_the_same_one() {
    use crate::harness::call_in_region;

    let router = build_router();
    // "All dashboards in your account are global, not region-specific." (`PutDashboard`)
    let (put_status, put_body) = call_in_region(
        &router,
        "eu-west-1",
        "PutDashboard",
        &put_request("global-dashboard", &body_with_widgets(1)),
    )
    .await;
    assert_eq!(
        put_status, 200,
        "case \"written through eu-west-1\": expected HTTP 200, got {put_status} {put_body}"
    );

    for region in ["us-east-1", "ap-southeast-2", "eu-west-1"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetDashboard",
            &json!({"DashboardName": "global-dashboard"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: a global dashboard must be readable in every Region, got {status} \
             {body}"
        );
        assert_eq!(
            text(&body, "DashboardArn").as_deref(),
            Some(expected_arn("global-dashboard").as_str()),
            "case {region:?}: the ARN of a global dashboard names no Region, got {body}"
        );
    }
}

#[tokio::test]
async fn list_dashboards_pages_at_the_documented_page_size_and_resumes_at_its_token() {
    let router = build_router();
    // "ListDashboards returns up to 1000 results on one page. If there are more than 1000
    // dashboards, you can call ListDashboards again and include the value you received for
    // NextToken in the first call, to receive the next 1000 results." (`ListDashboards`) — so
    // one dashboard past that page size is what makes a second page exist.
    let total = 1_001_usize;
    let mut expected: Vec<String> = (0..total).map(|index| format!("dash-{index:05}")).collect();
    for name in &expected {
        ok(&router, "PutDashboard", &put_request(name, "{}")).await;
    }
    expected.sort();

    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut request = json!({});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(&router, "ListDashboards", &request).await;
        let names = reported_names(&page);
        assert!(
            names.len() <= 1_000,
            "case \"page {pages}\": a page must hold at most 1000 entries, got {}",
            names.len()
        );
        seen.extend(names);
        pages += 1;
        assert!(
            pages <= 3,
            "case \"paging terminates\": expected two pages over {total} dashboards, got {pages}"
        );
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }

    assert_eq!(
        pages, 2,
        "case \"one dashboard past the page size\": expected exactly two pages, got {pages}"
    );
    assert_eq!(
        seen, expected,
        "case \"the resumed page\": paging must report every dashboard exactly once, in name \
         order"
    );
}

#[tokio::test]
async fn tags_belong_to_the_dashboard_the_create_gave_them_to() {
    // "You can use this parameter only when creating a new dashboard. If you specify Tags
    // when updating an existing dashboard, the tag updates are ignored. To add or update tags
    // on an existing dashboard, use TagResource." (`PutDashboardInput$Tags`) — rows differ in
    // how many tags the create carries and what the later update asks for.
    let cases: Vec<TagCase> = vec![
        (
            "created with no tags at all",
            json!(null),
            json!([{"Key": "Env", "Value": "prod"}]),
            vec![],
        ),
        (
            "created with one tag, updated with another",
            json!([{"Key": "Env", "Value": "prod"}]),
            json!([{"Key": "Env", "Value": "dev"}, {"Key": "Team", "Value": "ops"}]),
            vec![("Env", "prod")],
        ),
        (
            "created with three tags, updated with none",
            json!([
                {"Key": "Env", "Value": "staging"},
                {"Key": "Team", "Value": "observability"},
                {"Key": "Tier", "Value": "1"},
            ]),
            json!(null),
            vec![
                ("Env", "staging"),
                ("Team", "observability"),
                ("Tier", "1"),
            ],
        ),
    ];

    for (case, created, updated, expected) in cases {
        let router = build_router();
        let mut create = put_request("tagged-dashboard", "{}");
        if !created.is_null() {
            create["Tags"] = created.clone();
        }
        ok(&router, "PutDashboard", &create).await;

        let arn = expected_arn("tagged-dashboard");
        let expected: Vec<Value> = expected
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let listed = ok(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            listed.get("Tags"),
            Some(&Value::Array(expected.clone())),
            "case {case:?}: the created tags are the dashboard's tag set, got {listed}"
        );

        // The update's tags are ignored, and the set the create wrote survives it.
        let mut update = put_request("tagged-dashboard", r#"{"widgets":[]}"#);
        if !updated.is_null() {
            update["Tags"] = updated.clone();
        }
        ok(&router, "PutDashboard", &update).await;
        let after = ok(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            after.get("Tags"),
            Some(&Value::Array(expected)),
            "case {case:?}: an update must leave the dashboard's tags alone, got {after}"
        );
    }
}

#[tokio::test]
async fn a_create_carrying_more_tags_than_a_resource_may_hold_is_refused() {
    // "You can associate as many as 50 tags with a dashboard." (`PutDashboardInput$Tags`) —
    // rows span the published bound and one past it.
    for count in [0_usize, 1, 50, 51] {
        let router = build_router();
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({"Key": format!("k{index}"), "Value": format!("v{index}")}))
            .collect();
        let mut request = put_request("tag-bound", "{}");
        request["Tags"] = Value::Array(tags);

        let (status, body) = call(&router, "PutDashboard", &request).await;
        if count <= 50 {
            assert_eq!(
                status, 200,
                "case \"{count} tags\": a tag count the documentation allows must be accepted, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case \"{count} tags\": expected a 400 InvalidParameterValueException, got \
                 {status} {body}"
            );
            // A refused request persists nothing.
            let listed = ok(&router, "ListDashboards", &json!({})).await;
            assert_eq!(
                reported_names(&listed),
                Vec::<String>::new(),
                "case \"{count} tags\": a refused PutDashboard must persist nothing, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn delete_dashboards_removes_every_dashboard_it_names() {
    // Rows differ in how many dashboards the request names, how many of them exist, and
    // whether the list is empty: "Deletes all dashboards that you specify." and this
    // operation declares no not-found error, so a name nothing holds is already in the state
    // the request asks for.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one of three", vec!["alpha"], vec!["beta", "gamma"]),
        ("two of three", vec!["alpha", "gamma"], vec!["beta"]),
        (
            "every dashboard held",
            vec!["alpha", "beta", "gamma"],
            vec![],
        ),
        (
            "a name nothing is stored under",
            vec!["delta"],
            vec!["alpha", "beta", "gamma"],
        ),
        (
            "a held name and an absent one",
            vec!["beta", "delta"],
            vec!["alpha", "gamma"],
        ),
        (
            "one name twice",
            vec!["beta", "beta"],
            vec!["alpha", "gamma"],
        ),
        (
            "no names at all",
            vec![],
            vec!["alpha", "beta", "gamma"],
        ),
    ];

    for (case, deleted, expected) in cases {
        let router = build_router();
        for name in ["alpha", "beta", "gamma"] {
            ok(&router, "PutDashboard", &put_request(name, "{}")).await;
        }

        let response = ok(
            &router,
            "DeleteDashboards",
            &json!({"DashboardNames": deleted}),
        )
        .await;
        assert_eq!(
            response,
            json!({}),
            "case {case:?}: DeleteDashboards models no output member, got {response}"
        );

        let listed = ok(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            reported_names(&listed),
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the dashboards the request did not name, \
             got {listed}"
        );
        for name in &deleted {
            let (status, body) = call(&router, "GetDashboard", &json!({"DashboardName": name}))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "DashboardNotFoundError"),
                "case {case:?}: {name} must be gone after the delete, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn delete_dashboards_refuses_a_request_outside_the_documented_rule() {
    // "You can specify up to 100 dashboards to delete." (`DeleteDashboards`) — rows span the
    // published bound, one past it, and a name outside the dashboard-name rule.
    let hundred: Vec<String> = (0..100).map(|index| format!("d{index}")).collect();
    let hundred_and_one: Vec<String> = (0..101).map(|index| format!("d{index}")).collect();
    let cases: Vec<(&str, Vec<String>, bool)> = vec![
        ("one name", vec!["kept".to_string()], true),
        ("100 names, the documented maximum", hundred, true),
        ("101 names, one past the maximum", hundred_and_one, false),
        (
            "a name over the length rule",
            vec!["n".repeat(256)],
            false,
        ),
        (
            "a name holding a character the rule forbids",
            vec!["my dashboard".to_string()],
            false,
        ),
        (
            "a held name beside a name the rule forbids",
            vec!["kept".to_string(), "my/dashboard".to_string()],
            false,
        ),
    ];

    for (case, names, expected_ok) in cases {
        let router = build_router();
        ok(&router, "PutDashboard", &put_request("kept", "{}")).await;

        let (status, body) = call(
            &router,
            "DeleteDashboards",
            &json!({"DashboardNames": names}),
        )
        .await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a request the documentation allows must be accepted, got \
                 {status} {body}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
        );
        // A refused request deletes nothing, including the dashboards it named correctly.
        let listed = ok(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            reported_names(&listed),
            vec!["kept".to_string()],
            "case {case:?}: a refused DeleteDashboards must delete nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn list_dashboards_reports_an_empty_list_before_any_dashboard_exists() {
    let router = build_router();
    let listed = ok(&router, "ListDashboards", &json!({})).await;
    assert_eq!(
        reported_names(&listed),
        Vec::<String>::new(),
        "case \"no dashboards created\": expected no entries, got {listed}"
    );
    assert_eq!(
        listed.get("NextToken"),
        None,
        "case \"no dashboards created\": expected no continuation token, got {listed}"
    );
}
