// SPDX-License-Identifier: CC0-1.0

//! The SFTP endpoint a connector's `Url` names, resolved against this emulator, and
//! the verdict a connection to it earns.
//!
//! A Transfer Family connector connects OUT to an SFTP server. Where that server is one
//! this emulator serves — a server of this account, reached at its own endpoint host —
//! the connection is one the emulator can make: the server's host key is the key the
//! host-key exchange presents, the connector's `UserSecretId` holds the credentials the
//! session authenticates with, and `TrustedHostKeys` is what the connector checks that
//! host key against. All three are read from PERSISTED state — this service's own store
//! and the account's Secrets Manager — so the verdict is about a session that was
//! really evaluated against the named target rather than a fixed answer.
//!
//! What the verdict covers, in the order the SFTP protocol settles it:
//!
//! 1. a defect in the stored configuration alone
//!    ([`crate::state::sftp_configuration_defect`]) ends the test before any session;
//! 2. the host-key exchange comes FIRST on the wire, so the host key is reported
//!    whenever the endpoint resolves — including for the connector that trusts none
//!    yet, which is the call a client reads the key to pin out of;
//! 3. then authentication, against the named user of that server and the SSH key the
//!    secret holds;
//! 4. then the host key the connector trusts, which must be the one the server
//!    presented.
//!
//! [`probe`] is the one place that whole walk lives: `TestConnection` reports it, and
//! [`super::connector_delivery`] uses it to decide whether an accepted transfer has a
//! session to run over.

use ::std::time::Instant;

use crate::ops::types::SftpConnectorConfig;
use crate::state as st;

/// `StatusMessage` for a connector whose `Url` names no SFTP server this emulator can
/// open a connection to: no endpoint at all, an endpoint outside this emulator, or a
/// server of this account that is not serving SFTP right now (one that is not `ONLINE`,
/// or whose `Protocols` do not include SFTP). The second sentence is the model's own
/// troubleshooting text for that condition.
const SFTP_ENDPOINT_UNREACHABLE: &str = "No SFTP connection could be opened to the \
     endpoint the connector configuration names. Verify the server URL in the \
     connector configuration, and verify that the login credentials work successfully \
     outside of the connector.";

/// `StatusMessage` for a connector whose `UserSecretId` names a secret whose value
/// cannot be read, or does not hold the `{\"Username\": ..., \"PrivateKey\": ...}`
/// document an SFTP connector's secret is made of. The second sentence is the model's
/// own troubleshooting text for that condition.
const SFTP_SECRET_UNUSABLE: &str = "The secret the connector configuration names could \
     not be read as an SFTP user's credentials. Verify that your secret name aligns \
     with the one in Transfer Role permissions, and verify that the secret exists and \
     is formatted correctly.";

/// `StatusMessage` for credentials the target server did not authenticate: the secret's
/// `Username` is no user of that server, or the key it holds is not one of that user's
/// SSH public keys.
const SFTP_CREDENTIALS_REJECTED: &str = "The SFTP server did not authenticate the \
     credentials the connector's secret holds. Verify the server URL in the connector \
     configuration, and verify that the login credentials work successfully outside of \
     the connector.";

/// `StatusMessage` for a connection whose host-key exchange presented a key the
/// connector does not trust: `TrustedHostKeys` is "the public portion of the host key,
/// or keys, that are used to identify the external server to which you are connecting",
/// so a key set that does not carry the presented key identifies another server.
const SFTP_HOST_KEY_NOT_TRUSTED: &str = "The host key the SFTP server presented is not \
     one of the connector configuration's TrustedHostKeys. Verify that the trusted host \
     key in the connector configuration matches the ssh-keyscan output.";

/// What follows the first label in a Transfer Family server's own endpoint host: a
/// server is reached at `<server-id>.server.transfer.<region>.amazonaws.com`, so the
/// label in FRONT of this is the `ServerId` the endpoint belongs to.
const SERVER_ENDPOINT_PREFIX: &str = "server.transfer.";

/// The DNS suffix every endpoint host of this partition ends in.
const ENDPOINT_SUFFIX: &str = ".amazonaws.com";

/// The session one connector would run its transfers over, once a connection to the
/// endpoint its `Url` names has been established and authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The server of this account the connector connected to.
    pub server_id: String,
    /// The user of that server the session authenticated as — whose home directory
    /// every remote path of this session resolves within.
    pub user_name: String,
}

/// What a connection attempt on one connector's SFTP configuration answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The host key the endpoint presented, when one was reached: `HostKey` is "the SSH
    /// public key of the remote SFTP server ... returned during the initial connection
    /// attempt", so it is reported for every attempt that got as far as the host-key
    /// exchange, whatever the verdict below.
    pub host_key: Option<String>,
    /// `TEST_CONNECTION_STATUS_OK` or `TEST_CONNECTION_STATUS_ERROR`.
    pub status: &'static str,
    /// The `StatusMessage` that says which: the model's success sentence, or the fixed
    /// text of the first condition that stopped the connection.
    pub message: &'static str,
    /// The authenticated session — `Some` exactly when [`Self::status`] is `OK`.
    pub session: Option<Session>,
}

impl Probe {
    /// The verdict for a connection that did not get established, with the condition
    /// that stopped it.
    fn refused(host_key: Option<String>, message: &'static str) -> Self {
        Self {
            host_key,
            status: st::TEST_CONNECTION_STATUS_ERROR,
            message,
            session: None,
        }
    }
}

/// The endpoint one connector's `Url` names, as this emulator holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    server_id: String,
    /// The public half of the host key that server serves with, when it has one that
    /// encodes.
    host_key: Option<String>,
}

/// Make the connection one connector's SFTP configuration describes and report what it
/// answered.
///
/// `region` is the Region the calling request was signed for — where a secret named by
/// a bare name, rather than by an ARN, is looked up. The state lock is taken for each
/// lookup and RELEASED before the secret is read, so no other request of this service
/// waits on Secrets Manager.
pub async fn probe(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    region: &str,
    url: Option<&str>,
    config: &SftpConnectorConfig,
) -> Probe {
    // The endpoint comes first, because the host-key exchange comes first: a key is
    // reported for every attempt that reached a server, including the attempts the
    // checks below then refuse.
    let endpoint = endpoint_of(state, url);
    let host_key = endpoint.as_ref().and_then(|found| found.host_key.clone());

    // A configuration this service can already see is unusable needs no session to
    // decide it.
    if let Some(defect) = st::sftp_configuration_defect(config) {
        return Probe::refused(host_key, defect);
    }
    let Some(endpoint) = endpoint else {
        return Probe::refused(host_key, SFTP_ENDPOINT_UNREACHABLE);
    };

    // `UserSecretId` is "the identifier for the secret ... that contains the SFTP
    // user's private key, password, or both": the credentials the session offers are
    // read out of it, never taken from the request. The configuration check above has
    // already refused a connector that names no secret, so the member is here.
    let secret_id = config.user_secret_id.as_deref().unwrap_or_default();
    let Some(credentials) = read_credentials(callback, region, secret_id).await else {
        return Probe::refused(host_key, SFTP_SECRET_UNUSABLE);
    };

    let authenticated = {
        let guard = state.lock();
        authenticates(&guard, &endpoint.server_id, &credentials)
    };
    if !authenticated {
        return Probe::refused(host_key, SFTP_CREDENTIALS_REJECTED);
    }

    // The key the server presented has to be one the connector trusts, compared as
    // KEYS: two encodings of one key (a differing comment, say) are the same key, which
    // is what the OpenSSH fingerprint of each decides.
    let presented = host_key.as_deref().and_then(key_fingerprint);
    let trusts_it = config
        .trusted_host_keys
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|trusted| key_fingerprint(trusted))
        .any(|trusted| Some(&trusted) == presented.as_ref());
    if !trusts_it {
        return Probe::refused(host_key, SFTP_HOST_KEY_NOT_TRUSTED);
    }

    Probe {
        host_key,
        status: st::TEST_CONNECTION_STATUS_OK,
        message: st::TEST_CONNECTION_SUCCESS_MESSAGE,
        session: Some(Session {
            server_id: endpoint.server_id,
            user_name: credentials.user_name,
        }),
    }
}

/// The endpoint `url` names, when it names one this emulator serves AND that endpoint
/// is accepting SFTP sessions.
///
/// A server that is not `ONLINE` has no SFTP endpoint listening ("the state of the
/// server ... `ONLINE` indicates that the server can accept jobs and transfer files"),
/// and one whose `Protocols` do not include SFTP serves another protocol at it, so
/// neither is an SFTP endpoint a connector can reach. The lock is taken MUTABLY because
/// a server's state transition is owed to the clock: the same settle every reader of
/// `State` performs, which materialises only a change that was already due.
fn endpoint_of(
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    url: Option<&str>,
) -> Option<Endpoint> {
    let server_id = endpoint_server_id(url?)?;
    let mut guard = state.lock();
    let server = guard.servers.get_mut(&server_id)?;
    server.settle(Instant::now());
    let serving = server.state == st::SERVER_STATE_ONLINE
        && server
            .protocols
            .iter()
            .any(|protocol| protocol == st::PROTOCOL_SFTP);
    if !serving {
        return None;
    }
    Some(Endpoint {
        host_key: guard.active_host_key_public(&server_id),
        server_id,
    })
}

/// The `ServerId` of the Transfer Family server endpoint `url` addresses, when it
/// addresses one.
///
/// A server's endpoint host is `<server-id>.server.transfer.<region>.amazonaws.com`, so
/// the server is named by the first label of the host and the rest of the host is what
/// says the endpoint is a Transfer server's at all: a partner host that merely starts
/// with the same label names somebody else's server. The scheme, any port and any path
/// are not part of the host.
pub fn endpoint_server_id(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host.split(':').next()?.to_ascii_lowercase();
    let (label, rest) = host.split_once('.')?;
    let is_server_endpoint =
        rest.starts_with(SERVER_ENDPOINT_PREFIX) && rest.ends_with(ENDPOINT_SUFFIX);
    (is_server_endpoint && is_server_id(label)).then(|| label.to_string())
}

/// Whether `label` is a `ServerId` (`len 19..19`, pattern `^s-([0-9a-f]{17})$`).
fn is_server_id(label: &str) -> bool {
    label.len() == 19
        && label.starts_with("s-")
        && label[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The credentials an SFTP connector's secret holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// `Username`: the user of the remote server the session authenticates as.
    pub user_name: String,
    /// `PrivateKey`: the SSH private key the session offers, when the secret holds one.
    pub private_key: Option<String>,
    /// `Password`: the password the session offers, when the secret holds one.
    pub password: Option<String>,
}

/// Read the SFTP user's credentials out of the secret `secret_id` names.
///
/// The secret's value is the documented
/// `{"Username": ..., "PrivateKey": ..., "Password": ...}` document, of which the user
/// name and at least one credential are what a session can be offered. `None` when the
/// secret cannot be read at all or its value is not that document — neither is a
/// credential this service may invent one for.
async fn read_credentials(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    secret_id: &str,
) -> Option<Credentials> {
    let region = secret_region(secret_id, region);
    let answer = secrets_manager_client(callback, &region)
        .get_secret_value()
        .secret_id(secret_id)
        .send()
        .await;
    let value = match answer {
        Ok(value) => value,
        Err(error) => {
            // Visible to an operator: the test reports that the secret could not be
            // read as credentials, and this is what says which answer it got.
            ::tracing::info!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the secret an SFTP connector names could not be read"
            );
            return None;
        }
    };
    let document: ::serde_json::Value = ::serde_json::from_str(value.secret_string()?).ok()?;
    let member = |name: &str| {
        document
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    };
    let credentials = Credentials {
        user_name: member("Username")?,
        private_key: member("PrivateKey"),
        password: member("Password"),
    };
    // A document that names a user and no credential at all offers the session nothing
    // to authenticate with, so it is not a credential document either.
    (credentials.private_key.is_some() || credentials.password.is_some()).then_some(credentials)
}

/// The Region the secret `secret_id` lives in: the Region segment of its ARN, and —
/// for a secret named by its bare name, which the ARN pattern is not — the Region the
/// request that reached this code was signed for.
fn secret_region(secret_id: &str, request_region: &str) -> String {
    secret_id
        .strip_prefix("arn:")
        .and_then(|rest| rest.split(':').nth(2))
        .filter(|segment| !segment.is_empty())
        .unwrap_or(request_region)
        .to_string()
}

/// Whether the target server authenticates `credentials`.
///
/// A SERVICE_MANAGED server authenticates an SFTP user by the SSH keys that user holds,
/// so the session is admitted when the user is one of that server's and its keys are
/// what the offered credential belongs to:
///
/// * a user the server does not hold, and a user that holds no readable SSH public key
///   at all, authenticate nothing — both are definite negatives;
/// * an offered PRIVATE key the emulator can read is admitted exactly when its public
///   half is one of that user's keys, compared as KEYS (by the OpenSSH fingerprint of
///   each), so an encoding difference is not a different key;
/// * an offered credential whose correspondence to those keys this emulator cannot
///   establish — a private key in a format it does not implement, a password, which a
///   service-managed server stores nothing to compare against — is EVIDENCE OF NOTHING
///   about that credential, so the session is not refused on it; what admits it is the
///   user's own keys, which are what the server authenticates with.
fn authenticates(
    state: &st::ComAmazonawsTransferTransferserviceState,
    server_id: &str,
    credentials: &Credentials,
) -> bool {
    let Some(user) = state
        .users
        .get(&(server_id.to_string(), credentials.user_name.clone()))
    else {
        return false;
    };
    let held: Vec<String> = user
        .ssh_public_keys
        .iter()
        .filter_map(|held| key_fingerprint(&held.ssh_public_key_body))
        .collect();
    if held.is_empty() {
        return false;
    }
    match credentials
        .private_key
        .as_deref()
        .and_then(st::read_host_key)
    {
        Some(offered) => held.contains(&offered.fingerprint),
        None => true,
    }
}

/// The OpenSSH SHA256 fingerprint of an SSH PUBLIC key, when `material` is one.
///
/// The identity of a key, read by the crate that implements the OpenSSH key formats:
/// comparing fingerprints is comparing keys, where comparing the encodings would make
/// a comment or a trailing newline a different key.
fn key_fingerprint(material: &str) -> Option<String> {
    Some(
        ::ssh_key::PublicKey::from_openssh(material)
            .ok()?
            .fingerprint(::ssh_key::HashAlg::Sha256)
            .to_string(),
    )
}

/// A Secrets Manager client over the IN-PROCESS callback into this same emulator: the
/// secret an SFTP connector names is one this emulator holds, so the lookup never
/// leaves the process.
///
/// The transport is in-process, so everything a network client needs and this one
/// cannot have is disabled: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given, and
/// there is nothing behind it to retry.
fn secrets_manager_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_secretsmanager::Client {
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server endpoint host names the server in its first label; anything else names
    /// no server of this emulator.
    #[test]
    fn a_server_endpoint_url_names_its_server() {
        let server = "s-0123456789abcdef0";
        // (case, url, the ServerId it names)
        let rows: [(&str, &str, Option<&str>); 8] = [
            (
                "the endpoint host of a server",
                "sftp://s-0123456789abcdef0.server.transfer.us-west-2.amazonaws.com",
                Some(server),
            ),
            (
                "the same host with a port and a path",
                "sftp://s-0123456789abcdef0.server.transfer.eu-west-1.amazonaws.com:22/inbox",
                Some(server),
            ),
            (
                "the same host with no scheme",
                "s-0123456789abcdef0.server.transfer.us-east-1.amazonaws.com",
                Some(server),
            ),
            (
                "a partner host whose first label looks like a ServerId",
                "sftp://s-0123456789abcdef0.partner.example.com",
                None,
            ),
            ("a partner host", "sftp://partner.example.com", None),
            (
                "a host whose first label is no ServerId",
                "sftp://files.server.transfer.us-west-2.amazonaws.com",
                None,
            ),
            (
                "a ServerId with a non-hex digit",
                "sftp://s-0123456789abcdefg.server.transfer.us-west-2.amazonaws.com",
                None,
            ),
            ("no host at all", "sftp://", None),
        ];
        for (case, url, expected) in rows {
            let actual = endpoint_server_id(url);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A secret named by an ARN is looked up where the ARN says it lives; one named by
    /// its bare name, where the request was signed for.
    #[test]
    fn a_secrets_region_comes_from_its_arn() {
        let rows = [
            (
                "an ARN naming its Region",
                "arn:aws:secretsmanager:eu-central-1:000000000000:secret:sftp-ab12cd",
                "eu-central-1",
            ),
            ("a bare secret name", "aws/transfer/partner", "us-west-2"),
            (
                "an ARN with an empty Region segment",
                "arn:aws:secretsmanager::000000000000:secret:sftp-ab12cd",
                "us-west-2",
            ),
        ];
        for (case, secret_id, expected) in rows {
            let actual = secret_region(secret_id, "us-west-2");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
