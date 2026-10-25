// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssoadmin-swbexternalservice service.
//!
//! The records here are the service's own, wire-independent view of an IAM Identity
//! Center instance, its permission sets and its account assignments. Handlers read and
//! write them through [`service_state`] and project them onto the modeled shapes in
//! `crate::ops::types`.
//!
//! An instance carries its KIND ([`InstanceType`]): an ACCOUNT instance (what
//! `CreateInstance` creates, for a standalone account) serves no permission sets, so the
//! whole permission-set / account-assignment surface is withdrawn on it — see
//! [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn`] and
//! [`ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`], which every operation of that surface asks
//! before looking anything up. The account's AMBIENT instance — the one `ListInstances`
//! hands out ([`SEED_INSTANCE_TYPE`]) — is an ACCOUNT instance, so that surface is
//! withdrawn on it until an `UpdateInstance` enables permission sets.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessControlAttribute, AccountAssignment, AccountAssignmentOperationStatus,
    ApplicationProvider, AttachedManagedPolicy, AuthenticationMethod,
    CustomerManagedPolicyReference, DisplayData, EncryptionConfigurationDetails, Grant,
    InstanceAccessControlAttributeConfiguration, InstanceMetadata,
    PermissionSet, PermissionsBoundary, PortalOptions, RegionMetadata, ResourceServerConfig,
    ResourceServerScopeDetails, ScopeDetails, Tag, TrustedTokenIssuerConfiguration,
};

/// The identity-store instance every `InstanceArn`-taking operation resolves against.
///
/// IAM Identity Center allows ONE instance per account, so the emulator seeds exactly
/// one at first touch of this state (see the `Default` impl) rather than minting
/// instances on demand. The id is fixed so repeated `ListInstances` calls — and the
/// permission-set ARNs derived from it — are stable for the life of the process.
///
/// `ssoins-` + 16 characters, the form the model's `InstanceArn` pattern accepts:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
pub const SEED_INSTANCE_ID: &str = "ssoins-7223b1d1ac5e1f0e";

/// The identity store connected to the seeded instance: `d-` + 10 hex digits, which
/// `InstanceMetadata$IdentityStoreId` (`^[a-zA-Z0-9-]*$`, 1..64) accepts and which
/// keeps `IdentityStoreArn` inside its modeled 62..93 length.
pub const SEED_IDENTITY_STORE_ID: &str = "d-9067f3a1c2";

/// The Region an instance is enabled in when the request that enabled it names none —
/// a call that carries no SigV4 credential scope to read a Region off. Matches
/// `RegionName`'s `^([a-z]+-){2,3}\d$` pattern.
///
/// It is a FALLBACK, not the Region of every instance: an instance is enabled in the
/// Region of the request that enabled it ([`request_region`]), which is what
/// `DescribeInstance$PrimaryRegion` ("The primary Region where the IAM Identity Center
/// instance was originally enabled") reports.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The seeded instance's display name (`NameType`, `^[\w+=,.@-]+$`).
pub const SEED_INSTANCE_NAME: &str = "IdentityCenter";

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope.
///
/// This is the only Region signal a request carries, and it is what an instance this
/// account holds is "enabled in": IAM Identity Center is a Regional enablement, so an
/// instance enabled through a call made in one Region reports that Region as its primary
/// one rather than a constant the emulator was built with.
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the function
/// that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in the
/// vendored, read-only framework, and a shared crate cannot be introduced for it without
/// editing the root `Cargo.toml`, which this project does not allow.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `InstanceStatus`/`RegionStatus` value for a usable instance: the model's
/// `InstanceStatus` enum is `CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS |
/// ACTIVE`, and a seeded instance is ready, so it is `ACTIVE` and stays `ACTIVE`.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `InstanceStatus::CREATE_IN_PROGRESS` — the status a freshly created instance carries
/// until the clock makes its activation due (see [`INSTANCE_ACTIVATION_SPAN`]).
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `InstanceStatus::DELETE_IN_PROGRESS` — the status an instance carries between the
/// `DeleteInstance` that started its removal and the instant that removal falls due (see
/// [`INSTANCE_DELETION_SPAN`]). `DescribeInstance`'s own documentation declares it
/// observable: "DELETE_IN_PROGRESS - The instance is being deleted. Returns
/// AccessDeniedException after the delete operation completes."
pub const STATUS_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `RegionStatus::ADDING` (the enum is `ACTIVE | ADDING | REMOVING`) — the status a
/// Region carries while, as `RegionMetadata$Status` documents it, the "Region extension
/// workflow is in progress".
pub const STATUS_ADDING: &str = "ADDING";

/// `RegionStatus::REMOVING` — the status a Region carries while the "Region removal
/// workflow is in progress" (`RegionMetadata$Status`), which is what `RemoveRegion` puts
/// the Region it names into: "The Region status is set to REMOVING and the Region record
/// is deleted when the workflow completes."
pub const STATUS_REMOVING: &str = "REMOVING";

/// How long after `DeleteInstance` the instance stops existing.
///
/// AWS publishes no duration for it, so this span is the emulator's own CHOSEN one, on
/// the same footing as [`INSTANCE_ACTIVATION_SPAN`]: it is never zero, so a caller that
/// describes the instance straight after deleting it still observes the
/// `DELETE_IN_PROGRESS` status the model declares and the operation documents, and it is
/// short enough that a test can wait it out. It is held as the instant the removal falls
/// due and applied by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`] — nothing sleeps.
pub const INSTANCE_DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after `CreateInstance` the new instance's move from `CREATE_IN_PROGRESS` to
/// `ACTIVE` falls due.
///
/// AWS documents no duration for it, so this span is the emulator's own CHOSEN one: it
/// is never zero, so a caller that reads the instance straight after creating it still
/// observes the transient status the model declares, and it is short enough that the
/// instance is usable within one test. It is stored as a due instant and materialised by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`] — nothing sleeps.
pub const INSTANCE_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `RemoveRegion` the Region record it names stops existing — the end of
/// the `REMOVING` the remove put it into ("This operation initiates an asynchronous
/// workflow to clean up IAM Identity Center resources in the specified additional Region.
/// The Region status is set to REMOVING and the Region record is deleted when the workflow
/// completes").
///
/// AWS publishes no duration for that workflow, so this span is the emulator's own CHOSEN
/// one, on the same footing as [`INSTANCE_DELETION_SPAN`]: it is never zero, so a caller
/// that reads the instance straight after the remove still observes the `REMOVING` status
/// the model declares, and it is short enough to be waited out inside one test. It is held
/// as the instant the removal falls due and applied by [`Instance::materialize_due`] —
/// nothing sleeps for it.
pub const REGION_REMOVAL_SPAN: Duration = Duration::from_millis(500);

/// Which KIND of IAM Identity Center instance a record is.
///
/// The distinction is AWS's: `CreateInstance` is documented as creating "an instance of
/// IAM Identity Center for a standalone Amazon Web Services account that is not managed
/// by Organizations or a member Amazon Web Services account in an organization" — an
/// ACCOUNT instance — while the instance of an AWS Organizations organization is enabled
/// through Organizations and not through this API. Multi-account permissions (permission
/// sets, and the account assignments built from them) are a feature of the organization
/// instance only, so the whole permission-set / account-assignment surface is withdrawn
/// on an account instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceType {
    /// An instance of a single standalone account: what `CreateInstance` creates.
    Account,
    /// The instance of an AWS Organizations organization.
    Organization,
}

impl InstanceType {
    /// Whether the permission-set / account-assignment surface is served on an instance
    /// of this kind — which is exactly what `DescribeInstance$PermissionSetsEnabled`
    /// ("Indicates whether permission sets are enabled for this Identity Center
    /// instance") reports.
    pub fn permission_sets_enabled(self) -> bool {
        matches!(self, Self::Organization)
    }
}

/// The kind of instance the account's AMBIENT instance is: the instance a client
/// discovers through `ListInstances` because IAM Identity Center was already enabled on
/// this account before the session started.
///
/// It is an ACCOUNT instance: the instance of a single standalone account, which serves
/// no multi-account permissions, so permission sets, the account assignments built from
/// them and the ABAC attribute configuration are all WITHDRAWN on it
/// (`DescribeInstance$PermissionSetsEnabled` reports `false`) until an `UpdateInstance`
/// turns that enablement on — "Enables permission sets for this Identity Center
/// instance", the one documented way it changes.
///
/// The ORGANIZATION instance of an AWS Organizations organization is enabled through
/// Organizations and not through this API, so no operation here hands one out; a caller
/// that needs that surface on this account's instance enables permission sets on it
/// explicitly.
pub const SEED_INSTANCE_TYPE: InstanceType = InstanceType::Account;

/// The refusal every operation of the permission-set / account-assignment surface
/// answers with when the instance it resolves is an ACCOUNT instance, as the message of
/// the modelled `ValidationException` (HTTP 400).
///
/// The 400/`ValidationException` classification is the requirement — a client that
/// branches on the error must be able to tell "this instance type can never do
/// assignments" from "the permission set is merely missing" (404) — and this is AWS's
/// own wording for the restriction.
pub const ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE: &str =
    "This operation is not supported for account instances of IAM Identity Center.";

/// The refusal `CreateInstance` answers with when the account already holds an
/// instance, as the message of the modelled `ServiceQuotaExceededException`.
///
/// The rule is the operation's own documentation: "You can create only one instance per
/// account and across all Amazon Web Services Regions", and "The CreateInstance request
/// is rejected if … An instance already exists in the same account". AWS reports it as a
/// QUOTA on the account — it names the account the quota was crossed in and points at
/// the published limits — so the message is BUILT from the account the request ran under
/// rather than being a constant about instances in general.
pub fn instance_quota_exceeded_message() -> String {
    format!(
        "You have exceeded IAM Identity Center quotas. Cannot create more than 1 \
         instance for {}. Please refer to \
         https://docs.aws.amazon.com/singlesignon/latest/userguide/limits.html",
        ::engine::emulator_account_id()
    )
}

/// The refusal a repeated `ClientToken` earns when the rest of the `CreateInstance`
/// request differs from the call that token belongs to, as the message of the modelled
/// `ConflictException` (HTTP 409).
///
/// The rule is the `ClientToken` member's own documentation: "Passing the same value to
/// a later call to an operation requires that you also pass the same value for all other
/// parameters".
pub const CLIENT_TOKEN_MISMATCH_MESSAGE: &str =
    "The ClientToken was already used for a CreateInstance request with different \
     parameters.";

/// The refusal an `InstanceArn` this account holds no instance for earns, as the message
/// of the modelled `AccessDeniedException` — the only not-found channel
/// `DescribeInstance`/`DeleteInstance` declare ("DELETE_IN_PROGRESS — … Returns
/// AccessDeniedException after the delete operation completes").
///
/// Built from the request: it names the ARN that resolved to nothing.
pub fn instance_not_found_message(instance_arn: &str) -> String {
    format!("No IAM Identity Center instance {instance_arn} exists in this account")
}

/// The refusal a `Scope` an application does not carry earns, as the message of the
/// modelled `ResourceNotFoundException`.
///
/// Built from the request: it names both the scope and the application it was looked for
/// on, because an unknown scope on a known application is a different failure from an
/// unknown application.
pub fn access_scope_not_found_message(application_arn: &str, scope: &str) -> String {
    format!("Could not find access scope {scope} for application {application_arn}.")
}

/// The refusal a `TrustedTokenIssuerArn` nothing holds earns, as the message of the
/// modelled `ResourceNotFoundException`.
///
/// Built from the request: AWS names the missing trusted token issuer by the ARN the
/// caller asked for, which is what tells "this ARN is gone" from "some other resource of
/// this call is missing".
pub fn trusted_token_issuer_not_found_message(trusted_token_issuer_arn: &str) -> String {
    format!("Could not find TrustedTokenIssuer with arn {trusted_token_issuer_arn}.")
}

/// The refusal an `ApplicationArn` this account holds no application for earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`trusted_token_issuer_not_found_message`]: it names the
/// ARN that resolved to nothing, which is what tells "this application is gone" from
/// "some other resource of this call is missing".
pub fn application_not_found_message(application_arn: &str) -> String {
    format!("Could not find Application with arn {application_arn}.")
}

/// The refusal a `DescribeApplicationAssignment` of a principal that holds no DIRECT
/// assignment to the application earns, as the message of the modelled
/// `ResourceNotFoundException`.
///
/// Built from the request: the assignment's identity is the whole tuple, so the refusal
/// names all three parts rather than the application alone.
pub fn application_assignment_not_found_message(
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> String {
    format!(
        "Could not find an assignment of {principal_type} {principal_id} to application \
         {application_arn}."
    )
}

/// The refusal a delete of an account assignment that is not granted earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: an assignment's identity is the whole (account, permission
/// set, principal type, principal id) tuple, so the refusal names every part of it
/// rather than one resource of the call.
pub fn account_assignment_not_found_message(
    account_id: &str,
    permission_set_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> String {
    format!(
        "Could not find an assignment of permission set {permission_set_arn} to \
         {principal_type} {principal_id} on account {account_id}."
    )
}

/// The refusal a `RequestId` this instance holds no assignment-operation status record
/// for earns, as the message of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`trusted_token_issuer_not_found_message`]: it names the
/// request id that resolved to nothing, which is what tells "no operation was ever
/// tracked under this id" from "some other resource of this call is missing". One helper
/// for both halves of that history: a creation status and a deletion status are looked up
/// the same way, by the id the operation answered with.
pub fn assignment_operation_not_found_message(request_id: &str) -> String {
    format!("Could not find the status of an assignment operation with request id {request_id}.")
}

/// `ApplicationStatus::ENABLED` (the enum is `ENABLED | DISABLED`) — the status an
/// application carries when the create that made it named none.
///
/// CHOSEN default: the model marks `CreateApplication$Status` optional and declares no
/// `@default` for it, and AWS publishes no value for an omitted one, so the emulator
/// picks the usable state — an application nobody disabled is enabled, which is also
/// what the `PortalOptions$Visibility` member of the same request defaults to.
pub const APPLICATION_STATUS_ENABLED: &str = "ENABLED";

/// `ApplicationVisibility::ENABLED` — the value `PortalOptions$Visibility` takes when
/// the request names none. This one is the MODEL's default, not a choice:
/// `smithy_query(query="shape",shape="ApplicationVisibility")` carries
/// `"default": "ENABLED"`.
pub const APPLICATION_VISIBILITY_ENABLED: &str = "ENABLED";

/// The assignment configuration an application carries before any
/// `PutApplicationAssignmentConfiguration` sets one.
///
/// The MODEL's default, not a choice: `AssignmentRequired` carries `"default": true` on
/// both the put's input and the get's output, and the member's own documentation states
/// it — "If AssignmentsRequired is true (default value), users don't have access to the
/// application unless an assignment is created using the CreateApplicationAssignment
/// API."
pub const APPLICATION_ASSIGNMENT_REQUIRED_DEFAULT: bool = true;

/// `SignInOrigin::APPLICATION` — the origin that redirects the user to the application's
/// own URL, which is why `SignInOptions$ApplicationUrl` is required with it.
pub const SIGN_IN_ORIGIN_APPLICATION: &str = "APPLICATION";

/// The refusal a `SignInOptions` with `Origin` `APPLICATION` and no `ApplicationUrl`
/// earns, as the message of the modelled `ValidationException`.
///
/// The rule is `SignInOptions$ApplicationUrl`'s own documentation: "The URL that accepts
/// authentication requests for an application. This is a required parameter if the
/// Origin parameter is APPLICATION." The model cannot express a conditionally required
/// member, so the boundary validator passes the request and the handler refuses it.
pub const APPLICATION_URL_REQUIRED_MESSAGE: &str =
    "PortalOptions.SignInOptions.ApplicationUrl is required when \
     PortalOptions.SignInOptions.Origin is APPLICATION.";

/// The refusal `CreateApplication` answers with when `ApplicationProviderArn` names a
/// published provider that cannot back a CUSTOMER MANAGED OAuth 2.0 application, as the
/// message of the modelled `ValidationException` (HTTP 400).
///
/// The rule is the operation's own documentation: it creates "an OAuth 2.0 customer
/// managed application … for the given application provider" and "does not support
/// creating SAML 2.0 customer managed applications or Amazon Web Services managed
/// applications" — a SAML 2.0 application is set up in the console and an Amazon Web
/// Services managed application is created by the service that owns its provider.
///
/// Built from the request: it names the provider ARN that cannot back the create, which
/// is what tells this refusal from the one a provider ARN nothing publishes earns
/// (`ResourceNotFoundException`).
pub fn application_provider_unsupported_message(application_provider_arn: &str) -> String {
    format!(
        "Application provider {application_provider_arn} cannot be used to create an \
         application. This operation creates OAuth 2.0 customer managed applications only."
    )
}

/// `KmsKeyType::AWS_OWNED_KMS_KEY` — the instance is encrypted with an Amazon Web
/// Services owned key, so no `KmsKeyArn` names it ("Cannot be specified when KeyType is
/// AWS_OWNED_KMS_KEY", `EncryptionConfiguration$KmsKeyArn`).
pub const KEY_TYPE_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";

/// `KmsKeyType::CUSTOMER_MANAGED_KEY` — the instance is encrypted with the caller's own
/// KMS key, which `KmsKeyArn` must name ("Required when KeyType is
/// CUSTOMER_MANAGED_KEY").
pub const KEY_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KEY";

/// `KmsKeyStatus::UPDATING` — an encryption-configuration change that has not taken
/// effect yet.
pub const KMS_KEY_STATUS_UPDATING: &str = "UPDATING";

/// `KmsKeyStatus::ENABLED` — the encryption configuration an instance is actually
/// encrypted under.
pub const KMS_KEY_STATUS_ENABLED: &str = "ENABLED";

/// How long after the `UpdateInstance` that sets an encryption configuration the
/// instance reports that configuration as `ENABLED` rather than `UPDATING`.
///
/// The key change is asynchronous — the model carries `KmsKeyStatus::UPDATING` and an
/// `EncryptionStatusReason` for the failure case precisely because the caller can observe
/// the change in flight — and AWS publishes no duration for it, so this span is the
/// emulator's OWN choice: non-zero, so a caller that reads straight after the update still
/// sees `UPDATING`, and short enough that a test can wait it out.
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// `InstanceAccessControlAttributeConfigurationStatus::ENABLED` — the terminal status of
/// an ABAC configuration whose enabling has finished.
pub const ABAC_STATUS_ENABLED: &str = "ENABLED";

/// `InstanceAccessControlAttributeConfigurationStatus::CREATION_IN_PROGRESS` — the status
/// an ABAC configuration carries between the create that enabled it and the instant the
/// clock makes its enablement due (see [`ABAC_ENABLE_SPAN`]).
pub const ABAC_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// How long after `CreateInstanceAccessControlAttributeConfiguration` the configuration
/// reports `ENABLED` rather than `CREATION_IN_PROGRESS`.
///
/// Enabling ABAC is asynchronous — the model carries the non-terminal
/// `CREATION_IN_PROGRESS` status and the operation's own documentation says "After a
/// successful response, call DescribeInstanceAccessControlAttributeConfiguration to
/// validate that InstanceAccessControlAttributeConfiguration was created", so the caller
/// can observe the enabling in flight — and AWS publishes no duration for it, so this
/// span is the emulator's OWN choice: non-zero, so a caller that describes straight after
/// the create still sees the transient status, and short enough that a test can wait it
/// out. It is held as the instant the change falls due and the status is DERIVED from it
/// (see [`AccessControlAttributeConfigurationRecord::status`]); nothing sleeps for it.
pub const ABAC_ENABLE_SPAN: Duration = Duration::from_millis(500);

/// `StatusValues::SUCCEEDED` — the terminal status of an assignment operation whose
/// completion the clock has already made due.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_PROVISIONED` — the provisioning status of
/// an account whose copy of a permission set is the permission set's CURRENT definition
/// ([`PermissionSetRecord::definition_revision`]).
pub const PROVISIONING_STATUS_LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_NOT_PROVISIONED` — the provisioning status
/// of an account that holds the permission set, but a copy of an EARLIER definition of
/// it: the permission set has changed since it was provisioned there, so the account does
/// not yet have the latest one.
pub const PROVISIONING_STATUS_LATEST_NOT_PROVISIONED: &str =
    "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// The provisioning status a provisioned copy of a permission set is in: the revision that
/// copy was provisioned at, against the permission set's revision now.
///
/// The ONE place that comparison is made, so the two projections of the same rows —
/// [`ComAmazonawsSsoadminSwbexternalserviceState::provisioned_accounts`] and
/// [`ComAmazonawsSsoadminSwbexternalserviceState::provisioned_permission_sets`] — cannot
/// drift apart about which copy is up to date.
fn provisioning_status_of(provisioned_revision: &str, current_revision: &str) -> &'static str {
    if provisioned_revision == current_revision {
        PROVISIONING_STATUS_LATEST_PROVISIONED
    } else {
        PROVISIONING_STATUS_LATEST_NOT_PROVISIONED
    }
}

/// The refusal a `ProvisioningStatus` outside the model's enum earns, as the message of
/// the caller's own `ValidationException`. BUILT from the member the request got wrong
/// and the values the model declares for it
/// ([`crate::ops::types::PROVISIONING_STATUS_VALUES`]), so the caller can fix the request
/// from this text alone.
pub fn provisioning_status_unknown_message(provisioning_status: &str) -> String {
    format!(
        "ProvisioningStatus must be one of {}, got {provisioning_status}",
        crate::ops::types::PROVISIONING_STATUS_VALUES.join(", ")
    )
}

/// `StatusValues::IN_PROGRESS` — the NON-TERMINAL status an assignment operation carries
/// from the call that starts it until the instant its completion falls due (see
/// [`ASSIGNMENT_OPERATION_SPAN`]).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// How long after the call that STARTS an account-assignment operation that operation's
/// move from `IN_PROGRESS` to `SUCCEEDED` falls due.
///
/// Creating an account assignment is asynchronous — the model's `StatusValues` enum
/// carries the non-terminal `IN_PROGRESS` and `CreateAccountAssignment`'s own
/// documentation says "After a successful response, call
/// DescribeAccountAssignmentCreationStatus to describe the status of an assignment
/// creation request", which is only meaningful while the operation is still in flight —
/// and AWS publishes no duration for it, so this span is the emulator's OWN choice, on
/// the same footing as [`ABAC_ENABLE_SPAN`]: never zero, so a caller that polls straight
/// after the create still observes the in-flight status, and short enough that a test can
/// wait it out. It is held as the instant the completion falls due and the status is
/// DERIVED from it ([`AssignmentOperationRecord::status`]); nothing sleeps for it.
pub const ASSIGNMENT_OPERATION_SPAN: Duration = Duration::from_millis(500);

/// The page size a paginated read uses when the caller sends no `MaxResults`, and the
/// ceiling it clamps a caller's value to: the model's `MaxResults` shape is
/// `range(min: 1, max: 100)`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The largest page `ListApplicationAccessScopes` can answer: the maximum its own
/// `MaxResults` MEMBER declares, which narrows the `MaxResults` shape's `range(min: 1,
/// max: 100)` and is the same bound its response's `Scopes` list carries
/// (`length(min: 0, max: 10)`), so no larger page is expressible.
///
/// The validating walk enforces the constraints the SCHEMA carries, which are the target
/// shape's, so the handler of an operation whose member narrows them states the narrower
/// bound itself — see [`crate::service::refusal::value_above_maximum`].
pub const MAX_ACCESS_SCOPES_PER_PAGE: i32 = 10;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores ([`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::account_assignments`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_operations`]).
///
/// This is NOT AWS's service quota — IAM Identity Center's published permission-set and
/// assignment limits are per-instance and adjustable, and nothing here claims to
/// reproduce them. It is the emulator's own MEMORY CEILING: every successful create
/// inserts a record that lives until a caller deletes it, so without a ceiling a caller
/// could grow the process until the host runs out of memory. A request that would cross
/// it is refused with the modelled `ServiceQuotaExceededException`, which AWS documents
/// as "the principal has crossed the permitted number of resources that can be
/// created", so the refusal is one a real client already handles.
///
/// One store does not refuse at this ceiling:
/// [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_deletions`] holds the
/// history of deletions that have ALREADY been applied, so it evicts its oldest records
/// instead (see
/// [`ComAmazonawsSsoadminSwbexternalserviceState::record_assignment_deletion`]).
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"permission sets"`.
pub fn quota_exceeded_message(store: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// The maximum number of ACCESS SCOPES this emulator keeps on any ONE application
/// ([`ApplicationRecord::access_scopes`]).
///
/// This is NOT an AWS quota — IAM Identity Center publishes none for it. It is the
/// emulator's own MEMORY CEILING, the per-application counterpart of
/// [`MAX_RECORDS_PER_STORE`]: `PutApplicationAccessScope` inserts one entry per distinct
/// `Scope` name, the model's `Scope` pattern admits unboundedly many of them, and a
/// stored scope lives until a caller deletes it — so without a ceiling a caller could
/// grow the process until the host runs out of memory. A put that would add a scope
/// beyond it is refused.
pub const MAX_SCOPES_PER_APPLICATION: usize = 100;

/// The message a put refuses a new access scope with once an application already holds
/// [`MAX_SCOPES_PER_APPLICATION`] of them; built from the request, which names the
/// application the ceiling was reached on.
pub fn access_scope_ceiling_message(application_arn: &str) -> String {
    format!(
        "Cannot add another access scope: this emulator holds at most \
         {MAX_SCOPES_PER_APPLICATION} access scopes per application, and \
         {application_arn} already holds that many"
    )
}

/// The maximum number of REGIONS this emulator keeps on any ONE instance
/// ([`Instance::regions`]).
///
/// This is NOT an AWS quota — IAM Identity Center publishes none for the number of
/// Regions an instance can be enabled in. It is the emulator's own MEMORY CEILING, the
/// per-instance counterpart of [`MAX_SCOPES_PER_APPLICATION`]: enabling an instance in
/// another Region adds one record per distinct `RegionName`, the model's `RegionName`
/// pattern (`^([a-z]+-){2,3}\d$`) admits unboundedly many of them, and a Region record
/// lives until a caller removes it — so without a ceiling a caller could grow the process
/// until the host runs out of memory. [`Instance::add_region`] is the one place a record
/// is appended, and it refuses an add that would cross this ceiling.
pub const MAX_REGIONS_PER_INSTANCE: usize = 50;

/// The message an add refuses a new Region with once an instance already holds
/// [`MAX_REGIONS_PER_INSTANCE`] of them; built from the instance the ceiling was reached
/// on, and worded as the modelled `ServiceQuotaExceededException` describes itself ("the
/// principal has crossed the permitted number of resources that can be created"), so the
/// refusal is one a real client already handles.
pub fn region_ceiling_message(instance_arn: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: this emulator holds at most {MAX_REGIONS_PER_INSTANCE} Regions per \
         instance, and {instance_arn} already holds that many"
    )
}

/// The maximum number of TAGS this emulator keeps on any ONE taggable resource (an
/// instance, a permission set, an application or a trusted token issuer).
///
/// Taken from the model rather than chosen: `TagList` — the shape of both
/// `TagResource$Tags` and `ListTagsForResource$Tags` — is declared `length(min: 0,
/// max: 75)`, so a resource that held more tags than this could not be reported by the
/// read at all. It doubles as the per-resource memory ceiling: a stored tag lives until
/// a caller removes it, and a write that would push a resource past it is refused with
/// the modelled `ServiceQuotaExceededException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 75;

/// The message a tagging write refuses with once a resource would carry more than
/// [`MAX_TAGS_PER_RESOURCE`] distinct keys; built from the request, which names the
/// resource the ceiling was reached on, and worded as the modelled
/// `ServiceQuotaExceededException` describes itself ("the principal has crossed the
/// permitted number of resources that can be created").
pub fn tag_ceiling_message(resource_arn: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: a resource carries at most {MAX_TAGS_PER_RESOURCE} tags, and \
         {resource_arn} cannot carry the requested ones as well"
    )
}

/// The refusal a `ResourceArn` this account holds no TAGGABLE resource for earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`application_not_found_message`]: it names the ARN that
/// resolved to nothing. An ARN held under a different instance than the `InstanceArn`
/// the request carried resolves to nothing too — see
/// [`ComAmazonawsSsoadminSwbexternalserviceState::taggable_resource_tags`].
pub fn taggable_resource_not_found_message(resource_arn: &str) -> String {
    format!("Could not find a taggable resource with arn {resource_arn}.")
}

/// Merge `requested` into the tag set `stored`, which is what a tagging write composes
/// with what a resource already carries: a key the resource already holds takes the
/// requested value in place, a key it does not hold is added, and every other key is
/// left untouched.
///
/// A tag set is keyed by `Tag$Key`, and the model declares no `@uniqueItems` on
/// `TagList`, so one request may name a key twice — and a create that stored its `Tags`
/// as the caller sent them may hold a key twice already. The value a key ends up with is
/// the one written LAST, and a key this merge writes is left holding ONE entry, so the
/// value stored here is the value [`tags_in_key_order`] reports.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) {
    for tag in requested {
        let mut written = false;
        stored.retain_mut(|held| {
            if held.key != tag.key {
                return true;
            }
            if written {
                // A second entry for the same key: the set holds one value per key, so
                // the entry this write just valued is the one that stays.
                return false;
            }
            held.value = tag.value.clone();
            written = true;
            true
        });
        if !written {
            stored.push(tag.clone());
        }
    }
}

/// How many DISTINCT keys `stored` would hold once `requested` had been merged into it
/// ([`merge_tags`]).
///
/// What [`MAX_TAGS_PER_RESOURCE`] is judged against, so overwriting the value of a key a
/// resource already carries never crosses the ceiling: it adds no tag.
pub fn merged_tag_count(stored: &[Tag], requested: &[Tag]) -> usize {
    let keys: BTreeSet<&str> = stored
        .iter()
        .chain(requested)
        .map(|tag| tag.key.as_str())
        .collect();
    keys.len()
}

/// The tag set `tags` as the `(page key, item)` pairs [`page`] enumerates: one entry per
/// distinct `Tag$Key`, in key order.
///
/// The KEY is both the page key and the identity of a tag, so the enumeration order is
/// stable across calls however the tags were written, and a pagination cursor resumes
/// after a key rather than after a position a later write would shift.
pub fn tags_in_key_order(tags: &[Tag]) -> Vec<(String, Tag)> {
    let ordered: BTreeMap<&str, &str> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    ordered
        .into_iter()
        .map(|(key, value)| {
            (
                key.to_string(),
                Tag {
                    key: key.to_string(),
                    value: value.to_string(),
                },
            )
        })
        .collect()
}

/// The `AuthenticationMethodType` an [`AuthenticationMethod`] union value carries.
///
/// `AuthenticationMethodItem$AuthenticationMethod` documents the relationship — "The
/// contents of this structure is determined by the `AuthenticationMethodType`" — so the
/// type is a PROPERTY of the union value rather than an independent member, and this
/// function is the single place the mapping lives. The model's `AuthenticationMethod`
/// union carries one variant (`Iam`) and its `AuthenticationMethodType` enum one value
/// (`IAM`), so today the mapping is total in both directions.
pub fn authentication_method_type_of(method: &AuthenticationMethod) -> &'static str {
    match method {
        AuthenticationMethod::Iam(_) => AUTHENTICATION_METHOD_TYPE_IAM,
    }
}

/// `AuthenticationMethodType::IAM` — the one value the model's `AuthenticationMethodType`
/// enum admits (`types::AUTHENTICATION_METHOD_TYPE_VALUES`).
pub const AUTHENTICATION_METHOD_TYPE_IAM: &str = "IAM";

/// The refusal a `GetApplicationAuthenticationMethod` of a type the application carries
/// no authentication method for earns, as the message of the modelled
/// `ResourceNotFoundException`.
///
/// Built from the request, like [`access_scope_not_found_message`]: it names both the
/// type and the application it was looked for on, because an unconfigured type on a known
/// application is a different failure from an unknown application.
pub fn authentication_method_not_found_message(
    application_arn: &str,
    authentication_method_type: &str,
) -> String {
    format!(
        "Could not find authentication method {authentication_method_type} for \
         application {application_arn}."
    )
}

/// The refusal a put whose `AuthenticationMethodType` disagrees with the variant its
/// `AuthenticationMethod` union carries earns, as the message of the modelled
/// `ValidationException`.
///
/// Built from the request: it names both halves, since the caller has to learn which of
/// the two to change.
pub fn authentication_method_type_mismatch_message(declared: &str, carried: &str) -> String {
    format!(
        "AuthenticationMethod carries a {carried} method, which does not match the \
         AuthenticationMethodType {declared} the request declares"
    )
}

/// The refusal an `IamAuthenticationMethod$ActorPolicy` that is not a JSON OBJECT earns,
/// as the message of the modelled `ValidationException`.
///
/// The rule is the member's own documentation — "An IAM policy document in JSON" — which
/// an `@document` member cannot express: every JSON value deserializes into it, and a
/// scalar, a list or a null is not a policy document. Nothing beyond that shape is
/// checked here: the emulator stores the document the caller sent verbatim rather than
/// judging the policy language inside it.
pub const ACTOR_POLICY_NOT_AN_OBJECT_MESSAGE: &str =
    "IamAuthenticationMethod.ActorPolicy must be a JSON object: an IAM policy document";

/// `GrantType::authorization_code` — the OAuth 2.0 Authorization Code grant
/// (`types::GRANT_TYPE_VALUES`).
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";

/// `GrantType::refresh_token` — the OAuth 2.0 Refresh Token grant.
pub const GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";

/// `GrantType::urn:ietf:params:oauth:grant-type:jwt-bearer` — the JWT Bearer Token
/// Authorization grant.
pub const GRANT_TYPE_JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// `GrantType::urn:ietf:params:oauth:grant-type:token-exchange` — the OAuth 2.0 Token
/// Exchange grant.
pub const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// The `GrantType` a [`Grant`] union value carries.
///
/// The `Grant` union documents the relationship itself — "Exactly one member of the union
/// must be specified, and must match the grant type selected" — so the type is a PROPERTY
/// of the union value rather than an independent member, and this function is the single
/// place the mapping lives. Every variant the model declares maps onto one enum value, so
/// the mapping is total in both directions.
pub fn grant_type_of(grant: &Grant) -> &'static str {
    match grant {
        Grant::AuthorizationCode(_) => GRANT_TYPE_AUTHORIZATION_CODE,
        Grant::RefreshToken(_) => GRANT_TYPE_REFRESH_TOKEN,
        Grant::JwtBearer(_) => GRANT_TYPE_JWT_BEARER,
        Grant::TokenExchange(_) => GRANT_TYPE_TOKEN_EXCHANGE,
    }
}

/// The refusal a `PutApplicationGrant` whose `GrantType` disagrees with the variant its
/// `Grant` union carries earns, as the message of the modelled `ValidationException`.
///
/// The rule is the `Grant` union's own documentation ("must match the grant type
/// selected"), which the model declares as two independent members and cannot tie
/// together. Built from the request: it names both halves, since the caller has to learn
/// which of the two to change.
pub fn grant_type_mismatch_message(declared: &str, carried: &str) -> String {
    format!(
        "Grant carries a {carried} grant, which does not match the GrantType {declared} \
         the request declares"
    )
}

/// The refusal a `Grant` that omits the member its own grant type documents as required
/// earns, as the message of the modelled `ValidationException`; `member` names the
/// missing member and `grant_type` the grant that requires it.
///
/// The rule is each member's own documentation — `AuthorizationCodeGrant$RedirectUris`
/// ("RedirectUris is required when the grant type is authorization_code") and
/// `JwtBearerGrant$AuthorizedTokenIssuers` ("AuthorizedTokenIssuers is required when the
/// grant type is JwtBearerGrant"). The model marks both members OPTIONAL, because a
/// member required only under one union variant is not expressible, so the boundary
/// validator passes the request and the handler refuses it.
pub fn grant_member_required_message(member: &str, grant_type: &str) -> String {
    format!("{member} is required when the grant type is {grant_type}.")
}

/// The refusal a `GrantType` the application carries no grant for earns, as the message
/// of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`authentication_method_not_found_message`]: it names both
/// the grant type and the application it was looked for on, because an unconfigured grant
/// type on a known application is a different failure from an unknown application.
pub fn grant_not_found_message(application_arn: &str, grant_type: &str) -> String {
    format!("Could not find grant {grant_type} for application {application_arn}.")
}

/// The attribute-based access control (ABAC) configuration of an instance, as the
/// `CreateInstanceAccessControlAttributeConfiguration` that enabled ABAC — and any later
/// `UpdateInstanceAccessControlAttributeConfiguration` — set it.
///
/// The attributes are held EXACTLY as the caller sent them (same order, same
/// `Source` list), because `DescribeInstanceAccessControlAttributeConfiguration` reports
/// "the list of IAM Identity Center identity store attributes that have been added to
/// your ABAC configuration" — a round trip of the caller's own mapping.
///
/// `Status` is DERIVED from [`enabled_at`](Self::enabled_at) rather than stored, so a
/// read never has to write to move the status on.
#[derive(Debug, Clone)]
pub struct AccessControlAttributeConfigurationRecord {
    /// `InstanceAccessControlAttributeConfiguration$AccessControlAttributes`, in the
    /// order the caller sent.
    pub attributes: Vec<AccessControlAttribute>,
    /// When the configuration stops being `CREATION_IN_PROGRESS` and becomes `ENABLED`:
    /// [`ABAC_ENABLE_SPAN`] after the create that enabled ABAC was accepted.
    pub enabled_at: SystemTime,
}

impl AccessControlAttributeConfigurationRecord {
    /// The configuration a create accepted at `accepted_at` writes.
    pub fn new(attributes: Vec<AccessControlAttribute>, accepted_at: SystemTime) -> Self {
        Self {
            attributes,
            enabled_at: accepted_at + ABAC_ENABLE_SPAN,
        }
    }

    /// `InstanceAccessControlAttributeConfigurationStatus` as of `now`:
    /// `CREATION_IN_PROGRESS` until the enablement is due, `ENABLED` after.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.enabled_at {
            ABAC_STATUS_CREATION_IN_PROGRESS
        } else {
            ABAC_STATUS_ENABLED
        }
    }

    /// This configuration as the modeled `InstanceAccessControlAttributeConfiguration`
    /// shape — what a describe reports, attribute for attribute.
    pub fn to_shape(&self) -> InstanceAccessControlAttributeConfiguration {
        InstanceAccessControlAttributeConfiguration {
            access_control_attributes: self.attributes.clone(),
        }
    }
}

/// The first attribute KEY that `attributes` carries twice, if any.
///
/// An ABAC configuration MAPS a key to an identity-source value
/// (`AccessControlAttribute$Key`: "The name of the attribute associated with your
/// identities in your identity source. This is used to map a specified attribute in your
/// identity source with an attribute in IAM Identity Center"), so one key cannot carry
/// two mappings: a request that sends the same key twice is refused rather than stored
/// with one of its two values silently winning. The comparison is case-sensitive, as the
/// key is an opaque identity-source attribute name.
pub fn duplicate_attribute_key(attributes: &[AccessControlAttribute]) -> Option<&str> {
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    attributes
        .iter()
        .find(|attribute| !seen.insert(attribute.key.as_str()))
        .map(|attribute| attribute.key.as_str())
}

/// The refusal a request carrying one attribute key twice earns, as the message of the
/// modelled `ValidationException` (HTTP 400). Built from the request: it names the
/// member and the key that breaks the constraint.
pub fn duplicate_attribute_key_message(key: &str) -> String {
    format!(
        "InstanceAccessControlAttributeConfiguration.AccessControlAttributes carries the \
         attribute key {key} more than once; each key maps exactly one identity source \
         attribute"
    )
}

/// The encryption configuration an instance is encrypted under.
///
/// Every instance HAS one: data at rest is encrypted from the moment the instance
/// exists, under an Amazon Web Services owned key ([`Self::aws_owned_default`]) until a
/// caller opts into a key of its own, so this is a property of the instance rather than
/// something a write brings into being. `UpdateInstance` REPLACES it; nothing removes it.
///
/// `EncryptionStatus` is DERIVED from [`effective_at`](Self::effective_at) rather than
/// stored, so a read never has to write to move the status on: the instant a change
/// becomes effective is fixed when the update is accepted, and every later read computes
/// the status the clock implies at the moment it is asked.
#[derive(Debug, Clone)]
pub struct EncryptionConfigurationRecord {
    /// `KmsKeyType`: [`KEY_TYPE_AWS_OWNED`] or [`KEY_TYPE_CUSTOMER_MANAGED`].
    pub key_type: String,
    /// The customer managed key this instance is encrypted under; `None` for an Amazon
    /// Web Services owned key, which no ARN names.
    pub kms_key_arn: Option<String>,
    /// When the configuration stops being `UPDATING` and becomes `ENABLED`:
    /// [`ENCRYPTION_UPDATE_DELAY`] after the update that set it was accepted, or `None`
    /// for a configuration that is ALREADY in effect — the Amazon Web Services owned
    /// default an instance carries from creation, which no key change is pending on.
    pub effective_at: Option<SystemTime>,
}

impl EncryptionConfigurationRecord {
    /// The configuration an `UpdateInstance` accepted at `accepted_at` writes.
    pub fn new(key_type: String, kms_key_arn: Option<String>, accepted_at: SystemTime) -> Self {
        Self {
            key_type,
            kms_key_arn,
            effective_at: Some(accepted_at + ENCRYPTION_UPDATE_DELAY),
        }
    }

    /// The configuration an instance nobody has reconfigured is encrypted under: an
    /// Amazon Web Services owned key, in effect already — so no `KmsKeyArn` names it
    /// ("Cannot be specified when KeyType is AWS_OWNED_KMS_KEY") and no key change is in
    /// flight.
    pub fn aws_owned_default() -> Self {
        Self {
            key_type: KEY_TYPE_AWS_OWNED.to_string(),
            kms_key_arn: None,
            effective_at: None,
        }
    }

    /// `KmsKeyStatus` as of `now`: `UPDATING` while a key change is not due yet,
    /// `ENABLED` once it is — and for a configuration already in effect.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.effective_at.is_some_and(|effective_at| now < effective_at) {
            KMS_KEY_STATUS_UPDATING
        } else {
            KMS_KEY_STATUS_ENABLED
        }
    }

    /// This configuration as the modeled `EncryptionConfigurationDetails` shape, as of
    /// `now`.
    ///
    /// `EncryptionStatusReason` is left absent: the model documents it as context for a
    /// FAILED update ("particularly useful when the encryption status is UPDATE_FAILED"),
    /// and nothing here fails a key change.
    pub fn to_shape(&self, now: SystemTime) -> EncryptionConfigurationDetails {
        EncryptionConfigurationDetails {
            key_type: Some(self.key_type.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            encryption_status: Some(self.status(now).to_string()),
            encryption_status_reason: None,
        }
    }
}

/// One enabled Region of an IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Region {
    pub region_name: String,
    pub status: String,
    pub added_date: DateTime,
    pub is_primary_region: bool,
    /// When the clock owes this Region record its REMOVAL — the end of the `REMOVING` a
    /// `RemoveRegion` put it into — or `None` for a Region nothing is removing. Stored as
    /// a due INSTANT and applied by [`Instance::materialize_due`], which drops the record;
    /// nothing sleeps for it.
    pub removed_due: Option<SystemTime>,
}

impl Region {
    /// The PRIMARY Region of an instance: the Region it was enabled in, in its terminal
    /// `ACTIVE` status. It is the one Region of `Regions` that carries `IsPrimaryRegion`,
    /// and `RemoveRegion` refuses to remove it ("The primary Region cannot be removed.").
    pub fn primary(region_name: &str, added_date: DateTime) -> Self {
        Self {
            is_primary_region: true,
            ..Self::active(region_name, added_date)
        }
    }

    /// An additional Region of an instance, in its terminal `ACTIVE` status: a Region
    /// whose extension workflow has finished. NOT the primary one — the primary Region is
    /// the one the instance was enabled in, which [`Instance::active`] holds.
    pub fn active(region_name: &str, added_date: DateTime) -> Self {
        Self {
            region_name: region_name.to_string(),
            status: STATUS_ACTIVE.to_string(),
            added_date,
            is_primary_region: false,
            removed_due: None,
        }
    }

    /// Start this Region's ENABLEMENT at this record: `RegionStatus::ADDING`, the value
    /// `RegionMetadata$Status` documents for a Region whose "extension workflow is in
    /// progress", which is what a Region carries until that workflow finishes and leaves
    /// it `ACTIVE`.
    ///
    /// `RegionStatus` (`ACTIVE | ADDING | REMOVING`) is a DIFFERENT enum from the
    /// instance-level `InstanceStatus` (`CREATE_IN_PROGRESS | CREATE_FAILED |
    /// DELETE_IN_PROGRESS | ACTIVE`), so a Region record never carries an instance-level
    /// value however the enablement was started: this is the one in-flight value it has.
    pub fn start_addition(&mut self) {
        self.status = STATUS_ADDING.to_string();
    }

    /// Whether an add or remove Region workflow is in progress on this Region: the two
    /// non-terminal values of the model's `RegionStatus` enum (`ADDING` — "Region extension
    /// workflow is in progress" — and `REMOVING` — "Region removal workflow is in
    /// progress").
    pub fn workflow_in_progress(&self) -> bool {
        self.status == STATUS_ADDING || self.status == STATUS_REMOVING
    }

    /// Start this Region's removal at `now`: `REMOVING` until the record's deletion falls
    /// due, [`REGION_REMOVAL_SPAN`] later.
    ///
    /// Idempotent on a Region already being removed — the due instant the first
    /// `RemoveRegion` fixed is kept, so a repeated remove cannot push the deletion out.
    pub fn start_removal(&mut self, now: SystemTime) {
        if self.removed_due.is_some() {
            return;
        }
        self.status = STATUS_REMOVING.to_string();
        self.removed_due = Some(now + REGION_REMOVAL_SPAN);
    }

    /// Whether the clock already owes this Region record its DELETION.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removed_due.is_some_and(|due| due <= now)
    }

    /// This Region as the modeled `RegionMetadata` shape.
    pub fn to_shape(&self) -> RegionMetadata {
        RegionMetadata {
            region_name: Some(self.region_name.clone()),
            status: Some(self.status.clone()),
            added_date: Some(self.added_date),
            is_primary_region: self.is_primary_region,
        }
    }
}

/// An IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Instance {
    pub instance_arn: String,
    pub identity_store_id: String,
    pub identity_store_arn: String,
    pub owner_account_id: String,
    /// `InstanceMetadata$Name` — optional in the model, so an instance created without
    /// one carries none rather than a name the service invented.
    pub name: Option<String>,
    pub created_date: DateTime,
    pub status: String,
    pub primary_region: String,
    pub regions: Vec<Region>,
    /// Account instance or organization instance: what the instance's INITIAL
    /// permission-set enablement comes from (see
    /// [`InstanceType::permission_sets_enabled`]).
    pub instance_type: InstanceType,
    /// `DescribeInstance$PermissionSetsEnabled` — "Indicates whether permission sets are
    /// enabled for this Identity Center instance", and what
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn`]
    /// asks. It starts at what [`InstanceType::permission_sets_enabled`] gives this kind
    /// of instance and `UpdateInstance` can turn it ON — "Enables permission sets for
    /// this Identity Center instance. The only accepted value is true. After permission
    /// sets are enabled, they cannot be disabled" — so it is held per instance rather
    /// than decided per read. `InstanceMetadata` (what `ListInstances` projects) carries
    /// no such member, so only `DescribeInstance` reports it.
    pub permission_sets_enabled: bool,
    /// The encryption configuration this instance's data at rest is encrypted under.
    /// Never absent: an instance is encrypted from the moment it exists, under the
    /// Amazon Web Services owned key default
    /// ([`EncryptionConfigurationRecord::aws_owned_default`]), and `UpdateInstance`
    /// REPLACES that configuration rather than bringing one into being.
    pub encryption: EncryptionConfigurationRecord,
    /// The instance's ABAC configuration, or `None` while ABAC is not enabled on it:
    /// `CreateInstanceAccessControlAttributeConfiguration` is what enables it,
    /// `UpdateInstanceAccessControlAttributeConfiguration` replaces the attributes it
    /// holds and `DeleteInstanceAccessControlAttributeConfiguration` disables it again,
    /// so an instance nobody has enabled ABAC on holds no attribute mapping rather than
    /// an invented one.
    pub access_control_attributes: Option<AccessControlAttributeConfigurationRecord>,
    /// The `ClientToken` of the `CreateInstance` call that minted this instance, when
    /// it sent one. The model marks that member `@idempotencyToken`, so a retry of the
    /// same call must answer with this instance rather than refusing it as a duplicate.
    pub client_token: Option<String>,
    /// The tags `CreateInstance` attached to the instance.
    pub tags: Vec<Tag>,
    /// When the clock owes this instance its move out of `CREATE_IN_PROGRESS`, or `None`
    /// for an instance that is already in a terminal status. Stored as a due INSTANT and
    /// applied by [`Instance::materialize_due`]; nothing sleeps for it.
    pub active_due: Option<SystemTime>,
    /// When the clock owes this instance its REMOVAL — the end of the
    /// `DELETE_IN_PROGRESS` the `DeleteInstance` that started it put the instance in — or
    /// `None` for an instance nothing is deleting. Stored as a due INSTANT and applied by
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`], which drops the
    /// record; nothing sleeps for it.
    pub deleted_due: Option<SystemTime>,
}

impl Instance {
    /// An instance in its terminal `ACTIVE` status, owned by the emulator's account and
    /// enabled in `region` alone — what an instance looks like once its creation has
    /// finished.
    ///
    /// `region` is the Region the instance was ENABLED in (for a create, the Region of
    /// the request that created it — [`request_region`]), which is what
    /// `PrimaryRegion` reports and the one Region of `Regions` that carries
    /// `IsPrimaryRegion`.
    pub fn active(
        instance_id: &str,
        identity_store_id: &str,
        instance_type: InstanceType,
        name: Option<String>,
        created_date: DateTime,
        region: &str,
    ) -> Self {
        Self {
            instance_arn: instance_arn(instance_id),
            identity_store_id: identity_store_id.to_string(),
            identity_store_arn: identity_store_arn(identity_store_id),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            name,
            created_date,
            status: STATUS_ACTIVE.to_string(),
            primary_region: region.to_string(),
            regions: vec![Region::primary(region, created_date)],
            instance_type,
            // What this KIND of instance starts with; `UpdateInstance` can enable
            // permission sets on an instance that starts without them.
            permission_sets_enabled: instance_type.permission_sets_enabled(),
            // An instance is encrypted at rest from the moment it exists, and
            // `CreateInstance` takes no encryption configuration to say otherwise, so it
            // starts under the Amazon Web Services owned key default; `UpdateInstance`
            // is what opts it into a customer managed key.
            encryption: EncryptionConfigurationRecord::aws_owned_default(),
            // ABAC is off until a caller enables it: `CreateInstance` takes no attribute
            // configuration, so there is none to report until a
            // `CreateInstanceAccessControlAttributeConfiguration` writes one.
            access_control_attributes: None,
            client_token: None,
            tags: Vec::new(),
            active_due: None,
            deleted_due: None,
        }
    }

    /// A just-created instance: `CREATE_IN_PROGRESS`, enabled in `region`, with its
    /// activation owed [`INSTANCE_ACTIVATION_SPAN`] after `now`.
    pub fn creating(
        instance_id: &str,
        identity_store_id: &str,
        instance_type: InstanceType,
        name: Option<String>,
        now: SystemTime,
        region: &str,
    ) -> Self {
        let mut instance = Self::active(
            instance_id,
            identity_store_id,
            instance_type,
            name,
            DateTime::from(now),
            region,
        );
        instance.status = STATUS_CREATE_IN_PROGRESS.to_string();
        // The INSTANCE is `CREATE_IN_PROGRESS` (an `InstanceStatus`), while each Region's
        // own enablement is still in flight, which `RegionStatus` spells `ADDING`: the two
        // members are typed by different enums, and only the instance-level one admits
        // `CREATE_IN_PROGRESS`.
        for region in &mut instance.regions {
            region.start_addition();
        }
        instance.active_due = Some(now + INSTANCE_ACTIVATION_SPAN);
        instance
    }

    /// Start this instance's deletion at `now`: `DELETE_IN_PROGRESS` until the removal
    /// falls due, [`INSTANCE_DELETION_SPAN`] later.
    ///
    /// Idempotent on an instance already being deleted — the due instant the first
    /// `DeleteInstance` fixed is kept, so a repeated delete cannot push the removal out.
    pub fn start_deletion(&mut self, now: SystemTime) {
        if self.deleted_due.is_some() {
            return;
        }
        // The creation is no longer owed: the instance is on its way out, not in. The
        // Regions keep the status they had — `RegionStatus` has no deleting value, and
        // the instance's own `Status` is what says it is going away.
        self.active_due = None;
        self.status = STATUS_DELETE_IN_PROGRESS.to_string();
        self.deleted_due = Some(now + INSTANCE_DELETION_SPAN);
    }

    /// Whether the clock already owes this instance its REMOVAL from the store.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.deleted_due.is_some_and(|due| due <= now)
    }

    /// Enable this instance in one MORE Region, or refuse the add when the instance
    /// already holds [`MAX_REGIONS_PER_INSTANCE`] of them.
    ///
    /// The one place a Region record is appended to an instance, so the emulator's memory
    /// ceiling cannot be bypassed by whatever enables a Region. The `Err` carries the
    /// message the refusal reports ([`region_ceiling_message`]), which a caller sees as
    /// the modelled `ServiceQuotaExceededException`.
    pub fn add_region(&mut self, region: Region) -> Result<(), String> {
        if self.regions.len() >= MAX_REGIONS_PER_INSTANCE {
            return Err(region_ceiling_message(&self.instance_arn));
        }
        self.regions.push(region);
        Ok(())
    }

    /// Apply what the CLOCK already owes this instance: an instance whose activation
    /// instant has passed is `ACTIVE`, in every Region it holds, and a Region whose own
    /// removal instant has passed ([`Region::start_removal`]) is no longer one of its
    /// Regions. Idempotent, and it changes nothing before an instant falls due, so a read
    /// that calls it answers exactly what a read at the same instant would have answered
    /// without it.
    ///
    /// The REMOVAL a delete owes the INSTANCE is applied by the store rather than here,
    /// since it drops the whole record: see
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`].
    pub fn materialize_due(&mut self, now: SystemTime) {
        // A Region whose removal workflow has run out no longer exists, whether or not
        // anybody called: `RemoveRegion` is documented as deleting the Region record "when
        // the workflow completes".
        self.regions.retain(|region| !region.removal_due(now));
        if self.deleted_due.is_some() {
            return;
        }
        if self.active_due.is_some_and(|due| due <= now) {
            self.active_due = None;
            self.status = STATUS_ACTIVE.to_string();
            for region in &mut self.regions {
                // A Region on its way OUT keeps its `REMOVING`: the instance's activation
                // says nothing about a removal workflow another call started.
                if region.removed_due.is_none() {
                    region.status = STATUS_ACTIVE.to_string();
                }
            }
        }
    }

    /// The Region of this instance whose name is `region_name`, or `None` when the
    /// instance is not enabled in it.
    pub fn region(&self, region_name: &str) -> Option<&Region> {
        self.regions
            .iter()
            .find(|region| region.region_name == region_name)
    }

    /// The same Region, for a handler that updates it.
    pub fn region_mut(&mut self, region_name: &str) -> Option<&mut Region> {
        self.regions
            .iter_mut()
            .find(|region| region.region_name == region_name)
    }

    /// This instance as the modeled `InstanceMetadata` shape.
    ///
    /// `StatusReason` is left absent: the model documents it as additional context for
    /// an instance in a non-ACTIVE state, and there is none to give — a
    /// `CREATE_IN_PROGRESS` instance is on its way to ACTIVE, which the status already
    /// says.
    pub fn to_metadata(&self) -> InstanceMetadata {
        InstanceMetadata {
            instance_arn: Some(self.instance_arn.clone()),
            identity_store_id: Some(self.identity_store_id.clone()),
            identity_store_arn: Some(self.identity_store_arn.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            name: self.name.clone(),
            created_date: Some(self.created_date),
            status: Some(self.status.clone()),
            status_reason: None,
            primary_region: Some(self.primary_region.clone()),
            regions: Some(self.regions.iter().map(Region::to_shape).collect()),
        }
    }
}

/// The `Path` a customer-managed policy reference carries when the request names none.
///
/// `CustomerManagedPolicyReference$Path`'s own documentation: "The path to the IAM policy
/// that you have configured in each account where you want to deploy your permission set.
/// The default is `/`." So the stored reference is normalised to it at write time and
/// every later read reports the same path, whether or not the writer sent one.
pub const DEFAULT_MANAGED_POLICY_PATH: &str = "/";

/// One customer managed policy reference ATTACHED to a permission set.
///
/// `CustomerManagedPolicyReference` is a (`Name`, `Path`) pair naming an IAM policy that
/// the caller has deployed in each member account, so the reference's IDENTITY is that
/// pair — the same name under a different path is a different policy. The stored `Path` is
/// always explicit: a reference attached without one is normalised to
/// [`DEFAULT_MANAGED_POLICY_PATH`], the value the member documents as its default, so
/// every later read and every identity comparison sees one form of the same reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerManagedPolicyReferenceRecord {
    /// `CustomerManagedPolicyReference$Name` (`ManagedPolicyName`, 1..128).
    pub name: String,
    /// `CustomerManagedPolicyReference$Path` (`ManagedPolicyPath`, 1..512), materialised.
    pub path: String,
}

impl CustomerManagedPolicyReferenceRecord {
    /// The record a modeled `CustomerManagedPolicyReference` describes, with its `Path`
    /// materialised to the documented default when the request named none.
    pub fn from_shape(reference: &CustomerManagedPolicyReference) -> Self {
        Self {
            name: reference.name.clone(),
            path: reference
                .path
                .clone()
                .unwrap_or_else(|| DEFAULT_MANAGED_POLICY_PATH.to_string()),
        }
    }

    /// This reference's IDENTITY, and the key an enumeration of a permission set's
    /// references orders and pages by.
    ///
    /// `|` separates the two parts unambiguously: it is in neither member's character
    /// class (`ManagedPolicyName` is `^[\w+=,.@-]+$` and `ManagedPolicyPath` is
    /// `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`), so no two distinct pairs share a key.
    pub fn identity(&self) -> String {
        format!("{}|{}", self.name, self.path)
    }

    /// This record as the modeled `CustomerManagedPolicyReference` shape — what
    /// `ListCustomerManagedPolicyReferencesInPermissionSet` reports, so a read answers
    /// with the pair the attach stored and the `Path` it stored for it.
    pub fn to_shape(&self) -> CustomerManagedPolicyReference {
        CustomerManagedPolicyReference {
            name: self.name.clone(),
            path: Some(self.path.clone()),
        }
    }
}

/// The greatest number of customer managed policy references this emulator holds on ONE
/// permission set.
///
/// NOT a figure the model carries: it is the emulator's own ceiling, the per-permission-set
/// counterpart of [`MAX_RECORDS_PER_STORE`] and of [`MAX_SCOPES_PER_APPLICATION`]. An
/// attach inserts one entry per distinct (`Name`, `Path`) pair, the two members' patterns
/// admit unboundedly many of them, and an attached reference lives until a caller detaches
/// it — so without a ceiling a caller could grow the process until the host runs out of
/// memory. An attach that would add a reference beyond it is refused with the modelled
/// `ServiceQuotaExceededException`, which AWS documents as "the principal has crossed the
/// permitted number of resources that can be created".
pub const MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET: usize = 20;

/// The message an attach refuses a new reference with once a permission set already holds
/// [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`] of them; built from the request,
/// which names the permission set the ceiling was reached on.
pub fn customer_managed_policy_ceiling_message(permission_set_arn: &str) -> String {
    format!(
        "Cannot attach another customer managed policy reference: this emulator holds at \
         most {MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET} customer managed policy \
         references per permission set, and {permission_set_arn} already holds that many"
    )
}

/// The refusal an ATTACH of a reference the permission set ALREADY holds earns, as the
/// message of the modelled `ConflictException` (HTTP 409) — "a conflict with a previous
/// successful write".
///
/// Built from the request: it names the pair that is already attached and the permission
/// set it is attached to, because a client has to tell which of the request's resources
/// the conflict is about.
pub fn customer_managed_policy_already_attached_message(
    permission_set_arn: &str,
    name: &str,
    path: &str,
) -> String {
    format!(
        "A customer managed policy reference with name {name} and path {path} is already \
         attached to permission set {permission_set_arn}."
    )
}

/// The refusal a detach of a reference the permission set does not hold earns, as the
/// message of the modelled `ResourceNotFoundException` (HTTP 404).
///
/// Built from the request: it names the pair that resolved to nothing and the permission
/// set it was looked for on, because an unattached reference on a known permission set is
/// a different failure from an unknown permission set.
pub fn customer_managed_policy_not_attached_message(
    permission_set_arn: &str,
    name: &str,
    path: &str,
) -> String {
    format!(
        "Could not find customer managed policy reference with name {name} and path \
         {path} attached to permission set {permission_set_arn}."
    )
}

/// Cursor scope of a `ListCustomerManagedPolicyReferencesInPermissionSet` enumeration:
/// the operation and the permission set whose references it walks (the instance included,
/// since the same permission-set ARN under another instance is another resource).
pub fn cursor_customer_managed_policies(instance_arn: &str, permission_set_arn: &str) -> String {
    format!("customerManagedPolicies|{instance_arn}|{permission_set_arn}")
}

/// The greatest number of AWS MANAGED POLICY ARNs this emulator holds on ONE permission
/// set.
///
/// NOT a figure the model carries: it is the emulator's own ceiling, the AWS-managed
/// counterpart of [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`]. An attach inserts
/// one entry per distinct `ManagedPolicyArn`, the member's pattern admits unboundedly many
/// of them, and an attached policy lives until a caller detaches it — so without a ceiling
/// a caller could grow the process until the host runs out of memory. An attach that would
/// add a policy beyond it is refused with the modelled `ServiceQuotaExceededException`,
/// which AWS documents as "the principal has crossed the permitted number of resources
/// that can be created".
pub const MAX_MANAGED_POLICIES_PER_PERMISSION_SET: usize = 20;

/// The message an attach refuses a new managed policy with once a permission set already
/// holds [`MAX_MANAGED_POLICIES_PER_PERMISSION_SET`] of them; built from the request,
/// which names the permission set the ceiling was reached on.
pub fn managed_policy_ceiling_message(permission_set_arn: &str) -> String {
    format!(
        "Cannot attach another managed policy: this emulator holds at most \
         {MAX_MANAGED_POLICIES_PER_PERMISSION_SET} managed policies per permission set, \
         and {permission_set_arn} already holds that many"
    )
}

/// The refusal an ATTACH of an AWS managed policy the permission set ALREADY carries
/// earns, as the message of the modelled `ConflictException` (HTTP 409) — "a conflict
/// with a previous successful write".
///
/// Built from the request, like
/// [`customer_managed_policy_already_attached_message`]: it names the ARN that is already
/// attached and the permission set it is attached to.
pub fn managed_policy_already_attached_message(
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> String {
    format!(
        "Managed policy with arn {managed_policy_arn} is already attached to permission \
         set {permission_set_arn}."
    )
}

/// The refusal a detach of an AWS managed policy the permission set does not hold earns,
/// as the message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: it names the ARN that resolved to nothing and the permission set
/// it was looked for on, because an unattached policy on a known permission set is a
/// different failure from an unknown permission set.
pub fn managed_policy_not_attached_message(
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> String {
    format!(
        "Could not find managed policy with arn {managed_policy_arn} attached to \
         permission set {permission_set_arn}."
    )
}

/// The permissions boundary attached to a permission set: EXACTLY ONE of an AWS managed
/// policy ARN or a customer managed policy reference.
///
/// Modeled as a structure with two optional members, but `PermissionsBoundary`'s own
/// documentation makes it a choice — "Specify either `CustomerManagedPolicyReference` to
/// use the name and path of a customer managed policy, or `ManagedPolicyArn` to use the
/// ARN of an Amazon Web Services managed policy" — so the state holds the CHOICE rather
/// than the pair, and a request that names both or neither never reaches the store
/// ([`PermissionsBoundaryRecord::from_shape`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsBoundaryRecord {
    /// `PermissionsBoundary$ManagedPolicyArn`.
    ManagedPolicy { managed_policy_arn: String },
    /// `PermissionsBoundary$CustomerManagedPolicyReference`, with `Path` normalised to
    /// [`DEFAULT_MANAGED_POLICY_PATH`] when the request named none.
    CustomerManagedPolicy { name: String, path: String },
}

impl PermissionsBoundaryRecord {
    /// The record a modeled `PermissionsBoundary` describes, or `None` when it names
    /// BOTH policy kinds or NEITHER — the one thing the model's two optional members
    /// cannot express, and which the shape's documented "specify either … or …" forbids.
    pub fn from_shape(boundary: &PermissionsBoundary) -> Option<Self> {
        match (
            &boundary.customer_managed_policy_reference,
            &boundary.managed_policy_arn,
        ) {
            (Some(reference), None) => {
                // The `Path` default is materialised in exactly ONE place —
                // [`CustomerManagedPolicyReferenceRecord::from_shape`] — so a boundary's
                // reference and an attached reference normalise identically.
                let reference = CustomerManagedPolicyReferenceRecord::from_shape(reference);
                Some(Self::CustomerManagedPolicy {
                    name: reference.name,
                    path: reference.path,
                })
            }
            (None, Some(managed_policy_arn)) => Some(Self::ManagedPolicy {
                managed_policy_arn: managed_policy_arn.clone(),
            }),
            _ => None,
        }
    }

    /// This record as the modeled `PermissionsBoundary` shape — what
    /// `GetPermissionsBoundaryForPermissionSet` answers with, so a read reports the
    /// choice the write stored and nothing else.
    pub fn to_shape(&self) -> PermissionsBoundary {
        match self {
            Self::ManagedPolicy { managed_policy_arn } => PermissionsBoundary {
                customer_managed_policy_reference: None,
                managed_policy_arn: Some(managed_policy_arn.clone()),
            },
            Self::CustomerManagedPolicy { name, path } => PermissionsBoundary {
                // Projected by the same record that projects an attached reference, so
                // the two cannot disagree about the shape they report.
                customer_managed_policy_reference: Some(
                    CustomerManagedPolicyReferenceRecord {
                        name: name.clone(),
                        path: path.clone(),
                    }
                    .to_shape(),
                ),
                managed_policy_arn: None,
            },
        }
    }
}

/// The refusal a `PermissionsBoundary` that names both policy kinds, or neither, earns —
/// as the message of the modelled `ValidationException` (HTTP 400).
///
/// BUILT from the members the request got wrong, which is what the shape's documented
/// choice ("Specify either `CustomerManagedPolicyReference` … or `ManagedPolicyArn`") is
/// about; the caller has to fix the request from this text alone.
pub const PERMISSIONS_BOUNDARY_CHOICE_MESSAGE: &str =
    "Specify either CustomerManagedPolicyReference or ManagedPolicyArn in \
     PermissionsBoundary, but not both.";

/// The refusal a delete of a permissions boundary a permission set does not carry earns, as
/// the message of the modelled `ResourceNotFoundException` (HTTP 404).
///
/// The BOUNDARY is the resource of that call ("Deletes the permissions boundary from a
/// specified PermissionSet"), so a permission set carrying none has nothing for the request
/// to name — the same answer an absent singleton child earns elsewhere in this service. The
/// message is built from the request, which names the permission set it was looked for on,
/// because an absent boundary on a known permission set is a different failure from an
/// unknown permission set.
pub fn permissions_boundary_not_attached_message(permission_set_arn: &str) -> String {
    format!("Could not find permissions boundary for permission set {permission_set_arn}.")
}

/// A permission set, keyed in [`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`]
/// by the ARN minted for it at creation.
///
/// `managed_policies`, `inline_policy` and `permissions_boundary` are held here so the
/// policy-attachment operations have one record to extend rather than a second map.
#[derive(Debug, Clone)]
pub struct PermissionSetRecord {
    pub permission_set_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub created_date: DateTime,
    pub session_duration: Option<String>,
    pub relay_state: Option<String>,
    pub tags: Vec<Tag>,
    /// The AWS MANAGED POLICY ARNs attached to this permission set.
    ///
    /// An attachment's IDENTITY is its `ManagedPolicyArn` — a permission set holds a given
    /// AWS managed policy at most once, so an attach of an ARN the set already carries is
    /// refused and a detach removes the one entry its ARN names. They live ON the
    /// permission-set record rather than in a second map, for the same reason as
    /// [`Self::customer_managed_policies`]: a permission set that no longer exists holds
    /// none of them, and the attach, detach and list operations all resolve the same
    /// permission set every other read resolves. Holds at most
    /// [`MAX_MANAGED_POLICIES_PER_PERMISSION_SET`] ARNs; an attach that would add one
    /// beyond that ceiling is refused rather than grown into.
    pub managed_policies: Vec<String>,
    /// The CUSTOMER MANAGED POLICY REFERENCES attached to this permission set, by
    /// [`CustomerManagedPolicyReferenceRecord::identity`].
    ///
    /// They live ON the permission-set record rather than in a second map, so a
    /// permission set that no longer exists holds none of them and the attach, detach and
    /// list operations all resolve the same permission set every other read resolves. A
    /// `BTreeMap` so an enumeration walks one stable order and can page by that identity,
    /// and so one identity names at most one stored reference — an attach of a reference
    /// already attached is refused before the insert. Holds at most
    /// [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`] references; an attach that
    /// would add one beyond that ceiling is refused rather than grown into.
    pub customer_managed_policies: BTreeMap<String, CustomerManagedPolicyReferenceRecord>,
    pub inline_policy: Option<String>,
    /// The permissions boundary `PutPermissionsBoundaryToPermissionSet` attached, if one
    /// is attached: a permission set carries AT MOST ONE, so a later put REPLACES it.
    pub permissions_boundary: Option<PermissionsBoundaryRecord>,
}

/// The name of the AWS managed policy `managed_policy_arn` names, as
/// `AttachedManagedPolicy$Name` ("The name of the Amazon Web Services managed policy")
/// reports it.
///
/// DERIVED from the ARN rather than stored beside it: `ManagedPolicyArn`'s pattern
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`)
/// makes the final `/`-separated segment the policy's name and everything before it the
/// path, so the name of an attached policy is a function of the ARN the caller attached
/// and the two can never disagree. An ARN with no `/` at all cannot match that pattern,
/// so the whole string is the only name such a value could have.
pub fn managed_policy_name(managed_policy_arn: &str) -> String {
    match managed_policy_arn.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => managed_policy_arn.to_string(),
    }
}

/// One attached AWS managed policy as the modeled `AttachedManagedPolicy` shape — what
/// `ListManagedPoliciesInPermissionSet` enumerates.
pub fn attached_managed_policy(managed_policy_arn: &str) -> AttachedManagedPolicy {
    AttachedManagedPolicy {
        name: Some(managed_policy_name(managed_policy_arn)),
        arn: Some(managed_policy_arn.to_string()),
    }
}

impl PermissionSetRecord {
    /// The AWS managed policies attached to this permission set, in ARN order, as the
    /// `(page key, item)` pairs [`page`] enumerates.
    ///
    /// The ARN is both the page key and the identity of an attachment — a permission set
    /// holds a given managed policy at most once — so the enumeration order is stable
    /// across calls however the attachments were made, and a pagination cursor resumes
    /// after a KEY rather than after a position a later attach would shift.
    pub fn attached_managed_policies(&self) -> Vec<(String, AttachedManagedPolicy)> {
        let mut arns: Vec<String> = self.managed_policies.clone();
        arns.sort();
        arns.dedup();
        arns.into_iter()
            .map(|arn| {
                let policy = attached_managed_policy(&arn);
                (arn, policy)
            })
            .collect()
    }

    /// A fingerprint of the permission set's DEFINITION as it stands now: everything a
    /// provisioning of it copies into an account.
    ///
    /// Two records whose access this permission set grants is identical share a revision,
    /// and any change to that access — its name, its description, its session duration,
    /// its relay state, an attached AWS managed policy, an attached customer managed
    /// policy reference, its inline policy, its permissions boundary — produces a
    /// different one. `tags` are NOT part of it: they label the permission set itself and
    /// change nothing about the access an account's copy of it grants.
    ///
    /// It is DERIVED from the stored fields on every call rather than stamped on a
    /// change, so it cannot drift from the definition it describes however the record was
    /// reached, and the operations that change a permission set need no bookkeeping of
    /// their own. Each list is preceded by its own section marker and its length, and
    /// [`request_fingerprint`] length-prefixes every part, so two different definitions
    /// cannot render alike.
    pub fn definition_revision(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.name.clone(),
            self.description.clone().unwrap_or_default(),
            self.session_duration.clone().unwrap_or_default(),
            self.relay_state.clone().unwrap_or_default(),
        ];
        let mut managed_policies = self.managed_policies.clone();
        managed_policies.sort();
        managed_policies.dedup();
        parts.push("managedPolicies".to_string());
        parts.push(managed_policies.len().to_string());
        parts.extend(managed_policies);
        parts.push("customerManagedPolicies".to_string());
        parts.push(self.customer_managed_policies.len().to_string());
        parts.extend(self.customer_managed_policies.keys().cloned());
        parts.push("inlinePolicy".to_string());
        parts.push(self.inline_policy.clone().unwrap_or_default());
        match &self.permissions_boundary {
            None => parts.push("noBoundary".to_string()),
            Some(PermissionsBoundaryRecord::ManagedPolicy { managed_policy_arn }) => {
                parts.push("boundary/managed".to_string());
                parts.push(managed_policy_arn.clone());
            }
            Some(PermissionsBoundaryRecord::CustomerManagedPolicy { name, path }) => {
                parts.push("boundary/customer".to_string());
                parts.push(name.clone());
                parts.push(path.clone());
            }
        }
        // The crate's one digest helper, so a revision is built exactly as every other
        // fingerprint of this service is.
        request_fingerprint(&parts)
    }

    /// This record as the modeled `PermissionSet` shape — what both
    /// `CreatePermissionSet` and `DescribePermissionSet` return, so a describe answers
    /// byte-identically to the create that stored it.
    pub fn to_shape(&self) -> PermissionSet {
        PermissionSet {
            name: Some(self.name.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            session_duration: self.session_duration.clone(),
            relay_state: self.relay_state.clone(),
        }
    }
}

/// A trusted token issuer connected to one IAM Identity Center instance, keyed in
/// [`ComAmazonawsSsoadminSwbexternalserviceState::trusted_token_issuers`] by the ARN
/// minted for it at creation.
///
/// The `Tags` a create carries are held here (rather than dropped) so the tagging
/// operations have one record to extend rather than a second map, and so the
/// idempotency fingerprint below covers every parameter of the request that made it.
#[derive(Debug, Clone)]
pub struct TrustedTokenIssuerRecord {
    pub trusted_token_issuer_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub trusted_token_issuer_type: String,
    pub configuration: TrustedTokenIssuerConfiguration,
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this issuer, when it carried one.
    ///
    /// Held ON the record, so the idempotency promise lives exactly as long as the
    /// resource it names: a deleted issuer frees its token instead of leaving a record
    /// that would answer a replay with an ARN nothing resolves.
    pub client_token: Option<String>,
    /// [`trusted_token_issuer_fingerprint`] of the request that minted this issuer, so
    /// a retry carrying the SAME token can be told from the same token carrying a
    /// DIFFERENT request.
    pub request_fingerprint: String,
}

/// An assignment of a permission set to a principal on one account.
///
/// Its identity is the tuple (instance, account, permission set, principal type,
/// principal id); [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_position`]
/// is the single place that tuple is compared.
#[derive(Debug, Clone)]
pub struct AccountAssignmentRecord {
    pub instance_arn: String,
    pub account_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
    /// The `RequestId` of the operation that granted this assignment, so a repeat of
    /// that create answers from the SAME stored status record instead of writing a
    /// second one for work that was already done.
    pub request_id: String,
    /// The permission set's definition revision ([`PermissionSetRecord::definition_revision`])
    /// as it stood when this assignment PROVISIONED the permission set into the account.
    ///
    /// Granting an account assignment provisions the permission set into the target
    /// account, so the account then holds the definition that was current at that
    /// instant. Keeping that revision beside the row is what lets a later read tell an
    /// account holding the permission set's LATEST definition from one whose copy has
    /// since been left behind by a change to the permission set, without having to
    /// re-derive what the account was given.
    pub provisioned_revision: String,
}

impl AccountAssignmentRecord {
    /// The record's sort/pagination key within one (account, permission set) filter:
    /// the part of its identity the filter does not already fix.
    pub fn page_key(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }

    /// This record as the modeled `AccountAssignment` shape.
    pub fn to_shape(&self) -> AccountAssignment {
        AccountAssignment {
            account_id: Some(self.account_id.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
        }
    }
}

/// A customer managed application `CreateApplication` created under one instance.
///
/// Every member `DescribeApplication` reports is held here, so a read answers from the
/// record the create wrote rather than from anything recomputed on the way out.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    /// The server-minted `ApplicationArn` ([`mint_application_arn`]) the application is
    /// stored under.
    pub application_arn: String,
    /// The instance the application belongs to: an application created under an instance
    /// stops existing with it (see
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::remove_instance`]).
    pub instance_arn: String,
    /// `ApplicationProviderArn` — the provider the application runs under, one of
    /// [`BUILT_IN_APPLICATION_PROVIDERS`].
    pub application_provider_arn: String,
    /// `DescribeApplication$ApplicationAccount` — "The account ID" the application lives
    /// in, the account segment of its own ARN.
    pub application_account: String,
    /// The identity store of the owning instance, reported as `IdentityStoreArn`.
    pub identity_store_arn: String,
    /// `Name`, as the caller sent it.
    pub name: String,
    /// `Description`, when the caller sent one.
    pub description: Option<String>,
    /// `ApplicationStatus`: `ENABLED` or `DISABLED`.
    pub status: String,
    /// `AssignmentRequired` — whether a principal needs a direct assignment to this
    /// application to reach it, as the last `PutApplicationAssignmentConfiguration` set
    /// it and as `GetApplicationAssignmentConfiguration` reports it.
    ///
    /// It lives ON the application record rather than in a second map, so an application
    /// that no longer exists holds no configuration and the configuration operations
    /// resolve the same application every other read resolves. An application nothing has
    /// configured carries [`APPLICATION_ASSIGNMENT_REQUIRED_DEFAULT`].
    pub assignment_required: bool,
    /// `PortalOptions`, normalised so the stored visibility is always explicit (see
    /// [`APPLICATION_VISIBILITY_ENABLED`]).
    pub portal_options: PortalOptions,
    /// `CreatedDate` — when this create was accepted.
    pub created_date: DateTime,
    /// `CreatedFrom` — the Region of the request that created it ([`request_region`]).
    pub created_from: String,
    /// `Tags`, as the caller sent them.
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this application, if it carried one.
    pub client_token: Option<String>,
    /// [`application_fingerprint`] of that create, so a retry carrying the SAME token
    /// can be told from the same token carrying a DIFFERENT request.
    pub request_fingerprint: String,
    /// The ACCESS SCOPES of this application, by `Scope` name, as
    /// `PutApplicationAccessScope` stored them.
    ///
    /// They live ON the application record rather than in a second map, so an
    /// application that no longer exists holds no scopes and the scope operations
    /// resolve the same application every other read resolves. The value is the scope's
    /// `AuthorizedTargets`: `None` for a scope stored with no authorized targets, which
    /// is a different statement from an empty list the model's `ScopeTargets`
    /// (`length 1..10`) cannot even express. A `BTreeMap` so an enumeration walks one
    /// stable order and can page by scope name. Holds at most
    /// [`MAX_SCOPES_PER_APPLICATION`] scopes; a put that would add one beyond that
    /// ceiling is refused rather than grown into.
    pub access_scopes: BTreeMap<String, Option<Vec<String>>>,
    /// The AUTHENTICATION METHODS of this application, by `AuthenticationMethodType`, as
    /// `PutApplicationAuthenticationMethod` stored them.
    ///
    /// On the application record for the same reason as [`Self::access_scopes`]: an
    /// application that no longer exists carries no authentication method, and the
    /// method operations resolve the same application every other read resolves. Keyed
    /// by the method's TYPE, which is what identifies it — a put of a type the
    /// application already carries replaces that method and leaves any other type alone.
    /// The key needs no memory ceiling beside [`MAX_SCOPES_PER_APPLICATION`]: the
    /// model's `AuthenticationMethodType` enum admits exactly one value, which the
    /// boundary validator enforces, so this map holds at most one entry per application.
    /// A `BTreeMap` so an enumeration walks one stable order and can page by type.
    pub authentication_methods: BTreeMap<String, AuthenticationMethod>,
    /// The OAuth 2.0 GRANTS of this application, by `GrantType`, as
    /// `PutApplicationGrant` stored them.
    ///
    /// On the application record for the same reason as [`Self::access_scopes`]: an
    /// application that no longer exists carries no grant, and the grant operations
    /// resolve the same application every other read resolves. Keyed by the grant's TYPE,
    /// which is what identifies it — a put of a type the application already carries
    /// replaces that grant and leaves every other type alone. The key needs no memory
    /// ceiling: the model's `GrantType` enum admits exactly four values
    /// (`types::GRANT_TYPE_VALUES`), which the boundary validator enforces, so this map
    /// holds at most four entries per application. A `BTreeMap` so an enumeration walks
    /// one stable order and can page by grant type.
    pub grants: BTreeMap<String, Grant>,
}

impl ApplicationRecord {
    /// The access scopes this application carries, in `Scope` order, as the
    /// `(page key, item)` pairs [`page`] enumerates.
    ///
    /// The scope NAME is both the page key and the identity of a scope — an application
    /// carries a given scope at most once, since [`Self::access_scopes`] is keyed by it —
    /// so the enumeration order is stable across calls however the scopes were put, and a
    /// pagination cursor resumes after a KEY rather than after a position a later put
    /// would shift.
    pub fn access_scope_details(&self) -> Vec<(String, ScopeDetails)> {
        self.access_scopes
            .iter()
            .map(|(scope, authorized_targets)| {
                (
                    scope.clone(),
                    ScopeDetails {
                        scope: scope.clone(),
                        // Reported from the STORED scope: absent for a scope put with no
                        // `AuthorizedTargets`, since `ScopeTargets` is `length 1..10` and
                        // cannot express an empty list.
                        authorized_targets: authorized_targets.clone(),
                    },
                )
            })
            .collect()
    }
}

/// A direct assignment of one principal to one application.
///
/// Its identity is the tuple (application, principal type, principal id) —
/// [`ComAmazonawsSsoadminSwbexternalserviceState::application_assignment_position`] is
/// the single place that tuple is compared — so granting the same assignment twice
/// grants it once.
#[derive(Debug, Clone)]
pub struct ApplicationAssignmentRecord {
    pub application_arn: String,
    pub principal_type: String,
    pub principal_id: String,
}

/// The status record of one create/delete assignment operation, keyed by its
/// `RequestId` so `DescribeAccountAssignment*Status` and
/// `ListAccountAssignment*Status` read real history rather than a synthesised answer.
///
/// `Status` is DERIVED from [`succeeded_due`](Self::succeeded_due) rather than stored, so
/// a read never has to write to move the status on: the instant the operation completes
/// is fixed when the call that started it is accepted, and every later read computes the
/// status the clock implies at the moment it is asked.
#[derive(Debug, Clone)]
pub struct AssignmentOperationRecord {
    pub request_id: String,
    /// The instance the operation ran under. A status record belongs to one instance,
    /// exactly as the assignment it reports on does, so a `RequestId` minted under
    /// another instance is as absent as one that was never minted.
    pub instance_arn: String,
    pub failure_reason: Option<String>,
    pub target_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
    /// When this operation stops being `IN_PROGRESS` and becomes `SUCCEEDED`:
    /// [`ASSIGNMENT_OPERATION_SPAN`] after the call that started it was accepted.
    pub succeeded_due: SystemTime,
}

impl AssignmentOperationRecord {
    /// `StatusValues` as of `now`: the non-terminal `IN_PROGRESS` until the operation's
    /// completion is due, `SUCCEEDED` after.
    ///
    /// The enum's third value, `FAILED`, is never reported: nothing here fails an
    /// assignment operation, which is also why [`Self::failure_reason`] — "the message
    /// that contains an error or exception in case of an operation failure" — stays
    /// absent.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.succeeded_due {
            STATUS_IN_PROGRESS
        } else {
            STATUS_SUCCEEDED
        }
    }

    /// This record as the modeled `AccountAssignmentOperationStatus` shape, as of `now`:
    /// the whole status object, tracking members and the assignment the operation is
    /// about — what a status DESCRIBE reports.
    pub fn to_shape(&self, now: SystemTime) -> AccountAssignmentOperationStatus {
        AccountAssignmentOperationStatus {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            failure_reason: self.failure_reason.clone(),
            target_id: Some(self.target_id.clone()),
            target_type: Some(self.target_type.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
            created_date: Some(self.created_date),
        }
    }

    /// The TRACKING view of this record, as of `now`: the status the operation is in, the
    /// `RequestId` — "the identifier for tracking the request operation" — to poll it by,
    /// and the date it was accepted, and none of the assignment detail.
    ///
    /// Those three members are exactly the ones the model's own
    /// `AccountAssignmentOperationStatusMetadata` carries, which is what the API reports
    /// for an operation a caller has not described yet; it is what the call that STARTS
    /// an operation answers with, since a caller holding the whole request already knows
    /// the assignment it asked for and what it does not yet know is the handle to poll.
    pub fn to_tracking_shape(&self, now: SystemTime) -> AccountAssignmentOperationStatus {
        AccountAssignmentOperationStatus {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            created_date: Some(self.created_date),
            failure_reason: None,
            target_id: None,
            target_type: None,
            permission_set_arn: None,
            principal_type: None,
            principal_id: None,
        }
    }
}

/// Per-service state for com-amazonaws-ssoadmin-swbexternalservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsoadminSwbexternalserviceState {
    /// Instances by `InstanceArn`. The account's AMBIENT instance is enabled into this
    /// map by the first request that reaches the store (see [`Self::on_request`]); an ARN
    /// absent from it is the `ResourceNotFoundException`/`AccessDeniedException` every
    /// instance-taking operation answers with.
    pub instances: BTreeMap<String, Instance>,
    /// Permission sets by `PermissionSetArn`. A `BTreeMap` so `ListPermissionSets`
    /// enumerates in one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records; a create that would cross that ceiling is
    /// refused rather than grown into.
    pub permission_sets: BTreeMap<String, PermissionSetRecord>,
    /// Account assignments. At most one row per identity tuple, and at most
    /// [`MAX_RECORDS_PER_STORE`] rows in all.
    pub account_assignments: Vec<AccountAssignmentRecord>,
    /// Trusted token issuers by `TrustedTokenIssuerArn`. A `BTreeMap` so an
    /// enumeration walks one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records.
    pub trusted_token_issuers: BTreeMap<String, TrustedTokenIssuerRecord>,
    /// Applications by `ApplicationArn`, each carrying its own access scopes. A
    /// `BTreeMap` so an enumeration walks one stable order and can page by key. Holds at
    /// most [`MAX_RECORDS_PER_STORE`] records.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// Direct application assignments. At most one row per (application, principal type,
    /// principal id) tuple, and at most [`MAX_RECORDS_PER_STORE`] rows in all.
    pub application_assignments: Vec<ApplicationAssignmentRecord>,
    /// Assignment operation statuses by `RequestId`: one per assignment currently
    /// granted, so a repeated create reads the record it already wrote instead of
    /// adding another, and a delete that removes an assignment removes its status
    /// record with it. Holds at most [`MAX_RECORDS_PER_STORE`] records.
    pub assignment_operations: BTreeMap<String, AssignmentOperationRecord>,
    /// Assignment DELETION statuses, newest last — the history
    /// `DescribeAccountAssignmentDeletionStatus` reads by `RequestId`, in the same record
    /// type a creation status uses ([`AssignmentOperationRecord`], which carries the
    /// instance it ran under), so the two views cannot disagree about the members of
    /// `AccountAssignmentOperationStatus`.
    ///
    /// Separate from [`Self::assignment_operations`] because the two are read by
    /// different operations: a `RequestId` a deletion carries is not a creation status and
    /// must not answer a describe of one. A deletion status is also history rather than a
    /// resource — by the time it is stored the assignment it reports on is gone, so it
    /// cannot be held beside that assignment. A `Vec` in insertion order, so the history
    /// is chronological; it holds at most [`MAX_RECORDS_PER_STORE`] records and the oldest
    /// is evicted beyond that (see [`Self::record_assignment_deletion`]) rather than a
    /// delete being refused for history nobody can grow.
    pub assignment_deletions: Vec<AssignmentOperationRecord>,
    /// Whether the account's IAM Identity Center ENABLEMENT has been decided already.
    ///
    /// The emulator's account gets its instance from the first request that needs one: a
    /// `CreateInstance` enables the instance the caller asked for
    /// ([`Self::on_instance_create`]), and any other request finds the account's AMBIENT
    /// instance ([`Self::on_request`]). This flag is what makes that a ONE-TIME decision:
    /// an instance a caller deletes stays deleted instead of being replaced by an ambient
    /// one on the next read, and an instance a caller created is the account's one
    /// instance rather than a second beside an invented one.
    pub ambient_instance_enabled: bool,
}

impl ComAmazonawsSsoadminSwbexternalserviceState {
    /// What every handler that RESOLVES an instance does BEFORE it looks at this store:
    /// enable the account's ambient instance if nothing has decided the account's
    /// enablement yet, then apply every transition the clock already owes
    /// ([`Self::materialize_due`]).
    ///
    /// `region` is the Region of the request ([`request_region`]) — the Region the
    /// ambient instance is enabled in, since the request is the only Region signal the
    /// emulator has. Nothing else about the request reaches the record, and a second
    /// request from another Region finds the instance already enabled and leaves its
    /// Region alone, so this fixes that Region ONCE and every later read of the instance
    /// answers the same.
    pub fn on_request(&mut self, region: &str, now: SystemTime) {
        self.enable_ambient_instance(region, now);
        self.materialize_due(now);
    }

    /// What `CreateInstance` does instead, before it looks at this store.
    ///
    /// It enables NO ambient instance: that caller is enabling the account's instance
    /// ITSELF, and inventing one for it first would refuse — on the account's
    /// one-instance quota — the very call that establishes the account's enablement. It
    /// still applies what the clock already owes, so the quota is judged against the
    /// instances that exist at this instant. Once the create has stored its instance,
    /// [`Self::instance_enablement_decided`] records that the account's enablement is
    /// settled, so no later read enables an ambient instance beside it.
    pub fn on_instance_create(&mut self, now: SystemTime) {
        self.materialize_due(now);
    }

    /// Record that the account's IAM Identity Center enablement is settled — a caller's
    /// `CreateInstance` enabled it — so no later request enables an ambient instance.
    pub fn instance_enablement_decided(&mut self) {
        self.ambient_instance_enabled = true;
    }

    /// Enable the account's ambient instance: the instance the emulator's account already
    /// holds, the stand-in for an account whose IAM Identity Center was enabled before
    /// this session started.
    ///
    /// It is an ACCOUNT instance ([`SEED_INSTANCE_TYPE`]), so the permission-set surface
    /// is withdrawn on it until an `UpdateInstance` enables permission sets. It is
    /// enabled at most once per store, and not at all when the store already holds an
    /// instance (one instance per account), so no request ever adds a second instance or
    /// revives a deleted one.
    fn enable_ambient_instance(&mut self, region: &str, now: SystemTime) {
        let already_decided = self.ambient_instance_enabled || !self.instances.is_empty();
        self.ambient_instance_enabled = true;
        if already_decided {
            return;
        }
        let instance = Instance::active(
            SEED_INSTANCE_ID,
            SEED_IDENTITY_STORE_ID,
            SEED_INSTANCE_TYPE,
            Some(SEED_INSTANCE_NAME.to_string()),
            DateTime::from(now),
            region,
        );
        self.instances
            .insert(instance.instance_arn.clone(), instance);
    }
}

impl ComAmazonawsSsoadminSwbexternalserviceState {
    /// The instance `instance_arn` names, or `None` when no such instance exists.
    pub fn instance(&self, instance_arn: &str) -> Option<&Instance> {
        self.instances.get(instance_arn)
    }

    /// The instance `instance_arn` names, for a handler that updates it.
    pub fn instance_mut(&mut self, instance_arn: &str) -> Option<&mut Instance> {
        self.instances.get_mut(instance_arn)
    }

    /// Apply every transition the CLOCK already owes: see [`Instance::materialize_due`],
    /// plus the REMOVAL a `DeleteInstance` owes — an instance whose deletion instant has
    /// passed no longer exists, whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let removed: Vec<String> = self
            .instances
            .values()
            .filter(|instance| instance.removal_due(now))
            .map(|instance| instance.instance_arn.clone())
            .collect();
        for instance_arn in &removed {
            self.remove_instance(instance_arn);
        }
        for instance in self.instances.values_mut() {
            instance.materialize_due(now);
        }
    }

    /// Drop the instance `instance_arn` names and everything held INSIDE it.
    ///
    /// A permission set, an account assignment, a trusted token issuer and an
    /// application each belong to one instance — every one of them is created under its
    /// `InstanceArn` — so an instance that no longer exists holds none of them: leaving
    /// them behind would let a later create of an instance inherit resources nobody
    /// created under it. The status record of an assignment operation goes with the
    /// permission set it names, and an application assignment with the application it
    /// grants access to, for the same reason.
    fn remove_instance(&mut self, instance_arn: &str) {
        self.instances.remove(instance_arn);
        self.permission_sets
            .retain(|_, record| record.instance_arn != instance_arn);
        self.account_assignments
            .retain(|record| record.instance_arn != instance_arn);
        self.trusted_token_issuers
            .retain(|_, record| record.instance_arn != instance_arn);
        self.assignment_operations
            .retain(|_, record| record.instance_arn != instance_arn);
        self.assignment_deletions
            .retain(|record| record.instance_arn != instance_arn);
        let orphaned_applications: Vec<String> = self
            .applications
            .values()
            .filter(|record| record.instance_arn == instance_arn)
            .map(|record| record.application_arn.clone())
            .collect();
        self.applications
            .retain(|_, record| record.instance_arn != instance_arn);
        self.application_assignments
            .retain(|record| !orphaned_applications.contains(&record.application_arn));
    }

    /// Whether the permission-set / account-assignment surface is WITHDRAWN on the
    /// instance `instance_arn` names, i.e. it is an account instance.
    ///
    /// `false` for an ARN this account holds no instance for: that request is a missing
    /// resource, which each operation answers with its own
    /// `ResourceNotFoundException`/`AccessDeniedException` — the restriction belongs to a
    /// RESOLVED instance, not to an unknown ARN. Every operation of the surface asks
    /// this BEFORE looking a permission set or an assignment up, because the refusal
    /// does not depend on whether that resource exists: on an account instance none ever
    /// can.
    pub fn permission_set_surface_withdrawn(&self, instance_arn: &str) -> bool {
        self.instance(instance_arn)
            .is_some_and(|instance| !instance.permission_sets_enabled)
    }

    /// Whether the attribute-based access control (ABAC) surface is WITHDRAWN on the
    /// instance `instance_arn` names, i.e. it is an account instance.
    ///
    /// ABAC maps identity-store attributes into the session tags that the policies of
    /// multi-account AWS access are evaluated against (`AccessControlAttribute`: "You can
    /// create permissions policies that determine who can access your Amazon Web Services
    /// resources based upon the configured attribute values"), so it is a feature of the
    /// SAME organization-instance surface as permission sets and the account assignments
    /// built from them — which is why this asks exactly
    /// [`Self::permission_set_surface_withdrawn`] rather than a second flag. As there, an
    /// ARN this account holds no instance for is NOT this refusal: it is a missing
    /// resource.
    pub fn abac_surface_withdrawn(&self, instance_arn: &str) -> bool {
        self.permission_set_surface_withdrawn(instance_arn)
    }

    /// The instance a previous `CreateInstance` minted under `client_token`, if that
    /// token was ever used. A retry of the same call answers from it instead of being
    /// refused as a duplicate, which is what `@idempotencyToken` asks of the member.
    pub fn instance_by_client_token(&self, client_token: &str) -> Option<&Instance> {
        self.instances.values().find(|instance| {
            instance
                .client_token
                .as_deref()
                .is_some_and(|stored| stored == client_token)
        })
    }

    /// The permission set `permission_set_arn` names WITHIN `instance_arn`.
    ///
    /// A permission set belongs to exactly one instance, so an ARN that exists under a
    /// different instance is as absent here as one that was never created.
    pub fn permission_set(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Option<&PermissionSetRecord> {
        self.permission_sets
            .get(permission_set_arn)
            .filter(|record| record.instance_arn == instance_arn)
    }

    /// The same permission set, for an operation that CHANGES it (attaching a policy or
    /// a permissions boundary, say).
    ///
    /// Resolves exactly as [`Self::permission_set`] — a record held under another
    /// instance is as absent here as one that was never created — so a write and a read
    /// of the same ARN never disagree about which permission set it names.
    pub fn permission_set_mut(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Option<&mut PermissionSetRecord> {
        self.permission_sets
            .get_mut(permission_set_arn)
            .filter(|record| record.instance_arn == instance_arn)
    }

    /// Whether `name` is already taken by a permission set of `instance_arn`.
    pub fn permission_set_name_taken(&self, instance_arn: &str, name: &str) -> bool {
        self.permission_sets
            .values()
            .any(|record| record.instance_arn == instance_arn && record.name == name)
    }

    /// The trusted token issuer `trusted_token_issuer_arn` names, or `None` when no
    /// such issuer exists.
    pub fn trusted_token_issuer(
        &self,
        trusted_token_issuer_arn: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers.get(trusted_token_issuer_arn)
    }

    /// The trusted token issuer a previous `CreateTrustedTokenIssuer` minted under
    /// `client_token`, if one still exists.
    pub fn trusted_token_issuer_for_client_token(
        &self,
        client_token: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The application `application_arn` names, or `None` when no such application
    /// exists — never created, or created under an instance that has since gone.
    pub fn application(&self, application_arn: &str) -> Option<&ApplicationRecord> {
        self.applications.get(application_arn)
    }

    /// The application `application_arn` names, for a handler that updates it (a
    /// `PutApplicationAccessScope` writing one of its access scopes, or a
    /// `DeleteApplicationGrant` removing one of its grants, say).
    pub fn application_mut(&mut self, application_arn: &str) -> Option<&mut ApplicationRecord> {
        self.applications.get_mut(application_arn)
    }

    /// The application a previous `CreateApplication` minted under `client_token`, if
    /// one still exists.
    pub fn application_for_client_token(&self, client_token: &str) -> Option<&ApplicationRecord> {
        self.applications
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The index of the application assignment with this identity tuple, if it exists.
    pub fn application_assignment_position(
        &self,
        application_arn: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> Option<usize> {
        self.application_assignments.iter().position(|row| {
            row.application_arn == application_arn
                && row.principal_type == principal_type
                && row.principal_id == principal_id
        })
    }

    /// The TAGGABLE resource `resource_arn` names: the ARN of the instance it belongs to
    /// (itself, for an instance) and the tag set it carries.
    ///
    /// The model's `TaggableResourceArn` admits four kinds of resource — an instance, a
    /// permission set, an application and a trusted token issuer — and each is stored
    /// under its own ARN, so the ARN alone says which record carries the tags. `None` for
    /// an ARN this account holds no resource under, including one whose instance has
    /// since gone (an instance holds its resources, see [`Self::remove_instance`]).
    fn taggable_resource(&self, resource_arn: &str) -> Option<(&str, &Vec<Tag>)> {
        if let Some(instance) = self.instances.get(resource_arn) {
            return Some((instance.instance_arn.as_str(), &instance.tags));
        }
        if let Some(record) = self.permission_sets.get(resource_arn) {
            return Some((record.instance_arn.as_str(), &record.tags));
        }
        if let Some(record) = self.applications.get(resource_arn) {
            return Some((record.instance_arn.as_str(), &record.tags));
        }
        self.trusted_token_issuers
            .get(resource_arn)
            .map(|record| (record.instance_arn.as_str(), &record.tags))
    }

    /// The tags the taggable resource `resource_arn` names carries, for an operation that
    /// REPORTS them.
    ///
    /// `instance_arn` is the request's optional `InstanceArn`: when a caller sends one, a
    /// resource held under a DIFFERENT instance is as absent here as one that was never
    /// created — the same rule [`Self::permission_set`] resolves a permission set by — so
    /// the read and the write below never disagree about which resource an ARN names.
    pub fn taggable_resource_tags(
        &self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Option<&Vec<Tag>> {
        let (owner, tags) = self.taggable_resource(resource_arn)?;
        instance_arn.is_none_or(|arn| arn == owner).then_some(tags)
    }

    /// The same tag set, for an operation that CHANGES it. Resolves exactly as
    /// [`Self::taggable_resource_tags`].
    pub fn taggable_resource_tags_mut(
        &mut self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Option<&mut Vec<Tag>> {
        let owner = self.taggable_resource(resource_arn)?.0.to_string();
        if instance_arn.is_some_and(|arn| arn != owner) {
            return None;
        }
        if let Some(instance) = self.instances.get_mut(resource_arn) {
            return Some(&mut instance.tags);
        }
        if let Some(record) = self.permission_sets.get_mut(resource_arn) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.applications.get_mut(resource_arn) {
            return Some(&mut record.tags);
        }
        self.trusted_token_issuers
            .get_mut(resource_arn)
            .map(|record| &mut record.tags)
    }

    /// Store the status of one assignment deletion, evicting the OLDEST records while
    /// the history is over [`MAX_RECORDS_PER_STORE`].
    ///
    /// A deletion is applied before this is called, so the history cannot refuse it: the
    /// emulator's memory ceiling is kept by dropping the oldest statuses instead, which
    /// is why this store is the one caller-driven store that evicts rather than refusing
    /// (a creation status belongs to a live assignment, so it is bounded by the
    /// assignments themselves).
    pub fn record_assignment_deletion(&mut self, record: AssignmentOperationRecord) {
        self.assignment_deletions.push(record);
        let excess = self
            .assignment_deletions
            .len()
            .saturating_sub(MAX_RECORDS_PER_STORE);
        self.assignment_deletions.drain(..excess);
    }

    /// The stored status of the assignment deletion `request_id` names WITHIN
    /// `instance_arn`.
    ///
    /// Scoped to the instance exactly as [`Self::assignment_operation`] is: a `RequestId`
    /// of another instance's deletion is as absent here as one that never ran.
    pub fn assignment_deletion(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&AssignmentOperationRecord> {
        self.assignment_deletions
            .iter()
            .find(|record| record.instance_arn == instance_arn && record.request_id == request_id)
    }

    /// Whether any stored assignment operation — a creation status or a deletion status —
    /// already carries `request_id`, so a freshly minted one is never reused for a second
    /// operation.
    pub fn assignment_request_id_taken(&self, request_id: &str) -> bool {
        self.assignment_operations.contains_key(request_id)
            || self
                .assignment_deletions
                .iter()
                .any(|record| record.request_id == request_id)
    }

    /// The index of the assignment with this identity tuple, if it exists.
    pub fn assignment_position(
        &self,
        instance_arn: &str,
        account_id: &str,
        permission_set_arn: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> Option<usize> {
        self.account_assignments.iter().position(|row| {
            row.instance_arn == instance_arn
                && row.account_id == account_id
                && row.permission_set_arn == permission_set_arn
                && row.principal_type == principal_type
                && row.principal_id == principal_id
        })
    }

    /// Every account the permission set `permission_set_arn` names is PROVISIONED into,
    /// with the provisioning status each of them is in, keyed by account id.
    ///
    /// An account holds a permission set once it has been provisioned there — which is
    /// what granting an account assignment on that account does — however many principals
    /// were assigned through it, so an account appears at most once. The copy in place is
    /// the one the LAST provisioning put there, so a provisioning that followed a change
    /// to the permission set brings the account back up to date.
    ///
    /// The status is COMPUTED, by comparing the revision each provisioning recorded
    /// ([`AccountAssignmentRecord::provisioned_revision`]) against the permission set's
    /// revision now: equal means the account holds the latest definition
    /// ([`PROVISIONING_STATUS_LATEST_PROVISIONED`]), and anything else means the
    /// permission set has changed since
    /// ([`PROVISIONING_STATUS_LATEST_NOT_PROVISIONED`]). A permission set no instance
    /// holds under `instance_arn` is provisioned nowhere, so the answer is empty.
    pub fn provisioned_accounts(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> BTreeMap<String, &'static str> {
        let Some(permission_set) = self.permission_set(instance_arn, permission_set_arn) else {
            return BTreeMap::new();
        };
        let revision = permission_set.definition_revision();
        let mut provisioned: BTreeMap<String, &'static str> = BTreeMap::new();
        for row in &self.account_assignments {
            if row.instance_arn != instance_arn || row.permission_set_arn != permission_set_arn {
                continue;
            }
            let status = provisioning_status_of(&row.provisioned_revision, &revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.account_id.clone(), status);
        }
        provisioned
    }

    /// Every permission set PROVISIONED into the account `account_id` names under
    /// `instance_arn`, with the provisioning status each of them is in, keyed by the
    /// permission set's ARN.
    ///
    /// The MIRROR of [`Self::provisioned_accounts`], over the same rows and with the same
    /// rule: an account holds a permission set once that permission set has been
    /// provisioned there — which is what granting an account assignment on the account
    /// does — however many principals were assigned through it, so a permission set
    /// appears at most once. The copy in place is the one the LAST provisioning put there.
    ///
    /// The status is COMPUTED the same way, against the revision of EACH permission set as
    /// it stands now: equal to the revision the provisioning recorded means the account
    /// holds that permission set's latest definition
    /// ([`PROVISIONING_STATUS_LATEST_PROVISIONED`]), and anything else means the permission
    /// set has changed since ([`PROVISIONING_STATUS_LATEST_NOT_PROVISIONED`]). A row whose
    /// permission set this instance no longer holds is provisioned nowhere, so it is left
    /// out.
    pub fn provisioned_permission_sets(
        &self,
        instance_arn: &str,
        account_id: &str,
    ) -> BTreeMap<String, &'static str> {
        let mut provisioned: BTreeMap<String, &'static str> = BTreeMap::new();
        // Each permission set's current revision, derived at most once however many rows
        // provisioned it into the account.
        let mut revisions: BTreeMap<&str, String> = BTreeMap::new();
        for row in &self.account_assignments {
            if row.instance_arn != instance_arn || row.account_id != account_id {
                continue;
            }
            if !revisions.contains_key(row.permission_set_arn.as_str()) {
                // A row whose permission set this instance no longer holds is provisioned
                // nowhere, so it is left out.
                let Some(permission_set) =
                    self.permission_set(instance_arn, &row.permission_set_arn)
                else {
                    continue;
                };
                revisions.insert(
                    row.permission_set_arn.as_str(),
                    permission_set.definition_revision(),
                );
            }
            let Some(revision) = revisions.get(row.permission_set_arn.as_str()) else {
                continue;
            };
            let status = provisioning_status_of(&row.provisioned_revision, revision);
            // Later rows were provisioned later, so the copy in place is the last one's.
            provisioned.insert(row.permission_set_arn.clone(), status);
        }
        provisioned
    }

    /// The status record of the assignment operation `request_id` names, WITHIN
    /// `instance_arn`.
    ///
    /// An assignment operation is performed under one instance, and the record carries
    /// it, so a `RequestId` minted under another instance is as absent here as one
    /// nothing was ever minted for — which is what keeps a `Describe…Status` read of one
    /// instance from reporting another instance's history. A record whose permission set
    /// has gone is absent too, because the delete removes it with the assignments it
    /// reported on ([`Self::remove_instance`], `DeletePermissionSet`).
    pub fn assignment_operation(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&AssignmentOperationRecord> {
        self.assignment_operations
            .get(request_id)
            .filter(|record| record.instance_arn == instance_arn)
    }
}

/// One of the application providers IAM Identity Center publishes ITSELF.
///
/// An application provider is not a caller-created resource: the model's
/// `ApplicationProviderArn` pattern pins the owner segment to `aws`
/// (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`), so every
/// provider belongs to AWS and no operation of this service creates or deletes one.
/// They are therefore a FIXED catalog ([`BUILT_IN_APPLICATION_PROVIDERS`]) rather than a
/// store: `DescribeApplicationProvider` reports an entry of it and refuses every other
/// ARN, and `ListApplicationProviders` enumerates the same entries, through
/// [`BuiltInApplicationProvider::to_shape`], so the two agree member for member.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInApplicationProvider {
    /// Everything after `applicationProvider/` in the provider's ARN: the service slug of
    /// an AWS managed provider (`glue`), or `catalog/<Name>` for one of the pre-integrated
    /// applications of the IAM Identity Center application catalog. The model's
    /// `ApplicationProviderArn` pattern admits `/` inside the id
    /// (`applicationProvider/[a-zA-Z0-9-/]+`), which is what makes the `catalog/` form an
    /// id and not a second path segment.
    pub id: &'static str,
    /// `ApplicationProvider$FederationProtocol` — "The protocol that the application
    /// provider uses to perform federation", one of the model's `FederationProtocol`
    /// enum values `SAML | OAUTH`.
    pub federation_protocol: &'static str,
    /// Whether the applications under this provider are managed by the CUSTOMER — set up
    /// through this API — rather than by Amazon Web Services on the customer's behalf.
    ///
    /// True for the custom application provider alone: an application of an AWS managed
    /// provider is created by the Amazon Web Services service that owns it, and a
    /// pre-integrated catalog application is set up in the console. This is what
    /// [`BuiltInApplicationProvider::accepts_customer_managed_application`] reads, and it
    /// is NOT reported on the wire — no member of the modelled `ApplicationProvider`
    /// carries it.
    pub customer_managed: bool,
    /// `DisplayData$DisplayName` — "The name of the application provider that appears in
    /// the portal" (`Name`, 1..100).
    pub display_name: &'static str,
    /// `DisplayData$Description` — "The description of the application provider that
    /// appears in the portal" (1..128) — when the provider carries wording of its own.
    /// `None` for a catalog application, whose portal description is DERIVED from its
    /// display name (see [`BuiltInApplicationProvider::description`]).
    pub description: Option<&'static str>,
    /// The access scopes `ApplicationProvider$ResourceServerConfig` reports for this
    /// provider — empty for a provider that exposes no resource server, which is then
    /// reported with no `ResourceServerConfig` at all rather than an empty one.
    pub scopes: &'static [BuiltInAccessScope],
}

/// One IAM Identity Center access scope of a provider's resource server, as
/// `ResourceServerConfig$Scopes` reports it: the scope NAME is the map key and the two
/// `ResourceServerScopeDetails` members (both 1..128) its value.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInAccessScope {
    /// The map key: `<service>:<scope>`, the form IAM Identity Center publishes an access
    /// scope under (the recorded catalog carries `glue:read_write` on the Glue provider).
    pub name: &'static str,
    /// `ResourceServerScopeDetails$DetailedTitle` — "The title of an access scope for a
    /// resource server".
    pub detailed_title: &'static str,
    /// `ResourceServerScopeDetails$LongDescription` — "The description of an access scope
    /// for a resource server".
    pub long_description: &'static str,
}

impl BuiltInAccessScope {
    /// This scope as the modeled `ResourceServerScopeDetails` shape.
    pub fn to_shape(&self) -> ResourceServerScopeDetails {
        ResourceServerScopeDetails {
            detailed_title: Some(self.detailed_title.to_string()),
            long_description: Some(self.long_description.to_string()),
        }
    }
}

/// The host every provider icon of the IAM Identity Center portal is served from.
///
/// SOURCE: recorded behavior — a catalog provider's `DisplayData$IconUrl` is an `https`
/// URL under this static asset host (`https://<host>/app-<digest>/icons/default.png`).
/// The digest in a recorded URL is environment detail, so it is DERIVED from the provider
/// id here (see [`BuiltInApplicationProvider::icon_url`]) rather than copied.
const PROVIDER_ICON_HOST: &str = "static.global.sso.amazonaws.com";

/// The portal description of a catalog application: its display name followed by this
/// word.
///
/// SOURCE: recorded behavior — the catalog entry whose `DisplayName` is `Statuspage`
/// carries `Description` `Statuspage Application`, i.e. a pre-integrated application's
/// portal description is its name as an application. An AWS managed provider carries
/// wording of its own instead ([`BuiltInApplicationProvider::description`]).
const CATALOG_DESCRIPTION_SUFFIX: &str = "Application";

/// `FederationProtocol::SAML`, the protocol of a pre-integrated catalog application: the
/// IAM Identity Center application catalog is a catalog of SAML 2.0 applications.
const FEDERATION_SAML: &str = "SAML";

/// `FederationProtocol::OAUTH`, the protocol of an AWS managed application provider that
/// receives a propagated identity (trusted identity propagation).
const FEDERATION_OAUTH: &str = "OAUTH";

impl BuiltInApplicationProvider {
    /// This provider's `DisplayData`, complete: every member the shape declares is
    /// reported, because every provider of the catalog HAS all three — the display name
    /// and the description from the record, the icon URL derived from the provider id.
    pub fn display_data(&self) -> DisplayData {
        DisplayData {
            display_name: Some(self.display_name.to_string()),
            icon_url: Some(self.icon_url()),
            description: Some(self.description()),
        }
    }

    /// This provider's `DisplayData$Description`: its own wording when it carries one,
    /// else the catalog rule — the display name as an application
    /// ([`CATALOG_DESCRIPTION_SUFFIX`]). Truncated on a character boundary to the
    /// modelled `Description` ceiling of 128, so no entry can put an over-long value on
    /// the wire.
    pub fn description(&self) -> String {
        let description = match self.description {
            Some(description) => description.to_string(),
            None => format!("{} {CATALOG_DESCRIPTION_SUFFIX}", self.display_name),
        };
        truncate_chars(&description, 128)
    }

    /// This provider's `DisplayData$IconUrl`:
    /// `https://<icon host>/app-<digest>/icons/default.png`.
    ///
    /// The digest is the first 16 hex digits of the SHA-256 of the provider ID — derived
    /// from the provider, so it is stable across reads and distinct per provider, and
    /// carries nothing from the request. The URL satisfies the modelled `IconUrl`
    /// (`^(http|https)://.*$`, 1..768).
    pub fn icon_url(&self) -> String {
        use ::sha2::Digest as _;
        let digest = ::hex::encode(::sha2::Sha256::digest(self.id.as_bytes()));
        format!(
            "https://{PROVIDER_ICON_HOST}/app-{}/icons/default.png",
            &digest[..16]
        )
    }

    /// This provider's `ResourceServerConfig`, or `None` when it exposes no resource
    /// server: an empty `Scopes` map would claim a resource server with no scopes, which
    /// is a different statement from having none.
    pub fn resource_server_config(&self) -> Option<ResourceServerConfig> {
        if self.scopes.is_empty() {
            return None;
        }
        Some(ResourceServerConfig {
            scopes: Some(
                self.scopes
                    .iter()
                    .map(|scope| (scope.name.to_string(), scope.to_shape()))
                    .collect(),
            ),
        })
    }

    /// This provider's ARN in the `aws` partition.
    ///
    /// `arn:aws:sso::aws:applicationProvider/<id>` — the provider ARN carries no region
    /// and the literal `aws` where a resource of the caller's would carry an account, so
    /// it is not built through `engine::emulator_arn`. The same provider exists in every
    /// partition the pattern admits (which is why
    /// [`built_in_application_provider`] matches on the id alone); an ENUMERATION has to
    /// report one ARN per entry, and it reports it in the partition the emulator serves.
    pub fn arn(&self) -> String {
        format!("arn:aws:sso::aws:applicationProvider/{}", self.id)
    }

    /// Whether an application can be CREATED under this provider through this service's
    /// API.
    ///
    /// `CreateApplication` makes an OAuth 2.0 customer managed application, so a provider
    /// backs one only when the customer manages the applications under it AND it federates
    /// over OAuth 2.0: an application whose provider federates over SAML 2.0 is set up in
    /// the console instead, and an application of an Amazon Web Services managed provider
    /// is created by the service that owns it on the customer's behalf. Every other
    /// operation of the provider surface is unaffected — a provider that backs no create
    /// is still described and enumerated like any other entry of the catalog.
    pub fn accepts_customer_managed_application(&self) -> bool {
        self.customer_managed && self.federation_protocol == FEDERATION_OAUTH
    }

    /// This provider as the modeled `ApplicationProvider` shape — the member-for-member
    /// projection `ListApplicationProviders` reports, holding exactly what
    /// `DescribeApplicationProvider` reports for the same entry.
    pub fn to_shape(&self) -> ApplicationProvider {
        ApplicationProvider {
            application_provider_arn: self.arn(),
            federation_protocol: Some(self.federation_protocol.to_string()),
            display_data: Some(self.display_data()),
            resource_server_config: self.resource_server_config(),
        }
    }
}

/// `text`, cut to at most `max_chars` characters on a character boundary.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// One AWS MANAGED application provider: an Amazon Web Services application that receives
/// a propagated identity from IAM Identity Center, so it federates over OAuth 2.0 and
/// carries its own portal description and — where it publishes one — its resource server's
/// access scopes.
///
/// The applications under such a provider are created by the Amazon Web Services service
/// that owns it, not by the customer, so none of them is customer managed.
const fn aws_managed_provider(
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    scopes: &'static [BuiltInAccessScope],
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_OAUTH,
        customer_managed: false,
        display_name,
        description: Some(description),
        scopes,
    }
}

/// The CUSTOM application provider: the one provider whose applications the CUSTOMER
/// creates and manages, over OAuth 2.0 — the provider `CreateApplication` mints an
/// application under ([`BuiltInApplicationProvider::accepts_customer_managed_application`]).
const fn customer_managed_provider(
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    scopes: &'static [BuiltInAccessScope],
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_OAUTH,
        customer_managed: true,
        display_name,
        description: Some(description),
        scopes,
    }
}

/// One PRE-INTEGRATED application of the IAM Identity Center application catalog: a SAML
/// 2.0 application whose ARN id is `catalog/<Name>`, whose portal description follows the
/// catalog rule ([`CATALOG_DESCRIPTION_SUFFIX`]) and which exposes no resource server of
/// its own (the application is the relying party, not an API an identity propagates into).
const fn catalog_application(
    id: &'static str,
    display_name: &'static str,
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_SAML,
        // A pre-integrated application is set up in the console, not through this API.
        customer_managed: false,
        display_name,
        description: None,
        scopes: &[],
    }
}

/// The built-in application providers of IAM Identity Center.
///
/// SOURCE: the Smithy model carries no list of them — `ApplicationProviderArn` is only a
/// pattern (`smithy_query(query="shape",shape="ApplicationProviderArn")`) and
/// `ApplicationProvider` only the per-entry structure — so the catalog is written out
/// here, from the sets IAM Identity Center publishes:
///
/// * the CUSTOM provider ([`customer_managed_provider`]) — the one provider a customer
///   managed application is created under;
/// * the AWS MANAGED providers ([`aws_managed_provider`]) — the Amazon Web Services
///   applications that receive a propagated identity (trusted identity propagation),
///   whose id is the service slug its `arn:aws:sso::aws:applicationProvider/<slug>` ARN
///   carries, and whose published access scopes are reported as its resource server;
/// * the APPLICATION CATALOG ([`catalog_application`]) — the pre-integrated SAML 2.0
///   applications of the IAM Identity Center application catalog, whose id is
///   `catalog/<Name>` (recorded behavior: the recorded catalog holds entries such as
///   `catalog/Statuspage`, `catalog/DuoNetworkGateway` and `catalog/DruvaInSync`
///   alongside service providers such as `glue`).
///
/// The MEMBERSHIP is behavior, not model: AWS adds applications to its catalog over time,
/// so this is the catalog the emulator publishes rather than a list any rule derives.
/// What IS a rule — every entry carries a complete `DisplayData`, an id of one of the two
/// forms above, and a `ResourceServerConfig` exactly when it publishes access scopes —
/// holds for every entry here, and for any entry added to it.
///
/// A provider exists in every partition the ARN pattern admits, so the lookup
/// ([`built_in_application_provider`]) matches on the id and not on the whole ARN.
pub const BUILT_IN_APPLICATION_PROVIDERS: &[BuiltInApplicationProvider] = &[
    // ---- The custom (customer managed) provider ---------------------------------------
    customer_managed_provider(
        "custom",
        "SSO Custom Application Provider",
        "SSO Custom Application Provider",
        &[],
    ),
    // ---- AWS managed providers -------------------------------------------------------
    aws_managed_provider(
        "athena",
        "Amazon Athena",
        "Interactive query service for data in Amazon S3",
        &[],
    ),
    aws_managed_provider(
        "emr",
        "Amazon EMR",
        "Managed cluster platform for big data frameworks",
        &[],
    ),
    aws_managed_provider(
        "emr-serverless",
        "Amazon EMR Serverless",
        "Serverless runtime for big data frameworks",
        &[],
    ),
    aws_managed_provider(
        "glue",
        "AWS Glue",
        "Serverless data integration service",
        &[BuiltInAccessScope {
            name: "glue:read_write",
            detailed_title: "Read and write AWS Glue data",
            long_description: "Grants access to read and write data through AWS Glue",
        }],
    ),
    aws_managed_provider(
        "lakeformation",
        "AWS Lake Formation",
        "Centrally governed permissions for a data lake",
        &[BuiltInAccessScope {
            name: "lakeformation:query",
            detailed_title: "Query Lake Formation data",
            long_description: "Grants access to query data governed by AWS Lake Formation",
        }],
    ),
    aws_managed_provider(
        "quicksight",
        "Amazon QuickSight",
        "Cloud-scale business intelligence service",
        &[],
    ),
    aws_managed_provider(
        "redshift",
        "Amazon Redshift",
        "Cloud data warehouse",
        &[BuiltInAccessScope {
            name: "redshift:connect",
            detailed_title: "Connect to Amazon Redshift",
            long_description: "Grants access to connect to an Amazon Redshift database",
        }],
    ),
    aws_managed_provider(
        "s3",
        "Amazon S3",
        "Object storage for any amount of data",
        &[BuiltInAccessScope {
            name: "s3:access_grants:read_write",
            detailed_title: "Read and write Amazon S3 data",
            long_description: "Grants access to read and write Amazon S3 data through access grants",
        }],
    ),
    // ---- The application catalog -----------------------------------------------------
    catalog_application("catalog/15Five", "15Five"),
    catalog_application("catalog/AbsorbLMS", "Absorb LMS"),
    catalog_application("catalog/Adobe", "Adobe"),
    catalog_application("catalog/Airtable", "Airtable"),
    catalog_application("catalog/Asana", "Asana"),
    catalog_application("catalog/AtlassianCloud", "Atlassian Cloud"),
    catalog_application("catalog/BambooHR", "BambooHR"),
    catalog_application("catalog/BetterCloud", "BetterCloud"),
    catalog_application("catalog/Bonusly", "Bonusly"),
    catalog_application("catalog/Box", "Box"),
    catalog_application("catalog/Bugsnag", "Bugsnag"),
    catalog_application("catalog/Calendly", "Calendly"),
    catalog_application("catalog/Canva", "Canva"),
    catalog_application("catalog/CircleCI", "CircleCI"),
    catalog_application("catalog/CiscoWebex", "Cisco Webex"),
    catalog_application("catalog/CitrixShareFile", "Citrix ShareFile"),
    catalog_application("catalog/ClickUp", "ClickUp"),
    catalog_application("catalog/Cloudflare", "Cloudflare"),
    catalog_application("catalog/Confluence", "Confluence"),
    catalog_application("catalog/Coupa", "Coupa"),
    catalog_application("catalog/CultureAmp", "Culture Amp"),
    catalog_application("catalog/Databricks", "Databricks"),
    catalog_application("catalog/Datadog", "Datadog"),
    catalog_application("catalog/Deputy", "Deputy"),
    catalog_application("catalog/Dialpad", "Dialpad"),
    catalog_application("catalog/Docebo", "Docebo"),
    catalog_application("catalog/DocuSign", "DocuSign"),
    catalog_application("catalog/Domo", "Domo"),
    catalog_application("catalog/DropboxBusiness", "Dropbox Business"),
    catalog_application("catalog/DruvaInSync", "Druva inSync"),
    catalog_application("catalog/DuoNetworkGateway", "Duo Network Gateway"),
    catalog_application("catalog/DynamicSignal", "Dynamic Signal"),
    catalog_application("catalog/Egnyte", "Egnyte"),
    catalog_application("catalog/Envoy", "Envoy"),
    catalog_application("catalog/Expensify", "Expensify"),
    catalog_application("catalog/Figma", "Figma"),
    catalog_application("catalog/Freshdesk", "Freshdesk"),
    catalog_application("catalog/Freshservice", "Freshservice"),
    catalog_application("catalog/GitHub", "GitHub"),
    catalog_application("catalog/GitLab", "GitLab"),
    catalog_application("catalog/Gong", "Gong"),
    catalog_application("catalog/GoogleWorkspace", "Google Workspace"),
    catalog_application("catalog/Greenhouse", "Greenhouse"),
    catalog_application("catalog/Gusto", "Gusto"),
    catalog_application("catalog/Hootsuite", "Hootsuite"),
    catalog_application("catalog/HubSpot", "HubSpot"),
    catalog_application("catalog/Igloo", "Igloo"),
    catalog_application("catalog/Intercom", "Intercom"),
    catalog_application("catalog/JFrogArtifactory", "JFrog Artifactory"),
    catalog_application("catalog/JamfPro", "Jamf Pro"),
    catalog_application("catalog/JiraCloud", "Jira Cloud"),
    catalog_application("catalog/KeeperSecurity", "Keeper Security"),
    catalog_application("catalog/Kisi", "Kisi"),
    catalog_application("catalog/Lattice", "Lattice"),
    catalog_application("catalog/LaunchDarkly", "LaunchDarkly"),
    catalog_application("catalog/Lessonly", "Lessonly"),
    catalog_application("catalog/Lever", "Lever"),
    catalog_application("catalog/Looker", "Looker"),
    catalog_application("catalog/Lucid", "Lucid"),
    catalog_application("catalog/Mailchimp", "Mailchimp"),
    catalog_application("catalog/Marketo", "Marketo"),
    catalog_application("catalog/Microsoft365", "Microsoft 365"),
    catalog_application("catalog/Miro", "Miro"),
    catalog_application("catalog/Mode", "Mode"),
    catalog_application("catalog/MongoDBAtlas", "MongoDB Atlas"),
    catalog_application("catalog/Namely", "Namely"),
    catalog_application("catalog/Netskope", "Netskope"),
    catalog_application("catalog/NewRelic", "New Relic"),
    catalog_application("catalog/Notion", "Notion"),
    catalog_application("catalog/OneTrust", "OneTrust"),
    catalog_application("catalog/Opsgenie", "Opsgenie"),
    catalog_application("catalog/PagerDuty", "PagerDuty"),
    catalog_application("catalog/Pingboard", "Pingboard"),
    catalog_application("catalog/Pipedrive", "Pipedrive"),
    catalog_application("catalog/Pluralsight", "Pluralsight"),
    catalog_application("catalog/Postman", "Postman"),
    catalog_application("catalog/Proxyclick", "Proxyclick"),
    catalog_application("catalog/Qualtrics", "Qualtrics"),
    catalog_application("catalog/Rollbar", "Rollbar"),
    catalog_application("catalog/SAPConcur", "SAP Concur"),
    catalog_application("catalog/Salesforce", "Salesforce"),
    catalog_application("catalog/Sentry", "Sentry"),
    catalog_application("catalog/ServiceNow", "ServiceNow"),
    catalog_application("catalog/Showpad", "Showpad"),
    catalog_application("catalog/Slack", "Slack"),
    catalog_application("catalog/Smartsheet", "Smartsheet"),
    catalog_application("catalog/Snowflake", "Snowflake"),
    catalog_application("catalog/Splunk", "Splunk"),
    catalog_application("catalog/Statuspage", "Statuspage"),
    catalog_application("catalog/SugarCRM", "SugarCRM"),
    catalog_application("catalog/SumoLogic", "Sumo Logic"),
    catalog_application("catalog/Tableau", "Tableau"),
    catalog_application("catalog/Talkdesk", "Talkdesk"),
    catalog_application("catalog/Tanium", "Tanium"),
    catalog_application("catalog/Teamwork", "Teamwork"),
    catalog_application("catalog/TerraformCloud", "Terraform Cloud"),
    catalog_application("catalog/Tipalti", "Tipalti"),
    catalog_application("catalog/Torii", "Torii"),
    catalog_application("catalog/Trello", "Trello"),
    catalog_application("catalog/Twilio", "Twilio"),
    catalog_application("catalog/Udemy", "Udemy"),
    catalog_application("catalog/UserTesting", "UserTesting"),
    catalog_application("catalog/Vanta", "Vanta"),
    catalog_application("catalog/Vonage", "Vonage"),
    catalog_application("catalog/Webflow", "Webflow"),
    catalog_application("catalog/Workday", "Workday"),
    catalog_application("catalog/WorkplaceByFacebook", "Workplace by Facebook"),
    catalog_application("catalog/Wrike", "Wrike"),
    catalog_application("catalog/Xero", "Xero"),
    catalog_application("catalog/Zendesk", "Zendesk"),
    catalog_application("catalog/Zoho", "Zoho"),
    catalog_application("catalog/Zoom", "Zoom"),
    catalog_application("catalog/Zscaler", "Zscaler"),
];

/// The provider id an `ApplicationProviderArn` names, or `None` when the ARN is not an
/// application-provider ARN at all.
///
/// Everything before `:sso::aws:applicationProvider/` is the `arn:<partition>` prefix,
/// which the model's pattern already constrained, so only the id has to be read out.
pub fn application_provider_id(application_provider_arn: &str) -> Option<&str> {
    let (prefix, id) = application_provider_arn.split_once(":sso::aws:applicationProvider/")?;
    prefix.starts_with("arn:").then_some(id)
}

/// The built-in provider `application_provider_arn` names, or `None` for any other
/// syntactically valid ARN — which `DescribeApplicationProvider` answers with the
/// modelled `ResourceNotFoundException`.
pub fn built_in_application_provider(
    application_provider_arn: &str,
) -> Option<&'static BuiltInApplicationProvider> {
    let id = application_provider_id(application_provider_arn)?;
    BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .find(|provider| provider.id == id)
}

/// The ARN of the instance whose id is `instance_id`.
///
/// `arn:aws:sso:::instance/<id>` — an IAM Identity Center instance ARN carries NO
/// region and NO account segment (the model's pattern ends the prefix at `sso:::`),
/// which is why it is not built through `engine::emulator_arn`; the owning account is
/// reported separately as `InstanceMetadata$OwnerAccountId`.
pub fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:sso:::instance/{instance_id}")
}

/// The ARN of the identity store whose id is `identity_store_id`, inside the modelled
/// `InstanceIdentityStoreArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:identitystore::\d{12}:identitystore/(d-[0-9a-f]{10}|…)$`.
pub fn identity_store_arn(identity_store_id: &str) -> String {
    format!(
        "arn:aws:identitystore::{}:identitystore/{identity_store_id}",
        ::engine::emulator_account_id()
    )
}

/// Mint a fresh instance id: `ssoins-` + 16 characters, the form the model's
/// `InstanceArn` pattern accepts
/// (`^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`).
pub fn mint_instance_id() -> String {
    format!("ssoins-{}", &mint_digest()[..16])
}

/// Mint a fresh identity-store id: `d-` + 10 hex digits, the form
/// `InstanceIdentityStoreArn` accepts and which keeps that ARN inside its modelled
/// 62..93 length.
pub fn mint_identity_store_id() -> String {
    format!("d-{}", &mint_digest()[..10])
}

/// Mint a fresh `PermissionSetArn` under `instance_arn`.
///
/// `arn:<partition>:sso:::permissionSet/<instance id>/ps-<16 chars>`, per the model's
/// `PermissionSetArn` pattern — the partition and the instance id are carried over
/// from the owning instance's ARN, and only the `ps-` suffix is minted.
pub fn mint_permission_set_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    // 16 hex digits, inside `ps-[a-zA-Z0-9-./]{16}` and unique per call so two creates
    // never collide.
    let suffix = &mint_digest()[..16];
    format!("arn:{partition}:sso:::permissionSet/{instance_id}/ps-{suffix}")
}

/// Mint a fresh `TrustedTokenIssuerArn` under `instance_arn`.
///
/// `arn:<partition>:sso::<account>:trustedTokenIssuer/<instance id>/tti-<uuid>`, per the
/// model's `TrustedTokenIssuerArn` pattern — unlike an instance or permission-set ARN,
/// this one carries the owning ACCOUNT (`:sso::\d{12}:`) and no region, so it is built
/// here rather than through `engine::emulator_arn`. The partition and instance id come
/// from the owning instance's ARN; only the `tti-` UUID is minted.
pub fn mint_trusted_token_issuer_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    format!(
        "arn:{partition}:sso::{}:trustedTokenIssuer/{instance_id}/tti-{}",
        ::engine::emulator_account_id(),
        mint_uuid_v4(),
    )
}

/// Mint a fresh `ApplicationArn` under `instance_arn`.
///
/// `arn:<partition>:sso::<account>:application/<instance id>/apl-<16 chars>`, per the
/// model's `ApplicationArn` pattern — like a `TrustedTokenIssuerArn` it carries the
/// owning ACCOUNT (`:sso::\d{12}:`) and no region. The partition and instance id come
/// from the owning instance's ARN; only the `apl-` suffix is minted, as 16 hex digits,
/// which is inside the pattern's `apl-[a-zA-Z0-9]{16}` and unique per call so two
/// creates never collide.
pub fn mint_application_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    format!(
        "arn:{partition}:sso::{}:application/{instance_id}/apl-{}",
        ::engine::emulator_account_id(),
        &mint_digest()[..16],
    )
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying
/// `CreateApplication` is scoped to — the same rule as
/// [`trusted_token_issuer_fingerprint`], over this operation's members.
///
/// `Tags` go in SORTED, because a tag set carries no order the caller chose; everything
/// else is length-prefixed in a fixed order by [`request_fingerprint`].
pub fn application_fingerprint(
    instance_arn: &str,
    application_provider_arn: &str,
    name: &str,
    description: Option<&str>,
    portal_options: &PortalOptions,
    status: &str,
    tags: &[Tag],
) -> String {
    let sign_in = portal_options.sign_in_options.as_ref();
    let mut parts = vec![
        instance_arn.to_string(),
        application_provider_arn.to_string(),
        name.to_string(),
        description.unwrap_or_default().to_string(),
        status.to_string(),
        portal_options.visibility.clone(),
        sign_in
            .map(|options| options.origin.clone())
            .unwrap_or_default(),
        sign_in
            .and_then(|options| options.application_url.clone())
            .unwrap_or_default(),
    ];
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
    request_fingerprint(&parts)
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying
/// `CreateTrustedTokenIssuer` is scoped to.
///
/// It is what tells a genuine retry (the same `ClientToken` AND the same request) from
/// the same token carrying a different request, which is a conflict rather than a
/// replay. `Tags` go in SORTED, because a tag set carries no order the caller chose.
pub fn trusted_token_issuer_fingerprint(
    instance_arn: &str,
    name: &str,
    trusted_token_issuer_type: &str,
    configuration: &TrustedTokenIssuerConfiguration,
    tags: &[Tag],
) -> String {
    let mut parts = vec![
        instance_arn.to_string(),
        name.to_string(),
        trusted_token_issuer_type.to_string(),
    ];
    match configuration {
        TrustedTokenIssuerConfiguration::OidcJwtConfiguration(oidc) => {
            parts.push("OidcJwtConfiguration".to_string());
            parts.push(oidc.issuer_url.clone());
            parts.push(oidc.claim_attribute_path.clone());
            parts.push(oidc.identity_store_attribute_path.clone());
            parts.push(oidc.jwks_retrieval_option.clone());
        }
    }
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
    request_fingerprint(&parts)
}

/// A stable digest of a request's parts, each LENGTH-PREFIXED so `["a", "bc"]` and
/// `["ab", "c"]` cannot collide.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// A fresh `RequestId`: the hyphenated lowercase UUID the model's `UUId` shape
/// requires (`length 36..36`, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`),
/// laid out as a version-4 UUID (the `4` in the third group, and the RFC 4122 variant
/// nibble `8`..`b` opening the fourth).
pub fn mint_request_id() -> String {
    mint_uuid_v4()
}

/// A fresh hyphenated lowercase version-4 UUID: `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-
/// [89ab][0-9a-f]{3}-[0-9a-f]{12}` (the `4` names the version and the RFC 4122 variant
/// nibble `8`..`b` opens the fourth group). It is the form the model's `UUId` shape and
/// the `tti-` segment of `TrustedTokenIssuerArn` both require.
fn mint_uuid_v4() -> String {
    let digest = mint_digest();
    let variant = match u8::from_str_radix(&digest[30..31], 16).unwrap_or_default() % 4 {
        0 => '8',
        1 => '9',
        2 => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..15],
        &digest[15..18],
        &digest[18..30],
    )
}

/// Per-process counter behind every minted identifier, so two mints in one process
/// differ whatever the clock reports.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// 64 lowercase hex digits unique to this mint.
///
/// SHA-256 of this process's id, the wall clock in nanoseconds and [`MINT_SEQUENCE`]:
/// the counter separates two mints within one process, the pid and clock separate two
/// processes, so no identifier built from it repeats. Nothing here comes from the
/// request, so a caller can neither predict an id nor steer it into another's.
fn mint_digest() -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(::std::process::id().to_le_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher.update(
        MINT_SEQUENCE
            .fetch_add(1, ::std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    ::hex::encode(hasher.finalize())
}

/// Cursor scope of `ListInstances`' pagination token. The enumeration takes no
/// filter, so its scope is the operation alone.
pub const CURSOR_INSTANCES: &str = "instances";

/// Cursor scope of `ListApplicationProviders`' pagination token. The enumeration walks
/// the whole AWS-owned catalog and takes no filter, so its scope is the operation alone.
pub const CURSOR_APPLICATION_PROVIDERS: &str = "applicationProviders";

/// Cursor scope of a `ListPermissionSets` enumeration: the operation and the instance
/// whose permission sets it walks.
pub fn cursor_permission_sets(instance_arn: &str) -> String {
    format!("permissionSets|{instance_arn}")
}

/// Cursor scope of a `ListManagedPoliciesInPermissionSet` enumeration: the operation and
/// the permission set whose attached AWS managed policies it walks, qualified by the
/// instance that permission set belongs to (the same pair the read resolves against).
pub fn cursor_managed_policies(instance_arn: &str, permission_set_arn: &str) -> String {
    format!("managedPolicies|{instance_arn}|{permission_set_arn}")
}

/// Cursor scope of a `ListApplicationAccessScopes` enumeration: the operation and the
/// application whose access scopes it walks (the same application every scope read
/// resolves against).
pub fn cursor_access_scopes(application_arn: &str) -> String {
    format!("accessScopes|{application_arn}")
}

/// Cursor scope of a `ListApplicationGrants` enumeration: the operation and the
/// application whose grants it walks, since the key (the grant's type) only
/// distinguishes rows WITHIN one application.
pub fn cursor_application_grants(application_arn: &str) -> String {
    format!("applicationGrants|{application_arn}")
}

/// Cursor scope of a `ListTagsForResource` enumeration: the operation and the resource
/// whose tags it walks, which is the whole filter that selected the rows.
pub fn cursor_tags(resource_arn: &str) -> String {
    format!("tags|{resource_arn}")
}

/// Cursor scope of a `ListAccountAssignments` enumeration: the operation and the whole
/// filter that selected the rows, since the key only distinguishes rows WITHIN it.
pub fn cursor_account_assignments(
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
) -> String {
    format!("accountAssignments|{instance_arn}|{account_id}|{permission_set_arn}")
}

/// Cursor scope of a `ListAccountsForProvisionedPermissionSet` enumeration: the operation
/// and the whole filter that selected the rows — the instance, the permission set whose
/// provisioned accounts it walks and the `ProvisioningStatus` it was filtered by, since
/// the key (an account id) only distinguishes rows within one such filter.
pub fn cursor_accounts_for_provisioned_permission_set(
    instance_arn: &str,
    permission_set_arn: &str,
    provisioning_status: Option<&str>,
) -> String {
    format!(
        "accountsForProvisionedPermissionSet|{instance_arn}|{permission_set_arn}|{}",
        provisioning_status.unwrap_or_default()
    )
}

/// Cursor scope of a `ListPermissionSetsProvisionedToAccount` enumeration: the operation
/// and the whole filter that selected the rows — the instance, the account whose
/// provisioned permission sets it walks and the `ProvisioningStatus` it was filtered by,
/// since the key (a permission-set ARN) only distinguishes rows within one such filter.
pub fn cursor_permission_sets_provisioned_to_account(
    instance_arn: &str,
    account_id: &str,
    provisioning_status: Option<&str>,
) -> String {
    format!(
        "permissionSetsProvisionedToAccount|{instance_arn}|{account_id}|{}",
        provisioning_status.unwrap_or_default()
    )
}

/// The refusal a `NextToken` this service did not issue earns, as the message of the
/// caller's own `ValidationException`.
pub const BAD_CURSOR_MESSAGE: &str =
    "NextToken is not a pagination token issued by this service for this enumeration";

/// Encode a pagination cursor: the SCOPE of the enumeration it belongs to (the
/// operation and the filter that selected the rows) and the key of the last item
/// returned on this page.
///
/// The cursor is SELF-IDENTIFYING — it carries a version marker and that scope, so a
/// token this service never issued (including one that happens to be valid hex) and a
/// token issued for a DIFFERENT scope (another operation, another instance, another
/// account or permission-set filter) are both rejected rather than silently resuming
/// inside an enumeration whose keys mean something else. Hex keeps the token inside
/// `Token`'s `^[-a-zA-Z0-9+=/_]*$` pattern even though the scope and the keys (ARNs)
/// contain characters it rejects, and it is reversible, so the next page resumes from
/// the key rather than from a positional offset that a concurrent create would shift.
pub fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("ssoadmin:v1:{scope}:{key}"))
}

/// Decode a cursor [`encode_page_token`] issued for `scope`; `None` for anything else.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("ssoadmin:v1:{scope}:"))
        .map(str::to_string)
}

/// The key a paginated read resumes after, from the caller's `NextToken`.
///
/// `Ok(None)` starts at the beginning (no token). `Err` is the refusal the caller's own
/// `ValidationException` carries: answering an empty page for a token this service did
/// not issue would silently end a caller's enumeration early.
pub fn cursor_after(
    scope: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, &'static str> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_page_token(scope, token)
            .map(Some)
            .ok_or(BAD_CURSOR_MESSAGE),
    }
}

/// One page of a key-ordered sequence.
///
/// `ordered` is ascending by key; `after` is the key the previous page ended on, so
/// this page is the items whose key sorts strictly after it. The returned token is
/// `Some` only when at least one item was left over — the last page carries none.
pub fn page<T>(
    scope: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;
    for (key, item) in ordered {
        if after.is_some_and(|cursor| key.as_str() <= cursor) {
            continue;
        }
        if items.len() == limit {
            truncated = true;
            break;
        }
        last_key = Some(key);
        items.push(item);
    }
    let next_token = if truncated {
        last_key.as_deref().map(|key| encode_page_token(scope, key))
    } else {
        None
    };
    (items, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsoadminSwbexternalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsoadminSwbexternalserviceState>)
}
