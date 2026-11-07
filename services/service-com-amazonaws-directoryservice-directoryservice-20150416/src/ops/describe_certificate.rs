// SPDX-License-Identifier: CC0-1.0

//! @op DescribeCertificate
//! Reports one certificate `RegisterCertificate` registered with a directory.
//!
//! A REPORTING operation: it reports the stored registration and changes nothing a later
//! caller sees. Its one write is `advance_clock`, which materialises the state
//! transitions the clock had already made due — including the registration's own move
//! out of `Registering` — so a read reports what was owed at that instant whether or not
//! anybody called.
//!
//! A registration lives on the directory it was made for, so both ids are resolved: an
//! id naming no stored directory is refused with `DirectoryDoesNotExistException`, and a
//! certificate id that directory holds no registration for with
//! `CertificateDoesNotExistException` — which is also what another directory's
//! certificate id is, since a registration is not visible through a directory it was not
//! made for.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeCertificate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")
//! ```
//!
//! Displays information about the certificate registered for secure LDAP or client
//! certificate authentication.
//!
//! ## Input
//!
//! * `CertificateId` (CertificateId, pattern ^c-[0-9a-f]{10}$) **required** — The identifier
//!   of the certificate.
//! * `DirectoryId` (DirectoryId, pattern ^d-[0-9a-f]{10}$) **required** — The identifier of
//!   the directory.
//!
//! ## Output
//!
//! * `Certificate` (Certificate) — Information about the certificate, including registered
//!   date time, certificate state, the reason for the state, expiration date time, and
//!   certificate common name.
//!
//! ## Errors
//!
//! * `CertificateDoesNotExistException` (HTTP 400) — The certificate is not present in the
//!   system for describe or deregister activities.
//! * `ClientException` (HTTP 400) — A client exception has occurred.
//! * `DirectoryDoesNotExistException` (HTTP 400) — The specified directory does not exist in
//!   the system.
//! * `InvalidParameterException` (HTTP 400) — One or more parameters are not valid.
//! * `ServiceException` (HTTP 500) — An exception has occurred in Directory Service.
//! * `UnsupportedOperationException` (HTTP 400) — The operation is not supported.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeCertificateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeCertificateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.directoryservice#DescribeCertificate")]
#[derive(Debug)]
pub enum DescribeCertificateErrorModeled {
    CertificateDoesNotExistException(CertificateDoesNotExistException),
    ClientException(ClientException),
    DirectoryDoesNotExistException(DirectoryDoesNotExistException),
    InvalidParameterException(InvalidParameterException),
    ServiceException(ServiceException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_certificate::DescribeCertificateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeCertificateError = crate::scaffolded::Scaffolded<DescribeCertificateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeCertificateErrorModeled> for DescribeCertificateError {
    fn from(modeled: DescribeCertificateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeCertificate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeCertificate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeCertificate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeCertificate<Eng>
{
    type Input = DescribeCertificateInput;
    type Output = DescribeCertificateOutput;
    type Error = DescribeCertificateError;

    const NAME: &'static str = "DescribeCertificate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CERTIFICATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CERTIFICATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        let mut store = state.lock();
        store.advance_clock(::std::time::SystemTime::now());

        let directory = store.directories.get(&input.directory_id).ok_or_else(|| {
            DescribeCertificateErrorModeled::DirectoryDoesNotExistException(
                crate::state::refusal(crate::state::no_such_directory(&input.directory_id)),
            )
        })?;

        // The lookup is within the directory the request names, so a certificate id of
        // another directory is as absent as one nothing registered.
        let certificate = directory
            .certificates
            .get(&input.certificate_id)
            .ok_or_else(|| {
                DescribeCertificateErrorModeled::CertificateDoesNotExistException(
                    crate::state::refusal(crate::state::CERTIFICATE_NOT_PRESENT),
                )
            })?;

        Ok(DescribeCertificateOutput {
            certificate: Some(certificate.to_certificate()),
        })
    }
}
