// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transfer-transferservice service.
//!
//! Everything AWS Transfer Family's server + user + AS2 profile + connector lifecycle
//! needs to answer a read from stored state lives here: the resource maps (servers,
//! their users, the AS2 profiles, the connectors, and the workflows a server's
//! `WorkflowDetails` attach), the
//! id-minting counter, the
//! documented create-time defaults and the clock-due server state transitions (the
//! `ServerTransition` rule `StartServer`/`StopServer` share), the ARN shapes, and the
//! opaque pagination token codec the `List*` operations share.
//!
//! The service-managed security policy catalog ([`SECURITY_POLICIES`]) lives here too.
//! No operation creates or changes a security policy — a server or connector only
//! attaches one by name — so the catalog is published data rather than per-account
//! state, and `DescribeSecurityPolicy` answers out of it.
//!
//! `UserCount` and `SshPublicKeyCount` are DERIVED at read time ([`
//! ComAmazonawsTransferTransferserviceState::user_count`],
//! [`User::ssh_public_key_count`]) rather than stored, so they cannot go stale when a
//! user or a key is added or removed. Every other member a reader reports is RESOLVED
//! once, when the resource is created or updated, so no reader has to re-apply a
//! default.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    EndpointDetails, HomeDirectoryMapEntry, IdentityProviderDetails, PosixProfile, ProtocolDetails,
    ProxyConfig, ResourceNotFoundException, S3StorageOptions, SftpPortWithOptions, Tag,
    WorkflowDetails, WorkflowStep,
};

/// The region used for an ARN when the request carries no SigV4 credential scope to
/// read one from. Matches the region the emulator's generated SDK profile uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `arn:aws:transfer:…` — this service's ARN namespace, per `OPERATIONS.md`.
const ARN_SERVICE: &str = "transfer";

/// The security policy a server gets when `CreateServer` names none.
///
/// AWS Transfer Family's current default security policy for a new server
/// ("TransferSecurityPolicy-2024-01 ... is the default security policy", AWS Transfer
/// Family User Guide, *Security policies for servers*), confirmed by the recorded
/// response of a server created without `SecurityPolicyName`. It satisfies
/// `SecurityPolicyName`'s modelled pattern
/// `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$`.
pub const DEFAULT_SECURITY_POLICY_NAME: &str = "TransferSecurityPolicy-2024-01";

/// `ProtocolDetails.PassiveIp`'s default.
///
/// Modelled doc: "The value `PassiveIp=AUTO` is assigned by default"; the recorded
/// response reports it for every server, whatever its protocols.
pub const DEFAULT_PASSIVE_IP: &str = "AUTO";

/// `ProtocolDetails.TlsSessionResumptionMode`'s documented default ("If a
/// TlsSessionResumptionMode value is not specified during CreateServer, it is set to
/// ENFORCED by default").
pub const DEFAULT_TLS_SESSION_RESUMPTION_MODE: &str = "ENFORCED";

/// `ProtocolDetails.SetStatOption`'s default: the `SetStatOption` enum's `DEFAULT`
/// member, i.e. `SETSTAT` is not ignored.
pub const DEFAULT_SET_STAT_OPTION: &str = "DEFAULT";

/// `ProtocolDetails.SftpPorts`' documented default port ("If you don't specify any
/// configurations during CreateServer, the service uses port 22 with
/// SERVER_TALK_FIRST by default").
pub const DEFAULT_SFTP_PORT: i32 = 22;

/// `SftpPortWithOptions.CommunicationMode`'s documented default — the other half of
/// the `SftpPorts` default sentence quoted on [`DEFAULT_SFTP_PORT`].
pub const DEFAULT_COMMUNICATION_MODE: &str = "SERVER_TALK_FIRST";

/// `ProtocolDetails.ProxyConfig.SftpMode`'s documented default ("If you don't specify
/// a value, the default is NONE").
pub const DEFAULT_PROXY_SFTP_MODE: &str = "NONE";

/// `S3StorageOptions.DirectoryListingOptimization`'s documented default ("If using the
/// API or CLI, this is disabled by default").
pub const DEFAULT_DIRECTORY_LISTING_OPTIMIZATION: &str = "DISABLED";

/// `Domain`'s `S3` value (the `Domain` enum is `[S3, EFS]`).
pub const DOMAIN_S3: &str = "S3";

/// `Domain`'s documented default ("The default value is S3").
pub const DEFAULT_DOMAIN: &str = DOMAIN_S3;

/// The `IdentityProviderType` enum's values (`[SERVICE_MANAGED, API_GATEWAY,
/// AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`). The three below `SERVICE_MANAGED` are the
/// ones `IdentityProviderDetails` documents itself as required for.
pub const IDENTITY_PROVIDER_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
pub const IDENTITY_PROVIDER_API_GATEWAY: &str = "API_GATEWAY";
pub const IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE: &str = "AWS_DIRECTORY_SERVICE";
pub const IDENTITY_PROVIDER_AWS_LAMBDA: &str = "AWS_LAMBDA";

/// `IdentityProviderType`'s documented default ("The default value is SERVICE_MANAGED").
pub const DEFAULT_IDENTITY_PROVIDER_TYPE: &str = IDENTITY_PROVIDER_SERVICE_MANAGED;

/// The `EndpointType` enum's two VPC-hosted values (`[PUBLIC, VPC, VPC_ENDPOINT]`).
/// A server with either one hosts its endpoint inside a VPC, so it is configured by
/// `EndpointDetails`.
pub const ENDPOINT_TYPE_VPC: &str = "VPC";
pub const ENDPOINT_TYPE_VPC_ENDPOINT: &str = "VPC_ENDPOINT";

/// `EndpointType`'s default: a server with no VPC endpoint settings is reachable over
/// the public internet.
pub const DEFAULT_ENDPOINT_TYPE: &str = "PUBLIC";

/// `IpAddressType`'s documented default ("The default value is IPV4").
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "IPV4";

/// `IpAddressType`'s dual-stack value (the `IpAddressType` enum is
/// `[IPV4, DUALSTACK]`).
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "DUALSTACK";

/// `Protocols`' default: SFTP alone, the only protocol a `PUBLIC`,
/// `SERVICE_MANAGED` server can serve.
pub const DEFAULT_PROTOCOL: &str = "SFTP";

/// The `Protocol` enum values whose documentation states a requirement on another
/// member of the server (the enum is `[SFTP, FTP, FTPS, AS2]`).
pub const PROTOCOL_FTP: &str = "FTP";
pub const PROTOCOL_FTPS: &str = "FTPS";
pub const PROTOCOL_AS2: &str = "AS2";

/// `HomeDirectoryType`'s value that makes `HomeDirectoryMappings` the user's home
/// directory (the enum is `[PATH, LOGICAL]`).
pub const HOME_DIRECTORY_TYPE_LOGICAL: &str = "LOGICAL";

/// `HomeDirectoryType` for a user whose write named none.
///
/// `HomeDirectoryType`'s documentation pairs the two values with the member each one
/// is configured through: "If HomeDirectoryType is LOGICAL, you must provide
/// mappings, using the HomeDirectoryMappings parameter. If, on the other hand,
/// HomeDirectoryType is PATH, you provide an absolute path using the HomeDirectory
/// parameter." [`home_directory_violation`] refuses every write that breaks that
/// pairing, so a write that names NO type configured a path rather than mappings —
/// a `PATH` user, which is what the recorded Transfer wire reports for a `CreateUser`
/// that named no type.
pub const DEFAULT_HOME_DIRECTORY_TYPE: &str = "PATH";

/// The `HomeDirectoryType` a user write leaves on the stored record.
///
/// The caller's value whenever it sent one, else [`DEFAULT_HOME_DIRECTORY_TYPE`].
/// Every user write resolves the type here, so the stored record carries a type for
/// every user and the readers project it without defaulting a second time.
pub fn resolved_home_directory_type(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_HOME_DIRECTORY_TYPE.to_string())
}

/// The page bound a `List*` call uses when the caller sends no `MaxResults`.
///
/// Equal to `MaxResults`' modelled maximum (`range 1..1000`), so an unbounded
/// request still cannot answer more than one modelled page.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How many servers the store holds before `CreateServer` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// AWS Transfer Family publishes an account- and Region-level quota on servers that an
/// account can have raised, and this emulator models neither Service Quotas nor any
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is far above any workflow an emulator serves,
/// and it is what stops `servers` growing without end: every record it holds is
/// released by `DeleteServer`, which also drops that server's users and their keys, so
/// the user map cannot outlive the bound either.
pub const MAX_STORED_SERVERS: usize = 200;

/// How many users ONE server holds before `CreateUser` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound, not
/// an account's quota. Together the two cap the whole store at
/// `MAX_STORED_SERVERS * MAX_USERS_PER_SERVER` users, and each stored record is itself
/// bounded by the modelled lengths of the members it holds (`HomeDirectoryMappings`
/// `len 1..50000`, `Policy` `len 0..2048`, `SshPublicKeyBody` `len 0..2048`, `Tags`
/// `len 1..50`), which the framework's boundary validator enforces before a handler
/// runs. `DeleteUser` releases one record; `DeleteServer` releases all of a server's.
pub const MAX_USERS_PER_SERVER: usize = 1_000;

/// How many connectors the store holds before a connector create refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota. It is what stops `connectors`
/// growing without end: every write consults
/// [`ComAmazonawsTransferTransferserviceState::has_connector_capacity`] before it
/// inserts, and `DeleteConnector` releases a record again. Each stored record is
/// itself bounded by the modelled lengths of the members it holds (`Url`
/// `len 0..255`), which the framework's boundary validator enforces before a handler
/// runs, so the bound caps the map in bytes as well as in count.
pub const MAX_STORED_CONNECTORS: usize = 200;

/// How many AS2 profiles the store holds before `CreateProfile` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota, since this emulator models
/// neither Service Quotas nor an account's adjustments. Each stored record is itself
/// bounded by the modelled lengths of the members it holds (`As2Id` `len 1..128`,
/// `CertificateIds` entries `len 22..22`, `Tags` `len 1..50`), which the framework's
/// boundary validator enforces before a handler runs, and `DeleteProfile` releases a
/// record.
pub const MAX_STORED_PROFILES: usize = 200;

/// How long after `CreateServer` or `StartServer` a server leaves `STARTING` for
/// `ONLINE`.
///
/// AWS documents the transition ("it can take a couple of minutes for the server to
/// be completely operational") but not a span the caller can rely on, so this span is
/// CHOSEN: long enough that a caller acting straight after the call still observes
/// `STARTING`, short enough not to stall a test. The due instant is stored
/// and materialised on the next read; nothing sleeps for it.
const SERVER_START_SPAN: Duration = Duration::from_millis(500);

/// How long after `StopServer` a server leaves `STOPPING` for `OFFLINE`.
///
/// AWS documents the intermediate `STOPPING` state but no span, so this is CHOSEN on
/// the same basis as [`SERVER_START_SPAN`]: a caller polling straight after
/// `StopServer` still observes `STOPPING`, and the `OFFLINE` it waits for comes due
/// shortly after without anything sleeping.
const SERVER_STOP_SPAN: Duration = Duration::from_millis(500);

/// A server's `State` right after `CreateServer`: instantiated, not yet able to serve
/// file operations.
pub const SERVER_STATE_STARTING: &str = "STARTING";

/// A server's `State` once the start the create began has completed.
pub const SERVER_STATE_ONLINE: &str = "ONLINE";

/// A server's `State` right after `StopServer`: the intermediate state the model
/// documents as "not fully able to respond, or not fully offline".
pub const SERVER_STATE_STOPPING: &str = "STOPPING";

/// A server's `State` once the stop has completed: the server exists but cannot
/// perform file transfer operations.
pub const SERVER_STATE_OFFLINE: &str = "OFFLINE";

/// A server's `State` when a start failed: the model documents `START_FAILED` as "an
/// error condition", which a caller answers by starting the server again. Nothing in
/// this emulator writes it — a start it begins always completes — but the shared
/// transition rule names it so a retry from it would begin a start rather than be
/// refused.
pub const SERVER_STATE_START_FAILED: &str = "START_FAILED";

/// A server's `State` when a stop failed. The counterpart of
/// [`SERVER_STATE_START_FAILED`], on the same basis.
pub const SERVER_STATE_STOP_FAILED: &str = "STOP_FAILED";

/// The HTTP status AWS Transfer Family answers `ResourceNotFoundException` with.
///
/// The model puts `@httpError(404)` on the shape, but the recorded Transfer wire
/// answers an unknown server or user with HTTP **400** and `__type`
/// `ResourceNotFoundException` — so the status on the wire is 400 and the shape's
/// `httpError` does not describe the service. Handlers return it through
/// [`not_found`], which keeps the body the model's own and changes only the status
/// line.
pub const RESOURCE_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// `ResourceType` for the server resource: the resource name capitalized, as the
/// recorded Transfer error bodies carry it.
pub const RESOURCE_TYPE_SERVER: &str = "Server";

/// `ResourceType` for the user resource. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_USER: &str = "User";

/// `ResourceType` for one of a user's SSH public keys, addressed by its
/// `SshPublicKeyId`. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_SSH_PUBLIC_KEY: &str = "SshPublicKey";

/// `ResourceType` for the AS2 profile resource, addressed by its `ProfileId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_PROFILE: &str = "Profile";

/// `ResourceType` for the workflow resource, addressed by its `WorkflowId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_WORKFLOW: &str = "Workflow";

/// The `WorkflowStepType` enum's values (`[COPY, CUSTOM, TAG, DELETE, DECRYPT]`).
/// Each one names the ONE `WorkflowStep.*StepDetails` member that configures it, which
/// is what [`workflow_step_violation`] enforces.
pub const WORKFLOW_STEP_TYPE_COPY: &str = "COPY";
pub const WORKFLOW_STEP_TYPE_CUSTOM: &str = "CUSTOM";
pub const WORKFLOW_STEP_TYPE_TAG: &str = "TAG";
pub const WORKFLOW_STEP_TYPE_DELETE: &str = "DELETE";
pub const WORKFLOW_STEP_TYPE_DECRYPT: &str = "DECRYPT";

/// `SourceFileLocation`'s documented default, on every step-details shape that carries
/// the member ("To use the previous file as the input, enter `${previous.file}`. In
/// this case, this workflow step uses the output file from the previous workflow step
/// as input. This is the default value").
pub const DEFAULT_SOURCE_FILE_LOCATION: &str = "${previous.file}";

/// `OverwriteExisting`'s documented default, on every step-details shape that carries
/// the member ("A flag that indicates whether to overwrite an existing file of the same
/// name. The default is `FALSE`"). It is the `OverwriteExisting` enum's `FALSE` value
/// (the enum is `[FALSE, TRUE]`).
pub const DEFAULT_OVERWRITE_EXISTING: &str = "FALSE";

/// How many workflows the store holds before `CreateWorkflow` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound on the
/// store, not a claim about any account's Service Quota, which this emulator does not
/// model. Each stored workflow is itself bounded by the modelled lengths of what it
/// holds (`Steps` and `OnExceptionSteps` `len 0..8`, `Description` `len 0..256`, `Tags`
/// `len 1..50`, `StructuredLogDestinations` `len 0..1`), which the framework's boundary
/// validator enforces before a handler runs, and `DeleteWorkflow` releases one.
pub const MAX_STORED_WORKFLOWS: usize = 200;

/// How many SSH public keys ONE user holds before `ImportSshPublicKey` refuses
/// instead of growing.
///
/// AWS Transfer Family's published service quota for a Transfer Family user is 10 SSH
/// public keys, so that is the bound here; this emulator models no Service Quotas
/// adjustments, so it is a fixed one. Each stored key is itself bounded by
/// `SshPublicKeyBody`'s modelled `len 0..2048`, which the framework's boundary
/// validator enforces before a handler runs, so this bounds a user's key set in bytes
/// as well as in count. `DeleteSshPublicKey` releases one, `DeleteUser` all of them.
pub const MAX_SSH_KEYS_PER_USER: usize = 10;

/// The not-found payload for the `resource_type` resource identified by `resource`.
///
/// The identifier travels in `Resource` ALONE: `Message` is a fixed generic string
/// derived from the resource type, so one caller's id never reaches the message a
/// client logs or asserts on. Every operation in this service that cannot find a
/// resource builds its error here, so the construction cannot drift between them.
pub fn resource_not_found(resource_type: &str, resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Unknown {}", resource_type.to_lowercase()),
        resource: resource.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The not-found payload for a `ServerId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_server(server_id: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_SERVER, server_id)
}

/// The not-found payload for a `UserName` the named server does not hold.
pub fn unknown_user(user_name: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_USER, user_name)
}

/// The not-found payload for a `ProfileId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_profile(profile_id: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_PROFILE, profile_id)
}

/// The not-found payload for a `WorkflowId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_workflow(workflow_id: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_WORKFLOW, workflow_id)
}

/// Lift an operation's modelled not-found variant into its error channel, answered
/// with [`RESOURCE_NOT_FOUND_HTTP_STATUS`] rather than the model's `@httpError`.
///
/// Generic over the operation's own `*ErrorModeled` enum, so every handler reaches
/// the one status decision through this function.
pub fn not_found<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, RESOURCE_NOT_FOUND_HTTP_STATUS)
}

// ---------------------------------------------------------------------------
// The service-managed security policy catalog
// ---------------------------------------------------------------------------
//
// Security policies are SERVICE-managed, not per-account resources: no Transfer
// Family operation creates, updates or deletes one. `CreateServer`/`UpdateServer`
// and `CreateConnector`/`UpdateConnector` only ATTACH a published policy by name, and
// `DescribeSecurityPolicy`/`ListSecurityPolicies` report the published set. So the
// catalog is fixed data the service publishes — which is why it lives here as a
// table rather than in [`ComAmazonawsTransferTransferserviceState`], and why nothing
// in this crate writes to it.
//
// Source: the per-policy tables in the AWS Transfer Family User Guide (*Security
// policies for AWS Transfer Family servers* and *Security policies for AWS Transfer
// Family SFTP connectors*), whose columns are exactly
// `DescribedSecurityPolicy`'s members — FIPS status, the protocols the policy covers
// and the enabled SSH ciphers, SSH KEXs, SSH MACs, TLS ciphers and SSH host key
// algorithms (`smithy_query(query="shape",shape="DescribedSecurityPolicy")`).

/// The `SecurityPolicyResourceType` enum's `SERVER` value: the policy is attachable to
/// a server.
pub const SECURITY_POLICY_TYPE_SERVER: &str = "SERVER";

/// The `SecurityPolicyResourceType` enum's `CONNECTOR` value: the policy is attachable
/// to an SFTP connector.
pub const SECURITY_POLICY_TYPE_CONNECTOR: &str = "CONNECTOR";

/// A server policy covers both file transfer protocols `SecurityPolicyProtocol`
/// models (`[SFTP, FTPS]`): its SSH algorithms apply to SFTP and its TLS ciphers to
/// FTPS.
const POLICY_PROTOCOLS_SFTP_FTPS: &[&str] = &[DEFAULT_PROTOCOL, PROTOCOL_FTPS];

/// An SSH-only policy covers SFTP alone — every SFTP connector policy, and the
/// post-quantum server policies, which change key exchange only and carry no TLS
/// ciphers.
const POLICY_PROTOCOLS_SFTP: &[&str] = &[DEFAULT_PROTOCOL];

/// The SSH ciphers the pre-2022 server policies enable: ChaCha20-Poly1305 on top of
/// the AES CTR and GCM set.
const SSH_CIPHERS_WITH_CHACHA: &[&str] = &[
    "chacha20-poly1305@openssh.com",
    "aes128-ctr",
    "aes192-ctr",
    "aes256-ctr",
    "aes128-gcm@openssh.com",
    "aes256-gcm@openssh.com",
];

/// The AES-only SSH cipher set: the 2022-03 and later server policies, every FIPS and
/// restricted policy, and every SFTP connector policy.
const SSH_CIPHERS_AES: &[&str] = &[
    "aes128-ctr",
    "aes192-ctr",
    "aes256-ctr",
    "aes128-gcm@openssh.com",
    "aes256-gcm@openssh.com",
];

/// The key exchanges the original 2018-11 server policy enables, including the SHA1
/// and group14 exchanges every later policy dropped.
const SSH_KEXS_LEGACY: &[&str] = &[
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group14-sha1",
    "diffie-hellman-group14-sha256",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group-exchange-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
];

/// The key exchanges the 2020-06 and later non-FIPS policies enable: the legacy set
/// without the SHA1 and group14 exchanges.
const SSH_KEXS_MODERN: &[&str] = &[
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group-exchange-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
];

/// The key exchanges the FIPS policies enable: the FIPS-approved ones only, so no
/// Curve25519.
const SSH_KEXS_FIPS: &[&str] = &[
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group-exchange-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
];

/// The hybrid NIST-P384 + Kyber-768 exchange the experimental post-quantum policies
/// offer first.
///
/// This published identifier is 56 characters, longer than the `len 0..50` the model
/// puts on `SecurityPolicyOption`'s member. That bound constrains a value a caller
/// SENDS, and no operation in this service takes a `SecurityPolicyOption` as input,
/// so reporting the published identifier is what lets a client match the algorithm
/// its SSH stack negotiates.
const SSH_KEX_PQ_KYBER_768: &str = "ecdh-nistp384-kyber-768r3-sha384-d00@openquantumsafe.org";

/// The hybrid NIST-P256 + Kyber-512 exchange the experimental post-quantum policies
/// offer second. Same length note as [`SSH_KEX_PQ_KYBER_768`].
const SSH_KEX_PQ_KYBER_512: &str = "ecdh-nistp256-kyber-512r3-sha256-d00@openquantumsafe.org";

/// The message authentication codes the 2018-11 server policy enables, including the
/// SHA1 and 64-bit UMAC codes every later policy dropped.
const SSH_MACS_LEGACY: &[&str] = &[
    "hmac-sha1",
    "hmac-sha1-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512",
    "hmac-sha2-512-etm@openssh.com",
    "umac-64@openssh.com",
    "umac-64-etm@openssh.com",
    "umac-128@openssh.com",
    "umac-128-etm@openssh.com",
];

/// The SHA2 and 128-bit UMAC codes the 2020-06 and later non-FIPS, non-restricted
/// policies enable.
const SSH_MACS_SHA2: &[&str] = &[
    "hmac-sha2-256",
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512",
    "hmac-sha2-512-etm@openssh.com",
    "umac-128@openssh.com",
    "umac-128-etm@openssh.com",
];

/// The codes the FIPS policies enable: HMAC-SHA2 only, so no UMAC.
const SSH_MACS_FIPS: &[&str] = &[
    "hmac-sha2-256",
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512",
    "hmac-sha2-512-etm@openssh.com",
];

/// The codes the restricted and post-quantum policies enable: encrypt-then-MAC only.
const SSH_MACS_ETM_ONLY: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "umac-128-etm@openssh.com",
];

/// The TLS ciphers the 2018-11 server policy enables, including the CBC suites every
/// later policy dropped.
const TLS_CIPHERS_LEGACY: &[&str] = &[
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The AEAD-only TLS 1.2 suites the 2020-06 through 2022-03 server policies enable.
const TLS_CIPHERS_TLS12_GCM: &[&str] = &[
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The suites the 2023-05 and later non-FIPS server policies enable: the TLS 1.2 AEAD
/// set plus the TLS 1.3 suites.
const TLS_CIPHERS_TLS13: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_CHACHA20_POLY1305_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The suites the 2023-05 and later FIPS server policies enable: [`TLS_CIPHERS_TLS13`]
/// without the ChaCha20-Poly1305 suite, which is not FIPS-approved.
const TLS_CIPHERS_TLS13_FIPS: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The host key algorithms a connector accepts from the remote SFTP server under a
/// non-FIPS connector policy. `SshHostKeyAlgorithms` "only applies to security
/// policies for connectors", so every server policy leaves it empty.
const HOST_KEY_ALGORITHMS_CONNECTOR: &[&str] = &[
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-256",
    "rsa-sha2-512",
    "ssh-ed25519",
    "ssh-rsa",
];

/// The host key algorithms a FIPS connector policy accepts: Ed25519 is not
/// FIPS-approved, so it is not among them.
const HOST_KEY_ALGORITHMS_CONNECTOR_FIPS: &[&str] = &[
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-256",
    "rsa-sha2-512",
];

/// No list for this member under this policy — the member is absent from the
/// description rather than reported empty (TLS ciphers on an SSH-only policy, host key
/// algorithms on a server policy).
const NO_OPTIONS: &[&str] = &[];

/// One published security policy, as `DescribeSecurityPolicy` reports it.
#[derive(Debug, Clone, Copy)]
pub struct SecurityPolicy {
    /// `SecurityPolicyName`: the policy's text name, the value a server or connector
    /// stores when it attaches this policy.
    pub name: &'static str,
    /// `Type`: [`SECURITY_POLICY_TYPE_SERVER`] or
    /// [`SECURITY_POLICY_TYPE_CONNECTOR`] — the resource kind the policy attaches to.
    pub resource_type: &'static str,
    /// `Fips`: whether the policy enables Federal Information Processing Standards
    /// algorithms only.
    pub fips: bool,
    /// `Protocols`: the file transfer protocols the policy's algorithms apply to.
    pub protocols: &'static [&'static str],
    /// `SshCiphers`.
    pub ssh_ciphers: &'static [&'static str],
    /// `SshKexs`.
    pub ssh_kexs: &'static [&'static str],
    /// `SshMacs`.
    pub ssh_macs: &'static [&'static str],
    /// `TlsCiphers` — empty for an SSH-only policy, which reports no TLS ciphers.
    pub tls_ciphers: &'static [&'static str],
    /// `SshHostKeyAlgorithms` — empty for a server policy, which reports none.
    pub ssh_host_key_algorithms: &'static [&'static str],
}

/// The experimental post-quantum key exchanges, non-FIPS: the two hybrids in front of
/// [`SSH_KEXS_MODERN`]'s classical fallbacks.
const SSH_KEXS_POST_QUANTUM_MODERN: &[&str] = &[
    SSH_KEX_PQ_KYBER_768,
    SSH_KEX_PQ_KYBER_512,
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group-exchange-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
];

/// The experimental post-quantum key exchanges, FIPS: the two hybrids in front of
/// [`SSH_KEXS_FIPS`]'s classical fallbacks.
const SSH_KEXS_POST_QUANTUM_FIPS: &[&str] = &[
    SSH_KEX_PQ_KYBER_768,
    SSH_KEX_PQ_KYBER_512,
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group-exchange-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
];

/// Every security policy this service publishes, sorted by `SecurityPolicyName`.
///
/// Sorted so a listing pages in a stable, resumable order (the same ordering
/// [`page_bounds`] needs) and so [`security_policy`] reads as a lookup in a catalog
/// rather than a search through unordered rows.
pub const SECURITY_POLICIES: &[SecurityPolicy] = &[
    // ---- SFTP connector policies: SSH only, so no TLS ciphers, and they are the
    // only policies that report host key algorithms. ----
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-2023-07",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-2024-03",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-FIPS-2023-07",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR_FIPS,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-FIPS-2024-03",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR_FIPS,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-PQ-SSH-Experimental-2023-07",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-PQ-SSH-FIPS-Experimental-2023-07",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR_FIPS,
    },
    // ---- Server policies. ----
    SecurityPolicy {
        name: "TransferSecurityPolicy-2018-11",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_WITH_CHACHA,
        ssh_kexs: SSH_KEXS_LEGACY,
        ssh_macs: SSH_MACS_LEGACY,
        tls_ciphers: TLS_CIPHERS_LEGACY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_WITH_CHACHA,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2022-03",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2023-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The policy a server gets when `CreateServer` names none; see
        // [`DEFAULT_SECURITY_POLICY_NAME`].
        name: DEFAULT_SECURITY_POLICY_NAME,
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2023-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS13_FIPS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2024-01",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS13_FIPS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2024-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS13_FIPS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-PQ-SSH-Experimental-2023-04",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        // Post-quantum key exchange is an SSH mechanism, so this policy covers SFTP
        // alone and reports no TLS ciphers.
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-PQ-SSH-FIPS-Experimental-2023-04",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-Restricted-2018-11",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-Restricted-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
];

/// The published policy named `name`, or `None` when the service publishes none.
///
/// Exact match: `SecurityPolicyName` is a text name the caller copies from the
/// catalog, and the real service does not fold case.
pub fn security_policy(name: &str) -> Option<&'static SecurityPolicy> {
    SECURITY_POLICIES.iter().find(|policy| policy.name == name)
}

/// Every published policy name, in catalog order — what a listing pages over.
pub fn security_policy_names() -> impl Iterator<Item = &'static str> {
    SECURITY_POLICIES.iter().map(|policy| policy.name)
}

/// One `SecurityPolicyOptions` member of a description: the listed algorithms, or
/// `None` when this policy has no such list.
///
/// A policy that enables nothing for a member reports the member as ABSENT rather
/// than as an empty list, because the member is modelled optional and the real
/// service only reports a list where it applies — `TlsCiphers` "only applies to
/// security policies for servers", `SshHostKeyAlgorithms` "only applies to security
/// policies for connectors".
pub fn policy_options(options: &[&str]) -> Option<Vec<String>> {
    (!options.is_empty()).then(|| options.iter().map(|option| (*option).to_string()).collect())
}

/// `ResourceType` for a security policy, addressed by its `SecurityPolicyName`. Same
/// convention as [`RESOURCE_TYPE_SERVER`]; it lives beside the catalog rather than
/// beside the per-account resource types because a policy is published, not created.
pub const RESOURCE_TYPE_SECURITY_POLICY: &str = "SecurityPolicy";

/// The not-found payload for a `SecurityPolicyName` the service publishes no policy
/// for. Built through [`resource_not_found`] like every other not-found in this
/// service, so the body cannot drift from them; "not found" here means "not in
/// [`SECURITY_POLICIES`]", never "not created yet".
pub fn unknown_security_policy(security_policy_name: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_SECURITY_POLICY, security_policy_name)
}

/// One stored SSH public key of a user.
#[derive(Debug, Clone)]
pub struct SshKey {
    /// The minted `key-` + 17 hex-digit identifier (`^key-[0-9a-f]{17}$`).
    pub ssh_public_key_id: String,
    /// The key material exactly as the caller supplied it.
    pub ssh_public_key_body: String,
    /// When this key was added to the user.
    pub date_imported: SystemTime,
}

/// One stored Transfer Family workflow, keyed by `WorkflowId`.
///
/// The steps are held in the caller's own order — a workflow runs its steps in the
/// order they were submitted — with every documented step-detail default already
/// applied by [`resolve_workflow_steps`], so a reader projects them without
/// defaulting a second time.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub workflow_id: String,
    pub arn: String,
    pub description: Option<String>,
    pub steps: Vec<WorkflowStep>,
    pub on_exception_steps: Vec<WorkflowStep>,
    pub structured_log_destinations: Vec<String>,
    pub tags: Vec<Tag>,
}

/// One stored Transfer Family user, keyed by `(ServerId, UserName)`.
#[derive(Debug, Clone)]
pub struct User {
    pub server_id: String,
    pub user_name: String,
    pub arn: String,
    pub role: String,
    pub home_directory: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory_mappings: Vec<HomeDirectoryMapEntry>,
    pub policy: Option<String>,
    pub posix_profile: Option<PosixProfile>,
    pub ssh_public_keys: Vec<SshKey>,
    pub tags: Vec<Tag>,
}

impl User {
    /// `SshPublicKeyCount`: the live number of keys stored for this user.
    pub fn ssh_public_key_count(&self) -> i32 {
        i32::try_from(self.ssh_public_keys.len()).unwrap_or(i32::MAX)
    }
}

/// One stored AS2 profile, keyed by `ProfileId`.
///
/// Everything `DescribeProfile` reports is RESOLVED here, at create time, so a reader
/// projects the stored record without re-deriving anything: the minted id, its ARN,
/// the AS2-name, the profile type and the certificate ids and tags the create carried.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The minted `p-` + 17 hex-digit identifier (`^p-([0-9a-f]{17})$`).
    pub profile_id: String,
    pub arn: String,
    /// The AS2-name (RFC 4130) exactly as the caller supplied it.
    pub as2_id: String,
    /// `LOCAL` or `PARTNER` — the `ProfileType` enum, enforced before a handler runs.
    pub profile_type: String,
    pub certificate_ids: Vec<String>,
    pub tags: Vec<Tag>,
}

/// One stored Transfer Family server, keyed by `ServerId`.
#[derive(Debug, Clone)]
pub struct Server {
    pub server_id: String,
    pub arn: String,
    pub domain: String,
    pub identity_provider_type: String,
    pub identity_provider_details: Option<IdentityProviderDetails>,
    pub endpoint_type: String,
    pub endpoint_details: Option<EndpointDetails>,
    pub protocols: Vec<String>,
    /// The protocol settings, with every member the model documents a default for
    /// already resolved at create time — so a reader reports what the server is
    /// configured with rather than "unconfigured".
    pub protocol_details: ProtocolDetails,
    pub logging_role: Option<String>,
    pub security_policy_name: String,
    pub pre_authentication_login_banner: Option<String>,
    pub post_authentication_login_banner: Option<String>,
    /// The Base64 SHA256 fingerprint of the server's host key, in the
    /// `SHA256:<base64>` form `ssh-keygen -l` prints.
    ///
    /// Computed at create time from a host key the emulator GENERATES for the server
    /// (as the service does for a server whose host key it manages) — see
    /// [`mint_host_key_fingerprint`]. Never invented: it is the real fingerprint of
    /// real key material.
    pub host_key_fingerprint: String,
    /// The log group structured logging is configured to publish to, or `None` when
    /// the server has NO structured-logging configuration.
    ///
    /// "Not configured" and "configured with nothing" are not the same server: a
    /// reader reports the first by OMITTING `StructuredLogDestinations` and the second
    /// cannot exist, because the model documents an empty value as what CLEARS the
    /// setting ("you can clear it, and in effect turn off structured logging, by
    /// providing an empty value for this parameter"). See
    /// [`configured_log_destinations`].
    pub structured_log_destinations: Option<Vec<String>>,
    /// The S3 storage settings, defaulted at create time like
    /// [`Self::protocol_details`].
    pub s3_storage_options: S3StorageOptions,
    pub ip_address_type: String,
    pub workflow_details: Option<WorkflowDetails>,
    pub certificate: Option<String>,
    pub state: String,
    /// The state this server owes the clock, and when it comes due.
    ///
    /// Set by `CreateServer`/`StartServer` to `(now + SERVER_START_SPAN, "ONLINE")` and
    /// by `StopServer` to `(now + SERVER_STOP_SPAN, "OFFLINE")`; materialised by
    /// [`Server::settle`] on the next read, never by sleeping.
    pub state_due: Option<(Instant, String)>,
    pub tags: Vec<Tag>,
}

impl Server {
    /// Apply the state transition the clock has already made due, if any.
    ///
    /// Called by the reporting operations that render `State`. It writes only a change
    /// that was owed at `now` whether or not anybody called, so two reads at the same
    /// instant answer the same thing.
    pub fn settle(&mut self, now: Instant) {
        if let Some((due, next)) = self.state_due.clone() {
            if now >= due {
                self.state = next;
                self.state_due = None;
            }
        }
    }

    /// Begin the stop the clock will complete: the intermediate `STOPPING` now,
    /// `OFFLINE` when [`SERVER_STOP_SPAN`] has elapsed.
    pub fn begin_stop(&mut self, now: Instant) {
        self.state = SERVER_STATE_STOPPING.to_string();
        self.state_due = Some((now + SERVER_STOP_SPAN, SERVER_STATE_OFFLINE.to_string()));
    }

    /// Begin the start the clock will complete: the intermediate `STARTING` now,
    /// `ONLINE` when [`SERVER_START_SPAN`] has elapsed.
    pub fn begin_start(&mut self, now: Instant) {
        self.state = SERVER_STATE_STARTING.to_string();
        self.state_due = Some((now + SERVER_START_SPAN, SERVER_STATE_ONLINE.to_string()));
    }

    /// What `StopServer` does to this server, by its current `State`.
    pub fn stop_decision(&self) -> ServerTransition {
        ServerTransition::decide(
            &self.state,
            SERVER_STATE_ONLINE,
            SERVER_STATE_OFFLINE,
            SERVER_STATE_STOPPING,
            SERVER_STATE_STOP_FAILED,
        )
    }

    /// What `StartServer` does to this server, by its current `State`.
    pub fn start_decision(&self) -> ServerTransition {
        ServerTransition::decide(
            &self.state,
            SERVER_STATE_OFFLINE,
            SERVER_STATE_ONLINE,
            SERVER_STATE_STARTING,
            SERVER_STATE_START_FAILED,
        )
    }

    /// Whether this server's `WorkflowDetails` attach `workflow_id` to either of the
    /// two triggers the shape carries (`OnUpload`, `OnPartialUpload`).
    ///
    /// A workflow attached to a server is IN USE by it, which is what `DeleteWorkflow`
    /// refuses on.
    pub fn references_workflow(&self, workflow_id: &str) -> bool {
        let Some(details) = self.workflow_details.as_ref() else {
            return false;
        };
        [&details.on_upload, &details.on_partial_upload]
            .into_iter()
            .flatten()
            .flatten()
            .any(|detail| detail.workflow_id == workflow_id)
    }
}

/// What a state-changing server call does, given the server's current `State`.
///
/// `StartServer` and `StopServer` share ONE rule, so the two handlers cannot drift:
/// the call begins its transition from the state it is documented to move the server
/// out of, and from its own failure state, which is what a caller retries; it has "no
/// impact" on a server that is already in, or already on its way to, the state it
/// targets; and it is refused when the server is in flight the other way, because
/// there is then no such transition to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerTransition {
    /// Write the intermediate state now and owe the terminal one to the clock.
    Begin,
    /// Nothing to do: answer success and leave the server, and any pending
    /// transition, exactly as they are.
    NoImpact,
    /// Refuse with the modelled `InvalidRequestException`.
    Refuse,
}

impl ServerTransition {
    /// Classify `state` for a call that moves a server out of `from`, towards `target`
    /// through the intermediate `in_flight`, and that is retried from `failed` — its
    /// own `*_FAILED` state, which the model documents as "an error condition" of this
    /// very transition.
    fn decide(state: &str, from: &str, target: &str, in_flight: &str, failed: &str) -> Self {
        if state == from || state == failed {
            Self::Begin
        } else if state == target || state == in_flight {
            Self::NoImpact
        } else {
            Self::Refuse
        }
    }
}

/// Resolve the `ProtocolDetails` a server is created with.
///
/// Every member the model documents a default for is materialised HERE, once, at create
/// time, so a reader rebuilds the server's real protocol settings instead of reporting
/// nothing configured. A member the caller supplied is kept exactly as given; only the
/// absent ones take their default. `As2Transports` has no documented default (it is
/// meaningful only for AS2 servers), so an absent one stays absent.
pub fn resolve_protocol_details(requested: Option<ProtocolDetails>) -> ProtocolDetails {
    let requested = requested.unwrap_or_default();
    ProtocolDetails {
        passive_ip: Some(
            requested
                .passive_ip
                .unwrap_or_else(|| DEFAULT_PASSIVE_IP.to_string()),
        ),
        tls_session_resumption_mode: Some(
            requested
                .tls_session_resumption_mode
                .unwrap_or_else(|| DEFAULT_TLS_SESSION_RESUMPTION_MODE.to_string()),
        ),
        set_stat_option: Some(
            requested
                .set_stat_option
                .unwrap_or_else(|| DEFAULT_SET_STAT_OPTION.to_string()),
        ),
        sftp_ports: Some(
            requested
                .sftp_ports
                .filter(|ports| !ports.is_empty())
                .map(|ports| {
                    ports
                        .into_iter()
                        .map(|port| SftpPortWithOptions {
                            sftp_port: port.sftp_port,
                            // `CommunicationMode` is optional per entry and defaults
                            // the same way the whole-list default does.
                            communication_mode: Some(port.communication_mode.unwrap_or_else(
                                || DEFAULT_COMMUNICATION_MODE.to_string(),
                            )),
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    vec![SftpPortWithOptions {
                        sftp_port: DEFAULT_SFTP_PORT,
                        communication_mode: Some(DEFAULT_COMMUNICATION_MODE.to_string()),
                    }]
                }),
        ),
        as2_transports: requested.as2_transports,
        proxy_config: Some(ProxyConfig {
            sftp_mode: Some(
                requested
                    .proxy_config
                    .and_then(|proxy| proxy.sftp_mode)
                    .unwrap_or_else(|| DEFAULT_PROXY_SFTP_MODE.to_string()),
            ),
        }),
    }
}

/// The STORED form of the structured-logging destination a request supplied.
///
/// `StructuredLogDestinations` is a configuration setting that is either SET or NOT
/// SET: the model documents an EMPTY value as what turns it off ("If you have
/// previously specified a log group for a server, you can clear it, and in effect turn
/// off structured logging, by providing an empty value for this parameter"), so an
/// empty list is stored as `None` — not configured — rather than as a configured empty
/// list, and a reader omits the member for it.
pub fn configured_log_destinations(requested: Option<Vec<String>>) -> Option<Vec<String>> {
    requested.filter(|destinations| !destinations.is_empty())
}

/// The STORED form of the logging role a request supplied, on the same rule as
/// [`configured_log_destinations`].
///
/// `LoggingRole` targets `NullableRole`, whose pattern `^(|arn:.*role/\S+)$` admits the
/// EMPTY STRING for exactly one purpose — clearing the role — so an empty value is
/// stored as not configured and reported absent, like any other logging member the
/// server does not have.
pub fn configured_logging_role(requested: Option<String>) -> Option<String> {
    requested.filter(|role| !role.is_empty())
}

/// Resolve the `S3StorageOptions` a server is created with, on the same rule as
/// [`resolve_protocol_details`].
pub fn resolve_s3_storage_options(requested: Option<S3StorageOptions>) -> S3StorageOptions {
    S3StorageOptions {
        directory_listing_optimization: Some(
            requested
                .and_then(|options| options.directory_listing_optimization)
                .unwrap_or_else(|| DEFAULT_DIRECTORY_LISTING_OPTIMIZATION.to_string()),
        ),
    }
}

/// Apply the `ProtocolDetails` an update NAMES onto the ones the server already has.
///
/// An update changes the members it names and leaves the rest of the server's protocol
/// settings where they were — then the same create-time resolution runs over the
/// result, so the stored settings stay fully materialised and no member can fall back
/// to "unset".
pub fn merge_protocol_details(
    stored: &ProtocolDetails,
    requested: ProtocolDetails,
) -> ProtocolDetails {
    resolve_protocol_details(Some(ProtocolDetails {
        passive_ip: requested.passive_ip.or_else(|| stored.passive_ip.clone()),
        tls_session_resumption_mode: requested
            .tls_session_resumption_mode
            .or_else(|| stored.tls_session_resumption_mode.clone()),
        set_stat_option: requested
            .set_stat_option
            .or_else(|| stored.set_stat_option.clone()),
        sftp_ports: requested.sftp_ports.or_else(|| stored.sftp_ports.clone()),
        as2_transports: requested
            .as2_transports
            .or_else(|| stored.as2_transports.clone()),
        proxy_config: requested
            .proxy_config
            .or_else(|| stored.proxy_config.clone()),
    }))
}

/// Apply the `S3StorageOptions` an update names onto the stored ones, on the same rule
/// as [`merge_protocol_details`].
pub fn merge_s3_storage_options(
    stored: &S3StorageOptions,
    requested: S3StorageOptions,
) -> S3StorageOptions {
    resolve_s3_storage_options(Some(S3StorageOptions {
        directory_listing_optimization: requested
            .directory_listing_optimization
            .or_else(|| stored.directory_listing_optimization.clone()),
    }))
}

/// Mint a host key for a new server and return its Base64 SHA256 fingerprint.
///
/// AWS generates a host key for a server whose host key it manages, and reports that
/// key's fingerprint — "equivalent to the output of the `ssh-keygen -l -f` command",
/// i.e. OpenSSH's `SHA256:<base64 of the SHA256 of the public key blob>`. So this
/// GENERATES a real Ed25519 SSH host key and asks `ssh-key` — the crate that
/// implements the OpenSSH key formats — for that key's fingerprint; the value is
/// computed from real key material rather than invented. The key itself is
/// per-server and per-run, like the real service's, and is not kept: the emulator
/// serves no SSH transport, so nothing can use the private half, and nothing may
/// return it.
pub fn mint_host_key_fingerprint() -> String {
    let keypair = ::ssh_key::private::Ed25519Keypair::random(&mut ::ssh_key::rand_core::OsRng);
    ::ssh_key::PrivateKey::from(keypair)
        .fingerprint(::ssh_key::HashAlg::Sha256)
        .to_string()
}

/// The Base64 SHA256 fingerprint of host key MATERIAL a caller supplied, when the
/// material is a key the emulator can read.
///
/// `CreateServer`/`UpdateServer` accept a `HostKey`; when it is an OpenSSH private key
/// — what `ssh-keygen` writes — that key IS the server's host key, so the fingerprint
/// reported for the server is parsed straight out of it and matches what
/// `ssh-keygen -l` prints for the same file. `None` for material in a format the
/// emulator cannot read (it implements no other private-key format and serves no SSH
/// transport): the server then keeps the host key it already has, whose genuine
/// fingerprint it reports, rather than one invented for material nothing read.
pub fn supplied_host_key_fingerprint(material: &str) -> Option<String> {
    ::ssh_key::PrivateKey::from_openssh(material)
        .ok()
        .map(|key| key.fingerprint(::ssh_key::HashAlg::Sha256).to_string())
}

// =============================================================================
// Cross-member requirements
//
// Some of this service's members are only meaningful in combination: the model's own
// documentation states, for a member, which value of ANOTHER member makes it
// required, or makes it settable at all. The framework's boundary validator enforces
// the single-member constraints (`@required`, `@length`, `@pattern`, enum value sets)
// before a handler runs, but it cannot see across members — so these requirements are
// checked here, by the create/update handler, BEFORE anything is persisted, and
// refused with the modelled `InvalidRequestException` (HTTP 400), which the model
// documents as "thrown when the client submits a malformed request".
//
// The wording below names the member and the condition it was submitted under, which
// is built entirely from the request; no recorded message text is reproduced.
// =============================================================================

/// "`<member>` must be specified when `<condition>`" — a member the model documents as
/// required under a condition the request meets, and did not send.
fn required_when(member: &str, condition: &str) -> String {
    format!("{member} must be specified when {condition}")
}

/// "`<member>` must be specified" — a member the model documents as part of what the
/// request names, with no condition attached, and did not send.
fn required_member(member: &str) -> String {
    format!("{member} must be specified")
}

/// "`<member>` cannot be specified when `<condition>`" — a member the model documents
/// as unusable under a condition the request meets, and sent anyway.
fn forbidden_when(member: &str, condition: &str) -> String {
    format!("{member} cannot be specified when {condition}")
}

/// "`<member>` can only be set when `<condition>`" — a member the model documents as
/// settable under ONE condition, sent under another.
fn settable_only_when(member: &str, condition: &str) -> String {
    format!("{member} can only be set when {condition}")
}

/// "`<member>` must be `<requirement>` when `<condition>`" — a member whose VALUE the
/// model constrains to another member's value.
fn must_be_when(member: &str, requirement: &str, condition: &str) -> String {
    format!("{member} must be {requirement} when {condition}")
}

/// The server configuration a `CreateServer` (or `UpdateServer`) request resolves to:
/// every member a documented cross-member requirement mentions, with the model's
/// documented defaults ALREADY applied.
///
/// Defaults are applied first because the requirements are about the configuration the
/// server will actually have: a request that omits `EndpointType` selects `PUBLIC`,
/// which needs no VPC endpoint settings at all.
#[derive(Debug)]
pub struct ServerConfig<'a> {
    pub domain: &'a str,
    pub endpoint_type: &'a str,
    pub endpoint_details: Option<&'a EndpointDetails>,
    pub identity_provider_type: &'a str,
    pub identity_provider_details: Option<&'a IdentityProviderDetails>,
    pub ip_address_type: &'a str,
    pub protocols: &'a [String],
    pub certificate: Option<&'a str>,
}

impl ServerConfig<'_> {
    /// Whether this configuration's `IdentityProviderType` is one of the three the
    /// model documents `IdentityProviderDetails` as required for.
    fn needs_identity_provider_details(&self) -> bool {
        matches!(
            self.identity_provider_type,
            IDENTITY_PROVIDER_API_GATEWAY
                | IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE
                | IDENTITY_PROVIDER_AWS_LAMBDA
        )
    }

    /// Whether `Protocols` names `protocol`.
    fn speaks(&self, protocol: &str) -> bool {
        self.protocols.iter().any(|named| named == protocol)
    }

    /// The first documented cross-member requirement this configuration violates,
    /// worded as the refusal states it — `None` when it violates none.
    ///
    /// Every rule below is a sentence in this service's own model:
    ///
    /// * `EndpointDetails` is "the virtual private cloud (VPC) endpoint settings that
    ///   are configured for your server", whose `SubnetIds` are "required to host your
    ///   server endpoint in your VPC" — so a server whose `EndpointType` hosts the
    ///   endpoint in a VPC cannot be configured without them.
    /// * each `EndpointDetails` member states the one `EndpointType` it "can only be
    ///   set when" (`VpcEndpointId` -> `VPC_ENDPOINT`, the rest -> `VPC`), and
    ///   `AddressAllocationIds` adds "You can't set address allocation IDs for servers
    ///   that have an `IpAddressType` set to `DUALSTACK`".
    /// * `IdentityProviderDetails` is "Required when `IdentityProviderType` is set to
    ///   `AWS_DIRECTORY_SERVICE`, `AWS_LAMBDA` or `API_GATEWAY`" and "Cannot be
    ///   specified when `IdentityProviderType` is set to `SERVICE_MANAGED`".
    /// * `Certificate` is "Required when `Protocols` is set to `FTPS`".
    /// * `Protocols` states "If `Protocol` includes either `FTP` or `FTPS`, then the
    ///   `EndpointType` must be `VPC` and the `IdentityProviderType` must be either
    ///   `AWS_DIRECTORY_SERVICE`, `AWS_LAMBDA`, or `API_GATEWAY`" and "If `Protocol`
    ///   includes `AS2`, then the `EndpointType` must be `VPC`, and domain must be
    ///   Amazon S3".
    pub fn cross_member_violation(&self) -> Option<String> {
        let endpoint_type = self.endpoint_type;
        let hosted_in_vpc =
            matches!(endpoint_type, ENDPOINT_TYPE_VPC | ENDPOINT_TYPE_VPC_ENDPOINT);

        if hosted_in_vpc && self.endpoint_details.is_none() {
            return Some(required_when(
                "EndpointDetails",
                &format!("EndpointType is {endpoint_type}"),
            ));
        }

        if let Some(details) = self.endpoint_details {
            for (member, submitted, only_with) in [
                (
                    "EndpointDetails.VpcId",
                    details.vpc_id.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.SubnetIds",
                    details.subnet_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.SecurityGroupIds",
                    details.security_group_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.AddressAllocationIds",
                    details.address_allocation_ids.is_some(),
                    ENDPOINT_TYPE_VPC,
                ),
                (
                    "EndpointDetails.VpcEndpointId",
                    details.vpc_endpoint_id.is_some(),
                    ENDPOINT_TYPE_VPC_ENDPOINT,
                ),
            ] {
                if submitted && endpoint_type != only_with {
                    return Some(settable_only_when(
                        member,
                        &format!("EndpointType is {only_with}"),
                    ));
                }
            }
            if details.address_allocation_ids.is_some()
                && self.ip_address_type == IP_ADDRESS_TYPE_DUALSTACK
            {
                return Some(forbidden_when(
                    "EndpointDetails.AddressAllocationIds",
                    &format!("IpAddressType is {IP_ADDRESS_TYPE_DUALSTACK}"),
                ));
            }
        }

        let provider_condition = format!("IdentityProviderType is {}", self.identity_provider_type);
        match (
            self.needs_identity_provider_details(),
            self.identity_provider_details.is_some(),
        ) {
            (true, false) => {
                return Some(required_when("IdentityProviderDetails", &provider_condition))
            }
            (false, true) => {
                return Some(forbidden_when(
                    "IdentityProviderDetails",
                    &provider_condition,
                ))
            }
            _ => {}
        }

        // An empty `Certificate` names no certificate: the member's `len 0..1600`
        // admits the empty string, so presence alone is not enough.
        if self.speaks(PROTOCOL_FTPS)
            && self
                .certificate
                .is_none_or(|certificate| certificate.trim().is_empty())
        {
            return Some(required_when(
                "Certificate",
                &format!("Protocols includes {PROTOCOL_FTPS}"),
            ));
        }

        for protocol in [PROTOCOL_FTP, PROTOCOL_FTPS] {
            if !self.speaks(protocol) {
                continue;
            }
            if endpoint_type != ENDPOINT_TYPE_VPC {
                return Some(must_be_when(
                    "EndpointType",
                    ENDPOINT_TYPE_VPC,
                    &format!("Protocols includes {protocol}"),
                ));
            }
            if !self.needs_identity_provider_details() {
                return Some(must_be_when(
                    "IdentityProviderType",
                    &format!(
                        "{IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE}, \
                         {IDENTITY_PROVIDER_AWS_LAMBDA} or {IDENTITY_PROVIDER_API_GATEWAY}"
                    ),
                    &format!("Protocols includes {protocol}"),
                ));
            }
        }

        if self.speaks(PROTOCOL_AS2) {
            if endpoint_type != ENDPOINT_TYPE_VPC {
                return Some(must_be_when(
                    "EndpointType",
                    ENDPOINT_TYPE_VPC,
                    &format!("Protocols includes {PROTOCOL_AS2}"),
                ));
            }
            if self.domain != DOMAIN_S3 {
                return Some(must_be_when(
                    "Domain",
                    DOMAIN_S3,
                    &format!("Protocols includes {PROTOCOL_AS2}"),
                ));
            }
        }

        None
    }
}

/// The documented cross-member requirement a user's home-directory configuration
/// violates, worded as the refusal states it — `None` when it violates none.
///
/// `HomeDirectoryType` states "If `HomeDirectoryType` is `LOGICAL`, you must provide
/// mappings, using the `HomeDirectoryMappings` parameter", and
/// `HomeDirectoryMappings` states "This value can be set only when
/// `HomeDirectoryType` is set to `LOGICAL`" — one requirement in each direction.
pub fn home_directory_violation(
    home_directory_type: Option<&str>,
    home_directory_mappings: Option<&[HomeDirectoryMapEntry]>,
) -> Option<String> {
    let condition = format!("HomeDirectoryType is {HOME_DIRECTORY_TYPE_LOGICAL}");
    let logical = home_directory_type == Some(HOME_DIRECTORY_TYPE_LOGICAL);
    let mapped = home_directory_mappings.is_some_and(|mappings| !mappings.is_empty());
    match (logical, mapped) {
        (true, false) => Some(required_when("HomeDirectoryMappings", &condition)),
        (false, true) => Some(settable_only_when("HomeDirectoryMappings", &condition)),
        _ => None,
    }
}

/// The first documented step requirement a submitted step LIST violates, worded as the
/// refusal states it and naming the step by its zero-based index in `member` — the way
/// a JSON request indexes the list — `None` when every step satisfies them.
///
/// `member` is the request member the list came from (`Steps` or `OnExceptionSteps`),
/// so one message locates the step a caller has to fix.
pub fn workflow_steps_violation(member: &str, steps: &[WorkflowStep]) -> Option<String> {
    steps
        .iter()
        .enumerate()
        .find_map(|(index, step)| workflow_step_violation(&format!("{member}[{index}]"), step))
}

/// The documented requirement ONE workflow step violates, worded as the refusal states
/// it — `None` when it violates none.
///
/// Both rules below are sentences in this service's own model:
///
/// * `WorkflowStep.Type` "specifies which of the following actions is being taken for
///   this step", and each `*StepDetails` member is documented as the details "for a
///   step that" performs exactly that one action — so a step carries the details
///   structure its `Type` names, and no other. A step that names no `Type` names no
///   action at all.
/// * each details shape documents what the step acts ON as part of what the step
///   "consists of": a `CUSTOM` step is "a custom step with an Lambda function target"
///   whose `Target` is "The ARN for the Lambda function that is being called", a `COPY`
///   step consists of "An Amazon S3 location for the destination of the file copy", and
///   a `TAG` step's `Tags` are the "1 to 10 key/value pairs" it adds. `DECRYPT`'s own
///   two (`Type`, `DestinationFileLocation`) are `@required` members of
///   `DecryptStepDetails`, so the boundary validator refuses those before a handler
///   runs, and `DELETE` acts on the source file alone.
fn workflow_step_violation(path: &str, step: &WorkflowStep) -> Option<String> {
    let Some(step_type) = step.r#type.as_deref().filter(|named| !named.is_empty()) else {
        return Some(required_member(&format!("{path}.Type")));
    };
    let condition = format!("Type is {step_type}");

    // The details member each step type selects: present for its own type, absent for
    // every other one.
    for (member, submitted, selected_by) in [
        (
            "CopyStepDetails",
            step.copy_step_details.is_some(),
            WORKFLOW_STEP_TYPE_COPY,
        ),
        (
            "CustomStepDetails",
            step.custom_step_details.is_some(),
            WORKFLOW_STEP_TYPE_CUSTOM,
        ),
        (
            "TagStepDetails",
            step.tag_step_details.is_some(),
            WORKFLOW_STEP_TYPE_TAG,
        ),
        (
            "DeleteStepDetails",
            step.delete_step_details.is_some(),
            WORKFLOW_STEP_TYPE_DELETE,
        ),
        (
            "DecryptStepDetails",
            step.decrypt_step_details.is_some(),
            WORKFLOW_STEP_TYPE_DECRYPT,
        ),
    ] {
        let member = format!("{path}.{member}");
        match (submitted, step_type == selected_by) {
            (false, true) => return Some(required_when(&member, &condition)),
            (true, false) => {
                return Some(settable_only_when(
                    &member,
                    &format!("Type is {selected_by}"),
                ))
            }
            _ => {}
        }
    }

    // What the step acts on. An empty string names nothing: `CustomStepTarget` is
    // `len 0..170`, so the modelled length admits one, and its pattern
    // `^arn:[a-z-]+:lambda:.*$` is what refuses it at the boundary — this is the
    // belt-and-braces check that an absent target is refused here either way.
    match step_type {
        WORKFLOW_STEP_TYPE_CUSTOM
            if step.custom_step_details.as_ref().is_some_and(|details| {
                details
                    .target
                    .as_deref()
                    .is_none_or(|target| target.trim().is_empty())
            }) =>
        {
            Some(required_when(
                &format!("{path}.CustomStepDetails.Target"),
                &condition,
            ))
        }
        WORKFLOW_STEP_TYPE_COPY
            if step
                .copy_step_details
                .as_ref()
                .is_some_and(|details| details.destination_file_location.is_none()) =>
        {
            Some(required_when(
                &format!("{path}.CopyStepDetails.DestinationFileLocation"),
                &condition,
            ))
        }
        WORKFLOW_STEP_TYPE_TAG
            if step.tag_step_details.as_ref().is_some_and(|details| {
                details.tags.as_ref().is_none_or(|tags| tags.is_empty())
            }) =>
        {
            Some(required_when(
                &format!("{path}.TagStepDetails.Tags"),
                &condition,
            ))
        }
        _ => None,
    }
}

/// The steps as they will be STORED: the caller's order, with every documented
/// step-detail default resolved once, here, so no reader has to re-apply one.
pub fn resolve_workflow_steps(steps: Vec<WorkflowStep>) -> Vec<WorkflowStep> {
    steps.into_iter().map(resolve_workflow_step).collect()
}

/// `current` when the caller sent one, else `fallback` — the one place a step-detail
/// default is applied.
fn defaulted(current: Option<String>, fallback: &str) -> Option<String> {
    Some(current.unwrap_or_else(|| fallback.to_string()))
}

/// One step with its documented defaults applied: every details shape that carries
/// `SourceFileLocation` defaults it to [`DEFAULT_SOURCE_FILE_LOCATION`], and every one
/// that carries `OverwriteExisting` defaults it to [`DEFAULT_OVERWRITE_EXISTING`].
fn resolve_workflow_step(mut step: WorkflowStep) -> WorkflowStep {
    if let Some(details) = step.copy_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
        details.overwrite_existing =
            defaulted(details.overwrite_existing.take(), DEFAULT_OVERWRITE_EXISTING);
    }
    if let Some(details) = step.custom_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.tag_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.delete_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
    }
    if let Some(details) = step.decrypt_step_details.as_mut() {
        details.source_file_location =
            defaulted(details.source_file_location.take(), DEFAULT_SOURCE_FILE_LOCATION);
        details.overwrite_existing =
            defaulted(details.overwrite_existing.take(), DEFAULT_OVERWRITE_EXISTING);
    }
    step
}

/// One stored AS2 or SFTP connector, keyed by its `ConnectorId`.
///
/// Holds the members a LISTING reports — `ListedConnector` is `{Arn, ConnectorId,
/// Url}` — each RESOLVED when the connector is written, so no reader re-derives one.
/// A write operation that reports more of a connector than a listing does
/// (`DescribeConnector`'s `AccessRole`, `SftpConfig`, `Status`, …) extends THIS record
/// rather than adding a second map beside it.
#[derive(Debug, Clone)]
pub struct Connector {
    /// The minted `"c-" + 17 hex` identifier (`ConnectorId` is `^c-([0-9a-f]{17})$`).
    pub connector_id: String,
    /// `arn:aws:transfer:<region>:<account>:connector/<ConnectorId>`, as
    /// [`connector_arn`] builds it.
    pub arn: String,
    /// The partner's AS2 or SFTP endpoint, exactly as the write supplied it.
    ///
    /// `Option` because `Url`'s own documentation makes it absent for one class of
    /// connector: "For VPC Lattice type connectors, the URL must be null." Its
    /// modelled `len 0..255` also admits the empty string, which is a PRESENT url and
    /// stays distinct from an absent one.
    pub url: Option<String>,
}

/// Per-service state for com-amazonaws-transfer-transferservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTransferTransferserviceState {
    /// Servers by `ServerId` (`"s-" + 17 hex`). `BTreeMap` so `ListServers` pages in a
    /// stable, resumable order.
    pub servers: BTreeMap<String, Server>,
    /// Users by `(ServerId, UserName)`, so one server's users are a contiguous,
    /// ordered range for `ListUsers`.
    pub users: BTreeMap<(String, String), User>,
    /// Connectors by `ConnectorId` (`"c-" + 17 hex`). `BTreeMap` for the same reason
    /// as [`Self::servers`]: `ListConnectors` pages it in a stable, resumable order.
    /// BOUNDED by [`MAX_STORED_CONNECTORS`], which
    /// [`Self::has_connector_capacity`] is the predicate for.
    pub connectors: BTreeMap<String, Connector>,
    /// AS2 profiles by `ProfileId` (`"p-" + 17 hex`). `BTreeMap` so `ListProfiles`
    /// pages in a stable, resumable order, like [`Self::servers`].
    pub profiles: BTreeMap<String, Profile>,
    /// Workflows by `WorkflowId` (`"w-" + 17 hex`). `BTreeMap` so `ListWorkflows` pages
    /// in a stable, resumable order.
    pub workflows: BTreeMap<String, Workflow>,
    /// Monotonic counter behind every minted id, so ids are unique and reproducible.
    pub next_id_seq: u64,
}

impl ComAmazonawsTransferTransferserviceState {
    /// Mint the next `<prefix>-<17 hex digits>` identifier.
    ///
    /// `ServerId` is `^s-([0-9a-f]{17})$`, `SshPublicKeyId` is `^key-[0-9a-f]{17}$`
    /// and `ProfileId` is `^p-([0-9a-f]{17})$`, so all three are this shape;
    /// `{:017x}` is exactly 17 lowercase hex digits for every
    /// counter value below 16^17, which a counter incremented once per created
    /// resource cannot reach.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id_seq += 1;
        format!("{prefix}-{:017x}", self.next_id_seq)
    }

    /// The users stored for `server_id`, in `ListUsers` paging order.
    ///
    /// A RANGE scan, not a filter over the whole map: the key is
    /// `(ServerId, UserName)`, so one server's users are a contiguous run, and this
    /// walks only that run. One allocation (the range's lower bound) for the whole
    /// walk.
    pub fn users_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a User> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.users
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, user)| user)
    }

    /// `UserCount`: the live number of users assigned to `server_id`.
    ///
    /// Walks only that server's run of the user map. A caller that needs the count for
    /// SEVERAL servers (`ListServers`) must use [`Self::user_counts`] instead, so the
    /// work stays one pass in total rather than one walk per listed server.
    pub fn user_count(&self, server_id: &str) -> i32 {
        i32::try_from(self.users_of(server_id).count()).unwrap_or(i32::MAX)
    }

    /// Whether `server_id` can take another user, i.e. is below
    /// [`MAX_USERS_PER_SERVER`].
    pub fn server_has_user_capacity(&self, server_id: &str) -> bool {
        self.users_of(server_id).count() < MAX_USERS_PER_SERVER
    }

    /// Whether the store can take another server, i.e. is below
    /// [`MAX_STORED_SERVERS`].
    pub fn has_server_capacity(&self) -> bool {
        self.servers.len() < MAX_STORED_SERVERS
    }

    /// Whether the store can take another connector, i.e. is below
    /// [`MAX_STORED_CONNECTORS`].
    ///
    /// The connector counterpart of [`Self::has_server_capacity`]: a connector write
    /// consults this BEFORE inserting and refuses with the modelled
    /// `ServiceUnavailableException` when it answers `false`, so no caller can grow the
    /// map without end.
    pub fn has_connector_capacity(&self) -> bool {
        self.connectors.len() < MAX_STORED_CONNECTORS
    }

    /// Whether the store can take another AS2 profile, i.e. is below
    /// [`MAX_STORED_PROFILES`].
    pub fn has_profile_capacity(&self) -> bool {
        self.profiles.len() < MAX_STORED_PROFILES
    }

    /// Whether the store can take another workflow, i.e. is below
    /// [`MAX_STORED_WORKFLOWS`].
    pub fn has_workflow_capacity(&self) -> bool {
        self.workflows.len() < MAX_STORED_WORKFLOWS
    }

    /// The ids of the servers that currently attach `workflow_id`, in `ServerId` order.
    ///
    /// A workflow attached to a server is in use by it; `DeleteWorkflow` refuses while
    /// this is non-empty, so the servers' `WorkflowDetails` cannot come to name a
    /// workflow that no longer exists.
    pub fn servers_using_workflow(&self, workflow_id: &str) -> Vec<&str> {
        self.servers
            .values()
            .filter(|server| server.references_workflow(workflow_id))
            .map(|server| server.server_id.as_str())
            .collect()
    }

    /// `UserCount` for every server that has users, in ONE pass over the user map.
    ///
    /// `ListServers` renders up to [`MAX_PAGE_SIZE`] servers per call; counting each
    /// one separately would cost `page x users`, a product no single bound covers.
    /// This makes the whole page cost `users + page`.
    pub fn user_counts(&self) -> BTreeMap<&str, i32> {
        let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
        for (server_id, _) in self.users.keys() {
            let count = counts.entry(server_id.as_str()).or_insert(0);
            *count = count.saturating_add(1);
        }
        counts
    }

    /// Materialise every server state transition the clock has already made due.
    pub fn settle_servers(&mut self, now: Instant) {
        for server in self.servers.values_mut() {
            server.settle(now);
        }
    }

    /// The due instant for a server created at `now`, and the state it owes.
    pub fn server_start_due(now: Instant) -> (Instant, String) {
        (now + SERVER_START_SPAN, SERVER_STATE_ONLINE.to_string())
    }
}

/// `arn:aws:transfer:<region>:<account>:server/<ServerId>`.
pub fn server_arn(region: &str, server_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("server/{server_id}"))
}

/// `arn:aws:transfer:<region>:<account>:profile/<ProfileId>`.
pub fn profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("profile/{profile_id}"))
}

/// `arn:aws:transfer:<region>:<account>:user/<ServerId>/<UserName>`.
pub fn user_arn(region: &str, server_id: &str, user_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("user/{server_id}/{user_name}"),
    )
}

/// `arn:aws:transfer:<region>:<account>:connector/<ConnectorId>`.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("connector/{connector_id}"))
}

/// `arn:aws:transfer:<region>:<account>:workflow/<WorkflowId>`.
pub fn workflow_arn(region: &str, workflow_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("workflow/{workflow_id}"))
}

/// The region the caller signed this request for.
///
/// Read from the SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/<service>/aws4_request`), which is the only
/// place the region appears in a request to an emulated endpoint. Parsed with
/// `emulator_server::sigv4_credential_scope`, the framework's single source of
/// SigV4-scope parsing, so this agrees with how routing and telemetry read the same
/// header — including on rejecting a header that is not a well-formed
/// `AWS4-HMAC-SHA256` scope. Falls back to [`DEFAULT_REGION`] when there is no such
/// scope, so an unsigned request still gets a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode the exclusive start key of the next page as an opaque `NextToken`.
///
/// Hex rather than the raw key so the token reads as opaque to a caller and carries
/// no character a JSON string or a URL has to escape; it decodes back to exactly one
/// key, so paging resumes deterministically.
pub fn encode_page_token(next_key: &str) -> String {
    ::hex::encode(next_key.as_bytes())
}

/// Decode a `NextToken` back to the exclusive start key.
///
/// `None` when the token was not produced by [`encode_page_token`] — the caller gets
/// `InvalidNextTokenException` rather than a silently restarted listing.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The index range of one page of an ordered key list, and the next page's token key.
///
/// `all` must be sorted (it comes straight out of a `BTreeMap`). `after` is the
/// exclusive start key decoded from the caller's `NextToken`; `limit` is the page
/// bound. Returns the half-open index range to render and, only when more remain, the
/// exclusive start key of the next page. Indices rather than cloned keys, so a caller
/// can slice the records it already holds instead of looking each one up again.
pub fn page_bounds(
    all: &[&str],
    after: Option<&str>,
    limit: usize,
) -> (::std::ops::Range<usize>, Option<String>) {
    let start = match after {
        // A token naming a key that has since been deleted still resumes right after
        // where it pointed, because this is a position, not a lookup.
        Some(key) => all.partition_point(|candidate| *candidate <= key),
        None => 0,
    };
    let end = start.saturating_add(limit).min(all.len());
    // The token is the LAST key of this page, which is exactly the exclusive start of
    // the next one — and only when a next one exists.
    let next = if end < all.len() {
        all.get(start..end)
            .and_then(<[&str]>::last)
            .map(|key| (*key).to_string())
    } else {
        None
    };
    (start..end, next)
}

/// Turn a caller's `MaxResults` into a page bound.
///
/// The modelled `range 1..1000` is already enforced before a handler runs, so an
/// out-of-range value never reaches here; an absent one means "no caller-imposed
/// bound", which is [`MAX_PAGE_SIZE`].
pub fn page_limit(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTransferTransferserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTransferTransferserviceState>)
}
