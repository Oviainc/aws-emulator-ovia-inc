// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CONFIGURATION VERSIONS this service publishes and
//! deploys at.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: the published type catalog is this service's authority for
//! configuration VERSIONS as well as for types, because a version names one shipped
//! revision of the service-owned template a type deploys. So
//!
//! * `ListQuickSetupTypes` reports, per type, the newest version of that type's own
//!   published history — a per-type value, since each type's template is versioned
//!   independently;
//! * a definition is deployed at a version that type publishes: the version the
//!   request names (matched by what it numbers, reported in the published spelling),
//!   the type's newest when the request names the modelled `LATEST` alias, and the
//!   type's newest when it names no version at all — so a caller always reads which
//!   version was deployed;
//! * a version the type does not publish names no template and is refused with the
//!   modelled 400, with nothing stored for it.
//!
//! The rows of each table differ along what the rule generalises over: the type, the
//! length and the shape of its published history, the spelling of the requested
//! version, and whether the request names one at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    type_version_order, QUICK_SETUP_TYPE_CATALOG, TYPE_VERSION_LATEST,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
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

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// One definition naming `type_`, carrying `version` only when the case supplies one.
fn definition(type_: &str, version: Option<&str>) -> Value {
    let mut definition = json!({ "Type": type_, "Parameters": { "TargetRegions": REGION } });
    if let Some(version) = version {
        definition["TypeVersion"] = json!(version);
    }
    definition
}

/// `POST /configurationManager` with one definition.
async fn create(
    router: &MultiServiceRouter,
    type_: &str,
    version: Option<&str>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/configurationManager",
        Some(&json!({
            "Name": "versions",
            "ConfigurationDefinitions": [definition(type_, version)],
        })),
    )
    .await
}

/// The one definition `GetConfigurationManager` reports for `manager_arn`.
async fn stored_definition(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .and_then(|definitions| definitions.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected one stored definition, got {answer:?}"))
}

/// The one configuration `ListConfigurations` reports for `manager_arn`, read back
/// through `GetConfiguration` — the deployment of the stored definition.
async fn deployed_configuration(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": manager_arn })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
    );
    let id = answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected one deployed configuration, got {answer:?}"))
        .to_string();
    let (status, got) = call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(&id)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfiguration must succeed, got {status} / {got:?}"
    );
    got
}

/// `PUT /configurationDefinition/{ManagerArn}/{Id}`.
async fn update_version(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    version: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(&json!({ "TypeVersion": version })),
    )
    .await
}

/// The ARN a successful create minted.
fn manager_arn(case: &str, status: u16, answer: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The catalog entry a type is published under.
fn entry(type_: &str) -> &'static service_com_amazonaws_ssmquicksetup_quicksetup::state::QuickSetupTypeEntry
{
    QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == type_)
        .unwrap_or_else(|| panic!("{type_:?} must be a published Quick Setup type"))
}

// ---------------------------------------------------------------------------
// The catalog: one version history per type, its newest published as LatestVersion.
// ---------------------------------------------------------------------------

/// `ListQuickSetupTypes` reports, for every type, the newest version of that type's
/// own published history — recomputed here from the history itself rather than read
/// from one shared constant — and the types do not all report the same version,
/// because each type's template advances on its own.
#[tokio::test]
async fn each_published_type_reports_the_newest_version_of_its_own_history() {
    let router = emulator();
    let (status, answer) = call(&router, "GET", "/listQuickSetupTypes", None).await;
    assert_eq!(
        status, 200,
        "ListQuickSetupTypes must answer 200, got {status} / {answer:?}"
    );
    let reported = answer
        .get("QuickSetupTypeList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a QuickSetupTypeList, got {answer:?}"))
        .clone();
    assert_eq!(
        reported.len(),
        QUICK_SETUP_TYPE_CATALOG.len(),
        "expected one entry per published type, got {reported:?}"
    );

    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for published in QUICK_SETUP_TYPE_CATALOG {
        let case = published.type_;
        // The newest of the type's own history, by what each version NUMBERS, computed
        // without asking the code under test which one that is.
        let newest = published
            .versions
            .iter()
            .copied()
            .max_by_key(|version| {
                type_version_order(version)
                    .unwrap_or_else(|| panic!("case {case}: {version:?} must be a numbered version"))
            })
            .unwrap_or_else(|| panic!("case {case}: a type must publish at least one version"));
        assert!(
            !published.versions.is_empty(),
            "case {case}: a published type must publish at least one version"
        );
        let got = reported
            .iter()
            .find(|got| got.get("Type").and_then(Value::as_str) == Some(case))
            .unwrap_or_else(|| panic!("case {case}: the catalog must report this type, got {reported:?}"));
        assert_eq!(
            got.get("LatestVersion").and_then(Value::as_str),
            Some(newest),
            "case {case}: expected the newest version of its history {:?}, got {got:?}",
            published.versions
        );
        distinct.insert(newest.to_string());
    }
    assert!(
        distinct.len() > 1,
        "the published types must not all report one shared version, got {distinct:?}"
    );
}

// ---------------------------------------------------------------------------
// A definition is deployed at, and reports, a version its type publishes.
// ---------------------------------------------------------------------------

/// What a create stores and every read reports is a version the catalog publishes for
/// the definition's own type: the one the request named, the type's newest for the
/// modelled `LATEST` alias, and the type's newest when the request named none. Each
/// row is read back through `GetConfigurationManager` and through the configuration
/// the service deployed from the definition.
#[tokio::test]
async fn a_definition_is_deployed_at_a_published_version_of_its_type() {
    // (case, type, the version the request names, the version every read must report)
    let rows: Vec<(&str, &str, Option<&str>, String)> = vec![
        (
            "no version named: the type's newest is deployed",
            "AWSQuickSetupType-CFGRecording",
            None,
            entry("AWSQuickSetupType-CFGRecording").latest_version().to_string(),
        ),
        (
            "no version named, on a type publishing exactly one",
            "AWSQuickSetupType-DevOpsGuru",
            None,
            entry("AWSQuickSetupType-DevOpsGuru").latest_version().to_string(),
        ),
        (
            "the LATEST alias",
            "AWSQuickSetupType-Distributor",
            Some(TYPE_VERSION_LATEST),
            entry("AWSQuickSetupType-Distributor").latest_version().to_string(),
        ),
        (
            "the oldest version the type publishes",
            "AWSQuickSetupType-CFGCPacks",
            Some(entry("AWSQuickSetupType-CFGCPacks").versions[0]),
            entry("AWSQuickSetupType-CFGCPacks").versions[0].to_string(),
        ),
        (
            "the newest version the type publishes, named outright",
            "AWSQuickSetupType-PatchPolicy",
            Some(entry("AWSQuickSetupType-PatchPolicy").latest_version()),
            entry("AWSQuickSetupType-PatchPolicy").latest_version().to_string(),
        ),
        (
            // The modelled pattern `^\d{1,3}(\.\d{1,3})?$|^LATEST$` admits a major with
            // no minor, which numbers the same version as `<major>.0`: the spelling the
            // catalog publishes is what is stored.
            "a published version spelled without its minor",
            "AWSQuickSetupType-Scheduler",
            Some("2"),
            "2.0".to_string(),
        ),
        (
            "a middle version of a three-version history",
            "AWSQuickSetupType-SSMHostMgmt",
            Some("1.1"),
            "1.1".to_string(),
        ),
    ];

    for (case, type_, named, want) in rows {
        // One emulator per row, so no row observes another row's writes.
        let router = emulator();
        let (status, answer) = create(&router, type_, named).await;
        let arn = manager_arn(case, status, &answer);

        let definition = stored_definition(&router, case, &arn).await;
        assert_eq!(
            definition.get("TypeVersion").and_then(Value::as_str),
            Some(want.as_str()),
            "case {case}: expected the stored definition at version {want:?}, got {definition:?}"
        );
        let configuration = deployed_configuration(&router, case, &arn).await;
        assert_eq!(
            configuration.get("TypeVersion").and_then(Value::as_str),
            Some(want.as_str()),
            "case {case}: expected the deployed configuration at version {want:?}, got {configuration:?}"
        );
        // Whatever was deployed, it is a version the catalog publishes for that type:
        // never the raw alias, and never a version nothing publishes.
        assert!(
            entry(type_).versions.contains(&want.as_str()),
            "case {case}: {want:?} must be one of the versions {type_} publishes {:?}",
            entry(type_).versions
        );
    }
}

/// An update moves a definition to a version its type publishes, and the deployed
/// configuration follows it: the request's own version, or the type's newest for the
/// `LATEST` alias.
#[tokio::test]
async fn an_update_moves_a_definition_to_a_published_version_of_its_type() {
    let type_ = "AWSQuickSetupType-SSMHostMgmt";
    let published = entry(type_);
    // (case, the version the update names, the version every read must then report)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("forward to a newer published version", "1.1", "1.1"),
        ("on to the newest published version", published.latest_version(), published.latest_version()),
        ("the LATEST alias", TYPE_VERSION_LATEST, published.latest_version()),
        ("back to the oldest published version", published.versions[0], published.versions[0]),
        ("a published version spelled without its minor", "2", "2.0"),
    ];

    for (case, named, want) in rows {
        let router = emulator();
        let (status, answer) = create(&router, type_, Some(published.versions[0])).await;
        let arn = manager_arn(case, status, &answer);
        let id = stored_definition(&router, case, &arn)
            .await
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: the stored definition must carry its Id"))
            .to_string();

        let (status, answer) = update_version(&router, &arn, &id, named).await;
        assert_eq!(
            status, 200,
            "case {case}: an update naming {named:?} must succeed, got {status} / {answer:?}"
        );

        let definition = stored_definition(&router, case, &arn).await;
        assert_eq!(
            definition.get("TypeVersion").and_then(Value::as_str),
            Some(want),
            "case {case}: expected the definition at version {want:?}, got {definition:?}"
        );
        let configuration = deployed_configuration(&router, case, &arn).await;
        assert_eq!(
            configuration.get("TypeVersion").and_then(Value::as_str),
            Some(want),
            "case {case}: expected the deployed configuration at version {want:?}, got {configuration:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A version the type does not publish is refused.
// ---------------------------------------------------------------------------

/// A version the catalog does not publish for the named type names no revision of
/// that type's template, so the create is refused with the modelled 400
/// `ValidationException` and nothing is stored for it — including a version this
/// service publishes for a DIFFERENT type, since each type's template is versioned
/// independently.
#[tokio::test]
async fn a_create_naming_a_version_the_type_does_not_publish_is_refused() {
    let router = emulator();

    // (case, type, the version the request names)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a version past everything the type publishes",
            "AWSQuickSetupType-SSMHostMgmt",
            "99.99",
        ),
        (
            // The widest form the modelled pattern admits is still only a form.
            "the widest version the modelled pattern admits",
            "AWSQuickSetupType-SSMHostMgmt",
            "999.999",
        ),
        (
            "a version below the first one published",
            "AWSQuickSetupType-DevOpsGuru",
            "0.9",
        ),
        (
            // `1.2` is published for `AWSQuickSetupType-CFGRecording` only.
            "a version another type publishes",
            "AWSQuickSetupType-SSMOpsCenter",
            "1.2",
        ),
        (
            // `2.0` is published for `AWSQuickSetupType-Scheduler`, not for this type.
            "a major another type publishes",
            "AWSQuickSetupType-DHMC",
            "2.0",
        ),
        (
            // `ConfigurationDefinitionInput$TypeVersion` carries only a
            // `@length(1, 128)`, so the form reaches the handler: three components name
            // no version.
            "a version with three components",
            "AWSQuickSetupType-SSM",
            "1.2.3",
        ),
        (
            "the alias in the wrong case",
            "AWSQuickSetupType-SSM",
            "latest",
        ),
        (
            "a version with four digits",
            "AWSQuickSetupType-SSM",
            "1234",
        ),
        (
            "text that numbers nothing",
            "AWSQuickSetupType-SSM",
            "newest",
        ),
    ];

    for (case, type_, named) in rows {
        let (status, answer) = create(&router, type_, Some(named)).await;
        assert_eq!(
            status, 400,
            "case {case}: {named:?} must be refused with 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected a ValidationException, got {answer:?}"
        );
    }

    // Not one of those creates stored anything: no manager, and no configuration.
    for (uri, member) in [
        ("/listConfigurationManagers", "ConfigurationManagersList"),
        ("/listConfigurations", "ConfigurationsList"),
    ] {
        let (status, answer) = call(&router, "POST", uri, Some(&json!({}))).await;
        assert_eq!(
            status, 200,
            "the listing must succeed, got {status} / {answer:?}"
        );
        let listed = answer
            .get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            listed.is_empty(),
            "a refused create must store nothing; {member} holds {listed:?}"
        );
    }
}

/// The same rule on the update path: a version the definition's own type does not
/// publish is refused, and the definition — and the configuration deployed from it —
/// stay at the version they were stored at.
#[tokio::test]
async fn an_update_naming_a_version_the_type_does_not_publish_is_refused() {
    let type_ = "AWSQuickSetupType-ResourceExplorer";
    let published = entry(type_);
    let router = emulator();
    let (status, answer) = create(&router, type_, Some(published.versions[0])).await;
    let arn = manager_arn("refusals", status, &answer);
    let stored = stored_definition(&router, "refusals", &arn).await;
    let id = stored
        .get("Id")
        .and_then(Value::as_str)
        .expect("the stored definition must carry its Id")
        .to_string();

    for (case, named) in [
        ("a version past everything the type publishes", "9.9"),
        ("the widest version the modelled pattern admits", "999.999"),
        // `1.2` is published for `AWSQuickSetupType-CFGRecording` only.
        ("a version another type publishes", "1.2"),
        // `2.0` is published for `AWSQuickSetupType-Scheduler`, not for this type.
        ("a major another type publishes", "2"),
    ] {
        let (status, answer) = update_version(&router, &arn, &id, named).await;
        assert_eq!(
            status, 400,
            "case {case}: {named:?} must be refused with 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected a ValidationException, got {answer:?}"
        );

        let after = stored_definition(&router, case, &arn).await;
        assert_eq!(
            after.get("TypeVersion").and_then(Value::as_str),
            Some(published.versions[0]),
            "case {case}: a refused update must leave the stored version alone, got {after:?}"
        );
        let configuration = deployed_configuration(&router, case, &arn).await;
        assert_eq!(
            configuration.get("TypeVersion").and_then(Value::as_str),
            Some(published.versions[0]),
            "case {case}: a refused update must leave the deployed version alone, got {configuration:?}"
        );
    }
}
