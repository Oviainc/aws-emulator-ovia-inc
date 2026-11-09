// SPDX-License-Identifier: CC0-1.0

//! @op DescribeType
//! Reads one record out of this account and Region's extension registry
//! ([`crate::state::registry`]) and reports the version the request selects.
//!
//! ## What the operation reports
//!
//! "Returns detailed information about an extension from the CloudFormation registry in
//! your current account and Region. If you specify a VersionId, DescribeType returns
//! information about that specific extension version. Otherwise, it returns information
//! about the default extension version." So a request does two things, and this handler
//! keeps them separate:
//!
//! 1. it IDENTIFIES an extension — by `Arn`, or by `Type` and `TypeName` ("Conditional:
//!    You must specify either TypeName and Type, or Arn"), with `PublisherId` naming the
//!    publisher the extension must come from, and in BOTH forms within the Region the
//!    request was served in: the registry is regional, so an ARN minted for another Region
//!    names nothing here;
//! 2. it SELECTS one version of it — the one `VersionId` names, the one
//!    `PublicVersionNumber` names, the one a version ARN pins, or the record's default
//!    version.
//!
//! Neither step invents anything: an extension the registry does not hold, and a version
//! the record does not carry, are both answered with the modelled `TypeNotFoundException`
//! (404, "The specified extension doesn't exist in the CloudFormation registry"), and a
//! request that identifies no extension at all is refused with the registry's own modelled
//! `CFNRegistryException` (400). A PRIVATE registration that has been DEREGISTERED — one with
//! no version left in active use — is no longer an extension this registry holds at all, so a
//! request that names no version of it, and therefore resolves its default version ("the
//! version used when the extension version isn't specified"), is that same 404; a request
//! naming one of its versions still reports that version, since "to view the deprecation
//! status of an extension or extension version, use DescribeType".
//!
//! The registry has two sources, and this handler reads them in that order: the records
//! this account's registry writes persist, and the public extensions Amazon Web Services
//! publishes, which are in every account's registry without anybody registering them
//! ([`crate::state::registry::aws_published`]).
//!
//! Which members come back is gated on the record's VISIBILITY, member by member as the
//! model documents it: the default version, the deprecation status, the logging
//! configuration, the execution role and the contract-test status are private-extension
//! members, while the publisher id, the public version number, `IsActivated` and
//! `AutoUpdate` belong to public third-party extensions. Nothing is written: this is a
//! reporting operation.
//!
//! NOTE ON THE MODEL: `SchemaHandlerPermissions` (a list of strings) was ADDED to
//! `DescribeTypeOutput` in this service's `smithy-model/` copy. The shipped model declares no
//! such member while AWS answers this read with the element present — empty for an extension
//! whose schema declares no handler permission — as the recorded wire responses for this
//! operation show. The emulator serialises what AWS puts on the wire, so the member is
//! modelled here; nothing else in the shipped model was changed for it. What it reports is
//! read out of the schema the registry holds for the described version
//! ([`crate::state::resource_schema::schema_handler_permissions`]), so the permissions and
//! the `Schema` they come from cannot disagree.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`; the
//! Region a request is served in comes from there ([`crate::state::request_region`]).
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Returns detailed information about an extension from the CloudFormation registry in your
//! current account and Region. If you specify a VersionId, DescribeType returns information
//! about that specific extension version. Otherwise, it returns information about the
//! default extension version. For more information, see Edit configuration data for
//! extensions in your account in the CloudFormation User Guide.
//!
//! ## Input
//!
//! * `Arn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension. Conditional: You must specify either
//!   TypeName and Type, or Arn.
//! * `PublicVersionNumber` (PublicVersionNumber, len 5.., pattern
//!   ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$) — The version number of a public third-party
//!   extension.
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The publisher ID
//!   of the extension publisher. Extensions provided by Amazon Web Services are not assigned
//!   a publisher ID.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The kind of extension.
//!   Conditional: You must specify either TypeName and Type, or Arn.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension. Conditional: You must specify either TypeName and Type, or Arn.
//! * `VersionId` (TypeVersionId, len 1..128, pattern ^[A-Za-z0-9-]+$) — The ID of a specific
//!   version of the extension. The version ID is the value at the end of the Amazon Resource
//!   Name (ARN) assigned to the extension version when it is registered. If you specify a
//!   VersionId, DescribeType returns information about that specific extension version.
//!   Otherwise, it returns information about the default extension version.
//!
//! ## Output
//!
//! * `Arn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension.
//! * `AutoUpdate` (AutoUpdate) — Whether CloudFormation automatically updates the extension
//!   in this account and Region when a new minor version is published by the extension
//!   publisher. Major versions released by the publisher must be manually updated. For more
//!   information, see Automatically use new versions of extensions in the CloudFormation User
//!   Guide.
//! * `ConfigurationSchema` (ConfigurationSchema, len 1..60000, pattern ^[\s\S]+$) — A JSON
//!   string that represent the current configuration data for the extension in this account
//!   and Region. To set the configuration data for an extension, use SetTypeConfiguration.
//! * `DefaultVersionId` (TypeVersionId, len 1..128, pattern ^[A-Za-z0-9-]+$) — The ID of the
//!   default version of the extension. The default version is used when the extension version
//!   isn't specified. This applies only to private extensions you have registered in your
//!   account. For public extensions, both those provided by Amazon Web Services and published
//!   by third parties, CloudFormation returns null. For more information, see RegisterType.
//!   To set the default version of an extension, use SetTypeDefaultVersion.
//! * `DeprecatedStatus` (DeprecatedStatus, one of: DEPRECATED | LIVE) — The deprecation
//!   status of the extension version. Valid values include: LIVE: The extension is activated
//!   or registered and can be used in CloudFormation operations, dependent on its
//!   provisioning behavior and visibility scope. DEPRECATED: The extension has been
//!   deactivated or deregistered and can no longer be used in CloudFormation operations. For
//!   public third-party extensions, CloudFormation returns null.
//! * `Description` (Description, len 1..1024) — The description of the extension.
//! * `DocumentationUrl` (OptionalSecureUrl, len 0..4096) — The URL of a page providing
//!   detailed documentation for this extension.
//! * `ExecutionRoleArn` (RoleARN2, len 1..256, pattern ^arn:.+:iam::[0-9]{12}:role/.+$) — The
//!   Amazon Resource Name (ARN) of the IAM execution role used to register the extension.
//!   This applies only to private extensions you have registered in your account. For more
//!   information, see RegisterType. If the registered extension calls any Amazon Web Services
//!   APIs, you must create an IAM execution role that includes the necessary permissions to
//!   call those Amazon Web Services APIs, and provision that execution role in your account.
//!   CloudFormation then assumes that execution role to provide your extension with the
//!   appropriate credentials.
//! * `IsActivated` (IsActivated) — Whether the extension is activated in the account and
//!   Region. This only applies to public third-party extensions. For all other extensions,
//!   CloudFormation returns null.
//! * `IsDefaultVersion` (IsDefaultVersion) — Whether the specified extension version is set
//!   as the default version. This applies only to private extensions you have registered in
//!   your account, and extensions published by Amazon Web Services. For public third-party
//!   extensions, whether they are activated in your account, CloudFormation returns null.
//! * `LastUpdated` (Timestamp) — When the specified extension version was registered. This
//!   applies only to: Private extensions you have registered in your account. For more
//!   information, see RegisterType. Public extensions you have activated in your account with
//!   auto-update specified. For more information, see ActivateType.
//! * `LatestPublicVersion` (PublicVersionNumber, len 5.., pattern
//!   ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$) — The latest version of a public extension that is
//!   available for use. This only applies if you specify a public extension, and you don't
//!   specify a version. For all other requests, CloudFormation returns null.
//! * `LoggingConfig` (LoggingConfig) — Contains logging configuration information for private
//!   extensions. This applies only to private extensions you have registered in your account.
//!   For public extensions, both those provided by Amazon Web Services and published by third
//!   parties, CloudFormation returns null. For more information, see RegisterType.
//! * `OriginalTypeArn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   For public extensions that have been activated for this account and Region, the Amazon
//!   Resource Name (ARN) of the public extension.
//! * `OriginalTypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — For public
//!   extensions that have been activated for this account and Region, the type name of the
//!   public extension. If you specified a TypeNameAlias when enabling the extension in this
//!   account and Region, CloudFormation treats that alias as the extension's type name within
//!   the account and Region, not the type name of the public extension. For more information,
//!   see Use aliases to refer to extensions in the CloudFormation User Guide.
//! * `ProvisioningType` (ProvisioningType, one of: FULLY_MUTABLE | IMMUTABLE |
//!   NON_PROVISIONABLE) — For resource type extensions, the provisioning behavior of the
//!   resource type. CloudFormation determines the provisioning type during registration,
//!   based on the types of handlers in the schema handler package submitted. Valid values
//!   include: FULLY_MUTABLE: The resource type includes an update handler to process updates
//!   to the type during stack update operations. IMMUTABLE: The resource type doesn't include
//!   an update handler, so the type can't be updated and must instead be replaced during
//!   stack update operations. NON_PROVISIONABLE: The resource type doesn't include all the
//!   following handlers, and therefore can't actually be provisioned. create read delete
//! * `PublicVersionNumber` (PublicVersionNumber, len 5.., pattern
//!   ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$) — The version number of a public third-party
//!   extension. This applies only if you specify a public extension you have activated in
//!   your account, or specify a public extension without specifying a version. For all other
//!   extensions, CloudFormation returns null.
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The publisher ID
//!   of the extension publisher. This applies only to public third-party extensions. For
//!   private registered extensions, and extensions provided by Amazon Web Services,
//!   CloudFormation returns null.
//! * `RequiredActivatedTypes` (RequiredActivatedTypes) — For extensions that are modules, the
//!   public third-party extensions that must be activated in your account in order for the
//!   module itself to be activated.
//! * `Schema` (TypeSchema, len 1..16777216) — The schema that defines the extension. For more
//!   information, see Resource type schema in the CloudFormation Command Line Interface (CLI)
//!   User Guide and the CloudFormation Hooks User Guide.
//! * `SchemaHandlerPermissions` (SchemaHandlerPermissions) — The permissions the handlers
//!   declared in the extension's schema require.
//! * `SourceUrl` (OptionalSecureUrl, len 0..4096) — The URL of the source code for the
//!   extension.
//! * `TimeCreated` (Timestamp) — When the specified private extension version was registered
//!   or activated in your account.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The kind of extension.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension. If the extension is a public third-party type you have activated with a
//!   type name alias, CloudFormation returns the type name alias. For more information, see
//!   ActivateType.
//! * `TypeTestsStatus` (TypeTestsStatus, one of: FAILED | IN_PROGRESS | NOT_TESTED | PASSED)
//!   — The contract test status of the registered extension version. To return the extension
//!   test status of a specific extension version, you must specify VersionId. This applies
//!   only to registered private extension versions. CloudFormation doesn't return this
//!   information for public extensions, whether they are activated in your account. PASSED:
//!   The extension has passed all its contract tests. An extension must have a test status of
//!   PASSED before it can be published. For more information, see Publishing extensions to
//!   make them available for public use in the CloudFormation Command Line Interface (CLI)
//!   User Guide. FAILED: The extension has failed one or more contract tests. IN_PROGRESS:
//!   Contract …
//! * `TypeTestsStatusDescription` (TypeTestsStatusDescription, len 1..1024, pattern
//!   ^[\s\S]+$) — The description of the test status. To return the extension test status of
//!   a specific extension version, you must specify VersionId. This applies only to
//!   registered private extension versions. CloudFormation doesn't return this information
//!   for public extensions, whether they are activated in your account.
//! * `Visibility` (Visibility, one of: PRIVATE | PUBLIC) — The scope at which the extension
//!   is visible and usable in CloudFormation operations. Valid values include: PRIVATE: The
//!   extension is only visible and usable within the account in which it is registered.
//!   CloudFormation marks any extensions you register as PRIVATE. PUBLIC: The extension is
//!   publicly visible and usable within any Amazon Web Services account.
//!
//! ## Errors
//!
//! * `CFNRegistryException` (HTTP 400) — An error occurred during a CloudFormation registry
//!   operation.
//! * `TypeNotFoundException` (HTTP 404) — The specified extension doesn't exist in the
//!   CloudFormation registry.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#DescribeType")]
#[derive(Debug)]
pub enum DescribeTypeErrorModeled {
    CFNRegistryException(CFNRegistryException),
    TypeNotFoundException(TypeNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_type::DescribeTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeTypeError = crate::scaffolded::Scaffolded<DescribeTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeTypeErrorModeled> for DescribeTypeError {
    fn from(modeled: DescribeTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeType<Eng>
{
    type Input = DescribeTypeInput;
    type Output = DescribeTypeOutput;
    type Error = DescribeTypeError;

    const NAME: &'static str = "DescribeType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // What the request identifies, and which version of it it selects. A combination
        // of members that cannot hold together is refused with the registry's own modelled
        // 400 before any state is touched.
        let (identifier, wanted) = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to read, so this
            // answers 501 rather than a truthful-looking "no such extension".
            return Err(crate::scaffolded::Scaffolded::not_implemented("DescribeType"));
        };
        let region = crate::state::request_region(ctx.parts());
        // A pure report, but one that must not withhold a transition the clock already made
        // due: `TestType` leaves a version's contract tests in progress and their conclusion
        // due a moment later, and this read is what "monitor[s] the current test status".
        // Settling materialises only that already-due conclusion — nothing is advanced
        // because it was read.
        let mut store = state.lock();
        store.settle(::std::time::SystemTime::now());

        let stored = match identifier {
            Identifier::Arn { extension, .. } => store.extension_by_arn(&region, extension),
            Identifier::Name {
                type_kind,
                type_name,
            } => store.extension_by_name(&region, type_kind, type_name),
        };
        // "If you specify a VersionId, DescribeType returns information about that specific
        // extension version. Otherwise, it returns information about the default extension
        // version": a request that names no version resolves the record's default version, and
        // a PRIVATE registration every version of which has been deregistered has none left in
        // active use — "deregistering that version results in the extension itself being
        // deregistered" — so there is no extension left for such a request to describe. A
        // request that names one of its versions resolves that version instead, which is the
        // read "to view the deprecation status of an extension or extension version".
        // `LatestPublicVersion` "only applies if you specify a public extension, and you
        // don't specify a version" — and a request that names no version is also the one that
        // resolves the record's DEFAULT version.
        let named_no_version = matches!(wanted, Wanted::Default);
        let stored = stored
            .filter(|extension| !named_no_version || !extension.is_private() || extension.is_live());
        // The registry also holds the public extensions Amazon Web Services publishes,
        // which are in every account's registry without anybody registering them (see
        // [`crate::state::registry::aws_published`]). A name this account registered
        // cannot be one of those — `RegisterType` reserves the `AWS` namespace — so the
        // two sources cannot disagree about one extension. Both are read in the Region the
        // request was served in: the registry is regional whichever source holds a record.
        let published = match (stored.is_none(), identifier) {
            (false, _) => None,
            (true, Identifier::Arn { extension, .. }) => {
                crate::state::registry::aws_published_by_arn(&region, extension)
            }
            (
                true,
                Identifier::Name {
                    type_kind,
                    type_name,
                },
            ) => crate::state::registry::aws_published(&region, type_kind, type_name),
        };
        let extension = stored
            .or(published.as_ref())
            .ok_or_else(|| type_not_found(identifier.named()))?;
        // `PublisherId` is "the publisher ID of the extension publisher", so a record
        // some other publisher published — or one nobody did, as the extensions "provided
        // by Amazon Web Services are not assigned a publisher ID" — is not the extension
        // the request named.
        if let Some(publisher_id) = input.publisher_id.as_deref() {
            if extension.publisher_id.as_deref() != Some(publisher_id) {
                return Err(type_not_found(identifier.named()));
            }
        }

        let version = match wanted {
            Wanted::Default => extension.default_version(),
            Wanted::Id(version_id) => extension.version(version_id),
            Wanted::PublicNumber(number) => extension.version_of_public_number(number),
        }
        .ok_or_else(|| type_not_found(&wanted.named_under(identifier.named())))?;

        // Which members come back is gated on the record's visibility, member by member as
        // the model documents it.
        let private = extension.is_private();
        let public_third_party = extension.is_public_third_party();
        // "To return the extension test status of a specific extension version, you must
        // specify VersionId" — a version ARN pins the same version id, so it counts as
        // naming one.
        let named_a_version_id = matches!(wanted, Wanted::Id(_));

        Ok(DescribeTypeOutput {
            // The permissions the extension's handlers require, read out of the schema that
            // defines it: the element is PRESENT whatever the record is — empty for an
            // extension whose schema declares no handler, and for one the registry holds no
            // schema for — so a client inspecting them sees the same shape for every
            // extension.
            schema_handler_permissions: Some(
                crate::state::resource_schema::schema_handler_permissions(
                    version.schema.as_deref(),
                ),
            ),
            // The ARN of what was described — this version's own ARN, the one `TestType`
            // documents handing back "into DescribeType to monitor the current test
            // status".
            arn: Some(version.arn.clone()),
            r#type: Some(extension.type_kind.clone()),
            // For an extension activated under an alias this is the alias, which is what
            // the record holds: "CloudFormation treats that alias as the extension's type
            // name within the account and Region".
            type_name: Some(extension.type_name.clone()),
            // "This applies only to private extensions you have registered in your
            // account. For public extensions, both those provided by Amazon Web Services
            // and published by third parties, CloudFormation returns null."
            default_version_id: private
                .then(|| extension.default_version_id.clone())
                .flatten(),
            // "This applies only to private extensions you have registered in your
            // account, and extensions published by Amazon Web Services. For public
            // third-party extensions ... CloudFormation returns null."
            is_default_version: (!public_third_party)
                .then(|| extension.is_default_version(&version.version_id)),
            // "This applies only to registered private extension versions. CloudFormation
            // doesn't return this information for public extensions."
            type_tests_status: (private && named_a_version_id)
                .then(|| version.type_tests_status.clone())
                .flatten(),
            type_tests_status_description: (private && named_a_version_id)
                .then(|| version.type_tests_status_description.clone())
                .flatten(),
            description: version.description.clone(),
            schema: version.schema.clone(),
            provisioning_type: version.provisioning_type.clone(),
            // "For public third-party extensions, CloudFormation returns null" — the
            // published extension's own record carries no deprecation status, since nothing
            // has deactivated or deregistered it. This account's own ACTIVATION of one does:
            // `DEPRECATED` is "the extension has been deactivated or deregistered and can no
            // longer be used in CloudFormation operations", which is a statement about the
            // copy this account activated, so a read of that copy reports it.
            deprecated_status: (!public_third_party || extension.is_activation())
                .then(|| version.deprecated_status.clone()),
            // "This applies only to private extensions you have registered in your
            // account. For public extensions ... CloudFormation returns null."
            logging_config: private
                .then(|| logging_config_view(version.logging_config.as_ref()))
                .flatten(),
            required_activated_types: crate::state::non_empty(
                version
                    .required_activated_types
                    .iter()
                    .map(required_activated_type_view)
                    .collect(),
            ),
            // "This applies only to private extensions you have registered in your
            // account."
            execution_role_arn: private
                .then(|| version.execution_role_arn.clone())
                .flatten(),
            // "The scope at which the extension is visible and usable in CloudFormation
            // operations": the scope the record is in
            // ([`crate::state::registry::StoredExtension::reported_visibility`]), which for
            // an extension this account ACTIVATED is this account and Region alone.
            visibility: Some(extension.reported_visibility().to_string()),
            source_url: version.source_url.clone(),
            documentation_url: version.documentation_url.clone(),
            last_updated: version.last_updated.map(crate::state::timestamp),
            time_created: version.time_created.map(crate::state::timestamp),
            // The configuration data `SetTypeConfiguration` holds "for the extension in
            // this account and Region", which is per extension rather than per version.
            configuration_schema: extension.configuration_schema.clone(),
            // "This applies only to public third-party extensions. For private registered
            // extensions, and extensions provided by Amazon Web Services, CloudFormation
            // returns null."
            publisher_id: public_third_party
                .then(|| extension.publisher_id.clone())
                .flatten(),
            // "For public extensions that have been activated for this account and
            // Region" — the public extension behind the local name.
            original_type_name: (!private)
                .then(|| extension.original_type_name.clone())
                .flatten(),
            original_type_arn: (!private)
                .then(|| extension.original_type_arn.clone())
                .flatten(),
            // "This applies only if you specify a public extension you have activated in
            // your account, or specify a public extension without specifying a version.
            // For all other extensions, CloudFormation returns null."
            public_version_number: (!private)
                .then(|| version.public_version_number.clone())
                .flatten(),
            latest_public_version: (!private && named_no_version)
                .then(|| extension.latest_public_version.clone())
                .flatten(),
            // "This only applies to public third-party extensions. For all other
            // extensions, CloudFormation returns null."
            is_activated: public_third_party.then_some(extension.is_activated).flatten(),
            auto_update: public_third_party.then_some(extension.auto_update).flatten(),
        })
    }
}

/// How a request identifies the extension to describe.
///
/// The model states the constraint across three members at once — "Conditional: You must
/// specify either TypeName and Type, or Arn" — so a request carries one form or the other,
/// never a mix and never neither.
#[derive(Debug, Clone, Copy)]
enum Identifier<'a> {
    /// `Arn`, which addresses the record on its own; a version ARN pins the version too.
    Arn {
        extension: &'a str,
        version_id: Option<&'a str>,
    },
    /// `Type` and `TypeName`, resolved in the Region the request was signed for.
    Name {
        type_kind: &'a str,
        type_name: &'a str,
    },
}

impl Identifier<'_> {
    /// What the request named the extension by, for the not-found message.
    fn named(&self) -> &str {
        match self {
            Self::Arn { extension, .. } => extension,
            Self::Name { type_name, .. } => type_name,
        }
    }
}

/// Which version of the identified extension the request selects.
#[derive(Debug, Clone, Copy)]
enum Wanted<'a> {
    /// The record's default version: "Otherwise, it returns information about the default
    /// extension version."
    Default,
    /// The version `VersionId`, or the version an extension-version ARN, names.
    Id(&'a str),
    /// The version a publisher published under this `PublicVersionNumber`.
    PublicNumber(&'a str),
}

impl Wanted<'_> {
    /// What the not-found message names when the extension was found but this version was
    /// not: the version, under the extension the request named.
    fn named_under(&self, extension: &str) -> String {
        match self {
            Self::Default => extension.to_string(),
            Self::Id(version_id) => format!("{extension}/{version_id}"),
            Self::PublicNumber(number) => format!("{extension}/{number}"),
        }
    }
}

/// Read what the request identifies and which version it selects, or `Err(message)` for a
/// combination of members that cannot hold together.
///
/// Every modelled constraint (each member's length and pattern, and `Type`'s enum) is
/// already enforced before the handler runs; what is left are the CONDITIONAL rules the
/// model states in prose.
fn read_request(input: &DescribeTypeInput) -> Result<(Identifier<'_>, Wanted<'_>), String> {
    let identifier = match (
        input.arn.as_deref(),
        input.r#type.as_deref(),
        input.type_name.as_deref(),
    ) {
        (Some(arn), None, None) => {
            let (extension, version_id) = crate::state::registry::split_version_arn(arn);
            Identifier::Arn {
                extension,
                version_id,
            }
        }
        (None, Some(type_kind), Some(type_name)) => Identifier::Name {
            type_kind,
            type_name,
        },
        // Both forms at once: the two identify the extension independently, so a request
        // carrying both states two identities for one read.
        (Some(_), _, _) => {
            return Err(
                "Specify either Arn, or Type and TypeName, but not both: an extension is \
                 identified by its ARN or by its kind and name"
                    .to_string(),
            )
        }
        // Neither form, or half of the name form.
        (None, _, _) => {
            return Err(
                "Either Arn, or both Type and TypeName, must be specified to identify an \
                 extension"
                    .to_string(),
            )
        }
    };

    let pinned_by_arn = match identifier {
        Identifier::Arn { version_id, .. } => version_id,
        Identifier::Name { .. } => None,
    };
    // A version ARN already names a version, so a `VersionId` beside it must be that same
    // version: two different ones select two different records.
    let version_id = match (input.version_id.as_deref(), pinned_by_arn) {
        (Some(named), Some(pinned)) if named != pinned => {
            return Err(format!(
                "VersionId {named} does not name the extension version the Arn does \
                 ({pinned})"
            ))
        }
        (named, pinned) => named.or(pinned),
    };

    let wanted = match (version_id, input.public_version_number.as_deref()) {
        // Two version selectors at once: `VersionId` names a version of the extension in
        // this account, `PublicVersionNumber` names one the publisher published.
        (Some(_), Some(_)) => {
            return Err(
                "Specify either VersionId or PublicVersionNumber, but not both: the two \
                 select the extension version independently"
                    .to_string(),
            )
        }
        (Some(version_id), None) => Wanted::Id(version_id),
        (None, Some(number)) => Wanted::PublicNumber(number),
        (None, None) => Wanted::Default,
    };
    Ok((identifier, wanted))
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> DescribeTypeError {
    DescribeTypeErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The modelled 404: "The specified extension doesn't exist in the CloudFormation
/// registry."
fn type_not_found(identifier: &str) -> DescribeTypeError {
    DescribeTypeErrorModeled::TypeNotFoundException(TypeNotFoundException {
        message: Some(crate::state::extension_does_not_exist(identifier)),
    })
    .into()
}

/// A version's stored logging configuration as the modelled member.
fn logging_config_view(
    config: Option<&crate::state::registry::StoredLoggingConfig>,
) -> Option<LoggingConfig> {
    config.map(|config| LoggingConfig {
        log_role_arn: config.log_role_arn.clone(),
        log_group_name: config.log_group_name.clone(),
    })
}

/// One stored `RequiredActivatedTypes` entry as the modelled member.
fn required_activated_type_view(
    required: &crate::state::registry::StoredRequiredActivatedType,
) -> RequiredActivatedType {
    RequiredActivatedType {
        type_name_alias: required.type_name_alias.clone(),
        original_type_name: required.original_type_name.clone(),
        publisher_id: required.publisher_id.clone(),
        supported_major_versions: crate::state::non_empty(
            required.supported_major_versions.clone(),
        ),
    }
}
