// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the role a LOCAL configuration deployment is administered
//! with, and the shared fixture the other suites take that role from.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. IAM is mounted beside this service on ONE
//! engine, the shape `core/router/src/lib.rs` mounts them in, because the account's roles
//! are IAM's records and not this service's.
//!
//! The rule under test: a configuration definition that deploys LOCALLY — whose
//! `Parameters` name `TargetAccounts`, documented as the ID of the account initiating the
//! configuration deployment, which a request provides only if it wants to deploy the
//! configuration locally (`smithy_query(query="doc",
//! shape="ConfigurationDefinitionInput", member="Parameters")`) — is served only when it
//! names a `LocalDeploymentAdministrationRoleArn`, "The ARN of the IAM role used to
//! administrate local configuration deployments" (`smithy_query(query="shape",
//! shape="ConfigurationDefinitionInput")`), AND the account's own IAM records show that
//! role letting the service that deploys a configuration assume it. Each half is its own
//! refusal, both 400 `ValidationException` with nothing stored:
//!
//! * a local deployment naming no such role at all is refused for the missing member;
//! * one naming a role the account's IAM does not show trusting
//!   `cloudformation.amazonaws.com` for `sts:AssumeRole` is refused for the trust policy.
//!
//! A definition deployed through an organization — `TargetOrganizationalUnits` and no
//! `TargetAccounts` — is held to neither half: the role is what a LOCAL deployment is
//! administered with.
//!
//! The rows span what the rule generalises over: which half of it a definition breaks,
//! how the trust document withholds the assume (it names another principal, it denies
//! this one explicitly, it is no policy at all, the role carries no document, the account
//! holds no such role), how many definitions one create carries and which of them is the
//! offending one, how the target list is written (one account, several, the key present
//! but empty), and which operation writes the definition — the create and the update
//! that turns an organization deployment into a local one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::local_administration_role::{
    trust_policy_message, DEPLOYMENT_SERVICE_PRINCIPAL,
};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The configuration type every definition below deploys: no case here is about the type,
/// so each uses the catalog's first entry and completes its documented parameters through
/// the shared fixture.
fn published_type() -> &'static str {
    QUICK_SETUP_TYPE_CATALOG[0].type_
}

/// An account id a definition may target that is not the one this emulator serves, so a
/// local deployment is not confused with the fan-out default.
const TARGET_ACCOUNT: &str = "111122223333";

/// The name of the administration role every suite's local-deployment fixture names.
pub(crate) const ADMINISTRATION_ROLE_NAME: &str = "QuickSetupStackSetLocalAdministration";

/// The ARN IAM holds [`ADMINISTRATION_ROLE_NAME`] under, built the way IAM builds one, so
/// the fixture and the records it is resolved against cannot disagree about it.
pub(crate) fn administration_role_arn() -> String {
    iam::state::role_arn("/", ADMINISTRATION_ROLE_NAME)
}

/// A trust document that lets `principal` assume the role, as a client writes one.
pub(crate) fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// A trust document that DENIES `principal` the assume, which IAM's own order makes final
/// whatever any `Allow` beside it says.
fn denying_trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": { "Service": principal },
                "Action": "sts:AssumeRole",
            },
            {
                "Effect": "Deny",
                "Principal": { "Service": principal },
                "Action": "sts:AssumeRole",
            },
        ],
    })
    .to_string()
}

/// This service and IAM on ONE engine, with the account already holding
/// [`ADMINISTRATION_ROLE_NAME`] trusting the service that deploys a configuration — the
/// fixture every suite whose definitions deploy locally builds its emulator from.
///
/// The role is written into the account's IAM records directly because it is a record the
/// account ALREADY holds in those suites, like a role an administrator created before
/// Quick Setup was used; the cases in THIS suite, which are about the rule rather than
/// about some other behaviour, create their roles through IAM's own `CreateRole` so what
/// is resolved is what that handler wrote.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_role(
        &engine,
        ADMINISTRATION_ROLE_NAME,
        Some(&trust_policy(DEPLOYMENT_SERVICE_PRINCIPAL)),
    );
    mounted(engine)
}

/// This service and IAM on ONE engine, with no role in the account at all.
fn bare_emulator() -> MultiServiceRouter {
    mounted(Arc::new(engine::InMemoryEngine::new()))
}

/// Mount both services on `engine`, exactly as the router mounts them.
fn mounted(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(iam::EmulatedService::new(engine));
    router
}

/// Write one role into the account's IAM records, with `trust` as its trust document (or
/// none at all).
pub(crate) fn seed_role(engine: &Arc<engine::InMemoryEngine>, name: &str, trust: Option<&str>) {
    let state = iam::state::service_state(&**engine).expect("the engine carries an IAM store");
    let mut store = state.lock();
    store.roles.insert(
        iam::state::name_key(name),
        iam::ops::types::Role {
            path: "/".to_string(),
            role_name: name.to_string(),
            role_id: format!("AROAFIXTURE{}", name.len()),
            arn: iam::state::role_arn("/", name),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: trust.map(str::to_string),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// A request signed for `ssm-quicksetup` in [`REGION`].
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
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a refusal carried.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment.
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

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One IAM call: IAM speaks `awsQuery`, so its parameters travel as a form body and its
/// answer comes back as XML.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/iam/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Create one IAM role named `name` with `trust` as its trust document, through IAM's own
/// `CreateRole`, and answer the ARN IAM persisted for it.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    trust: &str,
) -> String {
    let (status, created) = iam_call(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let open = "<Arn>";
    let close = "</Arn>";
    let start = created
        .find(open)
        .map(|at| at + open.len())
        .unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    let end = created[start..]
        .find(close)
        .map(|at| at + start)
        .unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    created[start..end].to_string()
}

/// One configuration definition: the published type, the parameters the case names
/// completed with every one that type requires, and the administration role the case
/// names (or none).
fn definition(parameters: Value, administration_role_arn: Option<&str>) -> Value {
    let type_ = published_type();
    let mut definition = json!({
        "Type": type_,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(type_, parameters),
    });
    if let Some(arn) = administration_role_arn {
        definition["LocalDeploymentAdministrationRoleArn"] = json!(arn);
    }
    definition
}

/// `POST /configurationManager` with the definitions a case names.
async fn create(router: &MultiServiceRouter, definitions: Vec<Value>) -> (u16, Value) {
    let body = json!({
        "Name": "local-deployment-role",
        "ConfigurationDefinitions": definitions,
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// How many managers this endpoint holds, so a refused create can be shown to have
/// stored nothing.
async fn stored_managers(router: &MultiServiceRouter) -> usize {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer["ConfigurationManagersList"]
        .as_array()
        .map_or(0, Vec::len)
}

/// A parameter map naming `accounts` as the local deployment's targets, plus the Region
/// every type's table requires.
fn local_parameters(accounts: &str) -> Value {
    json!({ "TargetAccounts": accounts, "TargetRegions": "us-east-1" })
}

/// A parameter map deploying through an organization instead.
fn organization_parameters() -> Value {
    json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" })
}

/// What a row expects of a create.
enum Expected {
    /// Served, and the manager it minted is readable.
    Served,
    /// Refused with this `Message`, and nothing stored.
    Refused(String),
}

/// The rule's first half: a local deployment that names no administration role is refused
/// for the missing member, whatever else the definition says — and an organization
/// deployment is not held to it.
#[tokio::test]
async fn a_local_deployment_must_name_an_administration_role() {
    let router = emulator();
    let missing =
        "The required parameter LocalDeploymentAdministrationRoleArn is missing.".to_string();
    let administration = administration_role_arn();

    // (case, the definitions the create carries, what it earns)
    let rows: Vec<(&str, Vec<Value>, Expected)> = vec![
        (
            "one account, no administration role",
            vec![definition(local_parameters(TARGET_ACCOUNT), None)],
            Expected::Refused(missing.clone()),
        ),
        (
            "three accounts, no administration role",
            vec![definition(
                local_parameters(&format!("{TARGET_ACCOUNT},444455556666,777788889999")),
                None,
            )],
            Expected::Refused(missing.clone()),
        ),
        (
            "the account list written with blanks around it",
            vec![definition(
                local_parameters(&format!("  {TARGET_ACCOUNT}  ")),
                None,
            )],
            Expected::Refused(missing.clone()),
        ),
        (
            // Both target keys: a definition that names accounts deploys locally even
            // where it names organizational units too.
            "accounts beside organizational units, no administration role",
            vec![definition(
                json!({
                    "TargetAccounts": TARGET_ACCOUNT,
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
                None,
            )],
            Expected::Refused(missing.clone()),
        ),
        (
            // The second definition of a create is the offending one: the rule is about
            // each definition, not about the first.
            "a second definition deploying locally with no administration role",
            vec![
                definition(organization_parameters(), None),
                definition(local_parameters(TARGET_ACCOUNT), None),
            ],
            Expected::Refused(missing.clone()),
        ),
        (
            "the administration role named",
            vec![definition(
                local_parameters(TARGET_ACCOUNT),
                Some(&administration),
            )],
            Expected::Served,
        ),
        (
            "every definition of a create naming it",
            vec![
                definition(local_parameters(TARGET_ACCOUNT), Some(&administration)),
                definition(local_parameters("444455556666"), Some(&administration)),
            ],
            Expected::Served,
        ),
        (
            // The rule's edge: the role is what a LOCAL deployment is administered with.
            "an organization deployment with no administration role",
            vec![definition(organization_parameters(), None)],
            Expected::Served,
        ),
        (
            // A key written with no entries names no account, so this is no local
            // deployment either — it is refused for naming no target at all.
            "the account key present but empty",
            vec![definition(
                json!({
                    "TargetAccounts": "",
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
                None,
            )],
            Expected::Served,
        ),
    ];

    for (case, definitions, expected) in rows {
        let before = stored_managers(&router).await;
        let (status, answer) = create(&router, definitions).await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the create must be served, got {status} / {answer:?}"
                );
                let arn = answer["ManagerArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
                    .to_string();
                let (status, read_back) = call(
                    &router,
                    "GET",
                    &format!("/configurationManager/{}", label(&arn)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created manager must be readable, got {status} / \
                     {read_back:?}"
                );
                assert_eq!(
                    stored_managers(&router).await,
                    before + 1,
                    "case {case:?}: a served create stores one manager"
                );
            }
            Expected::Refused(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the create must be refused, got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {answer:?}"
                );
                let actual = error_message(&answer);
                assert_eq!(
                    actual, message,
                    "case {case:?}: expected {message:?}, got {actual:?}"
                );
                assert_eq!(
                    stored_managers(&router).await,
                    before,
                    "case {case:?}: a refused create stores nothing"
                );
            }
        }
    }
}

/// The rule's second half: a local deployment is served only where the account's IAM
/// records show its administration role letting the deploying service assume it.
#[tokio::test]
async fn a_local_deployments_administration_role_must_admit_the_deployer() {
    // The engine is held here because two rows need a trust document IAM's own
    // `CreateRole` refuses to write (text that is no JSON at all), so those records are
    // written into the account's IAM store directly; every other role below is created
    // through `CreateRole`, so what this service resolves is what that handler wrote.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_role(&engine, "TrustDocumentIsNoPolicy", Some("not a policy at all"));
    let router = mounted(Arc::clone(&engine));
    let refused = trust_policy_message();

    // The roles the rows name, each created through IAM's own `CreateRole`.
    let trusting = given_role(
        &router,
        "a role trusting the deployer",
        "DeployerTrusted",
        &trust_policy(DEPLOYMENT_SERVICE_PRINCIPAL),
    )
    .await;
    let trusting_several = given_role(
        &router,
        "a role trusting the deployer among others",
        "DeployerTrustedAmongOthers",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": ["ssm.amazonaws.com", DEPLOYMENT_SERVICE_PRINCIPAL] },
                "Action": ["sts:AssumeRole", "sts:TagSession"],
            }],
        })
        .to_string(),
    )
    .await;
    let other_principal = given_role(
        &router,
        "a role trusting another principal",
        "OtherPrincipalTrusted",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;
    let denied = given_role(
        &router,
        "a role denying the deployer",
        "DeployerDenied",
        &denying_trust_policy(DEPLOYMENT_SERVICE_PRINCIPAL),
    )
    .await;
    let not_a_policy = iam::state::role_arn("/", "TrustDocumentIsNoPolicy");

    // (case, the administration role the definition names, what it earns)
    let rows: Vec<(&str, String, Expected)> = vec![
        (
            "a role whose trust policy admits the deployer",
            trusting,
            Expected::Served,
        ),
        (
            "a role admitting the deployer among several principals and actions",
            trusting_several,
            Expected::Served,
        ),
        (
            "a role trusting another service principal",
            other_principal,
            Expected::Refused(refused.clone()),
        ),
        (
            "a role whose trust policy denies the deployer explicitly",
            denied,
            Expected::Refused(refused.clone()),
        ),
        (
            "a role whose trust document is not a policy at all",
            not_a_policy,
            Expected::Refused(refused.clone()),
        ),
        (
            "an ARN the account holds no role for",
            iam::state::role_arn("/", "NeverCreated"),
            Expected::Refused(refused.clone()),
        ),
        (
            "a role of another account",
            "arn:aws:iam::111122223333:role/Elsewhere".to_string(),
            Expected::Refused(refused.clone()),
        ),
    ];

    for (case, role_arn, expected) in rows {
        let before = stored_managers(&router).await;
        let (status, answer) = create(
            &router,
            vec![definition(local_parameters(TARGET_ACCOUNT), Some(&role_arn))],
        )
        .await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the create must be served, got {status} / {answer:?}"
                );
                let arn = answer["ManagerArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
                    .to_string();
                let (status, read_back) = call(
                    &router,
                    "GET",
                    &format!("/configurationManager/{}", label(&arn)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created manager must be readable, got {status} / \
                     {read_back:?}"
                );
                // The role the create named is what the manager reports holding.
                assert_eq!(
                    read_back["ConfigurationDefinitions"][0]
                        ["LocalDeploymentAdministrationRoleArn"]
                        .as_str(),
                    Some(role_arn.as_str()),
                    "case {case:?}: the stored definition must carry the role the create \
                     named, got {read_back:?}"
                );
            }
            Expected::Refused(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the create must be refused, got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {answer:?}"
                );
                let actual = error_message(&answer);
                assert_eq!(
                    actual, message,
                    "case {case:?}: expected {message:?}, got {actual:?}"
                );
                assert_eq!(
                    stored_managers(&router).await,
                    before,
                    "case {case:?}: a refused create stores nothing"
                );
            }
        }
    }
}

/// A role the account holds with NO trust document at all shows no grant either: a record
/// `CreateRole` cannot write, so it is written into the account's records directly.
#[tokio::test]
async fn a_role_carrying_no_trust_document_admits_no_deployer() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_role(&engine, "NoTrustDocument", None);
    let router = mounted(engine);

    let role_arn = iam::state::role_arn("/", "NoTrustDocument");
    let (status, answer) = create(
        &router,
        vec![definition(local_parameters(TARGET_ACCOUNT), Some(&role_arn))],
    )
    .await;
    assert_eq!(
        status, 400,
        "a role with no trust document must be refused, got {status} / {answer:?}"
    );
    let actual = error_message(&answer);
    assert_eq!(
        actual,
        trust_policy_message(),
        "expected {:?}, got {actual:?}",
        trust_policy_message()
    );
    assert_eq!(
        stored_managers(&router).await,
        0,
        "a refused create stores nothing"
    );
}

/// The same rule on the other operation that writes a definition: an update that turns an
/// organization deployment into a local one is held to both halves, and a definition that
/// passes them is read back carrying what the update stored.
#[tokio::test]
async fn an_update_that_makes_a_deployment_local_is_held_to_the_same_rule() {
    let router = bare_emulator();
    let administration = given_role(
        &router,
        "the administration role",
        ADMINISTRATION_ROLE_NAME,
        &trust_policy(DEPLOYMENT_SERVICE_PRINCIPAL),
    )
    .await;
    let untrusting = given_role(
        &router,
        "a role trusting another principal",
        "NotTheDeployer",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;

    // One manager deploying through an organization, so every update below is what makes
    // its definition local.
    let (status, answer) = create(&router, vec![definition(organization_parameters(), None)]).await;
    assert_eq!(
        status, 200,
        "the create must be served, got {status} / {answer:?}"
    );
    let manager_arn = answer["ManagerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ManagerArn, got {answer:?}"))
        .to_string();
    let (status, manager) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&manager_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "the manager must be readable, got {manager:?}");
    let definition_id = manager["ConfigurationDefinitions"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a definition Id, got {manager:?}"))
        .to_string();
    let uri = format!(
        "/configurationDefinition/{}/{}",
        label(&manager_arn),
        label(&definition_id)
    );

    // (case, the update body, what it earns)
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "adding target accounts and no administration role",
            json!({ "Parameters": { "TargetAccounts": TARGET_ACCOUNT } }),
            Expected::Refused(
                "The required parameter LocalDeploymentAdministrationRoleArn is missing."
                    .to_string(),
            ),
        ),
        (
            "adding target accounts and a role the deployer cannot assume",
            json!({
                "Parameters": { "TargetAccounts": TARGET_ACCOUNT },
                "LocalDeploymentAdministrationRoleArn": untrusting,
            }),
            Expected::Refused(trust_policy_message()),
        ),
        (
            "naming a role the deployer cannot assume while staying an organization \
             deployment",
            json!({ "LocalDeploymentAdministrationRoleArn": untrusting }),
            Expected::Served,
        ),
        (
            "adding target accounts and the administration role together",
            json!({
                "Parameters": { "TargetAccounts": TARGET_ACCOUNT },
                "LocalDeploymentAdministrationRoleArn": administration,
            }),
            Expected::Served,
        ),
    ];

    for (case, update, expected) in rows {
        let (status, answer) = call(&router, "PUT", &uri, Some(&update)).await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the update must be served, got {status} / {answer:?}"
                );
            }
            Expected::Refused(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the update must be refused, got {status} / {answer:?}"
                );
                let actual = error_message(&answer);
                assert_eq!(
                    actual, message,
                    "case {case:?}: expected {message:?}, got {actual:?}"
                );
            }
        }
    }

    // The definition the accepted updates left behind is what a read reports: the local
    // target they added and the administration role they stored.
    let (status, manager) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&manager_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "the manager must be readable, got {manager:?}");
    let stored = &manager["ConfigurationDefinitions"][0];
    assert_eq!(
        stored["Parameters"]["TargetAccounts"].as_str(),
        Some(TARGET_ACCOUNT),
        "the accepted update must store the local target, got {stored:?}"
    );
    assert_eq!(
        stored["LocalDeploymentAdministrationRoleArn"].as_str(),
        Some(administration.as_str()),
        "the accepted update must store the administration role, got {stored:?}"
    );
}
