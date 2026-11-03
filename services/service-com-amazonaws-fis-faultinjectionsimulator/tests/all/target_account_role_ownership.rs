// SPDX-License-Identifier: CC0-1.0

//! RULE: a target account configuration names ONE account and the IAM role FIS assumes in
//! it, so the two members have to agree — the account field of the `roleArn` is the account
//! the role belongs to, and a role belonging to any other account is refused with
//! `ValidationException` (HTTP 400), persisting nothing. The refusal names the `roleArn`
//! parameter and states the relationship, and it is the same refusal whichever account the
//! role actually belongs to.
//!
//! `RoleArn` is modelled only as a string within `@length(20..2048)`, `pattern ^[\S]+$`
//! (`smithy_query(query="shape",shape="RoleArn")`), so neither the ARN form nor this
//! relationship is expressible in the model: both are server-side rules, applied before any
//! state write. The one that fits the value is the one that refuses it — a value that is no
//! ARN at all names no account to compare, so it is refused as a malformed ARN, which is
//! the rule `target_account_configuration_errors.rs` owns.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization. The rows differ
//! along what the rule generalises over: whether the role belongs to the configured
//! account, HOW it differs when it does not (another account entirely, the emulator's own
//! account, an account field that is no account id, an empty account field), the shape of
//! the role that does belong (a plain role, a role with a path, another partition), and
//! which operation carries it (`CreateTargetAccountConfiguration` and
//! `UpdateTargetAccountConfiguration`, which both name an account and may carry a role for
//! it).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// The account this emulator's own ARNs carry — a role of it belongs to NO other account,
/// which is what one of the mismatch rows is about.
const EMULATOR_ACCOUNT: &str = "000000000000";

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

    /// The `roleArn` of the `targetAccountConfiguration` this answer carries.
    fn role_arn(&self) -> &str {
        self.body["targetAccountConfiguration"]["roleArn"]
            .as_str()
            .unwrap_or_default()
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

/// An experiment template to hang configurations off, reported by its minted id.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": format!("ownership-template-{label}"),
            "description": format!("role ownership: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-role-ownership",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            },
            "experimentOptions": { "accountTargeting": "multi-account" }
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected the template to be created, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["experimentTemplate"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no template id in {:?}", answer.body))
        .to_string()
}

/// A create body carrying `role_arn`, plus a `clientToken` so a retry is a replay.
fn create_body(role_arn: &str, client_token: &str) -> Value {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(role_arn));
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

/// One row: the account being configured, the `roleArn` offered for it, and whether that
/// role belongs to that account.
struct Row {
    case: &'static str,
    account_id: &'static str,
    role_arn: String,
    /// `true` when the role belongs to the configured account, so the call is accepted.
    owned: bool,
}

/// The rows the ownership rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a plain role of the configured account",
            account_id: "111122223333",
            role_arn: "arn:aws:iam::111122223333:role/sim2real-fis".to_string(),
            owned: true,
        },
        Row {
            case: "a role with a path, of the configured account",
            account_id: "222233334444",
            role_arn: "arn:aws:iam::222233334444:role/service-role/nested/fis".to_string(),
            owned: true,
        },
        Row {
            case: "a role of the configured account in another partition",
            account_id: "333344445555",
            role_arn: "arn:aws-cn:iam::333344445555:role/fis-目标账户".to_string(),
            owned: true,
        },
        Row {
            case: "a role of another account entirely",
            account_id: "444455556666",
            role_arn: "arn:aws:iam::999988887777:role/sim2real-fis".to_string(),
            owned: false,
        },
        Row {
            case: "a role of the emulator's own account, configuring another",
            account_id: "555566667777",
            role_arn: format!("arn:aws:iam::{EMULATOR_ACCOUNT}:role/sim2real-fis"),
            owned: false,
        },
        Row {
            case: "a role whose account field is no account id",
            account_id: "666677778888",
            role_arn: "arn:aws:iam::not-an-account-id:role/sim2real-fis-malformed".to_string(),
            owned: false,
        },
        Row {
            case: "a role whose account field is empty",
            account_id: "777788889999",
            role_arn: "arn:aws:iam:::role/sim2real-fis".to_string(),
            owned: false,
        },
        Row {
            case: "a role of an account that only PREFIXES the configured one",
            account_id: "888899990000",
            role_arn: "arn:aws:iam::888899990:role/sim2real-fis".to_string(),
            owned: false,
        },
    ]
}

#[tokio::test]
async fn a_target_account_configuration_takes_only_a_role_of_the_account_it_configures() {
    let router = emulator();
    let template_id = template(&router, "create").await;

    // Every refusal of a role that does not belong to the account is the SAME refusal: the
    // message does not vary with which account the role belongs to. Collected here so the
    // rows are compared against each other rather than against a recorded string.
    let mut refusals: Vec<(&str, String)> = Vec::new();

    for row in rows() {
        let answer = create_configuration(
            &router,
            &template_id,
            row.account_id,
            create_body(&row.role_arn, &format!("token-create-{}", row.account_id)),
        )
        .await;

        if row.owned {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 for a role of the configured account, \
                 got {} with {:?}",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                answer.role_arn(),
                row.role_arn,
                "case {:?}: expected the create to report the roleArn it carried, got {:?}",
                row.case,
                answer.body
            );
            // The round trip: the read reports what the create persisted.
            let fetched = get_configuration(&router, &template_id, row.account_id).await;
            assert_eq!(
                fetched.role_arn(),
                row.role_arn,
                "case {:?}: expected the read to report the created roleArn, got {:?}",
                row.case,
                fetched.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 for a role of another account, got {} with {:?}",
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
            message.contains("roleArn") && message.contains("accountId"),
            "case {:?}: expected the refusal to name the roleArn parameter and the \
             accountId it must belong to, got {message:?}",
            row.case
        );
        // Nothing was persisted under the name the refused create named.
        let fetched = get_configuration(&router, &template_id, row.account_id).await;
        assert_eq!(
            fetched.status, 404,
            "case {:?}: a refused create must persist nothing, got {} with {:?}",
            row.case, fetched.status, fetched.body
        );
        refusals.push((row.case, message));
    }

    let (first_case, first_message) = refusals
        .first()
        .cloned()
        .expect("the table carries rows whose role belongs to another account");
    for (case, message) in &refusals {
        assert_eq!(
            message, &first_message,
            "case {case:?}: expected the same refusal as {first_case:?} \
             ({first_message:?}), got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_update_takes_only_a_role_of_the_account_the_configuration_names() {
    let router = emulator();
    let template_id = template(&router, "update").await;

    for row in rows() {
        // The record the update is applied to, created with a role that DOES belong to the
        // account, so what a refusal leaves behind is visible.
        let baseline = format!("arn:aws:iam::{}:role/fis-ownership-baseline", row.account_id);
        let created = create_configuration(
            &router,
            &template_id,
            row.account_id,
            create_body(&baseline, &format!("token-update-{}", row.account_id)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
            row.case, created.status, created.body
        );

        let answer =
            update_configuration(&router, &template_id, row.account_id, &row.role_arn).await;
        let expected_status = if row.owned { 200 } else { 400 };
        assert_eq!(
            answer.status, expected_status,
            "case {:?}: expected HTTP {expected_status} from the update, got {} with {:?}",
            row.case, answer.status, answer.body
        );

        let fetched = get_configuration(&router, &template_id, row.account_id).await;
        let expected_role = if row.owned { &row.role_arn } else { &baseline };
        assert_eq!(
            fetched.role_arn(),
            *expected_role,
            "case {:?}: expected the stored roleArn to be {expected_role:?}, got {:?}",
            row.case,
            fetched.body
        );
        if !row.owned {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {:?}: expected ValidationException, got {:?}",
                row.case,
                answer.body
            );
            assert!(
                answer.message().contains("roleArn") && answer.message().contains("accountId"),
                "case {:?}: expected the refusal to name the roleArn parameter and the \
                 accountId it must belong to, got {:?}",
                row.case,
                answer.message()
            );
        }
    }
}

/// RULE (the complement): the ownership comparison is made against an `accountId` that IS
/// an account number. `TargetAccountId` is modelled `@length(12..48)`, so a value inside
/// those bounds that is no account id names no account for a role to belong to — refusing
/// THAT value is the business of the rule on `accountId` itself — and such a request is
/// decided by the ARN form alone.
#[tokio::test]
async fn an_account_id_that_is_no_account_number_is_not_compared_against() {
    let router = emulator();
    let template_id = template(&router, "not-an-account").await;

    // The rows differ along the shape of the `accountId`: an account id with extra digits,
    // a non-numeric value, and a value at `TargetAccountId`'s published ceiling — each
    // offered a well-formed role ARN of a DIFFERENT account, which the ownership rule
    // cannot be applied to.
    let rows: [(&str, String); 3] = [
        ("thirteen-digits", "1111222233334".to_string()),
        ("not-numeric-at-all", "an-account-alias".to_string()),
        ("at-the-published-ceiling", "9".repeat(48)),
    ];
    for (case, account_id) in rows {
        let role_arn = format!("arn:aws:iam::{EMULATOR_ACCOUNT}:role/fis-unowned");
        let answer = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(&role_arn, &format!("token-unowned-{case}")),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the ARN form alone to decide the request, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.role_arn(),
            role_arn,
            "case {case:?}: expected the create to report the roleArn it carried, got {:?}",
            answer.body
        );

        // And the ARN form still decides it: a value that is no ARN at all is refused by
        // the rule that fits it, not by the ownership one.
        let malformed = create_configuration(
            &router,
            &template_id,
            &account_id,
            create_body(
                "this-is-not-a-valid-iam-role-arn-000000",
                &format!("token-unowned-malformed-{case}"),
            ),
        )
        .await;
        assert_eq!(
            malformed.status, 400,
            "case {case:?}: expected HTTP 400 for a value that is no ARN, got {} with {:?}",
            malformed.status, malformed.body
        );
        assert_eq!(
            malformed.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            malformed.body
        );
        assert!(
            malformed.message().contains("roleArn"),
            "case {case:?}: expected the refusal to name the member, got {:?}",
            malformed.message()
        );
    }
}
