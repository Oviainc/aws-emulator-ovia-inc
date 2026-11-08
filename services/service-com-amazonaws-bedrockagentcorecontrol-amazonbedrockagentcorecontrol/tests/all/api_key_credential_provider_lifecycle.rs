// SPDX-License-Identifier: CC0-1.0

//! `CreateApiKeyCredentialProvider` and the read that reports what it persisted.
//!
//! The rule under test: a served create records ONE API key credential provider
//! of this account's token vault under the `name` the request chose — minting its
//! `acps` token-vault ARN and reporting the Secrets Manager secret the API key
//! lives in — and every later read answers from that one record. Where the secret
//! comes from is the provider's own `apiKeySecretSource`:
//!
//! * `MANAGED`, the source a request naming none asks for, puts the caller's
//!   `apiKey` into a secret of the caller's account under a name derived from the
//!   vault and the provider, as the JSON document a secret's value is, and reports
//!   that secret's ARN with the JSON key the document carries the key under;
//! * `EXTERNAL`, which the request's `apiKeySecretConfig` asks for, resolves the
//!   secret the caller already holds and reports its ARN and the `jsonKey` the key
//!   is extracted with, writing nothing into it.
//!
//! Either way the ARN the create answers with names a secret Secrets Manager
//! really holds, which is why every suite here mounts the emulator's own Secrets
//! Manager beside the control plane on ONE engine — what the assembled emulator
//! does (`core/router`'s `build_router` registers every service crate) — and reads
//! the secret through that service's own front door (`GetSecretValue`).
//!
//! The rows span what the rule generalises over: the secret source (named and
//! implied), the provider's name (the shortest and the longest
//! `CredentialProviderName` admits, and the other characters its pattern admits),
//! the size of the API key (down to one character and up to the longest
//! `DefaultApiKeyType` admits), whether the request carries tags, and the Region
//! it is signed for. The recorded case is one row.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The Region the rows are signed for unless one names another.
const REGION: &str = "us-east-1";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The token vault a request naming none addresses, which the provider's ARN
/// nests it under.
const VAULT: &str = "default";

/// The longest name `CredentialProviderName` admits: length 1..128, pattern
/// `^[a-zA-Z0-9\-_]+$` (`smithy_query(query="shape",
/// shape="CredentialProviderName")`).
const MAX_PROVIDER_NAME_LEN: usize = 128;

/// The longest API key `DefaultApiKeyType` admits: length ..65536
/// (`smithy_query(query="operation", operation="CreateApiKeyCredentialProvider")`).
/// A managed secret carries the key inside a JSON document, and Secrets Manager
/// holds at most 65536 characters of secret value (`smithy_query(query="shape",
/// shape="SecretStringType",
/// model_path="services/service-com-amazonaws-secretsmanager-secretsmanager/\
/// smithy-model/com-amazonaws-secretsmanager-secretsmanager.json")`), so the
/// longest key a managed provider can be created with is that bound less the
/// document's own `{"apiKey":""}` characters.
const MAX_API_KEY_LEN: usize = 65_536;
const MANAGED_DOCUMENT_OVERHEAD: usize = 13;

/// The name the secret holding a managed provider's key is created under, and the
/// JSON key that secret's value carries the key under. Spelled out here rather
/// than read off the implementation, so a drift in either is a failing test.
fn managed_secret_name(provider: &str) -> String {
    format!("bedrock-agentcore-identity/{VAULT}/apikey/{provider}")
}
const MANAGED_SECRET_JSON_KEY: &str = "apiKey";

/// The ARN a provider named `name` carries in `region`.
fn provider_arn(region: &str, name: &str) -> String {
    format!("arn:aws:acps:{region}:{ACCOUNT}:token-vault/{VAULT}/apikeycredentialprovider/{name}")
}

/// The prefix the ARN of a secret named `name` carries in `region`: a created
/// secret's ARN ends in six characters that belong to the secret rather than to
/// its name.
fn secret_arn_prefix(region: &str, name: &str) -> String {
    format!("arn:aws:secretsmanager:{region}:{ACCOUNT}:secret:{name}-")
}

/// The emulator the rows run against: the control plane, and Secrets Manager
/// beside it on the one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator with Secrets Manager answering, which is what the assembled
    /// emulator is.
    fn with_secrets_manager() -> Self {
        Self::over_engine(true)
    }

    /// An emulator with ONLY the control plane mounted: there is no Secrets
    /// Manager to hold an API key.
    fn without_secrets_manager() -> Self {
        Self::over_engine(false)
    }

    fn over_engine(secrets_manager: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and
        // the handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        if secrets_manager {
            router.register_service(
                service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        Self { router }
    }

    /// One `restJson1` request to the control plane, signed for `region` — which is
    /// what the router routes on and what the handlers read the Region from.
    async fn control(&self, uri: &str, region: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header("content-type", "application/json")
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    async fn create_provider(&self, region: &str, body: &Value) -> Answer {
        self.control("/identities/CreateApiKeyCredentialProvider", region, body)
            .await
    }

    async fn get_provider(&self, region: &str, name: &str) -> Answer {
        self.control(
            "/identities/GetApiKeyCredentialProvider",
            region,
            &json!({"name": name}),
        )
        .await
    }

    /// One Secrets Manager call (`awsJson1_1`, `secretsmanager.<Operation>`) signed
    /// for `region`. The credential scope names `secretsmanager`, which is what the
    /// router claims on.
    async fn secrets(&self, region: &str, operation: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/secretsmanager/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://secretsmanager.{region}.amazonaws.com/"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{operation}"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// The string value Secrets Manager holds for `secret_id`.
    async fn secret_string(&self, case: &str, region: &str, secret_id: &str) -> String {
        let answer = self
            .secrets(region, "GetSecretValue", &json!({"SecretId": secret_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetSecretValue for {secret_id:?}, got {} and \
             body {}",
            answer.status, answer.body
        );
        answer.body["SecretString"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a SecretString for {secret_id:?}, got body {}",
                    answer.body
                )
            })
            .to_string()
    }

    /// Create a secret of the CALLER's own, as an external provider references one,
    /// and report its ARN.
    async fn put_external_secret(
        &self,
        case: &str,
        region: &str,
        name: &str,
        value: &str,
    ) -> String {
        let answer = self
            .secrets(
                region,
                "CreateSecret",
                // A create stakes the secret's initial version, which a raw HTTP
                // request has to name: `ClientRequestToken` is 32..64 characters
                // (`smithy_query(query="operation", operation="CreateSecret",
                // model_path="services/service-com-amazonaws-secretsmanager-\
                // secretsmanager/smithy-model/\
                // com-amazonaws-secretsmanager-secretsmanager.json")`).
                &json!({
                    "Name": name,
                    "SecretString": value,
                    "ClientRequestToken": format!("{:0>32}", name.replace('/', "-")),
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSecret for {name:?}, got {} and body {}",
            answer.status, answer.body
        );
        answer.body["ARN"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a created secret reports its ARN, got body {}",
                    answer.body
                )
            })
            .to_string()
    }

    /// Every secret name Secrets Manager holds in `region`.
    async fn secret_names(&self, case: &str, region: &str) -> Vec<String> {
        let answer = self.secrets(region, "ListSecrets", &json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from ListSecrets, got {} and body {}",
            answer.status, answer.body
        );
        let mut names: Vec<String> = answer.body["SecretList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|secret| secret["Name"].as_str().map(str::to_string))
            .collect();
        names.sort();
        names
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` and `awsJson1_1` both carry in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Where a row's provider keeps its API key.
#[derive(Debug, Clone)]
enum Source {
    /// The service manages the secret: the row's `apiKey` is what goes into it.
    /// `named` is whether the request spells `apiKeySecretSource` out or leaves it
    /// to the rule.
    Managed { named: bool },
    /// The caller manages the secret: it is created before the provider is, under
    /// `secret_name`, holding `secret_value`, and the key is extracted with
    /// `json_key`. `named` is whether the request spells the source out or lets the
    /// reference imply it.
    External {
        secret_name: &'static str,
        secret_value: &'static str,
        json_key: &'static str,
        named: bool,
    },
}

impl Source {
    /// The `apiKeySecretSource` the provider ends up with.
    fn effective(&self) -> &'static str {
        match self {
            Self::Managed { .. } => "MANAGED",
            Self::External { .. } => "EXTERNAL",
        }
    }
}

/// One provider a row creates.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    name: String,
    api_key: String,
    source: Source,
    tags: bool,
    region: &'static str,
}

/// The providers the rule is asserted over: one row per thing it generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest name the model admits, with the source left to the rule",
            name: "a".to_string(),
            api_key: "k".to_string(),
            source: Source::Managed { named: false },
            tags: false,
            region: REGION,
        },
        Row {
            case: "the hyphen and underscore the name pattern admits, source named MANAGED",
            name: "partner-api_key_2026".to_string(),
            api_key: "sk-live-9f8e7d6c5b4a".to_string(),
            source: Source::Managed { named: true },
            tags: true,
            region: REGION,
        },
        Row {
            case: "the longest name the model admits, in a Region that is not the default",
            name: format!("L{}", "o".repeat(MAX_PROVIDER_NAME_LEN - 1)),
            api_key: "x".repeat(4_096),
            source: Source::Managed { named: true },
            tags: false,
            region: "eu-west-2",
        },
        Row {
            case: "the longest API key a managed secret's document can carry",
            name: "long_key_provider".to_string(),
            api_key: "y".repeat(MAX_API_KEY_LEN - MANAGED_DOCUMENT_OVERHEAD),
            source: Source::Managed { named: false },
            tags: true,
            region: REGION,
        },
        Row {
            case: "an external secret, with the source named EXTERNAL",
            name: "external-provider".to_string(),
            api_key: String::new(),
            source: Source::External {
                secret_name: "partner/api/credentials",
                secret_value: r#"{"apiKey":"caller-owned-key"}"#,
                json_key: "apiKey",
                named: true,
            },
            tags: true,
            region: REGION,
        },
        Row {
            case: "an external secret implied by the reference alone",
            name: "implied_external".to_string(),
            api_key: String::new(),
            source: Source::External {
                secret_name: "partner.two@credentials",
                secret_value: r#"{"token":"second-caller-owned-key"}"#,
                json_key: "token",
                named: false,
            },
            tags: false,
            region: "ap-southeast-2",
        },
    ]
}

/// The tags a row carrying them sends.
fn row_tags() -> Value {
    json!({"owner": "platform", "environment": "test"})
}

/// The create request for `row`, with the external secret (if any) already in
/// Secrets Manager — whose ARN is reported back so the row can assert the provider
/// reports exactly it.
async fn create_request(emulator: &Emulator, row: &Row) -> (Value, Option<String>) {
    let mut body = json!({"name": row.name});
    let mut external_arn = None;
    match &row.source {
        Source::Managed { named } => {
            body["apiKey"] = json!(row.api_key);
            if *named {
                body["apiKeySecretSource"] = json!("MANAGED");
            }
        }
        Source::External {
            secret_name,
            secret_value,
            json_key,
            named,
        } => {
            let arn = emulator
                .put_external_secret(row.case, row.region, secret_name, secret_value)
                .await;
            body["apiKeySecretConfig"] = json!({"secretId": secret_name, "jsonKey": json_key});
            if *named {
                body["apiKeySecretSource"] = json!("EXTERNAL");
            }
            external_arn = Some(arn);
        }
    }
    if row.tags {
        body["tags"] = row_tags();
    }
    (body, external_arn)
}

/// Assert the five members a create and a read both report describe `row`'s
/// provider, and report the secret ARN they named.
fn assert_provider_members(
    case: &str,
    body: &Value,
    row: &Row,
    external_arn: Option<&str>,
) -> String {
    assert_eq!(
        body["name"],
        json!(row.name),
        "case {case:?}: expected the provider's own name, got body {body}"
    );
    assert_eq!(
        body["credentialProviderArn"],
        json!(provider_arn(row.region, &row.name)),
        "case {case:?}: expected the provider's token-vault ARN, got body {body}"
    );
    assert_eq!(
        body["apiKeySecretSource"],
        json!(row.source.effective()),
        "case {case:?}: expected the effective secret source, got body {body}"
    );
    let secret_arn = body["apiKeySecretArn"]["secretArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an apiKeySecretArn, got body {body}"))
        .to_string();
    match (&row.source, external_arn) {
        (Source::Managed { .. }, _) => {
            let prefix = secret_arn_prefix(row.region, &managed_secret_name(&row.name));
            assert!(
                secret_arn.starts_with(&prefix) && secret_arn.len() > prefix.len(),
                "case {case:?}: a managed provider's key lives in the secret derived for it, \
                 expected an ARN under {prefix:?}, got {secret_arn:?}"
            );
            assert_eq!(
                body["apiKeySecretJsonKey"],
                json!(MANAGED_SECRET_JSON_KEY),
                "case {case:?}: a managed secret reports the JSON key its value carries the API \
                 key under, got body {body}"
            );
        }
        (Source::External { json_key, .. }, Some(arn)) => {
            assert_eq!(
                secret_arn, arn,
                "case {case:?}: an external provider reports the ARN of the secret it \
                 references, got {secret_arn:?}"
            );
            assert_eq!(
                body["apiKeySecretJsonKey"],
                json!(json_key),
                "case {case:?}: expected the caller's jsonKey, got body {body}"
            );
        }
        (Source::External { .. }, None) => {
            panic!("case {case:?}: an external row must have created its own secret")
        }
    }
    secret_arn
}

/// The API key a managed secret's value carries, under the JSON key the provider
/// reports it is extracted with.
fn managed_key(case: &str, stored: &str) -> String {
    let document: Value = serde_json::from_str(stored).unwrap_or_else(|error| {
        panic!("case {case:?}: a managed secret's value is a JSON document, got {error}")
    });
    document[MANAGED_SECRET_JSON_KEY]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the API key under {MANAGED_SECRET_JSON_KEY:?} in the \
                 managed secret"
            )
        })
        .to_string()
}

/// A served create records the provider, puts a managed key into Secrets Manager
/// (and leaves an external one's secret alone), and the read reports exactly what
/// the create answered.
#[tokio::test]
async fn a_created_provider_reads_back_with_its_secret() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_secrets_manager();
        let (request, external_arn) = create_request(&emulator, &row).await;

        let created = emulator.create_provider(row.region, &request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateApiKeyCredentialProvider, got {} and \
             body {}",
            created.status, created.body
        );
        let secret_arn =
            assert_provider_members(case, &created.body, &row, external_arn.as_deref());

        // The ARN the create answered with names a secret Secrets Manager really
        // holds, carrying the key the provider is for.
        let stored = emulator.secret_string(case, row.region, &secret_arn).await;
        match &row.source {
            Source::Managed { .. } => {
                let held = managed_key(case, &stored);
                assert_eq!(
                    held.len(),
                    row.api_key.len(),
                    "case {case:?}: expected the stored key to be {} characters, got {}",
                    row.api_key.len(),
                    held.len()
                );
                assert!(
                    held == row.api_key,
                    "case {case:?}: expected the secret to carry the provider's own key under \
                     {MANAGED_SECRET_JSON_KEY:?}, got a different value"
                );
            }
            Source::External { secret_value, .. } => {
                assert_eq!(
                    stored, *secret_value,
                    "case {case:?}: an external provider's secret is left exactly as the caller \
                     stored it"
                );
            }
        }

        // The round trip: the read reports the one record the create persisted.
        let read = emulator.get_provider(row.region, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetApiKeyCredentialProvider, got {} and body \
             {}",
            read.status, read.body
        );
        let read_secret_arn =
            assert_provider_members(case, &read.body, &row, external_arn.as_deref());
        assert_eq!(
            read_secret_arn, secret_arn,
            "case {case:?}: the read must report the secret the create named"
        );
        // A provider nothing has updated was created and last updated at the one
        // instant, and both are reported.
        let created_time = read.body["createdTime"].as_f64().unwrap_or_default();
        assert!(
            created_time > 0.0,
            "case {case:?}: expected a createdTime, got body {}",
            read.body
        );
        assert_eq!(
            read.body["lastUpdatedTime"], read.body["createdTime"],
            "case {case:?}: a provider nothing has updated reports one instant for both, got \
             body {}",
            read.body
        );
    }
}

/// The name is the identifier, so a second create under a name this account
/// already holds is a conflict: the first provider — and its one secret — stand.
#[tokio::test]
async fn a_repeated_name_is_refused_and_changes_nothing() {
    let case = "a name already held by a provider";
    let emulator = Emulator::with_secrets_manager();
    let name = "duplicate_provider";
    let first = emulator
        .create_provider(
            REGION,
            &json!({"name": name, "apiKey": "first-key", "apiKeySecretSource": "MANAGED"}),
        )
        .await;
    assert_eq!(
        first.status, 201,
        "case {case:?}: expected HTTP 201 from the first create, got {} and body {}",
        first.status, first.body
    );
    let secret_arn = first.body["apiKeySecretArn"]["secretArn"]
        .as_str()
        .expect("a served create reports its secret ARN")
        .to_string();

    let again = emulator
        .create_provider(REGION, &json!({"name": name, "apiKey": "second-key"}))
        .await;
    assert_eq!(
        (again.status, again.error_code()),
        (409, "ConflictException".to_string()),
        "case {case:?}: a repeated provider name is a conflict, got {} and body {}",
        again.status,
        again.body
    );

    // The refused create replaced nothing: the key in the secret is still the
    // first one's, and the account holds exactly the one managed secret.
    let stored = emulator.secret_string(case, REGION, &secret_arn).await;
    let held = managed_key(case, &stored);
    assert_eq!(
        held, "first-key",
        "case {case:?}: the refused create may not replace the stored key, got {held:?}"
    );
    let names = emulator.secret_names(case, REGION).await;
    assert_eq!(
        names,
        vec![managed_secret_name(name)],
        "case {case:?}: exactly one secret is held for one provider, got {names:?}"
    );
}

/// The provider a request names has to exist: a read of a name this account holds
/// no provider under is the modelled 404.
#[tokio::test]
async fn a_read_of_an_unknown_name_is_not_found() {
    let emulator = Emulator::with_secrets_manager();
    for (case, name) in [
        ("nothing was ever created", "absent_provider"),
        ("a name differing only in case from one that exists", "Stored_Provider"),
    ] {
        let created = emulator
            .create_provider(
                REGION,
                &json!({"name": "stored_provider", "apiKey": "stored-key"}),
            )
            .await;
        assert!(
            created.status == 201 || created.status == 409,
            "case {case:?}: the stored provider must be there, got {} and body {}",
            created.status,
            created.body
        );
        let read = emulator.get_provider(REGION, name).await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the modelled 404 for {name:?}, got {} and body {}",
            read.status,
            read.body
        );
        // The refusal names the resource type and the name the request addressed.
        assert_eq!(
            read.message(),
            format!("ApiKeyCredentialProvider not found for {name}"),
            "case {case:?}: unexpected refusal wording for {name:?}"
        );
    }
}

/// An external provider referencing a secret nobody holds is refused, and nothing
/// is recorded for it: the ARN its answer would carry has to name a secret Secrets
/// Manager really holds.
#[tokio::test]
async fn an_external_secret_that_is_not_there_is_refused() {
    let emulator = Emulator::with_secrets_manager();
    for (case, secret_id) in [
        ("a secret named by name", "no/such/secret"),
        (
            "a secret named by ARN",
            "arn:aws:secretsmanager:us-east-1:000000000000:secret:no-such-secret-aaaaaa",
        ),
    ] {
        let name = "external_without_secret";
        let created = emulator
            .create_provider(
                REGION,
                &json!({
                    "name": name,
                    "apiKeySecretSource": "EXTERNAL",
                    "apiKeySecretConfig": {"secretId": secret_id, "jsonKey": "apiKey"},
                }),
            )
            .await;
        assert_eq!(
            (created.status, created.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected the modelled 404 for {secret_id:?}, got {} and body {}",
            created.status,
            created.body
        );
        let read = emulator.get_provider(REGION, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create records no provider, got {} and body {}",
            read.status, read.body
        );
    }
}

/// The two rules the model states in prose: an external provider needs the
/// reference it is external to, and a managed one needs the key the service stores.
#[tokio::test]
async fn a_request_missing_what_its_source_needs_is_refused() {
    let emulator = Emulator::with_secrets_manager();
    for (case, member, request) in [
        (
            "EXTERNAL with no secret reference",
            "apiKeySecretConfig",
            json!({"name": "no_reference", "apiKeySecretSource": "EXTERNAL"}),
        ),
        (
            "MANAGED with no API key at all",
            "apiKey",
            json!({"name": "no_key", "apiKeySecretSource": "MANAGED"}),
        ),
        (
            "the default source with an empty API key",
            "apiKey",
            json!({"name": "empty_key", "apiKey": ""}),
        ),
    ] {
        let refused = emulator.create_provider(REGION, &request).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected the modelled 400, got {} and body {}",
            refused.status,
            refused.body
        );
        let message = refused.message();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name {member:?}, got {message:?}"
        );
        // Nothing is recorded for a refused create.
        let name = request["name"].as_str().unwrap_or_default();
        let read = emulator.get_provider(REGION, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create records no provider, got {} and body {}",
            read.status, read.body
        );
    }
}

/// With no Secrets Manager to hold or resolve the key, there is no secret ARN to
/// report, so the create reports no success and records nothing: the provider's
/// answer names a secret Secrets Manager holds or it names nothing. That holds for
/// either source — including an external reference spelled as a whole secret ARN,
/// which is a string the request carried and not an answer Secrets Manager gave.
#[tokio::test]
async fn without_secrets_manager_no_provider_is_created() {
    for (case, name, request) in [
        (
            "a managed secret with nothing to store it in",
            "unbacked_managed",
            json!({"name": "unbacked_managed", "apiKey": "orphan-key"}),
        ),
        (
            "an external reference named by name",
            "unbacked_external_name",
            json!({
                "name": "unbacked_external_name",
                "apiKeySecretSource": "EXTERNAL",
                "apiKeySecretConfig": {"secretId": "partner/credentials", "jsonKey": "apiKey"},
            }),
        ),
        (
            "an external reference named by ARN",
            "unbacked_external_arn",
            json!({
                "name": "unbacked_external_arn",
                "apiKeySecretConfig": {
                    "secretId":
                        "arn:aws:secretsmanager:us-east-1:000000000000:secret:partner-abc123",
                    "jsonKey": "apiKey",
                },
            }),
        ),
    ] {
        let emulator = Emulator::without_secrets_manager();
        let created = emulator.create_provider(REGION, &request).await;
        assert_eq!(
            (created.status, created.error_code()),
            (500, "InternalServerException".to_string()),
            "case {case:?}: a provider whose secret cannot be resolved is not created, got {} \
             and body {}",
            created.status,
            created.body
        );
        let read = emulator.get_provider(REGION, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: nothing may be recorded for it, got {} and body {}",
            read.status, read.body
        );
    }
}
