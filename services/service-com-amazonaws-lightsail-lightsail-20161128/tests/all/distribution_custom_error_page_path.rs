// SPDX-License-Identifier: CC0-1.0

//! The page path a Lightsail distribution's custom error responses name, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a `responsePagePath` a custom error response carries is
//! the path to an object of the distribution's origin, so it has to begin with a
//! forward slash — the modelled
//! `DistributionCustomErrorResponse$responsePagePath` documentation states it
//! ("The path must begin with a forward slash (/) and reference an object that
//! is available from the origin") and the bare `string` the member targets
//! cannot. Both operations that take the list from a request answer to it, and a
//! refused request changes nothing; a path that satisfies it is stored and read
//! back as sent.

use serde_json::{json, Value};

// The harness, the wire error code, the instance stand-up and the distribution
// reads this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, Emulator};

/// The one Region a distribution lives in and its API is served from, from the
/// modelled `LightsailDistribution$location` documentation ("all distributions
/// are located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The `status` a distribution settles into once a configuration has rolled out.
const DEPLOYED: &str = "Deployed";

/// The instance every distribution here pulls content from.
const ORIGIN_INSTANCE: &str = "error-page-origin";

/// The distribution the update cases apply their lists to.
const DISTRIBUTION: &str = "error-page-cdn";

/// The bundle every distribution here is created on, an id of the service's
/// published CDN plans (`GetDistributionBundles`).
const BUNDLE_ID: &str = "small_1_0";

/// The value Lightsail puts in a not-found body's `code` member.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The member a refusal of this rule has to name.
const MEMBER: &str = "responsePagePath";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One custom error response list, and whether the rule admits it.
///
/// The rows differ along everything the rule generalises over: the form of the
/// path (an object at the root, a nested one, a wildcard-free directory index, a
/// percent-encoded segment, a bare root), its length, whether the member is
/// carried at all, how many entries the list carries, which entry in the list
/// carries the offending path, and the ways a value can fail to begin with a
/// forward slash (nothing at all, only whitespace, whitespace before the slash,
/// a relative path, a dot-relative path, an absolute URL, a backslash).
struct Row {
    case: &'static str,
    /// The `customErrorResponses` list the request carries.
    responses: fn() -> Value,
    /// The entry the rule does NOT admit — its index in the list and the page
    /// path it carries — or `None` when the rule admits every entry, so a
    /// refusal has to name exactly the entry that is wrong.
    offends: Option<(usize, &'static str)>,
}

/// The table every case below walks.
const ROWS: &[Row] = &[
    Row {
        case: "an object at the root of the origin",
        responses: || json!([entry(404, "404", Some("/404.html"))]),
        offends: None,
    },
    Row {
        case: "the root path itself",
        responses: || json!([entry(403, "403", Some("/"))]),
        offends: None,
    },
    Row {
        case: "a nested object several directories down",
        responses: || json!([entry(500, "503", Some("/errors/static/v2/50x-page.html"))]),
        offends: None,
    },
    Row {
        case: "a directory index with no file name",
        responses: || json!([entry(502, "502", Some("/errors/"))]),
        offends: None,
    },
    Row {
        case: "a percent-encoded segment",
        responses: || json!([entry(400, "400", Some("/errors/%20spaced%20/bad-request.html"))]),
        offends: None,
    },
    Row {
        case: "two entries, both naming a path of the origin",
        responses: || {
            json!([
                entry(404, "404", Some("/404.html")),
                entry(403, "200", Some("/denied.html")),
            ])
        },
        offends: None,
    },
    Row {
        case: "an entry that names no page path at all",
        responses: || json!([entry(404, "404", None)]),
        offends: None,
    },
    Row {
        case: "an empty list",
        responses: || json!([]),
        offends: None,
    },
    Row {
        case: "a path with no leading slash",
        responses: || json!([entry(404, "404", Some("404.html"))]),
        offends: Some((0, "404.html")),
    },
    Row {
        case: "an empty path",
        responses: || json!([entry(404, "404", Some(""))]),
        offends: Some((0, "")),
    },
    Row {
        case: "a path of nothing but whitespace",
        responses: || json!([entry(404, "404", Some("   "))]),
        offends: Some((0, "   ")),
    },
    Row {
        case: "whitespace before the leading slash",
        responses: || json!([entry(404, "404", Some("   /404.html"))]),
        offends: Some((0, "   /404.html")),
    },
    Row {
        case: "a relative path several segments long",
        responses: || json!([entry(500, "500", Some("errors/static/50x.html"))]),
        offends: Some((0, "errors/static/50x.html")),
    },
    Row {
        case: "a dot-relative path",
        responses: || json!([entry(500, "500", Some("./50x.html"))]),
        offends: Some((0, "./50x.html")),
    },
    Row {
        case: "an absolute URL rather than a path",
        responses: || json!([entry(404, "404", Some("https://example.com/404.html"))]),
        offends: Some((0, "https://example.com/404.html")),
    },
    Row {
        case: "a backslash where the forward slash belongs",
        responses: || json!([entry(404, "404", Some("\\404.html"))]),
        offends: Some((0, "\\404.html")),
    },
    Row {
        case: "the second of two entries carries the offending path",
        responses: || {
            json!([
                entry(404, "404", Some("/404.html")),
                entry(403, "200", Some("denied.html")),
            ])
        },
        offends: Some((1, "denied.html")),
    },
];

/// One `DistributionCustomErrorResponse`, with the page path when the case
/// carries one.
fn entry(origin_status: i64, response_code: &str, page_path: Option<&str>) -> Value {
    let mut response = json!({
        "errorCode": origin_status,
        "responseCode": response_code,
        "errorCachingMinTTL": 10,
    });
    if let Some(page_path) = page_path {
        response["responsePagePath"] = json!(page_path);
    }
    response
}

/// `value` with every null member dropped, so a list sent without an optional
/// member compares equal to the one a read reports.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Array(entries) => Value::Array(entries.iter().map(without_nulls).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// Stand up the origin instance and the distribution the update cases apply
/// their lists to, settled into [`DEPLOYED`] so each case starts from a stable
/// record.
async fn stand_up(emulator: &Emulator) {
    create_running_instance(emulator, ORIGIN_INSTANCE, &format!("{DISTRIBUTION_REGION}a")).await;
    emulator
        .ok("CreateDistribution", create_body(DISTRIBUTION, json!([])))
        .await;
    wait_for_distribution_status(emulator, DISTRIBUTION, DEPLOYED).await;
}

/// The `CreateDistribution` body for `name`, carrying `responses` as its custom
/// error response list.
fn create_body(name: &str, responses: Value) -> Value {
    json!({
        "distributionName": name,
        "bundleId": BUNDLE_ID,
        "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
        "defaultCacheBehavior": {"behavior": "cache"},
        "customErrorResponses": responses,
    })
}

/// Assert `body` is this rule's refusal: the modelled `InvalidInputException`,
/// whose message names the entry that is wrong — the member, its index in the
/// list, and the page path the request carried there.
fn assert_refusal(case: &str, status: u16, body: &Value, index: usize, path: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "InvalidInputException",
        "case {case}: expected InvalidInputException, got {code} {body:?}"
    );
    let names = format!("customErrorResponses[{index}].{MEMBER} '{path}'");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains(&names)),
        "case {case}: expected a message naming {names}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// UpdateDistribution
// ---------------------------------------------------------------------------

/// RULE: `UpdateDistribution` stores a custom error response list whose every
/// page path begins with a forward slash and reports it back unchanged, and
/// refuses one that carries a path that does not — changing nothing.
#[tokio::test]
async fn update_distribution_takes_only_custom_error_page_paths_of_the_origin() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for row in ROWS {
        let case = row.case;
        let before = distribution(&emulator, DISTRIBUTION).await;
        let responses = (row.responses)();
        let (status, body) = emulator
            .call(
                "UpdateDistribution",
                json!({
                    "distributionName": DISTRIBUTION,
                    "customErrorResponses": responses.clone(),
                }),
            )
            .await;

        if let Some((index, path)) = row.offends {
            assert_refusal(case, status, &body, index, path);
            let after = distribution(&emulator, DISTRIBUTION).await;
            assert_eq!(
                after, before,
                "case {case}: expected the refused update to change nothing, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        // The round trip: what the update stored is what the next read reports.
        let reported = wait_for_distribution_status(&emulator, DISTRIBUTION, DEPLOYED).await;
        let got = without_nulls(&reported["customErrorResponses"]);
        assert_eq!(
            got,
            without_nulls(&responses),
            "case {case}: expected {responses:?} read back, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateDistribution
// ---------------------------------------------------------------------------

/// RULE: the same rule holds on the create — a distribution is created with the
/// list it was given when every page path begins with a forward slash, and a
/// request carrying one that does not creates no distribution at all.
#[tokio::test]
async fn create_distribution_takes_only_custom_error_page_paths_of_the_origin() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for (row_index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("created-cdn-{row_index}");
        let responses = (row.responses)();
        let (status, body) = emulator
            .call("CreateDistribution", create_body(&name, responses.clone()))
            .await;

        if let Some((index, path)) = row.offends {
            assert_refusal(case, status, &body, index, path);
            // A refused create stores nothing, so the name addresses no
            // distribution at all.
            let (read_status, read_body) = emulator
                .call("GetDistributions", json!({"distributionName": name}))
                .await;
            assert_eq!(
                read_status, 400,
                "case {case}: expected the refused create to store nothing, got {read_status} \
                 {read_body:?}"
            );
            assert_eq!(
                read_body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: expected the {DOES_NOT_EXIST} code, got {read_body:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        let reported = distribution(&emulator, &name).await;
        let got = without_nulls(&reported["customErrorResponses"]);
        assert_eq!(
            got,
            without_nulls(&responses),
            "case {case}: expected {responses:?} read back, got {got:?}"
        );
    }
}
