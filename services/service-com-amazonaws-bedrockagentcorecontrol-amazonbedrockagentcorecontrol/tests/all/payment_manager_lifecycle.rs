// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the payment manager lifecycle (`CreatePaymentManager`,
//! `GetPaymentManager`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The longest `PaymentManagerName` the model admits: the pattern
/// `^[a-zA-Z][a-zA-Z0-9]{0,47}$` is one leading letter and up to 47 more characters
/// (`smithy_query operation=CreatePaymentManager`).
const MAX_NAME_LEN: usize = 48;

/// `PaymentsDescription`'s published bounds, `len 1..4096` (`smithy_query
/// operation=CreatePaymentManager`).
const MIN_DESCRIPTION_LEN: usize = 1;
const MAX_DESCRIPTION_LEN: usize = 4096;

/// The span `src/state/payment_managers.rs` gives the `CREATING` -> `READY` transition,
/// plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_300);

/// An IAM role ARN inside the modelled `RoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$` (`smithy_query
/// operation=CreatePaymentManager`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/PaymentManagerRole";

/// A customer managed KMS key ARN inside the modelled `KmsKeyArn` pattern
/// `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`
/// (`smithy_query operation=CreatePaymentManager`).
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012";

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreatePaymentManager`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

/// The `AuthorizerConfiguration` union's one member, `customJWTAuthorizer`, with the
/// `discoveryUrl` its pattern `^.+/\.well-known/openid-configuration$` admits
/// (`smithy_query shape=CustomJWTAuthorizerConfiguration`).
fn jwt_authorizer() -> Value {
    json!({
        "customJWTAuthorizer": {
            "discoveryUrl": "https://auth.example.com/.well-known/openid-configuration",
            "allowedClients": ["payments-checkout"]
        }
    })
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreatePaymentManager`: `POST /payments/managers`
/// (`smithy_query operation=CreatePaymentManager`).
async fn create_manager(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/payments/managers", Some(body)).await
}

/// `GetPaymentManager`: `GET /payments/managers/{paymentManagerId}`.
async fn get_manager(router: &MultiServiceRouter, payment_manager_id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/payments/managers/{payment_manager_id}"),
        None,
    )
    .await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `PaymentManagerId`'s pattern, `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` (`smithy_query
/// shape=PaymentManagerId`), checked by hand because this crate carries no regex engine:
/// the stem the id carries for `name` — which `PaymentManagerName` allows upper case in
/// and the id's pattern does not — then a 10-character lower-case suffix.
fn is_modelled_manager_id(id: &str, name: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{}-", name.to_lowercase())) else {
        return false;
    };
    suffix.len() == 10
        && suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant. `createdAt` and `lastUpdatedAt` target
/// `DateTimestamp`, which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetPaymentManager`), so an epoch-seconds number is not the modelled type.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// A payment manager is created from the request and read back through
/// `GetPaymentManager`, for every shape of the request `CreatePaymentManager` accepts.
///
/// The rows differ along the members the request carries and the sizes and forms they
/// take — the name's length and character classes, the authorizer type and the
/// configuration that goes with it, the description at each published bound of
/// `PaymentsDescription` (`len 1..4096`), the tag map's size, whether the data is
/// encrypted under a customer managed key, and whether an idempotency token is sent —
/// which is what the stored manager reports. A created manager is `CREATING`:
/// `PaymentManagerStatus` is `CREATING | UPDATING | DELETING | READY | CREATE_FAILED |
/// UPDATE_FAILED | DELETE_FAILED` (`smithy_query shape=PaymentManagerStatus`), and the
/// transition to `READY` is owed by the clock, not by the create.
#[tokio::test]
async fn a_payment_manager_is_created_and_read_back_for_every_request_shape() {
    let router = router();

    let min_description = "x".repeat(MIN_DESCRIPTION_LEN);
    let max_description = "d".repeat(MAX_DESCRIPTION_LEN);
    let longest_name = format!("n{}", "A".repeat(MAX_NAME_LEN - 1));

    // (case, name, the members beyond `name`, `authorizerType` and `roleArn`)
    let rows: [(&str, String, Value); 8] = [
        (
            "only the required members",
            "plainManager".to_string(),
            json!({}),
        ),
        (
            "the shortest description the model admits",
            "minDescribed".to_string(),
            json!({"description": min_description}),
        ),
        (
            "the longest description the model admits",
            "maxDescribed".to_string(),
            json!({"description": max_description}),
        ),
        (
            "a tagged manager",
            "taggedManager".to_string(),
            json!({
                "description": "the storefront's payment manager",
                "tags": {"owner": "retail", "environment": "test"}
            }),
        ),
        (
            "an empty tag map",
            "untaggedManager".to_string(),
            json!({"tags": {}}),
        ),
        (
            "a customer managed KMS key",
            "encryptedManager".to_string(),
            json!({"kmsKeyArn": KMS_KEY_ARN}),
        ),
        (
            "a one-letter name and an idempotency token",
            "m".to_string(),
            json!({"clientToken": client_token("firstShape")}),
        ),
        (
            "the longest name the model admits, carrying upper case",
            longest_name,
            json!({"description": "the widest name the pattern admits"}),
        ),
    ];

    // The authorizer type decides whether a configuration goes with the request: the two
    // values of `PaymentsAuthorizerType` are the same rule at its two ends.
    let authorizer_rows: [(&str, String, Value); 2] = [
        (
            "a digits-and-case name authorized by a bearer token",
            "payments2Manager".to_string(),
            json!({"authorizerType": "CUSTOM_JWT", "authorizerConfiguration": jwt_authorizer()}),
        ),
        (
            "a bearer-token authorizer alongside tags and a description",
            "jwtTagged".to_string(),
            json!({
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": jwt_authorizer(),
                "description": "authorized with a bearer token",
                "tags": {"tier": "gold"}
            }),
        ),
    ];

    for (case, name, extra) in rows.into_iter().chain(authorizer_rows) {
        let mut body = json!({
            "name": name,
            "authorizerType": "AWS_IAM",
            "roleArn": ROLE_ARN,
        });
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_manager(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreatePaymentManager, got {status} ({created})"
        );

        let id = text(&created, "paymentManagerId");
        assert!(
            is_modelled_manager_id(&id, &name),
            "case {case:?}: expected a paymentManagerId of the form \
             <name lower-cased>-<10 lower-case alphanumerics>, got {id:?}"
        );
        assert_eq!(
            text(&created, "paymentManagerArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:payment-manager/{id}"),
            "case {case:?}: expected the modelled PaymentManagerArn for {id}, got {}",
            text(&created, "paymentManagerArn")
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a created manager is CREATING, got {}",
            text(&created, "status")
        );
        assert_eq!(
            text(&created, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&created, "name")
        );
        assert_eq!(
            created["authorizerType"], body["authorizerType"],
            "case {case:?}: the authorizer type reads back as sent, got {}",
            created["authorizerType"]
        );
        assert_eq!(
            created["authorizerConfiguration"],
            body.get("authorizerConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the authorizer configuration reads back as sent, got {}",
            created["authorizerConfiguration"]
        );
        assert_eq!(
            text(&created, "roleArn"),
            ROLE_ARN,
            "case {case:?}: the role ARN reads back as sent, got {}",
            text(&created, "roleArn")
        );
        assert_eq!(
            created["kmsKeyArn"],
            body.get("kmsKeyArn").cloned().unwrap_or(Value::Null),
            "case {case:?}: the KMS key ARN reads back as sent, got {}",
            created["kmsKeyArn"]
        );
        let expected_tags = body
            .get("tags")
            .filter(|tags| !tags.as_object().is_some_and(serde_json::Map::is_empty))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            created["tags"], expected_tags,
            "case {case:?}: the tags read back as sent, got {}",
            created["tags"]
        );
        assert_eq!(
            created["workloadIdentityDetails"]["workloadIdentityArn"]
                .as_str()
                .unwrap_or_default(),
            format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:\
                 workload-identity-directory/default/workload-identity/{id}"
            ),
            "case {case:?}: expected the manager's own workload identity, got {}",
            created["workloadIdentityDetails"]
        );
        assert_date_time(case, "createdAt", &created["createdAt"]);

        let (status, fetched) = get_manager(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetPaymentManager, got {status} ({fetched})"
        );
        for member in [
            "paymentManagerId",
            "paymentManagerArn",
            "name",
            "authorizerType",
            "authorizerConfiguration",
            "roleArn",
            "kmsKeyArn",
            "tags",
            "workloadIdentityDetails",
            "createdAt",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetPaymentManager reports the {member} CreatePaymentManager \
                 handed back, got {} against {}",
                fetched[member], created[member]
            );
        }
        assert_eq!(
            fetched["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {case:?}: the description reads back as sent, got {}",
            fetched["description"]
        );
        assert_eq!(
            fetched["lastUpdatedAt"], fetched["createdAt"],
            "case {case:?}: expected lastUpdatedAt and createdAt to be the one create instant, \
             got {} and {}",
            fetched["lastUpdatedAt"], fetched["createdAt"]
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert_date_time(case, member, &fetched[member]);
        }

        // A read reports the stored manager without changing it, so a second read
        // answers exactly as the first did.
        let (status, reread) = get_manager(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the second GetPaymentManager, got {status} \
             ({reread})"
        );
        assert_eq!(
            reread, fetched,
            "case {case:?}: a read does not change what the next reader sees, got {reread} \
             against {fetched}"
        );
    }
}

/// The clock-owed transition: a manager a caller reads straight after the create is
/// still `CREATING`, and reports `READY` once the span the create owes has passed —
/// with every other member of the read unchanged.
#[tokio::test]
async fn a_created_payment_manager_settles_from_creating_to_ready() {
    let router = router();

    // The rows differ along the request shape, so the transition is the manager's own
    // and not a property of one configuration.
    let rows: [(&str, Value); 2] = [
        (
            "an IAM-authorized manager",
            json!({"name": "settlingIam", "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN}),
        ),
        (
            "a bearer-token-authorized manager",
            json!({
                "name": "settlingJwt",
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": jwt_authorizer(),
                "roleArn": ROLE_ARN,
                "tags": {"owner": "payments"}
            }),
        ),
    ];

    let mut created_managers: Vec<(&str, String, Value)> = Vec::new();
    for (case, body) in rows {
        let (status, created) = create_manager(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreatePaymentManager, got {status} ({created})"
        );
        let id = text(&created, "paymentManagerId");

        let (status, fetched) = get_manager(&router, &id).await;
        assert_eq!(
            (status, text(&fetched, "status")),
            (200, "CREATING".to_string()),
            "case {case:?}: a caller acting straight after the create still sees CREATING, got \
             {status} ({fetched})"
        );
        created_managers.push((case, id, fetched));
    }

    // Stored, never slept on: the transition is due at its instant whether or not
    // anybody looks, so one wait settles every manager created above.
    tokio::time::sleep(PAST_SETTLE).await;

    for (case, id, before) in created_managers {
        let (status, after) = get_manager(&router, &id).await;
        assert_eq!(
            (status, text(&after, "status")),
            (200, "READY".to_string()),
            "case {case:?}: past the settle span the CREATING -> READY transition is due, got \
             {status} ({after})"
        );
        for member in ["createdAt", "lastUpdatedAt", "paymentManagerArn", "name"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: the clock-owed status change rewrites no other member, but \
                 {member} moved from {} to {}",
                before[member], after[member]
            );
        }
    }
}

/// What `CreatePaymentManager` refuses, and with which modelled error: a `CUSTOM_JWT`
/// request carrying no authorizer configuration — "If you specify CUSTOM_JWT as the
/// authorizerType, you must provide an authorizerConfiguration" (`smithy_query doc
/// shape=CreatePaymentManager`) — and a member outside the constraints the model
/// publishes.
///
/// The rows differ along the rule that refuses the request, and each names the status and
/// error code a client branches on. The bounds come from `PaymentManagerName`'s pattern,
/// `PaymentsDescription`'s `len 1..4096`, `RoleArn`'s and `KmsKeyArn`'s patterns and
/// `PaymentsAuthorizerType`'s value list (`smithy_query operation=CreatePaymentManager`),
/// each tested one step past its published edge.
#[tokio::test]
async fn create_payment_manager_refuses_a_request_outside_the_rules_the_model_states() {
    let router = router();

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 8] = [
        (
            "CUSTOM_JWT with no authorizer configuration",
            json!({"name": "jwtNoConfig", "authorizerType": "CUSTOM_JWT", "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
        (
            "a name one character past the pattern",
            json!({
                "name": format!("n{}", "A".repeat(MAX_NAME_LEN)),
                "authorizerType": "AWS_IAM",
                "roleArn": ROLE_ARN
            }),
            400,
            "ValidationException",
        ),
        (
            "a name the pattern does not admit",
            json!({"name": "1manager", "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
        (
            "a name carrying a character class the pattern excludes",
            json!({"name": "payment_manager", "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
        (
            "a description one character short of its length bound",
            json!({
                "name": "emptyDescription",
                "authorizerType": "AWS_IAM",
                "roleArn": ROLE_ARN,
                "description": ""
            }),
            400,
            "ValidationException",
        ),
        (
            "a description one character past its length bound",
            json!({
                "name": "overlongDescription",
                "authorizerType": "AWS_IAM",
                "roleArn": ROLE_ARN,
                "description": "d".repeat(MAX_DESCRIPTION_LEN + 1)
            }),
            400,
            "ValidationException",
        ),
        (
            "a roleArn the pattern does not admit",
            json!({
                "name": "badRole",
                "authorizerType": "AWS_IAM",
                "roleArn": "arn:aws:iam::123456789012:user/not-a-role"
            }),
            400,
            "ValidationException",
        ),
        (
            "an authorizerType outside the enum's value list",
            json!({"name": "noneAuthorizer", "authorizerType": "NONE", "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answered) = create_manager(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
    }

    // Nothing was persisted by any refusal above: the names they carried are free, and
    // the same request with the one missing member is accepted.
    let (status, created) = create_manager(
        &router,
        &json!({
            "name": "jwtNoConfig",
            "authorizerType": "CUSTOM_JWT",
            "authorizerConfiguration": jwt_authorizer(),
            "roleArn": ROLE_ARN
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"CUSTOM_JWT with the configuration it needs\": expected HTTP 202, got {status} \
         ({created})"
    );
    assert_eq!(
        created["authorizerConfiguration"],
        jwt_authorizer(),
        "case \"CUSTOM_JWT with the configuration it needs\": the configuration reads back as \
         sent, got {}",
        created["authorizerConfiguration"]
    );
}

/// A replayed `clientToken` answers with the manager it first produced instead of
/// minting a second one: "If this token matches a previous request, the service ignores
/// the request, but doesn't return an error" (`smithy_query doc
/// shape=CreatePaymentManagerRequest member=clientToken`).
///
/// The rows differ along what the replay carries beside the token — the same body, a
/// different name, a different authorizer — because the token, not the body, decides.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_manager_it_first_created() {
    let router = router();

    let token = client_token("replayedToken");
    let first = json!({
        "name": "replayedManager",
        "authorizerType": "AWS_IAM",
        "roleArn": ROLE_ARN,
        "clientToken": token
    });
    let (status, created) = create_manager(&router, &first).await;
    assert_eq!(
        status, 202,
        "the first create of a token succeeds, got {status} ({created})"
    );
    let id = text(&created, "paymentManagerId");

    // (case, the replayed body)
    let rows: [(&str, Value); 3] = [
        ("the same request again", first.clone()),
        (
            "the same token with a different name",
            json!({
                "name": "otherName",
                "authorizerType": "AWS_IAM",
                "roleArn": ROLE_ARN,
                "clientToken": token
            }),
        ),
        (
            "the same token with a different authorizer",
            json!({
                "name": "replayedManager",
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": jwt_authorizer(),
                "roleArn": ROLE_ARN,
                "clientToken": token
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, replayed) = create_manager(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a replayed token is not an error, got {status} ({replayed})"
        );
        assert_eq!(
            text(&replayed, "paymentManagerId"),
            id,
            "case {case:?}: expected the manager the token first produced ({id}), got {}",
            text(&replayed, "paymentManagerId")
        );
        assert_eq!(
            replayed["name"], created["name"],
            "case {case:?}: the replay reports the stored manager, not the body it carried, got {}",
            replayed["name"]
        );
    }

    // A different token is a different request, so it mints its own manager.
    let (status, other) = create_manager(
        &router,
        &json!({
            "name": "replayedManager",
            "authorizerType": "AWS_IAM",
            "roleArn": ROLE_ARN,
            "clientToken": client_token("secondToken")
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"a second token\": expected HTTP 202, got {status} ({other})"
    );
    assert_ne!(
        text(&other, "paymentManagerId"),
        id,
        "case \"a second token\": expected a manager of its own, got {id}"
    );
    let (status, fetched) = get_manager(&router, &text(&other, "paymentManagerId")).await;
    assert_eq!(
        (status, text(&fetched, "paymentManagerId")),
        (200, text(&other, "paymentManagerId")),
        "case \"a second token\": the second manager reads back on its own id, got {status} \
         ({fetched})"
    );
}

/// `GetPaymentManager` answers the modelled 404 for an id this account holds no manager
/// under, and the modelled 400 for one outside `PaymentManagerId`'s constraints.
///
/// The rows differ along the id the request carries: well-formed but unknown, the id of
/// a manager that was never created under a name this account does hold, and ids one
/// step past each published edge of `PaymentManagerId` (`len 12..211`, pattern
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`, `smithy_query operation=GetPaymentManager`).
#[tokio::test]
async fn get_payment_manager_refuses_an_id_no_manager_is_stored_under() {
    let router = router();

    let (status, created) = create_manager(
        &router,
        &json!({"name": "storedManager", "authorizerType": "AWS_IAM", "roleArn": ROLE_ARN}),
    )
    .await;
    assert_eq!(
        status, 202,
        "the setup create succeeds, got {status} ({created})"
    );
    let stored_id = text(&created, "paymentManagerId");

    // The stored id with its last suffix character swapped for a different one: a
    // well-formed id of the same stem that names no stored manager.
    let (head, last) = stored_id.split_at(stored_id.len() - 1);
    let neighbour_id = format!("{head}{}", if last == "a" { "b" } else { "a" });

    // (case, the id the request carries, expected status, expected error code)
    let rows: [(&str, String, u16, &str); 5] = [
        (
            "a well-formed id nothing was stored under",
            "absentmanager-abcdefghij".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored manager's id with its suffix changed",
            neighbour_id,
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id one character short of the shortest the model admits",
            "a-abcdefghi".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an id carrying upper case, which the pattern excludes",
            "StoredManager-abcdefghij".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an id one character past the length bound",
            format!("{}-abcdefghij", "a".repeat(201)),
            400,
            "ValidationException",
        ),
    ];

    for (case, id, expected_status, expected_code) in rows {
        let (status, answered) = get_manager(&router, &id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for id {id:?}, got {status} \
             ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
        if expected_status == 404 {
            assert_eq!(
                text(&answered, "message"),
                format!("Payment manager with ID {id} does not exist"),
                "case {case:?}: expected the missing manager worded in terms of the id the \
                 request named, got {}",
                text(&answered, "message")
            );
        }
    }

    // The manager that WAS stored still reads back, so no refusal above touched it.
    let (status, fetched) = get_manager(&router, &stored_id).await;
    assert_eq!(
        (status, text(&fetched, "paymentManagerId")),
        (200, stored_id.clone()),
        "case \"the stored manager\": expected HTTP 200 for {stored_id}, got {status} ({fetched})"
    );
}
