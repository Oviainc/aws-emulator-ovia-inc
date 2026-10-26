// SPDX-License-Identifier: CC0-1.0

//! @op DeactivateType
//! Deactivates one ACTIVATION in this account and Region's extension registry
//! ([`crate::state::registry`]): the public third-party extension the request identifies
//! stops being activated here.
//!
//! ## What the operation does
//!
//! "Deactivates a public third-party extension, such as a resource or module, or a
//! CloudFormation Hook when you no longer use it. Deactivating an extension deletes the
//! configuration details that are associated with it... Once deactivated, an extension
//! can't be used in any CloudFormation operation... In addition, deactivated extensions
//! aren't automatically updated if a new version of the extension is released." So the call
//! does two things, and this handler keeps them separate:
//!
//! 1. it IDENTIFIES an activation — by `Arn`, the ARN "for the extension in this account and
//!    Region", or by `Type` and `TypeName` ("Conditional: You must specify either Arn, or
//!    TypeName and Type"), in the Region the request was served in: the registry is
//!    regional, so an ARN minted for another Region names nothing here;
//! 2. it WRITES the deactivation onto that record
//!    ([`crate::state::registry::StoredExtension::deactivate`]): the configuration data goes,
//!    `IsActivated` and `AutoUpdate` become false, and every version of the extension becomes
//!    `DEPRECATED` — the status that states the extension "has been deactivated or
//!    deregistered and can no longer be used in CloudFormation operations".
//!
//! What this operation can name is exactly the records that are activated public third-party
//! extensions of this account and Region
//! ([`crate::state::registry::StoredExtension::is_activated_third_party`]): a private
//! registration, an extension Amazon Web Services published (nobody activated those) and an
//! activation that is already deactivated are all outside it, and so is a name or an ARN no
//! record carries — each is the modelled `TypeNotFoundException` (404, "the specified
//! extension doesn't exist in the CloudFormation registry"). A request that identifies no
//! extension at all is refused with the registry's own modelled `CFNRegistryException`
//! (400), writing nothing.
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
//! # Modeled contract for `DeactivateType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Deactivates a public third-party extension, such as a resource or module, or a
//! CloudFormation Hook when you no longer use it. Deactivating an extension deletes the
//! configuration details that are associated with it. To temporarily disable a
//! CloudFormation Hook instead, you can use SetTypeConfiguration. Once deactivated, an
//! extension can't be used in any CloudFormation operation. This includes stack update
//! operations where the stack template includes the extension, even if no updates are being
//! made to the extension. In addition, deactivated extensions aren't automatically updated
//! if a new version of the extension is released. To see which extensions are currently
//! activated, use ListTypes.
//!
//! ## Input
//!
//! * `Arn` (PrivateTypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$) — The
//!   Amazon Resource Name (ARN) for the extension in this account and Region. Conditional:
//!   You must specify either Arn, or TypeName and Type.
//! * `Type` (ThirdPartyType, one of: HOOK | MODULE | RESOURCE) — The extension type.
//!   Conditional: You must specify either Arn, or TypeName and Type.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The type
//!   name of the extension in this account and Region. If you specified a type name alias
//!   when enabling the extension, use the type name alias. Conditional: You must specify
//!   either Arn, or TypeName and Type.
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
// `DeactivateTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeactivateTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#DeactivateType")]
#[derive(Debug)]
pub enum DeactivateTypeErrorModeled {
    CFNRegistryException(CFNRegistryException),
    TypeNotFoundException(TypeNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::deactivate_type::DeactivateTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeactivateTypeError = crate::scaffolded::Scaffolded<DeactivateTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeactivateTypeErrorModeled> for DeactivateTypeError {
    fn from(modeled: DeactivateTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeactivateType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeactivateType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeactivateType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeactivateType<Eng>
{
    type Input = DeactivateTypeInput;
    type Output = DeactivateTypeOutput;
    type Error = DeactivateTypeError;

    const NAME: &'static str = "DeactivateType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEACTIVATE_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEACTIVATE_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // What the request identifies. A combination of members that cannot hold together is
        // refused with the registry's own modelled 400 before any state is touched.
        let identifier = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to deactivate an
            // extension in, so this answers 501 rather than a deactivation nothing recorded.
            return Err(crate::scaffolded::Scaffolded::not_implemented("DeactivateType"));
        };
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // The settle walk every operation makes first, so this call reads and writes a
        // registry in which every transition the clock had already made due is installed.
        store.settle(::std::time::SystemTime::now());

        // The ACTIVATION the request names, in the Region the request was served in. Only an
        // activated public third-party extension can be deactivated, so that predicate is
        // part of resolving the record rather than a second refusal after it: anything else
        // is not an extension this operation can name.
        //
        // An `Arn` is matched whole. It is "the Amazon Resource Name (ARN) for the extension
        // in this account and Region" — the extension's own ARN, which carries no version
        // segment, so an ARN that names a version names no record here.
        let arn = store
            .extensions_in(&region)
            .find(|extension| {
                extension.is_activated_third_party()
                    && match identifier {
                        Identifier::Arn(arn) => extension.arn == arn,
                        Identifier::Name {
                            type_kind,
                            type_name,
                        } => extension.type_kind == type_kind && extension.type_name == type_name,
                    }
            })
            .map(|extension| extension.arn.clone())
            .ok_or_else(|| type_not_found(identifier.named()))?;

        let Some(extension) = store.extension_by_arn_mut(&region, &arn) else {
            // Unreachable: the record was resolved just above, under the same lock. Answered
            // as the modelled 404 rather than with a panic, which would leave the shared
            // store poisoned for every other caller.
            return Err(type_not_found(identifier.named()));
        };
        extension.deactivate();

        Ok(DeactivateTypeOutput {})
    }
}

/// How a request identifies the activation to deactivate.
///
/// The model states the constraint across three members at once — "Conditional: You must
/// specify either Arn, or TypeName and Type" — so a request carries one form or the other,
/// never a mix and never neither.
#[derive(Debug, Clone, Copy)]
enum Identifier<'a> {
    /// `Arn`, "the Amazon Resource Name (ARN) for the extension in this account and Region",
    /// which addresses the record on its own.
    Arn(&'a str),
    /// `Type` and `TypeName`, resolved in the Region the request was served in. For an
    /// extension activated under an alias that is the alias, which is the name this account
    /// addresses the extension by.
    Name {
        type_kind: &'a str,
        type_name: &'a str,
    },
}

impl Identifier<'_> {
    /// What the request named the extension by, for the not-found message.
    fn named(&self) -> &str {
        match self {
            Self::Arn(arn) => arn,
            Self::Name { type_name, .. } => type_name,
        }
    }
}

/// Read what the request identifies, or `Err(message)` for a combination of members that
/// cannot hold together.
///
/// Every modelled constraint (each member's length and pattern, and `Type`'s enum) is
/// already enforced before the handler runs; what is left are the CONDITIONAL rules the
/// model states in prose — the same ones `DescribeType` reads, since the two identify an
/// extension by the same members.
fn read_request(input: &DeactivateTypeInput) -> Result<Identifier<'_>, String> {
    match (
        input.arn.as_deref(),
        input.r#type.as_deref(),
        input.type_name.as_deref(),
    ) {
        (Some(arn), None, None) => Ok(Identifier::Arn(arn)),
        (None, Some(type_kind), Some(type_name)) => Ok(Identifier::Name {
            type_kind,
            type_name,
        }),
        // Both forms at once: the two identify the extension independently, so a request
        // carrying both states two identities for one deactivation.
        (Some(_), _, _) => Err(
            "Specify either Arn, or Type and TypeName, but not both: an extension is \
             identified by its ARN or by its kind and name"
                .to_string(),
        ),
        // Neither form, or half of the name form.
        (None, _, _) => Err(
            "Either Arn, or both Type and TypeName, must be specified to identify an \
             extension"
                .to_string(),
        ),
    }
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> DeactivateTypeError {
    DeactivateTypeErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The modelled 404: "The specified extension doesn't exist in the CloudFormation
/// registry."
fn type_not_found(identifier: &str) -> DeactivateTypeError {
    DeactivateTypeErrorModeled::TypeNotFoundException(TypeNotFoundException {
        message: Some(crate::state::extension_does_not_exist(identifier)),
    })
    .into()
}
