// SPDX-License-Identifier: CC0-1.0

//! @op TestType
//! Starts the contract tests of ONE version of a registered private extension, and hands
//! back the ARN of the version being tested.
//!
//! ## What this operation does
//!
//! "Tests a registered extension to make sure it meets all necessary requirements for being
//! published in the CloudFormation registry... If you don't specify a version,
//! CloudFormation uses the default version of the extension in your account and Region for
//! testing. Once you've initiated testing on an extension using TestType, you can pass the
//! returned TypeVersionArn into DescribeType to monitor the current test status and test
//! status description for the extension. An extension must have a test status of PASSED
//! before it can be published."
//!
//! So the call is a WRITE on one extension version: it leaves that version's
//! `TypeTestsStatus` at `IN_PROGRESS` and the conclusion the tests reach over the model the
//! registry holds for it due a moment later (see
//! [`crate::state::registry::StoredTypeVersion::start_type_tests`] and
//! [`crate::state::registry::type_tests_outcome`]), which `DescribeType` is what reads
//! back.
//!
//! Three things gate it, in this order:
//!
//! 1. The CALLER: testing an extension is a step on the way to publishing one, and the
//!    account doing it must be registered with CloudFormation as an extension publisher
//!    ([`crate::state::publisher`], which `RegisterPublisher` writes). An account that holds
//!    no such registration is refused with the registry's modelled 400 before anything about
//!    the extension is resolved — a call naming an extension the registry does not hold is
//!    refused for the caller, not for the extension.
//! 2. WHAT the request identifies: "you must specify Arn, or TypeName and Type", so a
//!    request carrying both forms, neither, or half of the name form identifies no extension
//!    and is refused with that same modelled 400. An ARN that names a version pins the
//!    version too, so a `VersionId` beside it must be that same version.
//! 3. WHICH record it names: an extension this account and Region's registry does not hold
//!    is the modelled 404, and so is a version the record does not carry. A record it holds
//!    but did not REGISTER — a public extension, whether provided by Amazon Web Services or
//!    published by a third party and activated here — carries no contract-test status at all
//!    (`DescribeType`: `TypeTestsStatus` "applies only to registered private extension
//!    versions"), so there is nothing on it to test and the call is refused with the
//!    modelled 400.
//!
//! `LogDeliveryBucket` is "the S3 bucket to which CloudFormation delivers the contract test
//! execution logs"; no modelled member reports it back and this emulator delivers no log
//! objects, so nothing is written for it rather than an object nobody put there.
//!
//! ---
//!
//! # Modeled contract for `TestType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Tests a registered extension to make sure it meets all necessary requirements for being
//! published in the CloudFormation registry. For resource types, this includes passing all
//! contracts tests defined for the type. For modules, this includes determining if the
//! module's model meets all necessary requirements. For more information, see Testing your
//! public extension before publishing in the CloudFormation Command Line Interface (CLI)
//! User Guide. If you don't specify a version, CloudFormation uses the default version of
//! the extension in your account and Region for testing. To perform testing, CloudFormation
//! assumes the execution role specified when the type was registered. For more information,
//! see RegisterType. Once you've initiated testing on an extension using TestType, you can
//! pass the returned TypeVersionArn into DescribeType to monitor the current test status
//! and test status description for the extension. An extension must have a test status of
//! PASSED before it can be published. For more information, see Publishing extensions to
//! make them available for public use in the CloudFormation Command Line Interface (CLI)
//! User Guide.
//!
//! ## Input
//!
//! * `Arn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension. Conditional: You must specify Arn, or
//!   TypeName and Type.
//! * `LogDeliveryBucket` (S3Bucket, len 3..63, pattern ^[\s\S]+$) — The S3 bucket to which
//!   CloudFormation delivers the contract test execution logs. CloudFormation delivers the
//!   logs by the time contract testing has completed and the extension has been assigned a
//!   test type status of PASSED or FAILED. The user calling TestType must be able to access
//!   items in the specified S3 bucket. Specifically, the user needs the following
//!   permissions: GetObject PutObject For more information, see Actions, Resources, and
//!   Condition Keys for Amazon S3 in the Identity and Access Management User Guide.
//! * `Type` (ThirdPartyType, one of: HOOK | MODULE | RESOURCE) — The type of the extension to
//!   test. Conditional: You must specify Arn, or TypeName and Type.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension to test. Conditional: You must specify Arn, or TypeName and Type.
//! * `VersionId` (TypeVersionId, len 1..128, pattern ^[A-Za-z0-9-]+$) — The version of the
//!   extension to test. You can specify the version id with either Arn, or with TypeName and
//!   Type. If you don't specify a version, CloudFormation uses the default version of the
//!   extension in this account and Region for testing.
//!
//! ## Output
//!
//! * `TypeVersionArn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension.
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
// `TestTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#TestType")]
#[derive(Debug)]
pub enum TestTypeErrorModeled {
    CFNRegistryException(CFNRegistryException),
    TypeNotFoundException(TypeNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_type::TestTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestTypeError = crate::scaffolded::Scaffolded<TestTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestTypeErrorModeled> for TestTypeError {
    fn from(modeled: TestTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TestType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestType<Eng>
{
    type Input = TestTypeInput;
    type Output = TestTypeOutput;
    type Error = TestTypeError;

    const NAME: &'static str = "TestType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to test an extension
            // in, so this answers 501 rather than a test nothing is running.
            return Err(crate::scaffolded::Scaffolded::not_implemented("TestType"));
        };
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // Settled FIRST, so a version whose earlier test the clock has already concluded
        // carries that conclusion rather than a stale `IN_PROGRESS` when this call replaces
        // it.
        store.settle(now);

        // The caller gate, before anything about the extension is resolved: an account that
        // has not registered as an extension publisher cannot call this operation at all.
        if store.publisher(None).is_none() {
            return Err(registry_exception(NOT_A_PUBLISHER.to_string()));
        }

        let (identifier, version_id) = read_request(&input).map_err(registry_exception)?;

        // The record the request names, in the Region the request was signed for: the
        // registry is regional, so a record another Region holds is not addressable here.
        let stored = match identifier {
            Identifier::Arn { extension, .. } => store.extension_by_arn(&region, extension),
            Identifier::Name {
                type_kind,
                type_name,
            } => store.extension_by_name(&region, type_kind, type_name),
        };
        let extension = match stored {
            Some(extension) => extension,
            // The registry also holds the public extensions Amazon Web Services publishes,
            // which are in every account's registry without anybody registering them: one
            // of those EXISTS, so what the call runs into is the public-extension refusal
            // rather than "doesn't exist".
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
                return Err(match published {
                    Some(published) => registry_exception(not_a_private_extension(&published.arn)),
                    None => type_not_found(identifier.named()),
                });
            }
        };
        if !extension.is_private() {
            return Err(registry_exception(not_a_private_extension(&extension.arn)));
        }
        // "If you don't specify a version, CloudFormation uses the default version of the
        // extension in your account and Region for testing."
        let version = extension
            .selected_version(version_id)
            .ok_or_else(|| type_not_found(&named_version(identifier.named(), version_id)))?;
        // What the write below addresses, and what the response reports: "the Amazon
        // Resource Name (ARN) of the extension", which for the version being tested is that
        // version's own ARN — the one this operation documents passing "into DescribeType to
        // monitor the current test status".
        let extension_arn = extension.arn.clone();
        let type_name = extension.type_name.clone();
        let tested = version.version_id.clone();
        let type_version_arn = version.arn.clone();

        let Some(version) = store
            .extension_by_arn_mut(&region, &extension_arn)
            .and_then(|extension| extension.version_mut(&tested))
        else {
            // Unreachable: the record and the version were both resolved just above, under
            // the same lock. Answered as the modelled 404 rather than with a panic, which
            // would leave the shared store poisoned for every other caller.
            return Err(type_not_found(&named_version(identifier.named(), version_id)));
        };
        version.start_type_tests(&type_name, now, crate::state::STATUS_TRANSITION_DELAY);

        Ok(TestTypeOutput {
            type_version_arn: Some(type_version_arn),
        })
    }
}

/// The refusal AWS answers an account that has not registered as a publisher of public
/// extensions, which is what this operation requires of its caller.
const NOT_A_PUBLISHER: &str =
    "You must register as a publisher with CloudFormation to call test-type.";

/// How a request identifies the extension to test.
///
/// The model states the constraint across three members at once — "Conditional: You must
/// specify Arn, or TypeName and Type" — so a request carries one form or the other, never a
/// mix and never neither.
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

/// What the not-found message names when the extension was found but the version the
/// request selected was not.
fn named_version(extension: &str, version_id: Option<&str>) -> String {
    match version_id {
        Some(version_id) => format!("{extension}/{version_id}"),
        None => extension.to_string(),
    }
}

/// Read what the request identifies and which version it selects, or `Err(message)` for a
/// combination of members that cannot hold together.
///
/// Every modelled constraint (each member's length and pattern, and `Type`'s enum) is
/// already enforced before the handler runs; what is left are the CONDITIONAL rules the
/// model states in prose — the same ones `DescribeType` reads, since the two identify an
/// extension and a version by the same members.
fn read_request(input: &TestTypeInput) -> Result<(Identifier<'_>, Option<&str>), String> {
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
        // carrying both states two identities for one test.
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
    // "You can specify the version id with either Arn, or with TypeName and Type": a
    // version ARN already names one, so a `VersionId` beside it must be that same version
    // — two different ones select two different records to test.
    let version_id = match (input.version_id.as_deref(), pinned_by_arn) {
        (Some(named), Some(pinned)) if named != pinned => {
            return Err(format!(
                "VersionId {named} does not name the extension version the Arn does \
                 ({pinned})"
            ))
        }
        (named, pinned) => named.or(pinned),
    };
    Ok((identifier, version_id))
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> TestTypeError {
    TestTypeErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The message answered when the request names a record the registry holds but this account
/// did not register: a public extension carries no contract-test status, so there is
/// nothing on it this operation can test.
fn not_a_private_extension(arn: &str) -> String {
    format!(
        "Cannot test {arn}: only a private extension registered in this account and Region \
         can be tested"
    )
}

/// The modelled 404: "The specified extension doesn't exist in the CloudFormation
/// registry."
fn type_not_found(identifier: &str) -> TestTypeError {
    TestTypeErrorModeled::TypeNotFoundException(TypeNotFoundException {
        message: Some(crate::state::extension_does_not_exist(identifier)),
    })
    .into()
}
