// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for who may CHANGE a secret another Amazon Web Services service
//! manages, driven over the wire through the router and read back through
//! `DescribeSecret`, `GetSecretValue` and `ListSecretVersionIds`.
//!
//! THE RULE: a secret that reports an `OwningService` is managed by that service, so only
//! that service changes it. "The secret is managed by another service, and you must use
//! that service to update it" is one of the documented causes of `InvalidRequestException`
//! (this service's Smithy model, which links the user guide's page on such secrets), so
//! every operation that changes a secret's value, its versions, its configuration or its
//! existence refuses a caller's call on an owned secret with `InvalidRequestException` at
//! HTTP 400 and leaves the secret exactly as it was — while the same call on a secret the
//! account owns, and the owning service's own call on the owned one, are served.
//!
//! The tables span what the rule generalises over: WHICH service owns the secret (including
//! one this emulator publishes no display name for), which operation makes the change and
//! which members it carries, and whether the request is a caller's or the owning service's
//! own. A request is the owning service's own when it presents the credential that service
//! minted for the call and recorded itself under
//! (`service_common::service_principal_calls`) — the one thing about a request a client
//! cannot choose for itself.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id an ordinary caller's requests present: a key neither STS nor IAM holds
/// a record for, which speaks for the account itself.
const CALLER_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id the owning service's own requests present, under which that service
/// records the call it is making as its own service principal.
const OWNER_KEY_ID: &str = "EMULATORAKOWNINGSERVICE";

/// The ARN of a Lambda rotation function, as `RotationLambdaARN` carries it.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:rotate-me";

/// Where `InvalidRequestException` sends a caller for this cause: the page the modelled
/// documentation of that error links ("For more information, see Secrets managed by other
/// Amazon Web Services services", this service's Smithy model).
const SERVICE_LINKED_SECRETS_DOC: &str =
    "https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html";

/// One owning service, and the secret it manages.
struct Owned {
    case: &'static str,
    /// The id the secret reports as `OwningService`.
    service_id: &'static str,
    /// The service principal its own calls are made as.
    principal: &'static str,
    /// The name such a secret carries: the reserved `<service>!…` form.
    name: &'static str,
    /// How AWS names that service in the refusal.
    service_name: &'static str,
}

/// The rows: three services that create and manage secrets of their own, named under their
/// own reserved prefixes, plus one whose name this emulator publishes none for — which is
/// still reported by the id `DescribeSecret` publishes for it.
const OWNERS: &[Owned] = &[
    Owned {
        case: "an RDS-managed master user secret",
        service_id: "rds",
        principal: "rds.amazonaws.com",
        name: "rds!db-db-ABCDEFGHIJKLMNOPQR",
        service_name: "Amazon RDS",
    },
    Owned {
        case: "a Redshift-managed admin secret",
        service_id: "redshift",
        principal: "redshift.amazonaws.com",
        name: "redshift!cluster-emu-one",
        service_name: "Amazon Redshift",
    },
    Owned {
        case: "an EventBridge connection secret",
        service_id: "events",
        principal: "events.amazonaws.com",
        name: "events!connection/emu/one",
        service_name: "Amazon EventBridge",
    },
    Owned {
        case: "a managing service this emulator publishes no name for",
        service_id: "exampleservice",
        principal: "exampleservice.amazonaws.com",
        name: "exampleservice!resource-one",
        service_name: "exampleservice",
    },
];

/// Which sentence a refused row is reported with: AWS words a change TO the secret one way
/// and a change to whether it EXISTS another, and both name the owning service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wording {
    /// "This secret is managed by <name> (service ID: <id>), and you must use that service
    /// to update it. For more information, see <the user guide's page>."
    Change,
    /// "Operation is not allowed on secret owned by <id>".
    Lifecycle,
}

/// The sentence a refused change to `row`'s secret carries.
fn refusal(row: &Owned, wording: Wording) -> String {
    match wording {
        Wording::Change => format!(
            "This secret is managed by {} (service ID: {}), and you must use that service to \
             update it. For more information, see {SERVICE_LINKED_SECRETS_DOC}.",
            row.service_name, row.service_id
        ),
        Wording::Lifecycle => {
            format!("Operation is not allowed on secret owned by {}", row.service_id)
        }
    }
}

/// A version id of the modelled length (`SecretVersionIdType` is 32..64), built from a
/// readable seed so a failing case names which version it meant.
fn version_id(seed: &str) -> String {
    format!("{seed:0<32}")
}

/// The id of the version every secret below is created holding.
fn initial_version() -> String {
    version_id("v0")
}

/// This service mounted over its own engine, with the engine kept so a test can record the
/// call an owning service is making as its own service principal.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// A fresh emulator holding only this service.
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Record that the requests signed with [`OWNER_KEY_ID`] are `principal`'s own calls,
    /// made for the resource the secret belongs to — exactly what the managing service
    /// records for a call it makes itself.
    fn acting_as(&self, principal: &str) {
        let now = SystemTime::now();
        let account = ::engine::emulator_account_id();
        self.engine
            .state::<::service_common::service_principal_calls::ServicePrincipalCalls>()
            .lock()
            .remember(
                OWNER_KEY_ID,
                ::service_common::service_principal_calls::ServicePrincipalCall {
                    principal: principal.to_string(),
                    source_arn: format!(
                        "arn:aws:{}:{REGION}:{account}:resource/emu-one",
                        principal.split('.').next().unwrap_or_default()
                    ),
                    source_account: account.to_string(),
                    expires_at: now + Duration::from_secs(600),
                },
                now,
            );
    }

    /// Send one `awsJson1_1` request signed with `access_key_id`, and return its status and
    /// JSON body.
    async fn call_as(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/secretsmanager/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One request that must be served, with its body.
    async fn ok(&self, access_key_id: &str, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call_as(access_key_id, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
        );
        parsed
    }

    /// One request that must be refused, with its wire error code and message.
    async fn err(
        &self,
        access_key_id: &str,
        operation: &str,
        body: Value,
        case: &str,
    ) -> (u16, String, String) {
        let (status, parsed) = self.call_as(access_key_id, operation, &body).await;
        let raw = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no __type in body {parsed} from {operation}"));
        let code = raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string();
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (status, code, message)
    }

    /// What `DescribeSecret` reports about the secret, as a whole.
    async fn described(&self, secret_id: &str, case: &str) -> Value {
        self.ok(CALLER_KEY_ID, "DescribeSecret", json!({"SecretId": secret_id}), case)
            .await
    }

    /// The version ids the secret holds, deprecated ones included, sorted.
    async fn version_ids(&self, secret_id: &str, case: &str) -> Vec<String> {
        let listed = self
            .ok(
                CALLER_KEY_ID,
                "ListSecretVersionIds",
                json!({"SecretId": secret_id, "IncludeDeprecated": true}),
                case,
            )
            .await;
        let mut ids: Vec<String> = listed
            .get("Versions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no Versions in {listed}"))
            .iter()
            .map(|entry| text(entry, "VersionId", case))
            .collect();
        ids.sort();
        ids
    }

    /// The version `AWSCURRENT` resolves to, and the value it holds.
    async fn current(&self, secret_id: &str, case: &str) -> (String, String) {
        let read = self
            .ok(CALLER_KEY_ID, "GetSecretValue", json!({"SecretId": secret_id}), case)
            .await;
        (
            text(&read, "VersionId", case),
            text(&read, "SecretString", case),
        )
    }

    /// Create the secret `row`'s service manages, as that service's own call, and report its
    /// ARN. The reserved name and the owning service are what such a create carries and a
    /// caller's cannot.
    async fn given_owned_secret(&self, row: &Owned, value: &str) -> String {
        self.acting_as(row.principal);
        let created = self
            .ok(
                OWNER_KEY_ID,
                "CreateSecret",
                json!({
                    "Name": row.name,
                    "SecretString": value,
                    "ClientRequestToken": initial_version(),
                }),
                row.case,
            )
            .await;
        let described = self.described(row.name, row.case).await;
        assert_eq!(
            described.get("OwningService").and_then(Value::as_str),
            Some(row.service_id),
            "case {:?}: expected the secret to report OwningService {:?}, got {described}",
            row.case,
            row.service_id
        );
        text(&created, "ARN", row.case)
    }
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// One row of the change table: the operation under test, the members its request carries,
/// and the calls that have to be made first for that request to be a valid change of the
/// secret (made by whoever the row's change is made by).
type Change = (&'static str, Value, Wording, Vec<(&'static str, Value)>);

/// Every operation that CHANGES a secret — its value, its versions, its configuration or
/// its existence — with the members its request needs to reach the secret it names and the
/// wording AWS refuses it with. The rows differ along the operation and along which members
/// one call carries, including the two documented forms of a delete
/// ("RecoveryWindowInDays", "ForceDeleteWithoutRecovery", `DeleteSecretRequest`, this
/// service's Smithy model) and both directions of a staging label (attached to a version,
/// and removed from one).
fn changes() -> Vec<Change> {
    vec![
        (
            "PutSecretValue",
            json!({
                "SecretString": "{\"password\":\"written-by-a-caller\"}",
                "ClientRequestToken": version_id("caller-put"),
            }),
            Wording::Change,
            Vec::new(),
        ),
        (
            "PutSecretValue",
            json!({
                "SecretBinary": "AAECAwQ=",
                "ClientRequestToken": version_id("caller-put-binary"),
                "VersionStages": ["AWSCURRENT"],
            }),
            Wording::Change,
            Vec::new(),
        ),
        (
            "UpdateSecret",
            json!({"Description": "a caller's description"}),
            Wording::Change,
            Vec::new(),
        ),
        (
            "UpdateSecret",
            json!({
                "SecretString": "{\"password\":\"updated-by-a-caller\"}",
                "ClientRequestToken": version_id("caller-update"),
            }),
            Wording::Change,
            Vec::new(),
        ),
        (
            "UpdateSecret",
            json!({"KmsKeyId": "alias/a-callers-key"}),
            Wording::Change,
            Vec::new(),
        ),
        ("DeleteSecret", json!({}), Wording::Lifecycle, Vec::new()),
        (
            "DeleteSecret",
            json!({"RecoveryWindowInDays": 7}),
            Wording::Lifecycle,
            Vec::new(),
        ),
        (
            "DeleteSecret",
            json!({"ForceDeleteWithoutRecovery": true}),
            Wording::Lifecycle,
            Vec::new(),
        ),
        ("RestoreSecret", json!({}), Wording::Lifecycle, Vec::new()),
        (
            "UpdateSecretVersionStage",
            json!({"VersionStage": "AWSCURRENT", "MoveToVersionId": initial_version()}),
            Wording::Change,
            Vec::new(),
        ),
        (
            // Removing a label takes one that is attached, so the row attaches it first.
            "UpdateSecretVersionStage",
            json!({"VersionStage": "blue", "RemoveFromVersionId": initial_version()}),
            Wording::Change,
            vec![(
                "UpdateSecretVersionStage",
                json!({"VersionStage": "blue", "MoveToVersionId": initial_version()}),
            )],
        ),
        (
            "RotateSecret",
            json!({"RotationLambdaARN": LAMBDA_ARN}),
            Wording::Change,
            Vec::new(),
        ),
        (
            // A rotation is cancelled on a secret one is configured for.
            "CancelRotateSecret",
            json!({}),
            Wording::Change,
            vec![(
                "RotateSecret",
                json!({"RotationLambdaARN": LAMBDA_ARN, "RotateImmediately": false}),
            )],
        ),
    ]
}

/// The request body for a row: the operation's own members plus the `SecretId`.
fn addressed_to(members: &Value, secret_id: &str) -> Value {
    let mut body = members.clone();
    body.as_object_mut()
        .expect("every row's members are an object")
        .insert("SecretId".to_string(), json!(secret_id));
    body
}

#[tokio::test]
async fn a_callers_change_to_a_service_owned_secret_is_refused_and_changes_nothing() {
    let stored = "{\"username\":\"the-managed-user\",\"password\":\"the-managed-password\"}";
    for row in OWNERS {
        for (operation, members, wording, setup) in changes() {
            let case = format!("{} changed by a caller's {operation}", row.case);
            let emulator = Emulator::new();
            let arn = emulator.given_owned_secret(row, stored).await;
            // What the row needs in place first is put there by the OWNING service, which is
            // the only one that may.
            for (prepare, body) in &setup {
                emulator
                    .ok(OWNER_KEY_ID, prepare, addressed_to(body, row.name), &case)
                    .await;
            }
            let before = emulator.described(row.name, &case).await;

            // Both forms of the id the request may name the secret by: the refusal is about
            // the secret, not about how it was addressed.
            for secret_id in [row.name.to_string(), arn.clone()] {
                let case = format!("{case}, addressed by {secret_id:?}");
                let (status, code, message) = emulator
                    .err(
                        CALLER_KEY_ID,
                        operation,
                        addressed_to(&members, &secret_id),
                        &case,
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with message {message:?}"
                );
                assert_eq!(
                    code, "InvalidRequestException",
                    "case {case:?}: expected InvalidRequestException, got {code:?} with message \
                     {message:?}"
                );
                assert_eq!(
                    message,
                    refusal(row, wording),
                    "case {case:?}: expected the sentence AWS words this refusal with, got \
                     {message:?}"
                );
            }

            // The refusal is inert: the secret reports what it reported before, AWSCURRENT
            // still resolves to the version it held, holding the value the managing service
            // stored, and no version was minted.
            let after = emulator.described(row.name, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused {operation} to leave the secret untouched"
            );
            let (current, value) = emulator.current(row.name, &case).await;
            assert_eq!(
                current,
                initial_version(),
                "case {case:?}: expected AWSCURRENT to stay on the stored version"
            );
            assert_eq!(
                value, stored,
                "case {case:?}: expected the managed value to stand, got {value:?}"
            );
            assert_eq!(
                emulator.version_ids(row.name, &case).await,
                vec![initial_version()],
                "case {case:?}: expected the refused {operation} to mint no version"
            );
        }
    }
}

#[tokio::test]
async fn the_owning_service_changes_the_secret_it_manages() {
    // The other side of the rule: the SAME calls, made as the service that owns the secret,
    // are served — and what they stored reads back. The rows are the owning services above.
    for row in OWNERS {
        let case = format!("{} written by its owning service", row.case);
        let emulator = Emulator::new();
        emulator
            .given_owned_secret(row, "{\"password\":\"the-first-password\"}")
            .await;

        let rotated = "{\"password\":\"the-rotated-password\"}";
        let staked = version_id("owner-put");
        let put = emulator
            .ok(
                OWNER_KEY_ID,
                "PutSecretValue",
                json!({
                    "SecretId": row.name,
                    "SecretString": rotated,
                    "ClientRequestToken": staked.clone(),
                }),
                &case,
            )
            .await;
        assert_eq!(
            text(&put, "VersionId", &case),
            staked,
            "case {case:?}: expected the staked version to be the one stored, got {put}"
        );
        let (current, value) = emulator.current(row.name, &case).await;
        assert_eq!(
            current, staked,
            "case {case:?}: expected AWSCURRENT to move to the version the owner staked"
        );
        assert_eq!(
            value, rotated,
            "case {case:?}: expected the owner's value to read back, got {value:?}"
        );

        // A caller is still refused after the owner's own write, and the owner's delete is
        // served: the gate is keyed on who is asking, not on the operation.
        let (status, code, _) = emulator
            .err(
                CALLER_KEY_ID,
                "PutSecretValue",
                json!({
                    "SecretId": row.name,
                    "SecretString": "{\"password\":\"a-callers\"}",
                    "ClientRequestToken": version_id("caller-after-owner"),
                }),
                &case,
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected a caller to stay refused after the owner's write"
        );
        let deleted = emulator
            .ok(
                OWNER_KEY_ID,
                "DeleteSecret",
                json!({"SecretId": row.name, "ForceDeleteWithoutRecovery": true}),
                &case,
            )
            .await;
        assert!(
            deleted.get("DeletionDate").is_some(),
            "case {case:?}: expected the owner's delete to report a DeletionDate, got {deleted}"
        );
    }
}

#[tokio::test]
async fn a_secret_no_service_owns_takes_every_one_of_those_changes() {
    // The gate is the OWNERSHIP, not the operation: each row above, made by the same caller
    // against a secret the account owns, is served.
    for (operation, members, _, setup) in changes() {
        let case = format!("{operation} on a caller's own secret");
        let emulator = Emulator::new();
        let name = "a-callers-own-secret";
        emulator
            .ok(
                CALLER_KEY_ID,
                "CreateSecret",
                json!({
                    "Name": name,
                    "SecretString": "{\"password\":\"the-callers-own\"}",
                    "ClientRequestToken": initial_version(),
                }),
                &case,
            )
            .await;
        for (prepare, body) in &setup {
            emulator
                .ok(CALLER_KEY_ID, prepare, addressed_to(body, name), &case)
                .await;
        }
        emulator
            .ok(
                CALLER_KEY_ID,
                operation,
                addressed_to(&members, name),
                &case,
            )
            .await;
    }
}
