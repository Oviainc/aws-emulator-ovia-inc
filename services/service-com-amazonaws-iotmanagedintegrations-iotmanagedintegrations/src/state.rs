// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotmanagedintegrations-iotmanagedintegrations service.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AuthConfig, AuthMaterial, CapabilityReport, CapabilitySchemaItem, ConfigurationStatus,
    EndpointConfig, GeneralAuthorizationName, PushConfig, ResourceNotFoundException,
    SecretsManager, WiFiSimpleSetupConfiguration,
};

/// The region an ARN carries when the request names none (the emulator's default
/// region, as documented on [`engine::emulator_arn`]).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `aws.auth#sigv4` name, which is also the ARN service segment the
/// `ManagedThingArn` pattern (`^arn:aws:iotmanagedintegrations:…`) requires.
pub const SERVICE_ARN_NAMESPACE: &str = "iotmanagedintegrations";

/// The `Role`, `AuthMaterialType`, `AuthType`, `HubNetworkMode`, `EncryptionType`,
/// `EndpointType`, `LogLevel`, `ConfigurationState` and `AssociationState` value lists the
/// handlers validate against and report, re-exported from the MODEL-GENERATED constants in
/// `crate::ops::types` so there is no second copy to drift from the model.
pub use crate::ops::types::{
    ASSOCIATION_STATE_VALUES, AUTH_MATERIAL_TYPE_VALUES, AUTH_TYPE_VALUES,
    CONFIGURATION_STATE_VALUES, ENCRYPTION_TYPE_VALUES, ENDPOINT_TYPE_VALUES,
    HUB_NETWORK_MODE_VALUES, LOG_LEVEL_VALUES, ROLE_VALUES,
};

/// The `ProvisioningStatus` a freshly created managed thing carries: it is not yet
/// associated with a physical device. Leaving this state needs the device to be
/// onboarded (a discovery or provisioning flow), not the passage of time, so no
/// transition out of it is owed by the clock. The full value list is
/// `crate::ops::types::PROVISIONING_STATUS_VALUES`.
pub const INITIAL_PROVISIONING_STATUS: &str = "UNASSOCIATED";

/// One stored managed thing: the authoritative record every managed-thing operation
/// reads and writes. `CreateManagedThing` inserts it, `GetManagedThing` /
/// `ListManagedThings` project it onto their output shapes, `UpdateManagedThing`
/// mutates it in place and `DeleteManagedThing` removes it.
#[derive(Debug, Clone)]
pub struct ManagedThing {
    /// The generated `ManagedThingId` this record is keyed by.
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A managed thing is a regional
    /// resource: only that region's endpoint resolves, lists, updates or deletes it.
    pub region: String,
    /// `Role` enum value (`CONTROLLER` | `DEVICE`).
    pub role: String,
    /// `ProvisioningStatus` enum value.
    pub provisioning_status: String,
    /// `AuthenticationMaterial` — `@sensitive`, so it is stored but never returned by
    /// any modelled output shape.
    pub authentication_material: String,
    /// `AuthMaterialType` enum value.
    pub authentication_material_type: String,
    pub owner: Option<String>,
    pub credential_locker_id: Option<String>,
    pub serial_number: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
    pub classification: Option<String>,
    pub capabilities: Option<String>,
    pub capability_report: Option<CapabilityReport>,
    pub capability_schemas: Vec<CapabilitySchemaItem>,
    /// `HubNetworkMode` enum value, set only by `UpdateManagedThing`.
    pub hub_network_mode: Option<String>,
    pub wifi_simple_setup_configuration: Option<WiFiSimpleSetupConfiguration>,
    pub meta_data: HashMap<String, String>,
    /// The named fields of the device bar code the create call supplied as
    /// `AuthenticationMaterial`, as [`bar_code_fields`] read them. A record's device
    /// attributes are projected from here when its `MetaData` names none — see
    /// [`DeviceAttribute`]. The material itself stays in
    /// [`Self::authentication_material`] and is never returned.
    pub bar_code_fields: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the thing was activated against a physical device. `None` until an
    /// onboarding flow activates it; no operation in this slice does.
    pub activated_at: Option<SystemTime>,
    /// Monotonic insertion sequence. `ListManagedThings` pages by it, so a page token
    /// keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// The `CloudConnectorType` every connector this service creates carries.
///
/// `CloudConnectorType` is `LISTED | UNLISTED`
/// (`smithy_query(query="shape",shape="CloudConnectorType")`): a LISTED connector is one
/// of the C2C connectors published for every account, which NO operation of this service
/// creates, and an UNLISTED one is the connector an account creates for itself.
/// `CreateCloudConnector` — "Creates a C2C (cloud-to-cloud) connector", and the only way a
/// record enters [`ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState::cloud_connectors`]
/// — is that call, so a stored connector reports this type.
pub const CREATED_CLOUD_CONNECTOR_TYPE: &str = "UNLISTED";

/// One stored C2C (cloud-to-cloud) cloud connector: the authoritative record
/// `CreateCloudConnector` inserts and `GetCloudConnector` projects onto its output shape.
///
/// It holds no timestamp because no modelled cloud-connector shape reports one
/// (`smithy_query(query="operation",operation="GetCloudConnector")`); the instant a
/// create happened is kept on its idempotency record, where the TTL needs it.
#[derive(Debug, Clone)]
pub struct CloudConnector {
    /// The generated `CloudConnectorId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. A cloud connector is a regional
    /// resource: only that region's endpoint resolves it.
    pub region: String,
    /// `DisplayName` — the connector's display name.
    pub name: String,
    pub description: Option<String>,
    /// The endpoint the connector reaches the third-party cloud through.
    pub endpoint_config: EndpointConfig,
    /// `EndpointType` enum value, absent when the create named none.
    pub endpoint_type: Option<String>,
    /// `CloudConnectorType` enum value — see [`CREATED_CLOUD_CONNECTOR_TYPE`].
    pub connector_type: String,
}

/// The most cloud connectors one store holds. A create past it is refused rather than
/// grown into, so a caller cannot make this emulator's memory grow without end. A bound
/// CHOSEN for the emulator, not a published AWS quota, like [`MAX_MANAGED_THINGS`].
pub const MAX_CLOUD_CONNECTORS: usize = 1_000;

/// The `ResourceType` this service reports a cloud connector under — the Smithy resource
/// name, which is also what `ResourceNotFoundException`'s `ResourceType` member carries
/// and the kind an idempotency token is scoped to ([`IdempotencyKey`]).
pub const CLOUD_CONNECTOR_RESOURCE_TYPE: &str = "CloudConnector";

/// The cloud-connector resource as a READ's reason names it — the resource's name in
/// words, which is what that sentence states instead of the identifier
/// ([`resource_not_found_message`]).
pub const CLOUD_CONNECTOR_RESOURCE_NAME: &str = "Cloud Connector";

/// One stored connector destination: the record `CreateConnectorDestination` inserts —
/// "a connector destination for connecting a cloud-to-cloud (C2C) connector to the
/// customer's Amazon Web Services account" — and `GetConnectorDestination` projects onto
/// its output shape.
///
/// It holds no timestamp because no modelled connector-destination shape reports one
/// (`smithy_query(query="operation",operation="GetConnectorDestination")`); the instant a
/// create happened is kept on its idempotency record, where the TTL needs it.
#[derive(Debug, Clone)]
pub struct ConnectorDestination {
    /// The generated `ConnectorDestinationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. A connector destination is a
    /// regional resource, like every other resource of this service: only that region's
    /// endpoint resolves it.
    pub region: String,
    /// The `CloudConnectorId` of the C2C connector this destination connects — required
    /// on the create, and RESOLVED there against [`Self::region`]'s cloud connectors, so
    /// every stored record names a connector that existed when it was created.
    pub cloud_connector_id: String,
    /// `ConnectorDestinationName`, absent when the create named none.
    pub name: Option<String>,
    /// `ConnectorDestinationDescription`, absent when the create named none.
    pub description: Option<String>,
    /// `AuthType` enum value (one of [`AUTH_TYPE_VALUES`]), absent when the create named
    /// none.
    pub auth_type: Option<String>,
    /// `AuthConfig` — `@required` on the create, stored exactly as it was supplied so a
    /// read reports the authentication configuration the caller submitted.
    pub auth_config: AuthConfig,
    /// `SecretsManager`, absent when the create named none.
    pub secrets_manager: Option<SecretsManager>,
    /// Monotonic insertion sequence, which is also what makes [`Self::id`] unique.
    pub sequence: u64,
}

impl ConnectorDestination {
    /// The `OAuthCompleteRedirectUrl` a read of this destination reports: the URL the
    /// OAuth configuration of its [`Self::auth_config`] names as where users are
    /// redirected after completing authorization
    /// (`smithy_query(query="shape",shape="OAuthConfig")` -> `oAuthCompleteRedirectUrl`),
    /// which is the same fact the output's own `OAuthCompleteRedirectUrl` member states.
    ///
    /// Reported only where that value satisfies the OUTPUT member's modelled constraints
    /// (`OAuthCompleteRedirectUrl`, len 1..1024, `^(http|https)://.*$`): the member it
    /// was supplied on targets an unconstrained `String`, so a value this one does not
    /// admit stays on the `AuthConfig` the read reports in full rather than being
    /// published on a member whose shape forbids it.
    pub fn o_auth_complete_redirect_url(&self) -> Option<String> {
        self.auth_config
            .o_auth
            .as_ref()?
            .o_auth_complete_redirect_url
            .as_deref()
            .filter(|url| {
                (url.starts_with("http://") || url.starts_with("https://"))
                    && (1..=MAX_O_AUTH_COMPLETE_REDIRECT_URL_LENGTH).contains(&url.chars().count())
            })
            .map(str::to_string)
    }

    /// Write `material` into this destination's General Authorization list under its
    /// `AuthMaterialName`: a name the list already holds is REPLACED, a name it does not
    /// hold is appended — what `GeneralAuthorizationUpdate` carries is "authorization
    /// materials to add or update"
    /// (`smithy_query(query="shape",shape="GeneralAuthorizationUpdate")`), the material's
    /// name being what says which of the two it is.
    ///
    /// Answers `false` when appending would take the list past
    /// [`MAX_GENERAL_AUTHORIZATION_MATERIALS`], so a caller refuses the request rather
    /// than storing a list the MODEL forbids; nothing is written in that case.
    pub fn upsert_auth_material(&mut self, material: AuthMaterial) -> bool {
        let materials = self.auth_config.general_authorization.get_or_insert_default();
        let held = materials
            .iter()
            .position(|held| held.auth_material_name == material.auth_material_name);
        match held {
            Some(at) => materials[at] = material,
            None if materials.len() < MAX_GENERAL_AUTHORIZATION_MATERIALS => {
                materials.push(material);
            }
            None => return false,
        }
        true
    }
}

/// The most `AuthMaterial`s one `AuthConfig` holds for General Authorization, as the model
/// declares it (`smithy_query(query="shape",shape="AuthConfig")` ->
/// `GeneralAuthorization` length max 3, the bound `AuthMaterials` itself carries).
pub const MAX_GENERAL_AUTHORIZATION_MATERIALS: usize = 3;

/// The `@length` maximum of `OAuthCompleteRedirectUrl`, in characters
/// (`smithy_query(query="shape",shape="OAuthCompleteRedirectUrl")` reports len 1..1024).
pub const MAX_O_AUTH_COMPLETE_REDIRECT_URL_LENGTH: usize = 1024;

/// The most connector destinations one store holds. A create past it is refused rather
/// than grown into, so a caller cannot make this emulator's memory grow without end. A
/// bound CHOSEN for the emulator, not a published AWS quota, like [`MAX_MANAGED_THINGS`].
pub const MAX_CONNECTOR_DESTINATIONS: usize = 1_000;

/// The `ResourceType` this service reports a connector destination under — the Smithy
/// resource name, which is also what `ResourceNotFoundException`'s `ResourceType` member
/// carries and the kind an idempotency token is scoped to ([`IdempotencyKey`]).
pub const CONNECTOR_DESTINATION_RESOURCE_TYPE: &str = "ConnectorDestination";

/// The connector-destination resource as a READ's reason names it — the resource's name in
/// words, which is what that sentence states instead of the identifier
/// ([`resource_name_not_found_message`]).
pub const CONNECTOR_DESTINATION_RESOURCE_NAME: &str = "Connector Destination";

/// The connector-destination resource as a MUTATION's reason names it: the same name in
/// SENTENCE case, because that reason is a sentence about it
/// ([`cannot_be_found_message`]) rather than a label like the read's.
pub const CONNECTOR_DESTINATION_RESOURCE_SUBJECT: &str = "Connector destination";

/// One stored over-the-air (OTA) task configuration: the authoritative record every
/// OTA-task-configuration operation reads and writes. `CreateOtaTaskConfiguration`
/// inserts it and `GetOtaTaskConfiguration` projects it onto its output shape.
#[derive(Debug, Clone)]
pub struct OtaTaskConfiguration {
    /// The generated `OtaTaskConfigurationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. An OTA task configuration is a
    /// regional resource: only that region's endpoint resolves it.
    pub region: String,
    /// `OtaTaskConfigurationName` — `@sensitive`, and absent when the create named none
    /// (the model marks no member of the create request `@required`).
    pub name: Option<String>,
    /// `OtaDescription`.
    pub description: Option<String>,
    /// `PushConfig`, stored exactly as the create supplied it so a read reports the
    /// configuration the caller submitted.
    pub push_config: Option<PushConfig>,
    pub created_at: SystemTime,
    /// Monotonic insertion sequence, taken from [`Self::take_sequence`], so the records
    /// have a deterministic order independent of the hash map.
    pub sequence: u64,
}

/// The `EncryptionType` an account's managed-integrations data is encrypted with until a
/// `PutDefaultEncryptionConfiguration` sets another: the service's own key, which needs no
/// `KmsKeyArn`. A member of [`ENCRYPTION_TYPE_VALUES`].
pub const DEFAULT_ENCRYPTION_TYPE: &str = "MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION";

/// The `EncryptionType` that encrypts with a customer managed AWS KMS key — the type
/// `KmsKeyArn` belongs to. A member of [`ENCRYPTION_TYPE_VALUES`].
pub const CUSTOMER_KEY_ENCRYPTION_TYPE: &str = "CUSTOMER_KEY_ENCRYPTION";

/// The `ConfigurationState` a default encryption configuration carries once the
/// encryption it names is in force. A member of [`CONFIGURATION_STATE_VALUES`].
pub const CONFIGURATION_STATE_ENABLED: &str = "ENABLED";

/// The `ConfigurationState` a default encryption configuration carries while the switch a
/// put started is not in force yet. A member of [`CONFIGURATION_STATE_VALUES`].
pub const CONFIGURATION_STATE_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// How long after a `PutDefaultEncryptionConfiguration` that CHANGES the configuration the
/// new encryption is in force.
///
/// The model declares `UPDATE_IN_PROGRESS` beside `ENABLED` on `ConfigurationState`, so
/// changing the key an account's data is encrypted under is an ASYNCHRONOUS change: the
/// call that starts it answers with a configuration that is not in force yet, and the
/// clock completes it. AWS publishes no duration for it, so this span is CHOSEN — inside
/// the emulator's 250..750 ms window, so a caller that reads straight after the put still
/// observes `UPDATE_IN_PROGRESS`. It is stored as a due instant and never slept on.
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// The most regions one store keeps a default encryption configuration for.
///
/// The configuration is keyed by the region the request was SIGNED for, which comes from
/// the caller, so past this bound a put for a region the store does not already hold is
/// refused rather than grown into: a caller that varies its signing region cannot make
/// this emulator's memory grow without end. A bound CHOSEN for the emulator (AWS publishes
/// no such quota, and offers far fewer regions than this), like [`MAX_MANAGED_THINGS`].
pub const MAX_CONFIGURED_REGIONS: usize = 64;

/// The longest region name this service will key a stored configuration by, in
/// characters: longer than every region AWS publishes (`us-gov-east-1` is 13), and short
/// enough that a stored key is bounded in size as well as in count.
pub const MAX_REGION_NAME_LENGTH: usize = 32;

/// Whether `region` is shaped like an AWS region name — 1..=[`MAX_REGION_NAME_LENGTH`]
/// characters of lowercase letters, digits and hyphens.
///
/// [`request_region`] reads the region out of the request's SigV4 credential scope, so an
/// operation that keys STATE by it checks the shape first: a configuration is then never
/// held under an arbitrary caller-supplied string.
pub fn is_region_name(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME_LENGTH
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The default encryption configuration of this account in ONE region: what the last
/// `PutDefaultEncryptionConfiguration` wrote, and when the switch it started comes into
/// force.
#[derive(Debug, Clone)]
pub struct DefaultEncryptionConfiguration {
    /// `EncryptionType` enum value.
    pub encryption_type: String,
    /// The customer managed KMS key the data is encrypted under. Set only with
    /// [`CUSTOMER_KEY_ENCRYPTION_TYPE`]: the service-owned encryption uses no key of the
    /// caller's, so it stores none.
    pub kms_key_arn: Option<String>,
    /// The instant the change the last put started comes into force, or `None` when
    /// nothing is changing. Before it the configuration reports
    /// [`CONFIGURATION_STATE_UPDATE_IN_PROGRESS`], from it
    /// [`CONFIGURATION_STATE_ENABLED`] — see [`Self::state_at`]. Storing the instant is
    /// what keeps a read a pure read: the state is DERIVED from the clock, so nothing has
    /// to be written when the switch becomes due.
    pub update_due_at: Option<SystemTime>,
}

impl Default for DefaultEncryptionConfiguration {
    /// The configuration an account has in a region no call has configured: managed
    /// integrations' own key, no customer key, and nothing in progress.
    fn default() -> Self {
        Self {
            encryption_type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key_arn: None,
            update_due_at: None,
        }
    }
}

impl DefaultEncryptionConfiguration {
    /// The `ConfigurationState` this configuration is in at `now`: the switch a put
    /// started is in progress until its due instant, and enabled from it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.update_due_at.is_some_and(|due| now < due) {
            CONFIGURATION_STATE_UPDATE_IN_PROGRESS
        } else {
            CONFIGURATION_STATE_ENABLED
        }
    }

    /// The modelled `ConfigurationStatus` of this configuration at `now`.
    ///
    /// `Error` stays absent: it describes a FAILED configuration update
    /// (`smithy_query(query="shape",shape="ConfigurationStatus")`), and no switch this
    /// emulator performs fails.
    pub fn status_at(&self, now: SystemTime) -> ConfigurationStatus {
        ConfigurationStatus {
            error: None,
            state: self.state_at(now).to_string(),
        }
    }

    /// Whether putting `encryption_type` with `kms_key_arn` would CHANGE this
    /// configuration — which is what decides whether a put starts a switch at all.
    pub fn would_change(&self, encryption_type: &str, kms_key_arn: Option<&str>) -> bool {
        self.encryption_type != encryption_type || self.kms_key_arn.as_deref() != kms_key_arn
    }
}

/// One stored event log configuration: the logging level this account has in force for ONE
/// log target. `CreateEventLogConfiguration` inserts it and `GetEventLogConfiguration`
/// projects it onto its output shape.
#[derive(Debug, Clone)]
pub struct EventLogConfiguration {
    /// The generated `LogConfigurationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. Every resource of this service is
    /// regional (see [`ManagedThing::region`]): only that region's endpoint resolves this
    /// configuration.
    pub region: String,
    /// `SmartHomeResourceType`: a resource type, or `*` for the whole account.
    pub resource_type: String,
    /// `SmartHomeResourceId`, absent when the configuration addresses every resource of
    /// [`Self::resource_type`] rather than one of them.
    pub resource_id: Option<String>,
    /// `LogLevel` enum value, one of [`LOG_LEVEL_VALUES`].
    pub event_log_level: String,
    /// Monotonic insertion sequence, which is also what makes [`Self::id`] unique. A later
    /// `ListEventLogConfigurations` pages by it, as `ListManagedThings` does.
    pub sequence: u64,
}

impl EventLogConfiguration {
    /// The log TARGET this configuration addresses: the resource type it names, and the
    /// resource within that type when it names one. The target is what an event log
    /// configuration configures, so two records with the same target would be two
    /// configurations of one thing — which is what makes a second create for a target a
    /// conflict rather than another record.
    pub fn target(&self) -> (&str, Option<&str>) {
        (self.resource_type.as_str(), self.resource_id.as_deref())
    }
}

/// The `ResourceType` this service reports an event log configuration under — the Smithy
/// resource name, which is what `ResourceNotFoundException`'s `ResourceType` member
/// carries, as [`MANAGED_THING_RESOURCE_TYPE`] does for a managed thing.
pub const EVENT_LOG_CONFIGURATION_RESOURCE_TYPE: &str = "EventLogConfiguration";

/// The event-log-configuration resource as a READ's reason names it — the resource's name
/// in words ([`resource_not_found_message`]), as [`MANAGED_THING_RESOURCE_NAME`] is for a
/// managed thing.
pub const EVENT_LOG_CONFIGURATION_RESOURCE_NAME: &str = "Event Log Configuration";

/// The most event log configurations one store holds. A create past it is refused rather
/// than grown into, so a caller cannot make this emulator's memory grow without end. A
/// bound CHOSEN for the emulator, like [`MAX_MANAGED_THINGS`], not a published AWS quota.
pub const MAX_EVENT_LOG_CONFIGURATIONS: usize = 1_000;

/// One stored account association: the authoritative record every account-association
/// operation reads and writes. `CreateAccountAssociation` inserts it, and
/// `GetAccountAssociation` / `ListAccountAssociations` project it onto their output
/// shapes.
#[derive(Debug, Clone)]
pub struct AccountAssociation {
    /// The generated `AccountAssociationId` this record is keyed by.
    pub id: String,
    /// `AccountAssociationArn`, as [`account_association_arn`] builds it.
    pub arn: String,
    /// The region the creating request was signed for. An account association is a
    /// regional resource, like every other resource of this service: only that region's
    /// endpoint resolves or lists it.
    pub region: String,
    /// The `ConnectorDestinationId` the create named — required there, so every record
    /// carries one. `ListAccountAssociations` filters on it.
    pub connector_destination_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `AssociationState` enum value, one of [`ASSOCIATION_STATE_VALUES`]. A fresh record
    /// carries [`INITIAL_ASSOCIATION_STATE`].
    pub association_state: String,
    /// `AccountAssociationErrorMessage` — why the association failed. `None` until an
    /// association enters `ASSOCIATION_FAILED`, which no operation in this slice does.
    pub error_message: Option<String>,
    /// `OAuthAuthorizationUrlOutput` — the third-party OAuth authorization URL the end
    /// user authenticates at. Always [`NO_OAUTH_AUTHORIZATION_URL`] here; see that
    /// constant for why.
    pub o_auth_authorization_url: String,
    /// `GeneralAuthorizationName` exactly as the create supplied it, so a read reports
    /// the authorization material the caller named.
    pub general_authorization: Option<GeneralAuthorizationName>,
    /// `TagsMap` — `@sensitive`, stored as the create supplied it and reported by
    /// `GetAccountAssociation`.
    pub tags: HashMap<String, String>,
    /// Monotonic insertion sequence. `ListAccountAssociations` pages by it, so a page
    /// token keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// The `AssociationState` a freshly created account association carries.
///
/// `CreateAccountAssociation` answers with an `OAuthAuthorizationUrl` that is
/// "backed with all the required parameters to perform end-user authentication", so the
/// association is complete only once the END USER has authenticated at the third-party
/// platform — an external event, not the passage of time, so no transition out of this
/// state is owed by the clock (the same reason [`INITIAL_PROVISIONING_STATUS`] is where a
/// managed thing stays until a device is onboarded). A member of
/// [`ASSOCIATION_STATE_VALUES`].
pub const INITIAL_ASSOCIATION_STATE: &str = "ASSOCIATION_IN_PROGRESS";

/// The `OAuthAuthorizationUrl` this emulator reports for an account association: none.
///
/// The URL AWS answers with points at the third-party platform's OAuth authorization
/// server, "backed with all the required parameters to perform end-user authentication" —
/// a live handshake with a server outside this emulator, which no stored
/// `ConnectorDestination` `AuthConfig` can stand in for: the association this URL belongs
/// to is not resolved against a stored destination at all (see
/// `crate::ops::create_account_association`). The modelled member has `@default ""` and
/// AWS itself reports it empty "when using General Authorization flows that do not require
/// OAuth", so the empty string is a modelled value rather than an invented one;
/// synthesising an `https://…` URL for a handshake nothing performs would be fabrication.
pub const NO_OAUTH_AUTHORIZATION_URL: &str = "";

/// The `ResourceType` this service reports an account association under — the Smithy
/// resource name, as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing. It is also
/// the resource kind an [`IdempotencyKey`] of a `CreateAccountAssociation` carries.
pub const ACCOUNT_ASSOCIATION_RESOURCE_TYPE: &str = "AccountAssociation";

/// The account-association resource as a READ's reason names it: the resource's name in
/// words, the way [`MANAGED_THING_RESOURCE_NAME`] is for a managed thing.
pub const ACCOUNT_ASSOCIATION_RESOURCE_NAME: &str = "Account Association";

/// The most `AccountAssociationItem`s one `ListAccountAssociations` page carries, and the
/// page size an absent `MaxResults` gets, for the same reason and on the same terms as
/// [`MANAGED_THING_PAGE_LIMIT`]: a caller reaches the rest of the store through the
/// `NextToken`, so one response stays bounded however many records are stored.
pub const ACCOUNT_ASSOCIATION_PAGE_LIMIT: usize = 100;

/// The most account associations one store holds, for the same reason and on the same
/// terms as [`MAX_MANAGED_THINGS`]: a bound CHOSEN for the emulator, not a published AWS
/// quota.
pub const MAX_ACCOUNT_ASSOCIATIONS: usize = 10_000;

/// Per-service state for com-amazonaws-iotmanagedintegrations-iotmanagedintegrations. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The authoritative managed-thing store, keyed by the generated managed-thing id.
    pub managed_things: HashMap<String, ManagedThing>,
    /// Insertion order of managed-thing ids, so `ListManagedThings` pages
    /// deterministically. An id is appended on create and removed on delete.
    pub managed_thing_order: Vec<String>,
    /// The authoritative OTA-task-configuration store, keyed by the generated
    /// `OtaTaskConfigurationId`.
    pub ota_task_configurations: HashMap<String, OtaTaskConfiguration>,
    /// The authoritative cloud-connector store, keyed by the generated
    /// `CloudConnectorId`. `CreateCloudConnector` inserts a record and
    /// `GetCloudConnector` projects it onto its output shape.
    pub cloud_connectors: HashMap<String, CloudConnector>,
    /// Insertion order of cloud-connector ids, so every operation that reports several
    /// of them reports them in the order they were created. An id is appended on create.
    pub cloud_connector_order: Vec<String>,
    /// The authoritative connector-destination store, keyed by the generated
    /// `ConnectorDestinationId`. `CreateConnectorDestination` inserts a record and
    /// `GetConnectorDestination` projects it onto its output shape.
    pub connector_destinations: HashMap<String, ConnectorDestination>,
    /// Insertion order of connector-destination ids, so every operation that reports
    /// several of them reports them in the order they were created. An id is appended on
    /// create.
    pub connector_destination_order: Vec<String>,
    /// A resource kind plus a `ClientToken` (`@idempotencyToken`) -> what the first call
    /// created, so a replayed create returns the original record instead of a duplicate.
    /// The key carries the resource kind because a `ClientToken` is scoped to the
    /// operation that minted it: the same token on two different creates is two tokens.
    /// Bounded by [`IDEMPOTENCY_TTL`] and [`MAX_IDEMPOTENCY_TOKENS`].
    pub idempotency_tokens: HashMap<IdempotencyKey, IdempotencyRecord>,
    /// Source of [`ManagedThing::sequence`] / [`OtaTaskConfiguration::sequence`] values
    /// and of the per-record suffix that makes a generated id unique.
    pub next_sequence: u64,
    /// The hub configuration a `PutHubConfiguration` has SET in each region, keyed by
    /// region. A region with no entry has had none set, so it reports the service
    /// default ([`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`]) and no `UpdatedAt`.
    /// Keyed by a region the request carries, so it is bounded by
    /// [`MAX_CONFIGURED_REGIONS`] like this service's other per-region configuration
    /// store. See [`HubConfiguration`].
    pub hub_configurations: HashMap<String, HubConfiguration>,
    /// This account's default encryption configuration, keyed by region: the setting is
    /// per account AND per region (`GetDefaultEncryptionConfiguration` reports it "for the
    /// Amazon Web Services account in the default or specified region"), so each region's
    /// endpoint reports its own. A region no put has configured holds no entry and reports
    /// [`DefaultEncryptionConfiguration::default`].
    pub default_encryption: HashMap<String, DefaultEncryptionConfiguration>,
    /// The authoritative event-log-configuration store, keyed by the generated
    /// `LogConfigurationId`. At most one record per log target per region — see
    /// [`Self::event_log_configuration_for_target`].
    pub event_log_configurations: HashMap<String, EventLogConfiguration>,
    /// The authoritative account-association store, keyed by the generated
    /// `AccountAssociationId`.
    pub account_associations: HashMap<String, AccountAssociation>,
    /// Insertion order of account-association ids, so `ListAccountAssociations` pages
    /// deterministically. An id is appended on create and removed on delete.
    pub account_association_order: Vec<String>,
}

/// One region's hub configuration: the hub token timer expiry an account set there, and
/// when it was set.
///
/// The hub configuration is a SINGLETON per account and region — `PutHubConfiguration`
/// carries no identifier and `GetHubConfiguration` resolves none — so one record per
/// region is the whole store, mirroring how every other resource of this service is
/// scoped to the region its request was signed for.
///
/// A record exists only where a caller SET the configuration: the expiry an unconfigured
/// region reports is [`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`], and its `UpdatedAt`
/// stays absent because nothing has updated it.
#[derive(Debug, Clone)]
pub struct HubConfiguration {
    /// `HubTokenTimerExpirySettingInSeconds` (modelled `long`, range 1..) exactly as the
    /// last accepted `PutHubConfiguration` set it.
    pub hub_token_timer_expiry_setting_in_seconds: i64,
    /// When that call set it — what `GetHubConfiguration` reports on `UpdatedAt`.
    pub updated_at: SystemTime,
}

/// The hub token timer expiry a region reports before any `PutHubConfiguration` sets
/// one.
///
/// A hub configuration always EXISTS: `GetHubConfiguration` recorded against an account
/// that had never put one still reports `HubTokenTimerExpirySettingInSeconds`, so the
/// setting has a service default rather than being absent until a caller defines it.
/// Neither the model nor the AWS documentation publishes that default's value, so this
/// is a value CHOSEN for the emulator — one day, inside the member's modelled range
/// (`long`, 1..). Only the VALUE is chosen; that the member is reported is recorded
/// behaviour.
pub const DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS: i64 = 86_400;

/// The most `ManagedThingSummary` items one `ListManagedThings` page carries, and the
/// page size an absent `MaxResults` gets. A caller reaches the rest of the store
/// through the `NextToken`, so one request's response stays bounded however many
/// records are stored.
pub const MANAGED_THING_PAGE_LIMIT: usize = 100;

/// The most managed things one store holds. A create past it is refused rather than
/// grown into, so a caller cannot make this emulator's memory grow without end. This
/// is a bound CHOSEN for the emulator, not a published AWS quota.
pub const MAX_MANAGED_THINGS: usize = 10_000;

/// The most OTA task configurations one store holds, for the same reason and on the same
/// terms as [`MAX_MANAGED_THINGS`]: a bound CHOSEN for the emulator, not a published AWS
/// quota.
pub const MAX_OTA_TASK_CONFIGURATIONS: usize = 10_000;

/// How long a `ClientToken` keeps replaying its original answer. A CHOSEN bound (AWS
/// does not publish this window), so the idempotency map cannot keep a record for a
/// caller-supplied token for the life of the process.
pub const IDEMPOTENCY_TTL: ::std::time::Duration = ::std::time::Duration::from_secs(60 * 60);

/// The most idempotency records one store keeps. A token minted past it evicts the
/// oldest, so the map is bounded even when every token is still inside its TTL.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// What a `ClientToken` replays into: the resource the first call created, and when the
/// token was minted (so [`IDEMPOTENCY_TTL`] can retire it). Which KIND of resource that
/// id names is the [`IdempotencyKey`] the record is stored under.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub resource_id: String,
    pub minted_at: SystemTime,
}

/// What an idempotency record is stored under: the kind of resource the create makes —
/// the resource type the service reports it as, e.g. [`MANAGED_THING_RESOURCE_TYPE`] —
/// and the `ClientToken` the caller sent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub resource_type: String,
    pub token: String,
}

impl IdempotencyKey {
    /// The key `token` occupies for `resource_type`.
    pub fn new(resource_type: &str, token: &str) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            token: token.to_string(),
        }
    }
}

impl ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The record `region`'s endpoint holds under `id`, if any. A record created in
    /// another region is not visible here.
    pub fn managed_thing_in(&self, region: &str, id: &str) -> Option<&ManagedThing> {
        self.managed_things
            .get(id)
            .filter(|thing| thing.region == region)
    }

    /// The OTA task configuration `region`'s endpoint holds under `id`, if any. A record
    /// created in another region is not visible here, as for every regional resource.
    pub fn ota_task_configuration_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&OtaTaskConfiguration> {
        self.ota_task_configurations
            .get(id)
            .filter(|configuration| configuration.region == region)
    }

    /// [`Self::managed_thing_in`], mutably.
    pub fn managed_thing_in_mut(&mut self, region: &str, id: &str) -> Option<&mut ManagedThing> {
        self.managed_things
            .get_mut(id)
            .filter(|thing| thing.region == region)
    }

    /// The hub configuration `region`'s endpoint holds, or `None` when no
    /// `PutHubConfiguration` has set one there.
    pub fn hub_configuration_in(&self, region: &str) -> Option<&HubConfiguration> {
        self.hub_configurations.get(region)
    }

    /// The hub token timer expiry `region` reports: the one its last accepted
    /// `PutHubConfiguration` set, else the service default
    /// ([`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`]) — the configuration exists
    /// whether or not a caller has ever set it.
    pub fn hub_token_timer_expiry_in(&self, region: &str) -> i64 {
        self.hub_configuration_in(region).map_or(
            DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS,
            |config| config.hub_token_timer_expiry_setting_in_seconds,
        )
    }

    /// Store `seconds` as `region`'s hub token timer expiry, stamped `at`, answering the
    /// record now held — or `None` when this store cannot take on another region.
    ///
    /// A put REPLACES the configuration — its request carries every member of it — so the
    /// stored expiry is always the last one accepted and `updated_at` is always when that
    /// call ran. Overwriting a region the store already holds always succeeds; taking on
    /// a region it does not is refused once the store holds [`MAX_CONFIGURED_REGIONS`] of
    /// them, which is what bounds this map's cardinality however many distinct regions a
    /// caller signs for — the same rule, and the same bound, as
    /// [`Self::set_default_encryption_in`].
    pub fn put_hub_configuration(
        &mut self,
        region: &str,
        seconds: i64,
        at: SystemTime,
    ) -> Option<HubConfiguration> {
        if !self.hub_configurations.contains_key(region)
            && self.hub_configurations.len() >= MAX_CONFIGURED_REGIONS
        {
            return None;
        }
        let record = HubConfiguration {
            hub_token_timer_expiry_setting_in_seconds: seconds,
            updated_at: at,
        };
        self.hub_configurations
            .insert(region.to_string(), record.clone());
        Some(record)
    }

    /// The default encryption configuration `region`'s endpoint reports: the one a put
    /// stored there, or the account default for a region none has configured.
    ///
    /// A pure read: a region nobody configured gets NO record written for it, so reading
    /// one region's configuration cannot change what a later caller sees anywhere.
    pub fn default_encryption_in(&self, region: &str) -> DefaultEncryptionConfiguration {
        self.default_encryption
            .get(region)
            .cloned()
            .unwrap_or_default()
    }

    /// Store `configuration` as the default encryption configuration of `region`,
    /// answering whether it was stored.
    ///
    /// The counterpart of [`Self::default_encryption_in`], so the keying rule — the region
    /// selects the record — lives in one place for both sides. Overwriting a region the
    /// store already holds always succeeds; taking on a region it does not is refused once
    /// the store holds [`MAX_CONFIGURED_REGIONS`] of them, which is what bounds this map's
    /// cardinality however many distinct regions a caller signs for.
    pub fn set_default_encryption_in(
        &mut self,
        region: &str,
        configuration: DefaultEncryptionConfiguration,
    ) -> bool {
        if !self.default_encryption.contains_key(region)
            && self.default_encryption.len() >= MAX_CONFIGURED_REGIONS
        {
            return false;
        }
        self.default_encryption
            .insert(region.to_string(), configuration);
        true
    }

    /// The cloud connector `region`'s endpoint holds under `id`, if any. A connector
    /// created in another region is not visible here.
    pub fn cloud_connector_in(&self, region: &str, id: &str) -> Option<&CloudConnector> {
        self.cloud_connectors
            .get(id)
            .filter(|connector| connector.region == region)
    }

    /// The connector destination `region`'s endpoint holds under `id`, if any. A
    /// destination created in another region is not visible here, as for every regional
    /// resource.
    pub fn connector_destination_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&ConnectorDestination> {
        self.connector_destinations
            .get(id)
            .filter(|destination| destination.region == region)
    }

    /// [`Self::connector_destination_in`], mutably.
    pub fn connector_destination_in_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut ConnectorDestination> {
        self.connector_destinations
            .get_mut(id)
            .filter(|destination| destination.region == region)
    }

    /// The record `region`'s endpoint holds under `id`, if any. A configuration created in
    /// another region is not visible here.
    pub fn event_log_configuration_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&EventLogConfiguration> {
        self.event_log_configurations
            .get(id)
            .filter(|configuration| configuration.region == region)
    }

    /// The configuration `region` already holds for the log target `resource_type` and
    /// `resource_id` name, if any.
    ///
    /// A target has at most ONE configuration — a logging level is a single setting of the
    /// thing it configures — so this is what a create consults before minting a record.
    pub fn event_log_configuration_for_target(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: Option<&str>,
    ) -> Option<&EventLogConfiguration> {
        self.event_log_configurations.values().find(|configuration| {
            configuration.region == region
                && configuration.target() == (resource_type, resource_id)
        })
    }

    /// The account association `region`'s endpoint holds under `id`, if any. A record
    /// created in another region is not visible here, as for every regional resource.
    pub fn account_association_in(&self, region: &str, id: &str) -> Option<&AccountAssociation> {
        self.account_associations
            .get(id)
            .filter(|association| association.region == region)
    }

    /// Take the next insertion sequence number.
    pub fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        sequence
    }

    /// Retire the idempotency records the clock already made due — the ones past
    /// [`IDEMPOTENCY_TTL`] at `now`, which were owed whether or not anyone called — so
    /// an expired token neither replays nor keeps occupying the map.
    pub fn retire_expired_idempotency_tokens(&mut self, now: SystemTime) {
        self.idempotency_tokens
            .retain(|_, record| match now.duration_since(record.minted_at) {
                Ok(age) => age < IDEMPOTENCY_TTL,
                // A record minted "after" now (a clock that went backwards) is not old.
                Err(_) => true,
            });
    }

    /// The id of the `resource_type` resource the first call carrying `token` created, if
    /// that token is still in the map. Call
    /// [`Self::retire_expired_idempotency_tokens`] first: a token past its TTL does not
    /// replay.
    pub fn replayed_resource_id(&self, resource_type: &str, token: &str) -> Option<String> {
        self.idempotency_tokens
            .get(&IdempotencyKey::new(resource_type, token))
            .map(|record| record.resource_id.clone())
    }

    /// Record that `token` created the `resource_type` resource `resource_id` at
    /// `minted_at`, so a retry of the same create replays it. Keeps the map under
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record_idempotency_token(
        &mut self,
        resource_type: &str,
        token: &str,
        resource_id: &str,
        minted_at: SystemTime,
    ) {
        self.make_room_for_idempotency_token();
        self.idempotency_tokens.insert(
            IdempotencyKey::new(resource_type, token),
            IdempotencyRecord {
                resource_id: resource_id.to_string(),
                minted_at,
            },
        );
    }

    /// Forget every token that replays into the `resource_type` resource `resource_id`:
    /// a `ClientToken` whose resource is gone must not replay into a deleted id.
    pub fn forget_idempotency_tokens_for(&mut self, resource_type: &str, resource_id: &str) {
        self.idempotency_tokens.retain(|key, record| {
            key.resource_type != resource_type || record.resource_id != resource_id
        });
    }

    /// Evict the oldest idempotency records while the map is at
    /// [`MAX_IDEMPOTENCY_TOKENS`], so minting one more cannot push it past the cap.
    pub fn make_room_for_idempotency_token(&mut self) {
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency_tokens
                .iter()
                .min_by_key(|(_, record)| record.minted_at)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.idempotency_tokens.remove(&token);
                }
                None => break,
            }
        }
    }
}

/// One device attribute of a managed thing: the output member that reports it, and the
/// names a request may carry it under.
///
/// A managed thing's device attributes are what its onboarding inputs name. Most of them
/// are members of `GetManagedThingResponse` (and some of `ManagedThingSummary`) that NO
/// request shape declares, because the real service learns them from the physical device.
/// This emulator has no device, so a record learns them from the two name/value channels
/// an onboarding request does carry:
///
/// * the `MetaData` pairs, the model's own free-form attribute channel, and
/// * the fields of the device bar code the create call supplies as
///   `AuthenticationMaterial` (see [`bar_code_fields`]).
///
/// Projecting `MetaData` onto these members is an EMULATOR CONVENTION, not modelled AWS
/// behaviour: the real service reads the device, and a metadata entry named `MacAddress`
/// would not surface on that member there. It is the only channel a caller of this
/// emulator has for a member no request shape declares.
///
/// A value is projected onto the member only when the member's OWN generated schema
/// admits it — [`Self::admits_value`] runs the framework's constraint walk over that
/// member's `@length` and `@pattern`, so nothing here holds a second copy of the model's
/// bounds. A value the member does not admit stays an ordinary metadata or bar-code
/// field, so a response never carries a value outside its shape's constraints.
#[derive(Debug)]
pub struct DeviceAttribute {
    /// The output member this attribute fills, which is also the primary name a request
    /// carries it under.
    pub member: &'static str,
    /// The standard abbreviations of the same attribute, which a product bar code labels
    /// its fields with (`UPC` for a universal product code, `EAN` for an international
    /// article number, `SN` for a serial number, `MAC` for a MAC address, `DSK` for a
    /// Z-Wave device-specific key). Matched by the same rule as [`Self::member`].
    pub aliases: &'static [&'static str],
}

impl DeviceAttribute {
    /// This attribute's value on `thing`: the `MetaData` entry that names it, else the
    /// bar-code field that names it, when that value satisfies the member's modelled
    /// constraints. `MetaData` wins because the caller set it on this resource
    /// explicitly, while a bar-code field describes the device the resource was made from.
    pub fn of(&self, thing: &ManagedThing) -> Option<String> {
        self.in_fields(&thing.meta_data)
            .or_else(|| self.in_fields(&thing.bar_code_fields))
    }

    /// This attribute's value among `fields`: the entry whose key names this member or one
    /// of its abbreviations — compared on alphanumerics only and case-insensitively, so
    /// `UniversalProductCode`, `universalProductCode` and `universal_product_code` all
    /// name it — when that value satisfies the member's modelled constraints.
    pub fn in_fields(&self, fields: &HashMap<String, String>) -> Option<String> {
        let wanted: Vec<String> = ::std::iter::once(self.member)
            .chain(self.aliases.iter().copied())
            .map(normalized_member_key)
            .collect();
        fields
            .iter()
            .find(|(key, _)| wanted.contains(&normalized_member_key(key)))
            .map(|(_, value)| value)
            .filter(|value| self.admits_value(value))
            .cloned()
    }

    /// This member's generated schema on `GetManagedThingResponse` — the one place its
    /// `@length` and `@pattern` live, captured from the model by codegen.
    ///
    /// `None` only if this entry names a member that shape does not declare, which
    /// `the_device_attribute_table_names_modelled_members` refuses. At runtime that
    /// fails CLOSED: [`Self::admits_value`] admits nothing, so the member stays absent
    /// rather than reporting an unvalidated value.
    pub fn schema(&self) -> Option<&'static ::aws_smithy_schema::Schema> {
        crate::ops::types::GET_MANAGED_THING_OUTPUT_SCHEMA
            .members()
            .iter()
            .copied()
            .find(|member| member.member_name() == Some(self.member))
    }

    /// Whether this member's own schema admits `value`, judged by the framework's
    /// constraint walk (`@length`, `@pattern`) rather than by a transcription of it.
    pub fn admits_value(&self, value: &str) -> bool {
        let Some(schema) = self.schema() else {
            return false;
        };
        ::aws_smithy_schema_server::constraints::validate(
            &OneMemberValue { schema, value },
            crate::ops::types::GET_MANAGED_THING_OUTPUT_SCHEMA,
        )
        .is_empty()
    }

    /// The member's modelled `@length` bounds, in characters, as its generated schema
    /// records them: `(0, usize::MAX)` for a member the model leaves unbounded.
    ///
    /// Read from the schema, never transcribed. Exposed so a caller that must generate a
    /// value of a legal — or deliberately illegal — length for this member asks the model
    /// for the bounds.
    pub fn length_bounds(&self) -> (usize, usize) {
        let length = self
            .schema()
            .and_then(::aws_smithy_schema::Schema::traits)
            .and_then(|traits| {
                traits.get(&::aws_smithy_schema_server::constraints::LengthTrait::TRAIT_ID)
            })
            .and_then(|found| {
                ::aws_smithy_schema::Trait::as_any(found)
                    .downcast_ref::<::aws_smithy_schema_server::constraints::LengthTrait>()
            });
        let clamp = |bound: Option<i64>, fallback: usize| {
            bound
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback)
        };
        match length {
            Some(length) => (clamp(length.min, 0), clamp(length.max, usize::MAX)),
            None => (0, usize::MAX),
        }
    }
}

/// One string member's value, serialized ALONE so the framework's constraint walk judges
/// it against that member's own schema. The walk only ever sees what `serialize_members`
/// writes, so writing one member checks exactly that member.
struct OneMemberValue<'a> {
    schema: &'static ::aws_smithy_schema::Schema,
    value: &'a str,
}

impl ::aws_smithy_schema::serde::SerializableStruct for OneMemberValue<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(self.schema, self.value)
    }
}

/// An attribute key reduced to what naming a member means: its alphanumerics, lowercased.
/// Separators (`-`, `_`, spaces) and letter case carry no meaning in a member name, so
/// they are dropped before the comparison.
fn normalized_member_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The named fields of a device bar code.
///
/// `AuthenticationMaterial` is the bar code read off the device, and
/// `AuthenticationMaterialType` says which kind. Two bar-code payload shapes carry NAMED
/// fields — a JSON object, and `Name:value` fields separated by `;` — and the modelled
/// `AuthMaterialString` pattern admits exactly the punctuation those two need (`{`, `}`,
/// `"`, `:`, `,`, `;`, `\`, `/`, space). Where the material is one of them, this is the
/// map it names. Any other payload — a digit-only Z-Wave SmartStart code, a base64 DPP
/// key — names no field, so this answers an empty map and no attribute is projected from
/// it.
pub fn bar_code_fields(material: &str) -> HashMap<String, String> {
    let material = material.trim();
    if material.starts_with('{') {
        return match ::serde_json::from_str::<::serde_json::Value>(material) {
            Ok(::serde_json::Value::Object(object)) => object
                .into_iter()
                .filter_map(|(key, value)| scalar_text(&value).map(|text| (key, text)))
                .collect(),
            _ => HashMap::new(),
        };
    }
    material
        .split(';')
        .filter_map(|field| field.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .collect()
}

/// The text of a JSON scalar, or `None` for a composite: a bar-code field names one
/// value, so an object or array under a name is not an attribute value.
fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// `AdvertisedProductId`.
pub static ADVERTISED_PRODUCT_ID: DeviceAttribute = DeviceAttribute {
    member: "AdvertisedProductId",
    aliases: &[],
};

/// `UniversalProductCode`.
pub static UNIVERSAL_PRODUCT_CODE: DeviceAttribute = DeviceAttribute {
    member: "UniversalProductCode",
    aliases: &["UPC"],
};

/// `InternationalArticleNumber`.
pub static INTERNATIONAL_ARTICLE_NUMBER: DeviceAttribute = DeviceAttribute {
    member: "InternationalArticleNumber",
    aliases: &["EAN", "IAN"],
};

/// `ConnectorPolicyId`.
pub static CONNECTOR_POLICY_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorPolicyId",
    aliases: &[],
};

/// `ConnectorDestinationId`.
pub static CONNECTOR_DESTINATION_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorDestinationId",
    aliases: &[],
};

/// `ConnectorDeviceId`.
pub static CONNECTOR_DEVICE_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorDeviceId",
    aliases: &[],
};

/// `DeviceSpecificKey`.
pub static DEVICE_SPECIFIC_KEY: DeviceAttribute = DeviceAttribute {
    member: "DeviceSpecificKey",
    aliases: &["DSK"],
};

/// `MacAddress`.
pub static MAC_ADDRESS: DeviceAttribute = DeviceAttribute {
    member: "MacAddress",
    aliases: &["MAC"],
};

/// `ParentControllerId`.
pub static PARENT_CONTROLLER_ID: DeviceAttribute = DeviceAttribute {
    member: "ParentControllerId",
    aliases: &[],
};

/// `SerialNumber`.
///
/// Unlike every other entry this member IS declared by the create and update requests, so
/// what the caller named there wins; this entry is how a record still learns the serial
/// number when only the device bar code carries it.
pub static SERIAL_NUMBER: DeviceAttribute = DeviceAttribute {
    member: "SerialNumber",
    aliases: &["SN"],
};

/// Every attribute the model reports on the read side only, so a caller that must cover
/// the whole class — a test, or a projection over all of them — enumerates it instead of
/// repeating the list. `SerialNumber` is not here: it has its own modelled request member.
pub static READ_SIDE_ATTRIBUTES: &[&DeviceAttribute] = &[
    &ADVERTISED_PRODUCT_ID,
    &UNIVERSAL_PRODUCT_CODE,
    &INTERNATIONAL_ARTICLE_NUMBER,
    &CONNECTOR_POLICY_ID,
    &CONNECTOR_DESTINATION_ID,
    &CONNECTOR_DEVICE_ID,
    &DEVICE_SPECIFIC_KEY,
    &MAC_ADDRESS,
    &PARENT_CONTROLLER_ID,
];

/// The Wi-Fi Simple Setup configuration a managed thing carries when no call has set
/// one: neither provisioning role enabled.
///
/// `WiFiSimpleSetupConfiguration` states which WSS provisioning roles the thing may
/// play, so every managed thing HAS one — a thing nobody configured plays neither role
/// — and a create stores that configuration rather than leaving the resource without
/// any. `TimeoutInMinutes` stays absent: the model publishes no default for it (modelled
/// range 5..15), so the emulator invents none.
pub fn default_wifi_simple_setup_configuration() -> WiFiSimpleSetupConfiguration {
    WiFiSimpleSetupConfiguration {
        enable_as_provisioner: Some(false),
        enable_as_provisionee: Some(false),
        timeout_in_minutes: None,
    }
}

/// A fresh opaque identifier: lowercase hex of the wall clock in nanoseconds plus a
/// per-store counter, so it is unique within a run and matches both the
/// `ManagedThingId` pattern (`^[a-zA-Z0-9:_-]*$`, len 1..64) and the `NextToken`
/// pattern (`^[a-zA-Z0-9=_-]+$`).
pub fn opaque_id(counter: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{:016x}{counter:08x}", nanos as u64)
}

/// A `NextToken` carrying the [`ManagedThing::sequence`] a page resumes at, encoded so
/// it is opaque to the caller and matches the modelled `NextToken` pattern
/// (`^[a-zA-Z0-9=_-]+$`). Encoding the cursor keeps `ListManagedThings` a pure read:
/// paging stores nothing, so a list cannot change what a later caller sees.
pub fn encode_page_token(resume_at: u64) -> String {
    format!("{resume_at:016x}")
}

/// The sequence a `NextToken` resumes at, or `None` when this service cannot have
/// issued the token: `minted` is the store's next sequence, so a cursor beyond it was
/// never handed out.
pub fn decode_page_token(token: &str, minted: u64) -> Option<u64> {
    (token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(token, 16).ok())
        .flatten()
        .filter(|resume_at| *resume_at <= minted)
}

/// The modelled timestamp for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => ::aws_smithy_types::DateTime::from_secs_f64(since.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The region this request was signed for, falling back to [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ResourceType` this service reports a managed thing under — the Smithy resource
/// name, which is also what `ResourceNotFoundException`'s `ResourceType` member carries
/// and what a mutation's reason names ([`identifier_not_found_message`]).
pub const MANAGED_THING_RESOURCE_TYPE: &str = "ManagedThing";

/// The managed-thing resource as a READ's reason names it — the resource's name in
/// words, which is what that sentence states instead of the identifier
/// ([`resource_not_found_message`]).
pub const MANAGED_THING_RESOURCE_NAME: &str = "Managed Thing";

/// The `ResourceType` this service reports an OTA task configuration under — the Smithy
/// resource name, as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing. It is also
/// the resource kind an [`IdempotencyKey`] of a `CreateOtaTaskConfiguration` carries.
pub const OTA_TASK_CONFIGURATION_RESOURCE_TYPE: &str = "OtaTaskConfiguration";

/// The OTA-task-configuration resource as a READ's reason names it: the resource's name
/// in words, the way [`MANAGED_THING_RESOURCE_NAME`] is for a managed thing.
pub const OTA_TASK_CONFIGURATION_RESOURCE_NAME: &str = "Ota Task Configuration";

/// How a MUTATION of this service states that the resource its `Identifier` named does
/// not exist: the resource's type, the identifier the request carried, and that it was
/// not found.
///
/// The text is BUILT FROM THE REQUEST, so it is part of the contract and not just an
/// example: the recorded 404s of `UpdateManagedThing` and `DeleteManagedThing` for an
/// absent managed thing both read `ManagedThing for identifier '<Identifier>' not found`.
/// Every operation that MUTATES a resource it resolved by the identifier its request
/// carries states its 404 through here, so one phrasing serves them all.
pub fn identifier_not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} for identifier '{identifier}' not found")
}

/// How a MUTATION states that the resource it was to change does not exist WITHOUT naming
/// the identifier: the resource as the sentence's subject, and that it cannot be found.
///
/// Recorded on `UpdateConnectorDestination` for a destination a delete had just removed as
/// `Connector destination cannot be found.` — a different sentence from the
/// identifier-quoting one the managed-thing mutations answer
/// ([`identifier_not_found_message`]), so each resource's mutating path words its own
/// through one of these two helpers rather than a sentence of its own.
pub fn cannot_be_found_message(resource_subject: &str) -> String {
    format!("{resource_subject} cannot be found.")
}

/// How a READ of this service states the same thing: the resource's name in words and
/// that no such resource exists, naming no identifier.
///
/// A read resolves the resource through this service's fetch path, which reports WHICH
/// KIND of resource was not found rather than which one — recorded on `GetManagedThing`
/// for an absent identifier as `Managed Thing resource not found.`, in the same fixture
/// whose mutations carry the identifier-derived sentence above. The identifier the caller
/// sent is still reported, on the `ResourceId` member the shape declares for it.
pub fn resource_not_found_message(resource_name: &str) -> String {
    format!("{resource_name} resource not found.")
}

/// The modelled `ResourceNotFoundException` a MUTATION answers for a managed thing the
/// store does not hold under the identifier a request named: the reason
/// ([`identifier_not_found_message`]) plus the `ResourceId` and `ResourceType` members
/// the shape declares.
pub fn managed_thing_not_found_on_mutation(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(identifier_not_found_message(
            MANAGED_THING_RESOURCE_TYPE,
            identifier,
        )),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for an event log configuration
/// the store does not hold under the `Id` the request named: the read path's reason
/// ([`resource_not_found_message`]) plus the `ResourceId` and `ResourceType` members, worded
/// through the same two helpers every other resource of this service uses.
pub fn event_log_configuration_not_found_on_read(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(
            EVENT_LOG_CONFIGURATION_RESOURCE_NAME,
        )),
        ..resource_not_found(EVENT_LOG_CONFIGURATION_RESOURCE_TYPE, id)
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for the same absence: the same
/// shape with the read path's reason ([`resource_not_found_message`]).
pub fn managed_thing_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(MANAGED_THING_RESOURCE_NAME)),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// How a READ states the same absence WITHOUT the word "resource": the resource's name in
/// words and that it was not found.
///
/// The read 404s of this service are not one sentence. A managed thing's reads
/// `Managed Thing resource not found.` ([`resource_not_found_message`]); a connector
/// destination's, recorded on `GetConnectorDestination` for an identifier a delete had just
/// removed, reads `Connector Destination not found` — the same two facts (the kind, and
/// that it is absent) with neither the noun nor the full stop. Each resource's read path
/// words its own, through one of these two helpers rather than a sentence of its own.
pub fn resource_name_not_found_message(resource_name: &str) -> String {
    format!("{resource_name} not found")
}

/// How a read of a resource reached THROUGH a managed thing states that the managed
/// thing exists but holds nothing of that kind: the owning resource's name in words and
/// the kind that is missing from it.
///
/// Recorded on `GetManagedThingCapabilities` for a managed thing no call ever gave a
/// capability member, as `Managed Thing capabilities not found` — a different absence
/// from the one [`resource_not_found_message`] words, which is the managed thing itself
/// not existing.
pub fn sub_resource_not_found_message(resource_name: &str, sub_resource: &str) -> String {
    format!("{resource_name} {sub_resource} not found")
}

/// The capability-report half of a managed thing, as the reason of a capability read's
/// 404 names it ([`sub_resource_not_found_message`]).
pub const MANAGED_THING_CAPABILITIES_SUB_RESOURCE: &str = "capabilities";

/// The modelled `ResourceNotFoundException` `GetManagedThingCapabilities` answers for a
/// managed thing that holds no capability member at all: the sub-resource reason above,
/// plus the `ResourceId` and `ResourceType` members every 404 of this service reports
/// the same way.
pub fn managed_thing_capabilities_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(sub_resource_not_found_message(
            MANAGED_THING_RESOURCE_NAME,
            MANAGED_THING_CAPABILITIES_SUB_RESOURCE,
        )),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// The `ResourceId` and `ResourceType` half of any resource's 404, which every path reports
/// the same way: the identifier the request carried, and the resource's type. The reason is
/// the caller's to fill, so this is not public — reach it through one of the
/// `*_not_found_on_read` / `*_not_found_on_mutation` helpers above.
fn resource_not_found(resource_type: &str, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: None,
        resource_id: Some(identifier.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for a cloud connector the
/// store does not hold under the identifier a request named, worded by the same read-path
/// sentence every read of this service uses ([`resource_not_found_message`]).
pub fn cloud_connector_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(CLOUD_CONNECTOR_RESOURCE_NAME)),
        ..resource_not_found(CLOUD_CONNECTOR_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for a connector destination the
/// store does not hold under the identifier a request named: the connector destination's
/// own read-path reason ([`resource_name_not_found_message`]) plus the `ResourceId` and
/// `ResourceType` members every 404 of this service reports the same way.
pub fn connector_destination_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_name_not_found_message(
            CONNECTOR_DESTINATION_RESOURCE_NAME,
        )),
        ..resource_not_found(CONNECTOR_DESTINATION_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` a MUTATION answers for a connector destination
/// the store does not hold under the `Identifier` a request named: the connector
/// destination's own mutating-path reason ([`cannot_be_found_message`]) plus the
/// `ResourceId` and `ResourceType` members every 404 of this service reports the same way.
pub fn connector_destination_not_found_on_mutation(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(cannot_be_found_message(
            CONNECTOR_DESTINATION_RESOURCE_SUBJECT,
        )),
        ..resource_not_found(CONNECTOR_DESTINATION_RESOURCE_TYPE, identifier)
    }
}

/// How a create states that a member of its request names a resource of ANOTHER kind that
/// this account does not have: the member the identifier arrived on, and that no such
/// resource exists.
///
/// The text is BUILT FROM THE REQUEST — the member at fault is what names it — so it is
/// part of the contract and not just an example: the recorded refusal of a
/// `CreateConnectorDestination` whose `CloudConnectorId` named no connector of the account
/// reads `The provided CloudConnectorId does not exist.` Every create that resolves a
/// reference states its refusal through here, so one phrasing serves them all.
///
/// No value is quoted back, the way every other refusal this service words does
/// (`crate::service::enum_value_set_refusal`), so an identifier the model marks
/// `@sensitive` is never published in an error body.
pub fn unknown_reference_message(member: &str) -> String {
    format!("The provided {member} does not exist.")
}

/// The modelled `ResourceNotFoundException` a READ answers for an OTA task configuration
/// the store does not hold under the identifier a request named: the read path's reason
/// ([`resource_not_found_message`]) plus the `ResourceId` and `ResourceType` members the
/// shape declares, reported exactly as a managed thing's read 404 reports them.
pub fn ota_task_configuration_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(
            OTA_TASK_CONFIGURATION_RESOURCE_NAME,
        )),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(OTA_TASK_CONFIGURATION_RESOURCE_TYPE.to_string()),
    }
}

/// The ARN of a managed thing in `region`, as the `ManagedThingArn` pattern requires.
pub fn managed_thing_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_ARN_NAMESPACE, region, &format!("managed-thing/{id}"))
}

/// The ARN of an account association in `region`, as the `AccountAssociationArn` pattern
/// (`^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:account-association/[0-9a-zA-Z]+$`)
/// requires.
pub fn account_association_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("account-association/{id}"),
    )
}

/// The modelled `ResourceNotFoundException` a READ answers for an account association the
/// store does not hold under the identifier a request named: the read path's reason
/// ([`resource_not_found_message`]) plus the `ResourceId` and `ResourceType` members,
/// worded through the same two helpers every other resource of this service uses.
pub fn account_association_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(
            ACCOUNT_ASSOCIATION_RESOURCE_NAME,
        )),
        ..resource_not_found(ACCOUNT_ASSOCIATION_RESOURCE_TYPE, identifier)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>)
}
