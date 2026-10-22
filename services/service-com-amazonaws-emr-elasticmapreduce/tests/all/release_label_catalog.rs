// SPDX-License-Identifier: CC0-1.0

//! `DescribeReleaseLabel` over the wire: the Amazon EMR release catalog.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.DescribeReleaseLabel` and an `awsJson1_1` body — so
//! these are the same bytes an SDK client sends.
//!
//! The rule under test: a release label the catalog offers is answered with the label as
//! the catalog spells it, the applications that release LINE bundles (each with the
//! version the newest catalog row at or before the line carries) and the Amazon Linux
//! releases its generation offers; any other label is refused with
//! `InvalidRequestException` (HTTP 400). The recorded case is one row of the tables
//! below.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::catalog_release_labels;
use service_com_amazonaws_emr_elasticmapreduce::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One row of the version rule: a release label, the applications it must report at a
/// given version, and the applications it must not report at all.
#[derive(Debug)]
struct VersionRow {
    label: &'static str,
    expected: &'static [(&'static str, &'static str)],
    absent: &'static [&'static str],
}

/// Rows of the version rule, written out by hand from the Amazon EMR Release Guide's
/// release-version and "Application versions" tables (the same source
/// `state::APPLICATION_CATALOG` records): for each label, applications that release must
/// report at that version, and applications it must NOT report at all — either because
/// the release predates them (Iceberg arrives at 6.5.0, Trino at 6.4.0) or because
/// Amazon EMR dropped them (Ganglia, gone from 7.0.0).
///
/// `emr-6.7.0` and `emr-5.33.1` are rows the issue's recorded unit does not cite; both
/// inherit their versions from an earlier line, which is what the rule says they do.
const VERSION_ROWS: &[VersionRow] = &[
    VersionRow {
        label: "emr-5.33.1",
        expected: &[("spark", "2.4.7"), ("hadoop", "2.10.1"), ("hive", "2.3.7")],
        absent: &["trino", "iceberg", "jupyterenterprisegateway"],
    },
    VersionRow {
        label: "emr-5.36.0",
        expected: &[("spark", "2.4.8"), ("hadoop", "2.10.1"), ("hive", "2.3.9")],
        absent: &["trino", "iceberg"],
    },
    VersionRow {
        label: "emr-6.0.0",
        expected: &[("spark", "2.4.4"), ("hadoop", "3.2.1"), ("hive", "3.1.2")],
        absent: &["trino", "iceberg"],
    },
    VersionRow {
        label: "emr-6.7.0",
        expected: &[
            ("spark", "3.2.0"),
            ("hadoop", "3.2.1"),
            ("trino", "367"),
            ("iceberg", "0.11.0"),
            ("ganglia", "3.7.2"),
        ],
        absent: &[],
    },
    VersionRow {
        label: "emr-6.15.0",
        expected: &[
            ("spark", "3.4.1"),
            ("hadoop", "3.3.6"),
            ("hive", "3.1.3"),
            ("trino", "426"),
            ("iceberg", "1.4.0"),
        ],
        absent: &[],
    },
    VersionRow {
        label: "emr-7.0.0",
        expected: &[("spark", "3.5.0"), ("hadoop", "3.3.6"), ("iceberg", "1.4.2")],
        absent: &["ganglia"],
    },
    VersionRow {
        label: "emr-7.5.0",
        expected: &[("spark", "3.5.2"), ("hadoop", "3.4.0"), ("trino", "435")],
        absent: &["ganglia"],
    },
];

/// Labels that name no release the catalog offers, and so are refused: a patch release
/// never published on an offered line, a line that does not exist, a label that is not
/// in `emr-<major>.<minor>.<patch>` form at all, and spellings (padded, upper case,
/// space-suffixed) that differ from how the service spells the label.
const REFUSED_LABELS: &[&str] = &[
    "emr-6.15.1",
    "emr-6.99.0",
    "emr-9.0.0",
    "emr-4.0.0",
    "emr-6.15",
    "emr-6.15.0.1",
    "emr-06.15.0",
    "EMR-6.15.0",
    "emr-6.15.0 ",
    "6.15.0",
    "emr-latest",
    "not-a-release",
    "",
];

/// The Amazon Linux generation each Amazon EMR major release is built on, as the prefix
/// of the `OSRelease$Label` values it offers: Amazon EMR 5.30.0 through 6.x run Amazon
/// Linux 2 (`2.0.<date>.<n>`), and 7.x runs Amazon Linux 2023 (`2023.<minor>.<date>.<n>`).
const OS_GENERATION_ROWS: &[(&str, &str)] = &[
    ("emr-5.30.0", "2.0."),
    ("emr-5.36.2", "2.0."),
    ("emr-6.10.1", "2.0."),
    ("emr-6.15.0", "2.0."),
    ("emr-7.0.0", "2023."),
    ("emr-7.5.0", "2023."),
];

/// `MaxResultsNumber`'s published range is 1..100 (`smithy_query(query="operation",
/// operation="DescribeReleaseLabel")`), so both bounds are accepted and the value one
/// past each is refused.
const MAX_RESULTS_ACCEPTED: &[i64] = &[1, 2, 99, 100];
const MAX_RESULTS_REFUSED: &[i64] = &[0, 101];

static NULL: Value = Value::Null;

/// One emulator: a router carrying only this service, over one shared engine.
struct Emr {
    router: MultiServiceRouter,
}

impl Emr {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "elasticmapreduce.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// `DescribeReleaseLabel` for `label`, which must answer 200.
    async fn describe(&self, label: &str) -> Value {
        self.describe_request(json!({ "ReleaseLabel": label })).await
    }

    async fn describe_request(&self, body: Value) -> Value {
        let (status, payload) = self.call("DescribeReleaseLabel", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body}: DescribeReleaseLabel must answer 200, got {status} with {payload}"
        );
        payload
    }
}

fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

/// The `(Name, Version)` pairs of the answer's `Applications`, in the order it reports
/// them.
fn applications(payload: &Value) -> Vec<(String, String)> {
    at(payload, "Applications")
        .as_array()
        .unwrap_or_else(|| panic!("Applications must be a list, got {payload}"))
        .iter()
        .map(|application| {
            (
                at(application, "Name").as_str().unwrap_or_default().to_string(),
                at(application, "Version").as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn os_release_labels(payload: &Value) -> Vec<String> {
    at(payload, "AvailableOSReleases")
        .as_array()
        .unwrap_or_else(|| panic!("AvailableOSReleases must be a list, got {payload}"))
        .iter()
        .map(|release| at(release, "Label").as_str().unwrap_or_default().to_string())
        .collect()
}

fn assert_refused(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = at(payload, "__type").as_str().unwrap_or_default();
    assert!(
        code.contains("InvalidRequestException"),
        "case {case:?}: expected InvalidRequestException, got {code:?} in {payload}"
    );
}

/// Every label the catalog offers is reported with its own spelling, a non-empty
/// application list in name order, Amazon Linux releases in the documented form, and no
/// pagination token.
#[tokio::test]
async fn every_offered_release_label_reports_its_applications_and_os_releases() {
    let emr = Emr::new();
    let labels = catalog_release_labels();
    assert!(
        labels.len() > 10,
        "the catalog must offer the Amazon EMR releases, got {labels:?}"
    );

    for label in &labels {
        let payload = emr.describe(label).await;
        assert_eq!(
            at(&payload, "ReleaseLabel").as_str(),
            Some(label.as_str()),
            "case {label:?}: the answer must describe the release asked for, got {payload}"
        );
        assert!(
            at(&payload, "NextToken").is_null(),
            "case {label:?}: NextToken is reserved for future use and must be absent, got {payload}"
        );

        let reported = applications(&payload);
        assert!(
            !reported.is_empty(),
            "case {label:?}: an Amazon EMR release bundles applications, got {payload}"
        );
        for (name, version) in &reported {
            assert!(
                !name.is_empty() && !version.is_empty(),
                "case {label:?}: every application needs a name and a version, got {reported:?}"
            );
        }
        let mut sorted = reported.clone();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            reported, sorted,
            "case {label:?}: applications must be reported in name order, got {reported:?}"
        );
        let mut names: Vec<&String> = reported.iter().map(|(name, _)| name).collect();
        let before = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "case {label:?}: an application must be reported once, got {reported:?}"
        );

        let os_releases = os_release_labels(&payload);
        assert!(
            !os_releases.is_empty(),
            "case {label:?}: an Amazon EMR release offers Amazon Linux releases, got {payload}"
        );
        for os_release in &os_releases {
            let parts: Vec<&str> = os_release.split('.').collect();
            assert!(
                parts.len() == 4
                    && parts.iter().all(|part| {
                        !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit())
                    })
                    && parts.iter().any(|part| part.len() == 8),
                "case {label:?}: Amazon Linux release {os_release:?} must be in the documented \
                 dotted release-note form, e.g. 2.0.20220218.1"
            );
        }
    }
}

/// The version rule: a release reports the version its LINE carries, and reports nothing
/// for an application that line does not bundle.
#[tokio::test]
async fn an_application_version_is_the_one_its_release_line_carries() {
    let emr = Emr::new();
    for VersionRow {
        label,
        expected,
        absent,
    } in VERSION_ROWS
    {
        let payload = emr.describe(label).await;
        let reported = applications(&payload);
        for (name, version) in *expected {
            let found = reported
                .iter()
                .find(|(reported_name, _)| reported_name == name)
                .map(|(_, reported_version)| reported_version.as_str());
            assert_eq!(
                found,
                Some(*version),
                "case {label:?}: expected {name} {version}, got {found:?} in {reported:?}"
            );
        }
        for name in *absent {
            let found = reported.iter().find(|(reported_name, _)| reported_name == name);
            assert!(
                found.is_none(),
                "case {label:?}: {name} is not bundled by that release, got {found:?}"
            );
        }
    }
}

/// Patch releases of one line bundle the same applications: the version rule is keyed by
/// the release LINE.
#[tokio::test]
async fn patch_releases_of_a_line_report_the_same_applications() {
    let emr = Emr::new();
    for (left, right) in [
        ("emr-5.36.0", "emr-5.36.2"),
        ("emr-6.10.0", "emr-6.10.1"),
        ("emr-6.0.0", "emr-6.0.1"),
    ] {
        let left_applications = applications(&emr.describe(left).await);
        let right_applications = applications(&emr.describe(right).await);
        assert_eq!(
            left_applications, right_applications,
            "case {left:?} vs {right:?}: patch releases of one line bundle the same applications"
        );
    }
}

/// The Amazon Linux generation an Amazon EMR release runs follows its major release.
#[tokio::test]
async fn os_releases_follow_the_amazon_linux_generation_of_the_release() {
    let emr = Emr::new();
    for (label, prefix) in OS_GENERATION_ROWS {
        let os_releases = os_release_labels(&emr.describe(label).await);
        assert!(
            !os_releases.is_empty()
                && os_releases
                    .iter()
                    .all(|os_release| os_release.starts_with(prefix)),
            "case {label:?}: expected Amazon Linux releases starting {prefix:?}, got {os_releases:?}"
        );
    }
}

/// A label that names no release the catalog offers is refused, and so is a request that
/// names no label at all.
#[tokio::test]
async fn a_label_naming_no_offered_release_is_refused() {
    let emr = Emr::new();
    for label in REFUSED_LABELS {
        let (status, payload) = emr
            .call("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        assert_refused(label, status, &payload);
    }

    let (status, payload) = emr.call("DescribeReleaseLabel", json!({})).await;
    assert_refused("<no ReleaseLabel>", status, &payload);

    // An explicit JSON `null` for a string member is refused by the protocol
    // deserializer (a 400 `ValidationException`) before the handler sees the request,
    // so this case pins the status rather than the handler's own error.
    let (status, payload) = emr
        .call("DescribeReleaseLabel", json!({ "ReleaseLabel": Value::Null }))
        .await;
    assert_eq!(
        status, 400,
        "case <null ReleaseLabel>: expected HTTP 400, got {status} with {payload}"
    );
}

/// `MaxResults` and `NextToken` are reserved for future use on this operation: a value
/// inside the published range is accepted and changes nothing about the answer, and one
/// outside it is refused by the modelled constraint.
#[tokio::test]
async fn reserved_pagination_members_do_not_change_the_answer() {
    let emr = Emr::new();
    let label = "emr-6.15.0";
    let baseline = emr.describe(label).await;

    for max_results in MAX_RESULTS_ACCEPTED {
        let payload = emr
            .describe_request(json!({ "ReleaseLabel": label, "MaxResults": max_results }))
            .await;
        assert_eq!(
            payload, baseline,
            "case MaxResults={max_results}: the member is reserved, so the answer must not change"
        );
    }

    for max_results in MAX_RESULTS_REFUSED {
        let (status, payload) = emr
            .call(
                "DescribeReleaseLabel",
                json!({ "ReleaseLabel": label, "MaxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: outside the published range 1..100, got {status} \
             with {payload}"
        );
    }

    let with_token = emr
        .describe_request(json!({ "ReleaseLabel": label, "NextToken": "reserved" }))
        .await;
    assert_eq!(
        with_token, baseline,
        "case NextToken: the member is reserved, so the answer must not change"
    );
}

/// Describing a release does not change what a later caller sees: the catalog is
/// reported, never consumed.
#[tokio::test]
async fn describing_a_release_changes_nothing_a_later_caller_sees() {
    let emr = Emr::new();
    for label in ["emr-6.15.0", "emr-7.5.0", "emr-5.36.2"] {
        let first = emr.describe(label).await;
        let _ = emr.describe("emr-6.0.0").await;
        let second = emr.describe(label).await;
        assert_eq!(
            first, second,
            "case {label:?}: a read must answer identically however many reads precede it"
        );
    }
}
