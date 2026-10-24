// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for where a Quick Setup configuration definition is deployed.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a configuration definition names where it is deployed through
//! its `Parameters`, and the service deploys ONE configuration per targeted account and
//! Region pair — the cross product of the accounts `TargetAccounts` names (the account
//! this emulator serves when it names none) and the Regions `TargetRegions` names (the
//! manager's own Region when it names none) — and each deployed configuration reports
//! that pair as its `Account` and `Region` and is individually readable by the id the
//! service generated for it. An update that changes those parameters changes the
//! deployed set the same way.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::MAX_DEPLOYMENT_TARGETS;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns, and so
/// the Region a definition naming no `TargetRegions` deploys in.
const REGION: &str = "us-west-2";

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for the reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

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

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
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
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// `CreateConfigurationManager` with ONE definition carrying `parameters`.
async fn create(router: &MultiServiceRouter, name: &str, parameters: &Value) -> (u16, Value) {
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{ "Type": TYPE, "Parameters": parameters }],
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// A create that must succeed, answering the manager ARN it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    parameters: &Value,
) -> String {
    let (status, answer) = create(router, name, parameters).await;
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

/// The `ConfigurationsList` rows reported for one manager.
async fn configurations_of(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> Vec<Value> {
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
    answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .clone()
}

/// `GET /getConfiguration/{ConfigurationId}`.
async fn get_configuration(router: &MultiServiceRouter, configuration_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(configuration_id)),
        None,
    )
    .await
}

/// The service-assigned id of the manager's single configuration definition.
async fn definition_id(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> String {
    let (status, manager) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {manager:?}"
    );
    manager["ConfigurationDefinitions"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a definition Id, got {manager:?}"))
        .to_string()
}

/// `PUT /configurationDefinition/{ManagerArn}/{Id}`.
async fn update_definition(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(body),
    )
    .await
}

/// The (`Account`, `Region`, `Id`) of every configuration deployed for one manager,
/// read through `ListConfigurations` and then confirmed, row by row, through
/// `GetConfiguration` — so the listing and the single read agree on every deployment.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String, String)> {
    let rows = configurations_of(router, case, manager_arn).await;
    let mut deployed = BTreeSet::new();
    for row in &rows {
        let id = row["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an Id, got {row:?}"));
        let account = row["Account"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an Account, got {row:?}"));
        let region = row["Region"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Region, got {row:?}"));

        let (status, got) = get_configuration(router, id).await;
        assert_eq!(
            status, 200,
            "case {case}: every deployed configuration must be readable by its id, got {status} / {got:?}"
        );
        assert_eq!(
            got["Account"].as_str(),
            Some(account),
            "case {case}: the Get and the listing must report one deployment account, got {got:?}"
        );
        assert_eq!(
            got["Region"].as_str(),
            Some(region),
            "case {case}: the Get and the listing must report one deployment Region, got {got:?}"
        );
        assert_eq!(
            got["ManagerArn"].as_str(),
            Some(manager_arn),
            "case {case}: a deployment must name the manager that owns it, got {got:?}"
        );
        assert!(
            deployed.insert((
                account.to_string(),
                region.to_string(),
                id.to_string()
            )),
            "case {case}: configuration id {id:?} was reported twice"
        );
    }
    assert_eq!(
        deployed.len(),
        rows.len(),
        "case {case}: expected one row per deployment, got {rows:?}"
    );
    deployed
}

/// The (account, Region) pairs of a deployed set, without the ids.
fn pairs(deployed: &BTreeSet<(String, String, String)>) -> BTreeSet<(String, String)> {
    deployed
        .iter()
        .map(|(account, region, _)| (account.clone(), region.clone()))
        .collect()
}

/// The id deployed at one (account, Region) pair, or `None` when nothing is.
fn id_at(
    deployed: &BTreeSet<(String, String, String)>,
    account: &str,
    region: &str,
) -> Option<String> {
    deployed
        .iter()
        .find(|(got_account, got_region, _)| got_account == account && got_region == region)
        .map(|(_, _, id)| id.clone())
}

/// The account this emulator serves, which a definition naming no `TargetAccounts`
/// deploys in.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One row of a fan-out table: the case, the definition's `Parameters`, and the
/// (account, Region) pairs the definition must then be deployed to.
type TargetCase = (&'static str, Value, Vec<(String, String)>);

/// A create deploys one configuration per targeted account and Region pair.
///
/// Each row differs along what the rule generalises over: how many Regions are
/// targeted, how many accounts, whether either is named at all, how the comma-separated
/// list is written (padding, empty entries, repeats), and whether the accounts are named
/// through `TargetAccounts` or left to the organizational units a definition may target
/// instead.
#[tokio::test]
async fn a_definition_deploys_one_configuration_per_targeted_account_and_region() {
    let router = emulator();
    let local = local_account();
    let other = "111122223333".to_string();
    let third = "444455556666".to_string();

    // (case, the definition's Parameters, the (account, Region) pairs it must deploy to)
    let cases: Vec<TargetCase> = vec![
        (
            "no target parameters at all",
            json!({}),
            vec![(local.clone(), REGION.to_string())],
        ),
        (
            "one targeted region",
            json!({ "TargetRegions": "eu-west-1" }),
            vec![(local.clone(), "eu-west-1".to_string())],
        ),
        (
            "three targeted regions",
            json!({ "TargetRegions": "us-east-1,eu-central-1,ap-southeast-2" }),
            vec![
                (local.clone(), "us-east-1".to_string()),
                (local.clone(), "eu-central-1".to_string()),
                (local.clone(), "ap-southeast-2".to_string()),
            ],
        ),
        (
            "a region list with padding, an empty entry and a repeat",
            json!({ "TargetRegions": " us-east-1 , ,us-east-1, ap-south-1 " }),
            vec![
                (local.clone(), "us-east-1".to_string()),
                (local.clone(), "ap-south-1".to_string()),
            ],
        ),
        (
            "organizational units and two regions",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "us-east-1,us-west-2",
            }),
            vec![
                (local.clone(), "us-east-1".to_string()),
                (local.clone(), "us-west-2".to_string()),
            ],
        ),
        (
            "one targeted account, no targeted region",
            json!({ "TargetAccounts": &other }),
            vec![(other.clone(), REGION.to_string())],
        ),
        (
            "two targeted accounts and two targeted regions",
            json!({
                "TargetAccounts": format!("{other},{third}"),
                "TargetRegions": "us-east-1,eu-west-1",
            }),
            vec![
                (other.clone(), "us-east-1".to_string()),
                (other.clone(), "eu-west-1".to_string()),
                (third.clone(), "us-east-1".to_string()),
                (third.clone(), "eu-west-1".to_string()),
            ],
        ),
        (
            "three targeted accounts in one region",
            json!({
                "TargetAccounts": format!("{local},{other},{third}"),
                "TargetRegions": "sa-east-1",
            }),
            vec![
                (local.clone(), "sa-east-1".to_string()),
                (other.clone(), "sa-east-1".to_string()),
                (third.clone(), "sa-east-1".to_string()),
            ],
        ),
    ];

    let mut every_id: BTreeSet<String> = BTreeSet::new();
    for (case, parameters, want) in cases {
        let manager_arn = create_ok(&router, case, "fanout", &parameters).await;
        let deployed = deployed(&router, case, &manager_arn).await;
        let want: BTreeSet<(String, String)> = want.into_iter().collect();
        assert_eq!(
            pairs(&deployed),
            want,
            "case {case}: expected deployments at {want:?}, got {deployed:?}"
        );
        for (_, _, id) in &deployed {
            assert!(
                every_id.insert(id.clone()),
                "case {case}: configuration id {id:?} was handed out twice"
            );
        }
    }
}

/// An update that rewrites the definition's target parameters rewrites the deployed set:
/// a target that stays keeps its configuration and its id, a target that goes keeps no
/// configuration (its id is then the modelled 404), and a newly named target gains one.
#[tokio::test]
async fn an_update_redeploys_the_definition_to_exactly_the_targets_it_now_names() {
    let case = "redeploy";
    let router = emulator();
    let local = local_account();
    let other = "111122223333".to_string();

    let manager_arn = create_ok(
        &router,
        case,
        "redeployed",
        &json!({ "TargetRegions": "us-east-1,us-west-2" }),
    )
    .await;
    let definition = definition_id(&router, case, &manager_arn).await;

    let initial = deployed(&router, case, &manager_arn).await;
    assert_eq!(
        pairs(&initial),
        [
            (local.clone(), "us-east-1".to_string()),
            (local.clone(), "us-west-2".to_string()),
        ]
        .into_iter()
        .collect::<BTreeSet<(String, String)>>(),
        "case {case}: the create must deploy one configuration per targeted region, got {initial:?}"
    );
    let kept = id_at(&initial, &local, "us-west-2")
        .unwrap_or_else(|| panic!("case {case}: expected a deployment in us-west-2, got {initial:?}"));
    let dropped = id_at(&initial, &local, "us-east-1")
        .unwrap_or_else(|| panic!("case {case}: expected a deployment in us-east-1, got {initial:?}"));

    // (case, the Parameters the update writes, the pairs the definition must then be
    // deployed to) — each row changes a different part of the target set: a Region
    // swapped, the accounts named, every target dropped.
    let steps: Vec<TargetCase> = vec![
        (
            "one region swapped for another",
            json!({ "TargetRegions": "us-west-2,eu-west-1" }),
            vec![
                (local.clone(), "us-west-2".to_string()),
                (local.clone(), "eu-west-1".to_string()),
            ],
        ),
        (
            "the same regions in another account",
            json!({ "TargetAccounts": &other, "TargetRegions": "us-west-2,eu-west-1" }),
            vec![
                (other.clone(), "us-west-2".to_string()),
                (other.clone(), "eu-west-1".to_string()),
            ],
        ),
        (
            "no targets at all, back to the manager's own region",
            json!({ "TargetRegions": "" }),
            vec![(local.clone(), REGION.to_string())],
        ),
    ];

    for (step, parameters, want) in steps {
        let (status, answer) = update_definition(
            &router,
            &manager_arn,
            &definition,
            &json!({ "Parameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {step}: UpdateConfigurationDefinition must succeed, got {status} / {answer:?}"
        );
        let after = deployed(&router, case, &manager_arn).await;
        let want: BTreeSet<(String, String)> = want.into_iter().collect();
        assert_eq!(
            pairs(&after),
            want,
            "case {step}: expected deployments at {want:?}, got {after:?}"
        );
    }

    // The target that survived the first update kept the id the create gave it (the next
    // case pins that); the second update moved the deployment into another account, so
    // both of the create's own configurations are gone — the Region being targeted again
    // by the third update deploys a fresh configuration, not the old one.
    let (status, answer) = get_configuration(&router, &kept).await;
    assert_eq!(
        status, 404,
        "case {case}: a configuration dropped by an update must stay gone when its target \
         is named again, got {status} / {answer:?}"
    );
    let (status, answer) = get_configuration(&router, &dropped).await;
    assert_eq!(
        status, 404,
        "case {case}: a configuration deployed to a target the definition no longer names \
         must be gone, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got {answer:?}"
    );
}

/// The id a target keeps across an update that still names it: the deployment is
/// re-submitted, not replaced, so a caller holding the id reads the new values from it.
#[tokio::test]
async fn a_target_the_update_still_names_keeps_its_configuration_id() {
    let case = "stable id";
    let router = emulator();
    let local = local_account();
    let manager_arn = create_ok(
        &router,
        case,
        "stable",
        &json!({ "TargetRegions": "us-east-1,us-west-2" }),
    )
    .await;
    let definition = definition_id(&router, case, &manager_arn).await;
    let before = deployed(&router, case, &manager_arn).await;
    let kept = id_at(&before, &local, "us-west-2")
        .unwrap_or_else(|| panic!("case {case}: expected a deployment in us-west-2, got {before:?}"));

    let (status, answer) = update_definition(
        &router,
        &manager_arn,
        &definition,
        &json!({
            "TypeVersion": "2.0",
            "Parameters": { "TargetRegions": "us-west-2,eu-west-1" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateConfigurationDefinition must succeed, got {status} / {answer:?}"
    );

    let after = deployed(&router, case, &manager_arn).await;
    assert_eq!(
        id_at(&after, &local, "us-west-2"),
        Some(kept.clone()),
        "case {case}: a target the update still names must keep its configuration id, got {after:?}"
    );
    let (status, got) = get_configuration(&router, &kept).await;
    assert_eq!(
        status, 200,
        "case {case}: that configuration must still be readable, got {status} / {got:?}"
    );
    assert_eq!(
        got["TypeVersion"].as_str(),
        Some("2.0"),
        "case {case}: the kept deployment must carry what the update wrote, got {got:?}"
    );
    assert_eq!(
        got["Parameters"]["TargetRegions"].as_str(),
        Some("us-west-2,eu-west-1"),
        "case {case}: the kept deployment must carry the parameters the update wrote, got {got:?}"
    );
    assert_eq!(
        got["ConfigurationDefinitionId"].as_str(),
        Some(definition.as_str()),
        "case {case}: the kept deployment must still name its definition, got {got:?}"
    );
}

/// A targeted account is named by its account id: `TargetAccounts` entries outside that
/// form are the modelled 400, and nothing is stored for the refused request.
#[tokio::test]
async fn a_target_account_that_is_not_an_account_id_is_refused() {
    let router = emulator();
    // Rows span the ways the value can miss the 12-digit account-id form, plus the two
    // accepted shapes: one id, and a comma-separated list of them.
    let cases: Vec<(&str, Value, u16)> = vec![
        ("one account id", json!({ "TargetAccounts": "111122223333" }), 200),
        (
            "two account ids",
            json!({ "TargetAccounts": "111122223333,444455556666" }),
            200,
        ),
        (
            "an account id with padding",
            json!({ "TargetAccounts": " 111122223333 " }),
            200,
        ),
        ("too few digits", json!({ "TargetAccounts": "11112222333" }), 400),
        (
            "too many digits",
            json!({ "TargetAccounts": "1111222233334" }),
            400,
        ),
        ("not digits at all", json!({ "TargetAccounts": "my-account" }), 400),
        (
            "an account alias beside a valid id",
            json!({ "TargetAccounts": "111122223333,my-account" }),
            400,
        ),
        (
            "an organizational unit named as an account",
            json!({ "TargetAccounts": "ou-1234-56789012" }),
            400,
        ),
    ];

    let mut stored_managers = 0_usize;
    for (case, parameters, want) in cases {
        let (status, answer) = create(&router, "accounts", &parameters).await;
        assert_eq!(
            status, want,
            "case {case}: expected {want}, got {status} / {answer:?}"
        );
        if want == 200 {
            stored_managers += 1;
        } else {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
        }
        // A refused create stores nothing: the manager count only ever moves on a
        // create that succeeded.
        let (status, listed) = call(
            &router,
            "POST",
            "/listConfigurationManagers",
            Some(&json!({ "MaxItems": 100 })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListConfigurationManagers must succeed, got {listed:?}"
        );
        let managers = listed["ConfigurationManagersList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a manager listing, got {listed:?}"));
        assert_eq!(
            managers.len(),
            stored_managers,
            "case {case}: expected {stored_managers} stored managers, got {listed:?}"
        );
    }
}

/// The fan-out is bounded: a definition naming exactly `MAX_DEPLOYMENT_TARGETS`
/// account-and-Region pairs is deployed to every one of them, and one naming more is the
/// modelled 400 — tested at the bound, one past it, and with the product split across
/// accounts as well as Regions.
#[tokio::test]
async fn a_definition_past_the_deployment_bound_is_refused() {
    let router = emulator();
    let regions = |count: usize| {
        (0..count)
            .map(|index| format!("eu-west-{}", index + 1))
            .collect::<Vec<String>>()
            .join(",")
    };
    let accounts = |count: usize| {
        (0..count)
            .map(|index| format!("{:012}", index + 1))
            .collect::<Vec<String>>()
            .join(",")
    };

    // (case, Parameters, expected status, how many deployments a success must produce)
    let cases: Vec<(&str, Value, u16, usize)> = vec![
        (
            "one region under the bound",
            json!({ "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS - 1) }),
            200,
            MAX_DEPLOYMENT_TARGETS - 1,
        ),
        (
            "exactly the bound, in regions",
            json!({ "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS) }),
            200,
            MAX_DEPLOYMENT_TARGETS,
        ),
        (
            "one region past the bound",
            json!({ "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS + 1) }),
            400,
            0,
        ),
        (
            "exactly the bound, split across accounts and regions",
            json!({
                "TargetAccounts": accounts(4),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            200,
            MAX_DEPLOYMENT_TARGETS,
        ),
        (
            "one account past the bound",
            json!({
                "TargetAccounts": accounts(5),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            400,
            0,
        ),
    ];

    for (case, parameters, want_status, want_deployments) in cases {
        let (status, answer) = create(&router, "bounded", &parameters).await;
        assert_eq!(
            status, want_status,
            "case {case}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
            continue;
        }
        let manager_arn = answer["ManagerArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
            .to_string();
        // The page bound is below the deployment bound, so the rows are counted by
        // paging the listing to its end.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = serde_json::Map::new();
            body.insert("ManagerArn".to_string(), json!(manager_arn));
            if let Some(token) = &token {
                body.insert("StartingToken".to_string(), json!(token));
            }
            let (status, page) =
                call(&router, "POST", "/listConfigurations", Some(&Value::Object(body))).await;
            assert_eq!(
                status, 200,
                "case {case}: ListConfigurations must succeed, got {status} / {page:?}"
            );
            for row in page["ConfigurationsList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {page:?}"))
            {
                seen.insert(row["Id"].as_str().unwrap_or_default().to_string());
            }
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            want_deployments,
            "case {case}: expected {want_deployments} deployments, got {}",
            seen.len()
        );
    }
}
