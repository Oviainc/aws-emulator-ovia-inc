// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that a stored configuration definition always names somewhere
//! to deploy to.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a Quick Setup configuration definition is deployed where its
//! parameters say, so every request that writes a definition's parameter map — the
//! create that stores one and the update that writes entries into one — is accepted only
//! when the map the definition then HOLDS provides a value for either `TargetAccounts`
//! or `TargetOrganizationalUnits`, and is otherwise refused with a 400
//! `ValidationException` having written nothing. The
//! model's documentation for the member is where the rule is stated: a value must be
//! provided for either of the two
//! (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
//! member="Parameters")`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns.
const REGION: &str = "us-west-2";

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for the reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The sentence the service answers when a parameter map names no deployment target.
const NO_TARGET_MESSAGE: &str =
    "Either TargetAccounts or TargetOrganizationalUnits must be defined in parameters.";

/// The parameters every manager below is created with, so each update starts from a
/// definition that DOES name a deployment target and a refused update is visibly the
/// one that left it alone.
fn baseline_parameters() -> Value {
    json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" })
}

/// [`baseline_parameters`] with every entry `entries` names written over it: the
/// parameter map a definition HOLDS once an update carrying `entries` has been served,
/// since an update writes the entries it names and leaves the rest of the stored map
/// alone.
fn parameters_after(entries: &Value) -> Value {
    let mut map = baseline_parameters()
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (key, value) in entries.as_object().cloned().unwrap_or_default() {
        map.insert(key, value);
    }
    Value::Object(map)
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

/// `CreateConfigurationManager` with ONE definition carrying `parameters`.
///
/// Every definition names the administration role a LOCAL deployment must name, since a
/// case here may target accounts and none of them is about that rule
/// (`tests/all/local_deployment_roles.rs`).
async fn create(router: &MultiServiceRouter, name: &str, parameters: &Value) -> (u16, Value) {
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": TYPE,
            "Parameters": parameters,
            "LocalDeploymentAdministrationRoleArn":
                super::local_deployment_roles::administration_role_arn(),
        }],
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// A create that must succeed, answering the manager ARN it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, parameters: &Value) -> String {
    let (status, answer) = create(router, "targets", parameters).await;
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

/// `GET /configurationManager/{ManagerArn}`, which must succeed.
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

/// `PUT /configurationDefinition/{ManagerArn}/{Id}` writing `parameters`.
async fn update_parameters(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    parameters: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(&json!({ "Parameters": parameters })),
    )
    .await
}

/// How many configuration managers the store holds.
async fn stored_managers(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({ "MaxItems": 100 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer["ConfigurationManagersList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a manager listing, got {answer:?}"))
        .len()
}

/// One row: the case, the parameter map, and whether a definition may be written with
/// it.
type TargetCase = (&'static str, Value, bool);

/// The table: parameter maps spanning what the rule generalises over — WHICH of the two
/// target keys provides the value, how many entries it names and how that
/// comma-separated value is written, whether a key is present but provides no value,
/// which other parameters are named beside it, and the spelling of the key itself, since
/// a parameter key is matched as the model writes it. Every row that must be ACCEPTED
/// also names `TargetRegions`, which a definition must name whatever it targets, so a
/// row is refused only for the reason under test.
fn cases() -> Vec<TargetCase> {
    vec![
        ("no parameter at all", json!({}), false),
        (
            "parameters that name no target",
            json!({ "UpdateSSMAgent": "true", "TargetType": "*" }),
            false,
        ),
        (
            // `TargetRegions` names the Regions to deploy to, not an account or an
            // organizational unit, so it leaves the definition with nowhere to deploy.
            "the target Regions only",
            json!({ "TargetRegions": "eu-west-1,us-east-1" }),
            false,
        ),
        (
            "one targeted account",
            json!({ "TargetAccounts": "111122223333", "TargetRegions": "us-east-1" }),
            true,
        ),
        (
            "three targeted accounts",
            json!({
                "TargetAccounts": "111122223333,444455556666,777788889999",
                "TargetRegions": "us-east-1",
            }),
            true,
        ),
        (
            "one targeted organizational unit",
            json!({ "TargetOrganizationalUnits": "ou-1234-56789012", "TargetRegions": "us-east-1" }),
            true,
        ),
        (
            "two targeted organizational units with padding",
            json!({
                "TargetOrganizationalUnits": " ou-1234-56789012 , r-abcd ",
                "TargetRegions": "us-east-1",
            }),
            true,
        ),
        (
            "both target keys at once",
            json!({
                "TargetAccounts": "111122223333",
                "TargetOrganizationalUnits": "ou-1234-56789012",
                "TargetRegions": "us-east-1",
            }),
            true,
        ),
        (
            "a deployment target beside the Regions and other parameters",
            json!({
                "TargetOrganizationalUnits": "ou-1234-56789012",
                "TargetRegions": "eu-west-1",
                "UpdateSSMAgent": "true",
            }),
            true,
        ),
        (
            "the accounts written empty",
            json!({ "TargetAccounts": "" }),
            false,
        ),
        (
            "the organizational units written as blanks",
            json!({ "TargetOrganizationalUnits": "   " }),
            false,
        ),
        (
            "both target keys written as separators only",
            json!({ "TargetAccounts": ",", "TargetOrganizationalUnits": " , " }),
            false,
        ),
        (
            "one target key written empty while the other provides a value",
            json!({
                "TargetAccounts": "",
                "TargetOrganizationalUnits": "ou-1234-56789012",
                "TargetRegions": "us-east-1",
            }),
            true,
        ),
        (
            // Parameter keys are matched as the model writes them, so a differently
            // cased key is a different parameter and names no target.
            "a target key in lower case",
            json!({ "targetaccounts": "111122223333", "TargetRegions": "us-east-1" }),
            false,
        ),
    ]
}

/// A create stores a definition only when its parameters name somewhere to deploy to,
/// and refuses the rest with the modelled 400 having stored nothing.
#[tokio::test]
async fn a_create_stores_a_definition_only_when_its_parameters_name_a_deployment_target() {
    let router = emulator();
    let mut stored = 0_usize;

    for (case, parameters, acceptable) in cases() {
        let (status, answer) = create(&router, "targets", &parameters).await;
        if acceptable {
            stored += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateConfigurationManager must accept {parameters:?}, \
                 got {status} / {answer:?}"
            );
            let manager_arn = answer["ManagerArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"));
            let (_, written) = stored_definition(&router, case, manager_arn).await;
            assert_eq!(
                written, parameters,
                "case {case:?}: the stored definition must carry the parameters the create \
                 named, got {written:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: CreateConfigurationManager must refuse {parameters:?}, \
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
        }
        // A refused create stores nothing, so the manager count only moves on a create
        // that succeeded.
        let held = stored_managers(&router, case).await;
        assert_eq!(
            held, stored,
            "case {case:?}: expected {stored} stored managers, got {held}"
        );
    }
}

/// One row of the update table: the case, the entries the update writes, and whether the
/// map the definition then HOLDS still names somewhere to deploy to.
type UpdateCase = (&'static str, Value, bool);

/// The table for an update, every row written into a stored map that names an
/// organizational unit to deploy to and the Region to deploy into
/// ([`baseline_parameters`]). The rows differ along what decides the merged map's
/// targets — whether the update names a target key at all, which of the two it names,
/// how many entries it names, whether it writes a stored target key with no value (the
/// way a stored target is dropped), how that empty value is written, and the spelling of
/// the key, since a parameter key is matched as the model writes it. No row touches the
/// stored `TargetRegions`, which a definition must name whatever it targets, so a row is
/// refused only for the reason under test.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        (
            // The update names no target key at all, so the definition keeps the one it
            // was stored with.
            "a parameter that names no target",
            json!({ "UpdateSSMAgent": "true" }),
            true,
        ),
        (
            "one targeted account added beside the stored units",
            json!({ "TargetAccounts": "111122223333" }),
            true,
        ),
        (
            "three targeted accounts added beside the stored units",
            json!({ "TargetAccounts": "111122223333,444455556666,777788889999" }),
            true,
        ),
        (
            "the stored organizational units replaced by others",
            json!({ "TargetOrganizationalUnits": "ou-1234-56789012,r-abcd" }),
            true,
        ),
        (
            // Writing the only target key the definition holds with no value leaves it
            // naming nowhere to deploy to.
            "the stored organizational units written empty",
            json!({ "TargetOrganizationalUnits": "" }),
            false,
        ),
        (
            "the stored organizational units written as blanks",
            json!({ "TargetOrganizationalUnits": "   " }),
            false,
        ),
        (
            "the stored organizational units written as separators only",
            json!({ "TargetOrganizationalUnits": " , " }),
            false,
        ),
        (
            "the stored organizational units dropped while an account is named",
            json!({ "TargetOrganizationalUnits": "", "TargetAccounts": "111122223333" }),
            true,
        ),
        (
            "both target keys written empty",
            json!({ "TargetAccounts": "", "TargetOrganizationalUnits": " , " }),
            false,
        ),
        (
            // Parameter keys are matched as the model writes them, so a differently
            // cased key is a different parameter and names no target.
            "the stored organizational units dropped for a lower-cased account key",
            json!({ "TargetOrganizationalUnits": "", "targetaccounts": "111122223333" }),
            false,
        ),
    ]
}

/// An update writes the entries its `Parameters` names into the map the definition
/// holds, so it is accepted only when the map it leaves behind still names somewhere to
/// deploy to — and a refused one leaves the stored definition, targets and all, exactly
/// as it was.
#[tokio::test]
async fn an_update_writes_parameter_entries_only_when_the_definition_still_names_a_target() {
    for (case, entries, acceptable) in update_cases() {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let manager_arn = create_ok(&router, case, &baseline_parameters()).await;
        let (id, before) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            before,
            baseline_parameters(),
            "case {case:?}: the baseline create must store the baseline parameters, got {before:?}"
        );

        let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
        let (_, after) = stored_definition(&router, case, &manager_arn).await;
        if acceptable {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateConfigurationDefinition must accept {entries:?}, \
                 got {status} / {answer:?}"
            );
            let want = parameters_after(&entries);
            assert_eq!(
                after, want,
                "case {case:?}: the stored definition must hold {want:?}, got {after:?}"
            );
        } else {
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
            assert_eq!(
                after,
                baseline_parameters(),
                "case {case:?}: a refused update must leave the stored parameters alone, \
                 got {after:?}"
            );
        }
    }
}

/// An update that carries no `Parameters` member at all leaves the stored map as it is,
/// so it is not touched by the rule: the definition it writes still names the target it
/// was stored with.
#[tokio::test]
async fn an_update_that_names_no_parameters_keeps_the_stored_target() {
    let case = "no Parameters member";
    let router = emulator();
    let manager_arn = create_ok(&router, case, &baseline_parameters()).await;
    let (id, before) = stored_definition(&router, case, &manager_arn).await;

    let (status, answer) = call(
        &router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(&manager_arn),
            label(&id)
        ),
        Some(&json!({ "TypeVersion": "LATEST" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an update naming no Parameters must succeed, got {status} / {answer:?}"
    );

    let (_, after) = stored_definition(&router, case, &manager_arn).await;
    assert_eq!(
        after, before,
        "case {case:?}: the stored parameters must be left as they were, got {after:?}"
    );
}
