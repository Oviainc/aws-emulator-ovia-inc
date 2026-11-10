// SPDX-License-Identifier: CC0-1.0

//! @op RegisterCertificate
//! Registers a certificate with a directory, for secure LDAP or for client certificate
//! authentication.
//!
//! The values this operation reports back about a certificate are the CERTIFICATE's own:
//! the PEM the request carries is decoded by the `x509-cert` parser
//! ([`crate::state::parse_certificate`]), the subject common name and the `notAfter`
//! validity bound are read out of it, and a PEM that does not decode is refused with the
//! modelled `InvalidCertificateException` rather than stored with invented ones.
//!
//! The registration is not in service the instant this call answers: the modelled
//! `CertificateState` enum carries the transient `Registering` before the settled
//! `Registered`, so the move between them is owed by the clock
//! ([`crate::state::TRANSITION_SPAN`]) and materialised by `advance_clock`.
//! `DescribeCertificate` is what reports the registration back.
//!
//! A certificate is registered FOR a directory, so a request naming no stored directory
//! is refused with `DirectoryDoesNotExistException`, and one naming a directory still
//! being built by the transition its create owes with `DirectoryUnavailableException` —
//! exactly as `AddIpRoutes` and `CreateComputer` refuse a directory that is not in
//! service. A directory registers a given certificate once
//! (`CertificateAlreadyExistsException`, compared on the decoded certificate rather
//! than on the text it arrived as) and at most
//! [`crate::state::MAX_CERTIFICATES_PER_DIRECTORY`] of them
//! (`CertificateLimitExceededException`).
//!
//! `UnsupportedOperationException` is left for behaviour this emulator is asked for and
//! does not serve: every directory this service stores accepts a registration.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterCertificate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")
//! ```
//!
//! Registers a certificate for a secure LDAP or client certificate authentication.
//!
//! ## Input
//!
//! * `CertificateData` (CertificateData, len 1..8192) **required** — The certificate PEM
//!   string that needs to be registered.
//! * `ClientCertAuthSettings` (ClientCertAuthSettings) — A ClientCertAuthSettings object that
//!   contains client certificate authentication settings.
//! * `DirectoryId` (DirectoryId, pattern ^d-[0-9a-f]{10}$) **required** — The identifier of
//!   the directory.
//! * `Type` (CertificateType, one of: ClientCertAuth | ClientLDAPS) — The function that the
//!   registered certificate performs. Valid values include ClientLDAPS or ClientCertAuth. The
//!   default value is ClientLDAPS.
//!
//! ## Output
//!
//! * `CertificateId` (CertificateId, pattern ^c-[0-9a-f]{10}$) — The identifier of the
//!   certificate.
//!
//! ## Errors
//!
//! * `CertificateAlreadyExistsException` (HTTP 400) — The certificate has already been
//!   registered into the system.
//! * `CertificateLimitExceededException` (HTTP 400) — The certificate could not be added
//!   because the certificate limit has been reached.
//! * `ClientException` (HTTP 400) — A client exception has occurred.
//! * `DirectoryDoesNotExistException` (HTTP 400) — The specified directory does not exist in
//!   the system.
//! * `DirectoryUnavailableException` (HTTP 400) — The specified directory is unavailable.
//! * `InvalidCertificateException` (HTTP 400) — The certificate PEM that was provided has
//!   incorrect encoding.
//! * `InvalidParameterException` (HTTP 400) — One or more parameters are not valid.
//! * `ServiceException` (HTTP 500) — An exception has occurred in Directory Service.
//! * `UnsupportedOperationException` (HTTP 400) — The operation is not supported.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterCertificateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterCertificateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.directoryservice#RegisterCertificate")]
#[derive(Debug)]
pub enum RegisterCertificateErrorModeled {
    CertificateAlreadyExistsException(CertificateAlreadyExistsException),
    CertificateLimitExceededException(CertificateLimitExceededException),
    ClientException(ClientException),
    DirectoryDoesNotExistException(DirectoryDoesNotExistException),
    DirectoryUnavailableException(DirectoryUnavailableException),
    InvalidCertificateException(InvalidCertificateException),
    InvalidParameterException(InvalidParameterException),
    ServiceException(ServiceException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_certificate::RegisterCertificateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterCertificateError = crate::scaffolded::Scaffolded<RegisterCertificateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterCertificateErrorModeled> for RegisterCertificateError {
    fn from(modeled: RegisterCertificateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterCertificate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterCertificate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterCertificate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterCertificate<Eng>
{
    type Input = RegisterCertificateInput;
    type Output = RegisterCertificateOutput;
    type Error = RegisterCertificateError;

    const NAME: &'static str = "RegisterCertificate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CERTIFICATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CERTIFICATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // The certificate is decoded BEFORE the store is reached, so nothing
        // caller-sized is parsed under the lock and a PEM this emulator cannot read is
        // refused the same way whether or not the id names a stored directory — and
        // before the directory's stage is looked at, so data that is not a certificate is
        // reported as such whatever state the directory is in. The modelled
        // `InvalidCertificateException` is exactly this failure, stated as Directory
        // Service words it: the registration it could not perform, for the directory the
        // request named.
        let parsed = crate::state::parse_certificate(&input.certificate_data).ok_or_else(|| {
            RegisterCertificateErrorModeled::InvalidCertificateException(crate::state::refusal(
                crate::state::certificate_not_valid(&input.directory_id),
            ))
        })?;

        // "Valid values include ClientLDAPS or ClientCertAuth. The default value is
        // ClientLDAPS." The modelled enum has already refused anything else.
        let certificate_type = input
            .r#type
            .unwrap_or_else(|| crate::state::DEFAULT_CERTIFICATE_TYPE.to_string());

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.advance_clock(now);

        // Every refusal is decided on an IMMUTABLE borrow, before the store mints
        // anything: an id and a sequence number are consumed only by a call that goes on
        // to store a registration, so a refused call leaves no gap in the id space.
        let directory = store.directories.get(&input.directory_id).ok_or_else(|| {
            // The registration hangs off the directory, so an id naming no stored
            // directory is refused and stores nothing.
            RegisterCertificateErrorModeled::DirectoryDoesNotExistException(
                crate::state::refusal(crate::state::no_such_directory(&input.directory_id)),
            )
        })?;

        // Registering a certificate changes the directory's own security configuration,
        // so it needs a directory that is in service: one still being built by the
        // transition its create owes is unavailable, which is the modelled refusal for
        // exactly that. The diagnostic is this service's shared one for a directory a
        // call cannot reach: the requested id and nothing about the stage it is in.
        if directory.stage != crate::state::STAGE_ACTIVE {
            return Err(
                RegisterCertificateErrorModeled::DirectoryUnavailableException(
                    crate::state::refusal(crate::state::directory_unavailable(
                        &input.directory_id,
                    )),
                )
                .into(),
            );
        }

        // What a certificate PERFORMS decides which directories it can be registered
        // with: the function has to be one the directory's own type offers — client-side
        // LDAPS for the types `EnableLDAPS` serves, and client (smart card)
        // authentication for the types that offer it — so a request asking for a function
        // this directory type does not have asks for something it does not support, which
        // is the modelled refusal for exactly that. Decided before anything is stored, so
        // the certificate inventory gains no entry. Each half states its own fixed
        // sentence, which names the directory type rather than quoting it.
        let offered_for = crate::state::certificate_function_directory_types(&certificate_type);
        if !offered_for.contains(&directory.directory_type.as_str()) {
            let message = if certificate_type == crate::state::CERTIFICATE_TYPE_CLIENT_CERT_AUTH {
                crate::state::CLIENT_AUTHENTICATION_UNSUPPORTED
            } else {
                crate::state::LDAPS_UNSUPPORTED
            };
            return Err(
                RegisterCertificateErrorModeled::UnsupportedOperationException(
                    crate::state::refusal(message),
                )
                .into(),
            );
        }

        // A directory registers a given certificate once. The comparison is on the DER
        // the PEM decoded to, so what a directory holds at most one of is the
        // CERTIFICATE rather than the text that carried it. The refusal is this error
        // class's own sentence, which states the fact and interpolates nothing.
        if directory
            .certificates
            .values()
            .any(|candidate| candidate.der == parsed.der)
        {
            return Err(
                RegisterCertificateErrorModeled::CertificateAlreadyExistsException(
                    crate::state::refusal(crate::state::CERTIFICATE_ALREADY_REGISTERED),
                )
                .into(),
            );
        }

        // Checked BEFORE the insert below, so a refused call stores nothing. The refusal
        // is this error class's own sentence; the bound itself is
        // `MAX_CERTIFICATES_PER_DIRECTORY`.
        if directory.certificates.len() >= crate::state::MAX_CERTIFICATES_PER_DIRECTORY {
            return Err(
                RegisterCertificateErrorModeled::CertificateLimitExceededException(
                    crate::state::refusal(crate::state::CERTIFICATE_LIMIT_REACHED),
                )
                .into(),
            );
        }

        // The request is accepted: minting needs the store itself, so it happens here,
        // between the checks above and the mutable borrow the insert needs.
        let certificate_id = store.mint_certificate_id();
        let seq = store.next_seq();
        let directory = store
            .directories
            .get_mut(&input.directory_id)
            .ok_or_else(crate::state::state_unavailable)?;

        directory.certificates.insert(
            certificate_id.clone(),
            crate::state::CertificateRecord {
                certificate_id: certificate_id.clone(),
                // The certificate is not in service the instant this call answers: the
                // move to `Registered` is owed by the clock, like a create's stage
                // transition.
                state: crate::state::CERTIFICATE_STATE_REGISTERING.to_string(),
                state_due_at: Some(now + crate::state::TRANSITION_SPAN),
                // Nothing has changed the state but the clock, so there is no state
                // change to describe.
                state_reason: None,
                common_name: parsed.common_name,
                registered_at: now,
                expires_at: parsed.expires_at,
                certificate_type,
                client_cert_auth_settings: input.client_cert_auth_settings,
                der: parsed.der,
                seq,
            },
        );

        // Read the id back out of the store, so the response cannot name a certificate
        // the insert did not persist.
        let stored = store
            .directories
            .get(&input.directory_id)
            .and_then(|directory| directory.certificates.get(&certificate_id))
            .ok_or_else(crate::state::state_unavailable)?;
        Ok(RegisterCertificateOutput {
            certificate_id: Some(stored.certificate_id.clone()),
        })
    }
}
