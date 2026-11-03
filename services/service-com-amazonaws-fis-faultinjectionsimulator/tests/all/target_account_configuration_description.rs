// SPDX-License-Identifier: CC0-1.0

//! The `description` of a FIS target account configuration, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one FIS's target-account-configuration
//! surface states for that member:
//!
//! * a target account configuration either HAS a description — with characters in it,
//!   reported back exactly as the request carried it — or has none at all, and a request
//!   carrying an explicitly empty `description` writes the second: a configuration written
//!   with one reports NO `description` member, which is the same answer a configuration
//!   that never had one gives, so an empty string is how a caller CLEARS a description;
//! * the rule holds on BOTH writes, the create and the update, and on every reader of a
//!   configuration (`GetTargetAccountConfiguration`,
//!   `ListTargetAccountConfigurations`, `GetExperimentTargetAccountConfiguration` and
//!   `ListExperimentTargetAccountConfigurations`), because what the readers report is
//!   what the write persisted;
//! * only the EMPTY string is a cleared description: whitespace is characters, so a
//!   description of a space or a newline round-trips exactly like any other.
//!
//! The HTTP harness below is local to this suite: the ones in the sibling suites are
//! private to those suites, and a test harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`, `@pattern ^[\s\S]*$`), spelled out here rather than
/// imported from the crate so a wrong bound in the model or the handler fails this suite.
const DESCRIPTION_CEILING: usize = 512;

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
    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    /// The `targetAccountConfiguration` this answer carries.
    fn configuration(&self) -> &Value {
        self.member("targetAccountConfiguration")
    }

    /// The `targetAccountConfigurations` page this answer carries.
    fn configurations(&self) -> &Vec<Value> {
        self.member("targetAccountConfigurations")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "no targetAccountConfigurations list in {:?}",
                    self.body
                )
            })
    }

    /// The listed configuration of `account_id`.
    fn listed(&self, case: &str, account_id: &str) -> &Value {
        self.configurations()
            .iter()
            .find(|candidate| candidate["accountId"] == json!(account_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no listed configuration for account {account_id:?} in {:?}",
                    self.body
                )
            })
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

/// Create an experiment template to hang target account configurations off, and report its
/// minted id.
///
/// The template targets MULTIPLE accounts, which is the targeting a target account
/// configuration belongs to: `accountTargeting` and the configurations a template holds
/// state the same thing between them.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            // `ClientToken` is modelled `pattern ^[\S]+$`, so the label carries no
            // whitespace.
            "clientToken": format!("template-{label}"),
            "description": format!("target account configuration description suite: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-target-account-descriptions",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
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

/// A create body carrying `roleArn` and a `clientToken`, plus `description` only when the
/// case names one — an omitted member has to stay off the wire for the "omitted" case to be
/// under test at all.
fn create_body(role_arn: &str, description: Option<&str>, client_token: &str) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
    }
    body.insert("clientToken".to_string(), json!(client_token));
    Value::Object(body)
}

/// An update body carrying `description` only when the case names one: `roleArn` and
/// `description` are both optional on `UpdateTargetAccountConfiguration`, and an empty
/// body is a legal update that carries nothing.
fn update_body(description: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(description) = description {
        body.insert("description".to_string(), json!(description));
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

/// `GetTargetAccountConfiguration`: the configuration the TEMPLATE holds.
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

/// `ListTargetAccountConfigurations`: every configuration the TEMPLATE holds.
async fn list_configurations(router: &MultiServiceRouter, template_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}/targetAccountConfigurations"),
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

/// `ListExperimentTargetAccountConfigurations`: every configuration an EXPERIMENT holds.
async fn list_experiment_configurations(
    router: &MultiServiceRouter,
    experiment_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/experiments/{experiment_id}/targetAccountConfigurations"),
        None,
    )
    .await
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

/// One row: a `description` a request may carry for a configuration.
struct Row {
    case: &'static str,
    /// The value the request carries, or `None` when the request omits the member.
    sent: Option<String>,
}

impl Row {
    /// The `description` a configuration written with [`Row::sent`] REPORTS: the value
    /// itself when it has characters in it, and nothing at all when it is the empty
    /// string — which is the same thing an omitted description leaves behind.
    fn reported(&self) -> Option<&str> {
        self.sent
            .as_deref()
            .filter(|description| !description.is_empty())
    }
}

/// The account one row's configuration is named under: twelve digits, so the row's
/// `roleArn` can belong to it, and one account per (group, row) so no write can touch
/// another row's record.
fn configured_account(group: u8, index: usize) -> String {
    format!("8{group}00000000{index:02}")
}

/// The rows the rule is asserted over.
///
/// They differ along everything the rule generalises over: whether the member is on the
/// wire at all, and — when it is — the value's size (the empty string, one character, the
/// published ceiling) and its characters (plain ASCII, non-ASCII, and whitespace only,
/// which `@pattern ^[\s\S]*$` admits and which is NOT an empty description).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "omitted",
            sent: None,
        },
        Row {
            case: "empty",
            sent: Some(String::new()),
        },
        Row {
            case: "one-character",
            sent: Some("x".to_string()),
        },
        Row {
            case: "a-single-space",
            sent: Some(" ".to_string()),
        },
        Row {
            case: "whitespace-only-newline-and-tab",
            sent: Some("\n\t".to_string()),
        },
        Row {
            case: "plain-text",
            sent: Some("the staging account".to_string()),
        },
        Row {
            case: "non-ascii",
            sent: Some("説明 — ünïcødé ✅ описание".to_string()),
        },
        Row {
            case: "at-the-published-ceiling",
            sent: Some("d".repeat(DESCRIPTION_CEILING)),
        },
    ]
}

/// A role of the account `account_id`: a target account configuration's `roleArn` has to
/// name a role of the account it configures.
fn role_arn(account_id: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/fis-target-account")
}

/// What the `description` of `configuration` has to be: `expected` exactly, or no member
/// at all when the configuration has no description.
fn assert_description(case: &str, source: &str, expected: Option<&str>, configuration: &Value) {
    match expected {
        Some(expected) => assert_eq!(
            configuration["description"],
            json!(expected),
            "case {case:?}: expected {source} to report a description of {} characters, \
             got {configuration:?}",
            expected.chars().count()
        ),
        None => assert!(
            configuration.get("description").is_none_or(Value::is_null),
            "case {case:?}: expected {source} to report NO description, got {configuration:?}"
        ),
    }
}

#[tokio::test]
async fn a_created_description_is_reported_by_every_reader_and_an_empty_one_is_no_description() {
    let router = emulator();
    let template_id = template(&router, "create").await;
    let rows = rows();

    for (index, row) in rows.iter().enumerate() {
        let account_id = configured_account(1, index);
        let created = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(
                &role_arn(&account_id),
                row.sent.as_deref(),
                &format!("token-create-{index}"),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, created.status, created.body
        );
        assert_description(
            row.case,
            "the create answer",
            row.reported(),
            created.configuration(),
        );

        // The round trip: what the create answered is what the store holds.
        let fetched = get_configuration(&router, &template_id, &account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetTargetAccountConfiguration, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.configuration(),
            created.configuration(),
            "case {:?}: the read disagreed with the create: {:?}",
            row.case,
            fetched.body
        );
        assert_description(
            row.case,
            "GetTargetAccountConfiguration",
            row.reported(),
            fetched.configuration(),
        );
    }

    // Every reader of the TEMPLATE's configurations agrees with the create.
    let listed = list_configurations(&router, &template_id).await;
    assert_eq!(
        listed.status, 200,
        "case create: expected HTTP 200 from ListTargetAccountConfigurations, got {} with {:?}",
        listed.status, listed.body
    );
    for (index, row) in rows.iter().enumerate() {
        let account_id = configured_account(1, index);
        assert_description(
            row.case,
            "ListTargetAccountConfigurations",
            row.reported(),
            listed.listed(row.case, &account_id),
        );
    }

    // And every reader of an EXPERIMENT's snapshot of the same configurations.
    let experiment_id = start_experiment(&router, &template_id, "create").await;
    let listed_for_experiment = list_experiment_configurations(&router, &experiment_id).await;
    assert_eq!(
        listed_for_experiment.status, 200,
        "case create: expected HTTP 200 from ListExperimentTargetAccountConfigurations, \
         got {} with {:?}",
        listed_for_experiment.status, listed_for_experiment.body
    );
    for (index, row) in rows.iter().enumerate() {
        let account_id = configured_account(1, index);
        let fetched = get_experiment_configuration(&router, &experiment_id, &account_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetExperimentTargetAccountConfiguration, \
             got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        assert_description(
            row.case,
            "GetExperimentTargetAccountConfiguration",
            row.reported(),
            fetched.configuration(),
        );
        assert_description(
            row.case,
            "ListExperimentTargetAccountConfigurations",
            row.reported(),
            listed_for_experiment.listed(row.case, &account_id),
        );
    }
}

/// The `description` every row of the update test starts from, so a row that CLEARS one
/// has something to clear and a row that omits the member has something to keep.
const BASELINE_DESCRIPTION: &str = "the description the create carried";

#[tokio::test]
async fn an_update_carrying_an_empty_description_clears_it_and_one_omitting_it_keeps_it() {
    let router = emulator();
    let template_id = template(&router, "update").await;
    let rows = rows();

    for (index, row) in rows.iter().enumerate() {
        let account_id = configured_account(2, index);
        let created = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(
                &role_arn(&account_id),
                Some(BASELINE_DESCRIPTION),
                &format!("token-update-{index}"),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
            row.case, created.status, created.body
        );

        // A `description` the update carries replaces the stored one — and an explicitly
        // empty one leaves the configuration with none at all; a member the update omits
        // keeps what the store holds.
        let expected = match row.sent {
            Some(_) => row.reported(),
            None => Some(BASELINE_DESCRIPTION),
        };

        let body = update_body(row.sent.as_deref());
        let updated = update_configuration(&router, &template_id, &account_id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, updated.status, updated.body
        );
        assert_description(
            row.case,
            "the update answer",
            expected,
            updated.configuration(),
        );

        // The round trip: what the update answered is what the store holds.
        let fetched = get_configuration(&router, &template_id, &account_id).await;
        assert_eq!(
            fetched.configuration(),
            updated.configuration(),
            "case {:?}: the read disagreed with the update: {:?}",
            row.case,
            fetched.body
        );
        assert_description(
            row.case,
            "GetTargetAccountConfiguration after the update",
            expected,
            fetched.configuration(),
        );

        // Idempotent: applying the same update again reports the same record, so clearing
        // a description twice is still a cleared description.
        let again = update_configuration(&router, &template_id, &account_id, body).await;
        assert_eq!(
            again.configuration(),
            updated.configuration(),
            "case {:?}: applying one update twice reported two different records: {:?}",
            row.case,
            again.body
        );

        // Writing back what the configuration REPORTS changes nothing either: the stored
        // value is already the normalized one.
        let rewritten =
            update_configuration(&router, &template_id, &account_id, update_body(expected)).await;
        assert_eq!(
            rewritten.configuration(),
            updated.configuration(),
            "case {:?}: writing back the reported description changed the record: {:?}",
            row.case,
            rewritten.body
        );
    }

    // A cleared description is indistinguishable from one that was never set: the
    // configuration an empty string cleared reports exactly what a configuration created
    // without a description reports, member for member but for the two that name it.
    let never_set = "830000000099";
    let created = create_configuration(
        &router,
        &template_id,
        never_set,
        create_body(&role_arn(never_set), None, "token-never-set"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case cleared-equals-never-set: expected HTTP 200 creating the comparison record, \
         got {} with {:?}",
        created.status, created.body
    );
    let cleared_index = rows
        .iter()
        .position(|row| row.sent.as_deref() == Some(""))
        .expect("the rows carry an explicitly empty description");
    let cleared_account = configured_account(2, cleared_index);
    let cleared = get_configuration(&router, &template_id, &cleared_account).await;
    assert_eq!(
        cleared.configuration().as_object().map(|members| members.len()),
        created
            .configuration()
            .as_object()
            .map(|members| members.len()),
        "case cleared-equals-never-set: a cleared description must leave the same members \
         as a configuration that never had one: {:?} against {:?}",
        cleared.body,
        created.body
    );
    assert_description(
        "cleared-equals-never-set",
        "GetTargetAccountConfiguration of the cleared configuration",
        None,
        cleared.configuration(),
    );

    // The template still holds exactly the records the creates made: no update created a
    // second one.
    let listed = list_configurations(&router, &template_id).await;
    assert_eq!(
        listed.configurations().len(),
        rows.len() + 1,
        "case update: expected the template to hold the {} configurations the creates \
         made, got {:?}",
        rows.len() + 1,
        listed.body
    );
}
