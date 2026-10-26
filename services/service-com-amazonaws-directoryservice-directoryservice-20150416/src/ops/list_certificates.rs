// SPDX-License-Identifier: CC0-1.0

//! @op ListCertificates
//! Lists the certificates `RegisterCertificate` registered with one directory.
//!
//! A REPORTING operation: it pages over the stored registrations and changes nothing a
//! later caller sees. Its one write is `advance_clock`, which materialises the state
//! transitions the clock had already made due — including a registration's own move out
//! of `Registering` — so a listing reports what was owed at that instant whether or not
//! anybody called.
//!
//! The registrations live on the directory they were made for, so the `DirectoryId` is
//! resolved first: an id naming no stored directory is refused with
//! `DirectoryDoesNotExistException`, while a stored directory nothing was registered
//! with is an EMPTY list rather than a refusal. The page is taken in registration order
//! over the whole set, and a `NextToken` is issued only when the directory holds more
//! than the page the request asked for; a token this service did not issue for this
//! listing and this directory is refused with `InvalidNextTokenException` rather than
//! served as a page of something else.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListCertificates`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json")
//! ```
//!
//! For the specified directory, lists all the certificates registered for a secure LDAP or
//! client certificate authentication.
//!
//! ## Input
//!
//! * `DirectoryId` (DirectoryId, pattern ^d-[0-9a-f]{10}$) **required** — The identifier of
//!   the directory.
//! * `Limit` (PageLimit, range 1..50) — The number of items that should show up on one page
//! * `NextToken` (NextToken) — A token for requesting another page of certificates if the
//!   NextToken response element indicates that more certificates are available. Use the value
//!   of the returned NextToken element in your request until the token comes back as null.
//!   Pass null if this is the first call.
//!
//! ## Output
//!
//! * `CertificatesInfo` (CertificatesInfo) — A list of certificates with basic details
//!   including certificate ID, certificate common name, certificate state.
//! * `NextToken` (NextToken) — Indicates whether another page of certificates is available
//!   when the number of available certificates exceeds the page limit.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — A client exception has occurred.
//! * `DirectoryDoesNotExistException` (HTTP 400) — The specified directory does not exist in
//!   the system.
//! * `InvalidNextTokenException` (HTTP 400) — The NextToken value is not valid.
//! * `InvalidParameterException` (HTTP 400) — One or more parameters are not valid.
//! * `ServiceException` (HTTP 500) — An exception has occurred in Directory Service.
//! * `UnsupportedOperationException` (HTTP 400) — The operation is not supported.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListCertificatesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListCertificatesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.directoryservice#ListCertificates")]
#[derive(Debug)]
pub enum ListCertificatesErrorModeled {
    ClientException(ClientException),
    DirectoryDoesNotExistException(DirectoryDoesNotExistException),
    InvalidNextTokenException(InvalidNextTokenException),
    InvalidParameterException(InvalidParameterException),
    ServiceException(ServiceException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_certificates::ListCertificatesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListCertificatesError = crate::scaffolded::Scaffolded<ListCertificatesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListCertificatesErrorModeled> for ListCertificatesError {
    fn from(modeled: ListCertificatesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListCertificates` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListCertificates<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListCertificates<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListCertificates<Eng>
{
    type Input = ListCertificatesInput;
    type Output = ListCertificatesOutput;
    type Error = ListCertificatesError;

    const NAME: &'static str = "ListCertificates";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CERTIFICATES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CERTIFICATES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // The cursor a previous page handed back: the registration rank of the
        // certificate this page resumes after, together with the filter it was issued
        // under. A token this service did not issue, or one issued for another listing
        // or another directory, is refused rather than served as a page of something
        // else.
        let filter = crate::state::cursor_filter(&[input.directory_id.as_str()]);
        let after_seq = match input.next_token.as_deref() {
            None => 0,
            Some(token) => crate::state::decode_cursor(CURSOR_KIND, token)
                .and_then(|value| {
                    let (seq, issued_for) = value.split_once(crate::state::CURSOR_SEPARATOR)?;
                    if issued_for != filter.as_str() {
                        return None;
                    }
                    seq.parse::<u64>().ok()
                })
                .ok_or_else(|| {
                    ListCertificatesErrorModeled::InvalidNextTokenException(
                        crate::state::invalid_next_token(),
                    )
                })?,
        };
        let page_size = crate::state::page_size(input.limit);

        let mut store = state.lock();
        store.advance_clock(::std::time::SystemTime::now());

        // The listing is of ONE directory's registrations, so an id naming no stored
        // directory is reported rather than served as an empty page.
        let directory = store.directories.get(&input.directory_id).ok_or_else(|| {
            ListCertificatesErrorModeled::DirectoryDoesNotExistException(crate::state::refusal(
                crate::state::no_such_directory(&input.directory_id),
            ))
        })?;

        // Every certificate registered with that directory, in the order they were
        // registered, from the cursor on. A directory nothing was registered with
        // contributes nothing, which is an empty list rather than a refusal.
        let mut selected: Vec<(u64, CertificateInfo)> = directory
            .certificates_in_registration_order()
            .into_iter()
            .filter(|certificate| certificate.seq > after_seq)
            .map(|certificate| (certificate.seq, certificate.to_certificate_info()))
            .collect();
        drop(store);

        let truncated = selected.len() > page_size;
        selected.truncate(page_size);
        let page_ended_after = selected.last().map(|(seq, _)| *seq);
        let certificates_info: Vec<CertificateInfo> =
            selected.into_iter().map(|(_, entry)| entry).collect();

        Ok(ListCertificatesOutput {
            certificates_info: Some(certificates_info),
            // Only a truncated page has a next one; the final page carries no token.
            next_token: if truncated {
                page_ended_after.map(|seq| {
                    crate::state::encode_cursor(
                        CURSOR_KIND,
                        &format!(
                            "{seq}{separator}{filter}",
                            separator = crate::state::CURSOR_SEPARATOR
                        ),
                    )
                })
            } else {
                None
            },
        })
    }
}

/// Which listing this operation's `NextToken` belongs to, so a token issued by another
/// paginated operation is refused rather than misread.
const CURSOR_KIND: &str = "certificates";
