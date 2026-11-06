// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that an update writes the parameter ENTRIES it names into the
//! map a configuration definition holds.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `ConfigurationParametersMap` is a map of independently named
//! parameters, so an update naming some of them writes those entries over the stored map
//! and leaves every entry it does not name as it was stored — and every rule a stored
//! parameter set must satisfy, the deployment targets it must name included, is judged
//! over the map the definition HOLDS once those entries are applied, never over the
//! entries one request happened to carry. `UpdateConfigurationDefinitionInput$Parameters`
//! is optional and the model states no cross-member requirement of its own for this
//! operation (`smithy_query(query="operation",
//! operation="UpdateConfigurationDefinition")`); the parameters a DEFINITION must name
//! are documented on `ConfigurationDefinitionInput$Parameters`, which a definition whose
//! stored map names them keeps satisfying.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    STATUS_DEPLOYING, STATUS_INITIALIZING, STATUS_SUCCEEDED, STATUS_TYPE_DEPLOYMENT,
    TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns, and so
/// the only Region whose endpoint serves it.
const REGION: &str = "us-west-2";

/// A configuration type the service publishes (`ListQuickSetupTypes`), so no call below
/// is refused for naming a type the service does not offer.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// An account id this emulator does not serve, used where a definition targets an
/// account by id.
const OTHER_ACCOUNT: &str = "111122223333";

/// The sentence the service answers when the parameter map a definition would hold names
/// no deployment target.
const NO_TARGET_MESSAGE: &str =
    "Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.";

/// Long enough that the clock-owed deployment transition has come due.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// The account this emulator serves, which a definition naming no `TargetAccounts` is
/// deployed in.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

/// One emulator with an empty store, mounted exactly as the router mounts it, with the
/// administration role every local deployment below names already in the account
/// (`tests/all/local_deployment_roles.rs`, which owns that rule).
fn emulator() -> MultiServiceRouter {
    super::local_deployment_roles::emulator()
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

/// The `Message` a refusal carries.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
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

/// `CreateConfigurationManager` with ONE definition carrying `parameters`, which must
/// succeed; answers the manager ARN it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, parameters: &Value) -> String {
    let body = json!({
        "Name": "partial-parameter-updates",
        "ConfigurationDefinitions": [{
            "Type": TYPE,
            "TypeVersion": "LATEST",
            "Parameters": parameters,
            "LocalDeploymentExecutionRoleName": "QuickSetupExecution",
            "LocalDeploymentAdministrationRoleArn":
                super::local_deployment_roles::administration_role_arn(),
        }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The manager a `GetConfigurationManager` reports, which must succeed.
async fn get_manager_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

/// The id and the `Parameters` of the manager's single stored configuration definition,
/// read back through `GetConfigurationManager`.
async fn stored_definition(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> (String, Value) {
    let manager = get_manager_ok(router, case, manager_arn).await;
    let definition = manager["ConfigurationDefinitions"][0].clone();
    let id = definition["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a definition Id, got {manager:?}"))
        .to_string();
    let parameters = definition
        .get("Parameters")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected Parameters, got {manager:?}"));
    (id, parameters)
}

/// `PUT /configurationDefinition/{ManagerArn}/{Id}` carrying `entries` as its whole
/// `Parameters` member.
async fn update_parameters(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    entries: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(&json!({ "Parameters": entries })),
    )
    .await
}

/// The (`Account`, `Region`) pair of every configuration deployed for one manager, read
/// through `ListConfigurations`.
async fn deployed_pairs(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String)> {
    let (status, page) = call(
        router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": manager_arn })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurations must succeed, got {status} / {page:?}"
    );
    page["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigurationsList, got {page:?}"))
        .iter()
        .map(|row| {
            let account = row["Account"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an Account, got {row:?}"));
            let region = row["Region"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Region, got {row:?}"));
            (account.to_string(), region.to_string())
        })
        .collect()
}

/// The `Deployment` summary's `Status`, which is what the clock settles.
fn deployment_status(case: &str, summaries: &Value) -> String {
    summaries
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected StatusSummaries, got {summaries:?}"))
        .iter()
        .find(|summary| {
            summary.get("StatusType").and_then(Value::as_str) == Some(STATUS_TYPE_DEPLOYMENT)
        })
        .and_then(|summary| summary.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {summaries:?}")
        })
        .to_string()
}

/// One row: the case, the parameter map the definition is created with, the entries one
/// update writes into it, the map the definition must then HOLD, and the account-and-
/// Region pairs it must then be deployed to.
type Row = (
    &'static str,
    Value,
    Value,
    Value,
    Vec<(String, String)>,
);

/// The table: partial updates spanning what the rule generalises over — HOW MANY entries
/// the update names, whether each one is a key the definition already holds or a new
/// one, whether the entry it writes changes the value or restates it, whether it is a
/// type-specific parameter or one of the common deployment-target keys, and which target
/// keys the STORED map provides (the accounts, the organizational units, or both). No
/// row re-sends the whole stored map, which is the point: every row leaves the entries it
/// does not name in place, so the deployment-target parameters of the stored definition
/// survive an update that never mentions them.
fn rows(local: &str) -> Vec<Row> {
    vec![
        (
            // The reported shape: a tag-targeted definition whose accounts and Regions
            // are stored, retargeted at another tag.
            "the tag targeting of an account-targeted definition",
            json!({
                "TargetType": "Tags",
                "TargetTagKey": "Environment",
                "TargetTagValue": "production",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "us-east-1",
            }),
            json!({
                "TargetType": "Tags",
                "TargetTagKey": "Team",
                "TargetTagValue": "payments",
            }),
            json!({
                "TargetType": "Tags",
                "TargetTagKey": "Team",
                "TargetTagValue": "payments",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "us-east-1",
            }),
            vec![(OTHER_ACCOUNT.to_string(), "us-east-1".to_string())],
        ),
        (
            "one type-specific parameter of an account-targeted definition",
            json!({
                "UpdateSSMAgent": "false",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "eu-west-1,ap-south-1",
            }),
            json!({ "UpdateSSMAgent": "true" }),
            json!({
                "UpdateSSMAgent": "true",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "eu-west-1,ap-south-1",
            }),
            vec![
                (OTHER_ACCOUNT.to_string(), "eu-west-1".to_string()),
                (OTHER_ACCOUNT.to_string(), "ap-south-1".to_string()),
            ],
        ),
        (
            "a parameter the definition did not hold at all",
            json!({ "TargetAccounts": OTHER_ACCOUNT, "TargetRegions": "us-east-1" }),
            json!({ "TargetType": "*" }),
            json!({
                "TargetType": "*",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "us-east-1",
            }),
            vec![(OTHER_ACCOUNT.to_string(), "us-east-1".to_string())],
        ),
        (
            // A common target key is a parameter like any other: naming the accounts
            // leaves the stored organizational units in place, and the accounts are what
            // the definition is then deployed to.
            "the accounts added to an organizational-unit-targeted definition",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" }),
            json!({ "TargetAccounts": OTHER_ACCOUNT }),
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetAccounts": OTHER_ACCOUNT,
                "TargetRegions": "us-east-1",
            }),
            vec![(OTHER_ACCOUNT.to_string(), "us-east-1".to_string())],
        ),
        (
            "several entries at once, one of them new",
            json!({
                "TargetType": "*",
                "TargetAccounts": local,
                "TargetRegions": "us-east-1",
            }),
            json!({
                "TargetType": "Tags",
                "TargetTagKey": "Environment",
                "TargetTagValue": "production",
            }),
            json!({
                "TargetType": "Tags",
                "TargetTagKey": "Environment",
                "TargetTagValue": "production",
                "TargetAccounts": local,
                "TargetRegions": "us-east-1",
            }),
            vec![(local.to_string(), "us-east-1".to_string())],
        ),
        (
            // Restating the value the definition already holds leaves the map exactly as
            // it was: an update is judged by what it stores, not by what it sent.
            "one entry whose value the definition already holds",
            json!({
                "TargetType": "*",
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "us-east-1",
            }),
            json!({ "TargetRegions": "us-east-1" }),
            json!({
                "TargetType": "*",
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "us-east-1",
            }),
            vec![(local.to_string(), "us-east-1".to_string())],
        ),
    ]
}

/// A partial `Parameters` update is served, writes exactly the entries it names into the
/// stored map, and leaves the definition deployed where the map it then holds says.
#[tokio::test]
async fn an_update_writes_the_parameter_entries_it_names_into_the_stored_map() {
    let local = local_account();
    for (case, stored, entries, held, targets) in rows(&local) {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let manager_arn = create_ok(&router, case, &stored).await;
        let (id, before) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            before, stored,
            "case {case:?}: the create must store the parameters it named, got {before:?}"
        );

        let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateConfigurationDefinition must accept {entries:?}, \
             got {status} / {answer:?}"
        );
        assert_eq!(
            answer,
            Value::Null,
            "case {case:?}: the modelled output carries no members, so the body must be \
             empty, got {answer:?}"
        );

        let (_, after) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            after, held,
            "case {case:?}: the definition must hold {held:?}, got {after:?}"
        );

        let want: BTreeSet<(String, String)> = targets.into_iter().collect();
        let pairs = deployed_pairs(&router, case, &manager_arn).await;
        assert_eq!(
            pairs, want,
            "case {case:?}: the definition must be deployed to exactly the targets the map \
             it holds names, expected {want:?}, got {pairs:?}"
        );

        // Replaying the same entries stores the same map, so the definition is left
        // exactly as the first call left it (`@idempotent`).
        let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed update must succeed, got {status} / {answer:?}"
        );
        let (_, replayed) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            replayed, held,
            "case {case:?}: a replayed update must leave {held:?}, got {replayed:?}"
        );
    }
}

/// A partial update is refused when the map the definition would HOLD names nowhere to
/// deploy to — which an update can only bring about by writing a stored target key with
/// no value — and the stored definition survives the refusal unchanged.
#[tokio::test]
async fn an_update_that_leaves_the_held_map_without_a_target_is_refused() {
    let local = local_account();
    // (case, the parameters the definition is created with, the entries the update writes)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the stored accounts written empty",
            json!({ "TargetAccounts": &local, "TargetRegions": "us-east-1" }),
            json!({ "TargetAccounts": "" }),
        ),
        (
            "the stored accounts written as separators only",
            json!({ "TargetAccounts": OTHER_ACCOUNT, "TargetRegions": "us-east-1" }),
            json!({ "TargetAccounts": " , " }),
        ),
        (
            "the only one of the two target keys the definition holds written empty",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" }),
            json!({ "TargetOrganizationalUnits": "", "TargetType": "*" }),
        ),
    ];

    for (case, stored, entries) in cases {
        let router = emulator();
        let manager_arn = create_ok(&router, case, &stored).await;
        let (id, before) = stored_definition(&router, case, &manager_arn).await;
        let pairs_before = deployed_pairs(&router, case, &manager_arn).await;

        let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateConfigurationDefinition must refuse {entries:?}, \
             got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            error_message(&answer),
            NO_TARGET_MESSAGE,
            "case {case:?}: expected the refusal to state the rule, got {answer:?}"
        );

        let (_, after) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored parameters alone, got {after:?}"
        );
        let pairs_after = deployed_pairs(&router, case, &manager_arn).await;
        assert_eq!(
            pairs_after, pairs_before,
            "case {case:?}: a refused update must leave the deployments alone, got {pairs_after:?}"
        );
    }
}

/// A partial update of a SETTLED definition runs the deployment again: the manager
/// reports a transient `Deployment` status straight after the call and re-settles into
/// its outcome once the span the clock owes has passed, with no further call.
#[tokio::test]
async fn a_partial_update_of_a_settled_definition_redeploys_it() {
    let case = "redeployment from a partial update";
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        case,
        &json!({
            "TargetType": "Tags",
            "TargetTagKey": "Environment",
            "TargetTagValue": "production",
            "TargetAccounts": OTHER_ACCOUNT,
            "TargetRegions": "us-east-1",
        }),
    )
    .await;
    let (id, _) = stored_definition(&router, case, &manager_arn).await;

    // Let the create's own deployment settle first, so what the update does to the
    // summary is observable.
    tokio::time::sleep(past_transition()).await;
    let settled = get_manager_ok(&router, case, &manager_arn).await;
    assert_eq!(
        deployment_status(case, &settled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "case {case:?}: the create's deployment must settle first, got {settled:?}"
    );

    let entries = json!({ "TargetTagKey": "Team", "TargetTagValue": "payments" });
    let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateConfigurationDefinition must accept {entries:?}, \
         got {status} / {answer:?}"
    );

    let fresh = get_manager_ok(&router, case, &manager_arn).await;
    let transient = deployment_status(case, &fresh["StatusSummaries"]);
    assert!(
        transient == STATUS_INITIALIZING || transient == STATUS_DEPLOYING,
        "case {case:?}: a caller reading straight after the update must see the deployment \
         running again, expected {STATUS_INITIALIZING} or {STATUS_DEPLOYING}, got {transient:?}"
    );

    tokio::time::sleep(past_transition()).await;
    let resettled = get_manager_ok(&router, case, &manager_arn).await;
    assert_eq!(
        deployment_status(case, &resettled["StatusSummaries"]),
        STATUS_SUCCEEDED,
        "case {case:?}: the re-deployment must settle without any other call, got {resettled:?}"
    );

    // The update changed two entries of the map and left the rest, the stored deployment
    // targets included, which is why the redeployment above went to the same targets.
    let (_, after) = stored_definition(&router, case, &manager_arn).await;
    assert_eq!(
        after,
        json!({
            "TargetType": "Tags",
            "TargetTagKey": "Team",
            "TargetTagValue": "payments",
            "TargetAccounts": OTHER_ACCOUNT,
            "TargetRegions": "us-east-1",
        }),
        "case {case:?}: the definition must hold the stored map with the update's entries \
         applied, got {after:?}"
    );
}
