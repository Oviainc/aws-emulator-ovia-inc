// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's source-credentials slice —
//! `ImportSourceCredentials` / `ListSourceCredentials` / `DeleteSourceCredentials` —
//! driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Every assertion is about state: what an import persisted is what the list reports,
//! what a delete removed stops being reported, and a read reports the same thing twice.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    AUTH_TYPE_VALUES, SERVER_TYPE_VALUES,
};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so a token-ARN assertion proves the region came from the
/// request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDTEST";

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The token ARN this emulator mints for `server_type`, as AWS documents the CLI's
/// `--arn arn:aws:codebuild:<region>:<account>:token/github`.
fn expected_arn(server_type: &str) -> String {
    format!(
        "arn:aws:codebuild:{REGION}:{}:token/{}",
        engine::EMULATOR_ACCOUNT_ID,
        server_type.to_ascii_lowercase()
    )
}

/// `ListSourceCredentials`' entries, sorted so a comparison is about the set and not
/// about this service's map order.
async fn list_credentials(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, body) = call(router, "ListSourceCredentials", json!({})).await;
    assert_eq!(
        status, 200,
        "ListSourceCredentials: expected 200, got {status} with {body}"
    );
    let mut infos = body["sourceCredentialsInfos"]
        .as_array()
        .unwrap_or_else(|| panic!("sourceCredentialsInfos must be a list, got {body}"))
        .clone();
    infos.sort_by_key(|info| info["arn"].as_str().unwrap_or_default().to_string());
    infos
}

/// One row of the import table: the credential a client imports, and the
/// `SourceCredentialsInfo` the service must then report for it.
struct Imported {
    case: &'static str,
    server_type: &'static str,
    auth_type: &'static str,
    token: &'static str,
    username: Option<&'static str>,
}

impl Imported {
    fn request(&self) -> Value {
        let mut body = json!({
            "serverType": self.server_type,
            "authType": self.auth_type,
            "token": self.token,
        });
        if let (Value::Object(target), Some(username)) = (&mut body, self.username) {
            target.insert("username".to_string(), json!(username));
        }
        body
    }

    /// The expected report: the arn, the server type and the auth type it was imported
    /// with, plus `resource` only for the auth types whose token IS a resource ARN.
    fn expected_info(&self) -> Value {
        let mut info = json!({
            "arn": expected_arn(self.server_type),
            "serverType": self.server_type,
            "authType": self.auth_type,
        });
        if state::RESOURCE_ARN_AUTH_TYPES.contains(&self.auth_type) {
            if let Value::Object(target) = &mut info {
                target.insert("resource".to_string(), json!(self.token));
            }
        }
        info
    }
}

/// The import table: one row per `ServerType` the model declares (catalog source:
/// `smithy_query(query="shape",shape="ServerType")` — BITBUCKET, GITHUB,
/// GITHUB_ENTERPRISE, GITLAB, GITLAB_SELF_MANAGED), each with a different `AuthType`
/// the API accepts, so a row differs from its neighbours in the server type, the auth
/// type, whether a username rides along and whether a `resource` comes back.
fn import_table() -> Vec<Imported> {
    vec![
        Imported {
            case: "bitbucket with a username and an app password",
            server_type: "BITBUCKET",
            auth_type: "BASIC_AUTH",
            token: "bitbucket-app-password",
            username: Some("bitbucket-user"),
        },
        Imported {
            case: "github with a personal access token",
            server_type: "GITHUB",
            auth_type: "PERSONAL_ACCESS_TOKEN",
            token: "ghp_personal_access_token",
            username: None,
        },
        Imported {
            case: "github enterprise through a codeconnections connection",
            server_type: "GITHUB_ENTERPRISE",
            auth_type: "CODECONNECTIONS",
            token: "arn:aws:codeconnections:us-west-2:000000000000:connection/abc",
            username: None,
        },
        Imported {
            case: "gitlab through a secrets manager secret",
            server_type: "GITLAB",
            auth_type: "SECRETS_MANAGER",
            token: "arn:aws:secretsmanager:us-west-2:000000000000:secret:gitlab-xyz",
            username: None,
        },
        Imported {
            case: "gitlab self managed with a personal access token",
            server_type: "GITLAB_SELF_MANAGED",
            auth_type: "PERSONAL_ACCESS_TOKEN",
            token: "glpat_personal_access_token",
            username: None,
        },
    ]
}

/// RULE: `ListSourceCredentials` reports one entry per set of source credentials the
/// account has imported, carrying the token ARN it was minted with, the server type and
/// auth type it was imported with, and the connection/secret ARN for the auth types
/// whose token is one — and nothing else. An account that has imported nothing reports
/// an empty list.
#[tokio::test]
async fn the_list_reports_exactly_the_credentials_that_were_imported() {
    let router = emulator();

    let empty = list_credentials(&router).await;
    assert_eq!(
        empty,
        Vec::<Value>::new(),
        "before any import: expected an empty list, got {empty:?}"
    );

    let table = import_table();
    let mut expected: Vec<Value> = Vec::new();
    for row in &table {
        let (status, body) = call(&router, "ImportSourceCredentials", row.request()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            row.case
        );
        assert_eq!(
            body["arn"].as_str().unwrap_or_default(),
            expected_arn(row.server_type),
            "case {:?}: expected the minted token ARN, got {body}",
            row.case
        );

        expected.push(row.expected_info());
        expected.sort_by_key(|info| info["arn"].as_str().unwrap_or_default().to_string());
        let listed = list_credentials(&router).await;
        assert_eq!(
            listed, expected,
            "case {:?}: the list must report every credential imported so far, got {listed:?}",
            row.case
        );
    }

    // A read changes nothing a later caller sees.
    let first = list_credentials(&router).await;
    let second = list_credentials(&router).await;
    assert_eq!(
        first, second,
        "two reads in a row must agree, got {first:?} then {second:?}"
    );

    // Deleting by token ARN removes exactly that credential.
    for row in &table {
        let arn = expected_arn(row.server_type);
        let (status, body) =
            call(&router, "DeleteSourceCredentials", json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteSourceCredentials, got {status} with {body}",
            row.case
        );
        assert_eq!(
            body["arn"].as_str().unwrap_or_default(),
            arn,
            "case {:?}: the delete must echo the ARN it removed, got {body}",
            row.case
        );

        expected.retain(|info| info["arn"].as_str().unwrap_or_default() != arn);
        let listed = list_credentials(&router).await;
        assert_eq!(
            listed, expected,
            "case {:?}: the deleted credential must stop being reported, got {listed:?}",
            row.case
        );
    }
}

/// RULE: an account holds one set of credentials per server type, so importing again
/// for a server type that already has one replaces it when `shouldOverwrite` is true or
/// absent ("The default value is true") and is refused with
/// `ResourceAlreadyExistsException` when it is false, leaving the stored record as it
/// was.
#[tokio::test]
async fn a_second_import_for_one_server_type_obeys_should_overwrite() {
    // Each row re-imports over a first, BASIC_AUTH import of the same server type.
    let rows: Vec<(&str, &str, Option<bool>, bool)> = vec![
        (
            "shouldOverwrite absent, so the documented default applies",
            "GITHUB",
            None,
            true,
        ),
        ("shouldOverwrite true", "BITBUCKET", Some(true), true),
        ("shouldOverwrite false", "GITLAB", Some(false), false),
    ];

    for (case, server_type, should_overwrite, overwrites) in rows {
        let router = emulator();
        let first = Imported {
            case,
            server_type,
            auth_type: "BASIC_AUTH",
            token: "first-token",
            username: Some("first-user"),
        };
        let (status, body) = call(&router, "ImportSourceCredentials", first.request()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first import must succeed, got {status} with {body}"
        );

        let mut again = json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "second-token",
        });
        if let (Value::Object(target), Some(flag)) = (&mut again, should_overwrite) {
            target.insert("shouldOverwrite".to_string(), json!(flag));
        }
        let (status, body) = call(&router, "ImportSourceCredentials", again).await;

        if overwrites {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "ResourceAlreadyExistsException",
                "case {case:?}: expected ResourceAlreadyExistsException, got {body}"
            );
        }

        // Either way the account still holds exactly one credential for that server
        // type; what changed is only whether it is the second import's.
        let listed = list_credentials(&router).await;
        let expected_auth_type = if overwrites {
            "PERSONAL_ACCESS_TOKEN"
        } else {
            "BASIC_AUTH"
        };
        assert_eq!(
            listed,
            vec![json!({
                "arn": expected_arn(server_type),
                "serverType": server_type,
                "authType": expected_auth_type,
            })],
            "case {case:?}: expected one credential with authType {expected_auth_type}, \
             got {listed:?}"
        );
    }
}

/// RULE: an import the API cannot perform is refused with HTTP 400
/// `InvalidInputException` and persists nothing — `authType` `OAUTH` ("An OAUTH
/// connection is not supported by the API and must be created using the CodeBuild
/// console") and a `username` sent with any auth type other than `BASIC_AUTH` ("This
/// parameter is not valid for other types of source providers or connections").
#[tokio::test]
async fn an_import_the_api_cannot_perform_is_refused_and_persists_nothing() {
    let router = emulator();

    let mut refused: Vec<(String, Value)> = Vec::new();
    // The console-only auth type, over the whole `ServerType` catalog (source:
    // `smithy_query(query="shape",shape="ServerType")`, mirrored in
    // `SERVER_TYPE_VALUES`).
    for server_type in SERVER_TYPE_VALUES {
        refused.push((
            format!("OAUTH for {server_type}"),
            json!({
                "serverType": server_type,
                "authType": state::CONSOLE_ONLY_AUTH_TYPE,
                "token": "oauth-token",
            }),
        ));
    }
    // A username with every auth type the model declares except the one it is valid
    // for (source: `smithy_query(query="shape",shape="AuthType")`, mirrored in
    // `AUTH_TYPE_VALUES`).
    for auth_type in AUTH_TYPE_VALUES
        .iter()
        .filter(|auth_type| **auth_type != state::USERNAME_AUTH_TYPE)
    {
        refused.push((
            format!("a username with authType {auth_type}"),
            json!({
                "serverType": "GITHUB",
                "authType": auth_type,
                "token": "some-token",
                "username": "not-for-this-auth-type",
            }),
        ));
    }

    for (case, body) in refused {
        let (status, response) = call(&router, "ImportSourceCredentials", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_type(&response),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {response}"
        );
        let listed = list_credentials(&router).await;
        assert_eq!(
            listed,
            Vec::<Value>::new(),
            "case {case:?}: a refused import must persist nothing, got {listed:?}"
        );
    }

    // The same username, with the one auth type it is valid for, is accepted.
    let accepted = Imported {
        case: "a username with BASIC_AUTH",
        server_type: "BITBUCKET",
        auth_type: state::USERNAME_AUTH_TYPE,
        token: "app-password",
        username: Some("bitbucket-user"),
    };
    let (status, body) = call(&router, "ImportSourceCredentials", accepted.request()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {body}",
        accepted.case
    );
    assert_eq!(
        list_credentials(&router).await,
        vec![accepted.expected_info()],
        "case {:?}: the accepted import must be the account's one credential",
        accepted.case
    );
}

/// RULE: `DeleteSourceCredentials` resolves its `arn` against the stored token ARNs, so
/// an ARN naming no stored credential is refused with HTTP 400
/// `ResourceNotFoundException` and every other credential survives the refusal.
#[tokio::test]
async fn deleting_an_arn_that_names_no_stored_credential_is_refused() {
    let router = emulator();

    let held = Imported {
        case: "the one credential this account holds",
        server_type: "GITHUB",
        auth_type: "PERSONAL_ACCESS_TOKEN",
        token: "ghp_token",
        username: None,
    };
    let (status, body) = call(&router, "ImportSourceCredentials", held.request()).await;
    assert_eq!(
        status, 200,
        "the setup import must succeed, got {status} with {body}"
    );

    let unknown: Vec<(&str, String)> = vec![
        (
            "a server type that was never imported",
            expected_arn("BITBUCKET"),
        ),
        (
            "the right server type in another region",
            expected_arn("GITHUB").replace(REGION, "eu-central-1"),
        ),
        (
            "an ARN of another resource kind",
            format!(
                "arn:aws:codebuild:{REGION}:{}:project/github",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("not an ARN at all", "GITHUB".to_string()),
    ];

    for (case, arn) in unknown {
        let (status, body) =
            call(&router, "DeleteSourceCredentials", json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            list_credentials(&router).await,
            vec![held.expected_info()],
            "case {case:?}: a refused delete must leave the stored credential in place"
        );
    }

    // And the ARN that does name it deletes it exactly once: the second attempt is the
    // same refusal.
    let arn = expected_arn(held.server_type);
    let (status, body) = call(
        &router,
        "DeleteSourceCredentials",
        json!({"arn": arn.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the stored credential: expected 200, got {status} with {body}"
    );
    let (status, body) = call(&router, "DeleteSourceCredentials", json!({"arn": arn})).await;
    assert_eq!(
        status, 400,
        "deleting it a second time: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_type(&body),
        "ResourceNotFoundException",
        "deleting it a second time: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        list_credentials(&router).await,
        Vec::<Value>::new(),
        "after deleting the only credential the list must be empty"
    );
}
