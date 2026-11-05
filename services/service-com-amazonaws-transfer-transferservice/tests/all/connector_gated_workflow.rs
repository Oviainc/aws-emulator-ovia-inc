// SPDX-License-Identifier: CC0-1.0

//! The connector-gated path: testing a connector against an SFTP server this emulator
//! serves, carrying a transfer out over it, and the workflow the delivered file starts.
//!
//! RULES under test, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. `TestConnection` answers for the CONNECTION to the endpoint the connector's `Url`
//!    names: an endpoint this emulator serves presents that server's own host key, which
//!    the answer reports whatever the verdict, and `Status` is `OK` with the model's
//!    success message exactly when the secret the connector names authenticates a user
//!    of that server and the connector trusts the presented host key — otherwise `ERROR`
//!    naming the first condition that stopped the connection
//!    ([`testing_a_connector_reports_the_verdict_of_the_connection_it_makes`]). The
//!    documented provisioning sequence follows from that one rule: probe, read
//!    `SftpConnectionDetails.HostKey`, pin it with `UpdateConnector`, probe again and get
//!    `OK` ([`the_documented_provisioning_sequence_brings_a_connector_into_service`]).
//! 2. An accepted outbound transfer is CARRIED OUT against that endpoint: each file is
//!    read from the storage the request named and written where it lands on the remote
//!    server — the path the request named, or the user's home directory, resolved the way
//!    that user's `HomeDirectoryType` resolves it — and the per-file result reports the
//!    outcome of that attempt once the transfer's span has elapsed, `COMPLETED` for bytes
//!    that really moved and `FAILED` with the storage's own code for a file that could
//!    not be read ([`an_outbound_transfer_lands_each_file_and_reports_what_the_attempt_did`]).
//! 3. A file that lands on a server whose `WorkflowDetails.OnUpload` names a workflow
//!    starts ONE execution of it, carrying the file's location and the session details,
//!    with a callback token per `CUSTOM` step; the token is consumed at most ONCE, and a
//!    replay of it addresses no running execution
//!    ([`an_upload_starts_the_servers_workflow_whose_callback_token_is_consumed_once`]).
//! 4. The execution records rule 3 creates are RELEASED with the workflow they belong
//!    to: nothing can address a run of a workflow that no longer exists
//!    ([`deleting_a_workflow_releases_its_executions`]).
//!
//! The rows differ along what each rule generalises over: which part of the connector's
//! configuration or of the target is at fault, the form of the remote destination (a path
//! the request names, the user's home directory, a `LOGICAL` mapping), the number of
//! files a transfer names, and whether the source object exists.
//!
//! Secrets Manager and Amazon S3 are MOUNTED beside the transfer service
//! ([`Emulator::with_connector_peers`]), because the connector data plane calls them: the
//! credentials a session authenticates with and the bytes a transfer moves are the real
//! emulated ones, over the same in-process callback the handlers use.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state as st;

use crate::harness::{
    generated_host_key, member_of, Answer, Emulator, ACCOUNT, REGION, ROLE, SETTLE_MARGIN,
};

/// `Status`'s two documented values ("Returns OK for successful test, or ERROR if the
/// test fails").
pub(crate) const STATUS_OK: &str = "OK";
const STATUS_ERROR: &str = "ERROR";

/// `TransferTableStatus` values (`smithy_query(query="shape",
/// shape="TransferTableStatus")` -> `[QUEUED, IN_PROGRESS, COMPLETED, FAILED]`).
pub(crate) const QUEUED: &str = "QUEUED";
pub(crate) const COMPLETED: &str = "COMPLETED";
pub(crate) const FAILED: &str = "FAILED";

/// `ExecutionStatus` values this suite asserts (the enum is
/// `[IN_PROGRESS, COMPLETED, EXCEPTION, HANDLING_EXCEPTION]`).
const IN_PROGRESS: &str = "IN_PROGRESS";

/// Wait out the span an accepted transfer takes before its outcome is reported,
/// whatever `src/state.rs` sets it to.
pub(crate) const PAST_TRANSFER_SPAN: Duration =
    st::CONNECTOR_TRANSFER_SPAN.saturating_add(SETTLE_MARGIN);

/// Wait out the span a server spends `STARTING` after `CreateServer`.
const PAST_START_SPAN: Duration = st::SERVER_START_SPAN.saturating_add(SETTLE_MARGIN);

/// A `Target` satisfying `CustomStepDetails.Target`'s modelled pattern
/// `^arn:[a-z-]+:lambda:.*$`: the function a custom step would invoke. No Lambda
/// service is mounted here, so the invocation reaches none — which is exactly the
/// pending step a callback is then made for, and what this suite drives.
const CUSTOM_STEP_TARGET: &str = "arn:aws:lambda:us-east-1:000000000000:function:pause";

/// The bytes the transferred file holds, so a delivered file can be compared with its
/// source rather than merely counted.
pub(crate) const FILE_BODY: &[u8] = b"col-a,col-b\n1,2\n";

/// One generated SSH key pair, as the suite hands it around: the OpenSSH private key a
/// secret holds and the public half a user is created with.
pub(crate) struct KeyPair {
    pub(crate) private: String,
    pub(crate) public: String,
}

/// A real Ed25519 key pair, produced by the OpenSSH key-format crate — so what the
/// server admits is a key the test generated, not a string either side pasted.
fn key_pair() -> KeyPair {
    let keypair = ssh_key::private::Ed25519Keypair::random(&mut ssh_key::rand_core::OsRng);
    let key = ssh_key::PrivateKey::from(keypair);
    KeyPair {
        private: key
            .to_openssh(ssh_key::LineEnding::LF)
            .expect("the generated key encodes as an OpenSSH private key")
            .to_string(),
        public: key
            .public_key()
            .to_openssh()
            .expect("the generated key encodes as an OpenSSH public key"),
    }
}

/// The public half of OpenSSH private key `material`, read back out of the material by
/// the key-format crate — the key a client connecting to that server is presented with.
fn public_half(material: &str) -> String {
    ssh_key::PrivateKey::from_openssh(material)
        .expect("the test's own key material parses as an OpenSSH private key")
        .public_key()
        .to_openssh()
        .expect("the parsed key encodes as an OpenSSH public key")
}

/// A Secrets Manager client over the emulator's own in-process callback.
fn secrets_manager(emulator: &Emulator) -> aws_sdk_secretsmanager::Client {
    let callback = emulator.callback();
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(aws_sdk_secretsmanager::config::Region::new(REGION))
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

/// An Amazon S3 client over the emulator's own in-process callback.
pub(crate) fn s3(emulator: &Emulator) -> aws_sdk_s3::Client {
    let callback = emulator.callback();
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// The SFTP endpoint of a Transfer Family server: a server is reached at
/// `<server-id>.server.transfer.<region>.amazonaws.com`.
pub(crate) fn server_endpoint(server_id: &str) -> String {
    format!("sftp://{server_id}.server.transfer.{REGION}.amazonaws.com")
}

/// One emulated target: a bucket, an `ONLINE` SFTP server whose host key the test
/// generated, and one user of it holding a key pair the test generated.
pub(crate) struct Target {
    pub(crate) emulator: Emulator,
    pub(crate) bucket: String,
    pub(crate) server_id: String,
    pub(crate) user_name: String,
    /// The public half of the server's host key — what a connection to it presents.
    pub(crate) host_key: String,
    /// The user's key pair: the private half is what a secret holds.
    pub(crate) user_key: KeyPair,
}

impl Target {
    /// Stand the target up: bucket, server (waited `ONLINE`), user.
    async fn stand_up(home_directory_type: &str, home_directory: &str, mappings: Value) -> Self {
        let emulator = Emulator::with_connector_peers();
        let bucket = "transfer-connector-bucket".to_string();
        s3(&emulator)
            .create_bucket()
            .bucket(&bucket)
            .send()
            .await
            .expect("the emulated S3 creates the bucket the transfer lands in");

        let (host_key_material, _) = generated_host_key();
        let host_key = public_half(&host_key_material);
        let server_id = emulator
            .create_server(json!({
                "Protocols": ["SFTP"],
                "HostKey": host_key_material,
            }))
            .await;
        // The server comes `ONLINE` on the clock; a connector cannot reach it before.
        tokio::time::sleep(PAST_START_SPAN).await;

        let user_key = key_pair();
        let user_name = "partner-user".to_string();
        let mut request = json!({
            "ServerId": &server_id,
            "UserName": &user_name,
            "Role": ROLE,
            "HomeDirectoryType": home_directory_type,
            "SshPublicKeyBody": &user_key.public,
        });
        if !home_directory.is_empty() {
            request["HomeDirectory"] = json!(home_directory);
        }
        if !mappings.is_null() {
            request["HomeDirectoryMappings"] = mappings;
        }
        let answer = emulator.call("CreateUser", request).await;
        assert_eq!(
            answer.status, 200,
            "CreateUser: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        Self {
            emulator,
            bucket,
            server_id,
            user_name,
            host_key,
            user_key,
        }
    }

    /// The target every rule but the `LOGICAL` row uses: the user's home directory is
    /// the bucket root, which is the `PATH` form.
    pub(crate) async fn with_home_at_bucket_root() -> Self {
        let target = Self::stand_up("PATH", "", Value::Null).await;
        let home = format!("/{}", target.bucket);
        Self::stand_up("PATH", &home, Value::Null).await
    }

    /// Store one secret holding `{"Username": ..., "PrivateKey": ...}` and answer its
    /// ARN — the document an SFTP connector's `UserSecretId` names.
    pub(crate) async fn store_secret(&self, name: &str, user_name: &str, private_key: &str) -> String {
        let value = json!({"Username": user_name, "PrivateKey": private_key}).to_string();
        secrets_manager(&self.emulator)
            .create_secret()
            .name(name)
            .secret_string(value)
            .send()
            .await
            .expect("the emulated Secrets Manager stores the connector's secret")
            .arn()
            .expect("a stored secret answers its ARN")
            .to_string()
    }

    /// Create a connector against this target's endpoint and answer its `ConnectorId`.
    pub(crate) async fn create_connector(
        &self,
        case: &str,
        sftp_config: Value,
        url: Option<&str>,
    ) -> String {
        let request = json!({
            "AccessRole": ROLE,
            "Url": url.map_or_else(|| server_endpoint(&self.server_id), str::to_string),
            "SftpConfig": sftp_config,
        });
        let answer = self.emulator.call("CreateConnector", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateConnector 200, got {} with body {}",
            answer.status, answer.body
        );
        answer
            .get("ConnectorId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorId, got {}", answer.body))
            .to_string()
    }

    /// Put one object into the bucket.
    ///
    /// `body` is anything a suite's bytes come as — a constant this crate holds, or a
    /// body one row computed for itself — so a suite that gives each file its own bytes
    /// needs no second copy of this.
    pub(crate) async fn put_object(&self, key: &str, body: impl Into<Vec<u8>>) {
        s3(&self.emulator)
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(body.into()))
            .send()
            .await
            .expect("the emulated S3 stores the source object");
    }

    /// The bytes stored at `key`, or `None` when the bucket holds no such object.
    pub(crate) async fn object(&self, key: &str) -> Option<Vec<u8>> {
        let answer = s3(&self.emulator)
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .ok()?;
        Some(answer.body.collect().await.ok()?.to_vec())
    }

    /// One `TestConnection`.
    pub(crate) async fn test_connection(&self, connector_id: &str) -> Answer {
        self.emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
            .await
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the verdict of the connection a connector makes.
// ---------------------------------------------------------------------------

/// Which part of the connection a row breaks — the thing the verdict has to decide on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Nothing is wrong: the session is established.
    None,
    /// The connector trusts no host key yet — the probe a key is pinned out of.
    NoTrustedKey,
    /// The connector trusts a host key, and not the one the endpoint presented.
    UntrustedKey,
    /// The secret cannot be read as credentials.
    Secret,
    /// The credentials the secret holds are not a user of this server's.
    Credentials,
    /// The `Url` names no endpoint this emulator serves.
    Endpoint,
}

/// RULE: `TestConnection` reports the verdict of the connection to the endpoint the
/// connector's `Url` names — the host key that endpoint presented whenever one was
/// reached, `OK` with the model's success message only for a session that authenticated
/// and trusted that key, and `ERROR` naming the first condition that stopped it.
///
/// The rows differ along WHICH part of the connection is at fault and along the form of
/// the fault (no trusted key versus another server's key, an unreadable secret versus
/// one naming an unknown user versus one holding the wrong key, an endpoint outside the
/// emulator versus a server that is not serving SFTP).
#[tokio::test]
async fn testing_a_connector_reports_the_verdict_of_the_connection_it_makes() {
    let target = Target::with_home_at_bucket_root().await;
    let secret = target
        .store_secret("aws/transfer/partner", &target.user_name, &target.user_key.private)
        .await;
    let stranger = key_pair();
    let strange_secret = target
        .store_secret("aws/transfer/stranger", "nobody-here", &stranger.private)
        .await;
    let wrong_key_secret = target
        .store_secret("aws/transfer/wrong-key", &target.user_name, &stranger.private)
        .await;
    let other_host_key = key_pair().public;
    // A private key in a PEM container this emulator does not implement (`ssh-keygen`'s
    // own container is `BEGIN OPENSSH PRIVATE KEY`): its correspondence to the user's
    // keys cannot be established either way, which is evidence of nothing about the
    // credential.
    let unreadable_pem = concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "MIIBOgIBAAJBALqYvC9mWtNEmMF8n0mT2a4nAsTBYjVMSQTOhX3bUcZ5\n",
        "-----END RSA PRIVATE KEY-----\n"
    );
    let pem_secret = target
        .store_secret("aws/transfer/pem", &target.user_name, unreadable_pem)
        .await;
    // A user of the server that holds no SSH public key at all: a service-managed
    // server has nothing to authenticate such a session with.
    let keyless_user = "keyless-user";
    let created = target
        .emulator
        .call(
            "CreateUser",
            json!({
                "ServerId": &target.server_id,
                "UserName": keyless_user,
                "Role": ROLE,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateUser for the keyless user: expected 200, got {} with body {}",
        created.status, created.body
    );
    let keyless_secret = target
        .store_secret("aws/transfer/keyless", keyless_user, &stranger.private)
        .await;

    // (case, SftpConfig, Url override, the fault the verdict must decide on)
    let rows: Vec<(&str, Value, Option<&str>, Fault)> = vec![
        (
            "the credentials and the trusted host key both match the target",
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::None,
        ),
        (
            "the host key is trusted beside another key, in second place",
            json!({"UserSecretId": &secret,
                   "TrustedHostKeys": [&other_host_key, &target.host_key]}),
            None,
            Fault::None,
        ),
        (
            "no trusted host key yet: the probe a client reads the key to pin out of",
            json!({"UserSecretId": &secret}),
            None,
            Fault::NoTrustedKey,
        ),
        (
            "the trusted host key is another server's",
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&other_host_key]}),
            None,
            Fault::UntrustedKey,
        ),
        (
            "the secret names a user the server does not hold",
            json!({"UserSecretId": &strange_secret, "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::Credentials,
        ),
        (
            "the secret holds a key that is not the user's",
            json!({"UserSecretId": &wrong_key_secret, "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::Credentials,
        ),
        (
            "the secret holds a key in a PEM container this emulator cannot read, for a \
             user of the server",
            json!({"UserSecretId": &pem_secret, "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::None,
        ),
        (
            "the secret names a user of the server that holds no SSH public key",
            json!({"UserSecretId": &keyless_secret, "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::Credentials,
        ),
        (
            "the secret named is one Secrets Manager does not hold",
            json!({"UserSecretId": "aws/transfer/never-stored",
                   "TrustedHostKeys": [&target.host_key]}),
            None,
            Fault::Secret,
        ),
        (
            "the Url names a partner host outside this emulator",
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
            Some("sftp://partner.example.com"),
            Fault::Endpoint,
        ),
        (
            "the Url names a server id this emulator never minted",
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
            Some("sftp://s-0000000000000dead.server.transfer.us-east-1.amazonaws.com"),
            Fault::Endpoint,
        ),
    ];

    let mut messages: Vec<(&str, Fault, String)> = Vec::new();
    for (case, sftp_config, url, fault) in rows {
        let connector_id = target.create_connector(case, sftp_config, url).await;
        let answer = target.test_connection(&connector_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected TestConnection 200, got {} with body {}",
            answer.status, answer.body
        );

        // The host key is presented by the host-key exchange, which happens before
        // anything else can fail — so it is reported for every row that reached the
        // server, and for none that reached no server.
        let reported_key = member_of(answer.get("SftpConnectionDetails"), "HostKey")
            .as_str()
            .map(str::to_string);
        let expected_key = (fault != Fault::Endpoint).then(|| target.host_key.clone());
        assert_eq!(
            reported_key, expected_key,
            "case {case:?}: expected SftpConnectionDetails.HostKey {expected_key:?}, got \
             {reported_key:?} in {}",
            answer.body
        );

        let expected_status = if fault == Fault::None {
            STATUS_OK
        } else {
            STATUS_ERROR
        };
        let status = answer.get("Status").as_str().unwrap_or_default().to_string();
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected Status {expected_status:?}, got {status:?} in {}",
            answer.body
        );

        let message = answer
            .get("StatusMessage")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if fault == Fault::None {
            assert_eq!(
                message,
                st::TEST_CONNECTION_SUCCESS_MESSAGE,
                "case {case:?}: expected the model's success message, got {message:?}"
            );
        } else {
            assert_ne!(
                message,
                st::TEST_CONNECTION_SUCCESS_MESSAGE,
                "case {case:?}: a failed test must not report success, got {message:?}"
            );
        }
        // No value the request carried reaches the diagnosis: it is fixed per condition.
        for value in [
            secret.as_str(),
            target.host_key.as_str(),
            other_host_key.as_str(),
            connector_id.as_str(),
        ] {
            assert!(
                !message.contains(value),
                "case {case:?}: expected no request data in StatusMessage, but it \
                 carries {value:?}: {message:?}"
            );
        }
        messages.push((case, fault, message));
    }

    // One fixed text per condition: same fault, same words; different fault, different
    // words.
    for (case, fault, message) in &messages {
        for (other_case, other_fault, other_message) in &messages {
            if fault == other_fault {
                assert_eq!(
                    message, other_message,
                    "cases {case:?} and {other_case:?} are both {fault:?}, so they must \
                     report the same diagnosis, got {message:?} and {other_message:?}"
                );
            } else {
                assert_ne!(
                    message, other_message,
                    "case {case:?} is {fault:?} and case {other_case:?} is \
                     {other_fault:?}, so they must not report the same diagnosis, got \
                     {message:?}"
                );
            }
        }
    }
}

/// RULE (the same one, walked end to end): the documented provisioning sequence brings a
/// connector into service — probe `TestConnection`, read
/// `SftpConnectionDetails.HostKey`, pin it with `UpdateConnector`, probe again and get
/// `Status` `OK`. A test changes nothing a later caller sees, so the first probe answers
/// the same thing twice and the connector a read reports is the one it found.
#[tokio::test]
async fn the_documented_provisioning_sequence_brings_a_connector_into_service() {
    let target = Target::with_home_at_bucket_root().await;
    let secret = target
        .store_secret("aws/transfer/partner", &target.user_name, &target.user_key.private)
        .await;
    let case = "a connector created without a trusted host key";
    let connector_id = target
        .create_connector(case, json!({"UserSecretId": &secret}), None)
        .await;

    // Probe: no trusted key yet, so the test fails — and reports the key to pin.
    let probe = target.test_connection(&connector_id).await;
    let again = target.test_connection(&connector_id).await;
    assert_eq!(
        (probe.status, &probe.body),
        (again.status, &again.body),
        "case {case:?}: two tests of one connector must answer the same thing, got {} {} \
         then {} {}",
        probe.status,
        probe.body,
        again.status,
        again.body
    );
    assert_eq!(
        probe.get("Status").as_str(),
        Some(STATUS_ERROR),
        "case {case:?}: expected the unpinned connector to fail its test, got {}",
        probe.body
    );
    let host_key = member_of(probe.get("SftpConnectionDetails"), "HostKey")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the probe to report a host key, got {}", probe.body)
        })
        .to_string();
    assert_eq!(
        host_key, target.host_key,
        "case {case:?}: expected the server's own host key, got {host_key:?}"
    );

    // Pin it, exactly as the documented sequence does.
    let update = target
        .emulator
        .call(
            "UpdateConnector",
            json!({
                "ConnectorId": &connector_id,
                "SftpConfig": {"UserSecretId": &secret, "TrustedHostKeys": [&host_key]},
            }),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "case {case:?}: expected UpdateConnector 200, got {} with body {}",
        update.status, update.body
    );

    let settled = target.test_connection(&connector_id).await;
    assert_eq!(
        settled.get("Status").as_str(),
        Some(STATUS_OK),
        "case {case:?}: expected the pinned connector's test to succeed, got {}",
        settled.body
    );
    assert_eq!(
        settled.get("StatusMessage").as_str(),
        Some(st::TEST_CONNECTION_SUCCESS_MESSAGE),
        "case {case:?}: expected the model's success message, got {}",
        settled.body
    );
    assert_eq!(
        member_of(settled.get("SftpConnectionDetails"), "HostKey").as_str(),
        Some(target.host_key.as_str()),
        "case {case:?}: expected the successful test to report the same host key, got {}",
        settled.body
    );
}

// ---------------------------------------------------------------------------
// Rule 2: what an accepted outbound transfer does to each file.
// ---------------------------------------------------------------------------

/// One row of the delivery table: how the destination is named and which files are sent.
struct Delivery {
    case: &'static str,
    home_directory_type: &'static str,
    /// The user's `HomeDirectory`, empty for a user created without one.
    home_directory: &'static str,
    /// The user's `HomeDirectoryMappings`, when the row creates a `LOGICAL` user.
    logical: bool,
    /// `RemoteDirectoryPath`, or `None` to let the file land in the home directory.
    remote_directory_path: Option<&'static str>,
    /// The objects to store before the transfer, by key.
    stored: &'static [&'static str],
    /// The `SendFilePaths` the transfer names, relative to the bucket.
    sent: &'static [&'static str],
    /// The per-file `(StatusCode, destination key)` the results must report, in order.
    expected: &'static [(&'static str, Option<&'static str>)],
}

/// The delivery rows: the destination forms the model documents, one and several files,
/// and a source object the storage does not hold.
const DELIVERIES: &[Delivery] = &[
    Delivery {
        case: "one file into the remote directory the request names",
        home_directory_type: "PATH",
        home_directory: "",
        logical: false,
        remote_directory_path: Some("landing"),
        stored: &["outbox/report.csv"],
        sent: &["outbox/report.csv"],
        expected: &[(COMPLETED, Some("landing/report.csv"))],
    },
    Delivery {
        case: "two files into one remote directory, in submission order",
        home_directory_type: "PATH",
        home_directory: "",
        logical: false,
        remote_directory_path: Some("landing/batch"),
        stored: &["outbox/a.csv", "outbox/b.csv"],
        sent: &["outbox/b.csv", "outbox/a.csv"],
        expected: &[
            (COMPLETED, Some("landing/batch/b.csv")),
            (COMPLETED, Some("landing/batch/a.csv")),
        ],
    },
    Delivery {
        case: "no RemoteDirectoryPath: the file lands in the user's home directory",
        home_directory_type: "PATH",
        home_directory: "home",
        logical: false,
        remote_directory_path: None,
        stored: &["outbox/report.csv"],
        sent: &["outbox/report.csv"],
        expected: &[(COMPLETED, Some("home/report.csv"))],
    },
    Delivery {
        case: "a LOGICAL user, whose mapping resolves the remote directory",
        home_directory_type: "LOGICAL",
        home_directory: "",
        logical: true,
        remote_directory_path: Some("/inbox"),
        stored: &["outbox/report.csv"],
        sent: &["outbox/report.csv"],
        expected: &[(COMPLETED, Some("logical/report.csv"))],
    },
    Delivery {
        case: "a source object the storage does not hold, beside one it does",
        home_directory_type: "PATH",
        home_directory: "",
        logical: false,
        remote_directory_path: Some("landing"),
        stored: &["outbox/present.csv"],
        sent: &["outbox/present.csv", "outbox/absent.csv"],
        expected: &[(COMPLETED, Some("landing/present.csv")), (FAILED, None)],
    },
];

/// RULE: an accepted outbound transfer is carried out against the endpoint the
/// connector names — each file read from the storage the request named and written
/// where it lands on the remote server — and its per-file result reports what that
/// attempt did, once the transfer's span has elapsed: `COMPLETED` for bytes that moved,
/// `FAILED` with the storage's own `FailureCode` for a file that could not be read, and
/// `QUEUED` before the span elapses.
#[tokio::test]
async fn an_outbound_transfer_lands_each_file_and_reports_what_the_attempt_did() {
    for row in DELIVERIES {
        let case = row.case;
        let target = row.stand_up().await;
        let secret = target
            .store_secret("aws/transfer/partner", &target.user_name, &target.user_key.private)
            .await;
        let connector_id = target
            .create_connector(
                case,
                json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
                None,
            )
            .await;
        for key in row.stored {
            target.put_object(key, FILE_BODY).await;
        }

        let mut request = json!({
            "ConnectorId": &connector_id,
            "SendFilePaths": row.sent
                .iter()
                .map(|key| format!("/{}/{key}", target.bucket))
                .collect::<Vec<String>>(),
        });
        if let Some(directory) = row.remote_directory_path {
            request["RemoteDirectoryPath"] = json!(row.remote_path(directory, &target.bucket));
        }
        let accepted = target.emulator.call("StartFileTransfer", request).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected StartFileTransfer 200, got {} with body {}",
            accepted.status, accepted.body
        );
        let transfer_id = accepted
            .get("TransferId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TransferId, got {}", accepted.body))
            .to_string();

        // Before the span elapses the transfer has not reported: every file is queued.
        let queued = results(&target, &connector_id, &transfer_id).await;
        for result in &queued {
            assert_eq!(
                member_of(result, "StatusCode").as_str(),
                Some(QUEUED),
                "case {case:?}: expected a file to be {QUEUED} before the transfer's \
                 span elapses, got {result}"
            );
        }

        tokio::time::sleep(PAST_TRANSFER_SPAN).await;
        let reported = results(&target, &connector_id, &transfer_id).await;
        let statuses: Vec<String> = reported
            .iter()
            .map(|result| {
                member_of(result, "StatusCode")
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected_statuses: Vec<String> = row
            .expected
            .iter()
            .map(|(status, _)| (*status).to_string())
            .collect();
        assert_eq!(
            statuses, expected_statuses,
            "case {case:?}: expected the per-file statuses {expected_statuses:?}, got \
             {statuses:?} in {reported:?}"
        );

        for (result, (status, destination)) in reported.iter().zip(row.expected) {
            match destination {
                // The bytes really moved: the destination holds exactly the source's.
                Some(key) => {
                    let landed = target.object(key).await;
                    assert_eq!(
                        landed.as_deref(),
                        Some(FILE_BODY),
                        "case {case:?}: expected the delivered file at {key:?} to hold \
                         the source's bytes, got {landed:?}"
                    );
                    for absent in ["FailureCode", "FailureMessage"] {
                        assert!(
                            member_of(result, absent).is_null(),
                            "case {case:?}: expected no {absent} on a {status} file, \
                             got {result}"
                        );
                    }
                }
                // The attempt failed: the code and the message are the storage's own.
                None => {
                    for named in ["FailureCode", "FailureMessage"] {
                        let value = member_of(result, named).as_str().unwrap_or_default();
                        assert!(
                            !value.is_empty(),
                            "case {case:?}: expected a {named} on a {status} file, got \
                             {result}"
                        );
                    }
                }
            }
        }
    }
}

impl Delivery {
    /// Stand up the target this row describes.
    async fn stand_up(&self) -> Target {
        let bucket = "transfer-connector-bucket";
        let home = if self.home_directory.is_empty() {
            String::new()
        } else {
            format!("/{bucket}/{}", self.home_directory)
        };
        let mappings = if self.logical {
            json!([{"Entry": "/inbox", "Target": format!("/{bucket}/logical")}])
        } else {
            Value::Null
        };
        Target::stand_up(self.home_directory_type, &home, mappings).await
    }

    /// The `RemoteDirectoryPath` this row sends: a `LOGICAL` user's path is the mapping
    /// `Entry` as written, while a `PATH` user's paths are the storage's own.
    fn remote_path(&self, directory: &str, bucket: &str) -> String {
        if self.logical {
            directory.to_string()
        } else {
            format!("/{bucket}/{directory}")
        }
    }
}

/// Every result of one accepted transfer, in submission order.
pub(crate) async fn results(target: &Target, connector_id: &str, transfer_id: &str) -> Vec<Value> {
    let answer = target
        .emulator
        .call(
            "ListFileTransferResults",
            json!({"ConnectorId": connector_id, "TransferId": transfer_id}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "expected ListFileTransferResults 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("FileTransferResults")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rule 3: the workflow a delivered file starts, and its callback token.
// ---------------------------------------------------------------------------

/// RULE: a file that lands on a server whose `WorkflowDetails.OnUpload` names a
/// workflow starts ONE execution of it, carrying the file's own location and the
/// session details of the upload, with a callback token per `CUSTOM` step; that token
/// is consumed at most once — the in-order callback is accepted and recorded, and a
/// REPLAY of it addresses no running execution, leaving `DescribeExecution`'s answer
/// exactly as it was.
///
/// The rows differ along the reported `CustomStepStatus`, which is what decides the
/// status the run then reports.
#[tokio::test]
async fn an_upload_starts_the_servers_workflow_whose_callback_token_is_consumed_once() {
    // (case, the status the callback reports, the status the run then reports)
    let rows = [
        ("the step reports SUCCESS", "SUCCESS", "COMPLETED"),
        ("the step reports FAILURE", "FAILURE", "EXCEPTION"),
    ];

    for (case, reported, settled) in rows {
        let target = Target::stand_up("PATH", "", Value::Null).await;
        let secret = target
            .store_secret("aws/transfer/partner", &target.user_name, &target.user_key.private)
            .await;
        let connector_id = target
            .create_connector(
                case,
                json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
                None,
            )
            .await;

        // The workflow the upload starts: one CUSTOM step, which is the step type that
        // hands its target a callback token.
        let workflow = target
            .emulator
            .call(
                "CreateWorkflow",
                json!({"Steps": [{
                    "Type": "CUSTOM",
                    "CustomStepDetails": {
                        "Name": "pause",
                        "Target": CUSTOM_STEP_TARGET,
                        "TimeoutSeconds": 300,
                    },
                }]}),
            )
            .await;
        assert_eq!(
            workflow.status, 200,
            "case {case:?}: expected CreateWorkflow 200, got {} with body {}",
            workflow.status, workflow.body
        );
        let workflow_id = workflow
            .get("WorkflowId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowId, got {}", workflow.body))
            .to_string();
        let execution_role = format!("arn:aws:iam::{ACCOUNT}:role/transfer-workflow");
        let attached = target
            .emulator
            .call(
                "UpdateServer",
                json!({
                    "ServerId": &target.server_id,
                    "WorkflowDetails": {"OnUpload": [
                        {"WorkflowId": &workflow_id, "ExecutionRole": &execution_role},
                    ]},
                }),
            )
            .await;
        assert_eq!(
            attached.status, 200,
            "case {case:?}: expected UpdateServer 200, got {} with body {}",
            attached.status, attached.body
        );

        // The upload: a file sent through the connector onto that server.
        target.put_object("outbox/trigger.txt", FILE_BODY).await;
        let accepted = target
            .emulator
            .call(
                "StartFileTransfer",
                json!({
                    "ConnectorId": &connector_id,
                    "SendFilePaths": [format!("/{}/outbox/trigger.txt", target.bucket)],
                    "RemoteDirectoryPath": format!("/{}/landing", target.bucket),
                }),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected StartFileTransfer 200, got {} with body {}",
            accepted.status, accepted.body
        );
        tokio::time::sleep(PAST_TRANSFER_SPAN).await;

        // ONE execution, carrying the file the run started on and the session details.
        let listed = target
            .emulator
            .call("ListExecutions", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListExecutions 200, got {} with body {}",
            listed.status, listed.body
        );
        let executions = listed.get("Executions").as_array().cloned().unwrap_or_default();
        assert_eq!(
            executions.len(),
            1,
            "case {case:?}: expected the delivered file to start one execution, got \
             {executions:?}"
        );
        let execution = &executions[0];
        let execution_id = member_of(execution, "ExecutionId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ExecutionId, got {execution}"))
            .to_string();
        assert_eq!(
            member_of(execution, "Status").as_str(),
            Some(IN_PROGRESS),
            "case {case:?}: expected a run whose custom step is pending to be \
             {IN_PROGRESS}, got {execution}"
        );
        let location = member_of(
            member_of(execution, "InitialFileLocation"),
            "S3FileLocation",
        );
        assert_eq!(
            (
                member_of(location, "Bucket").as_str(),
                member_of(location, "Key").as_str()
            ),
            (Some(target.bucket.as_str()), Some("landing/trigger.txt")),
            "case {case:?}: expected the run to start on the file that landed, got \
             {location}"
        );
        let user_details = member_of(member_of(execution, "ServiceMetadata"), "UserDetails");
        assert_eq!(
            (
                member_of(user_details, "ServerId").as_str(),
                member_of(user_details, "UserName").as_str()
            ),
            (
                Some(target.server_id.as_str()),
                Some(target.user_name.as_str())
            ),
            "case {case:?}: expected the session details of the upload, got {user_details}"
        );

        // Read back through the implemented reader: a run in progress is described.
        let described = target.describe_execution(&workflow_id, &execution_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeExecution 200 for a run in progress, got {} \
             with body {}",
            described.status, described.body
        );
        let detail = described.get("Execution").clone();
        assert_eq!(
            member_of(&detail, "ExecutionId").as_str(),
            Some(execution_id.as_str()),
            "case {case:?}: expected the described run to be the one asked for, got {detail}"
        );
        assert_eq!(
            member_of(&detail, "Status").as_str(),
            Some(IN_PROGRESS),
            "case {case:?}: expected the described run to be {IN_PROGRESS}, got {detail}"
        );
        assert_eq!(
            member_of(&detail, "ExecutionRole").as_str(),
            Some(execution_role.as_str()),
            "case {case:?}: expected the role the server's attachment names, got {detail}"
        );

        // The token the step was handed: it travels to the step's function alone, so a
        // test reads it where the record holds it.
        let tokens = pending_tokens(&target, &workflow_id, &execution_id);
        assert_eq!(
            tokens.len(),
            1,
            "case {case:?}: expected one pending custom step, got {tokens:?}"
        );
        let token = tokens[0].clone();

        // The in-order callback is accepted once and recorded.
        let callback = target
            .callback(&workflow_id, &execution_id, &token, reported)
            .await;
        assert_eq!(
            (callback.status, &callback.body),
            (200, &json!({})),
            "case {case:?}: expected the first callback to be accepted with an empty \
             200, got {} {}",
            callback.status,
            callback.body
        );
        let recorded = step_states(&target, &workflow_id, &execution_id);
        assert_eq!(
            recorded.get(&token).cloned().flatten(),
            Some(reported.to_string()),
            "case {case:?}: expected the reported status on the step the token \
             addresses, got {recorded:?}"
        );

        // What a reader answers after the run has finished, BEFORE the replay.
        let before = target.describe_execution(&workflow_id, &execution_id).await;

        // The replay: the token addresses no running execution any more.
        let replay = target
            .callback(&workflow_id, &execution_id, &token, "SUCCESS")
            .await;
        assert_eq!(
            (replay.status, replay.error_code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the replayed token to be refused 404 \
             ResourceNotFoundException, got {} with body {}",
            replay.status,
            replay.body
        );
        assert_eq!(
            member_of(&replay.body, "Message").as_str(),
            Some(st::NOT_FOUND_EXECUTION.message),
            "case {case:?}: expected the fixed execution not-found message, got {}",
            replay.body
        );

        // Nothing the replay touched: the recorded outcome and the reader's answer.
        let after = target.describe_execution(&workflow_id, &execution_id).await;
        assert_eq!(
            (before.status, &before.body),
            (after.status, &after.body),
            "case {case:?}: expected DescribeExecution to answer identically before and \
             after the replay, got {} {} then {} {}",
            before.status,
            before.body,
            after.status,
            after.body
        );
        let unchanged = step_states(&target, &workflow_id, &execution_id);
        assert_eq!(
            unchanged, recorded,
            "case {case:?}: a refused replay must change nothing, got {unchanged:?}"
        );

        // The run now reports the status its callback made due.
        let listed = target
            .emulator
            .call("ListExecutions", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_eq!(
            member_of(&listed.get("Executions").as_array().cloned().unwrap_or_default()[0],
                      "Status")
            .as_str(),
            Some(settled),
            "case {case:?}: expected the finished run to report {settled:?}, got {}",
            listed.body
        );
    }
}

impl Target {
    /// One `DescribeExecution`.
    async fn describe_execution(&self, workflow_id: &str, execution_id: &str) -> Answer {
        self.emulator
            .call(
                "DescribeExecution",
                json!({"WorkflowId": workflow_id, "ExecutionId": execution_id}),
            )
            .await
    }

    /// One `SendWorkflowStepState` callback, over the wire.
    async fn callback(
        &self,
        workflow_id: &str,
        execution_id: &str,
        token: &str,
        status: &str,
    ) -> Answer {
        self.emulator
            .call(
                "SendWorkflowStepState",
                json!({
                    "WorkflowId": workflow_id,
                    "ExecutionId": execution_id,
                    "Token": token,
                    "Status": status,
                }),
            )
            .await
    }
}

/// The outcome recorded for each custom step of one run, as the store holds it. The
/// callback token reaches the step's own function and no API answer, so this is where a
/// test reads it.
fn step_states(
    target: &Target,
    workflow_id: &str,
    execution_id: &str,
) -> BTreeMap<String, Option<String>> {
    let store = target.emulator.store();
    let guard = store.lock();
    guard
        .workflow_executions
        .get(&(workflow_id.to_string(), execution_id.to_string()))
        .map(|execution| execution.step_states.clone())
        .unwrap_or_default()
}

/// The tokens of one run's steps that have not reported yet.
fn pending_tokens(target: &Target, workflow_id: &str, execution_id: &str) -> Vec<String> {
    step_states(target, workflow_id, execution_id)
        .into_iter()
        .filter(|(_, outcome)| outcome.is_none())
        .map(|(token, _)| token)
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 4: the lifetime of the execution records rule 3 creates.
// ---------------------------------------------------------------------------

/// RULE: a workflow's executions are released with the workflow — an execution is
/// addressed by its `(WorkflowId, ExecutionId)` pair, so nothing can resolve a run of a
/// workflow that no longer exists — and no OTHER workflow's runs are touched.
///
/// The rows differ along how many runs the deleted workflow holds (one, several) and
/// along how far each run has got (a step still pending, a step that has reported).
/// The records are SEEDED here, as `crate::workflow_execution_listing` seeds its own:
/// the rule under test is what a delete releases, not what an upload starts, which
/// [`an_upload_starts_the_servers_workflow_whose_callback_token_is_consumed_once`]
/// covers.
#[tokio::test]
async fn deleting_a_workflow_releases_its_executions() {
    let emulator = Emulator::new();

    // (case, the runs to seed, as (ExecutionId, the outcome of its one custom step))
    type Run<'a> = (&'a str, Option<&'a str>);
    let rows: Vec<(&str, Vec<Run<'_>>)> = vec![
        ("one run whose step is still pending", vec![("00000001-0000-4000-8000-000000000001", None)]),
        (
            "several runs, one pending and one already reported",
            vec![
                ("00000002-0000-4000-8000-000000000002", None),
                ("00000003-0000-4000-8000-000000000003", Some("SUCCESS")),
            ],
        ),
    ];

    for (case, runs) in rows {
        let deleted = create_workflow(&emulator, case).await;
        let kept = create_workflow(&emulator, case).await;
        for (execution_id, outcome) in &runs {
            for workflow_id in [&deleted, &kept] {
                seed_execution(&emulator, workflow_id, execution_id, *outcome);
            }
        }

        let answer = emulator
            .call("DeleteWorkflow", json!({"WorkflowId": &deleted}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteWorkflow 200, got {} with body {}",
            answer.status, answer.body
        );

        let released = stored_execution_ids(&emulator, &deleted);
        assert!(
            released.is_empty(),
            "case {case:?}: expected the deleted workflow's runs to be released, got \
             {released:?}"
        );
        let held = stored_execution_ids(&emulator, &kept);
        let expected: Vec<String> = runs
            .iter()
            .map(|(execution_id, _)| (*execution_id).to_string())
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected another workflow's runs to be untouched, got {held:?}"
        );
    }
}

/// Create one workflow with a single `CUSTOM` step and answer its `WorkflowId`.
async fn create_workflow(emulator: &Emulator, case: &str) -> String {
    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({"Steps": [{
                "Type": "CUSTOM",
                "CustomStepDetails": {"Name": "pause", "Target": CUSTOM_STEP_TARGET},
            }]}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowId, got {}", answer.body))
        .to_string()
}

/// Seed one execution record with a single custom step carrying `outcome`.
fn seed_execution(
    emulator: &Emulator,
    workflow_id: &str,
    execution_id: &str,
    outcome: Option<&str>,
) {
    let store = emulator.store();
    let mut guard = store.lock();
    guard.workflow_executions.insert(
        (workflow_id.to_string(), execution_id.to_string()),
        st::WorkflowExecution {
            workflow_id: workflow_id.to_string(),
            execution_id: execution_id.to_string(),
            initial_file_location: None,
            service_metadata: None,
            step_states: [("step_one".to_string(), outcome.map(str::to_string))]
                .into_iter()
                .collect(),
        },
    );
}

/// The `ExecutionId`s the store holds for one workflow, in its own order.
fn stored_execution_ids(emulator: &Emulator, workflow_id: &str) -> Vec<String> {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .executions_of(workflow_id)
        .map(|execution| execution.execution_id.clone())
        .collect()
}
