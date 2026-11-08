// SPDX-License-Identifier: CC0-1.0

//! AWS FIS target account configurations of an EXPERIMENT, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one
//! `GetExperimentTargetAccountConfiguration` states: an experiment's target account
//! configurations are the snapshot its template held when `StartExperiment` ran, so a
//! read named by the (`experimentId`, `accountId`) pair reports that snapshot's record
//! member for member (`accountId`, `roleArn` and `description` exactly as the create
//! carried them, down to whether `description` was absent or empty), it reports the same
//! record however often it is called and whatever the experiment's status is, and a pair
//! the snapshot does not hold — an unknown experiment, an account the experiment was not
//! started with, or an account configured on another template or added to this template
//! after the start — answers `ResourceNotFoundException` (HTTP 404). A request outside a
//! modelled constraint is refused with HTTP 400 before any of that.
//!
//! The HTTP harness below is local to this suite, as it is in the sibling suites: a test
//! harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`, from
/// `smithy_query(query="shape",shape="ExperimentTargetAccountConfiguration")`), spelled
/// out here rather than imported from the crate so a wrong bound in the model or the
/// handler fails this suite.
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// Comfortably past the span an experiment's `initiating` -> `running` transition falls
/// due in, so a read after it reports the settled status.
const PAST_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);

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

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    /// The `targetAccountConfiguration` this answer carries.
    fn configuration(&self) -> &Value {
        self.member("targetAccountConfiguration")
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

/// Create an experiment template to hang target account configurations off, and report
/// its minted id. `label` only has to be unique within one test; whitespace in it is
/// collapsed because `ClientToken` is modelled `pattern ^[\S]+$`.
///
/// The template targets MULTIPLE accounts, which is the targeting a target account
/// configuration belongs to: `accountTargeting` and the configurations a template holds
/// state the same thing between them. [`single_account_template`] is the one for a case
/// that hangs no configuration off its template.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    create_template(router, label, "multi-account").await
}

/// Create an experiment template that targets a SINGLE account — the documented default —
/// for a case whose experiment holds no target account configuration at all.
async fn single_account_template(router: &MultiServiceRouter, label: &str) -> String {
    create_template(router, label, "single-account").await
}

async fn create_template(router: &MultiServiceRouter, label: &str, targeting: &str) -> String {
    let token: String = label
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("template-{token}"),
            "description": format!("experiment target account configuration suite: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-experiment-target-accounts",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "experimentOptions": { "accountTargeting": targeting },
            // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
            // documented 1 to 720 minutes.
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            }
        })),
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

/// A create body carrying `roleArn`, plus `description` only when the case names one — an
/// absent member has to stay absent on the wire.
fn create_body(role_arn: &str, description: Option<&str>, client_token: &str) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
    }
    body.insert("clientToken".to_string(), json!(client_token));
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

/// `GetTargetAccountConfiguration`: the configuration the TEMPLATE holds.
async fn get_template_configuration(
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

async fn get_experiment(router: &MultiServiceRouter, experiment_id: &str) -> Answer {
    send(router, "GET", &format!("/experiments/{experiment_id}"), None).await
}

/// Start an experiment from `template_id` and report its minted id.
async fn start_experiment(
    router: &MultiServiceRouter,
    template_id: &str,
    token: &str,
) -> (String, Value) {
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
    let id = answer.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {token:?}: no experiment.id in {:?}", answer.body))
        .to_string();
    (id, answer.body)
}

/// An account id of exactly `len` characters: `TargetAccountId` is modelled
/// `@length(12..48)`, `pattern ^[\S]+$`, and AWS account ids are digit strings.
fn account_id_of(len: usize) -> String {
    let base = "000000000001".to_string();
    if len <= base.len() {
        base[..len].to_string()
    } else {
        format!("{base}{}", "9".repeat(len - base.len()))
    }
}

/// One row: what the create request carries for one account of the template the
/// experiment is started from.
struct Row {
    case: &'static str,
    account_id: String,
    role_arn: String,
    description: Option<String>,
}

/// The rows the snapshot rule is asserted over.
///
/// They differ along everything the rule generalises over: the digits of the
/// `accountId` naming the configuration, the
/// shape of the `roleArn` of that account (a plain role, a role with a path, a role in
/// another partition), and whether the `description` is absent, empty, plain text,
/// non-ASCII, or at `TargetAccountConfigurationDescription`'s published ceiling.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "account-id-at-the-floor-no-description",
            account_id: account_id_of(ACCOUNT_ID_FLOOR),
            role_arn: format!(
                "arn:aws:iam::{}:role/FisTargetAccount",
                account_id_of(ACCOUNT_ID_FLOOR)
            ),
            description: None,
        },
        Row {
            case: "empty-description",
            account_id: "210987654321".to_string(),
            role_arn: "arn:aws:iam::210987654321:role/service-role/fis-target".to_string(),
            description: Some(String::new()),
        },
        Row {
            case: "plain-description",
            account_id: "333333333333".to_string(),
            role_arn: "arn:aws:iam::333333333333:role/fis-staging".to_string(),
            description: Some("the staging account".to_string()),
        },
        Row {
            case: "non-ascii-description-other-partition-role",
            account_id: "444444444444".to_string(),
            role_arn: "arn:aws-cn:iam::444444444444:role/fis-目标账户".to_string(),
            description: Some("説明 — ünïcødé ✅ описание".to_string()),
        },
        Row {
            case: "description-at-the-ceiling",
            account_id: "555555555555".to_string(),
            role_arn: "arn:aws:iam::555555555555:role/fis-ceiling".to_string(),
            description: Some("d".repeat(DESCRIPTION_CEILING)),
        },
    ]
}

/// What the configuration for one row has to look like, whoever reports it.
fn assert_matches_row(case: &str, source: &str, row: &Row, configuration: &Value) {
    assert_eq!(
        configuration["accountId"],
        json!(row.account_id),
        "case {case:?}: expected {source} to name accountId {:?}, got {configuration:?}",
        row.account_id
    );
    assert_eq!(
        configuration["roleArn"],
        json!(row.role_arn),
        "case {case:?}: expected {source} to report the roleArn the create carried, \
         got {configuration:?}"
    );
    // A description with characters in it round-trips exactly, while an explicitly empty
    // one is normalized like an omitted one: the configuration then has no description at
    // all, which is the rule `target_account_configuration_description.rs` owns.
    match row
        .description
        .as_deref()
        .filter(|description| !description.is_empty())
    {
        Some(description) => assert_eq!(
            configuration["description"],
            json!(description),
            "case {case:?}: expected {source} to report the description the create carried \
             ({} characters), got {configuration:?}",
            description.chars().count()
        ),
        None => assert!(
            configuration.get("description").is_none_or(Value::is_null),
            "case {case:?}: expected {source} to report NO description for a create that \
             named none, got {configuration:?}"
        ),
    }
}

#[tokio::test]
async fn an_experiment_reports_the_target_account_configurations_it_was_started_with() {
    let router = emulator();
    let template_id = template(&router, "snapshot").await;

    let rows = rows();
    for row in &rows {
        let created = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                &format!("token-{}", row.case),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, created.status, created.body
        );
    }

    let (experiment_id, started) = start_experiment(&router, &template_id, "snapshot").await;
    assert_eq!(
        started["experiment"]["targetAccountConfigurationsCount"],
        json!(rows.len()),
        "case snapshot: expected the experiment to carry its template's count of {}, got {started:?}",
        rows.len()
    );

    for row in &rows {
        // The experiment reports the record the template was configured with.
        let fetched = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetExperimentTargetAccountConfiguration, \
             got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_matches_row(
            row.case,
            "the experiment read",
            row,
            fetched.configuration(),
        );

        // The same members the template's own read reports: the experiment snapshots the
        // configuration, it does not reshape it.
        let from_template =
            get_template_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            fetched.configuration(),
            from_template.configuration(),
            "case {:?}: the experiment read disagreed with the template read: {:?} vs {:?}",
            row.case,
            fetched.body,
            from_template.body
        );

        // Reading twice reports the same record: a read changes nothing.
        let again = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
        assert_eq!(
            again.configuration(),
            fetched.configuration(),
            "case {:?}: two reads of one experiment configuration disagreed: {:?}",
            row.case,
            again.body
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_same_snapshot_whatever_the_experiments_status_is() {
    let router = emulator();
    let template_id = template(&router, "across-statuses").await;
    let row = Row {
        case: "across-statuses",
        account_id: "666666666666".to_string(),
        role_arn: "arn:aws:iam::666666666666:role/fis-across-statuses".to_string(),
        description: Some("read at every status".to_string()),
    };
    let created = create_configuration(
        &router,
        &template_id,
        &row.account_id,
        create_body(
            &row.role_arn,
            row.description.as_deref(),
            "token-across-statuses",
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    let (experiment_id, _) = start_experiment(&router, &template_id, "across-statuses").await;

    // The rows here are the statuses the experiment passes through: a freshly started
    // experiment is still starting up, the clock then owes it `running`, and
    // `StopExperiment` moves it on again.
    let initiating = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
    assert_eq!(
        initiating.status, 200,
        "case initiating: expected HTTP 200, got {} with {:?}",
        initiating.status, initiating.body
    );
    assert_matches_row(row.case, "the read while initiating", &row, initiating.configuration());

    // Reading the configuration did not advance the experiment: the transient status is
    // still there for the caller that looks straight after the start.
    let polled = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        polled.member("experiment")["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "initiating",
        "case initiating: reading a target account configuration must not advance the \
         experiment, got {:?}",
        polled.body
    );

    tokio::time::sleep(PAST_TRANSITION).await;

    let running = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
    assert_eq!(
        running.configuration(),
        initiating.configuration(),
        "case running: the snapshot must not change when the experiment's status does, \
         got {:?}",
        running.body
    );

    let stopped = send(
        &router,
        "DELETE",
        &format!("/experiments/{experiment_id}"),
        None,
    )
    .await;
    assert_eq!(
        stopped.status, 200,
        "case stopping: expected HTTP 200 from StopExperiment, got {} with {:?}",
        stopped.status, stopped.body
    );
    let after_stop = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
    assert_eq!(
        after_stop.configuration(),
        initiating.configuration(),
        "case stopping: a stopped experiment still reports the snapshot it was started \
         with, got {:?}",
        after_stop.body
    );
}

#[tokio::test]
async fn a_configuration_added_after_the_start_is_not_part_of_the_experiment() {
    let router = emulator();
    let template_id = template(&router, "added-later").await;
    let at_start = "777777777777";
    let added_later = "888888888888";

    let created = create_configuration(
        &router,
        &template_id,
        at_start,
        create_body(
            "arn:aws:iam::777777777777:role/fis-at-start",
            Some("configured before the experiment started"),
            "token-at-start",
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 creating the pre-start configuration, got {} with {:?}",
        created.status, created.body
    );

    let (experiment_id, _) = start_experiment(&router, &template_id, "added-later").await;

    let created = create_configuration(
        &router,
        &template_id,
        added_later,
        create_body(
            "arn:aws:iam::888888888888:role/fis-added-later",
            None,
            "token-added-later",
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 creating the post-start configuration, got {} with {:?}",
        created.status, created.body
    );

    // The template holds both; the experiment holds only what it was started with.
    for (case, account_id, expected) in [
        ("configured-at-start", at_start, 200u16),
        ("added-after-the-start", added_later, 404u16),
    ] {
        let from_template = get_template_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            from_template.status, 200,
            "case {case:?}: expected the TEMPLATE to hold the configuration, got {} with {:?}",
            from_template.status, from_template.body
        );

        let from_experiment =
            get_experiment_configuration(&router, &experiment_id, account_id).await;
        assert_eq!(
            from_experiment.status, expected,
            "case {case:?}: expected HTTP {expected} from \
             GetExperimentTargetAccountConfiguration, got {} with {:?}",
            from_experiment.status, from_experiment.body
        );
        if expected == 404 {
            assert_eq!(
                from_experiment.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                from_experiment.body
            );
        }
    }

    // The count the experiment reports is the one it was started with, too.
    let experiment = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        experiment.member("experiment")["targetAccountConfigurationsCount"],
        json!(1),
        "case added-after-the-start: the experiment counts only the configurations it was \
         started with, got {:?}",
        experiment.body
    );
}

#[tokio::test]
async fn a_name_the_experiment_does_not_hold_answers_resource_not_found() {
    let router = emulator();
    let configured = template(&router, "configured").await;
    // This one holds no configuration at all, so it targets a single account.
    let other = single_account_template(&router, "other").await;
    let account_id = "999999999999";
    let created = create_configuration(
        &router,
        &configured,
        account_id,
        create_body(
            "arn:aws:iam::999999999999:role/fis-isolated",
            Some("configured on one template only"),
            "token-isolated",
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    let (experiment_id, _) = start_experiment(&router, &configured, "isolated").await;
    let (other_experiment_id, _) = start_experiment(&router, &other, "isolated-other").await;

    // An experiment id no record carries, within the modelled `^[\S]+$` and
    // `@length(0..64)`.
    let unknown_experiment = "EXP000000000099";

    // The rows differ along WHICH half of the name misses: the experiment, the account, or
    // both — and a configured account read on an experiment started from another template
    // is a miss too.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown-experiment", unknown_experiment, account_id),
        ("unknown-account", &experiment_id, "121212121212"),
        ("unknown-both", unknown_experiment, "121212121212"),
        (
            "configured-on-another-experiment",
            &other_experiment_id,
            account_id,
        ),
    ];
    for (case, experiment, account) in cases {
        let fetched = get_experiment_configuration(&router, experiment, account).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected HTTP 404 from \
             GetExperimentTargetAccountConfiguration, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );
    }

    // A refusal persisted nothing: the experiment still holds exactly its snapshot.
    let fetched = get_experiment_configuration(&router, &experiment_id, account_id).await;
    assert_eq!(
        fetched.status, 200,
        "case after-refusals: the configured pair still reads back, got {} with {:?}",
        fetched.status, fetched.body
    );
    let experiment = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        experiment.member("experiment")["targetAccountConfigurationsCount"],
        json!(1),
        "case after-refusals: a refused read must not change the experiment, got {:?}",
        experiment.body
    );
}

#[tokio::test]
async fn a_read_outside_a_modelled_bound_is_refused() {
    let router = emulator();
    let template_id = single_account_template(&router, "bounds").await;
    let (experiment_id, _) = start_experiment(&router, &template_id, "bounds").await;

    // `accountId` on each of its published bounds, and one past each. Inside the bounds
    // the experiment holds no configuration for the account, so the modelled 404 is the
    // right answer; outside them the request never reaches the handler.
    for (case, len, expected) in [
        ("accountId at the floor", ACCOUNT_ID_FLOOR, 404u16),
        ("accountId at the ceiling", ACCOUNT_ID_CEILING, 404u16),
        ("accountId below the floor", ACCOUNT_ID_FLOOR - 1, 400u16),
        ("accountId above the ceiling", ACCOUNT_ID_CEILING + 1, 400u16),
    ] {
        let account_id = account_id_of(len);
        assert_eq!(
            account_id.chars().count(),
            len,
            "case {case:?}: the test built an accountId of the wrong length: {account_id:?}"
        );
        let answer = get_experiment_configuration(&router, &experiment_id, &account_id).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
