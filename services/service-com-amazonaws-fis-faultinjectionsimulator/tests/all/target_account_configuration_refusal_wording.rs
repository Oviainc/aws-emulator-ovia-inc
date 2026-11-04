// SPDX-License-Identifier: CC0-1.0

//! RULES: how FIS WORDS the refusals of the target-account-configuration sub-resource of
//! an experiment template. The statuses and the error shapes are asserted by the sibling
//! suites; what this one pins is the message, which is part of the contract:
//!
//! * a call that names a sub-resource reports the resource THAT operation failed to
//!   resolve: the read and the update resolve the parent first, so a request whose
//!   `experimentTemplateId` resolves to no template is refused as the TEMPLATE missing and
//!   only a template that resolves and holds no configuration for the account is refused
//!   as the CONFIGURATION missing, while the delete reports the configuration for either
//!   miss;
//! * a create for a (template, account) pair the template already has a configuration for
//!   is refused `ConflictException` with a message naming the resource that already
//!   exists — the owning template first, then the target account;
//! * a create offered to a template that does not target multiple accounts is refused
//!   `ValidationException` with a message naming the template, the account that owns it
//!   and the `accountTargeting` that template currently carries.
//!
//! `ResourceNotFoundException$message`, `ConflictException$message` and
//! `ValidationException$message` are modelled only as `ExceptionMessage`
//! (`@length(0..1024)`,
//! `smithy_query(query="shape",shape="ExceptionMessage,TargetAccountId")`), so the text is
//! a server-side rule and this suite is where it is held.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. Each rule is asserted over a TABLE whose rows differ along what the rule
//! generalises over: which half of the resource's name misses, which operation carries the
//! call, which template and account the request names, the width of the `accountId` within
//! `TargetAccountId`'s published `@length(12..48)`, and which form the template's
//! `accountTargeting` takes. The HTTP harness below is local to this suite, as it is in
//! the sibling suites: a test harness is not shared state.
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// The message FIS answers for an experiment-template reference that resolves to no
/// record, whichever operation resolved it — including a call that went on to name one of
/// the template's sub-resources.
const TEMPLATE_NOT_FOUND: &str = "ExperimentTemplate not found.";

/// The message FIS answers for a target account configuration a template that RESOLVES
/// does not hold.
const CONFIGURATION_NOT_FOUND: &str = "TargetAccountConfiguration not found.";

/// `TargetAccountId`'s published floor (`@length(min: 12, max: 48)`,
/// `smithy_query(query="shape",shape="TargetAccountId")`), spelled out here rather than
/// imported from the crate so a wrong bound in the model or the handler fails this suite.
const TARGET_ACCOUNT_ID_FLOOR: usize = 12;

/// See [`TARGET_ACCOUNT_ID_FLOOR`] — the ceiling of the same `@length`.
const TARGET_ACCOUNT_ID_CEILING: usize = 48;

/// A SigV4 credential scope naming this service, which is what routes a request to fis.
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

/// A `CreateExperimentTemplate` body carrying `experiment_options` (omitted entirely when
/// the row names none), with everything else the operation requires.
fn template_body(label: &str, experiment_options: &Value) -> Value {
    let mut body = Map::new();
    body.insert("clientToken".to_string(), json!(format!("template-{label}")));
    body.insert(
        "description".to_string(),
        json!(format!("refusal wording suite: {label}")),
    );
    body.insert(
        "roleArn".to_string(),
        json!("arn:aws:iam::000000000000:role/fis-wording"),
    );
    body.insert("stopConditions".to_string(), json!([{ "source": "none" }]));
    // `targets` is required of the REQUEST (the service refuses a create without it) even
    // though the model leaves it optional; an empty map is admissible, and the actions this
    // template declares target nothing.
    body.insert("targets".to_string(), json!({}));
    // `aws:fis:wait` takes a required `duration` parameter (an ISO 8601 duration within
    // the documented 1 to 720 minutes), which the create refuses without.
    body.insert(
        "actions".to_string(),
        json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } } }),
    );
    if !experiment_options.is_null() {
        body.insert("experimentOptions".to_string(), experiment_options.clone());
    }
    Value::Object(body)
}

/// Create an experiment template carrying `experiment_options`, and report its minted id.
async fn template(router: &MultiServiceRouter, label: &str, experiment_options: &Value) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(template_body(label, experiment_options)),
    )
    .await;
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

/// A template a target account configuration belongs to: one that targets MULTIPLE
/// accounts.
async fn multi_account_template(router: &MultiServiceRouter, label: &str) -> String {
    template(
        router,
        label,
        &json!({ "accountTargeting": "multi-account" }),
    )
    .await
}

/// The path of one target account configuration of a template.
fn configuration_path(template_id: &str, account_id: &str) -> String {
    format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}")
}

/// A create body naming a role of the account it configures (the two members have to
/// agree about that, which is the rule `target_account_role_ownership.rs` asserts), plus
/// `clientToken` when the case names one.
fn create_body(account_id: &str, client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn(account_id)));
    if let Some(client_token) = client_token {
        body.insert("clientToken".to_string(), json!(client_token));
    }
    Value::Object(body)
}

/// A role of `account_id` wherever that value is an account NUMBER; for a wider
/// `TargetAccountId` no role can belong to it, so the ownership rule does not apply and
/// this emulator's own account carries the role.
fn role_arn(account_id: &str) -> String {
    if account_id.len() == TARGET_ACCOUNT_ID_FLOOR && account_id.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("arn:aws:iam::{account_id}:role/fis-wording-target");
    }
    format!(
        "arn:aws:iam::{}:role/fis-wording-target",
        engine::emulator_account_id()
    )
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
        &configuration_path(template_id, account_id),
        Some(body),
    )
    .await
}

/// The three operations that name an EXISTING target account configuration: the HTTP
/// method and body its path takes, and whether that operation reports the PARENT template
/// as the missing resource when the template does not resolve.
fn sub_resource_calls() -> [(&'static str, &'static str, Option<Value>, bool); 3] {
    [
        ("GetTargetAccountConfiguration", "GET", None, true),
        (
            "UpdateTargetAccountConfiguration",
            "PATCH",
            Some(json!({ "description": "an update the refusal discards" })),
            true,
        ),
        (
            "DeleteTargetAccountConfiguration",
            "DELETE",
            None,
            false,
        ),
    ]
}

#[tokio::test]
async fn a_sub_resource_miss_names_the_resource_its_operation_failed_to_resolve() {
    let router = emulator();
    let configured = multi_account_template(&router, "wording-configured").await;
    let other = multi_account_template(&router, "wording-other").await;
    let configured_account = "606060606060";
    let created = create_configuration(
        &router,
        &configured,
        configured_account,
        create_body(configured_account, Some("wording-create-token")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    // A template that was created and then DELETED: its id resolves to no record either,
    // so a sub-resource call on it is judged exactly as one naming an id never minted.
    let deleted = multi_account_template(&router, "wording-deleted").await;
    let removed = send(
        &router,
        "DELETE",
        &format!("/experimentTemplates/{deleted}"),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case setup: expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
        removed.status, removed.body
    );

    // Template ids no record carries, each within the modelled `^[\S]+$` and
    // `@length(0..64)`: a minted-looking id nothing minted, a short one, one at the
    // member's ceiling, and the id of a template that has been deleted.
    let unminted = "EXT000000000099".to_string();
    let short = "x".to_string();
    let at_ceiling = "T".repeat(64);
    // An `accountId` at `TargetAccountId`'s published ceiling, which the boundary admits
    // and no template is configured for.
    let wide_account = format!("6363636363{}", "6".repeat(TARGET_ACCOUNT_ID_CEILING - 10));
    // The rows differ along which half of the name misses, the shape of the template id,
    // and the account named — which neither message varies with. The last field says
    // whether the TEMPLATE resolves, which is what decides the message of an operation
    // that resolves the parent first.
    let cases: [(&str, &str, &str, bool); 7] = [
        ("unminted-template", &unminted, configured_account, false),
        ("short-template-id", &short, "616161616161", false),
        (
            "template-id-at-the-ceiling",
            &at_ceiling,
            configured_account,
            false,
        ),
        ("deleted-template", &deleted, configured_account, false),
        (
            "template-resolves-account-does-not",
            &configured,
            "626262626262",
            true,
        ),
        (
            "template-resolves-account-is-wide",
            &configured,
            &wide_account,
            true,
        ),
        (
            "configured-on-another-template",
            &other,
            configured_account,
            true,
        ),
    ];

    for (case, template_id, account_id, template_resolves) in cases {
        for (operation, method, body, resolves_parent_first) in sub_resource_calls() {
            let expected = if resolves_parent_first && !template_resolves {
                TEMPLATE_NOT_FOUND
            } else {
                CONFIGURATION_NOT_FOUND
            };
            let answer = send(
                &router,
                method,
                &configuration_path(template_id, account_id),
                body,
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {operation}/{case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation}/{case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {operation}/{case:?}: expected message {expected:?}, got {:?}",
                answer.message()
            );
        }
    }

    // The configuration that DOES exist is untouched by every refusal above, so what they
    // refused was the name and not the operation.
    let fetched = send(
        &router,
        "GET",
        &configuration_path(&configured, configured_account),
        None,
    )
    .await;
    assert_eq!(
        fetched.status, 200,
        "case live-configuration: expected HTTP 200 from GetTargetAccountConfiguration, \
         got {} with {:?}",
        fetched.status, fetched.body
    );
}

#[tokio::test]
async fn a_duplicate_configuration_conflict_names_the_template_and_the_account() {
    let router = emulator();
    let first_template = multi_account_template(&router, "conflict-first").await;
    let second_template = multi_account_template(&router, "conflict-second").await;

    // The rows differ along the template the pair names, the width of the account id
    // within `TargetAccountId`'s published `@length(12..48)` (its floor, a value between
    // the bounds, and its ceiling), and whether the second create carries a client token
    // the store has never seen — all of which the conflict message either interpolates or
    // must not vary with.
    let wide_account = format!("5454545454{}", "5".repeat(20 - 10));
    let ceiling_account = format!("5555555555{}", "5".repeat(TARGET_ACCOUNT_ID_CEILING - 10));
    let cases: [(&str, &str, &str, Option<&str>); 4] = [
        (
            "floor-width-account",
            &first_template,
            "505050505050",
            None,
        ),
        (
            "account-between-the-bounds",
            &first_template,
            &wide_account,
            Some("conflict-fresh-token"),
        ),
        (
            "account-id-at-the-ceiling",
            &second_template,
            &ceiling_account,
            None,
        ),
        (
            "another-template-same-account-width",
            &second_template,
            "515151515151",
            Some("conflict-another-token"),
        ),
    ];

    for (case, template_id, account_id, token) in cases {
        let created = create_configuration(
            &router,
            template_id,
            account_id,
            create_body(account_id, Some(&format!("created-{case}"))),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with {:?}",
            created.status, created.body
        );

        let second =
            create_configuration(&router, template_id, account_id, create_body(account_id, token))
                .await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected HTTP 409 for a second create of one account, got {} \
             with {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            second.body
        );
        let expected = format!(
            "TargetAccountConfiguration for {template_id} and target account {account_id} \
             already exists."
        );
        assert_eq!(
            second.message(),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            second.message()
        );
    }
}

#[tokio::test]
async fn a_single_account_template_refusal_names_the_template_the_account_and_the_targeting() {
    let router = emulator();

    // The rows differ along the FORM the template's `accountTargeting` takes: no
    // experiment options at all (so the documented default applies), the default spelled
    // out, and the default beside another option — every shape of template the rule
    // refuses.
    let rows: [(&str, Value, &str); 3] = [
        ("no-experiment-options", json!(null), "404040404040"),
        (
            "default-targeting-spelled-out",
            json!({ "accountTargeting": "single-account" }),
            "414141414141",
        ),
        (
            "default-targeting-beside-another-option",
            json!({ "accountTargeting": "single-account", "emptyTargetResolutionMode": "skip" }),
            "424242424242",
        ),
    ];

    for (case, options, account_id) in rows {
        let template_id = template(&router, case, &options).await;
        let answer = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(account_id, Some(&format!("single-{case}"))),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for a configuration on a template that does \
             not target multiple accounts, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );

        // The message's variable parts come from the request and the template's own
        // state: the template id the path named, the account that owns the template (the
        // account segment of its ARN) and the `accountTargeting` a read of the template
        // reports.
        let fetched = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{template_id}"),
            None,
        )
        .await;
        let stored = fetched.member("experimentTemplate").clone();
        let targeting = stored["experimentOptions"]["accountTargeting"]
            .as_str()
            .unwrap_or_default();
        let owner = stored["arn"]
            .as_str()
            .unwrap_or_default()
            .split(':')
            .nth(4)
            .unwrap_or_default();
        assert_eq!(
            owner,
            engine::emulator_account_id(),
            "case {case:?}: expected the template to be owned by this emulator's account, \
             got {:?}",
            stored["arn"]
        );
        let expected = format!(
            "ExperimentTemplate {template_id} in AWS account {owner} is not configured as \
             multi account. Account targeting value is: {targeting}."
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            answer.message()
        );

        // Nothing was persisted: the name the refused create named holds nothing.
        let miss = send(
            &router,
            "GET",
            &configuration_path(&template_id, account_id),
            None,
        )
        .await;
        assert_eq!(
            miss.status, 404,
            "case {case:?}: a refused create must persist nothing, got {} with {:?}",
            miss.status, miss.body
        );
        assert_eq!(
            miss.message(),
            CONFIGURATION_NOT_FOUND,
            "case {case:?}: expected message {CONFIGURATION_NOT_FOUND:?}, got {:?}",
            miss.message()
        );
    }

    // The control: a template that DOES target multiple accounts takes the same create,
    // and the configuration reads back — so what the rows above refused is the targeting.
    let accepted_template = multi_account_template(&router, "wording-accepted").await;
    let account_id = "434343434343";
    let created = create_configuration(
        &router,
        &accepted_template,
        account_id,
        create_body(account_id, Some("single-control-token")),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case multi-account-control: expected HTTP 200 from \
         CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );
    let fetched = send(
        &router,
        "GET",
        &configuration_path(&accepted_template, account_id),
        None,
    )
    .await;
    assert_eq!(
        fetched.member("targetAccountConfiguration"),
        created.member("targetAccountConfiguration"),
        "case multi-account-control: expected the read to report the created \
         configuration, got {:?}",
        fetched.body
    );
}
