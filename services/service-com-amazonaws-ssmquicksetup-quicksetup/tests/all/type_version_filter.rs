// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `TypeVersion` `Filters` key of `ListConfigurations`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a filter selects the records whose value for its key the filter
//! NAMES, and a configuration version is named the way every member naming a version is
//! read — resolved against the versions the record's own type publishes — rather than
//! compared as raw text. So every spelling of a published version names it (a major
//! alone numbers `<major>.0`, as the modelled `TypeVersion` pattern
//! `^\d{1,3}(\.\d{1,3})?$|^LATEST$` admits), the modelled `LATEST` alias names each
//! type's newest published version, and a value naming no version of a record's type
//! excludes it. Every other filterable member holds text that is its own value, so it
//! is named by exactly that text.
//!
//! The rows differ along what the rule generalises over: the type, the length and shape
//! of its published history, the version a record is deployed at, the spelling the
//! filter names it by, how many values one filter carries, and whether a second filter
//! narrows beside it.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    type_version_order, QuickSetupTypeEntry, QUICK_SETUP_TYPE_CATALOG, TYPE_VERSION_LATEST,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for, and the one Region every definition
/// deploys into — so each definition fans out to exactly one configuration.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes
/// on and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
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
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
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

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The catalog entry a type is published under: the authority for the versions that
/// type offers, which every expectation below is computed from rather than written out.
fn entry(type_: &str) -> &'static QuickSetupTypeEntry {
    QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == type_)
        .unwrap_or_else(|| panic!("{type_:?} must be a published Quick Setup type"))
}

/// The newest version a type publishes, by what each version of its history NUMBERS —
/// recomputed here from that history rather than asked of the code under test.
fn newest_version(type_: &str) -> &'static str {
    entry(type_)
        .versions
        .iter()
        .copied()
        .max_by_key(|version| {
            type_version_order(version)
                .unwrap_or_else(|| panic!("{version:?} must be a numbered version"))
        })
        .unwrap_or_else(|| panic!("{type_:?} must publish at least one version"))
}

/// Whether a filter value of `wanted` names the version `version` of a record
/// deploying `type_`: the rule under test, written out from the published catalog.
///
/// The modelled alias names whichever version that type publishes as its newest. Every
/// other value names the version it NUMBERS, so a version is named by each of its
/// spellings — with its minor and, when that minor is zero, without — while text that
/// numbers nothing names nothing.
fn names_version(type_: &str, version: &str, wanted: &str) -> bool {
    if wanted == TYPE_VERSION_LATEST {
        return newest_version(type_) == version;
    }
    type_version_order(wanted).is_some_and(|wanted| Some(wanted) == type_version_order(version))
}

/// The deployments this suite stores: one configuration definition per row, each
/// deployed at a version its own type publishes. The rows span a type with a
/// three-version history at each of its versions, a type whose history skips the minors
/// between its majors, a type publishing exactly one version, and a type whose newest
/// published version carries a non-zero minor.
const DEPLOYMENTS: &[(&str, &str)] = &[
    ("AWSQuickSetupType-SSMHostMgmt", "1.0"),
    ("AWSQuickSetupType-SSMHostMgmt", "1.1"),
    ("AWSQuickSetupType-SSMHostMgmt", "2.0"),
    ("AWSQuickSetupType-Scheduler", "1.0"),
    ("AWSQuickSetupType-DevOpsGuru", "1.0"),
    ("AWSQuickSetupType-CFGRecording", "1.2"),
];

/// An emulator holding one configuration per entry of [`DEPLOYMENTS`], created through
/// one manager so every configuration is served by the signed Region's endpoint.
async fn emulator_with_deployments() -> MultiServiceRouter {
    let router = emulator();
    let definitions: Vec<Value> = DEPLOYMENTS
        .iter()
        .map(|(type_, version)| {
            json!({
                "Type": type_,
                "TypeVersion": version,
                // Somewhere to deploy to, the Region to deploy into and every parameter
                // the table documented for that type marks `(Required)`: no row here is
                // about those requirements.
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    type_,
                    json!({
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": REGION,
                    }),
                ),
            })
        })
        .collect();
    let (status, answer) = call(
        &router,
        "POST",
        "/configurationManager",
        Some(&json!({ "Name": "versions", "ConfigurationDefinitions": definitions })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    router
}

/// The `(Type, TypeVersion)` pair of every configuration `ListConfigurations` reports
/// for `body`, which identifies a row of [`DEPLOYMENTS`] uniquely.
async fn listed(router: &MultiServiceRouter, case: &str, body: &Value) -> Deployments {
    let (status, answer) = call(router, "POST", "/listConfigurations", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case}: the listing must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .iter()
        .map(|row| {
            let type_ = row
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a row must report its Type, got {row:?}"));
            let version = row.get("TypeVersion").and_then(Value::as_str).unwrap_or_else(
                || panic!("case {case}: a row must report its TypeVersion, got {row:?}"),
            );
            (type_.to_string(), version.to_string())
        })
        .collect()
}

/// One deployment as a listing reports it: its configuration type and the version it
/// is deployed at, which identifies a row of [`DEPLOYMENTS`] uniquely.
type Deployment = (String, String);

/// The deployments one case expects a listing to report, in a set so the comparison is
/// of membership rather than of order.
type Deployments = BTreeSet<Deployment>;

/// One row of a table keyed by the values ONE filter carries: the case, those values,
/// and the deployments the listing must report for them.
type ValuesRow<'a> = (&'a str, Vec<&'a str>, Deployments);

/// One row of a table keyed by a whole `Filters` list: the case, that list, and the
/// deployments the listing must report for it.
type FiltersRow<'a> = (&'a str, Value, Deployments);

/// The deployments of [`DEPLOYMENTS`] a predicate admits, as a set of pairs.
fn expected(keep: impl Fn(&str, &str) -> bool) -> Deployments {
    DEPLOYMENTS
        .iter()
        .filter(|(type_, version)| keep(type_, version))
        .map(|(type_, version)| (type_.to_string(), version.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// What a `TypeVersion` filter value names.
// ---------------------------------------------------------------------------

/// A `TypeVersion` filter reports the configurations whose deployed version one of its
/// `Values` names: every spelling of a published version names it, the modelled alias
/// names each type's newest, several values name the union of what each names, and a
/// value naming no version of a record's type reports nothing.
#[tokio::test]
async fn a_type_version_filter_reports_the_configurations_its_values_name() {
    let router = emulator_with_deployments().await;

    // Every stored deployment is listed before any filter narrows it.
    let all = listed(&router, "unfiltered", &json!({})).await;
    assert_eq!(
        all,
        expected(|_, _| true),
        "every deployment must be listed unfiltered, got {all:?}"
    );

    // (case, the values the filter carries, the deployments it must report)
    let rows: Vec<ValuesRow> = vec![
        (
            "a major alone names the version published as <major>.0",
            vec!["1"],
            expected(|_, version| version == "1.0"),
        ),
        (
            "a major whose .0 only one of these types is deployed at",
            vec!["2"],
            expected(|_, version| version == "2.0"),
        ),
        (
            "the same version spelled with its zero minor",
            vec!["1.0"],
            expected(|_, version| version == "1.0"),
        ),
        (
            "a version whose minor is not zero, spelled in full",
            vec!["1.1"],
            expected(|_, version| version == "1.1"),
        ),
        (
            "a version only the longest published history reaches",
            vec!["1.2"],
            expected(|_, version| version == "1.2"),
        ),
        (
            "the modelled alias names each type's newest published version",
            vec![TYPE_VERSION_LATEST],
            expected(|type_, version| newest_version(type_) == version),
        ),
        (
            "two values name the union of what each names",
            vec!["1", "2"],
            expected(|_, version| version == "1.0" || version == "2.0"),
        ),
        (
            "a spelling and the alias together",
            vec!["1.1", TYPE_VERSION_LATEST],
            expected(|type_, version| version == "1.1" || newest_version(type_) == version),
        ),
        (
            // Three digits is the widest major the modelled `TypeVersion` pattern
            // (`^\d{1,3}(\.\d{1,3})?$|^LATEST$`) admits, and no type publishes it.
            "a major no type publishes",
            vec!["999"],
            BTreeSet::new(),
        ),
        (
            "a major below every published version",
            vec!["0"],
            BTreeSet::new(),
        ),
        (
            "a minor no deployment is at",
            vec!["2.1"],
            BTreeSet::new(),
        ),
        (
            "text that numbers no version",
            vec!["newest"],
            BTreeSet::new(),
        ),
        (
            // The alias means something only against the published catalog, so no other
            // casing of it is the alias.
            "the alias in the wrong case",
            vec!["latest"],
            BTreeSet::new(),
        ),
    ];

    for (case, values, want) in rows {
        let got = listed(
            &router,
            case,
            &json!({ "Filters": [{ "Key": "TypeVersion", "Values": values }] }),
        )
        .await;
        assert_eq!(
            got, want,
            "case {case}: filtering on {values:?} must report {want:?}, got {got:?}"
        );
    }
}

/// The rule swept over every type deployed here and every candidate spelling, with the
/// expectation computed from the published catalog instead of written out: for each
/// type, a `TypeVersion` filter reports that type's deployments the value names, and no
/// others.
#[tokio::test]
async fn every_spelling_of_a_version_names_the_deployments_at_it() {
    let router = emulator_with_deployments().await;

    // Majors with and without a minor, a minor past every history, and the alias.
    let spellings = [
        "0", "1", "2", "3", "1.0", "1.1", "1.2", "2.0", "2.1", TYPE_VERSION_LATEST,
    ];
    let types: BTreeSet<&str> = DEPLOYMENTS.iter().map(|(type_, _)| *type_).collect();

    for type_ in types {
        for wanted in spellings {
            let case = format!("{type_} filtered on {wanted:?}");
            let got = listed(
                &router,
                &case,
                &json!({
                    "Filters": [
                        { "Key": "Type", "Values": [type_] },
                        { "Key": "TypeVersion", "Values": [wanted] },
                    ],
                }),
            )
            .await;
            let want = expected(|row_type, row_version| {
                row_type == type_ && names_version(row_type, row_version, wanted)
            });
            assert_eq!(
                got, want,
                "case {case}: expected {want:?}, got {got:?}"
            );
        }
    }

    // Nothing in the sweep narrowed the store away: every deployment is still listed.
    let all = listed(&router, "after the sweep", &json!({})).await;
    assert_eq!(
        all,
        expected(|_, _| true),
        "a listing must not change what a later one reports, got {all:?}"
    );
}

/// The version spelling a listing REPORTS names the record it came from: filtering on
/// a row's own `Type` and `TypeVersion` reports that row and nothing else.
#[tokio::test]
async fn the_reported_version_names_the_configuration_that_reported_it() {
    let router = emulator_with_deployments().await;
    let all = listed(&router, "round trip", &json!({})).await;
    assert!(
        !all.is_empty(),
        "the suite must deploy something to read back, got {all:?}"
    );

    for (type_, version) in &all {
        let case = format!("{type_} at {version}");
        let got = listed(
            &router,
            &case,
            &json!({
                "Filters": [
                    { "Key": "Type", "Values": [type_] },
                    { "Key": "TypeVersion", "Values": [version] },
                ],
            }),
        )
        .await;
        let want: Deployments = [(type_.clone(), version.clone())].into_iter().collect();
        assert_eq!(
            got, want,
            "case {case}: a row's own reported version must name it, expected {want:?}, got {got:?}"
        );
    }
}

/// A `TypeVersion` filter narrows BESIDE the other filter keys rather than instead of
/// them: every filter must hold, and the keys whose values are their own text keep
/// matching as text.
#[tokio::test]
async fn a_type_version_filter_narrows_beside_the_text_valued_keys() {
    let router = emulator_with_deployments().await;
    let scheduler = "AWSQuickSetupType-Scheduler";
    let host_mgmt = "AWSQuickSetupType-SSMHostMgmt";

    // (case, filters, the deployments the listing must report)
    let rows: Vec<FiltersRow> = vec![
        (
            "a type and a version both hold",
            json!([
                { "Key": "Type", "Values": [scheduler] },
                { "Key": "TypeVersion", "Values": ["1"] },
            ]),
            expected(|type_, version| type_ == scheduler && version == "1.0"),
        ),
        (
            "a type nothing is deployed at that version reports nothing",
            json!([
                { "Key": "Type", "Values": ["AWSQuickSetupType-DevOpsGuru"] },
                { "Key": "TypeVersion", "Values": ["2"] },
            ]),
            BTreeSet::new(),
        ),
        (
            "the alias beside a type reports that type's newest deployment",
            json!([
                { "Key": "Type", "Values": [host_mgmt] },
                { "Key": "TypeVersion", "Values": [TYPE_VERSION_LATEST] },
            ]),
            expected(|type_, version| {
                type_ == host_mgmt && newest_version(host_mgmt) == version
            }),
        ),
        (
            "the Region every definition deployed into, beside a version",
            json!([
                { "Key": "Region", "Values": [REGION] },
                { "Key": "TypeVersion", "Values": ["1"] },
            ]),
            expected(|_, version| version == "1.0"),
        ),
        (
            "a Region nothing deployed into reports nothing",
            json!([
                { "Key": "Region", "Values": ["eu-north-1"] },
                { "Key": "TypeVersion", "Values": [TYPE_VERSION_LATEST] },
            ]),
            BTreeSet::new(),
        ),
        (
            "a type whose text differs only in case is not that type",
            json!([
                { "Key": "Type", "Values": ["awsquicksetuptype-scheduler"] },
                { "Key": "TypeVersion", "Values": ["1"] },
            ]),
            BTreeSet::new(),
        ),
    ];

    for (case, filters, want) in rows {
        let got = listed(&router, case, &json!({ "Filters": filters })).await;
        assert_eq!(
            got, want,
            "case {case}: filtering on {filters:?} must report {want:?}, got {got:?}"
        );
    }
}

/// `ConfigurationDefinitionId` and `Id` stay text-matched: each names exactly the
/// record reporting it, so the version rule did not spread to the keys whose values
/// have one spelling. Read from the listing itself, since both are service-generated.
#[tokio::test]
async fn the_identifier_keys_still_name_exactly_their_own_record() {
    let router = emulator_with_deployments().await;
    let (status, answer) = call(&router, "POST", "/listConfigurations", Some(&json!({}))).await;
    assert_eq!(
        status, 200,
        "the listing must succeed, got {status} / {answer:?}"
    );
    let rows = answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ConfigurationsList, got {answer:?}"))
        .clone();
    assert_eq!(
        rows.len(),
        DEPLOYMENTS.len(),
        "expected one configuration per deployment, got {rows:?}"
    );

    let mut seen: HashSet<String> = HashSet::new();
    for key in ["Id", "ConfigurationDefinitionId"] {
        for row in &rows {
            let value = row
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a row must report its {key}, got {row:?}"));
            let case = format!("{key} {value}");
            let got = listed(
                &router,
                &case,
                &json!({ "Filters": [{ "Key": key, "Values": [value] }] }),
            )
            .await;
            let want: Deployments = [(
                row["Type"].as_str().unwrap_or_default().to_string(),
                row["TypeVersion"].as_str().unwrap_or_default().to_string(),
            )]
            .into_iter()
            .collect();
            assert_eq!(
                got, want,
                "case {case}: expected the one record reporting it {want:?}, got {got:?}"
            );
            assert!(
                seen.insert(format!("{key}={value}")),
                "case {case}: every record must report its own {key}"
            );
        }
    }
}

/// A `Filters` key `ListConfigurations` does not filter on is still refused with the
/// modelled 400 `ValidationException`, so a narrowing request is never answered with
/// unnarrowed results.
#[tokio::test]
async fn a_filter_key_this_listing_does_not_filter_on_is_still_refused() {
    let router = emulator_with_deployments().await;

    for (case, key) in [
        // `ManagerArn` names the manager through this operation's own member instead.
        ("the manager's ARN", "ManagerArn"),
        ("a member of the other listing's shape", "Name"),
        ("a version member spelled differently", "Version"),
        ("the empty key", ""),
    ] {
        let (status, answer) = call(
            &router,
            "POST",
            "/listConfigurations",
            Some(&json!({ "Filters": [{ "Key": key, "Values": ["1"] }] })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: key {key:?} must be refused with 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected a ValidationException, got {answer:?}"
        );
    }
}
