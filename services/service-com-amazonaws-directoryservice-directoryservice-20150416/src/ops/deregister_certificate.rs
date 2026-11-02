// SPDX-License-Identifier: CC0-1.0

//! @op DeregisterCertificate
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeregisterCertificate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")
//! ```
//!
//! Deletes from the system the certificate that was registered for secure LDAP or client
//! certificate authentication.
//!
//! ## Input
//!
//! * `CertificateId` (CertificateId, pattern ^c-[0-9a-f]{10}$) **required** — The identifier
//!   of the certificate.
//! * `DirectoryId` (DirectoryId, pattern ^d-[0-9a-f]{10}$) **required** — The identifier of
//!   the directory.
//!
//! ## Errors
//!
//! * `CertificateDoesNotExistException` (HTTP 400) — The certificate is not present in the
//!   system for describe or deregister activities.
//! * `CertificateInUseException` (HTTP 400) — The certificate is being used for the LDAP
//!   security connection and cannot be removed without disabling LDAP security.
//! * `ClientException` (HTTP 400) — A client exception has occurred.
//! * `DirectoryDoesNotExistException` (HTTP 400) — The specified directory does not exist in
//!   the system.
//! * `DirectoryUnavailableException` (HTTP 400) — The specified directory is unavailable.
//! * `InvalidParameterException` (HTTP 400) — One or more parameters are not valid.
//! * `ServiceException` (HTTP 500) — An exception has occurred in Directory Service.
//! * `UnsupportedOperationException` (HTTP 400) — The operation is not supported.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeregisterCertificateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeregisterCertificateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.directoryservice#DeregisterCertificate")]
#[derive(Debug)]
pub enum DeregisterCertificateErrorModeled {
    CertificateDoesNotExistException(CertificateDoesNotExistException),
    CertificateInUseException(CertificateInUseException),
    ClientException(ClientException),
    DirectoryDoesNotExistException(DirectoryDoesNotExistException),
    DirectoryUnavailableException(DirectoryUnavailableException),
    InvalidParameterException(InvalidParameterException),
    ServiceException(ServiceException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::deregister_certificate::DeregisterCertificateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeregisterCertificateError = crate::scaffolded::Scaffolded<DeregisterCertificateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeregisterCertificateErrorModeled> for DeregisterCertificateError {
    fn from(modeled: DeregisterCertificateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeregisterCertificate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeregisterCertificate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeregisterCertificate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeregisterCertificate<Eng>
{
    type Input = DeregisterCertificateInput;
    type Output = DeregisterCertificateOutput;
    type Error = DeregisterCertificateError;

    const NAME: &'static str = "DeregisterCertificate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_CERTIFICATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_CERTIFICATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.advance_clock(now);

        // The registration hangs off the directory the request names, so an id naming no
        // stored directory is refused and changes nothing.
        let directory = store.directories.get(&input.directory_id).ok_or_else(|| {
            DeregisterCertificateErrorModeled::DirectoryDoesNotExistException(
                crate::state::refusal(crate::state::no_such_directory(&input.directory_id)),
            )
        })?;

        // Deleting a certificate from the system changes the directory's own security
        // configuration, exactly as registering one does, so it needs a directory that is
        // in service: one still being built by the transition its create owes is
        // unavailable. The diagnostic is this service's shared one for a directory a call
        // cannot reach: the requested id and nothing about the stage it is in.
        if directory.stage != crate::state::STAGE_ACTIVE {
            return Err(
                DeregisterCertificateErrorModeled::DirectoryUnavailableException(
                    crate::state::refusal(crate::state::directory_unavailable(
                        &input.directory_id,
                    )),
                )
                .into(),
            );
        }

        // The lookup is within the directory the request names, so a certificate id of
        // another directory is as absent as one nothing registered — the same resolution
        // `DescribeCertificate` makes, worded by the same shared diagnostic.
        let certificate = directory
            .certificates
            .get(&input.certificate_id)
            .ok_or_else(|| {
                DeregisterCertificateErrorModeled::CertificateDoesNotExistException(
                    crate::state::refusal(crate::state::no_such_certificate(
                        &input.certificate_id,
                        &input.directory_id,
                    )),
                )
            })?;

        // A certificate that secures the directory's LDAP calls cannot be taken out from
        // under them: while LDAP security is in force, the registrations performing
        // `ClientLDAPS` are what serves it, which is the modelled refusal for exactly
        // that failure. A certificate performing client certificate authentication serves
        // no LDAP security connection, so the switch does not hold it.
        if certificate.certificate_type == crate::state::CERTIFICATE_TYPE_CLIENT_LDAPS
            && directory.ldaps_security_in_force()
        {
            return Err(DeregisterCertificateErrorModeled::CertificateInUseException(
                crate::state::refusal(format!(
                    "Certificate {certificate_id} is being used for the LDAP security \
                     connection of directory {directory_id} and cannot be removed without \
                     disabling LDAP security",
                    certificate_id = input.certificate_id,
                    directory_id = input.directory_id
                )),
            )
            .into());
        }

        // The request is accepted. The certificate is not out of the system the instant
        // this call answers: the registration leaving the directory is owed by the clock,
        // like a removed IP route leaving it, so until then the registration is still
        // reported — in the transient state the modelled enum carries for exactly this.
        let certificate = store
            .directories
            .get_mut(&input.directory_id)
            .and_then(|directory| directory.certificates.get_mut(&input.certificate_id))
            .ok_or_else(crate::state::state_unavailable)?;
        // A teardown already under way is not restarted: the registration leaves the
        // store when the deregister that started it owed it, not one span after the last
        // call repeating the request.
        if certificate.state != crate::state::CERTIFICATE_STATE_DEREGISTERING {
            certificate.state = crate::state::CERTIFICATE_STATE_DEREGISTERING.to_string();
            certificate.state_due_at = Some(now + crate::state::TRANSITION_SPAN);
            // Nothing but this call has changed the state, so there is no state change to
            // describe.
            certificate.state_reason = None;
        }

        Ok(DeregisterCertificateOutput {})
    }
}
