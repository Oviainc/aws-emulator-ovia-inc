// SPDX-License-Identifier: CC0-1.0

//! @op PublishType
//! Publishes the DEFAULT version of a private extension this account and Region registered
//! as a PUBLIC extension of that Region, and hands back the ARN the public extension is
//! addressed by.
//!
//! ## What this operation does
//!
//! "Publishes the specified extension to the CloudFormation registry as a public extension
//! in this Region. Public extensions are available for use by all CloudFormation users... To
//! publish an extension, you must be registered as a publisher with CloudFormation."
//!
//! So the call WRITES a registry record: the public extension
//! ([`crate::state::registry::published_record`]), carrying the version it published and the
//! public version number that version was published under, which `DescribeType` is what
//! reads back. The private registration it was published from is left exactly as it was —
//! a public extension is a record of its own, visible "within any Amazon Web Services
//! account", which a private registration is not.
//!
//! Five things gate it, in this order:
//!
//! 1. WHAT the request identifies: "you must specify Arn, or TypeName and Type", so a
//!    request carrying both forms, neither, or half of the name form identifies no extension
//!    and is refused with the registry's modelled `CFNRegistryException` (400). This is read
//!    BEFORE anything is resolved, because it is a property of the request alone.
//! 2. The VERSION an `Arn` OF THIS ACCOUNT pins past the extension it addresses: what a
//!    publication publishes is the extension's default version, never a version the request
//!    picked, so such an `Arn` is refused with the modelled 400 carrying
//!    [`ONLY_DEFAULT_VERSION_CAN_BE_PUBLISHED`]. CloudFormation parses an ARN of the caller's
//!    own account itself ([`crate::state::registry::arn_names_this_account`]), so this is read
//!    from the ARN alone, before the caller is judged and before the registry is consulted; an
//!    ARN whose account segment is
//!    another account's addresses nothing of this account and is left to the account-scoped
//!    refusal below, which is the only answer it can reach.
//! 3. The CALLER: "to publish an extension, you must be
//!    registered as a publisher with CloudFormation" ([`crate::state::publisher`], which
//!    `RegisterPublisher` writes), and the publisher id that registration carries is the one
//!    the published extension belongs to. Whether the calling account may publish at all is
//!    judged BEFORE the registry is consulted, so an account holding no verified registration
//!    is refused with the modelled 400 carrying [`not_a_verified_publisher`] whether or not a
//!    record answers to what the request identifies, and nothing about this account's registry
//!    reaches a caller that gate is shut for.
//! 4. WHICH record the request names, resolved against the registry of THIS account and
//!    Region — an `Arn` by the extension it addresses, and the name form by kind and name. An
//!    identifier that resolves to no record of this account and Region is refused with the
//!    modelled 400 carrying [`NOT_FOUND_IN_YOUR_ACCOUNT`]: existence and ownership are what a
//!    publication resolves next, so an unregistered extension is never reported as a malformed
//!    request. A record the registry holds but this account did not REGISTER — a public
//!    extension, whether provided by Amazon Web Services or published by a third party and
//!    activated here — is not a private extension to publish, so it too is refused with the
//!    modelled 400; `Arn` is a `PrivateTypeArn` for that same reason.
//! 5. The DEFAULT VERSION and the VERSION NUMBER: the record's default version, "used when
//!    the extension version isn't specified". A record carrying none is the modelled
//!    `TypeNotFoundException` (404), and one whose default version's contract tests have not
//!    concluded `PASSED` is refused with the modelled 400 — "an extension must have a test
//!    status of PASSED before it can be published", which `TestType` is what reaches. Then
//!    the version number: "you cannot specify a version number the first time you publish a
//!    type. CloudFormation automatically sets the first version number to be 1.0.0", so a
//!    first publication carrying one is refused and one carrying none is assigned that
//!    number; afterwards "if you don't specify a version number, CloudFormation increments
//!    the version number by one minor version release"
//!    ([`crate::state::registry::next_minor_version_number`]), and a number the publisher has
//!    already published another version of this extension under is refused — the registry
//!    addresses a public version BY that number, so two versions cannot share one.
//!
//! Publishing the same version again publishes it as the NEXT public version of the
//! extension, which is what "CloudFormation increments the version number by one minor
//! version release" states: every version a publisher has published stays addressable by
//! the number it was published under, so a number already published is refused rather than
//! overwritten.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so engine
//! access is guaranteed by the time any request runs. Reach the store through
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
//! # Modeled contract for `PublishType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Publishes the specified extension to the CloudFormation registry as a public extension
//! in this Region. Public extensions are available for use by all CloudFormation users. For
//! more information about publishing extensions, see Publishing extensions to make them
//! available for public use in the CloudFormation Command Line Interface (CLI) User Guide.
//! To publish an extension, you must be registered as a publisher with CloudFormation. For
//! more information, see RegisterPublisher.
//!
//! ## Input
//!
//! * `Arn` (PrivateTypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$) — The
//!   Amazon Resource Name (ARN) of the extension. Conditional: You must specify Arn, or
//!   TypeName and Type.
//! * `PublicVersionNumber` (PublicVersionNumber, len 5.., pattern
//!   ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$) — The version number to assign to this version of
//!   the extension. Use the following format, and adhere to semantic versioning when
//!   assigning a version number to your extension: MAJOR.MINOR.PATCH For more information,
//!   see Semantic Versioning 2.0.0. If you don't specify a version number, CloudFormation
//!   increments the version number by one minor version release. You cannot specify a version
//!   number the first time you publish a type. CloudFormation automatically sets the first
//!   version number to be 1.0.0.
//! * `Type` (ThirdPartyType, one of: HOOK | MODULE | RESOURCE) — The type of the extension.
//!   Conditional: You must specify Arn, or TypeName and Type.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension. Conditional: You must specify Arn, or TypeName and Type.
//!
//! ## Output
//!
//! * `PublicTypeArn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) assigned to the public extension upon publication.
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
// `PublishTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PublishTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#PublishType")]
#[derive(Debug)]
pub enum PublishTypeErrorModeled {
    CFNRegistryException(CFNRegistryException),
    TypeNotFoundException(TypeNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::publish_type::PublishTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PublishTypeError = crate::scaffolded::Scaffolded<PublishTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PublishTypeErrorModeled> for PublishTypeError {
    fn from(modeled: PublishTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PublishType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PublishType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PublishType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PublishType<Eng>
{
    type Input = PublishTypeInput;
    type Output = PublishTypeOutput;
    type Error = PublishTypeError;

    const NAME: &'static str = "PublishType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUBLISH_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUBLISH_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // What the request identifies, judged before anything is resolved: it is a property
        // of the request alone.
        let identifier = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to publish an extension
            // into, so this answers 501 rather than an ARN nothing stands behind.
            return Err(crate::scaffolded::Scaffolded::not_implemented("PublishType"));
        };
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // Settled FIRST, so a version whose contract tests the clock has already concluded
        // is published — or refused — on that conclusion rather than on a stale
        // `IN_PROGRESS`.
        store.settle(now);

        // The version an `Arn` of THIS account pins, read from the ARN alone: an ARN carrying
        // the caller's own account id is one CloudFormation parses itself, and its resource
        // portion addresses the extension — `type/<kind>/<Type-Name>` — so a further segment
        // pins a version, and what a publication publishes is the extension's default
        // version. An ARN of ANOTHER account addresses nothing of this account, so it is not
        // read this far: it can only ever reach the account-scoped existence refusal below.
        if let Identifier::Arn {
            extension,
            version_id: Some(_),
        } = identifier
        {
            if crate::state::registry::arn_names_this_account(extension) {
                return Err(registry_exception(
                    ONLY_DEFAULT_VERSION_CAN_BE_PUBLISHED.to_string(),
                ));
            }
        }

        // The CALLER, judged before the registry is consulted: "to publish an extension, you
        // must be registered as a publisher with CloudFormation", so an account holding no
        // verified registration may publish nothing at all and is told that rather than
        // something about a registry it may not publish into — whether or not the extension
        // the request names is registered here. The publisher id the registration carries is
        // the one the publication below belongs to.
        let publisher_id = match store.publisher(None) {
            Some(registered)
                if registered.status == crate::state::publisher::publisher_status::VERIFIED =>
            {
                registered.publisher_id.clone()
            }
            _ => return Err(registry_exception(not_a_verified_publisher())),
        };

        // The record the request names, in the Region the request was signed for, resolved
        // once the caller may publish at all: the registry is regional, so a record another
        // Region holds is not addressable here. An `Arn` resolves by the EXTENSION it
        // addresses, whatever it carries after it. A name a publication has already written a
        // public record for is held by two records at once, so the name form resolves the
        // REGISTRATION first and falls back to whatever else the name holds — which is what
        // the public-extension refusal below is answered from.
        let stored = match identifier {
            Identifier::Arn { extension, .. } => store.extension_by_arn(&region, extension),
            Identifier::Name {
                type_kind,
                type_name,
            } => store
                .registered_extension_by_name(&region, type_kind, type_name)
                .or_else(|| store.extension_by_name(&region, type_kind, type_name)),
        };
        let extension = match stored {
            Some(extension) => extension,
            // The registry also holds the public extensions Amazon Web Services publishes,
            // which are in every account's registry without anybody registering them: one
            // of those EXISTS, so what the call runs into is the public-extension refusal
            // rather than the not-registered one.
            None => {
                let published = match identifier {
                    Identifier::Arn { extension, .. } => {
                        crate::state::registry::aws_published_by_arn(&region, extension)
                    }
                    Identifier::Name {
                        type_kind,
                        type_name,
                    } => crate::state::registry::aws_published(&region, type_kind, type_name),
                };
                return Err(registry_exception(match published {
                    Some(published) => not_a_private_extension(&published.arn),
                    None => NOT_FOUND_IN_YOUR_ACCOUNT.to_string(),
                }));
            }
        };
        if !extension.is_private() {
            return Err(registry_exception(not_a_private_extension(&extension.arn)));
        }

        // The version a publication publishes: the one the record's `DefaultVersionId`
        // names, which is "used when the extension version isn't specified".
        let version = extension
            .default_version()
            .ok_or_else(|| type_not_found(identifier.named()))?;
        // "An extension must have a test status of PASSED before it can be published."
        if version.type_tests_status.as_deref()
            != Some(crate::state::registry::type_tests_status::PASSED)
        {
            return Err(registry_exception(not_tested(
                &version.arn,
                version.type_tests_status.as_deref(),
            )));
        }
        let type_kind = extension.type_kind.clone();
        let type_name = extension.type_name.clone();
        let source = version.clone();

        // What is already published for this extension in this Region decides both whether
        // this caller is the publisher it belongs to and the version number the publication
        // is assigned.
        let already_published = store.published_extension(&region, &type_kind, &type_name);
        if let Some(published) = already_published {
            if published.publisher_id.as_deref() != Some(publisher_id.as_str()) {
                return Err(registry_exception(published_by_another_publisher(
                    &type_name,
                    &published.arn,
                )));
            }
        }
        let public_version_number = assign_version_number(
            input.public_version_number.as_deref(),
            already_published,
            &type_name,
        )?;

        let public_type_arn =
            store.publish_extension_version(crate::state::registry::Publication {
                region: &region,
                type_kind: &type_kind,
                type_name: &type_name,
                publisher_id: &publisher_id,
                version: &source,
                public_version_number: &public_version_number,
                published_at: now,
            });

        Ok(PublishTypeOutput {
            // "The Amazon Resource Name (ARN) assigned to the public extension upon
            // publication", which is the ARN the record just written is addressed by.
            public_type_arn: Some(public_type_arn),
        })
    }
}

/// The refusal AWS answers a request whose `Arn` \u2014 one of this account \u2014 names one VERSION of
/// an extension: what a publication publishes is the extension's DEFAULT version, so the ARN
/// has to name the extension. Entirely fixed text: no part of it comes from the request or
/// from the store.
const ONLY_DEFAULT_VERSION_CAN_BE_PUBLISHED: &str =
    "Only default version can be published. Please remove versionId from Arn";

/// The refusal AWS answers a request identifying an extension that is registered in no
/// account — or in an account and Region other than the one the request is served in: a
/// publication resolves the extension against THIS account's registry, and what it did not
/// resolve there is not publishable, whatever else the request carries. Entirely fixed text:
/// no part of it comes from the request or from the store.
pub const NOT_FOUND_IN_YOUR_ACCOUNT: &str = "Type not found in your account.";

/// The refusal AWS answers an account that is not a verified publisher of public extensions,
/// which is what this operation requires of its caller before it consults the registry the
/// request names an extension of.
///
/// The wording is fixed and the single variable part is the 12-digit account id the request
/// was signed under ([`::emulator_server::emulator_account_id`]) — taken from the caller
/// rather than from any member, since it is the CALLING account the registration is missing
/// for — and it names the call that fills the gap: `RegisterPublisher`.
fn not_a_verified_publisher() -> String {
    format!(
        "The account {account_id} is not verified as a publisher. Please register as \
         publisher first",
        account_id = ::emulator_server::emulator_account_id()
    )
}

/// How a request identifies the extension to publish.
///
/// The model states the constraint across three members at once — "Conditional: You must
/// specify Arn, or TypeName and Type" — so a request carries one form or the other, never a
/// mix and never neither.
#[derive(Debug, Clone, Copy)]
enum Identifier<'a> {
    /// `Arn`, which addresses the record on its own: the extension's own ARN, and the version
    /// id the request pinned after it where it pinned one. The record is resolved by the
    /// extension either way, and the version segment is refused on an ARN of this account —
    /// only the default version is published.
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

/// Read what the request identifies, or `Err(message)` for a combination of members that
/// cannot hold together.
///
/// Every modelled constraint (each member's length and pattern, and `Type`'s enum) is
/// already enforced before the handler runs; what is left are the CONDITIONAL rules the
/// model states in prose — the same ones `DescribeType` and `TestType` read. A version
/// segment an `Arn` carries is READ here rather than refused here: the `PrivateTypeArn`
/// pattern (`…:type/.+$`) admits one, so it is a well-formed identifier of a record, and
/// which refusal it reaches depends on the ARN's own account segment.
fn read_request(input: &PublishTypeInput) -> Result<Identifier<'_>, String> {
    match (
        input.arn.as_deref(),
        input.r#type.as_deref(),
        input.type_name.as_deref(),
    ) {
        (Some(arn), None, None) => {
            let (extension, version_id) = crate::state::registry::split_version_arn(arn);
            Ok(Identifier::Arn {
                extension,
                version_id,
            })
        }
        (None, Some(type_kind), Some(type_name)) => Ok(Identifier::Name {
            type_kind,
            type_name,
        }),
        // Both forms at once: the two identify the extension independently, so a request
        // carrying both states two identities for one publication.
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

/// The public version number this publication is assigned: the one the request names, or the
/// one CloudFormation assigns a request that names none.
///
/// `already_published` is the public extension this publisher has published for the
/// extension so far, which is what the two documented rules are read against: a FIRST
/// publication is numbered by CloudFormation, and a later one carrying no number advances
/// the latest by one minor version release.
fn assign_version_number(
    requested: Option<&str>,
    already_published: Option<&crate::state::registry::StoredExtension>,
    type_name: &str,
) -> Result<String, PublishTypeError> {
    let Some(published) = already_published else {
        return match requested {
            // "You cannot specify a version number the first time you publish a type.
            // CloudFormation automatically sets the first version number to be 1.0.0."
            Some(_) => Err(registry_exception(
                FIRST_PUBLICATION_IS_NUMBERED.to_string(),
            )),
            None => Ok(crate::state::registry::FIRST_PUBLIC_VERSION_NUMBER.to_string()),
        };
    };
    match requested {
        // A public version is addressed BY its number, so the one this publication is
        // assigned cannot be one the publisher has already published this extension under.
        Some(requested) if published.version_of_public_number(requested).is_some() => Err(
            registry_exception(number_already_published(type_name, requested)),
        ),
        Some(requested) => Ok(requested.to_string()),
        // "If you don't specify a version number, CloudFormation increments the version
        // number by one minor version release."
        None => {
            let latest = published
                .latest_public_version
                .as_deref()
                .unwrap_or(crate::state::registry::FIRST_PUBLIC_VERSION_NUMBER);
            let increment = |number: &str| {
                crate::state::registry::next_minor_version_number(number)
                    .ok_or_else(|| registry_exception(cannot_increment(type_name, number)))
            };
            // A public version is addressed BY its number here too: where the minor release
            // after the latest is one the extension is already published under — which a
            // publication that NAMED a number below the latest leaves behind — the release
            // after that is the one this publication takes. Bounded by the versions the
            // record holds, since every step moves the minor version up by one.
            let mut candidate = increment(latest)?;
            for _ in 0..=crate::state::registry::MAX_PUBLISHED_VERSIONS {
                if published.version_of_public_number(&candidate).is_none() {
                    return Ok(candidate);
                }
                candidate = increment(&candidate)?;
            }
            Err(registry_exception(cannot_increment(type_name, latest)))
        }
    }
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> PublishTypeError {
    PublishTypeErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The message answered when the request names a record the registry holds but this account
/// did not register: a public extension is already published, so there is nothing of it this
/// operation can publish.
fn not_a_private_extension(arn: &str) -> String {
    format!(
        "Cannot publish {arn}: only a private extension registered in this account and \
         Region can be published"
    )
}

/// The message answered when the version this publication would publish has not passed its
/// contract tests: "an extension must have a test status of PASSED before it can be
/// published", so the status the version does carry is what the refusal names.
fn not_tested(version_arn: &str, type_tests_status: Option<&str>) -> String {
    match type_tests_status {
        Some(status) => format!(
            "Cannot publish {version_arn}: an extension must have a test status of PASSED \
             before it can be published, and this version's test status is {status}"
        ),
        None => format!(
            "Cannot publish {version_arn}: an extension must have a test status of PASSED \
             before it can be published, and this version carries no test status"
        ),
    }
}

/// The message answered a FIRST publication that names a version number, which is the rule
/// `PublicVersionNumber` states: entirely fixed text, since the rule names nothing of the
/// request.
const FIRST_PUBLICATION_IS_NUMBERED: &str =
    "You cannot specify a version number the first time you publish a type. CloudFormation \
     automatically sets the first version number to be 1.0.0";

/// The message answered when the ARN this publication would be addressed by is already held
/// by a public extension of ANOTHER publisher: a public extension belongs to the publisher
/// that published it.
fn published_by_another_publisher(type_name: &str, public_arn: &str) -> String {
    format!("Cannot publish {type_name}: {public_arn} is published by another publisher")
}

/// The message answered when the extension is already published under the version number
/// the request names: a public version is addressed BY its number, so a second version
/// cannot be published under one.
fn number_already_published(type_name: &str, public_version_number: &str) -> String {
    format!("Version {public_version_number} of {type_name} is already published")
}

/// The message answered when the version number the next publication would be assigned
/// cannot be computed from the latest published one, so the request has to name one.
fn cannot_increment(type_name: &str, latest: &str) -> String {
    format!(
        "Specify a version number to publish {type_name} under: the version number {latest} \
         it is published under cannot be incremented by one minor version release"
    )
}

/// The modelled 404: "The specified extension doesn't exist in the CloudFormation
/// registry."
fn type_not_found(identifier: &str) -> PublishTypeError {
    PublishTypeErrorModeled::TypeNotFoundException(TypeNotFoundException {
        message: Some(crate::state::extension_does_not_exist(identifier)),
    })
    .into()
}
