// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` means to `CreateTargetAccountConfiguration`, end to end over
//! HTTP.
//!
//! The rule: an `@idempotencyToken` identifies the REQUEST it was first used with, not
//! merely the resource that request created. A later create carrying that token and the
//! SAME request reports the configuration the first call created, and one carrying it
//! with a request that differs in ANY member — the template or the account that name the
//! configuration, the `roleArn`, the `description` — is a parameter mismatch, refused with
//! `ConflictException` (HTTP 409) having created and changed nothing. Two requests that
//! the service stores identically are the same request: an omitted `description` and an
//! explicitly empty one are one stored value, so one is a retry of the other.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. The HTTP harness
//! below is local to this suite, as each suite's is.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

    /// The error `message`, or the empty string when the body carries none.
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

/// Create a MULTI-ACCOUNT experiment template — the targeting a target account
/// configuration belongs to — and report its minted id.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("template-{label}"),
            "description": format!("target account configuration idempotency: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-token-suite",
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

/// How many target account configurations the template `template_id` holds, as the
/// template itself reports it.
async fn configuration_count(router: &MultiServiceRouter, template_id: &str) -> u64 {
    let answer = send(
        router,
        "GET",
        &format!("/experimentTemplates/{template_id}"),
        None,
    )
    .await;
    answer.member("experimentTemplate")["targetAccountConfigurationsCount"]
        .as_u64()
        .unwrap_or_else(|| {
            panic!(
                "no experimentTemplate.targetAccountConfigurationsCount in {:?}",
                answer.body
            )
        })
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

/// A role ARN of the account `account_id`: a configuration's `roleArn` has to name a role
/// of the account it configures, so every value a case carries belongs to its own account.
fn owned_role_arn(account_id: &str, role_name: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/{role_name}")
}

/// The `ConflictException` message a token carried by another request earns.
fn token_conflict_message(token: &str) -> String {
    format!("clientToken {token} is not assigned to this resource.")
}

/// What the first create of each case carried, and what the second create changes.
struct Row {
    case: &'static str,
    /// The account the second create names — the first always configures
    /// [`FIRST_ACCOUNT`].
    account_id: &'static str,
    /// The role the second create carries, when it differs from the first's.
    role_name: &'static str,
    /// The description the second create carries.
    description: Option<&'static str>,
    /// Whether the second create names the OTHER template.
    other_template: bool,
    /// The status the second create earns.
    expected: u16,
}

/// The account the first create of every row configures.
const FIRST_ACCOUNT: &str = "111122223333";
/// The role name the first create of every row carries.
const FIRST_ROLE: &str = "fis-first";
/// The description the first create of every row carries.
const FIRST_DESCRIPTION: &str = "the request the token was first used with";

#[tokio::test]
async fn a_token_carried_by_a_different_request_is_a_conflict() {
    // The rows differ along WHICH member of the request the second create changes — one
    // per member a target account configuration is built from, including the two that
    // NAME it — plus the identical repeat, twice: one identical replay passing is not
    // evidence on its own that the request was rendered the same way both times.
    let rows = [
        Row {
            case: "identical repeat",
            account_id: FIRST_ACCOUNT,
            role_name: FIRST_ROLE,
            description: Some(FIRST_DESCRIPTION),
            other_template: false,
            expected: 200,
        },
        Row {
            case: "identical repeat again",
            account_id: FIRST_ACCOUNT,
            role_name: FIRST_ROLE,
            description: Some(FIRST_DESCRIPTION),
            other_template: false,
            expected: 200,
        },
        Row {
            case: "another roleArn",
            account_id: FIRST_ACCOUNT,
            role_name: "fis-second",
            description: Some(FIRST_DESCRIPTION),
            other_template: false,
            expected: 409,
        },
        Row {
            case: "another description",
            account_id: FIRST_ACCOUNT,
            role_name: FIRST_ROLE,
            description: Some("a description the first request did not carry"),
            other_template: false,
            expected: 409,
        },
        Row {
            case: "no description at all",
            account_id: FIRST_ACCOUNT,
            role_name: FIRST_ROLE,
            description: None,
            other_template: false,
            expected: 409,
        },
        Row {
            case: "another accountId",
            account_id: "222233334444",
            role_name: FIRST_ROLE,
            description: Some(FIRST_DESCRIPTION),
            other_template: false,
            expected: 409,
        },
        Row {
            case: "another experimentTemplateId",
            account_id: FIRST_ACCOUNT,
            role_name: FIRST_ROLE,
            description: Some(FIRST_DESCRIPTION),
            other_template: true,
            expected: 409,
        },
    ];

    for row in rows {
        let case = row.case;
        // A fresh emulator per row: the token of one row says nothing about another's.
        let router = emulator();
        let first_template = template(&router, "first").await;
        let other_template = template(&router, "other").await;
        let token = "shared-token";

        let first = create_configuration(
            &router,
            &first_template,
            FIRST_ACCOUNT,
            create_body(
                &owned_role_arn(FIRST_ACCOUNT, FIRST_ROLE),
                Some(FIRST_DESCRIPTION),
                Some(token),
            ),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with {:?}",
            first.status, first.body
        );

        let target_template = if row.other_template {
            &other_template
        } else {
            &first_template
        };
        let second = create_configuration(
            &router,
            target_template,
            row.account_id,
            create_body(
                &owned_role_arn(row.account_id, row.role_name),
                row.description,
                Some(token),
            ),
        )
        .await;
        assert_eq!(
            second.status, row.expected,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            row.expected, second.status, second.body
        );
        if row.expected == 200 {
            assert_eq!(
                second.configuration(),
                first.configuration(),
                "case {case:?}: expected the repeat to report the configuration the first \
                 create made, got {:?}",
                second.body
            );
        } else {
            assert_eq!(
                second.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                second.body
            );
            assert_eq!(
                second.message(),
                token_conflict_message(token),
                "case {case:?}: expected the token-mismatch message, got {:?}",
                second.body
            );
        }

        // Whatever the answer, the store holds exactly the one configuration the token
        // created, with the members the FIRST request carried.
        let fetched = get_configuration(&router, &first_template, FIRST_ACCOUNT).await;
        assert_eq!(
            fetched.configuration(),
            &json!({
                "accountId": FIRST_ACCOUNT,
                "roleArn": owned_role_arn(FIRST_ACCOUNT, FIRST_ROLE),
                "description": FIRST_DESCRIPTION,
            }),
            "case {case:?}: expected the first request's configuration to be unchanged, \
             got {:?}",
            fetched.body
        );
        assert_eq!(
            configuration_count(&router, &first_template).await,
            1,
            "case {case:?}: expected the first template to hold exactly one configuration"
        );
        assert_eq!(
            configuration_count(&router, &other_template).await,
            0,
            "case {case:?}: expected the other template to hold no configuration"
        );
        // A refused create left no SECOND resource behind under the reused token.
        if row.expected == 409 && row.account_id != FIRST_ACCOUNT {
            let missed = get_configuration(&router, target_template, row.account_id).await;
            assert_eq!(
                missed.status, 404,
                "case {case:?}: expected no configuration for the second account, got {} \
                 with {:?}",
                missed.status, missed.body
            );
        }
    }
}

#[tokio::test]
async fn a_description_the_store_holds_the_same_way_is_the_same_request() {
    // Two requests the service STORES identically are one request: an omitted description
    // and an explicitly empty one both leave a configuration with no description, so
    // either one repeated as the other is a retry and not a mismatch.
    for (case, first, second) in [
        ("empty then omitted", Some(""), None),
        ("omitted then empty", None, Some("")),
        ("empty twice", Some(""), Some("")),
        ("omitted twice", None, None),
    ] {
        let router = emulator();
        let template_id = template(&router, "description-normalization").await;
        let token = "normalized-description-token";
        let account_id = "333344445555";
        let role_arn = owned_role_arn(account_id, "fis-normalized");

        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(&role_arn, first, Some(token)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with {:?}",
            created.status, created.body
        );
        let repeated = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(&role_arn, second, Some(token)),
        )
        .await;
        assert_eq!(
            repeated.status, 200,
            "case {case:?}: expected the repeat to answer HTTP 200, got {} with {:?}",
            repeated.status, repeated.body
        );
        assert_eq!(
            repeated.configuration(),
            &json!({ "accountId": account_id, "roleArn": role_arn }),
            "case {case:?}: expected a configuration with no description, got {:?}",
            repeated.body
        );
        assert_eq!(
            configuration_count(&router, &template_id).await,
            1,
            "case {case:?}: expected the repeat to leave one configuration"
        );
    }
}

#[tokio::test]
async fn a_second_create_under_a_fresh_token_conflicts_on_the_account() {
    // The other conflict this operation answers, which the token rule must not swallow: a
    // create for an account the template is already configured for is refused on the
    // ACCOUNT, and says so — whether it carries a token the store has never seen or none
    // at all.
    for (case, token) in [("another token", Some("a-token-never-used")), ("no token", None)] {
        let router = emulator();
        let template_id = template(&router, "account-conflict").await;
        let account_id = "444455556666";

        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(
                &owned_role_arn(account_id, "fis-first"),
                None,
                Some("the-creating-token"),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with {:?}",
            created.status, created.body
        );

        let second = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(&owned_role_arn(account_id, "fis-second"), None, token),
        )
        .await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected HTTP 409 for a second create of one account, got {} \
             with {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.message(),
            format!(
                "Experiment template {template_id} already has a target account \
                 configuration for account {account_id}"
            ),
            "case {case:?}: expected the already-configured message, got {:?}",
            second.body
        );
    }
}

#[tokio::test]
async fn a_token_whose_configuration_is_gone_creates_afresh() {
    // A token resolves to a LIVE configuration: once the one it created has been deleted
    // it has nothing to report, so the next create carrying it creates again — with
    // whatever members that request carries, identical to the first or not.
    for (case, role_name, description) in [
        ("the same request", FIRST_ROLE, Some(FIRST_DESCRIPTION)),
        ("a different request", "fis-after-delete", None),
    ] {
        let router = emulator();
        let template_id = template(&router, "recreate-after-delete").await;
        let token = "recycled-token";
        let account_id = "555566667777";

        let created = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(
                &owned_role_arn(account_id, FIRST_ROLE),
                Some(FIRST_DESCRIPTION),
                Some(token),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with {:?}",
            created.status, created.body
        );
        let deleted = delete_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {} with {:?}",
            deleted.status, deleted.body
        );

        let recreated = create_configuration(
            &router,
            &template_id,
            account_id,
            create_body(
                &owned_role_arn(account_id, role_name),
                description,
                Some(token),
            ),
        )
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected the token to be free again, got {} with {:?}",
            recreated.status, recreated.body
        );
        // Read it back: what the recreate persisted is what this request carried, not
        // what the deleted one did.
        let fetched = get_configuration(&router, &template_id, account_id).await;
        assert_eq!(
            fetched.configuration()["roleArn"],
            json!(owned_role_arn(account_id, role_name)),
            "case {case:?}: expected the recreated configuration to carry this request's \
             role, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.configuration()["description"],
            description.map_or(Value::Null, |description| json!(description)),
            "case {case:?}: expected the recreated configuration to carry this request's \
             description, got {:?}",
            fetched.body
        );
    }
}
