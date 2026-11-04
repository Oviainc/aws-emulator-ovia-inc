// SPDX-License-Identifier: CC0-1.0

//! What AWS FIS REFUSES around target account configurations, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. Four rules are under test, each asserted over a table that spans the
//! class of inputs the rule generalises over rather than one recorded request:
//!
//! * a member that names an IAM role (`RoleArn`: `CreateExperimentTemplate.roleArn`,
//!   `CreateTargetAccountConfiguration.roleArn`,
//!   `UpdateTargetAccountConfiguration.roleArn`) is accepted only when its value is a
//!   well-formed IAM role ARN — `arn:<partition>:iam::<12-digit account>:role/<path and
//!   name>` — and a value inside `RoleArn`'s modelled `@length(20..2048)`/`^[\S]+$` that
//!   is not one is refused with HTTP 400 `ValidationException` while the stored
//!   configuration keeps the `roleArn` it already held. The role of a target account
//!   configuration also has to BELONG to the account it configures, which is the rule
//!   `target_account_role_ownership.rs` asserts; every row here carries a role of its own
//!   account, so what it refuses can only be the ARN form;
//! * a target account configuration a template that RESOLVES does not hold is refused 404
//!   `ResourceNotFoundException` with the service's constant message, which names neither
//!   the template nor the account, while a call whose TEMPLATE does not resolve is refused
//!   with the template-level message instead: the parent is resolved before the child;
//! * an operation addressing a configuration of an EXPERIMENT resolves the PARENT first
//!   the same way: an `experimentId` that names no record is refused 404
//!   `ResourceNotFoundException` reporting the EXPERIMENT as missing, with a constant that
//!   echoes no name, and only an experiment that resolves reports its missing CHILD
//!   configuration — that one with a message BUILT FROM THE REQUEST: the experiment id
//!   first, then the target account id;
//! * a `nextToken` a paginated list never issued is refused 400 `ValidationException`
//!   with one constant message, whatever the token and whichever list it is offered to.
//!
//! The HTTP harness below is local to this suite, as it is in the sibling suites: a test
//! harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `RoleArn`'s published bounds (`@length(min: 20, max: 2048)`,
/// `smithy_query(query="shape",shape="RoleArn")`), spelled out here rather than imported
/// from the crate so a wrong bound in the model or the handler fails this suite.
const ROLE_ARN_CEILING: usize = 2048;

/// The message FIS answers for a target account configuration an experiment template that
/// RESOLVES does not hold — a constant, so neither half of the resource's name appears
/// in it.
const TEMPLATE_SCOPED_NOT_FOUND: &str = "TargetAccountConfiguration not found.";

/// The message FIS answers when the experiment template a sub-resource call names does not
/// resolve at all: the parent is resolved before the child, so the refusal names the
/// TEMPLATE and not the configuration.
const PARENT_TEMPLATE_NOT_FOUND: &str = "ExperimentTemplate not found.";

/// The message FIS answers for a target account configuration an EXPERIMENT that resolves
/// does not hold: built from the request, naming the experiment and then the account.
fn experiment_scoped_not_found(experiment_id: &str, account: &str) -> String {
    format!(
        "TargetAccountConfiguration for experiment {experiment_id} \
         and target account {account} not found"
    )
}

/// The message FIS answers for an `experimentId` that names no experiment, on an operation
/// addressing a child of one — a constant naming the resource KIND that did not resolve.
const EXPERIMENT_NOT_FOUND: &str = "Experiment not found.";

/// The message FIS answers for a `nextToken` no call of the list issued — a constant,
/// whatever the token and whichever list it was offered to.
const INVALID_NEXT_TOKEN: &str = "The provided nextToken is invalid.";

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message the answer carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    /// The `targetAccountConfiguration` this answer carries.
    fn configuration(&self) -> &Value {
        self.member("targetAccountConfiguration")
    }

    /// The `roleArn` of the `targetAccountConfiguration` this answer carries.
    fn role_arn(&self) -> &str {
        self.configuration()["roleArn"].as_str().unwrap_or_default()
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(FALLBACK_REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A `CreateExperimentTemplate` body carrying `role_arn`, with everything else the
/// operation requires, and the `accountTargeting` a target account configuration belongs
/// to: `accountTargeting` and the configurations a template holds state the same thing
/// between them, so a configuration is only created on a template that targets MULTIPLE
/// accounts.
fn template_body(label: &str, role_arn: &str) -> Value {
    template_body_targeting(label, role_arn, "multi-account")
}

fn template_body_targeting(label: &str, role_arn: &str, targeting: &str) -> Value {
    let token: String = label
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    json!({
        "clientToken": format!("template-{token}"),
        "description": format!("target account configuration refusal suite: {label}"),
        "roleArn": role_arn,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "experimentOptions": { "accountTargeting": targeting },
        // `aws:fis:wait` takes a required `duration` parameter (an ISO 8601 duration
        // within the documented 1 to 720 minutes), which the create refuses without.
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } } }
    })
}

/// Create an experiment template to hang target account configurations off, and report
/// its minted id.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    created_template(
        router,
        label,
        template_body(label, "arn:aws:iam::000000000000:role/fis-refusals"),
    )
    .await
}

/// Create an experiment template that targets a SINGLE account — the documented default —
/// for a case whose experiment holds no target account configuration.
async fn single_account_template(router: &MultiServiceRouter, label: &str) -> String {
    created_template(
        router,
        label,
        template_body_targeting(
            label,
            "arn:aws:iam::000000000000:role/fis-refusals",
            "single-account",
        ),
    )
    .await
}

async fn created_template(router: &MultiServiceRouter, label: &str, body: Value) -> String {
    let answer = send(router, "POST", "/experimentTemplates", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

/// Start an experiment from `template_id` and report its minted id.
async fn start_experiment(router: &MultiServiceRouter, template_id: &str, token: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": format!("start-{token}"),
            "experimentTemplateId": template_id
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {token:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {token:?}: no experiment.id in {:?}", answer.body))
        .to_string()
}

/// A create body carrying `roleArn`, plus `clientToken` when the case names one.
fn create_body(role_arn: &str, client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(client_token) = client_token {
        body.insert("clientToken".to_string(), json!(client_token));
    }
    Value::Object(body)
}

async fn create_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    body: Value,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(body),
    )
    .await
}

async fn get_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

async fn update_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    role_arn: &str,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(json!({ "roleArn": role_arn })),
    )
    .await
}

/// `GetExperimentTargetAccountConfiguration`: the configuration the EXPERIMENT holds.
async fn get_experiment_configuration(
    router: &MultiServiceRouter,
    experiment_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experiments/{experiment_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

/// The `roleArn` a configuration this suite updates is CREATED with: a role of the account
/// it configures (the two members have to agree about that), so what a refused update
/// failed to overwrite is visible.
fn baseline_role_arn(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/fis-refusal-baseline")
}

/// The values that are NOT well-formed IAM role ARNs, each inside `RoleArn`'s modelled
/// `@length(20..2048)` and `pattern ^[\S]+$` so the refusal can only come from the ARN
/// rule.
///
/// The rows differ along WHICH part of the ARN form is violated: no ARN at all, a
/// non-numeric or wrong-width account id, a region on a global service's ARN, another
/// service, another IAM resource type, a missing or empty role-name segment, an empty
/// partition, a colon inside the resource, a truncated ARN, and a wrong-case prefix.
fn malformed_role_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "not-an-arn-at-all",
            "this-is-not-a-valid-iam-role-arn-000000".to_string(),
        ),
        (
            "account-id-not-numeric",
            "arn:aws:iam::not-an-account-id:role/fis-target".to_string(),
        ),
        (
            "account-id-eleven-digits",
            "arn:aws:iam::12345678901:role/fis-target".to_string(),
        ),
        (
            "account-id-thirteen-digits",
            "arn:aws:iam::1234567890123:role/fis-target".to_string(),
        ),
        (
            "region-on-a-global-service",
            "arn:aws:iam:us-east-1:123456789012:role/fis-target".to_string(),
        ),
        (
            "another-service",
            "arn:aws:sts::123456789012:role/fis-assumed".to_string(),
        ),
        (
            "another-iam-resource-type",
            "arn:aws:iam::123456789012:user/fis-not-a-role".to_string(),
        ),
        (
            "no-role-name",
            "arn:aws:iam::123456789012:role/".to_string(),
        ),
        (
            "empty-path-segment",
            "arn:aws:iam::123456789012:role//fis-target".to_string(),
        ),
        (
            "empty-partition",
            "arn::iam::123456789012:role/fis-target".to_string(),
        ),
        (
            "colon-inside-the-resource",
            "arn:aws:iam::123456789012:role/fis:target".to_string(),
        ),
        (
            "truncated-before-the-resource",
            "arn:aws:iam::123456789012".to_string(),
        ),
        (
            "upper-case-prefix",
            "ARN:AWS:IAM::123456789012:role/FisTarget".to_string(),
        ),
    ]
}

/// The values that ARE well-formed IAM role ARNs, each with the account it is configured
/// FOR: the rows differ along partition, the role path, the characters of the role name,
/// and the overall length — the shortest form the syntax admits and `RoleArn`'s published
/// ceiling.
fn well_formed_role_arns() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "plain-role",
            "123456789012",
            "arn:aws:iam::123456789012:role/FisTargetAccount".to_string(),
        ),
        (
            "service-role-path",
            "210987654321",
            "arn:aws:iam::210987654321:role/service-role/fis-target".to_string(),
        ),
        (
            "nested-path",
            "310987654321",
            "arn:aws-us-gov:iam::310987654321:role/path/to/fis-target".to_string(),
        ),
        (
            "china-partition-non-ascii-name",
            "444444444444",
            "arn:aws-cn:iam::444444444444:role/fis-目标账户".to_string(),
        ),
        (
            "shortest-well-formed",
            "000000000000",
            "arn:aws:iam::000000000000:role/r".to_string(),
        ),
        ("role-name-at-the-length-ceiling", "555555555555", {
            let prefix = "arn:aws:iam::555555555555:role/";
            format!("{prefix}{}", "r".repeat(ROLE_ARN_CEILING - prefix.len()))
        }),
    ]
}

#[tokio::test]
async fn a_role_arn_that_is_not_an_iam_role_arn_is_refused_and_never_persists() {
    let router = emulator();
    let template_id = template(&router, "role-arn-rule").await;

    // The CREATE path: a malformed `roleArn` is refused and nothing is stored under the
    // name it tried to configure.
    for (index, (case, role_arn)) in malformed_role_arns().into_iter().enumerate() {
        let account_id = format!("3000000000{index:02}");
        let answer = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(&role_arn, Some(&format!("token-create-{index}"))),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({role_arn:?}): expected HTTP 400 from \
             CreateTargetAccountConfiguration, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({role_arn:?}): expected ValidationException, got {:?}",
            answer.body
        );

        let fetched = get_configuration(&router, &template_id, &account_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must persist nothing, got {} with {:?}",
            fetched.status, fetched.body
        );
    }

    // The UPDATE path: the same rule, and the stored `roleArn` survives every refusal.
    for (index, (case, role_arn)) in malformed_role_arns().into_iter().enumerate() {
        let account_id = format!("4000000000{index:02}");
        let created = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(
                &baseline_role_arn(&account_id),
                Some(&format!("token-update-{index}")),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
            created.status, created.body
        );

        let answer = update_configuration(&router, &template_id, &account_id, &role_arn).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({role_arn:?}): expected HTTP 400 from \
             UpdateTargetAccountConfiguration, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({role_arn:?}): expected ValidationException, got {:?}",
            answer.body
        );

        let fetched = get_configuration(&router, &template_id, &account_id).await;
        assert_eq!(
            fetched.role_arn(),
            baseline_role_arn(&account_id),
            "case {case:?}: a refused update must leave the stored roleArn alone, got {:?}",
            fetched.body
        );
    }

    // The same rule on `CreateExperimentTemplate.roleArn`, the service's other member
    // naming an IAM role.
    for (case, role_arn) in malformed_role_arns() {
        let answer = send(
            &router,
            "POST",
            "/experimentTemplates",
            Some(template_body(&format!("bad-{case}"), &role_arn)),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case template/{case:?} ({role_arn:?}): expected HTTP 400 from \
             CreateExperimentTemplate, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case template/{case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_well_formed_role_arn_is_accepted_by_the_create_and_the_update() {
    let router = emulator();
    let template_id = template(&router, "well-formed-role-arns").await;

    for (case, account_id, role_arn) in well_formed_role_arns() {
        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(&role_arn, Some(&format!("token-ok-{account_id}"))),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({} characters): expected HTTP 200 from \
             CreateTargetAccountConfiguration, got {} with {:?}",
            role_arn.chars().count(),
            created.status,
            created.body
        );
        assert_eq!(
            created.role_arn(),
            role_arn,
            "case {case:?}: expected the create to report the roleArn it carried, got {:?}",
            created.body
        );

        // The round trip: the read reports what the create persisted.
        let fetched = get_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            fetched.role_arn(),
            role_arn,
            "case {case:?}: expected the read to report the created roleArn, got {:?}",
            fetched.body
        );

        // And an UPDATE onto another well-formed value replaces it, so the rule admits
        // the same class on both paths. Another role of the same account: the role an
        // update names belongs to the account it configures too.
        let replacement = format!("arn:aws:iam::{account_id}:role/service-role/fis-replacement");
        let updated = update_configuration(&router, &template_id, account_id, &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateTargetAccountConfiguration, got {} \
             with {:?}",
            updated.status, updated.body
        );
        let fetched = get_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            fetched.role_arn(),
            replacement,
            "case {case:?}: expected the read to report the updated roleArn, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_template_scoped_configuration_miss_answers_the_services_constant_message() {
    let router = emulator();
    let configured = template(&router, "constant-message-configured").await;
    let other = template(&router, "constant-message-other").await;
    let account_id = "606060606060";
    let created = create_configuration(
        &router,
        &configured,
        account_id,
        create_body(&baseline_role_arn(account_id), Some("token-constant-message")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    // An experiment-template id no record carries, within the modelled `^[\S]+$` and
    // `@length(0..64)`.
    let unknown_template = "EXT000000000099";

    // The rows differ along WHICH half of the name misses — the template, the account, or
    // both — and along the template and account the request named, which neither message
    // varies with. A row whose TEMPLATE does not resolve is refused as the template
    // missing (the parent is resolved first); a row whose template resolves and holds no
    // configuration for the account is refused as the configuration missing.
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "unknown-template",
            unknown_template,
            account_id,
            PARENT_TEMPLATE_NOT_FOUND,
        ),
        (
            "unknown-account",
            &configured,
            "616161616161",
            TEMPLATE_SCOPED_NOT_FOUND,
        ),
        (
            "another-unknown-account",
            &configured,
            "626262626262",
            TEMPLATE_SCOPED_NOT_FOUND,
        ),
        (
            "unknown-both",
            unknown_template,
            "616161616161",
            PARENT_TEMPLATE_NOT_FOUND,
        ),
        (
            "configured-on-another-template",
            &other,
            account_id,
            TEMPLATE_SCOPED_NOT_FOUND,
        ),
    ];
    for (case, template_id, account, expected) in cases {
        let fetched = get_configuration(&router, template_id, account).await;
        assert_eq!(
            fetched.status, 404,
            "case get/{case:?}: expected HTTP 404, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case get/{case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.message(),
            expected,
            "case get/{case:?}: expected message {expected:?}, got {:?}",
            fetched.message()
        );

        let updated =
            update_configuration(&router, template_id, account, &baseline_role_arn(account)).await;
        assert_eq!(
            updated.status, 404,
            "case update/{case:?}: expected HTTP 404, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_code(),
            "ResourceNotFoundException",
            "case update/{case:?}: expected ResourceNotFoundException, got {:?}",
            updated.body
        );
        assert_eq!(
            updated.message(),
            expected,
            "case update/{case:?}: expected message {expected:?}, got {:?}",
            updated.message()
        );
    }
}

#[tokio::test]
async fn an_experiment_scoped_configuration_miss_reports_the_parent_before_the_child() {
    let router = emulator();
    let template_id = template(&router, "experiment-scoped-message").await;
    let configured_account = "707070707070";
    let created = create_configuration(
        &router,
        &template_id,
        configured_account,
        create_body(
            &baseline_role_arn(configured_account),
            Some("token-experiment-message"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );
    let first = start_experiment(&router, &template_id, "message-first").await;
    let second = start_experiment(&router, &template_id, "message-second").await;

    // An experiment id no record carries, within the modelled `^[\S]+$` and
    // `@length(0..64)`.
    let unknown_experiment = "EXP000000000099";

    // The rows differ along which of the two names does not resolve — the PARENT experiment
    // or the CHILD configuration — and, within each, along the experiment named (two
    // distinct started experiments and ids of two shapes the store holds nothing for) and
    // the account named (several, at both of `TargetAccountId`'s published bounds). The
    // parent is resolved first, so a row whose experiment does not resolve is reported as
    // the experiment missing, whatever account it names, and never as a configuration
    // missing from an experiment that does not exist.
    let long_account = format!("7171717171{}", "7".repeat(38));
    let cases: [(&str, &str, &str, bool); 6] = [
        ("first-experiment-unknown-account", &first, "717171717171", true),
        ("second-experiment-unknown-account", &second, "727272727272", true),
        (
            "second-experiment-account-id-at-the-ceiling",
            &second,
            &long_account,
            true,
        ),
        (
            "unknown-experiment-configured-account",
            unknown_experiment,
            configured_account,
            false,
        ),
        (
            "unknown-experiment-unknown-account",
            unknown_experiment,
            "737373737373",
            false,
        ),
        (
            "an-id-of-the-template-collections-shape",
            "EXT000000000001",
            configured_account,
            false,
        ),
    ];
    for (case, experiment_id, account, experiment_resolves) in cases {
        let answer = get_experiment_configuration(&router, experiment_id, account).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from GetExperimentTargetAccountConfiguration, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        let expected = if experiment_resolves {
            experiment_scoped_not_found(experiment_id, account)
        } else {
            EXPERIMENT_NOT_FOUND.to_string()
        };
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            answer.message()
        );
        if !experiment_resolves {
            assert!(
                !answer.message().contains(experiment_id) && !answer.message().contains(account),
                "case {case:?}: a parent miss must echo neither name, got {:?}",
                answer.message()
            );
        }
    }

    // The configuration the experiment DOES hold is still readable, so the refusal above
    // is about the name and not about the operation.
    let found = get_experiment_configuration(&router, &first, configured_account).await;
    assert_eq!(
        found.status, 200,
        "case configured-account: expected HTTP 200 from \
         GetExperimentTargetAccountConfiguration, got {} with {:?}",
        found.status, found.body
    );
}

#[tokio::test]
async fn a_next_token_no_list_issued_answers_the_services_constant_message() {
    let router = emulator();
    let template_id = single_account_template(&router, "next-token-rule").await;
    let configurations = format!("/experimentTemplates/{template_id}/targetAccountConfigurations");
    let experiment_id = start_experiment(&router, &template_id, "next-token-rule").await;
    let experiment_configurations =
        format!("/experiments/{experiment_id}/targetAccountConfigurations");
    let resolved_targets = format!("/experiments/{experiment_id}/resolvedTargets");

    // The rows differ along the list the token is offered to and along the token's shape:
    // a plain word, a well-formed id of ANOTHER collection, a page token of another list,
    // and a page token whose payload is outside `TargetAccountId`'s bounds.
    let cases: [(&str, &str, &str); 13] = [
        ("ListExperimentTemplates", "/experimentTemplates", "nonsense"),
        (
            "ListExperimentTemplates",
            "/experimentTemplates",
            "EXP000000000001",
        ),
        (
            "ListExperimentTemplates",
            "/experimentTemplates",
            "targetAccountConfiguration.123456789012",
        ),
        ("ListExperiments", "/experiments", "not-a-real-token"),
        ("ListExperiments", "/experiments", "EXT000000000001"),
        ("ListExperiments", "/experiments", "EXP00000000000"),
        (
            "ListTargetAccountConfigurations",
            &configurations,
            "not-a-real-token",
        ),
        (
            "ListTargetAccountConfigurations",
            &configurations,
            "EXT000000000001",
        ),
        (
            "ListTargetAccountConfigurations",
            &configurations,
            "targetAccountConfiguration.short",
        ),
        (
            "ListExperimentTargetAccountConfigurations",
            &experiment_configurations,
            "nonsense",
        ),
        (
            "ListExperimentTargetAccountConfigurations",
            &experiment_configurations,
            "targetAccountConfiguration.",
        ),
        (
            "ListExperimentResolvedTargets",
            &resolved_targets,
            "nonsense",
        ),
        (
            "ListExperimentResolvedTargets",
            &resolved_targets,
            "targetAccountConfiguration.123456789012",
        ),
    ];
    for (operation, path, token) in cases {
        let answer = send(&router, "GET", &format!("{path}?nextToken={token}"), None).await;
        assert_eq!(
            answer.status, 400,
            "case {operation}/{token:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {operation}/{token:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            INVALID_NEXT_TOKEN,
            "case {operation}/{token:?}: expected message {INVALID_NEXT_TOKEN:?}, got {:?}",
            answer.message()
        );
    }
}
