// SPDX-License-Identifier: CC0-1.0

//! RULE: an experiment template's `experimentOptions.accountTargeting` and the target
//! account configurations it holds state ONE thing between them — which accounts its
//! experiments run in — so the two have to agree. A target account configuration is
//! required when `accountTargeting` of `experimentOptions` is set to `multi-account`
//! (`smithy_query(query="doc",shape="CreateTargetAccountConfiguration")`), so:
//!
//! * `StartExperiment` on a template that targets multiple accounts and holds NO target
//!   account configuration is refused with `ValidationException` (HTTP 400) and mints no
//!   experiment; and
//! * a target account configuration offered to a template that targets a SINGLE account is
//!   refused the same way and persists nothing.
//!
//! The relationship lives on two different resources, so no modelled constraint can state
//! it (`accountTargeting` is modelled `multi-account | single-account` and the
//! configurations are a sub-resource of the template): it is a server-side rule, applied
//! before anything is minted or written.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. The rows differ
//! along what the rule generalises over: which targeting the template carries (absent — so
//! the documented default applies — `single-account`, `multi-account`), how many
//! configurations it holds (none, one, several), whether the configurations were removed
//! again after being created, what the start itself asks for (`actionsMode`), and which
//! operation the rule is carried by.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// The `accountTargeting` FIS applies when a template names none — `AccountTargeting` is
/// modelled `multi-account | single-account` and the documented default is the caller's
/// own account.
const DEFAULT_TARGETING: &str = "single-account";

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

/// Create an experiment template carrying `options` as its `experimentOptions` (nothing
/// when `options` is `null`), and report its minted id.
async fn template(router: &MultiServiceRouter, label: &str, options: &Value) -> String {
    let mut body = json!({
        "clientToken": format!("targeting-template-{label}"),
        "description": format!("account targeting agreement: {label}"),
        "roleArn": "arn:aws:iam::000000000000:role/fis-account-targeting",
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
        // documented 1 to 720 minutes.
        "actions": {
            "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
        }
    });
    if !options.is_null() {
        body["experimentOptions"] = options.clone();
    }
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

async fn delete_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        None,
    )
    .await
}

async fn list_configurations(router: &MultiServiceRouter, template_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations"),
        None,
    )
    .await
}

async fn get_template(router: &MultiServiceRouter, template_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}"),
        None,
    )
    .await
}

async fn get_experiment(router: &MultiServiceRouter, experiment_id: &str) -> Answer {
    send(router, "GET", &format!("/experiments/{experiment_id}"), None).await
}

/// The experiments the store holds for `template_id`, which is how a refused start is
/// shown to have minted nothing.
async fn experiments_of(router: &MultiServiceRouter, template_id: &str) -> usize {
    let answer = send(
        router,
        "GET",
        &format!("/experiments?experimentTemplateId={template_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from ListExperiments, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiments")
        .as_array()
        .map_or(0, Vec::len)
}

/// The account ids a row configures: distinct, twelve-digit account numbers.
fn accounts(index: usize, count: usize) -> Vec<String> {
    let row = u64::try_from(index).expect("the row index fits a u64");
    (0..count)
        .map(|offset| {
            let offset = u64::try_from(offset).expect("the account offset fits a u64");
            format!("{:012}", 111_111_000_000_u64 + row * 10 + offset)
        })
        .collect()
}

/// One row of the start rule: the template's experiment options, how many accounts it is
/// configured for, whether those configurations are deleted again before the start, and
/// what the start itself asks for.
struct StartRow {
    case: &'static str,
    options: Value,
    configured: usize,
    /// Every configuration created above is deleted again before the start, so the rule is
    /// shown to be about what the template holds AT THE START and not about what was ever
    /// created on it.
    deleted_again: bool,
    /// The `experimentOptions` the start request carries, if any.
    start_options: Value,
    /// Whether FIS runs this template.
    runnable: bool,
}

fn start_rows() -> Vec<StartRow> {
    vec![
        StartRow {
            case: "no experiment options at all",
            options: json!(null),
            configured: 0,
            deleted_again: false,
            start_options: json!(null),
            runnable: true,
        },
        StartRow {
            case: "the default targeting spelled out, no configurations",
            options: json!({ "accountTargeting": DEFAULT_TARGETING }),
            configured: 0,
            deleted_again: false,
            start_options: json!(null),
            runnable: true,
        },
        StartRow {
            case: "single-account beside another option, no configurations",
            options: json!({
                "accountTargeting": DEFAULT_TARGETING,
                "emptyTargetResolutionMode": "skip"
            }),
            configured: 0,
            deleted_again: false,
            start_options: json!(null),
            runnable: true,
        },
        StartRow {
            case: "multi-account with no configuration",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 0,
            deleted_again: false,
            start_options: json!(null),
            runnable: false,
        },
        StartRow {
            case: "multi-account with no configuration, carrying out no action",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 0,
            deleted_again: false,
            // `actionsMode: skip-all` carries out none of the template's actions, and the
            // rule is about the accounts the run is for, so it refuses this start too.
            start_options: json!({ "actionsMode": "skip-all" }),
            runnable: false,
        },
        StartRow {
            case: "multi-account with one configuration",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 1,
            deleted_again: false,
            start_options: json!(null),
            runnable: true,
        },
        StartRow {
            case: "multi-account with several configurations",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 3,
            deleted_again: false,
            start_options: json!(null),
            runnable: true,
        },
        StartRow {
            case: "multi-account whose only configuration was deleted again",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 1,
            deleted_again: true,
            start_options: json!(null),
            runnable: false,
        },
        StartRow {
            case: "multi-account whose several configurations were deleted again",
            options: json!({ "accountTargeting": "multi-account" }),
            configured: 3,
            deleted_again: true,
            start_options: json!(null),
            runnable: false,
        },
    ]
}

#[tokio::test]
async fn a_multi_account_template_runs_only_once_it_names_a_target_account() {
    let router = emulator();

    // Every refusal states the same relationship, so the rows are compared against each
    // other rather than against a recorded string.
    let mut refusals: Vec<(&str, String)> = Vec::new();

    for (index, row) in start_rows().into_iter().enumerate() {
        let template_id = template(&router, &format!("start-{index}"), &row.options).await;
        let configured = accounts(index, row.configured);
        for account in &configured {
            let created = create_configuration(
                &router,
                &template_id,
                account,
                json!({
                    "roleArn": format!("arn:aws:iam::{account}:role/fis-target"),
                    "clientToken": format!("targeting-config-{index}-{account}")
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected HTTP 200 creating the configuration for {account}, \
                 got {} with {:?}",
                row.case, created.status, created.body
            );
        }
        if row.deleted_again {
            for account in &configured {
                let deleted = delete_configuration(&router, &template_id, account).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {:?}: expected HTTP 200 deleting the configuration for {account}, \
                     got {} with {:?}",
                    row.case, deleted.status, deleted.body
                );
            }
        }
        let expected_count = if row.deleted_again { 0 } else { row.configured };

        let mut start = Map::new();
        start.insert(
            "clientToken".to_string(),
            json!(format!("targeting-start-{index}")),
        );
        start.insert("experimentTemplateId".to_string(), json!(template_id));
        if !row.start_options.is_null() {
            start.insert("experimentOptions".to_string(), row.start_options.clone());
        }
        let answer = send(&router, "POST", "/experiments", Some(Value::Object(start))).await;

        if row.runnable {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            let experiment_id = answer.member("experiment")["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: no experiment.id in {:?}", row.case, answer.body)
                })
                .to_string();
            // The round trip: the experiment the start minted carries the accounts the
            // template named, and a read of it reports the same record.
            let fetched = get_experiment(&router, &experiment_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {:?}: expected HTTP 200 from GetExperiment, got {} with {:?}",
                row.case, fetched.status, fetched.body
            );
            assert_eq!(
                fetched.member("experiment")["targetAccountConfigurationsCount"],
                json!(expected_count),
                "case {:?}: expected the experiment to carry {expected_count} target \
                 account configurations, got {:?}",
                row.case,
                fetched.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 for a multi-account template naming no target \
             account, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            answer.body
        );
        let message = answer.message().to_string();
        assert!(
            message.contains("accountTargeting") && message.contains("multi-account"),
            "case {:?}: expected the refusal to name the option and the targeting it \
             states, got {message:?}",
            row.case
        );
        // Nothing was minted: no experiment of that template exists.
        assert_eq!(
            experiments_of(&router, &template_id).await,
            0,
            "case {:?}: a refused start must mint no experiment",
            row.case
        );
        refusals.push((row.case, message));
    }

    let (first_case, first_message) = refusals
        .first()
        .cloned()
        .expect("the table carries rows a multi-account template cannot be run from");
    for (case, message) in &refusals {
        assert_eq!(
            message, &first_message,
            "case {case:?}: expected the same refusal as {first_case:?} \
             ({first_message:?}), got {message:?}"
        );
    }
}

/// One row of the create rule: the template's experiment options, and the configuration
/// offered to it.
struct CreateRow {
    case: &'static str,
    options: Value,
    account_id: &'static str,
    /// The role FIS would assume in that account. It belongs to the account the row
    /// configures wherever that `accountId` is an account NUMBER, so the ownership rule
    /// `target_account_role_ownership.rs` owns never decides a row here.
    role_arn: &'static str,
    description: Option<&'static str>,
    client_token: Option<&'static str>,
    /// Whether the template is one a target account configuration belongs to.
    accepted: bool,
}

fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            case: "multi-account, a plain configuration",
            options: json!({ "accountTargeting": "multi-account" }),
            account_id: "222222222222",
            role_arn: "arn:aws:iam::222222222222:role/fis-target",
            description: None,
            client_token: None,
            accepted: true,
        },
        CreateRow {
            case: "multi-account, a configuration carrying a description and a token",
            options: json!({
                "accountTargeting": "multi-account",
                "emptyTargetResolutionMode": "skip"
            }),
            account_id: "333333333333",
            role_arn: "arn:aws:iam::333333333333:role/service-role/nested/fis",
            description: Some("the account this experiment also runs in"),
            client_token: Some("targeting-create-token"),
            accepted: true,
        },
        CreateRow {
            case: "no experiment options at all",
            options: json!(null),
            account_id: "444444444444",
            role_arn: "arn:aws:iam::444444444444:role/fis-target",
            description: None,
            client_token: None,
            accepted: false,
        },
        CreateRow {
            case: "the default targeting spelled out",
            options: json!({ "accountTargeting": DEFAULT_TARGETING }),
            account_id: "555555555555",
            role_arn: "arn:aws-cn:iam::555555555555:role/fis-target",
            description: Some("a description the refusal discards"),
            client_token: Some("targeting-refused-token"),
            accepted: false,
        },
        CreateRow {
            case: "single-account beside another option",
            options: json!({
                "accountTargeting": DEFAULT_TARGETING,
                "emptyTargetResolutionMode": "skip"
            }),
            // `TargetAccountId` is modelled `@length(12..48)`, so an account id longer
            // than an account number is admitted at the boundary and this rule decides it
            // just the same (no role can BELONG to such a value, so the ownership rule is
            // not applied to it).
            account_id: "6666666666666666",
            role_arn: "arn:aws:iam::666666666666:role/fis-target",
            description: None,
            client_token: None,
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_target_account_configuration_attaches_only_to_a_multi_account_template() {
    let router = emulator();

    let mut refusals: Vec<(&str, String)> = Vec::new();

    for (index, row) in create_rows().into_iter().enumerate() {
        let template_id = template(&router, &format!("create-{index}"), &row.options).await;
        let mut body = Map::new();
        body.insert("roleArn".to_string(), json!(row.role_arn));
        if let Some(description) = row.description {
            body.insert("description".to_string(), json!(description));
        }
        if let Some(client_token) = row.client_token {
            body.insert("clientToken".to_string(), json!(client_token));
        }
        let answer = create_configuration(
            &router,
            &template_id,
            row.account_id,
            Value::Object(body),
        )
        .await;

        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, \
                 got {} with {:?}",
                row.case, answer.status, answer.body
            );
            // The round trip: the read reports what the create persisted.
            let fetched = get_configuration(&router, &template_id, row.account_id).await;
            assert_eq!(
                fetched.member("targetAccountConfiguration"),
                answer.member("targetAccountConfiguration"),
                "case {:?}: expected the read to report the created configuration, got {:?}",
                row.case,
                fetched.body
            );
            let template = get_template(&router, &template_id).await;
            assert_eq!(
                template.member("experimentTemplate")["targetAccountConfigurationsCount"],
                json!(1),
                "case {:?}: expected the template to count its one configuration, got {:?}",
                row.case,
                template.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 for a configuration on a single-account \
             template, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            answer.body
        );
        let message = answer.message().to_string();
        assert!(
            message.contains("accountTargeting") && message.contains("multi-account"),
            "case {:?}: expected the refusal to name the option and the targeting it \
             requires, got {message:?}",
            row.case
        );
        // Nothing was persisted: neither the read of that name nor the template's own
        // count knows about it.
        let fetched = get_configuration(&router, &template_id, row.account_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {:?}: a refused create must persist nothing, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        let template = get_template(&router, &template_id).await;
        assert_eq!(
            template.member("experimentTemplate")["targetAccountConfigurationsCount"],
            json!(0),
            "case {:?}: a refused create must leave the template counting none, got {:?}",
            row.case,
            template.body
        );
        let listed = list_configurations(&router, &template_id).await;
        assert_eq!(
            listed.member("targetAccountConfigurations"),
            &json!([]),
            "case {:?}: a refused create must leave the list empty, got {:?}",
            row.case,
            listed.body
        );
        refusals.push((row.case, message));
    }

    let (first_case, first_message) = refusals
        .first()
        .cloned()
        .expect("the table carries rows a configuration cannot be created on");
    for (case, message) in &refusals {
        assert_eq!(
            message, &first_message,
            "case {case:?}: expected the same refusal as {first_case:?} \
             ({first_message:?}), got {message:?}"
        );
    }
}
