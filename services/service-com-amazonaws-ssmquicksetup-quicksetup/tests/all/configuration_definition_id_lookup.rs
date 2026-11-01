// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how a request's `ConfigurationDefinitionId` is resolved.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a request member that ADDRESSES a resource is resolved against
//! what the signed Region's endpoint holds before the response is built, and one naming
//! no such resource answers the modelled `ResourceNotFoundException` (HTTP 404) rather
//! than a success reporting nothing — so a caller can tell "that resource deploys
//! nothing" from "that resource does not exist". A configuration definition lives on the
//! manager that declares it, so the manager a request names is part of that resolution:
//! an id held by another manager, by a manager of another Region, or by no manager at all
//! names no definition the request addressed. Narrowing by `Filters` is a different
//! thing and is unaffected: a filter value selects records, it does not address a
//! resource (`tests/all/type_version_filter.rs`).
//!
//! The rows differ along what the rule generalises over: whether the request scopes the
//! id to a manager, which manager it scopes it to, the Region the manager was created
//! through, the length and character class of the id (both bounds of the modelled
//! `^[a-z0-9-]{1,20}$` pattern and one case past each), whether the id is a real one's
//! neighbouring text, how many configurations the definition it names deploys, and
//! whether the manager holding it is still in the store at all.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    QUICK_SETUP_TYPE_CATALOG, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region whose endpoint every request below is signed for unless a case names
/// another.
const REGION: &str = "us-west-2";

/// A second Region, so a case can hold a definition through an endpoint other than the
/// one that looks for it.
const OTHER_REGION: &str = "eu-central-1";

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

/// A request signed for `ssm-quicksetup` in `region`, the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
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

/// The `Message` a refusal carried, read through either casing restJson1 may render an
/// error structure's single message member under.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The sentence a refusal must carry when the request scoped the id to a manager —
/// written out here rather than read from the crate, so the test pins the text itself.
fn scoped_message(manager_arn: &str, id: &str) -> String {
    format!("configuration manager {manager_arn} holds no configuration definition with ID {id}")
}

/// The sentence a refusal must carry when the request scoped the id to nothing but the
/// endpoint it reached.
fn unscoped_message(id: &str) -> String {
    format!("no configuration definition with ID {id} exists")
}

/// The sentence a refusal must carry when the ARN the request named holds no manager at
/// all: the manager is resolved first, so that is the resource reported absent.
fn manager_message(manager_arn: &str) -> String {
    format!("Quick setup manager with arn: {manager_arn} could not be found.")
}

/// One configuration as `ListConfigurations` reports it, by what identifies which
/// definition deployed it where.
type Row = (String, String, String);

/// What a case expects of one `ListConfigurations` call.
#[derive(Debug)]
enum Expect {
    /// The modelled 200, reporting exactly these configurations.
    Page(BTreeSet<Row>),
    /// The modelled 404 carrying this `Message`.
    NotFound(String),
    /// The modelled 400: the value never reaches the handler's lookup, because the
    /// constraint walk refuses it first.
    Invalid,
}

/// One manager this suite created: its ARN and the ids the service assigned the
/// definitions it declares, read back through `GetConfigurationManager`.
struct Manager {
    arn: String,
    definition_ids: Vec<String>,
}

/// Create one configuration manager through `region`'s endpoint with one configuration
/// definition per entry of `target_regions`, then read it back for the ids the service
/// assigned its definitions.
///
/// `Type` must name a configuration type this service publishes and no case here is about
/// the type, so every definition uses the catalog's first entry; its parameters name
/// somewhere to deploy to and the Regions to deploy into, and provide every parameter the
/// table documented for that type marks `(Required)`, which every accepted definition
/// must. A definition deploys one configuration per Region it targets, so the entries
/// differ in how many configurations the definition they describe fans out to.
async fn create_manager(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    target_regions: &[&[&str]],
) -> Manager {
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let definitions: Vec<Value> = target_regions
        .iter()
        .map(|targets| {
            json!({
                "Type": type_,
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    type_,
                    json!({
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": targets.join(","),
                    }),
                ),
            })
        })
        .collect();
    let (status, answer) = call(
        router,
        region,
        "POST",
        "/configurationManager",
        Some(&json!({ "Name": name, "ConfigurationDefinitions": definitions })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name}: expected a ManagerArn, got {answer:?}"))
        .to_string();

    let (status, answer) = call(
        router,
        region,
        "GET",
        &format!("/configurationManager/{}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: the created manager must read back, got {status} / {answer:?}"
    );
    let definition_ids: Vec<String> = answer["ConfigurationDefinitions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected ConfigurationDefinitions, got {answer:?}"))
        .iter()
        .map(|definition| {
            definition
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {name}: a definition must report its Id, got {definition:?}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        definition_ids.len(),
        target_regions.len(),
        "case {name}: expected one definition per entry, got {definition_ids:?}"
    );
    Manager {
        arn,
        definition_ids,
    }
}

/// The configurations `ListConfigurations` reports for `body` through `region`'s
/// endpoint, as a set so the comparison is of membership rather than of order.
async fn listed(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> BTreeSet<Row> {
    let (status, answer) = call(router, region, "POST", "/listConfigurations", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case}: the listing must succeed, got {status} / {answer:?}"
    );
    rows(case, &answer)
}

/// The rows of a successful listing's `ConfigurationsList`.
fn rows(case: &str, answer: &Value) -> BTreeSet<Row> {
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .iter()
        .map(|row| {
            let field = |key: &str| {
                row.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: a row must report its {key}, got {row:?}"))
                    .to_string()
            };
            (
                field("ManagerArn"),
                field("ConfigurationDefinitionId"),
                field("Region"),
            )
        })
        .collect()
}

/// Drive one `ListConfigurations` through `region`'s endpoint and assert what the case
/// expects of it.
async fn assert_listing(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
    want: &Expect,
) {
    let (status, answer) = call(router, region, "POST", "/listConfigurations", Some(body)).await;
    match want {
        Expect::Page(expected) => {
            assert_eq!(
                status, 200,
                "case {case}: {body:?} must be answered, got {status} / {answer:?}"
            );
            let got = rows(case, &answer);
            assert_eq!(
                got, *expected,
                "case {case}: expected {expected:?}, got {got:?}"
            );
        }
        Expect::NotFound(message) => {
            assert_eq!(
                status, 404,
                "case {case}: {body:?} must be refused with the modelled 404, got {status} / {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {code:?} / {answer:?}"
            );
            let actual = error_message(&answer);
            assert_eq!(
                actual, *message,
                "case {case}: expected {message:?}, got {actual:?}"
            );
        }
        Expect::Invalid => {
            assert_eq!(
                status, 400,
                "case {case}: {body:?} must be refused with the modelled 400, got {status} / {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {code:?} / {answer:?}"
            );
        }
    }
}

/// The rows of `all` that `definition_id` deployed.
fn deployed_by(all: &BTreeSet<Row>, definition_id: &str) -> BTreeSet<Row> {
    all.iter()
        .filter(|(_, id, _)| id == definition_id)
        .cloned()
        .collect()
}

/// A well-formed ARN no create minted, satisfying the modelled `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`
/// (`smithy_query(query="operation",operation="ListConfigurations")`).
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/11111111-2222-3333-4444-555555555555";

/// The whole table: a definition id that resolves reports exactly the configurations that
/// definition deployed, and one that resolves to nothing is refused as absent.
#[tokio::test]
async fn a_configuration_definition_id_is_resolved_before_any_page_is_built() {
    let router = emulator();
    // One manager whose definitions fan out to a different number of Regions each, and a
    // second manager, so a request can name one manager's definition id while scoping it
    // to the other.
    let first = create_manager(
        &router,
        REGION,
        "first",
        &[&["us-east-1"], &[REGION, "us-east-2"], &["eu-west-1"]],
    )
    .await;
    let second = create_manager(&router, REGION, "second", &[&["us-east-1"]]).await;

    let all = listed(&router, REGION, "unfiltered", &json!({})).await;
    assert_eq!(
        all.len(),
        5,
        "the suite must deploy one configuration per targeted Region, got {all:?}"
    );

    // The longest and the shortest id the modelled `^[a-z0-9-]{1,20}$` pattern admits,
    // and one character past each bound; the service assigns ids far shorter than the
    // maximum, so none of these four names a stored definition.
    let longest_id = "abcdefghij0123456789";
    let too_long_id = "abcdefghij01234567890";
    // Neighbouring text of a real id: same characters plus one, so a lookup that matched
    // on anything but the whole id would answer this one.
    let neighbour_id = format!("{}0", first.definition_ids[0]);

    // (case, the Region the request is signed for, the request body, what it must answer)
    let cases: Vec<(&str, &str, Value, Expect)> = vec![
        (
            "a definition of the manager the request names",
            REGION,
            json!({ "ManagerArn": first.arn, "ConfigurationDefinitionId": first.definition_ids[0] }),
            Expect::Page(deployed_by(&all, &first.definition_ids[0])),
        ),
        (
            "a definition deployed to two Regions",
            REGION,
            json!({ "ManagerArn": first.arn, "ConfigurationDefinitionId": first.definition_ids[1] }),
            Expect::Page(deployed_by(&all, &first.definition_ids[1])),
        ),
        (
            "the same definition with no manager named",
            REGION,
            json!({ "ConfigurationDefinitionId": first.definition_ids[1] }),
            Expect::Page(deployed_by(&all, &first.definition_ids[1])),
        ),
        (
            "the only definition of the other manager",
            REGION,
            json!({ "ManagerArn": second.arn, "ConfigurationDefinitionId": second.definition_ids[0] }),
            Expect::Page(deployed_by(&all, &second.definition_ids[0])),
        ),
        (
            "a definition of the other manager, scoped to this one",
            REGION,
            json!({ "ManagerArn": second.arn, "ConfigurationDefinitionId": first.definition_ids[0] }),
            Expect::NotFound(scoped_message(&second.arn, &first.definition_ids[0])),
        ),
        (
            "a definition of this manager, scoped to the other",
            REGION,
            json!({ "ManagerArn": first.arn, "ConfigurationDefinitionId": second.definition_ids[0] }),
            Expect::NotFound(scoped_message(&first.arn, &second.definition_ids[0])),
        ),
        (
            "an id no manager holds, scoped to one",
            REGION,
            json!({ "ManagerArn": first.arn, "ConfigurationDefinitionId": "zzz" }),
            Expect::NotFound(scoped_message(&first.arn, "zzz")),
        ),
        (
            "an id no manager holds, scoped to nothing",
            REGION,
            json!({ "ConfigurationDefinitionId": "zzz" }),
            Expect::NotFound(unscoped_message("zzz")),
        ),
        (
            "the shortest id the pattern admits",
            REGION,
            json!({ "ConfigurationDefinitionId": "a" }),
            Expect::NotFound(unscoped_message("a")),
        ),
        (
            "the longest id the pattern admits",
            REGION,
            json!({ "ConfigurationDefinitionId": longest_id }),
            Expect::NotFound(unscoped_message(longest_id)),
        ),
        (
            "an id of hyphens alone",
            REGION,
            json!({ "ConfigurationDefinitionId": "---" }),
            Expect::NotFound(unscoped_message("---")),
        ),
        (
            "the neighbouring text of a real id",
            REGION,
            json!({ "ConfigurationDefinitionId": neighbour_id }),
            Expect::NotFound(unscoped_message(&neighbour_id)),
        ),
        (
            "one character past the pattern's length bound",
            REGION,
            json!({ "ConfigurationDefinitionId": too_long_id }),
            Expect::Invalid,
        ),
        (
            "characters outside the pattern's class",
            REGION,
            json!({ "ConfigurationDefinitionId": "ZZZ" }),
            Expect::Invalid,
        ),
        (
            "the empty id the pattern's lower bound excludes",
            REGION,
            json!({ "ConfigurationDefinitionId": "" }),
            Expect::Invalid,
        ),
        (
            // The manager is resolved first, so an absent one is the resource reported.
            "a real id scoped to a manager that was never created",
            REGION,
            json!({ "ManagerArn": UNKNOWN_MANAGER_ARN, "ConfigurationDefinitionId": first.definition_ids[0] }),
            Expect::NotFound(manager_message(UNKNOWN_MANAGER_ARN)),
        ),
        (
            "a definition of an endpoint of another Region",
            OTHER_REGION,
            json!({ "ConfigurationDefinitionId": first.definition_ids[0] }),
            Expect::NotFound(unscoped_message(&first.definition_ids[0])),
        ),
    ];

    for (case, region, body, want) in &cases {
        assert_listing(&router, region, case, body, want).await;
    }

    // None of those reads changed what a later caller sees.
    let after = listed(&router, REGION, "after the table", &json!({})).await;
    assert_eq!(
        after, all,
        "a listing must not change what a later one reports, expected {all:?}, got {after:?}"
    );
}

/// The rule swept over every definition this suite stored, with the expectation computed
/// from the full listing rather than written out: each id reports exactly the
/// configurations it deployed when it is scoped to its own manager or to nothing, and is
/// refused as absent when it is scoped to any other manager.
#[tokio::test]
async fn every_stored_definition_id_names_exactly_its_own_configurations() {
    let router = emulator();
    let managers = vec![
        create_manager(&router, REGION, "one", &[&["us-east-1"], &["eu-west-1", REGION]]).await,
        create_manager(&router, REGION, "two", &[&["us-east-2"]]).await,
        create_manager(&router, REGION, "three", &[&[REGION], &["us-east-1"], &["ap-south-1"]]).await,
    ];
    let all = listed(&router, REGION, "unfiltered", &json!({})).await;

    for manager in &managers {
        for id in &manager.definition_ids {
            let want = deployed_by(&all, id);
            assert!(
                !want.is_empty(),
                "case {id}: a stored definition must have deployed something, got {all:?}"
            );

            for (case, body) in [
                (
                    format!("{id} scoped to its own manager"),
                    json!({ "ManagerArn": manager.arn, "ConfigurationDefinitionId": id }),
                ),
                (
                    format!("{id} scoped to nothing"),
                    json!({ "ConfigurationDefinitionId": id }),
                ),
            ] {
                assert_listing(&router, REGION, &case, &body, &Expect::Page(want.clone())).await;
            }

            for other in &managers {
                if other.arn == manager.arn {
                    continue;
                }
                let case = format!("{id} scoped to the manager {}", other.arn);
                assert_listing(
                    &router,
                    REGION,
                    &case,
                    &json!({ "ManagerArn": other.arn, "ConfigurationDefinitionId": id }),
                    &Expect::NotFound(scoped_message(&other.arn, id)),
                )
                .await;
            }
        }
    }
}

/// A definition held through another Region's endpoint is absent here and present there:
/// the resolution is of what the endpoint the request reached holds, not of what the
/// store holds.
#[tokio::test]
async fn a_definition_of_another_regions_endpoint_is_absent_here() {
    let router = emulator();
    let local = create_manager(&router, REGION, "local", &[&[REGION]]).await;
    let remote = create_manager(&router, OTHER_REGION, "remote", &[&["us-east-1"]]).await;

    let here = listed(&router, REGION, "the local endpoint", &json!({})).await;
    let there = listed(&router, OTHER_REGION, "the other endpoint", &json!({})).await;

    for (case, region, body, want) in [
        (
            "the remote definition through its own endpoint",
            OTHER_REGION,
            json!({ "ManagerArn": remote.arn, "ConfigurationDefinitionId": remote.definition_ids[0] }),
            Expect::Page(deployed_by(&there, &remote.definition_ids[0])),
        ),
        (
            "the remote definition through this endpoint, scoped to nothing",
            REGION,
            json!({ "ConfigurationDefinitionId": remote.definition_ids[0] }),
            Expect::NotFound(unscoped_message(&remote.definition_ids[0])),
        ),
        (
            // The manager's ARN carries the other Region, so this endpoint holds no
            // manager under it and that is the resource reported absent.
            "the remote definition through this endpoint, scoped to its manager",
            REGION,
            json!({ "ManagerArn": remote.arn, "ConfigurationDefinitionId": remote.definition_ids[0] }),
            Expect::NotFound(manager_message(&remote.arn)),
        ),
        (
            "the local definition through the other endpoint",
            OTHER_REGION,
            json!({ "ConfigurationDefinitionId": local.definition_ids[0] }),
            Expect::NotFound(unscoped_message(&local.definition_ids[0])),
        ),
        (
            "the local definition through its own endpoint",
            REGION,
            json!({ "ManagerArn": local.arn, "ConfigurationDefinitionId": local.definition_ids[0] }),
            Expect::Page(deployed_by(&here, &local.definition_ids[0])),
        ),
    ] {
        assert_listing(&router, region, case, &body, &want).await;
    }
}

/// A definition id that resolved while its manager was in the store is absent once that
/// manager's teardown has come due: what the lookup answers is read off the store, not
/// off what any one request carried.
#[tokio::test]
async fn a_definition_id_of_a_torn_down_manager_becomes_absent() {
    let router = emulator();
    let doomed = create_manager(&router, REGION, "doomed", &[&["us-east-1"]]).await;
    let id = doomed.definition_ids[0].clone();

    let all = listed(&router, REGION, "before the delete", &json!({})).await;
    let want = deployed_by(&all, &id);
    assert!(
        !want.is_empty(),
        "the definition must have deployed something before the delete, got {all:?}"
    );
    assert_listing(
        &router,
        REGION,
        "before the delete",
        &json!({ "ConfigurationDefinitionId": id }),
        &Expect::Page(want),
    )
    .await;

    let (status, answer) = call(
        &router,
        REGION,
        "DELETE",
        &format!("/configurationManager/{}", label(&doomed.arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must be accepted, got {status} / {answer:?}"
    );
    // The delete starts an asynchronous teardown, so the manager and the definitions it
    // declares leave the store only once that teardown comes due.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;

    for (case, body, want) in [
        (
            "after the teardown, scoped to nothing",
            json!({ "ConfigurationDefinitionId": id }),
            Expect::NotFound(unscoped_message(&id)),
        ),
        (
            "after the teardown, scoped to the manager that is gone",
            json!({ "ManagerArn": doomed.arn, "ConfigurationDefinitionId": id }),
            Expect::NotFound(manager_message(&doomed.arn)),
        ),
    ] {
        assert_listing(&router, REGION, case, &body, &want).await;
    }
}
