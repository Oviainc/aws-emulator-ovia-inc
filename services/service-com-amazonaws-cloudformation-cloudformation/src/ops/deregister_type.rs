// SPDX-License-Identifier: CC0-1.0

//! @op DeregisterType
//! Deregisters ONE record of this account and Region's extension registry
//! ([`crate::state::registry`]), or one version of it: what the request identifies is
//! marked `DEPRECATED` and leaves active use.
//!
//! ## What the operation does
//!
//! "Marks an extension or extension version as DEPRECATED in the CloudFormation registry,
//! removing it from active use. Deprecated extensions or extension versions cannot be used
//! in CloudFormation operations." So the call does two things, and this handler keeps them
//! separate:
//!
//! 1. it IDENTIFIES a registration — by `Arn`, the ARN of the extension (a `PrivateTypeArn`,
//!    which carries this account's id, so it addresses a record this account registered),
//!    or by `Type` and `TypeName` ("Conditional: You must specify either TypeName and Type,
//!    or Arn"), in the Region the request was served in: the registry is regional, so an ARN
//!    minted for another Region names nothing here. `VersionId` — or the version segment an
//!    extension-version ARN carries — narrows the call to ONE version of that record;
//! 2. it DEPRECATES what was identified
//!    ([`crate::state::registry::StoredTypeVersion::deprecate`] for one version,
//!    [`crate::state::registry::StoredExtension::deprecate_every_version`] for the extension
//!    itself). The two forms divide the work between them, and the message each refusal
//!    carries is what states that division:
//!
//!    * a request naming a VERSION deregisters that version, which has to be one in active
//!      use and not the record's DEFAULT version — the default version is the one "used when
//!      the extension version isn't specified", so it is the version in use, and the refusal
//!      names the remedy: deregister the extension instead;
//!    * a request naming NO version deregisters the extension, which is every version of it
//!      at once, and it can do that only once no more than ONE version is left in active use:
//!      "to deregister an entire extension, you must individually deregister all active
//!      versions of that extension". A record with no version left in active use is the
//!      extension "itself being deregistered and marked as deprecated in the registry".
//!
//!    Both refusals are the registry's own modelled `CFNRegistryException` (400), writing
//!    nothing.
//!
//!    Which form takes the DEFAULT version out of use is recorded behavior, and it is the
//!    extension form: a deregistration of the default version is refused whether or not
//!    another version is still in active use, and it is refused with the sentence that sends
//!    the caller to the extension form. That form is what the two sentences the operation
//!    documents describe the outcome of — "if an extension has only a single active version,
//!    deregistering that version results in the extension itself being deregistered and
//!    marked as deprecated in the registry", and "if you do deregister the default version of
//!    an extension, the extension type itself is deregistered as well and marked as
//!    deprecated": deregistering the extension deprecates every version of it, the default
//!    one included, so the documented outcome is exactly what the extension form leaves
//!    behind. What is NOT available is reaching that outcome through the version form, which
//!    is what the recorded refusal of a default version says.
//!
//! What this operation can name is exactly the records that are PRIVATE registrations of this
//! account and Region still in active use
//! ([`crate::state::registry::StoredExtension::is_live`]): a public extension — whether Amazon
//! Web Services published it or a third party published and this account activated it, which
//! `DeactivateType` is what removes from use — a record another Region's registry holds, a
//! registration every version of which is already deprecated, and a name or an ARN no record
//! carries are all outside it, and each is the modelled `TypeNotFoundException` (404, "the
//! specified extension doesn't exist in the CloudFormation registry"); so is a version id the
//! resolved record holds no version in active use under. A request that identifies no
//! extension at all is refused with `CFNRegistryException` (400), writing nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
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
//! # Modeled contract for `DeregisterType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Marks an extension or extension version as DEPRECATED in the CloudFormation registry,
//! removing it from active use. Deprecated extensions or extension versions cannot be used
//! in CloudFormation operations. To deregister an entire extension, you must individually
//! deregister all active versions of that extension. If an extension has only a single
//! active version, deregistering that version results in the extension itself being
//! deregistered and marked as deprecated in the registry. You can't deregister the default
//! version of an extension if there are other active version of that extension. If you do
//! deregister the default version of an extension, the extension type itself is
//! deregistered as well and marked as deprecated. To view the deprecation status of an
//! extension or extension version, use DescribeType. For more information, see Remove
//! third-party private extensions from your account in the CloudFormation User Guide.
//!
//! ## Input
//!
//! * `Arn` (PrivateTypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$) — The
//!   Amazon Resource Name (ARN) of the extension. Conditional: You must specify either
//!   TypeName and Type, or Arn.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The kind of extension.
//!   Conditional: You must specify either TypeName and Type, or Arn.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension. Conditional: You must specify either TypeName and Type, or Arn.
//! * `VersionId` (TypeVersionId, len 1..128, pattern ^[A-Za-z0-9-]+$) — The ID of a specific
//!   version of the extension. The version ID is the value at the end of the Amazon Resource
//!   Name (ARN) assigned to the extension version when it is registered.
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
// `DeregisterTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeregisterTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#DeregisterType")]
#[derive(Debug)]
pub enum DeregisterTypeErrorModeled {
    CFNRegistryException(CFNRegistryException),
    TypeNotFoundException(TypeNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::deregister_type::DeregisterTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeregisterTypeError = crate::scaffolded::Scaffolded<DeregisterTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeregisterTypeErrorModeled> for DeregisterTypeError {
    fn from(modeled: DeregisterTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeregisterType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeregisterType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeregisterType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeregisterType<Eng>
{
    type Input = DeregisterTypeInput;
    type Output = DeregisterTypeOutput;
    type Error = DeregisterTypeError;

    const NAME: &'static str = "DeregisterType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // What the request identifies, and which version of it it names. A combination of
        // members that cannot hold together is refused with the registry's own modelled 400
        // before any state is touched.
        let (identifier, wanted) = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to deregister an
            // extension from, so this answers 501 rather than a deprecation nothing recorded.
            return Err(crate::scaffolded::Scaffolded::not_implemented("DeregisterType"));
        };
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // The settle walk every operation makes first, so this call reads and writes a
        // registry in which every transition the clock had already made due is installed.
        store.settle(::std::time::SystemTime::now());

        // The REGISTRATION the request names, in the Region the request was served in. Only
        // a private record of this account and Region still in active use can be
        // deregistered, so both predicates are part of resolving the record rather than a
        // second refusal after it: anything else is not an extension this operation can name.
        //
        // The name form resolves the registration directly, so a name a publication has also
        // written a public record for still resolves to the record this account registered.
        let arn = match identifier {
            Identifier::Arn { extension, .. } => store.extension_by_arn(&region, extension),
            Identifier::Name {
                type_kind,
                type_name,
            } => store.registered_extension_by_name(&region, type_kind, type_name),
        }
        .filter(|extension| extension.is_private() && extension.is_live())
        .map(|extension| extension.arn.clone())
        .ok_or_else(|| type_not_found(identifier.named()))?;

        let Some(extension) = store.extension_by_arn_mut(&region, &arn) else {
            // Unreachable: the record was resolved just above, under the same lock. Answered
            // as the modelled 404 rather than with a panic, which would leave the shared
            // store poisoned for every other caller.
            return Err(type_not_found(identifier.named()));
        };

        let version_id = match wanted {
            // "To deregister an entire extension, you must individually deregister all active
            // versions of that extension": the whole extension can be deregistered only once
            // it holds no more than ONE version in active use — every other version has to go
            // individually first — and that last one leaving use is what leaves the extension
            // itself deregistered.
            Wanted::Extension => {
                if extension.live_versions().count() > 1 {
                    return Err(registry_exception(
                        MORE_THAN_ONE_ACTIVE_VERSION.to_string(),
                    ));
                }
                extension.deprecate_every_version();
                return Ok(DeregisterTypeOutput {});
            }
            Wanted::Version(version_id) => version_id,
        };

        // The version the request names, which has to be one the record holds in active use:
        // a version already deprecated is already out of use, so there is none of it left to
        // deregister.
        if extension
            .version(version_id)
            .is_none_or(|version| !version.is_live())
        {
            return Err(type_not_found(&wanted.named_under(identifier.named())));
        }
        // The version form deregisters a NON-DEFAULT version: the default version is the one
        // "used when the extension version isn't specified", so it is the version in use and
        // the refusal states the remedy — deregister the extension instead, which is the form
        // that takes its last version out of use. Recorded behavior: a deregistration of the
        // default version is refused whether or not another version is still in active use, so
        // this precondition is not a count of the other versions.
        if extension.is_default_version(version_id) {
            let type_name = extension.type_name.clone();
            return Err(registry_exception(cannot_deregister_default_version(
                version_id,
                &type_name,
            )));
        }

        let Some(version) = extension.version_mut(version_id) else {
            // Unreachable for the same reason as above: resolved under this lock.
            return Err(type_not_found(&wanted.named_under(identifier.named())));
        };
        version.deprecate();

        Ok(DeregisterTypeOutput {})
    }
}

/// How a request identifies the extension to deregister.
///
/// The model states the constraint across three members at once — "Conditional: You must
/// specify either TypeName and Type, or Arn" — so a request carries one form or the other,
/// never a mix and never neither.
#[derive(Debug, Clone, Copy)]
enum Identifier<'a> {
    /// `Arn`, "the Amazon Resource Name (ARN) of the extension", which addresses the record
    /// on its own; an extension-version ARN pins the version too.
    Arn {
        extension: &'a str,
        version_id: Option<&'a str>,
    },
    /// `Type` and `TypeName`, resolved in the Region the request was served in.
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

/// What the request deregisters: the extension, or one version of it.
#[derive(Debug, Clone, Copy)]
enum Wanted<'a> {
    /// The extension itself, which a request naming no version deregisters.
    Extension,
    /// The version `VersionId`, or the version an extension-version ARN, names: "the ID of a
    /// specific version of the extension".
    Version(&'a str),
}

impl Wanted<'_> {
    /// What the not-found message names when the extension was found but this version was
    /// not: the version, under the extension the request named.
    fn named_under(&self, extension: &str) -> String {
        match self {
            Self::Extension => extension.to_string(),
            Self::Version(version_id) => format!("{extension}/{version_id}"),
        }
    }
}

/// Read what the request identifies and which version it names, or `Err(message)` for a
/// combination of members that cannot hold together.
///
/// Every modelled constraint (each member's length and pattern, and `Type`'s enum) is
/// already enforced before the handler runs; what is left are the CONDITIONAL rules the
/// model states in prose — the same ones `DescribeType` reads, since the two identify an
/// extension and select a version of it by the same members.
fn read_request(input: &DeregisterTypeInput) -> Result<(Identifier<'_>, Wanted<'_>), String> {
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
        // carrying both states two identities for one deregistration.
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
    // version: two different ones name two different versions for one deregistration.
    let wanted = match (input.version_id.as_deref(), pinned_by_arn) {
        (Some(named), Some(pinned)) if named != pinned => {
            return Err(format!(
                "VersionId {named} does not name the extension version the Arn does \
                 ({pinned})"
            ))
        }
        (named, pinned) => match named.or(pinned) {
            Some(version_id) => Wanted::Version(version_id),
            None => Wanted::Extension,
        },
    };
    Ok((identifier, wanted))
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> DeregisterTypeError {
    DeregisterTypeErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The message answered a request that would take the record's DEFAULT version out of active
/// use while other versions are still in it, which is the rule `DeregisterType` states: the
/// default version is the one a request naming none resolves to, so it is the last version of
/// the record to leave use.
///
/// Fixed text, with exactly two variable parts: the `VersionId` of the version the request
/// named, and the name of the extension it is a version of — the remedy the sentence states,
/// since deregistering the extension is what takes its default version out of use.
///
/// The noun is part of the FIXED text, not of the extension's kind: the recorded refusal
/// words it "the resource type" for a type name that ends in `::MODULE`, so it is the same
/// sentence whatever kind the record is.
fn cannot_deregister_default_version(version_id: &str, type_name: &str) -> String {
    format!(
        "Version '{version_id}' is the default version and cannot be deregistered. Deregister \
         the resource type '{type_name}' instead."
    )
}

/// The message answered a request to deregister the WHOLE extension while more than one
/// version of it is still in active use: the rule `DeregisterType` states as "to deregister an
/// entire extension, you must individually deregister all active versions of that extension".
///
/// Entirely fixed text: no part of it comes from the request or from the store.
const MORE_THAN_ONE_ACTIVE_VERSION: &str =
    "This type has more than one active version. Please deregister non-default active versions \
     before attempting to deregister the type.";

/// The modelled 404: "The specified extension doesn't exist in the CloudFormation
/// registry."
fn type_not_found(identifier: &str) -> DeregisterTypeError {
    DeregisterTypeErrorModeled::TypeNotFoundException(TypeNotFoundException {
        message: Some(crate::state::extension_does_not_exist(identifier)),
    })
    .into()
}
