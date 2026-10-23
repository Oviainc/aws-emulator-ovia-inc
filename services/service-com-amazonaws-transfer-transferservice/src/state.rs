// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transfer-transferservice service.
//!
//! Everything AWS Transfer Family's server + user + web app + AS2 profile + connector
//! + host-key lifecycle needs to answer a read from stored state lives here: the
//! resource maps (servers, their users and host keys, the web apps `ListWebApps`
//! pages, the AS2 profiles, the connectors, and the
//! workflows a server's
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
//! A tag set is NOT a map of its own: it belongs to the resource that carries it, so
//! the tag plane ([`ComAmazonawsTransferTransferserviceState::tags_of`],
//! [`ComAmazonawsTransferTransferserviceState::tags_mut`]) resolves a resource by its
//! stored ARN and reads or rewrites that resource's own `tags` — which is why a tag
//! written through `TagResource` is at once what the resource's own reader reports.
//!
//! `UserCount` and `SshPublicKeyCount` are DERIVED at read time ([`
//! ComAmazonawsTransferTransferserviceState::user_count`],
//! [`User::ssh_public_key_count`]) rather than stored, so they cannot go stale when a
//! user or a key is added or removed. Every other member a reader reports is RESOLVED
//! once, when the resource is created or updated, so no reader has to re-apply a
//! default.
//!
//! The `Message` of a not-found, an already-exists or a documented state refusal is
//! built here, as a FIXED text per condition that carries no request data: the
//! identifier a caller needs to see travels in the error's own `Resource`/
//! `ResourceType` members, so an id can never reach a message a client logs or asserts
//! on, and a text can never be derived from an internal name. A handler still words
//! its own refusals for conditions this module holds no text for (a store-capacity
//! refusal, say); the one message built from the request is the framework's
//! `ValidationException`, whose template (member path + modelled constraint, no value)
//! lives in `crate::service`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AccessDeniedException, As2ConnectorConfig, ConnectorEgressConfig,
    ConnectorVpcLatticeEgressConfig, DescribedWebAppEndpointDetails,
    DescribedWebAppIdentityProviderDetails, EndpointDetails, HomeDirectoryMapEntry,
    IdentityProviderDetails, InvalidRequestException, PosixProfile, ProtocolDetails, ProxyConfig,
    ResourceExistsException, ResourceNotFoundException, S3StorageOptions, SftpConnectorConfig,
    SftpPortWithOptions, Tag, UpdateConnectorEgressConfig, WebAppUnits, WorkflowDetails,
    WorkflowStep,
};

/// The file location and the session details an execution record reports, named through
/// their module rather than added to the shared `use` list above: that list is edited by
/// every change this crate's services make, and an execution record is the only reader
/// of these two shapes.
use crate::ops::types::FileLocation as ExecutionFileLocation;
use crate::ops::types::ServiceMetadata as ExecutionServiceMetadata;

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

/// `WebAppEndpointPolicy`'s documented default ("Setting for the type of endpoint
/// policy for the web app. The default value is STANDARD"). The enum is
/// `[FIPS, STANDARD]`.
pub const DEFAULT_WEB_APP_ENDPOINT_POLICY: &str = "STANDARD";

/// `WebAppEndpointType`'s publicly-accessible value (the enum is `[PUBLIC, VPC]`),
/// which is what a web app created without `EndpointDetails` is hosted on: the member
/// is documented as "PUBLIC for publicly accessible endpoints and VPC for VPC-hosted
/// endpoints that provide network isolation", and the only `WebAppEndpointDetails`
/// member is `Vpc`.
pub const WEB_APP_ENDPOINT_TYPE_PUBLIC: &str = "PUBLIC";

/// `WebAppEndpointType`'s VPC-hosted value — see [`WEB_APP_ENDPOINT_TYPE_PUBLIC`].
pub const WEB_APP_ENDPOINT_TYPE_VPC: &str = "VPC";

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

/// How many web apps the store holds before `CreateWebApp` refuses instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's OWN bound on
/// the store, not a claim about any account's quota (this emulator models no Service
/// Quotas). It is what stops `web_apps` growing without end; `DeleteWebApp` releases a
/// record. Each stored record is itself bounded by the modelled lengths of the members
/// it holds (`WebAppAccessEndpoint` `len 1..1024`, `Tags` `len 1..50`), which the
/// framework's boundary validator enforces before a handler runs.
pub const MAX_STORED_WEB_APPS: usize = 200;

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

/// `ResourceType` for a directory access grant, addressed by its `ExternalId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_ACCESS: &str = "Access";

/// An `ExternalId` the resolved server holds no access grant for. Its own entry in the
/// not-found table, like every other kind.
pub const NOT_FOUND_ACCESS: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_ACCESS,
    message: "Unknown access",
};

/// `ResourceType` for the connector resource, addressed by its `ConnectorId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_CONNECTOR: &str = "Connector";

/// A `ConnectorId` nothing in the store answers to. Its own entry in the not-found
/// table, like every other kind.
pub const NOT_FOUND_CONNECTOR: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_CONNECTOR,
    message: "Unknown connector",
};

/// `ResourceType` for the web app resource. Same convention as
/// [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_WEB_APP: &str = "WebApp";

/// A `WebAppId` nothing in the store answers to. Its own entry in the not-found
/// table, like every other kind.
///
/// The text names the web app resource as the service's own recorded wording does —
/// the camel-case resource name, as [`NOT_FOUND_SECURITY_POLICY`] also spells it —
/// rather than a prose rendering of it.
pub const NOT_FOUND_WEB_APP: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_WEB_APP,
    message: "Unknown WebApp",
};

/// `ResourceType` for the AS2 profile resource, addressed by its `ProfileId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_PROFILE: &str = "Profile";

/// `ResourceType` for the workflow resource, addressed by its `WorkflowId`. Same
/// convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_WORKFLOW: &str = "Workflow";

/// `ResourceType` for one execution of a workflow, addressed by its `ExecutionId`.
/// Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_EXECUTION: &str = "Execution";

/// An `ExecutionId` the resolved workflow is not running. Its own entry in the
/// not-found table, like every other kind.
pub const NOT_FOUND_EXECUTION: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_EXECUTION,
    message: "Unknown execution",
};

/// `ResourceType` as the duplicate-`UserName` `ResourceExistsException` carries it:
/// the resource name in lower case, which is the value this service has answered that
/// one condition with since its duplicate check was written. Nothing recorded
/// contradicts either spelling, so both are left exactly as they were; only the
/// `Message` of that error is this module's concern.
pub const RESOURCE_TYPE_USER_LOWERCASE: &str = "user";

/// One kind of Transfer resource as a not-found error REPORTS it: the `ResourceType`
/// the error body carries, and the fixed `Message` the service answers for a
/// reference to that kind it cannot resolve.
///
/// The two travel together, in a table of named constants, because the `Message` is
/// the only thing in the body that says WHICH reference was bad: deriving it from the
/// `ResourceType` token (or from any other internal name) would make the
/// caller-visible wording follow this crate's internals. A resource kind is added by
/// adding a constant below, with the text the service answers for it.
#[derive(Debug, Clone, Copy)]
pub struct NotFoundResource {
    /// The `ResourceType` member of the error body.
    pub resource_type: &'static str,
    /// The `Message` member: fixed for this kind, carrying no request data.
    pub message: &'static str,
}

/// A `ServerId` nothing in the store answers to.
pub const NOT_FOUND_SERVER: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SERVER,
    message: "Unknown server",
};

/// A `UserName` the resolved server does not hold.
pub const NOT_FOUND_USER: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_USER,
    message: "Unknown user",
};

/// An `SshPublicKeyId` the resolved user does not hold. The service names this
/// resource by its ID in the text, which is what distinguishes it from a user that
/// could not be resolved.
pub const NOT_FOUND_SSH_PUBLIC_KEY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SSH_PUBLIC_KEY,
    message: "Unknown public key ID",
};

/// A `ProfileId` nothing in the store answers to.
pub const NOT_FOUND_PROFILE: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_PROFILE,
    message: "Unknown profile",
};

/// A `WorkflowId` nothing in the store answers to.
pub const NOT_FOUND_WORKFLOW: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_WORKFLOW,
    message: "Unknown workflow",
};

/// The fixed `Message` for a `CreateUser` whose `UserName` the server already holds.
pub const USER_ALREADY_EXISTS: &str = "User already exists";

/// The fixed `Message` for an `ImportSshPublicKey` whose body the user already holds.
pub const SSH_PUBLIC_KEY_ALREADY_EXISTS: &str = "Public key already exists";

/// The fixed `Message` for an `UpdateServer` that would change `IpAddressType` on a
/// server that is not [`SERVER_STATE_OFFLINE`].
///
/// The model states the rule on `IpAddressType` itself: "It cannot be changed while
/// the server is online. You must stop the server before modifying this parameter."
/// The text states that requirement, so it is the same whichever non-`OFFLINE` state
/// the server is in.
pub const IP_ADDRESS_TYPE_NEEDS_OFFLINE_SERVER: &str =
    "Server must be OFFLINE to change IpAddressType";

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

/// The `ExecutionStatus` enum's values (`[IN_PROGRESS, COMPLETED, EXCEPTION,
/// HANDLING_EXCEPTION]`), documented on `ListedExecution.Status` as "in progress,
/// completed, exception encountered, or handling the exception".
///
/// `HANDLING_EXCEPTION` has no constant: it is the window in which a workflow is
/// running its `OnExceptionSteps`, and this emulator runs no step, so no execution it
/// holds is ever inside that window.
pub const EXECUTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
pub const EXECUTION_STATUS_COMPLETED: &str = "COMPLETED";
pub const EXECUTION_STATUS_EXCEPTION: &str = "EXCEPTION";

/// The `CustomStepStatus` enum's values (`[SUCCESS, FAILURE]`) — the outcome a
/// `SendWorkflowStepState` callback reports for the custom step its `Token` addresses,
/// and so the two outcomes [`WorkflowExecution::reported_status`] reads back off a
/// step.
pub const CUSTOM_STEP_STATUS_SUCCESS: &str = "SUCCESS";
pub const CUSTOM_STEP_STATUS_FAILURE: &str = "FAILURE";

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

/// `ResourceType` for one of a server's host keys, addressed by its `HostKeyId`.
/// Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_HOST_KEY: &str = "HostKey";

/// A `HostKeyId` the resolved server does not hold — never imported, or already
/// deleted. Its own entry in the not-found table, like every other kind.
pub const NOT_FOUND_HOST_KEY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_HOST_KEY,
    message: "Unknown host key",
};

/// `HostKeyStorageType`'s value for a key the service itself stores (the enum is
/// `[SERVICE_MANAGED, SECRETS_MANAGER]`).
pub const HOST_KEY_STORAGE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";

/// `HostKeyStorageType`'s value for a key whose private material lives in a Secrets
/// Manager secret.
pub const HOST_KEY_STORAGE_SECRETS_MANAGER: &str = "SECRETS_MANAGER";

/// `StorageType` for an import that named none.
///
/// `StorageType` documents Secrets Manager storage as the OPT-IN half of the pair
/// ("When you specify SECRETS_MANAGER, Transfer Family stores the private host key
/// material in an Secrets Manager secret"), so a request that specifies nothing leaves
/// the key stored by Transfer Family itself.
pub const DEFAULT_HOST_KEY_STORAGE_TYPE: &str = HOST_KEY_STORAGE_SERVICE_MANAGED;

/// How many host keys ONE server holds before `ImportHostKey` refuses instead of
/// growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound on
/// the store, not a claim about any account's quota. Each stored record is itself
/// bounded by the modelled lengths of what a caller may send (`HostKeyBody`
/// `len 0..4096`, `Description` `len 0..200`, `Tags` `len 1..50`), which the
/// framework's boundary validator enforces before a handler runs, so this bounds a
/// server's key set in bytes as well as in count. `DeleteHostKey` releases one record,
/// `DeleteServer` all of a server's.
pub const MAX_HOST_KEYS_PER_SERVER: usize = 10;

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

// -----------------------------------------------------------------------------
// Connectors
// -----------------------------------------------------------------------------

/// The `ConnectorEgressType` enum's values (`[SERVICE_MANAGED, VPC_LATTICE]`):
/// `SERVICE_MANAGED` "uses Transfer Family managed NAT gateways", `VPC_LATTICE`
/// "routes traffic through customer VPCs using VPC Lattice".
pub const CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
pub const CONNECTOR_EGRESS_TYPE_VPC_LATTICE: &str = "VPC_LATTICE";

/// `DescribedConnector.EgressType`'s modelled default (`@default("SERVICE_MANAGED")`):
/// a connector created with no `EgressConfig` is "a connector without egress
/// configuration", i.e. a service-managed one.
pub const DEFAULT_CONNECTOR_EGRESS_TYPE: &str = CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED;

/// `DescribedConnector.Status`'s modelled default (`@default("ACTIVE")`), documented as
/// "ready for operations".
///
/// This is the status a created connector carries: `CreateConnector` records the
/// parameters of a connection and the connector is usable as soon as it answers, and
/// the `ConnectorStatus` enum's other values describe a create or update still in
/// flight (`PENDING`) or one that failed (`ERRORED`) — neither of which this emulator
/// can enter, because a configuration it cannot hold is refused at the create call
/// itself rather than recorded and then failed.
pub const CONNECTOR_STATUS_ACTIVE: &str = "ACTIVE";

/// `ConnectorVpcLatticeEgressConfig.PortNumber`'s documented default ("Defaults to 22
/// if not specified"), which is also the SFTP port.
pub const DEFAULT_VPC_LATTICE_PORT: i32 = 22;

/// `SftpConnectorConfig.MaxConcurrentConnections`' documented default ("The default
/// value is `1`"), which the model also carries as `@default(1)`.
pub const DEFAULT_MAX_CONCURRENT_CONNECTIONS: i32 = 1;

/// `SftpConnectorConfig.MaxConcurrentConnections`' documented maximum ("The maximum
/// values is `5`").
///
/// The member's modelled `@range` carries only `min: 1`, so the boundary validator
/// refuses 0 and below but not 6 and above: the upper bound lives in the member's own
/// documentation, so it is checked by the create handler.
pub const MAX_CONCURRENT_CONNECTIONS: i32 = 5;

/// The not-found payload for a reference of `kind` the store could not resolve.
///
/// The identifier travels in `Resource` ALONE: `Message` is the fixed text the
/// service answers for that KIND of reference ([`NotFoundResource::message`]), so one
/// caller's id never reaches the message a client logs or asserts on, and the text
/// does not change when an internal name does. Every operation in this service that
/// cannot find a resource builds its error here, so the construction cannot drift
/// between them.
pub fn resource_not_found(kind: &NotFoundResource, resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: kind.message.to_string(),
        resource: resource.to_string(),
        resource_type: kind.resource_type.to_string(),
    }
}

/// The not-found payload for a `ServerId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_server(server_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SERVER, server_id)
}

/// The not-found payload for a `UserName` the named server does not hold.
pub fn unknown_user(user_name: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_USER, user_name)
}

/// The not-found payload for an `SshPublicKeyId` the named user does not hold —
/// never imported, or already deleted.
pub fn unknown_ssh_public_key(ssh_public_key_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SSH_PUBLIC_KEY, ssh_public_key_id)
}

/// The not-found payload for an `ExternalId` the named server holds no access grant
/// for — never granted, or already deleted.
pub fn unknown_access(external_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_ACCESS, external_id)
}

/// The not-found payload for a `ConnectorId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_connector(connector_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_CONNECTOR, connector_id)
}

/// The not-found payload for an `ExecutionId` the named workflow is not running — the
/// execution never started, or has already finished.
pub fn unknown_execution(execution_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_EXECUTION, execution_id)
}

/// The message the modelled `AccessDeniedException` carries.
///
/// It is the shape's own documented sentence
/// (`smithy_query(query="shape",shape="AccessDeniedException")`: "You do not have
/// sufficient access to perform this action."). AWS's own text names the caller's ARN
/// and the action, which is account- and principal-specific; the requirement is the
/// 403 classification and the code, so the modelled sentence is what this states, and
/// it deliberately says nothing about whether the named resource exists.
pub const INSUFFICIENT_ACCESS_MESSAGE: &str =
    "You do not have sufficient access to perform this action.";

/// The authorization refusal for a request this emulator cannot authorize.
///
/// Carries [`INSUFFICIENT_ACCESS_MESSAGE`] and nothing else, so the refusal is the
/// same for every request and discloses nothing about the resource it named. Returned
/// through the operation's own modelled `AccessDeniedException` variant, so it keeps
/// the model's HTTP 403.
///
/// WHICH identifier is judged here, and which is reported not found, is settled by the
/// resource a permission for the action NAMES: `SendWorkflowStepState` is addressed at
/// a workflow (its `ExecutionId` and `Token` address a run and a step INSIDE that
/// workflow), so an unresolvable `WorkflowId` is refused here and an execution the
/// resolved workflow is not running is reported not found. An operation whose input
/// names ONE resource and nothing inside it resolves that resource through the
/// modelled not-found instead (`DescribeWebApp`), so this refusal never makes an
/// operation's own `ResourceNotFoundException` unreachable.
pub fn access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(INSUFFICIENT_ACCESS_MESSAGE.to_string()),
    }
}

/// Whether `identity_provider_type` names a CUSTOMER-run identity provider — one the
/// server delegates authentication to — rather than Transfer's own user store.
///
/// The `IdentityProviderType` enum is `[SERVICE_MANAGED, API_GATEWAY,
/// AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`, and the three non-`SERVICE_MANAGED` values are
/// exactly the ones `IdentityProviderDetails` documents itself as required for: each
/// one points at something outside the service (a REST endpoint, a directory, a
/// function). `SERVICE_MANAGED` points at nothing: its users are the ones `CreateUser`
/// stores.
pub fn is_external_identity_provider(identity_provider_type: &str) -> bool {
    matches!(
        identity_provider_type,
        IDENTITY_PROVIDER_API_GATEWAY
            | IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE
            | IDENTITY_PROVIDER_AWS_LAMBDA
    )
}

/// `TestIdentityProvider`'s refusal for a server that delegates authentication to
/// nothing, so there is no identity provider for the call to test.
///
/// The wording is the model's own: "If you provide a server ID for a server that uses
/// service-managed users, you get an error: An error occurred
/// (InvalidRequestException) when calling the TestIdentityProvider operation:
/// s-server-ID not configured for external auth" — the one documented message of this
/// refusal, built from the `ServerId` the request named.
pub fn not_configured_for_external_auth(server_id: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: format!("{server_id} not configured for external auth"),
    }
}

/// The refusal for an access-grant request addressed at a server that has no
/// directory to hold groups.
///
/// An access grant IS a directory group: `ExternalId` is documented as "A unique
/// identifier that is required to identify specific groups within your directory",
/// read out of Active Directory with `Get-ADGroup`. Only a server whose
/// `IdentityProviderType` is `AWS_DIRECTORY_SERVICE` authenticates against a
/// directory, so on any other server there is no group an `ExternalId` could name —
/// which is a malformed request, not a missing grant. The message names the member and
/// the configuration the request was submitted under; both come from the request and
/// the stored server.
pub fn accesses_require_directory_provider(
    server_id: &str,
    identity_provider_type: &str,
) -> InvalidRequestException {
    InvalidRequestException {
        message: must_be_when(
            "IdentityProviderType",
            IDENTITY_PROVIDER_AWS_DIRECTORY_SERVICE,
            &format!(
                "an access is addressed by ExternalId on server {server_id}, \
                 whose IdentityProviderType is {identity_provider_type}"
            ),
        ),
    }
}

/// The not-found payload for a `WebAppId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_web_app(web_app_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_WEB_APP, web_app_id)
}

/// The not-found payload for a `ProfileId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_profile(profile_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_PROFILE, profile_id)
}

/// The already-exists payload for a `UserName` the named server already holds.
///
/// `Message` is the service's fixed text for the condition; the name travels in
/// `Resource`, which is where a caller reads which reference collided.
pub fn user_already_exists(user_name: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: USER_ALREADY_EXISTS.to_string(),
        resource: user_name.to_string(),
        resource_type: RESOURCE_TYPE_USER_LOWERCASE.to_string(),
    }
}

/// The already-exists payload for an SSH public key body the named user already
/// holds. Same construction as [`user_already_exists`]: fixed text, identifier in
/// `Resource`.
pub fn ssh_public_key_already_exists(user_name: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: SSH_PUBLIC_KEY_ALREADY_EXISTS.to_string(),
        resource: user_name.to_string(),
        resource_type: RESOURCE_TYPE_USER.to_string(),
    }
}

/// The not-found payload for a `WorkflowId` the store does not hold — never created,
/// or already deleted.
pub fn unknown_workflow(workflow_id: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_WORKFLOW, workflow_id)
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

/// A `SecurityPolicyName` the service publishes no policy for. The text names the
/// policy resource as the service's own wording does, like every other entry in the
/// not-found table.
pub const NOT_FOUND_SECURITY_POLICY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SECURITY_POLICY,
    message: "Unknown SecurityPolicy",
};

/// The not-found payload for a `SecurityPolicyName` the service publishes no policy
/// for. Built through [`resource_not_found`] like every other not-found in this
/// service, so the body cannot drift from them; "not found" here means "not in
/// [`SECURITY_POLICIES`]", never "not created yet".
pub fn unknown_security_policy(security_policy_name: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SECURITY_POLICY, security_policy_name)
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

/// One stored host key of a server, keyed by `(ServerId, HostKeyId)`.
///
/// The private material the import carried is NOT kept: nothing in this emulator can
/// use it (it serves no SSH transport) and no modelled member returns it. What the
/// readers report is derived from that material once, at import time — the genuine
/// OpenSSH fingerprint and algorithm name [`read_host_key`] parses out of it — so a
/// read reports a real property of a real key rather than an echo of the request.
#[derive(Debug, Clone)]
pub struct HostKey {
    /// The minted `hostkey-` + 17 hex-digit identifier (`^hostkey-[0-9a-f]{17}$`).
    pub host_key_id: String,
    /// `arn:aws:transfer:<region>:<account>:host-key/<ServerId>/<HostKeyId>`.
    pub arn: String,
    /// The Base64 SHA256 fingerprint of the imported key, in the `SHA256:<base64>`
    /// form `ssh-keygen -l` prints.
    pub fingerprint: String,
    /// `DescribedHostKey.Type`: the imported key's SSH algorithm name, one of the
    /// values that member documents (`ssh-rsa`, `ssh-ed25519`,
    /// `ecdsa-sha2-nistp256/384/521`).
    pub key_type: String,
    /// The caller's `Description`, absent when the import named none.
    pub description: Option<String>,
    /// `DateImported`: when this key was added to the server.
    pub date_imported: SystemTime,
    /// The resolved `HostKeyStorageType` (see [`DEFAULT_HOST_KEY_STORAGE_TYPE`]).
    pub storage_type: String,
    pub tags: Vec<Tag>,
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
/// Everything `DescribeProfile` and `ListProfiles` report is RESOLVED here, at create
/// time, so a reader projects the stored record without re-deriving anything: the
/// minted id, its ARN, the AS2-name, the profile type and the certificate ids and tags
/// the create carried.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The minted `p-` + 17 hex-digit identifier (`^p-([0-9a-f]{17})$`).
    pub profile_id: String,
    pub arn: String,
    /// The AS2-name (RFC 4130) exactly as the caller supplied it.
    pub as2_id: String,
    /// `LOCAL` or `PARTNER` — the `ProfileType` enum, enforced before a handler runs.
    pub profile_type: String,
    /// The certificate identifiers the create named, stored as given: this service
    /// holds no certificate records yet (`ImportCertificate` is still a scaffold with
    /// its own issue), so there is nothing to resolve them against.
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
    /// [`mint_host_key`]. Never invented: it is the real fingerprint of
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

/// One stored Transfer Family connector, keyed by `ConnectorId`.
///
/// A connector is a RECORD of the parameters of a connection to a partner's AS2 or
/// SFTP endpoint. The emulator speaks neither protocol, so nothing here is a live
/// connection; every member a reader reports is resolved once, when the connector is
/// created, so no reader has to re-apply a default. Every connector operation reads
/// this ONE record — the members a listing reports (`ListedConnector` is
/// `{Arn, ConnectorId, Url}`) and the ones only `DescribeConnector` reports live
/// together here rather than in a second map beside it.
#[derive(Debug, Clone)]
pub struct Connector {
    /// The minted `"c-" + 17 hex` identifier (`ConnectorId` is `^c-([0-9a-f]{17})$`).
    pub connector_id: String,
    /// `arn:aws:transfer:<region>:<account>:connector/<ConnectorId>`, as
    /// [`connector_arn`] builds it.
    pub arn: String,
    /// The partner endpoint, as supplied, when it names one.
    ///
    /// A blank `Url` names no endpoint (the member's `len 0..255` admits the empty
    /// string), which is the reading [`ConnectorConfig::cross_member_violation`] gives
    /// it too — so it is stored as absent, never as `""`. Absent exactly for a
    /// `VPC_LATTICE` connector, whose `Url` the model documents as "must be null":
    /// every other connector is refused without one.
    pub url: Option<String>,
    pub as2_config: Option<As2ConnectorConfig>,
    /// The SFTP settings, with `MaxConcurrentConnections` already resolved at create
    /// time by [`resolve_sftp_connector_config`].
    pub sftp_config: Option<SftpConnectorConfig>,
    pub access_role: String,
    pub logging_role: Option<String>,
    pub security_policy_name: Option<String>,
    /// The VPC Lattice egress settings, with `PortNumber` resolved at create time by
    /// [`resolve_connector_egress_config`]. `ConnectorEgressConfig` is a union with one
    /// member, so the variant is carried by [`Self::egress_type`] and this holds the
    /// settings themselves.
    pub egress_config: Option<ConnectorVpcLatticeEgressConfig>,
    pub egress_type: String,
    pub ip_address_type: Option<String>,
    pub status: String,
    pub tags: Vec<Tag>,
}

/// The `EgressType` a connector created with `egress_config` has.
///
/// `ConnectorEgressConfig`'s only member is `VpcLattice`, so a request that carries an
/// egress configuration is a `VPC_LATTICE` connector and one that carries none takes
/// `EgressType`'s modelled default. Both the create handler's cross-member checks and
/// the stored record read the type from here, so the two cannot disagree.
pub fn connector_egress_type(egress_config: Option<&ConnectorEgressConfig>) -> &'static str {
    match egress_config {
        Some(_) => CONNECTOR_EGRESS_TYPE_VPC_LATTICE,
        None => DEFAULT_CONNECTOR_EGRESS_TYPE,
    }
}

/// The endpoint a connector's `Url` names, when it names one.
///
/// A blank value names no endpoint: `Url` is modelled `len 0..255`, so the empty
/// string passes the boundary validator, and a connector that may not carry a URL at
/// all must not report one either. Only the emptiness test ignores surrounding
/// whitespace; a value that names an endpoint is kept exactly as the caller sent it.
/// Both the cross-member check and the stored record read the member through here, so
/// the two cannot disagree.
pub fn connector_url(url: Option<&str>) -> Option<String> {
    url.filter(|url| !url.trim().is_empty())
        .map(str::to_string)
}

/// Resolve the `SftpConnectorConfig` a connector is stored with.
///
/// `MaxConcurrentConnections` is the one member the model documents a default for, so
/// it is materialised HERE, once, at create time. The member is modelled non-optional
/// with `@default(1)`, so a request that omits it arrives as the schema's default
/// rather than as "absent", and its `@range{min: 1}` is refused at the boundary before
/// a handler runs: the floor below is therefore belt-and-braces, for a value that
/// reaches the store unset, and never a validation rule of its own.
pub fn resolve_sftp_connector_config(requested: SftpConnectorConfig) -> SftpConnectorConfig {
    SftpConnectorConfig {
        max_concurrent_connections: if requested.max_concurrent_connections
            >= DEFAULT_MAX_CONCURRENT_CONNECTIONS
        {
            requested.max_concurrent_connections
        } else {
            DEFAULT_MAX_CONCURRENT_CONNECTIONS
        },
        ..requested
    }
}

/// Resolve the egress settings a `VPC_LATTICE` connector is stored with: the Resource
/// Configuration as supplied, and `PortNumber` at its documented default when the
/// request named none.
pub fn resolve_connector_egress_config(
    requested: ConnectorEgressConfig,
) -> ConnectorVpcLatticeEgressConfig {
    match requested {
        ConnectorEgressConfig::VpcLattice(config) => ConnectorVpcLatticeEgressConfig {
            port_number: Some(config.port_number.unwrap_or(DEFAULT_VPC_LATTICE_PORT)),
            ..config
        },
    }
}

/// Apply the `UpdateConnectorEgressConfig` an update NAMES onto the egress settings the
/// connector already has — `Err(message)` when the connector has none.
///
/// `UpdateConnectorVpcLatticeEgressConfig` makes EVERY member optional where
/// `ConnectorVpcLatticeEgressConfig` requires the Resource Configuration ARN, so an
/// update MODIFIES an existing egress configuration member by member ("Updates the
/// egress configuration for the connector, allowing you to modify how traffic is
/// routed") — the same rule [`merge_protocol_details`] follows for a server, and the
/// create-time resolution runs over the result so the stored settings stay fully
/// materialised.
///
/// A connector with no egress configuration is a `SERVICE_MANAGED` one, and nothing in
/// the model names `EgressType` as an updatable member, so such a request has no egress
/// configuration to modify: it is refused with the condition the member is settable
/// under, the same reading [`ConnectorConfig::cross_member_violation`] gives
/// `EgressConfig` at create time.
pub fn merge_connector_egress_config(
    stored: Option<&ConnectorVpcLatticeEgressConfig>,
    requested: UpdateConnectorEgressConfig,
) -> Result<ConnectorVpcLatticeEgressConfig, String> {
    let UpdateConnectorEgressConfig::VpcLattice(requested) = requested;
    let Some(stored) = stored else {
        return Err(settable_only_when(
            "EgressConfig",
            &format!("EgressType is {CONNECTOR_EGRESS_TYPE_VPC_LATTICE}"),
        ));
    };
    Ok(resolve_connector_egress_config(
        ConnectorEgressConfig::VpcLattice(ConnectorVpcLatticeEgressConfig {
            resource_configuration_arn: requested
                .resource_configuration_arn
                .unwrap_or_else(|| stored.resource_configuration_arn.clone()),
            port_number: requested.port_number.or(stored.port_number),
        }),
    ))
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

/// The genuine fingerprint and SSH algorithm name of private key MATERIAL a caller
/// supplied, when the material is a key the emulator can read.
///
/// `None` for material in a format the emulator cannot read: it implements no
/// private-key format other than OpenSSH's (what `ssh-keygen` writes) and serves no
/// SSH transport, so there is nothing to compute a fingerprint or an algorithm name
/// from — and neither may be invented for material nothing parsed.
pub fn read_host_key(material: &str) -> Option<(String, String)> {
    let key = ::ssh_key::PrivateKey::from_openssh(material).ok()?;
    Some((
        key.fingerprint(::ssh_key::HashAlg::Sha256).to_string(),
        key.algorithm().as_str().to_string(),
    ))
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
    read_host_key(material).map(|(fingerprint, _algorithm)| fingerprint)
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
// The wording below names the member and, where the violated requirement is one the
// MODEL states about that member alone (a list's non-empty minimum, say), that
// requirement; otherwise the condition the member was submitted under. Both are built
// from the model's own names and its own closed value sets — never from a value the
// caller chose.
// =============================================================================

/// "`<member>` must not be empty" — a collection member the model gives a non-empty
/// minimum (`@length` min 1), required by a documented condition and sent empty or
/// absent. The requirement stated is the LIST's own minimum, which is the modelled
/// violation, rather than the other member whose value made the list required.
fn must_not_be_empty(member: &str) -> String {
    format!("{member} must not be empty")
}

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

/// "exactly one of `<first>` and `<second>` must be specified" — two members the model
/// documents as alternatives, exactly one of which configures the resource.
fn exactly_one_of(first: &str, second: &str) -> String {
    format!("exactly one of {first} and {second} must be specified")
}

/// "`<member>` must be at most `<maximum>`" — a member whose documented upper bound the
/// model's own constraints do not carry.
fn must_be_at_most(member: &str, maximum: i32) -> String {
    format!("{member} must be at most {maximum}")
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
    ///
    /// Those three are exactly the types that delegate authentication to something
    /// outside the service, so the enum membership is listed once, in
    /// [`is_external_identity_provider`], and read from here.
    fn needs_identity_provider_details(&self) -> bool {
        is_external_identity_provider(self.identity_provider_type)
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

/// The connector configuration a `CreateConnector` request resolves to: every member a
/// documented cross-member requirement mentions, with `EgressType` — which the rest are
/// stated against — already resolved from `EgressConfig`.
#[derive(Debug)]
pub struct ConnectorConfig<'a> {
    pub url: Option<&'a str>,
    pub as2_config: Option<&'a As2ConnectorConfig>,
    pub sftp_config: Option<&'a SftpConnectorConfig>,
    pub egress_type: &'a str,
}

impl ConnectorConfig<'_> {
    /// The first documented cross-member requirement this configuration violates,
    /// worded as the refusal states it — `None` when it violates none.
    ///
    /// Every rule below is a sentence in this service's own model:
    ///
    /// * `CreateConnector` states "You must specify exactly one configuration object:
    ///   either for AS2 (`As2Config`) or SFTP (`SftpConfig`)".
    /// * `EgressConfig` "determines how traffic is routed from the connector to the
    ///   SFTP server", and `ConnectorEgressType`'s `VPC_LATTICE` "routes traffic through
    ///   customer VPCs" to that same SFTP server — so only an SFTP connector has one.
    /// * `Url` states "When creating AS2 connectors or service-managed SFTP connectors
    ///   (connectors without egress configuration), you must provide a URL to specify
    ///   the remote server endpoint. For VPC Lattice type connectors, the URL must be
    ///   null."
    /// * `MaxConcurrentConnections` states "The maximum values is `5`" — a bound the
    ///   member's `@range` does not carry (see [`MAX_CONCURRENT_CONNECTIONS`]).
    pub fn cross_member_violation(&self) -> Option<String> {
        if self.as2_config.is_some() == self.sftp_config.is_some() {
            return Some(exactly_one_of("As2Config", "SftpConfig"));
        }

        let vpc_lattice = self.egress_type == CONNECTOR_EGRESS_TYPE_VPC_LATTICE;
        if vpc_lattice && self.sftp_config.is_none() {
            return Some(settable_only_when("EgressConfig", "SftpConfig is specified"));
        }

        // An empty `Url` names no endpoint — the same reading [`connector_url`] gives
        // it when the connector is stored, and the one `Certificate` already gets in
        // `ServerConfig::cross_member_violation`.
        let endpoint_named = connector_url(self.url).is_some();
        match (vpc_lattice, endpoint_named) {
            (true, true) => {
                return Some(forbidden_when(
                    "Url",
                    &format!("EgressType is {CONNECTOR_EGRESS_TYPE_VPC_LATTICE}"),
                ))
            }
            (false, false) => {
                return Some(required_when(
                    "Url",
                    &format!("EgressType is {CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED}"),
                ))
            }
            _ => {}
        }

        if self
            .sftp_config
            .is_some_and(|config| config.max_concurrent_connections > MAX_CONCURRENT_CONNECTIONS)
        {
            return Some(must_be_at_most(
                "SftpConfig.MaxConcurrentConnections",
                MAX_CONCURRENT_CONNECTIONS,
            ));
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
///
/// The missing-mappings direction states `HomeDirectoryMappings`' OWN modelled
/// minimum (`@length` min 1, max 50000), because that is the constraint an absent or
/// empty list violates; the opposite direction has no modelled constraint of its own
/// to state, so it names the condition the value was sent under.
pub fn home_directory_violation(
    home_directory_type: Option<&str>,
    home_directory_mappings: Option<&[HomeDirectoryMapEntry]>,
) -> Option<String> {
    let logical = home_directory_type == Some(HOME_DIRECTORY_TYPE_LOGICAL);
    let mapped = home_directory_mappings.is_some_and(|mappings| !mappings.is_empty());
    match (logical, mapped) {
        (true, false) => Some(must_not_be_empty("HomeDirectoryMappings")),
        (false, true) => Some(settable_only_when(
            "HomeDirectoryMappings",
            &format!("HomeDirectoryType is {HOME_DIRECTORY_TYPE_LOGICAL}"),
        )),
        _ => None,
    }
}

/// One stored directory access grant, keyed by `(ServerId, ExternalId)`.
///
/// An access grant is a GROUP in the directory the server authenticates against (see
/// [`accesses_require_directory_provider`]), so a grant exists only on a server whose
/// `IdentityProviderType` is `AWS_DIRECTORY_SERVICE`, and the server id is part of its
/// key: the same directory group may be granted access on several servers
/// independently. `BTreeMap` on that key so one server's grants are a contiguous,
/// ordered range for `ListAccesses`.
///
/// `DeleteAccess` reads only the grant's identity; the members below are the ones
/// `DescribedAccess` reports, which `CreateAccess` / `UpdateAccess` (their own stub
/// issues) resolve when they write a grant.
#[derive(Debug, Clone, Default)]
pub struct Access {
    pub server_id: String,
    pub external_id: String,
    pub home_directory: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory_mappings: Vec<HomeDirectoryMapEntry>,
    pub policy: Option<String>,
    pub posix_profile: Option<PosixProfile>,
    pub role: Option<String>,
}

/// One stored Transfer Family web app, keyed by `WebAppId`.
///
/// Every member a reader reports is RESOLVED here, at create time — the documented
/// `WebAppEndpointPolicy` default, the `EndpointType` the endpoint configuration
/// selects, the web app's own endpoint host name, the `AccessEndpoint` (the caller's
/// custom URL or the default one), and the IAM Identity Center application the create
/// registered. So `DescribeWebApp` projects the stored record without re-applying a
/// default, and the identity-provider and endpoint details are kept in the `Described*`
/// shapes the readers answer with. `ListWebApps` pages this map in `WebAppId` order and
/// projects `ListedWebApp` from the same record.
#[derive(Debug, Clone)]
pub struct WebApp {
    pub web_app_id: String,
    pub arn: String,
    /// The identity provider as the readers report it, with the `ApplicationArn` the
    /// create minted already in place.
    pub identity_provider_details: Option<DescribedWebAppIdentityProviderDetails>,
    /// The URL users are given for this web app: the caller's custom
    /// `AccessEndpoint`, or [`default_access_endpoint`] when the create named none.
    pub access_endpoint: String,
    /// The web app's own unique endpoint host name — see [`web_app_endpoint`].
    pub web_app_endpoint: String,
    /// `WebAppUnits` exactly as the caller set it. The model documents no default, so
    /// a create that named none leaves this `None` rather than inventing a unit count.
    pub web_app_units: Option<WebAppUnits>,
    pub web_app_endpoint_policy: String,
    /// `PUBLIC` or `VPC`, selected by whether the create configured a VPC endpoint.
    pub endpoint_type: String,
    /// The endpoint configuration as the readers report it, with the `VpcEndpointId`
    /// the create minted for a VPC-hosted web app. `None` for a `PUBLIC` web app,
    /// which has no VPC endpoint to report.
    pub endpoint_details: Option<DescribedWebAppEndpointDetails>,
    pub tags: Vec<Tag>,
    /// The branding `UpdateWebAppCustomization` assigns to this web app and
    /// `DescribeWebAppCustomization` reports — see [`WebAppCustomization`]. It lives on
    /// the web app rather than in a map of its own because a customization has no
    /// identifier of its own: it is addressed by `WebAppId`, and
    /// `DescribedWebAppCustomization.Arn` is documented as "the ARN for the web app",
    /// i.e. [`Self::arn`]. A web app no customization was assigned to carries the
    /// default (every member unset), which no `CreateWebApp` member can set.
    pub customization: WebAppCustomization,
}

/// One web app's customization: the page title, logo and icon a caller assigned to it.
///
/// `UpdateWebAppCustomization` is documented as "assigns new customization properties
/// to a web app. You can modify the icon file, logo file, and title", and models each
/// of those three members OPTIONAL beside the `@required` `WebAppId` — so an update
/// assigns the members it carries and leaves the others as they stand
/// ([`Self::assign`]); removing a customization is what the service's own
/// `DeleteWebAppCustomization` is for, not an omitted member. Each member is `None`
/// until an update assigns it, and `DescribeWebAppCustomization` reports exactly the
/// ones that were: `Title` is modelled `len 0..100`, so `Some(String::new())` is a
/// title a caller really did assign (the empty one) and is distinct from never having
/// assigned one, while `LogoFile`/`FaviconFile` are modelled `len 1..`, so no empty
/// blob can be stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebAppCustomization {
    pub title: Option<String>,
    pub logo_file: Option<Vec<u8>>,
    pub favicon_file: Option<Vec<u8>>,
}

impl WebAppCustomization {
    /// Assign the customization members an update CARRIES, leaving the ones it omits
    /// exactly as they stand.
    ///
    /// The one place this service applies the partial-update rule stated on
    /// [`WebAppCustomization`], so an update and any later reader cannot disagree about
    /// what an omitted member means.
    pub fn assign(
        &mut self,
        title: Option<String>,
        logo_file: Option<Vec<u8>>,
        favicon_file: Option<Vec<u8>>,
    ) {
        if let Some(title) = title {
            self.title = Some(title);
        }
        if let Some(logo_file) = logo_file {
            self.logo_file = Some(logo_file);
        }
        if let Some(favicon_file) = favicon_file {
            self.favicon_file = Some(favicon_file);
        }
    }
}

/// One workflow execution's custom-step callbacks, keyed by `(WorkflowId,
/// ExecutionId)`.
///
/// A custom workflow step hands its target resource an `ExecutionId`, a `WorkflowId`
/// and a `Token`, and the step stays pending until `SendWorkflowStepState` reports an
/// outcome for that token. An execution exists only while a workflow runs over an
/// uploaded file, so this is the state that callback is resolved against — and the
/// record `ListExecutions` projects one `ListedExecution` from.
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecution {
    pub workflow_id: String,
    pub execution_id: String,
    /// "the file location when the execution begins" (`ListedExecution`
    /// `InitialFileLocation`) — the uploaded file the run started on. `None` for a run
    /// recorded with no file location, which reports none rather than an empty
    /// structure.
    pub initial_file_location: Option<ExecutionFileLocation>,
    /// "the session details that are associated with a workflow": the server, user and
    /// session the upload that started this run arrived on. `None` when the record
    /// carries none.
    pub service_metadata: Option<ExecutionServiceMetadata>,
    /// The outcome reported for each pending custom step, by the `Token` that step was
    /// handed: `None` until its callback arrives, then the `CustomStepStatus` it
    /// carried. A token this map does not hold addresses no custom step of this
    /// execution.
    pub step_states: BTreeMap<String, Option<String>>,
}

impl WorkflowExecution {
    /// The `ExecutionStatus` a reader reports for this run, DERIVED from the custom-step
    /// callbacks recorded on it rather than stored — like `UserCount`, so it cannot go
    /// stale when a callback arrives.
    ///
    /// The custom-step callbacks are the WHOLE of what a run waits on: a custom step is
    /// the one step type that hands its target a `Token` and stays pending until
    /// `SendWorkflowStepState` reports an outcome for it, while every other
    /// `WorkflowStepType` (`COPY`, `TAG`, `DELETE`, `DECRYPT`) is applied by the service
    /// itself with nothing to wait for. So:
    ///
    /// * a step that reported `FAILURE` is the "exception encountered" the
    ///   `ExecutionStatus` value `EXCEPTION` names;
    /// * a run whose every custom step has reported `SUCCESS` has nothing left to wait
    ///   for, so it is `COMPLETED` — vacuously so for a run with no custom step at all,
    ///   which is a workflow of non-custom steps alone;
    /// * otherwise a step is still pending and the run is `IN_PROGRESS`.
    ///
    /// `HANDLING_EXCEPTION` is never reported: it is the window a run spends executing
    /// its `OnExceptionSteps`, and this emulator executes no step, so no record it holds
    /// is inside that window.
    pub fn reported_status(&self) -> &'static str {
        let reported_failure = self
            .step_states
            .values()
            .any(|outcome| outcome.as_deref() == Some(CUSTOM_STEP_STATUS_FAILURE));
        let every_step_succeeded = self
            .step_states
            .values()
            .all(|outcome| outcome.as_deref() == Some(CUSTOM_STEP_STATUS_SUCCESS));
        if reported_failure {
            EXECUTION_STATUS_EXCEPTION
        } else if every_step_succeeded {
            EXECUTION_STATUS_COMPLETED
        } else {
            EXECUTION_STATUS_IN_PROGRESS
        }
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

/// Per-service state for com-amazonaws-transfer-transferservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTransferTransferserviceState {
    /// Servers by `ServerId` (`"s-" + 17 hex`). `BTreeMap` so `ListServers` pages in a
    /// stable, resumable order.
    pub servers: BTreeMap<String, Server>,
    /// Users by `(ServerId, UserName)`, so one server's users are a contiguous,
    /// ordered range for `ListUsers`.
    pub users: BTreeMap<(String, String), User>,
    /// Directory access grants by `(ServerId, ExternalId)`, so one server's grants are
    /// a contiguous, ordered range for `ListAccesses`.
    pub accesses: BTreeMap<(String, String), Access>,
    /// Running workflow executions by `(WorkflowId, ExecutionId)`, so one workflow's
    /// executions are a contiguous, ordered range for `ListExecutions`.
    pub workflow_executions: BTreeMap<(String, String), WorkflowExecution>,
    /// Web apps by `WebAppId` (`"webapp-" + 17 hex`). `BTreeMap` so `ListWebApps` pages
    /// in a stable, resumable order.
    pub web_apps: BTreeMap<String, WebApp>,
    /// Host keys by `(ServerId, HostKeyId)`, so one server's keys are a contiguous,
    /// ordered range for `ListHostKeys`.
    pub host_keys: BTreeMap<(String, String), HostKey>,
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
    /// `ServerId` is `^s-([0-9a-f]{17})$`, `SshPublicKeyId` is `^key-[0-9a-f]{17}$`,
    /// `HostKeyId` is `^hostkey-[0-9a-f]{17}$`,
    /// `ConnectorId` is `^c-([0-9a-f]{17})$`, `ProfileId` is `^p-([0-9a-f]{17})$` and
    /// `WebAppId` is `^webapp-[0-9a-f]{17}$`,
    /// so all of them are this shape;
    /// `{:017x}` is exactly 17 lowercase hex digits for every
    /// counter value below 16^17, which a counter incremented once per created
    /// resource cannot reach.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.mint_hex_id(prefix, 17)
    }

    /// The ONE minting rule: bump the counter and render it as `<prefix>-<digits
    /// lowercase hex digits>`, zero-padded. Every minted identifier in this service is
    /// this shape and differs only in its prefix and width, so the counter bump and the
    /// formatting live here once.
    fn mint_hex_id(&mut self, prefix: &str, digits: usize) -> String {
        self.next_id_seq += 1;
        format!("{prefix}-{:0width$x}", self.next_id_seq, width = digits)
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

    /// The host keys stored for `server_id`, in `ListHostKeys` paging order.
    ///
    /// A RANGE scan over that server's contiguous run of the key map, on the same rule
    /// as [`Self::users_of`].
    pub fn host_keys_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a HostKey> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.host_keys
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, host_key)| host_key)
    }

    /// The executions stored for `workflow_id`, in `ListExecutions` paging order.
    ///
    /// A RANGE scan over that workflow's contiguous run of the execution map, on the
    /// same rule as [`Self::users_of`].
    pub fn executions_of<'a>(
        &'a self,
        workflow_id: &'a str,
    ) -> impl Iterator<Item = &'a WorkflowExecution> + 'a {
        use ::std::ops::Bound;
        let lower = (workflow_id.to_string(), String::new());
        self.workflow_executions
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((wid, _), _)| wid == workflow_id)
            .map(|(_, execution)| execution)
    }

    /// Whether `server_id` can take another host key, i.e. is below
    /// [`MAX_HOST_KEYS_PER_SERVER`].
    pub fn server_has_host_key_capacity(&self, server_id: &str) -> bool {
        self.host_keys_of(server_id).count() < MAX_HOST_KEYS_PER_SERVER
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

    /// Whether the store can take another web app, i.e. is below
    /// [`MAX_STORED_WEB_APPS`].
    pub fn has_web_app_capacity(&self) -> bool {
        self.web_apps.len() < MAX_STORED_WEB_APPS
    }

    /// Mint the `apl-<16 hex digits>` identifier of the IAM Identity Center
    /// application a `CreateWebApp` registers for the new web app.
    ///
    /// `IdentityCenterApplicationArn`'s modelled pattern ends in `apl-[a-zA-Z0-9]{16}`
    /// — 16 characters, one fewer than [`Self::mint_id`] emits — so this is
    /// [`Self::mint_hex_id`] at that width. It shares the one counter, so an
    /// application id is unique across this store like every other minted id.
    pub fn mint_identity_center_application_id(&mut self) -> String {
        self.mint_hex_id("apl", 16)
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

/// `arn:aws:transfer:<region>:<account>:webapp/<WebAppId>`.
pub fn web_app_arn(region: &str, web_app_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("webapp/{web_app_id}"))
}

/// The DNS label suffix of a web app's own endpoint host name.
///
/// `WebAppEndpoint` is documented as "the unique URL for your Transfer Family web app
/// ... the value that you use when you configure Origins on CloudFront", i.e. a host
/// name unique to the web app in its Region. The service publishes no host-name
/// grammar, so this suffix is the emulator's OWN: what matters, and what
/// [`web_app_endpoint`] guarantees, is that the name is derived from the web app's id
/// and Region and so is unique, stable across reads, and different for two web apps.
/// The emulator serves no web app UI, so nothing resolves this name.
const WEB_APP_ENDPOINT_SUFFIX: &str = "transfer-webapp";

/// `<WebAppId>.transfer-webapp.<region>.on.aws` — the web app's own endpoint host
/// name. Derived from the id and the Region, never stored input; see
/// [`WEB_APP_ENDPOINT_SUFFIX`].
pub fn web_app_endpoint(region: &str, web_app_id: &str) -> String {
    format!("{web_app_id}.{WEB_APP_ENDPOINT_SUFFIX}.{region}.on.aws")
}

/// The `AccessEndpoint` of a web app whose create named none.
///
/// `AccessEndpoint` is documented as "the URL that you provide to your users for them
/// to interact with the Transfer Family web app. You can specify a custom URL or use
/// the default value" — so when no custom URL is given the default is the `https://`
/// URL of the web app's own endpoint, the only URL the service has for the app.
pub fn default_access_endpoint(web_app_endpoint: &str) -> String {
    format!("https://{web_app_endpoint}")
}

/// The ARN of the IAM Identity Center application a `CreateWebApp` registers in the
/// Identity Center instance the caller named, or `None` when the caller named no
/// instance.
///
/// `DescribedIdentityCenterConfig.ApplicationArn` is documented as "the ARN for the
/// IAM Identity Center application: this value is set automatically when you create
/// your web app" — an application inside the given INSTANCE, which is why the
/// instance's own id is carried over from `InstanceArn` and only the `apl-` half is
/// minted. `None` when `InstanceArn` names no instance: there is then no instance to
/// register an application in, and nothing is invented. `InstanceArn`'s modelled
/// pattern (`^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`) is enforced
/// before a handler runs, so a present one always carries the `:instance/` segment
/// this reads.
pub fn identity_center_application_arn(
    instance_arn: &str,
    application_id: &str,
) -> Option<String> {
    let (_, instance) = instance_arn.split_once(":instance/")?;
    Some(::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/{application_id}"),
    ))
}

/// `arn:aws:transfer:<region>:<account>:host-key/<ServerId>/<HostKeyId>`.
///
/// The model declares no `aws.api#arn` template for a host key, so this follows the
/// shape its templates give every OTHER resource a server owns —
/// `user/{ServerId}/{UserName}`, `agreement/{ServerId}/{AgreementId}` — with the
/// resource name the API uses for this one.
pub fn host_key_arn(region: &str, server_id: &str, host_key_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("host-key/{server_id}/{host_key_id}"),
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

// ---------------------------------------------------------------------------
// What writing an access grant adds to the resource
// ---------------------------------------------------------------------------
//
// The access grant itself — [`Access`], the `accesses` map, `RESOURCE_TYPE_ACCESS`,
// `NOT_FOUND_ACCESS`/[`unknown_access`] and the directory-provider rule
// [`accesses_require_directory_provider`] every access operation refuses a
// non-directory server with — is declared above, beside the resources it sits with.
// `CreateAccess` is the first operation that WRITES a grant, so what a write needs and
// a delete did not is added here: the store bound, the already-exists payload for a
// grant the server already has, and the two lookups over one server's run of the map.

/// How many access grants ONE server holds before `CreateAccess` refuses with the
/// modelled `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_USERS_PER_SERVER`] — the emulator's own bound on
/// the store, not an account's quota. `DeleteAccess` releases one grant and
/// `DeleteServer` all of a server's.
pub const MAX_ACCESSES_PER_SERVER: usize = 1_000;

/// The fixed `Message` for a `CreateAccess` whose `ExternalId` the server already has
/// a grant for.
pub const ACCESS_ALREADY_EXISTS: &str = "Access already exists";

/// The already-exists payload for an `ExternalId` the named server already has a grant
/// for. Same construction as [`user_already_exists`]: the service's fixed text for the
/// condition, with the colliding identifier in `Resource` alone.
pub fn access_already_exists(external_id: &str) -> ResourceExistsException {
    ResourceExistsException {
        message: ACCESS_ALREADY_EXISTS.to_string(),
        resource: external_id.to_string(),
        resource_type: RESOURCE_TYPE_ACCESS.to_string(),
    }
}

impl ComAmazonawsTransferTransferserviceState {
    /// The access grants stored for `server_id`, in `ListAccesses` paging order.
    ///
    /// A RANGE scan over the contiguous run of the `(ServerId, ExternalId)` key, on
    /// the same basis as [`Self::users_of`].
    pub fn accesses_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a Access> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.accesses
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, access)| access)
    }

    /// Whether `server_id` can take another access grant, i.e. is below
    /// [`MAX_ACCESSES_PER_SERVER`].
    pub fn server_has_access_capacity(&self, server_id: &str) -> bool {
        self.accesses_of(server_id).count() < MAX_ACCESSES_PER_SERVER
    }
}

// ---------------------------------------------------------------------------
// The tag plane
// ---------------------------------------------------------------------------
//
// A tag set is not a resource of its own: every taggable resource already stores its
// own `tags`, written by the create (or import) that made it and reported by that
// resource's own reader. So `TagResource`/`UntagResource`/`ListTagsForResource` add no
// map here — they RESOLVE a resource by the ARN the caller sends and then read or
// rewrite that resource's existing set, which is why a tag written through
// `TagResource` is immediately visible to `DescribeServer`, `DescribeUser`,
// `DescribeHostKey`, `DescribeWebApp`, `DescribeProfile`, `DescribeConnector` and
// `DescribeWorkflow` without any of them changing.

/// How many tags ONE resource carries before a tag write is refused instead of
/// growing the set.
///
/// The modelled `Tags` list is `len 1..50`
/// (`smithy_query(query="shape",shape="Tags")`), and `Tags` is the shape EVERY reader
/// reports a resource's tag set through — `ListTagsForResource`'s output member and
/// each `Describe*`'s — so a set past 50 could not be reported without violating the
/// model. That makes 50 the bound here, and it is also the AWS-wide limit of 50 tags
/// per resource. `UntagResource` releases capacity again.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `ResourceType` for a resource addressed by its ARN alone.
///
/// A tag write resolves its target by ARN, so when nothing in the store answers to
/// that ARN the service has no kind to name for it: the ARN may name a kind this
/// emulator does not hold, or no Transfer resource at all. This is the emulator's own
/// token for that case, on the same capitalized convention as
/// [`RESOURCE_TYPE_SERVER`]; the ARN the caller sent travels in `Resource`, never in
/// the message.
pub const RESOURCE_TYPE_RESOURCE: &str = "Resource";

/// An ARN no stored resource answers to. Its own entry in the not-found table, like
/// every other kind.
pub const NOT_FOUND_TAGGED_RESOURCE: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_RESOURCE,
    message: "Unknown resource",
};

/// The not-found payload for a tag write whose `Arn` names no stored resource.
///
/// Built through [`resource_not_found`] like every other not-found in this service,
/// so the ARN travels in `Resource` alone and the `Message` is the fixed text for the
/// condition.
pub fn unknown_tagged_resource(arn: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_TAGGED_RESOURCE, arn)
}

/// The refusal for a tag write that would push a resource past
/// [`MAX_TAGS_PER_RESOURCE`]: a FIXED text for the condition, with the bound it
/// states and no request data.
pub fn too_many_tags() -> InvalidRequestException {
    InvalidRequestException {
        message: format!("A resource cannot carry more than {MAX_TAGS_PER_RESOURCE} tags"),
    }
}

/// The tag set the merge of `incoming` into `tags` would have, as a COUNT, without
/// touching either.
///
/// A tag key identifies one tag on a resource, so a key already present is replaced
/// rather than added: only the keys that are new to `tags` grow the set, and a key
/// repeated inside `incoming` grows it once. A write consults this BEFORE mutating,
/// so a refused write leaves the set exactly as it was.
pub fn merged_tag_count(tags: &[Tag], incoming: &[Tag]) -> usize {
    let present: ::std::collections::BTreeSet<&str> =
        tags.iter().map(|tag| tag.key.as_str()).collect();
    let added: ::std::collections::BTreeSet<&str> = incoming
        .iter()
        .map(|tag| tag.key.as_str())
        .filter(|key| !present.contains(key))
        .collect();
    present.len() + added.len()
}

/// Merge `incoming` into `tags`: a key the resource already carries takes the
/// incoming value IN PLACE, a key it does not carry is appended.
///
/// A tag key is unique per resource (tags are a key-value map, not a multiset), so
/// tagging with a key that is already there is an update and not a second tag — which
/// is also why a key repeated within one request leaves the LAST value sent.
pub fn merge_tags(tags: &mut Vec<Tag>, incoming: Vec<Tag>) {
    for tag in incoming {
        match tags.iter_mut().find(|present| present.key == tag.key) {
            Some(present) => present.value = tag.value,
            None => tags.push(tag),
        }
    }
}

/// Detach every tag whose key is in `keys`, keeping the rest in their stored order.
///
/// A key the resource does not carry removes nothing and is not an error: the request
/// asks for those keys to be absent, and after it they are.
pub fn remove_tags(tags: &mut Vec<Tag>, keys: &[String]) {
    tags.retain(|tag| !keys.contains(&tag.key));
}

impl ComAmazonawsTransferTransferserviceState {
    /// Every stored resource that carries a tag set, as `(ARN, tags)` pairs.
    ///
    /// The ONE list of what is taggable in this service, and the order
    /// [`Self::tags_mut`] resolves in. A resource kind becomes taggable by storing an
    /// `arn` and a `tags` of its own and being added to both.
    fn taggable(&self) -> impl Iterator<Item = (&str, &[Tag])> {
        let servers = self
            .servers
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let users = self
            .users
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let host_keys = self
            .host_keys
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let web_apps = self
            .web_apps
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let profiles = self
            .profiles
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let connectors = self
            .connectors
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        let workflows = self
            .workflows
            .values()
            .map(|r| (r.arn.as_str(), r.tags.as_slice()));
        servers
            .chain(users)
            .chain(host_keys)
            .chain(web_apps)
            .chain(profiles)
            .chain(connectors)
            .chain(workflows)
    }

    /// The tag set of the resource `arn` identifies, or `None` when no stored resource
    /// answers to that ARN.
    ///
    /// Matched against each resource's OWN stored ARN, so the region and account id in
    /// the ARN are part of the identity: an ARN that differs from the one the create
    /// returned names no resource, rather than resolving by id alone.
    pub fn tags_of(&self, arn: &str) -> Option<&[Tag]> {
        self.taggable()
            .find(|(stored, _)| *stored == arn)
            .map(|(_, tags)| tags)
    }

    /// The tag set of the resource `arn` identifies, for a write.
    ///
    /// The same resolution as [`Self::tags_of`] over the same maps in the same order;
    /// an ARN is unique across the store, so at most one record answers to it.
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(found) = self.servers.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.users.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.host_keys.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.web_apps.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.profiles.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.connectors.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        if let Some(found) = self.workflows.values_mut().find(|r| r.arn == arn) {
            return Some(&mut found.tags);
        }
        None
    }
}
