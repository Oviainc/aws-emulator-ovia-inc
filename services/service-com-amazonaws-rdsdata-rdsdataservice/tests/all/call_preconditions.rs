// SPDX-License-Identifier: CC0-1.0

//! The PRECONDITIONS a Data API call is admitted by — the identity it signed as, the
//! cluster it addresses, the secret it authenticates with and the database it names —
//! driven end to end against the services that OWN those facts, mounted beside the Data
//! API on one shared engine exactly as the assembled router mounts them.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one call:
//!
//! * a statement runs only on a cluster whose Data API HTTP endpoint is turned on, and a
//!   `resourceArn` the RDS control plane holds no live cluster for has no such endpoint
//!   either: both are HTTP 400 `HttpEndpointNotEnabledException` naming the ARN the
//!   request carried ([`only_a_cluster_whose_data_api_endpoint_is_on_serves_statements`]);
//! * the four faults of one working call come back as four DISTINCT modelled classes, so
//!   a client can tell a dead cluster from a bad ARN from an unusable secret from a bad
//!   database, and no refused call writes anything
//!   ([`each_single_fault_of_one_call_has_its_own_modelled_class`]);
//! * the credentials the secret holds have to be a user of the cluster: the database
//!   refuses a login it has no user for, and the Data API passes that refusal through
//!   ([`credentials_that_are_not_the_clusters_are_refused_by_the_database`]);
//! * every operation is authorized against `rds-data:<OperationName>` on the cluster
//!   before it resolves anything, so an identity no policy grants it is refused 403
//!   ([`every_operation_is_authorized_against_the_cluster_first`]).
//!
//! Every resource a case needs is created through its OWNER's own operations — the
//! cluster with `rds:CreateDBCluster`, the secret with `secretsmanager:CreateSecret` (or
//! by RDS itself, for a cluster created with `ManageMasterUserPassword`), the identity
//! with `iam:CreateUser` / `iam:CreateAccessKey` / `iam:PutUserPolicy` — so what the Data
//! API resolves is a real resource of this emulator and not a fixture written into its
//! store.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and the Region every resource lives in.
const REGION: &str = "us-east-1";

/// The master user every cluster here is created with.
const MASTER_USER: &str = "pgadmin";

/// The database every cluster here is created with (`DBName`), which is the one a
/// statement may name.
const DATABASE: &str = "appdb";

/// The access key id standing for the account's own published credential: a key neither
/// STS nor IAM holds a record for, which is the position AWS account root credentials are
/// in and what every in-process call another service makes presents.
const ACCOUNT_KEY_ID: &str = "AKIDEMULATOR";

/// One emulator: the Data API with the owners of the cluster, the secret, the key it is
/// encrypted under and the caller's identity mounted beside it, over ONE shared engine.
struct Cloud {
    router: MultiServiceRouter,
}

impl Cloud {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Verifying signatures is the emulator's concern, not this service's; these tests
        // sign nothing, so the check is off and the credential scope is read for routing
        // and for the access key id alone.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// An Amazon RDS client over this emulator's own in-process callback.
    fn rds(&self) -> aws_sdk_rds::Client {
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone());
        let config = aws_sdk_rds::Config::builder()
            .behavior_version(aws_sdk_rds::config::BehaviorVersion::latest())
            .region(aws_sdk_rds::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://rds.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(aws_sdk_rds::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_rds::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_rds::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_rds::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_rds::Client::from_conf(config)
    }

    /// A Secrets Manager client over the same callback.
    fn secrets(&self) -> aws_sdk_secretsmanager::Client {
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone());
        let config = aws_sdk_secretsmanager::Config::builder()
            .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
            .region(aws_sdk_secretsmanager::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://secretsmanager.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_secretsmanager::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_secretsmanager::Client::from_conf(config)
    }

    /// Create a cluster the way a Data API caller does — the HTTP endpoint on or off as
    /// `http_endpoint` says, RDS managing the master user password — and answer its ARN
    /// and the ARN of the secret RDS minted for it.
    async fn create_cluster(&self, identifier: &str, http_endpoint: bool) -> (String, String) {
        let created = self
            .rds()
            .create_db_cluster()
            .db_cluster_identifier(identifier)
            .engine("aurora-postgresql")
            .master_username(MASTER_USER)
            .manage_master_user_password(true)
            .database_name(DATABASE)
            .enable_http_endpoint(http_endpoint)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "CreateDBCluster {identifier} must be served: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
        let cluster = created
            .db_cluster()
            .unwrap_or_else(|| panic!("CreateDBCluster {identifier} reports its cluster"));
        let arn = cluster
            .db_cluster_arn()
            .unwrap_or_else(|| panic!("a created cluster carries its ARN"))
            .to_string();
        let secret_arn = cluster
            .master_user_secret()
            .and_then(aws_sdk_rds::types::MasterUserSecret::secret_arn)
            .unwrap_or_else(|| panic!("a managed-password cluster reports its secret"))
            .to_string();
        (arn, secret_arn)
    }

    /// Create a secret holding `{"username", "password"}` — what a Data API caller that
    /// manages its own credentials creates — and answer its ARN.
    async fn create_secret(&self, name: &str, username: &str) -> String {
        self.create_secret_holding(
            name,
            &json!({"username": username, "password": "ThePassw0rd!"}).to_string(),
        )
        .await
    }

    /// Create a secret holding exactly `value`, whatever shape it has, and answer its ARN.
    async fn create_secret_holding(&self, name: &str, value: &str) -> String {
        self.secrets()
            .create_secret()
            .name(name)
            .secret_string(value)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "CreateSecret {name} must be served: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            })
            .arn()
            .unwrap_or_else(|| panic!("a created secret carries its ARN"))
            .to_string()
    }

    /// One Data API call, signed with `access_key_id` — what names the principal the
    /// request speaks for.
    async fn call_as(&self, access_key_id: &str, path: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://rds-data.{REGION}.amazonaws.com{path}"))
            .header("host", format!("rds-data.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/rds-data/aws4_request, \
                     SignedHeaders=host, Signature=unverified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// `ExecuteStatement` as the account's own credential.
    async fn execute(&self, body: Value) -> (u16, Value) {
        self.call_as(ACCOUNT_KEY_ID, "/Execute", body).await
    }

    /// `ExecuteStatement` that must be served.
    async fn run(&self, body: Value) -> Value {
        let (status, answered) = self.execute(body.clone()).await;
        assert_eq!(status, 200, "statement {body} failed with {answered}");
        answered
    }

    /// One awsQuery call to IAM, as the account's own credential: the status and the XML
    /// body.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
        (status, body)
    }

    /// Create an IAM user with no policies at all and an access key for it, and answer
    /// the user's ARN and that key id — a brand-new identity, which is the position IAM's
    /// implicit deny covers.
    async fn create_user(&self, user_name: &str) -> (String, String) {
        let (status, created) = self.iam("CreateUser", &[("UserName", user_name)]).await;
        assert_eq!(status, 200, "CreateUser {user_name}: got {created}");
        let arn = element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser {user_name} reports the user's ARN: {created}"));
        let (status, keyed) = self
            .iam("CreateAccessKey", &[("UserName", user_name)])
            .await;
        assert_eq!(status, 200, "CreateAccessKey {user_name}: got {keyed}");
        let access_key_id = element(&keyed, "AccessKeyId").unwrap_or_else(|| {
            panic!("CreateAccessKey {user_name} reports the key id: {keyed}")
        });
        (arn, access_key_id)
    }

    /// Put `document` on the user as its inline policy, as `iam:PutUserPolicy` does.
    async fn put_user_policy(&self, user_name: &str, document: &Value) {
        let (status, answered) = self
            .iam(
                "PutUserPolicy",
                &[
                    ("UserName", user_name),
                    ("PolicyName", "inline"),
                    ("PolicyDocument", &document.to_string()),
                ],
            )
            .await;
        assert_eq!(status, 200, "PutUserPolicy {user_name}: got {answered}");
    }
}

/// The text of the first `<name>` element of an XML body, or `None` when it carries none.
fn element(body: &str, name: &str) -> Option<String> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = body.find(&opening)? + opening.len();
    let end = body[start..].find(&closing)? + start;
    Some(body[start..end].to_string())
}

/// One form value as a client puts it in an `application/x-www-form-urlencoded` body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The wire error code restJson1 carries in `__type`.
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The message a refusal carries.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// The refusal AWS words for a cluster that has no Data API endpoint.
fn endpoint_refusal(resource_arn: &str) -> String {
    format!("HttpEndpoint is not enabled for resource {resource_arn}.")
}

/// RULE: the Data API serves a statement only against a cluster whose Data API HTTP
/// endpoint is turned on. A `resourceArn` the RDS control plane holds no LIVE cluster for
/// — one never created, one in another Region, one whose delete has been accepted — has
/// no such endpoint either, so every one of them is HTTP 400
/// `HttpEndpointNotEnabledException` naming the ARN the request carried, and no statement
/// runs.
///
/// The rows differ along WHY there is no endpoint: the setting is off, the cluster was
/// never created, the ARN names another Region, and the cluster is being deleted.
#[tokio::test]
async fn only_a_cluster_whose_data_api_endpoint_is_on_serves_statements() {
    let cloud = Cloud::new();
    let (serving, serving_secret) = cloud.create_cluster("data-api-on", true).await;
    let (disabled, disabled_secret) = cloud.create_cluster("data-api-off", false).await;
    let (deleted, deleted_secret) = cloud.create_cluster("data-api-deleted", true).await;
    cloud
        .rds()
        .delete_db_cluster()
        .db_cluster_identifier("data-api-deleted")
        .skip_final_snapshot(true)
        .send()
        .await
        .expect("DeleteDBCluster must be served");

    // The POSITIVE control: the cluster whose endpoint is on answers the statement.
    let served = cloud
        .run(json!({
            "resourceArn": serving,
            "secretArn": serving_secret,
            "database": DATABASE,
            "sql": "SELECT 1",
        }))
        .await;
    assert_eq!(
        served["records"],
        json!([[{"longValue": 1}]]),
        "case the cluster whose endpoint is on: expected the statement's result, got {served}"
    );

    // (case, the cluster the call addresses, the secret it carries)
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a cluster created with the endpoint disabled",
            disabled,
            disabled_secret,
        ),
        (
            "a cluster ARN nothing was ever created for",
            format!("arn:aws:rds:{REGION}:000000000000:cluster:never-created"),
            serving_secret.clone(),
        ),
        (
            "the serving cluster's identifier in another Region",
            "arn:aws:rds:eu-west-1:000000000000:cluster:data-api-on".to_string(),
            serving_secret.clone(),
        ),
        (
            "a cluster whose delete has been accepted",
            deleted,
            deleted_secret,
        ),
    ];
    for (case, resource_arn, secret_arn) in refused {
        let (status, body) = cloud
            .execute(json!({
                "resourceArn": resource_arn,
                "secretArn": secret_arn,
                "sql": "SELECT 1",
            }))
            .await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "HttpEndpointNotEnabledException",
            "case {case}: expected HttpEndpointNotEnabledException, got {body}"
        );
        assert_eq!(
            message(&body),
            endpoint_refusal(&resource_arn),
            "case {case}: expected the refusal to name the ARN the request carried, got {body}"
        );
    }

    // The same gate on every operation of the service, not only the two that carry a
    // statement: a transaction is opened on a cluster that serves the Data API or not at
    // all.
    let (no_endpoint, no_endpoint_secret) = cloud.create_cluster("data-api-off-2", false).await;
    for (operation, path, body) in [
        (
            "BeginTransaction",
            "/BeginTransaction",
            json!({"resourceArn": no_endpoint, "secretArn": no_endpoint_secret}),
        ),
        (
            "BatchExecuteStatement",
            "/BatchExecute",
            json!({
                "resourceArn": no_endpoint,
                "secretArn": no_endpoint_secret,
                "sql": "SELECT 1",
            }),
        ),
        (
            "CommitTransaction",
            "/CommitTransaction",
            json!({
                "resourceArn": no_endpoint,
                "secretArn": no_endpoint_secret,
                "transactionId": "0123456789abcdef0123456789abcdef",
            }),
        ),
        (
            "RollbackTransaction",
            "/RollbackTransaction",
            json!({
                "resourceArn": no_endpoint,
                "secretArn": no_endpoint_secret,
                "transactionId": "0123456789abcdef0123456789abcdef",
            }),
        ),
    ] {
        let (status, answered) = cloud.call_as(ACCOUNT_KEY_ID, path, body).await;
        assert_eq!(
            status, 400,
            "case {operation} on a cluster with no endpoint: expected HTTP 400, got {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "HttpEndpointNotEnabledException",
            "case {operation}: expected HttpEndpointNotEnabledException, got {answered}"
        );
        assert_eq!(
            message(&answered),
            endpoint_refusal(&no_endpoint),
            "case {operation}: expected the refusal to name the ARN, got {answered}"
        );
    }
}

/// RULE: each single fault of one working call comes back as its OWN modelled class, so a
/// client can tell them apart: a statement with nothing in it and an ARN that is not an
/// ARN are validation faults, a cluster with no Data API endpoint is
/// `HttpEndpointNotEnabledException`, a secret that cannot be retrieved is
/// `SecretsErrorException`, a secret that is not credentials is `InvalidSecretException`,
/// and a database the cluster does not have is `DatabaseErrorException`. The faults are
/// decided in that order — the statement's own shape before the cluster, the cluster
/// before the secret — and no refused call writes anything.
#[tokio::test]
async fn each_single_fault_of_one_call_has_its_own_modelled_class() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("one-working-call", true).await;
    let (broken, _) = cloud.create_cluster("endpoint-off", false).await;
    let unusable = format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:absent-AbCdEf");
    let not_json = cloud
        .create_secret_holding("not-a-json-secret", "just a password")
        .await;

    // The working call, which every row below breaks in exactly one way.
    cloud
        .run(json!({
            "resourceArn": cluster,
            "secretArn": secret,
            "database": DATABASE,
            "sql": "CREATE TABLE kept (id INT)",
        }))
        .await;
    cloud
        .run(json!({
            "resourceArn": cluster,
            "secretArn": secret,
            "database": DATABASE,
            "sql": "INSERT INTO kept (id) VALUES (1)",
        }))
        .await;

    let insert = "INSERT INTO kept (id) VALUES (2)";
    // (case, the members the call carries, the class it comes back as)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a statement with nothing in it",
            json!({
                "resourceArn": cluster,
                "secretArn": secret,
                "database": DATABASE,
                "sql": "",
            }),
            "ValidationException",
        ),
        (
            "a cluster ARN that is not an ARN",
            json!({
                "resourceArn": "one-working-call",
                "secretArn": secret,
                "database": DATABASE,
                "sql": insert,
            }),
            "ValidationException",
        ),
        (
            "a cluster whose Data API endpoint is off",
            json!({
                "resourceArn": broken,
                "secretArn": secret,
                "database": DATABASE,
                "sql": insert,
            }),
            "HttpEndpointNotEnabledException",
        ),
        (
            "a secret Secrets Manager holds nothing for",
            json!({
                "resourceArn": cluster,
                "secretArn": unusable,
                "database": DATABASE,
                "sql": insert,
            }),
            "SecretsErrorException",
        ),
        (
            "a secret that is not a credentials document",
            json!({
                "resourceArn": cluster,
                "secretArn": not_json,
                "database": DATABASE,
                "sql": insert,
            }),
            "InvalidSecretException",
        ),
        (
            "a database the cluster does not have",
            json!({
                "resourceArn": cluster,
                "secretArn": secret,
                "database": "no-such-database",
                "sql": insert,
            }),
            "DatabaseErrorException",
        ),
        (
            // The statement's own shape is decided FIRST, so an empty statement on a
            // broken cluster reports the statement.
            "a statement with nothing in it AND a cluster with no endpoint",
            json!({
                "resourceArn": broken,
                "secretArn": unusable,
                "database": DATABASE,
                "sql": "",
            }),
            "ValidationException",
        ),
        (
            // The cluster is decided before the secret, so a call with both faults
            // reports the cluster.
            "a cluster with no endpoint AND an unusable secret",
            json!({
                "resourceArn": broken,
                "secretArn": unusable,
                "database": DATABASE,
                "sql": insert,
            }),
            "HttpEndpointNotEnabledException",
        ),
    ];
    let mut seen: Vec<&str> = Vec::new();
    for (case, members, expected) in rows {
        let (status, body) = cloud.execute(members).await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            expected,
            "case {case}: expected {expected}, got {body}"
        );
        seen.push(expected);
    }

    // The classes the rows produced are PAIRWISE DISTINCT across the four faults of a
    // working call, which is what makes them tellable apart.
    let mut distinct: Vec<&str> = seen.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct,
        vec![
            "DatabaseErrorException",
            "HttpEndpointNotEnabledException",
            "InvalidSecretException",
            "SecretsErrorException",
            "ValidationException",
        ],
        "case the whole table: expected one class per fault, got {distinct:?}"
    );

    // Not one of the refused calls wrote anything.
    let counted = cloud
        .run(json!({
            "resourceArn": cluster,
            "secretArn": secret,
            "database": DATABASE,
            "sql": "SELECT count(*) AS rows FROM kept",
        }))
        .await;
    assert_eq!(
        counted["records"],
        json!([[{"longValue": 1}]]),
        "case after every refusal: expected only the working call's row, got {counted}"
    );
}

/// RULE: the Data API authenticates to the database with the credentials the secret
/// holds, so a secret whose user the cluster does not have is refused by the DATABASE:
/// HTTP 400 `DatabaseErrorException` carrying the engine's own access-denied diagnostic —
/// the user it tried, the address the Data API connected from, whether a password was
/// supplied, and the engine's error code and SQL state for an authentication failure.
///
/// The rows differ along WHOSE credentials the secret holds: the cluster's own master
/// user (which logs in), another user, the master user's name in another case, and a
/// secret that holds no user name at all.
#[tokio::test]
async fn credentials_that_are_not_the_clusters_are_refused_by_the_database() {
    let cloud = Cloud::new();
    let (cluster, managed) = cloud.create_cluster("login-cluster", true).await;
    let own = cloud.create_secret("own-credentials", MASTER_USER).await;
    let stranger = cloud.create_secret("other-credentials", "nobody").await;
    let other_case = cloud
        .create_secret("cased-credentials", &MASTER_USER.to_uppercase())
        .await;
    let password_only = cloud
        .create_secret_holding("password-only", &json!({"password": "ThePassw0rd!"}).to_string())
        .await;

    // (case, the secret the call carries, the refusal it is answered with)
    let rows: Vec<(&str, String, Option<String>)> = vec![
        ("the secret RDS manages for the cluster", managed, None),
        ("a secret holding the cluster's master user", own, None),
        (
            "a secret holding another user",
            stranger,
            Some(access_denied("nobody", true)),
        ),
        (
            "a secret holding the master user's name in another case",
            other_case,
            Some(access_denied(&MASTER_USER.to_uppercase(), true)),
        ),
        (
            "a secret holding a password and no user name",
            password_only,
            Some(access_denied("", true)),
        ),
    ];
    for (case, secret_arn, refusal) in rows {
        let (status, body) = cloud
            .execute(json!({
                "resourceArn": cluster,
                "secretArn": secret_arn,
                "database": DATABASE,
                "sql": "SELECT 1",
            }))
            .await;
        match refusal {
            None => {
                assert_eq!(status, 200, "case {case}: expected the statement served, got {body}");
                assert_eq!(
                    body["records"],
                    json!([[{"longValue": 1}]]),
                    "case {case}: expected the statement's result, got {body}"
                );
            }
            Some(expected) => {
                assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
                assert_eq!(
                    error_code(&body),
                    "DatabaseErrorException",
                    "case {case}: expected DatabaseErrorException, got {body}"
                );
                assert_eq!(
                    message(&body),
                    expected,
                    "case {case}: expected the engine's access-denied diagnostic, got {body}"
                );
            }
        }
    }
}

/// The engine's own diagnostic for a login it has no user for, as the Data API passes it
/// through.
fn access_denied(user: &str, with_password: bool) -> String {
    format!(
        "Access denied for user '{user}'@'127.0.0.1' (using password: {}); Error code: 1045; \
         SQLState: 28000",
        if with_password { "YES" } else { "NO" }
    )
}

/// RULE: every operation of this service evaluates the identity-based IAM policies in
/// force on the principal its credentials name against `rds-data:<OperationName>` on the
/// CLUSTER the request addressed, before it resolves the cluster or the secret and before
/// anything runs. A request no statement allows falls to IAM's implicit deny and is
/// answered HTTP 403 `AccessDeniedException` naming the principal, the action and the
/// cluster; one a matching `Allow` covers reaches the operation's own answer; an explicit
/// `Deny` refuses whatever the grants say.
///
/// The rows differ along the document in force (none at all, an allow of exactly the
/// action, an allow of the whole service, an allow scoped to another cluster, a deny
/// beside the allow) and along the operation called.
#[tokio::test]
async fn every_operation_is_authorized_against_the_cluster_first() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("authorized-cluster", true).await;
    let (user_arn, access_key_id) = cloud.create_user("data-api-caller").await;
    let action = "rds-data:ExecuteStatement";

    let allow = |resource: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
        })
    };
    // (case, the document in force — `None` for a principal carrying none, the refusal
    // clause expected or `None` for a call that is served)
    let rows: Vec<(&str, Option<Value>, Option<String>)> = vec![
        (
            "a user carrying no policy at all",
            None,
            Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
        ),
        (
            "an allow of exactly the action on the cluster",
            Some(allow(&cluster)),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "Action": "rds-data:*", "Resource": "*"}],
            })),
            None,
        ),
        (
            "an allow scoped to another cluster",
            Some(allow(&format!(
                "arn:aws:rds:{REGION}:000000000000:cluster:another-cluster"
            ))),
            Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
        ),
        (
            "an allow of another service's action",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "rds:DescribeDBClusters",
                    "Resource": "*",
                }],
            })),
            Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
        ),
        (
            "a deny beside the allow",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "rds-data:*", "Resource": "*"},
                    {"Effect": "Deny", "Action": action, "Resource": "*"},
                ],
            })),
            Some("with an explicit deny in an identity-based policy".to_string()),
        ),
    ];
    for (case, document, refusal) in rows {
        match &document {
            Some(document) => cloud.put_user_policy("data-api-caller", document).await,
            None => {
                let (status, answered) = cloud
                    .iam(
                        "DeleteUserPolicy",
                        &[("UserName", "data-api-caller"), ("PolicyName", "inline")],
                    )
                    .await;
                assert!(
                    status == 200 || status == 404,
                    "case {case}: taking the policy off must be served, got {status} {answered}"
                );
            }
        }
        let (status, body) = cloud
            .call_as(
                &access_key_id,
                "/Execute",
                json!({
                    "resourceArn": cluster,
                    "secretArn": secret,
                    "database": DATABASE,
                    "sql": "SELECT 1",
                }),
            )
            .await;
        match refusal {
            None => {
                assert_eq!(status, 200, "case {case}: expected the call served, got {body}");
                assert_eq!(
                    body["records"],
                    json!([[{"longValue": 1}]]),
                    "case {case}: expected the statement's result, got {body}"
                );
            }
            Some(reason) => {
                assert_eq!(status, 403, "case {case}: expected HTTP 403, got {status} {body}");
                assert_eq!(
                    error_code(&body),
                    "AccessDeniedException",
                    "case {case}: expected AccessDeniedException, got {body}"
                );
                assert_eq!(
                    message(&body),
                    format!(
                        "User: {user_arn} is not authorized to perform: {action} on resource: \
                         {cluster} {reason}"
                    ),
                    "case {case}: expected IAM's own wording, got {body}"
                );
            }
        }
    }

    // The same gate on every other operation of the service: the user carries no policy
    // again, so each one falls to the implicit deny and names its OWN action.
    let (status, answered) = cloud
        .iam(
            "DeleteUserPolicy",
            &[("UserName", "data-api-caller"), ("PolicyName", "inline")],
        )
        .await;
    assert!(
        status == 200 || status == 404,
        "taking the policy off must be served, got {status} {answered}"
    );
    for (operation, path, body) in [
        (
            "BatchExecuteStatement",
            "/BatchExecute",
            json!({"resourceArn": cluster, "secretArn": secret, "sql": "SELECT 1"}),
        ),
        (
            "BeginTransaction",
            "/BeginTransaction",
            json!({"resourceArn": cluster, "secretArn": secret}),
        ),
        (
            "CommitTransaction",
            "/CommitTransaction",
            json!({
                "resourceArn": cluster,
                "secretArn": secret,
                "transactionId": "0123456789abcdef0123456789abcdef",
            }),
        ),
        (
            "RollbackTransaction",
            "/RollbackTransaction",
            json!({
                "resourceArn": cluster,
                "secretArn": secret,
                "transactionId": "0123456789abcdef0123456789abcdef",
            }),
        ),
    ] {
        let (status, answered) = cloud.call_as(&access_key_id, path, body).await;
        assert_eq!(
            status, 403,
            "case {operation}: expected HTTP 403, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "AccessDeniedException",
            "case {operation}: expected AccessDeniedException, got {answered}"
        );
        let expected = format!(
            "User: {user_arn} is not authorized to perform: rds-data:{operation} on resource: \
             {cluster} because no identity-based policy allows the rds-data:{operation} action"
        );
        assert_eq!(
            message(&answered),
            expected,
            "case {operation}: expected the refusal to name its own action, got {answered}"
        );
    }

    // The account's own published credential is not a principal the IAM store holds
    // policies for, so it is served — which is also what every in-process call another
    // emulated service makes presents.
    let served = cloud
        .run(json!({
            "resourceArn": cluster,
            "secretArn": secret,
            "database": DATABASE,
            "sql": "SELECT 1",
        }))
        .await;
    assert_eq!(
        served["records"],
        json!([[{"longValue": 1}]]),
        "case the account's own credential: expected the call served, got {served}"
    );
}
