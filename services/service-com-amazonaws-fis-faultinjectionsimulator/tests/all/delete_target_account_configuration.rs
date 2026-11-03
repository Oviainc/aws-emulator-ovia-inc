// SPDX-License-Identifier: CC0-1.0

//! `DeleteTargetAccountConfiguration`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one FIS's
//! `DeleteTargetAccountConfiguration` surface states:
//!
//! * a target account configuration is a SUB-RESOURCE of an experiment template, named by
//!   the (`experimentTemplateId`, `accountId`) pair, and the delete removes exactly the
//!   one that pair names, answering HTTP 200 with the record it removed, member for
//!   member as the store held it;
//! * after the delete that name holds nothing: a read, an update or a second delete of it
//!   answers `ResourceNotFoundException` (HTTP 404), the owning template's
//!   `targetAccountConfigurationsCount` is one lower, and every OTHER configuration of
//!   that template is left exactly as it was;
//! * the creating `@idempotencyToken` goes with the record, so a create replaying that
//!   token after the delete creates afresh rather than reporting the removed record;
//! * an already-started experiment holds the snapshot its template had at start, so
//!   deleting the template's configuration changes nothing an experiment reports;
//! * a delete naming a template the store does not hold, or an account that template has
//!   no configuration for (including an account configured on a DIFFERENT template),
//!   answers `ResourceNotFoundException` (HTTP 404) and removes nothing;
//! * a request outside a modelled constraint on `accountId` or `experimentTemplateId` is
//!   refused with HTTP 400 before any of that, and removes nothing.
//!
//! The HTTP harness below is local to this suite: the ones in the sibling suites are
//! private to those suites, and a test harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`), spelled out here
/// rather than imported from the crate so a wrong bound in the model or the handler fails
/// this suite.
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `RoleArn`'s published ceiling (`@length(min: 20, max: 2048)`).
const ROLE_ARN_CEILING: usize = 2048;

/// The length of the SHORTEST well-formed IAM role ARN:
/// `arn:aws:iam::<12 digits>:role/<one character>` — `"arn:aws:iam::"` (13) plus the
/// twelve-digit account id, `":role/"` (6) and a one-character role name. It is above
/// `RoleArn`'s modelled floor of 20, which is why a configuration is created with this
/// length rather than with that floor: a `roleArn` at the floor is inside the shape's
/// bounds and still not an ARN, so the service refuses it (see
/// `target_account_configuration_errors.rs`).
const SHORTEST_ROLE_ARN: usize = 32;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// `ExperimentTemplateId`'s published ceiling (`@length(min: 0, max: 64)`).
const TEMPLATE_ID_CEILING: usize = 64;

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
async fn template(router: &MultiServiceRouter, label: &str) -> String {
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
            "description": format!("delete target account configuration suite: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-delete-target-account-configurations",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            // A target account configuration belongs to a template that targets MULTIPLE
            // accounts: `accountTargeting` and the configurations a template holds state
            // the same thing between them.
            "experimentOptions": { "accountTargeting": "multi-account" },
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
    body: Value,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}"),
        Some(body),
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

async fn get_template(router: &MultiServiceRouter, template_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}"),
        None,
    )
    .await
}

/// `GetExperimentTargetAccountConfiguration`: the configuration an EXPERIMENT holds.
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

/// A create body carrying `roleArn`, plus `description` and `clientToken` only when the
/// case names them — an absent member has to stay absent on the wire.
fn create_body(role_arn: &str, description: Option<&str>, client_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
    }
    if let Some(client_token) = client_token {
        body.insert("clientToken".to_string(), json!(client_token));
    }
    Value::Object(body)
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

/// A well-formed IAM role ARN of exactly `len` characters in the account `account_id`,
/// padded in the role name. `len` has to be at least [`SHORTEST_ROLE_ARN`].
///
/// The account is the CONFIGURED one because a target account configuration's `roleArn`
/// has to name a role of the account it configures.
fn role_arn_of(account_id: &str, len: usize) -> String {
    let prefix = format!("arn:aws:iam::{account_id}:role/");
    if len <= prefix.len() {
        format!("arn:aws:iam::1:role/{}", "x".repeat(len.saturating_sub(20)))[..len].to_string()
    } else {
        format!("{prefix}{}", "r".repeat(len - prefix.len()))
    }
}

/// One row: the configuration one account is created with, and then deleted.
struct Row {
    case: &'static str,
    account_id: String,
    role_arn: String,
    description: Option<String>,
    client_token: Option<String>,
}

/// The rows the delete rule is asserted over.
///
/// They differ along everything that rule generalises over: the length and shape of the
/// `accountId` naming the configuration being removed (both published bounds of
/// `TargetAccountId`), the length and shape of the stored `roleArn` (the shortest
/// well-formed ARN, a role with a path, a role at the published ceiling), whether the
/// stored `description`
/// is absent, empty, non-ASCII or at `TargetAccountConfigurationDescription`'s published
/// ceiling, and whether the configuration was created with a `clientToken` at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "account id at the published floor, no description, no token",
            account_id: account_id_of(ACCOUNT_ID_FLOOR),
            role_arn: role_arn_of(&account_id_of(ACCOUNT_ID_FLOOR), SHORTEST_ROLE_ARN),
            description: None,
            client_token: None,
        },
        Row {
            case: "account id at the published ceiling, role at the published ceiling",
            account_id: account_id_of(ACCOUNT_ID_CEILING),
            // An `accountId` at `TargetAccountId`'s ceiling is no account NUMBER, so it
            // names no account for a role to belong to: any well-formed role ARN is
            // admissible for it.
            role_arn: role_arn_of("000000000000", ROLE_ARN_CEILING),
            description: Some("the longest name this service accepts".to_string()),
            client_token: Some("token-ceiling".to_string()),
        },
        Row {
            case: "role with a path, empty description",
            account_id: "222222222222".to_string(),
            role_arn: "arn:aws:iam::222222222222:role/fis/nested/path/role".to_string(),
            description: Some(String::new()),
            client_token: Some("token-empty-description".to_string()),
        },
        Row {
            case: "non-ASCII description",
            account_id: "333333333333".to_string(),
            role_arn: "arn:aws:iam::333333333333:role/fis-ünïcode".to_string(),
            description: Some("cuenta de producción — región añadida ✓".to_string()),
            client_token: None,
        },
        Row {
            case: "description at the published ceiling",
            account_id: "444444444444".to_string(),
            role_arn: "arn:aws:iam::444444444444:role/fis-long-description".to_string(),
            description: Some("d".repeat(DESCRIPTION_CEILING)),
            client_token: Some("token-long-description".to_string()),
        },
    ]
}

/// What the configuration for one row has to look like, whoever reports it: the account
/// id it is named under, the stored `roleArn`, and the stored `description` — absent when
/// the record holds none.
fn assert_matches_row(case: &str, source: &str, row: &Row, configuration: &Value) {
    assert_eq!(
        configuration["accountId"],
        json!(row.account_id),
        "case {case:?}: {source} reported the wrong accountId: {configuration:?}"
    );
    assert_eq!(
        configuration["roleArn"],
        json!(row.role_arn),
        "case {case:?}: {source} reported the wrong roleArn: {configuration:?}"
    );
    // An explicitly empty `description` is normalized like an omitted one, so a row
    // created with one reports no description at all (the rule
    // `target_account_configuration_description.rs` owns).
    match row
        .description
        .as_deref()
        .filter(|description| !description.is_empty())
    {
        Some(description) => assert_eq!(
            configuration["description"],
            json!(description),
            "case {case:?}: {source} reported the wrong description: {configuration:?}"
        ),
        None => assert!(
            configuration.get("description").is_none(),
            "case {case:?}: {source} invented a description: {configuration:?}"
        ),
    }
}

/// The template's `targetAccountConfigurationsCount`.
async fn configuration_count(router: &MultiServiceRouter, template_id: &str) -> i64 {
    let answer = get_template(router, template_id).await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from GetExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["targetAccountConfigurationsCount"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!("no experimentTemplate.targetAccountConfigurationsCount in {:?}", answer.body)
        })
}

#[tokio::test]
async fn a_delete_removes_the_named_configuration_and_reports_what_it_removed() {
    let router = emulator();
    let template_id = template(&router, "delete-round-trip").await;
    let rows = rows();

    for row in &rows {
        let created = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                row.client_token.as_deref(),
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
    let total = i64::try_from(rows.len()).expect("the row count fits an i64");
    assert_eq!(
        configuration_count(&router, &template_id).await,
        total,
        "case setup: the template has to hold every created configuration"
    );

    for (deleted_so_far, row) in rows.iter().enumerate() {
        let deleted = delete_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, deleted.status, deleted.body
        );
        // The answer is the record that was removed, member for member.
        assert_matches_row(
            row.case,
            "DeleteTargetAccountConfiguration",
            row,
            deleted.configuration(),
        );

        // That name now holds nothing: the read, the update and a second delete of it all
        // refuse.
        let fetched = get_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {:?}: expected HTTP 404 from GetTargetAccountConfiguration after the \
             delete, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException from the read after the delete, \
             got {:?}",
            row.case,
            fetched.body
        );
        let updated = update_configuration(
            &router,
            &template_id,
            &row.account_id,
            json!({ "description": "an update of a configuration that is gone" }),
        )
        .await;
        assert_eq!(
            updated.status, 404,
            "case {:?}: expected HTTP 404 from UpdateTargetAccountConfiguration after the \
             delete, got {} with {:?}",
            row.case, updated.status, updated.body
        );
        let deleted_again = delete_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            deleted_again.status, 404,
            "case {:?}: expected HTTP 404 from a second DeleteTargetAccountConfiguration, \
             got {} with {:?}",
            row.case, deleted_again.status, deleted_again.body
        );
        assert_eq!(
            deleted_again.error_code(),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException from the second delete, got {:?}",
            row.case,
            deleted_again.body
        );

        // One configuration left, and the count the template reports is derived from what
        // it still holds.
        let remaining = total - i64::try_from(deleted_so_far + 1).expect("the index fits an i64");
        assert_eq!(
            configuration_count(&router, &template_id).await,
            remaining,
            "case {:?}: expected the template to hold {remaining} configurations after the \
             delete",
            row.case
        );

        // Every configuration the delete did NOT name is untouched.
        for untouched in rows.iter().skip(deleted_so_far + 1) {
            let fetched = get_configuration(&router, &template_id, &untouched.account_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {:?}: deleting {:?} must leave {:?} readable, got {} with {:?}",
                row.case, row.account_id, untouched.account_id, fetched.status, fetched.body
            );
            assert_matches_row(
                untouched.case,
                "GetTargetAccountConfiguration after another account's delete",
                untouched,
                fetched.configuration(),
            );
        }
    }
}

#[tokio::test]
async fn a_create_replaying_the_token_of_a_deleted_configuration_creates_afresh() {
    let router = emulator();
    let template_id = template(&router, "token-after-delete").await;
    let account_id = "555555555555";
    let token = "token-recreated";

    let created = create_configuration(
        &router,
        &template_id,
        account_id,
        create_body(
            "arn:aws:iam::555555555555:role/fis-before-delete",
            Some("before the delete"),
            Some(token),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    let deleted = delete_configuration(&router, &template_id, account_id).await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected HTTP 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The token lived on the record the delete removed, so this is a FIRST create for
    // that name again, not a replay reporting the removed record.
    let recreated = create_configuration(
        &router,
        &template_id,
        account_id,
        create_body(
            "arn:aws:iam::555555555555:role/fis-after-delete",
            None,
            Some(token),
        ),
    )
    .await;
    assert_eq!(
        recreated.status, 200,
        "case recreate: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        recreated.status, recreated.body
    );
    assert_eq!(
        recreated.configuration()["roleArn"],
        json!("arn:aws:iam::555555555555:role/fis-after-delete"),
        "case recreate: expected the new roleArn, got {:?}",
        recreated.body
    );
    assert!(
        recreated.configuration().get("description").is_none(),
        "case recreate: the removed record's description must not come back, got {:?}",
        recreated.body
    );

    let fetched = get_configuration(&router, &template_id, account_id).await;
    assert_eq!(
        fetched.status, 200,
        "case recreate: expected HTTP 200 from GetTargetAccountConfiguration, got {} with {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        fetched.configuration(),
        recreated.configuration(),
        "case recreate: the read has to report what the create answered, got {:?}",
        fetched.body
    );
    assert_eq!(
        configuration_count(&router, &template_id).await,
        1,
        "case recreate: the template holds exactly the one re-created configuration"
    );
}

#[tokio::test]
async fn a_delete_of_a_templates_configuration_leaves_a_started_experiment_alone() {
    let router = emulator();
    let template_id = template(&router, "experiment-isolated").await;
    let rows = rows();

    for row in &rows {
        let created = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            create_body(
                &row.role_arn,
                row.description.as_deref(),
                row.client_token.as_deref(),
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
    let experiment_id = start_experiment(&router, &template_id, "isolated").await;
    let total = i64::try_from(rows.len()).expect("the row count fits an i64");

    // Delete EVERY configuration off the template: the experiment holds the snapshot it
    // was started with, so none of this reaches it.
    for row in &rows {
        let deleted = delete_configuration(&router, &template_id, &row.account_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, deleted.status, deleted.body
        );
    }
    assert_eq!(
        configuration_count(&router, &template_id).await,
        0,
        "case experiment-isolated: the template holds nothing after every delete"
    );

    for row in &rows {
        let fetched = get_experiment_configuration(&router, &experiment_id, &row.account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetExperimentTargetAccountConfiguration \
             after the template's delete, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_matches_row(
            row.case,
            "GetExperimentTargetAccountConfiguration after the template's delete",
            row,
            fetched.configuration(),
        );
    }
    let experiment = get_experiment(&router, &experiment_id).await;
    assert_eq!(
        experiment.member("experiment")["targetAccountConfigurationsCount"],
        json!(total),
        "case experiment-isolated: the experiment keeps the count it was started with, \
         got {:?}",
        experiment.body
    );
}

#[tokio::test]
async fn a_delete_naming_a_configuration_the_store_does_not_hold_answers_resource_not_found() {
    let router = emulator();
    let configured = template(&router, "delete-configured").await;
    let other = template(&router, "delete-other").await;
    let account_id = "777777777777";
    let created = create_configuration(
        &router,
        &configured,
        account_id,
        create_body(
            "arn:aws:iam::777777777777:role/fis-isolated",
            Some("configured on one template only"),
            Some("token-delete-isolated"),
        ),
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

    // The rows differ along WHICH half of the name misses: the template, the account, or
    // both — and a configured account deleted on another template is a miss too.
    let cases: [(&str, &str, &str); 4] = [
        ("unknown-template", unknown_template, account_id),
        ("unknown-account", &configured, "888888888888"),
        ("unknown-both", unknown_template, "888888888888"),
        ("configured-on-another-template", &other, account_id),
    ];
    for (case, template_id, account) in cases {
        let deleted = delete_configuration(&router, template_id, account).await;
        assert_eq!(
            deleted.status, 404,
            "case {case:?}: expected HTTP 404 from DeleteTargetAccountConfiguration, got {} \
             with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            deleted.body
        );
    }

    // Every refused delete removed nothing: the one configured record is still there.
    let fetched = get_configuration(&router, &configured, account_id).await;
    assert_eq!(
        fetched.status, 200,
        "case refused-deletes: the configured record must survive, got {} with {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        configuration_count(&router, &configured).await,
        1,
        "case refused-deletes: the template still holds its one configuration"
    );
}

#[tokio::test]
async fn a_delete_outside_a_modelled_bound_is_refused_and_removes_nothing() {
    let router = emulator();
    let template_id = template(&router, "delete-bounds").await;

    // The account the bounds cases are run against, so a wrongly accepted request would
    // be visible as a removal.
    let account_id = account_id_of(ACCOUNT_ID_FLOOR);
    let created = create_configuration(
        &router,
        &template_id,
        &account_id,
        create_body(
            &format!("arn:aws:iam::{account_id}:role/fis-bounds"),
            Some("still here"),
            Some("token-delete-bounds"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected HTTP 200 from CreateTargetAccountConfiguration, got {} with {:?}",
        created.status, created.body
    );

    // Each published bound of each constrained member of the delete's input, and one past
    // it. A value INSIDE the bounds that names no record is a 404, not a 400: the bound
    // is what separates them.
    // The accepted delete of the stored record comes LAST, so every refusal above it is
    // asserted against a store that still holds it.
    let cases: Vec<(String, String, String, u16)> = vec![
        (
            format!("accountId at the ceiling ({ACCOUNT_ID_CEILING})"),
            template_id.clone(),
            account_id_of(ACCOUNT_ID_CEILING),
            404,
        ),
        (
            "accountId below the floor".to_string(),
            template_id.clone(),
            account_id_of(ACCOUNT_ID_FLOOR - 1),
            400,
        ),
        (
            "accountId above the ceiling".to_string(),
            template_id.clone(),
            account_id_of(ACCOUNT_ID_CEILING + 1),
            400,
        ),
        (
            format!("experimentTemplateId at the ceiling ({TEMPLATE_ID_CEILING})"),
            "E".repeat(TEMPLATE_ID_CEILING),
            account_id_of(ACCOUNT_ID_FLOOR),
            404,
        ),
        (
            "experimentTemplateId above the ceiling".to_string(),
            "E".repeat(TEMPLATE_ID_CEILING + 1),
            account_id_of(ACCOUNT_ID_FLOOR),
            400,
        ),
        (
            format!("accountId at the floor ({ACCOUNT_ID_FLOOR})"),
            template_id.clone(),
            account_id_of(ACCOUNT_ID_FLOOR),
            200,
        ),
    ];
    for (case, template_label, account, expected) in cases {
        let answer = delete_configuration(&router, &template_label, &account).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected} from DeleteTargetAccountConfiguration, \
             got {} with {:?}",
            answer.status, answer.body
        );
        if expected == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            // The refused request removed nothing: the stored record is still readable.
            let fetched = get_configuration(&router, &template_id, &account_id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: a refused delete must remove nothing, got {} with {:?}",
                fetched.status, fetched.body
            );
        }
    }

    // The one accepted delete, and only it, removed the record.
    let fetched = get_configuration(&router, &template_id, &account_id).await;
    assert_eq!(
        fetched.status, 404,
        "case accepted-delete: expected HTTP 404 after the in-bounds delete, got {} with {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        configuration_count(&router, &template_id).await,
        0,
        "case accepted-delete: the template holds nothing after its one configuration went"
    );
}
