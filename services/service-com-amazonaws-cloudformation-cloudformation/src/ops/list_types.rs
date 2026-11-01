// SPDX-License-Identifier: CC0-1.0

//! @op ListTypes
//! IMPLEMENTED: reports one bounded page of this account and Region's extension registry
//! ([`crate::state::registry`]), narrowed by the request's filters.
//!
//! ## What the operation reports
//!
//! "Returns summary information about all extensions, including your private resource
//! types, modules, and Hooks as well as all public extensions from Amazon Web Services and
//! third-party publishers." So the listing is served from the same two sources every
//! registry read is — the records this account's registry writers persist
//! ([`crate::state::ComAmazonawsCloudformationCloudformationState::extensions_in`]) and
//! the public extensions Amazon Web Services publishes, which are in every account's
//! registry without anybody registering them
//! ([`crate::state::registry::aws_published_all`]: the resource types it provides and the
//! extensions it has PUBLISHED, which are Hooks as well as resource types, so the public
//! catalogue is no more one KIND of extension than the private one is) — and nothing is
//! invented: a request
//! that matches no record answers an empty list rather than a fabricated one.
//!
//! A record is reported at its DEFAULT version, since the request names none: that is the
//! version `DescribeType` describes for the same record, so the summary and the detailed
//! read cannot disagree. Which members a summary carries is gated on the record's
//! visibility, member by member as the model documents each one.
//!
//! ## What narrows the listing
//!
//! Every filter member is independent, and every one the request gives must hold, which is
//! what the model means by "filters must be compatible with Visibility to return valid
//! results. For example, specifying AWS_TYPES for Category and PRIVATE for Visibility
//! returns an empty list of types": the two narrow the same listing, so an incompatible
//! pair narrows it to nothing. `Visibility` defaults to `PRIVATE` and `DeprecatedStatus`
//! to `LIVE` (the defaults the model states for them); `Type`, `ProvisioningType` and each
//! member of `Filters` narrow nothing when absent.
//!
//! ## Ordering and paging
//!
//! `@paginated(inputToken: NextToken, outputToken: NextToken, items: TypeSummaries,
//! pageSize: MaxResults)`: one response carries at most `MaxResults` summaries and the
//! token that resumes just past them. The listing is ordered by type name — and by
//! extension kind where one name is held by two kinds — so the order is a property of the
//! records rather than of the walk, and a caller paging through the collection sees each
//! record exactly once.
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
//! # Modeled contract for `ListTypes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Returns summary information about all extensions, including your private resource types,
//! modules, and Hooks as well as all public extensions from Amazon Web Services and
//! third-party publishers.
//!
//! ## Input
//!
//! * `DeprecatedStatus` (DeprecatedStatus, one of: DEPRECATED | LIVE) — The deprecation
//!   status of the extension that you want to get summary information about. Valid values
//!   include: LIVE: The extension is registered for use in CloudFormation operations.
//!   DEPRECATED: The extension has been deregistered and can no longer be used in
//!   CloudFormation operations.
//! * `Filters` (TypeFilters) — Filter criteria to use in determining which extensions to
//!   return. Filters must be compatible with Visibility to return valid results. For example,
//!   specifying AWS_TYPES for Category and PRIVATE for Visibility returns an empty list of
//!   types, but specifying PUBLIC for Visibility returns the desired list.
//! * `MaxResults` (MaxResults, range 1..100) — The maximum number of results to be returned
//!   with a single call. If the number of available results exceeds this maximum, the
//!   response includes a NextToken value that you can assign to the NextToken request
//!   parameter to get the next set of results.
//! * `NextToken` (NextToken, len 1..1024) — The token for the next set of items to return.
//!   (You received this token from a previous call.)
//! * `ProvisioningType` (ProvisioningType, one of: FULLY_MUTABLE | IMMUTABLE |
//!   NON_PROVISIONABLE) — For resource types, the provisioning behavior of the resource type.
//!   CloudFormation determines the provisioning type during registration, based on the types
//!   of handlers in the schema handler package submitted. Valid values include:
//!   FULLY_MUTABLE: The resource type includes an update handler to process updates to the
//!   type during stack update operations. IMMUTABLE: The resource type doesn't include an
//!   update handler, so the type can't be updated and must instead be replaced during stack
//!   update operations. NON_PROVISIONABLE: The resource type doesn't include create, read,
//!   and delete handlers, and therefore can't actually be provisioned. The default is
//!   FULLY_MUTABLE.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The type of extension.
//! * `Visibility` (Visibility, one of: PRIVATE | PUBLIC) — The scope at which the extensions
//!   are visible and usable in CloudFormation operations. Valid values include: PRIVATE:
//!   Extensions that are visible and usable within this account and Region. This includes:
//!   Private extensions you have registered in this account and Region. Public extensions
//!   that you have activated in this account and Region. PUBLIC: Extensions that are publicly
//!   visible and available to be activated within any Amazon Web Services account. This
//!   includes extensions from Amazon Web Services and third-party publishers. The default is
//!   PRIVATE.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 1..1024) — If the request doesn't return all the remaining
//!   results, NextToken is set to a token. To retrieve the next set of results, call this
//!   action again and assign that token to the request object's NextToken parameter. If the
//!   request returns all results, NextToken is set to null.
//! * `TypeSummaries` (TypeSummaries) — A list of TypeSummary structures that contain
//!   information about the specified extensions.
//!
//! ## Errors
//!
//! * `CFNRegistryException` (HTTP 400) — An error occurred during a CloudFormation registry
//!   operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::registry::{self, StoredExtension, StoredTypeVersion};

/// The model's `Category` enum, "the category of extensions to return"
/// (`smithy_query(query="shape", shape="Category")`: `REGISTERED | ACTIVATED |
/// THIRD_PARTY | AWS_TYPES`).
mod category {
    /// "Private extensions that have been registered for this account and Region."
    pub const REGISTERED: &str = "REGISTERED";
    /// "Public extensions that have been activated for this account and Region."
    pub const ACTIVATED: &str = "ACTIVATED";
    /// "Extensions available for use from publishers other than Amazon. This includes:
    /// private extensions registered in the account; public extensions from publishers
    /// other than Amazon, whether activated or not."
    pub const THIRD_PARTY: &str = "THIRD_PARTY";
    /// "Extensions available for use from Amazon."
    pub const AWS_TYPES: &str = "AWS_TYPES";
}

/// What the request narrows the listing to.
///
/// Each member is read off the request once, and every one that is set must hold for a
/// record to be reported: "filters must be compatible with Visibility to return valid
/// results", so a combination nothing can satisfy reports nothing rather than being
/// refused.
struct Narrowing<'a> {
    /// `Visibility`, "the scope at which the extensions are visible and usable in
    /// CloudFormation operations … The default is PRIVATE".
    visibility: &'a str,
    /// `DeprecatedStatus`, matched against the reported version's own status. The
    /// registry's deprecation filter defaults to `LIVE` — the default the same member
    /// carries on the registry's other version listing (`ListTypeVersions`:
    /// "the default is LIVE") — so a deregistered extension is reported only to a request
    /// that asks for one.
    deprecated_status: &'a str,
    /// `Type`, "the type of extension". Absent, it admits every kind.
    type_kind: Option<&'a str>,
    /// `ProvisioningType`, "for resource types, the provisioning behavior of the resource
    /// type", matched against the reported version's own. Absent, it admits every
    /// behaviour — including the extensions that have none, since a module and a Hook
    /// carry no provisioning behaviour at all.
    provisioning_type: Option<&'a str>,
    /// `Filters$Category`, see [`category`].
    category: Option<&'a str>,
    /// `Filters$PublisherId`, "the publisher ID of the extension publisher": a record some
    /// other publisher published, or one nobody did, is not in the listing it narrows to.
    publisher_id: Option<&'a str>,
    /// `Filters$TypeNamePrefix`, "a prefix to use as a filter for results", matched against
    /// the name this account addresses the extension by.
    type_name_prefix: Option<&'a str>,
}

impl<'a> Narrowing<'a> {
    /// Read the request's filters, with the model's documented defaults for the two that
    /// have one.
    ///
    /// Every modelled constraint (each enum's value set, each filter's length and pattern)
    /// is already enforced before the handler runs, so what is left is which records the
    /// values admit.
    fn read(input: &'a ListTypesInput) -> Self {
        let filters = input.filters.as_ref();
        Self {
            visibility: input
                .visibility
                .as_deref()
                .unwrap_or(registry::visibility::PRIVATE),
            deprecated_status: input
                .deprecated_status
                .as_deref()
                .unwrap_or(registry::deprecated_status::LIVE),
            type_kind: input.r#type.as_deref(),
            provisioning_type: input.provisioning_type.as_deref(),
            category: filters.and_then(|filters| filters.category.as_deref()),
            publisher_id: filters.and_then(|filters| filters.publisher_id.as_deref()),
            type_name_prefix: filters.and_then(|filters| filters.type_name_prefix.as_deref()),
        }
    }

    /// Whether this record, at the version the summary would report, is one the request
    /// asked for.
    fn admits(&self, extension: &StoredExtension, version: &StoredTypeVersion) -> bool {
        extension.visibility == self.visibility
            && version.deprecated_status == self.deprecated_status
            && self
                .type_kind
                .is_none_or(|kind| extension.type_kind == kind)
            && self.provisioning_type.is_none_or(|provisioning| {
                version.provisioning_type.as_deref() == Some(provisioning)
            })
            && self
                .category
                .is_none_or(|category| in_category(extension, category))
            && self
                .publisher_id
                .is_none_or(|publisher| extension.publisher_id.as_deref() == Some(publisher))
            && self
                .type_name_prefix
                .is_none_or(|prefix| extension.type_name.starts_with(prefix))
    }
}

/// Whether a record belongs to one of the model's extension categories.
fn in_category(extension: &StoredExtension, category: &str) -> bool {
    match category {
        // "Private extensions that have been registered for this account and Region."
        category::REGISTERED => extension.is_private(),
        // "Public extensions that have been activated for this account and Region" — the
        // ones an activation put in this registry, which is what `IsActivated` records.
        category::ACTIVATED => extension.is_activated == Some(true),
        // "Extensions available for use from publishers other than Amazon. This includes:
        // private extensions registered in the account. Public extensions from publishers
        // other than Amazon, whether activated or not." A publisher id is exactly what
        // marks the second set: "extensions provided by Amazon Web Services are not
        // assigned a publisher ID".
        category::THIRD_PARTY => extension.is_private() || extension.publisher_id.is_some(),
        // "Extensions available for use from Amazon": the public ones no third party
        // published.
        category::AWS_TYPES => !extension.is_private() && extension.publisher_id.is_none(),
        // The member is a modelled enum, so the boundary validator has already refused
        // anything outside the four values; a value this build does not know narrows to
        // nothing rather than widening the listing.
        _ => false,
    }
}

/// One registry record, at the version a summary reports, as the modelled `TypeSummary`.
///
/// `publisher` is the publisher registration behind the record's `PublisherId`, where this
/// store holds one: the identity a publisher was verified by is a property of that
/// registration rather than of the extension. A record this registry PUBLISHES carries the
/// id of a publisher no call registered, so the publisher behind it is read back from the
/// registry's own catalogue of publishers ([`registry::published_publisher`]) — one rule,
/// two places a publisher record can live.
fn type_summary(
    extension: &StoredExtension,
    version: &StoredTypeVersion,
    publisher: Option<&crate::state::publisher::StoredPublisher>,
) -> TypeSummary {
    let private = extension.is_private();
    let third_party = extension.is_public_third_party();
    let catalogued = extension
        .publisher_id
        .as_deref()
        .and_then(registry::published_publisher);
    TypeSummary {
        r#type: Some(extension.type_kind.clone()),
        // For an extension activated under an alias this is the alias, which is what the
        // record holds: "CloudFormation treats that alias as the extension's type name
        // within the account and Region".
        type_name: Some(extension.type_name.clone()),
        // "The Amazon Resource Name (ARN) of the extension" — the extension's own, not one
        // version's: a summary reports the extension.
        type_arn: Some(extension.arn.clone()),
        // "This applies only to private extensions you have registered in your account.
        // For public extensions, both those provided by Amazon and published by third
        // parties, CloudFormation returns null."
        default_version_id: private
            .then(|| extension.default_version_id.clone())
            .flatten(),
        // "When the specified extension version was registered. This applies only to:
        // private extensions you have registered in your account; public extensions you
        // have activated in your account with auto-update specified. For all other
        // extension types, CloudFormation returns null." A record a PUBLISHER published is
        // reported with that instant too, whether or not this account activated it: the
        // recorded public listing carries `LastUpdated` on a public Hook whose
        // `IsActivated` is false, so a record with a publisher behind it reports when the
        // version it is held at entered the registry.
        last_updated: (private || extension.auto_update == Some(true) || third_party)
            .then_some(version.last_updated)
            .flatten()
            .map(crate::state::timestamp),
        description: version.description.clone(),
        // "The ID of the extension publisher, if the extension is published by a third
        // party. Extensions published by Amazon don't return a publisher ID."
        publisher_id: third_party
            .then(|| extension.publisher_id.clone())
            .flatten(),
        // "The type of account used as the identity provider when registering this
        // publisher with CloudFormation", which the publisher's own record is what carries:
        // the registration this account wrote, or the catalogue's record of the publisher
        // whose extensions this registry publishes.
        publisher_identity: third_party
            .then(|| {
                publisher
                    .and_then(|publisher| publisher.identity_provider())
                    .or(catalogued.map(|catalogued| catalogued.identity_provider))
            })
            .flatten()
            .map(str::to_string),
        // "The publisher name, as defined in the public profile for that publisher in the
        // service used to verify the publisher identity": part of the publisher's record
        // too, which a `RegisterPublisher` registration does not carry and the catalogue
        // does.
        publisher_name: third_party
            .then(|| catalogued.map(|catalogued| catalogued.publisher_name.to_string()))
            .flatten(),
        // "For public extensions that have been activated for this account and Region, the
        // type name of the public extension."
        original_type_name: (!private)
            .then(|| extension.original_type_name.clone())
            .flatten(),
        // "For public extensions that have been activated for this account and Region, the
        // version of the public extension to be used for CloudFormation operations in this
        // account and Region."
        public_version_number: (!private)
            .then(|| version.public_version_number.clone())
            .flatten(),
        // "For public extensions that have been activated for this account and Region, the
        // latest version of the public extension that is available." Reported for a public
        // third-party record whether or not this account activated it, for the same reason
        // `LastUpdated` is: the recorded public listing carries `LatestPublicVersion` on a
        // public Hook whose `IsActivated` is false. A public record with no publisher
        // behind it was published by nobody, so it has no latest published version to
        // report.
        latest_public_version: third_party
            .then(|| extension.latest_public_version.clone())
            .flatten(),
        // "Whether the extension is activated for this account and Region. This applies
        // only to third-party public extensions."
        is_activated: third_party.then_some(extension.is_activated).flatten(),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTypesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#ListTypes")]
#[derive(Debug)]
pub enum ListTypesErrorModeled {
    CFNRegistryException(CFNRegistryException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_types::ListTypesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTypesError = crate::scaffolded::Scaffolded<ListTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTypesErrorModeled> for ListTypesError {
    fn from(modeled: ListTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTypes<Eng>
{
    type Input = ListTypesInput;
    type Output = ListTypesOutput;
    type Error = ListTypesError;

    const NAME: &'static str = "ListTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to list, so this
            // answers 501 rather than an empty, truthful-looking list.
            return Err(crate::scaffolded::Scaffolded::not_implemented("ListTypes"));
        };
        let region = crate::state::request_region(ctx.parts());
        let narrowing = Narrowing::read(&input);
        // A pure report: nothing here writes. There is no `settle` call either, because a
        // registry record carries no status the clock owes — the stack clock settles
        // stacks and change sets, neither of which this read reports.
        let store = state.lock();

        // Both sources of the registry, in one list: the records this account's registry
        // writers persisted, and the public extensions Amazon Web Services publishes,
        // which are in every account's registry without anybody registering them. A
        // published name this account's registry already holds a record for is reported
        // once, from the stored record — `RegisterType` reserves the `AWS` namespace, so
        // the two cannot disagree about one extension.
        let mut records: Vec<StoredExtension> = store.extensions_in(&region).cloned().collect();
        for published in registry::aws_published_all(&region) {
            let already_held = records.iter().any(|stored| {
                stored.type_kind == published.type_kind && stored.type_name == published.type_name
            });
            if !already_held {
                records.push(published);
            }
        }
        // Ordered by what identifies an extension in this registry — its name, and its
        // kind where one name is held by two kinds — so the order is a property of the
        // records rather than of the walk, and the pages of one collection partition it.
        records.sort_by(|left, right| {
            (&left.type_name, &left.type_kind).cmp(&(&right.type_name, &right.type_kind))
        });

        let summaries: Vec<TypeSummary> = records
            .iter()
            .filter_map(|extension| {
                // The version a summary reports is the record's default version, since the
                // request names none. A record holding no version has nothing registered
                // under it, so the listing passes over it rather than reporting an
                // extension with no registered version behind it.
                let version = extension.default_version()?;
                let publisher = extension
                    .publisher_id
                    .as_deref()
                    .and_then(|publisher_id| store.publisher(Some(publisher_id)));
                narrowing
                    .admits(extension, version)
                    .then(|| type_summary(extension, version, publisher))
            })
            .collect();

        // `MaxResults` is "the maximum number of results to be returned with a single
        // call", and the modelled range (1..100) is already enforced before this handler
        // runs.
        let max_results = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok());
        let (type_summaries, next_token) = crate::state::paginate_at_most(
            crate::state::PageOf::TypeSummaries,
            summaries,
            input.next_token.as_deref(),
            max_results,
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        Ok(ListTypesOutput {
            type_summaries: Some(type_summaries),
            next_token,
        })
    }
}
