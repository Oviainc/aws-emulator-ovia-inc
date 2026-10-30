// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the token vault's KMS configuration
//! (`SetTokenVaultCMK`, `GetTokenVault`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The `tokenVaultId` of the vault an account holds without creating one, and the one a
/// request naming no vault addresses (`src/state.rs`'s `DEFAULT_TOKEN_VAULT_ID`).
const DEFAULT_VAULT: &str = "default";

/// `KeyType`'s full value list, `CustomerManagedKey | ServiceManagedKey`
/// (`smithy_query shape=KeyType`).
const KEY_TYPE_CUSTOMER_MANAGED: &str = "CustomerManagedKey";
const KEY_TYPE_SERVICE_MANAGED: &str = "ServiceManagedKey";

/// `TokenVaultIdType`'s length bounds, `len 1..64`
/// (`smithy_query shape=TokenVaultIdType`).
const VAULT_ID_MIN_LENGTH: usize = 1;
const VAULT_ID_MAX_LENGTH: usize = 64;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `SetTokenVaultCMK`: `POST /identities/set-token-vault-cmk`
/// (`smithy_query operation=SetTokenVaultCMK`).
async fn set_cmk(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "/identities/set-token-vault-cmk", body).await
}

/// `GetTokenVault`: `POST /identities/get-token-vault`.
async fn get_vault(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "/identities/get-token-vault", body).await
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

/// The three members every token vault response carries.
fn reported(body: &Value) -> (Value, Value, Value) {
    (
        body["tokenVaultId"].clone(),
        body["kmsConfiguration"].clone(),
        body["lastModifiedDate"].clone(),
    )
}

/// A customer managed key ARN in `partition`/`region`, per `KmsKeyArn`'s pattern
/// `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`
/// (`smithy_query shape=KmsConfiguration`). The account id is `engine::mock_account_id`.
fn key_arn(partition: &str, region: &str, key: &str) -> String {
    format!("arn:{partition}:kms:{region}:000000000000:key/{key}")
}

/// Every KMS configuration the model admits is stored and read back as it was set, for a
/// request that names the vault and for one that leaves `tokenVaultId` to its default.
///
/// The rows differ along the key type, whether a key ARN accompanies it and which
/// partition that ARN is in, because `kmsConfiguration` is replaced wholesale by a set:
/// the configuration a later read reports is the last one written, never a merge of two.
#[tokio::test]
async fn every_kms_configuration_is_stored_and_read_back_unchanged() {
    let router = router();

    // (case, the kmsConfiguration sent, whether the request names the vault)
    let rows: [(&str, Value, bool); 6] = [
        (
            "a service managed key, vault named",
            json!({"keyType": KEY_TYPE_SERVICE_MANAGED}),
            true,
        ),
        (
            "a customer managed key in the aws partition",
            json!({
                "keyType": KEY_TYPE_CUSTOMER_MANAGED,
                "kmsKeyArn": key_arn("aws", "us-east-1", "11111111-2222-3333-4444-555555555555"),
            }),
            true,
        ),
        (
            "a customer managed key, vault left to the default",
            json!({
                "keyType": KEY_TYPE_CUSTOMER_MANAGED,
                "kmsKeyArn": key_arn("aws", "eu-west-2", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            }),
            false,
        ),
        (
            "a customer managed key in the aws-us-gov partition",
            json!({
                "keyType": KEY_TYPE_CUSTOMER_MANAGED,
                "kmsKeyArn": key_arn("aws-us-gov", "us-gov-west-1", "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"),
            }),
            true,
        ),
        (
            "a customer managed key in the aws-cn partition",
            json!({
                "keyType": KEY_TYPE_CUSTOMER_MANAGED,
                "kmsKeyArn": key_arn("aws-cn", "cn-north-1", "ffffffff-0000-1111-2222-333333333333"),
            }),
            false,
        ),
        (
            "back to a service managed key, which carries no key ARN",
            json!({"keyType": KEY_TYPE_SERVICE_MANAGED}),
            false,
        ),
    ];

    let mut previous_modified = 0.0_f64;
    for (case, kms_configuration, names_vault) in rows {
        let mut request = json!({"kmsConfiguration": kms_configuration.clone()});
        if names_vault {
            request["tokenVaultId"] = json!(DEFAULT_VAULT);
        }

        let (status, answer) = set_cmk(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from SetTokenVaultCMK, got {status} ({answer})"
        );
        let (id, stored, modified) = reported(&answer);
        assert_eq!(
            (id.clone(), stored.clone()),
            (json!(DEFAULT_VAULT), kms_configuration.clone()),
            "case {case:?}: SetTokenVaultCMK answers with the vault it re-keyed and the \
             configuration it was given, got ({id}, {stored})"
        );
        let modified_seconds = modified.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: lastModifiedDate is an epoch-seconds number, got {modified}")
        });
        assert!(
            modified_seconds >= previous_modified,
            "case {case:?}: expected lastModifiedDate at or after the previous set's \
             {previous_modified}, got {modified_seconds}"
        );
        previous_modified = modified_seconds;

        // The round trip: the same three members, through the reader, for a request that
        // names the vault and for one that does not.
        for named in [true, false] {
            let read = if named {
                json!({"tokenVaultId": DEFAULT_VAULT})
            } else {
                json!({})
            };
            let (status, fetched) = get_vault(&router, &read).await;
            assert_eq!(
                status, 200,
                "case {case:?} (tokenVaultId named: {named}): expected HTTP 200 from \
                 GetTokenVault, got {status} ({fetched})"
            );
            assert_eq!(
                reported(&fetched),
                (json!(DEFAULT_VAULT), kms_configuration.clone(), modified.clone()),
                "case {case:?} (tokenVaultId named: {named}): GetTokenVault reports exactly what \
                 the set stored, got {fetched}"
            );
        }
    }
}

/// A vault nobody holds is refused by both operations, and the refusal writes nothing.
///
/// This service models no operation that creates a token vault, so every id other than
/// the account's default one names a vault that does not exist. The rows differ along the
/// shape of that id — including both published length bounds, which are inside
/// `TokenVaultIdType` and so reach the handler rather than the boundary validator.
#[tokio::test]
async fn an_id_naming_no_vault_is_refused_and_changes_nothing() {
    let router = router();

    let keeper = json!({
        "keyType": KEY_TYPE_CUSTOMER_MANAGED,
        "kmsKeyArn": key_arn("aws", "us-east-1", "9999aaaa-bbbb-cccc-dddd-eeeeffff0000"),
    });
    let (status, kept) = set_cmk(&router, &json!({"kmsConfiguration": keeper.clone()})).await;
    assert_eq!(
        status, 200,
        "the configuration a later refusal must leave untouched is stored, got {status} ({kept})"
    );
    let untouched = reported(&kept);

    // (case, the tokenVaultId sent)
    let rows: [(&str, String); 5] = [
        ("an id at the published minimum length", "d".repeat(VAULT_ID_MIN_LENGTH)),
        ("an id at the published maximum length", "v".repeat(VAULT_ID_MAX_LENGTH)),
        ("an id that only looks like the default", "default2".to_string()),
        ("an id carrying the admitted separators", "other-token_vault".to_string()),
        ("an id in another case", DEFAULT_VAULT.to_uppercase()),
    ];

    for (case, token_vault_id) in rows {
        // (the operation, its route's answer, the wording that operation's refusal takes)
        let calls: [(&str, (u16, Value), &str); 2] = [
            (
                "SetTokenVaultCMK",
                set_cmk(
                    &router,
                    &json!({"tokenVaultId": token_vault_id, "kmsConfiguration": keeper.clone()}),
                )
                .await,
                "Failed to update token vault because it doesn't exist. Retry the request with a \
                 different resource identifier.",
            ),
            (
                "GetTokenVault",
                get_vault(&router, &json!({"tokenVaultId": token_vault_id})).await,
                "Failed to retrieve token vault because it doesn't exist. Retry the request with \
                 a different resource identifier.",
            ),
        ];

        for (operation, (status, answer), expected_message) in calls {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} ({operation} on {token_vault_id:?}): expected HTTP 404 \
                 ResourceNotFoundException, got {status} ({answer})"
            );
            assert_eq!(
                answer["message"].as_str().unwrap_or_default(),
                expected_message,
                "case {case:?} ({operation}): unexpected refusal message, got {answer}"
            );
        }

        let (status, fetched) = get_vault(&router, &json!({})).await;
        assert_eq!(
            (status, reported(&fetched)),
            (200, untouched.clone()),
            "case {case:?}: the refused set left the default vault exactly as it was, got \
             {fetched}"
        );
    }
}

/// The vault every account already holds reports the key the service manages, and reports
/// it the same way however often it is read: a read of a vault no caller has re-keyed
/// neither creates it nor re-dates it.
#[tokio::test]
async fn the_untouched_default_vault_reports_a_stable_service_managed_key() {
    let router = router();

    let (status, first) = get_vault(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "the default vault is readable before anything writes to it, got {status} ({first})"
    );
    assert_eq!(
        first["tokenVaultId"], json!(DEFAULT_VAULT),
        "a request naming no vault reads the default one, got {first}"
    );
    assert_eq!(
        first["kmsConfiguration"],
        json!({"keyType": KEY_TYPE_SERVICE_MANAGED}),
        "an untouched vault is encrypted under the key the service manages, with no key ARN of \
         the caller's, got {first}"
    );

    let (status, again) = get_vault(&router, &json!({"tokenVaultId": DEFAULT_VAULT})).await;
    assert_eq!(
        (status, reported(&again)),
        (200, reported(&first)),
        "a second read reports the same vault, unchanged by the first, got {again}"
    );
}

/// What the request members are refused for, before any vault is reached: the constraints
/// `TokenVaultIdType` and `SetTokenVaultCMKRequest` declare.
///
/// The id rows walk `len 1..64`'s bounds and one past the maximum, and the pattern
/// `^[a-zA-Z0-9\-_]+$` (`smithy_query shape=TokenVaultIdType`). An id INSIDE the
/// constraints but naming no vault is the 404 the test above pins, not a 400.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = router();

    let configuration = json!({"keyType": KEY_TYPE_SERVICE_MANAGED});

    // (case, the request body, expected status)
    let rows: [(&str, Value, u16); 7] = [
        (
            "an id one below the published minimum length",
            json!({"tokenVaultId": "", "kmsConfiguration": configuration.clone()}),
            400,
        ),
        (
            "an id at the published minimum length",
            json!({
                "tokenVaultId": "d".repeat(VAULT_ID_MIN_LENGTH),
                "kmsConfiguration": configuration.clone(),
            }),
            404,
        ),
        (
            "an id at the published maximum length",
            json!({
                "tokenVaultId": "v".repeat(VAULT_ID_MAX_LENGTH),
                "kmsConfiguration": configuration.clone(),
            }),
            404,
        ),
        (
            "an id one past the published maximum length",
            json!({
                "tokenVaultId": "v".repeat(VAULT_ID_MAX_LENGTH + 1),
                "kmsConfiguration": configuration.clone(),
            }),
            400,
        ),
        (
            "an id outside the admitted characters",
            json!({"tokenVaultId": "not a vault!", "kmsConfiguration": configuration.clone()}),
            400,
        ),
        (
            "no kmsConfiguration at all",
            json!({"tokenVaultId": DEFAULT_VAULT}),
            400,
        ),
        (
            "a key ARN outside KmsKeyArn's pattern",
            json!({
                "tokenVaultId": DEFAULT_VAULT,
                "kmsConfiguration": {
                    "keyType": KEY_TYPE_CUSTOMER_MANAGED,
                    "kmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/too-short",
                },
            }),
            400,
        ),
    ];

    for (case, body, expected_status) in rows {
        let (status, answer) = set_cmk(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answer})"
        );
        let expected_code = if expected_status == 400 {
            "ValidationException"
        } else {
            "ResourceNotFoundException"
        };
        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answer})",
            error_code(&answer)
        );
    }
}

/// Two sets of one vault in flight together are ordered, not interleaved: each answers
/// with the configuration it was given, and the vault a later read reports is exactly one
/// writer's answer — the same configuration AND the same `lastModifiedDate`, so neither a
/// merge of the two nor a timestamp belonging to the losing write can pass.
#[tokio::test]
async fn concurrent_sets_each_answer_their_own_configuration_and_one_of_them_wins() {
    let router = router();

    let first = json!({
        "keyType": KEY_TYPE_CUSTOMER_MANAGED,
        "kmsKeyArn": key_arn("aws", "us-east-1", "11111111-1111-1111-1111-111111111111"),
    });
    let second = json!({
        "keyType": KEY_TYPE_CUSTOMER_MANAGED,
        "kmsKeyArn": key_arn("aws", "us-east-1", "22222222-2222-2222-2222-222222222222"),
    });

    let left_request = json!({"kmsConfiguration": first.clone()});
    let right_request =
        json!({"tokenVaultId": DEFAULT_VAULT, "kmsConfiguration": second.clone()});
    let (left, right) = tokio::join!(
        set_cmk(&router, &left_request),
        set_cmk(&router, &right_request),
    );

    for (case, (status, answer), sent) in [
        ("the writer that named no vault", &left, &first),
        ("the writer that named the vault", &right, &second),
    ] {
        assert_eq!(
            (*status, answer["kmsConfiguration"].clone()),
            (200, sent.clone()),
            "case {case:?}: a concurrent set is answered with the configuration it sent, got \
             {status} ({answer})"
        );
    }

    let (status, fetched) = get_vault(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "the vault is readable after both sets, got {status} ({fetched})"
    );
    let winner = [&left, &right]
        .into_iter()
        .find(|(_, answer)| answer["kmsConfiguration"] == fetched["kmsConfiguration"])
        .map(|(_, answer)| reported(answer));
    assert_eq!(
        winner,
        Some(reported(&fetched)),
        "expected the vault to hold exactly one writer's answer, whole — one of {:?} or {:?} \
         with that writer's own lastModifiedDate, got {fetched}",
        reported(&left.1),
        reported(&right.1)
    );
}
