// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAddonConfiguration
//! IMPLEMENTED. Answers one entry of the add-on catalog this service's state
//! holds (`crate::state::AddonCatalogRecord`): the configuration schema the
//! named build is installed with, and the Kubernetes service accounts it runs as
//! with the managed policies Amazon EKS recommends for each.
//!
//! This is a REPORTING operation: it writes nothing. Unlike
//! `DescribeAddonVersions`, which reports a catalog and narrows it, this one
//! RESOLVES one `(addonName, addonVersion)` pair — both members are `@required`
//! — so a pair the catalog does not publish is refused rather than answered
//! with an empty document. BOTH halves are the same refusal,
//! `ResourceNotFoundException` (404): the pair names one published build, so an
//! add-on name the catalog does not carry and a version it publishes no build of
//! are the same miss — the schema asked about does not exist — and the sentence
//! states the pair the request named
//! (`crate::state::addon_schema_not_found_message`).
//!
//! The schema is the one the catalog holds for that build: the add-on's OWN
//! configurable keys, each with the JSON type it takes, beside the namespace the
//! add-on is installed into and the service account it runs as, and nothing else
//! is allowed in a configuration document. Its `required` list is the build's own
//! required configuration — the same thing that makes
//! `AddonVersionInfo$requiresConfiguration` true for that build, so the flag
//! `DescribeAddonVersions` publishes and the schema this operation publishes
//! cannot disagree — and it is the same document `CreateAddon` validates
//! `configurationValues` against.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAddonConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns configuration options.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by DescribeAddonVersions.
//! * `addonVersion` (String) **required** — The version of the add-on. The version must match
//!   one of the versions returned by DescribeAddonVersions .
//!
//! ## Output
//!
//! * `addonName` (String) — The name of the add-on.
//! * `addonVersion` (String) — The version of the add-on. The version must match one of the
//!   versions returned by DescribeAddonVersions .
//! * `configurationSchema` (String) — A JSON schema that's used to validate the configuration
//!   values you provide when an add-on is created or updated.
//! * `podIdentityConfiguration` (AddonPodIdentityConfigurationList) — The Kubernetes service
//!   account name used by the add-on, and any suggested IAM policies. Use this information to
//!   create an IAM Role for the add-on.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAddonConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeAddonConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeAddonConfiguration")]
#[derive(Debug)]
pub enum DescribeAddonConfigurationErrorModeled {
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_addon_configuration::DescribeAddonConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeAddonConfigurationError = crate::scaffolded::Scaffolded<DescribeAddonConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeAddonConfigurationErrorModeled> for DescribeAddonConfigurationError {
    fn from(modeled: DescribeAddonConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAddonConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAddonConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAddonConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAddonConfiguration<Eng>
{
    type Input = DescribeAddonConfigurationInput;
    type Output = DescribeAddonConfigurationOutput;
    type Error = DescribeAddonConfigurationError;

    const NAME: &'static str = "DescribeAddonConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ADDON_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeAddonConfigurationError::from(
                DescribeAddonConfigurationErrorModeled::ServerException(ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        // The catalog is GLOBAL: an add-on version is published for the service, not
        // per Region or per account, so nothing here reads a region or an account
        // from the request. Which builds are published is a function of the clock
        // (see `crate::state::AddonCatalogVersionRecord::is_published`).
        let now = SystemTime::now();

        let guard = store.lock();
        // The pair is ONE lookup: the add-on must be published and it must have the
        // build named for it, and whichever half misses, what the caller asked about
        // does not exist.
        let addon = guard
            .published_addon(&input.addon_name, now)
            .ok_or_else(|| not_found(&input.addon_name, &input.addon_version))?;
        let version = addon
            .published_versions(now)
            .find(|version| version.addon_version == input.addon_version)
            .ok_or_else(|| not_found(&input.addon_name, &input.addon_version))?;

        Ok(DescribeAddonConfigurationOutput {
            addon_name: Some(addon.addon_name.clone()),
            addon_version: Some(version.addon_version.clone()),
            configuration_schema: Some(addon.configuration_schema(version)),
            pod_identity_configuration: Some(addon.to_pod_identity_configuration()),
        })
    }
}

/// Refuse a `(addonName, addonVersion)` pair the catalog publishes no build of.
///
/// `ResourceNotFoundException` (404) is the modelled "The specified resource
/// could not be found" channel, and the sentence is this operation's own: the
/// SCHEMA asked for is what was not found, stated with the pair the request's
/// query string named (`crate::state::addon_schema_not_found`).
fn not_found(addon_name: &str, addon_version: &str) -> DescribeAddonConfigurationError {
    DescribeAddonConfigurationErrorModeled::ResourceNotFoundException(
        state::addon_schema_not_found(addon_name, addon_version),
    )
    .into()
}

