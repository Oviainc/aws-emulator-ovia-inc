// SPDX-License-Identifier: CC0-1.0

//! The `ClientCertificate` records of the account: the MATERIAL one carries, the window it
//! is valid for, the order the collection is listed in, and the reference a `Stage` holds
//! on one.
//!
//! A client certificate is "a client certificate used to configure client-side SSL
//! authentication while sending requests to the integration endpoint"
//! (`smithy_query(query="shape", shape="ClientCertificate")`), and API Gateway is what
//! GENERATES it: the caller supplies no key material, so the certificate a record reports
//! is issued here and the private half is the service's — it is never stored and no
//! operation of this service reports it.
//!
//! A child module of [`super`] rather than more of that file: the record's material and
//! its validity window are one concern, and keeping them together leaves the state module
//! itself to the records of the other resources.

use super::{ComAmazonawsApigatewayBackplanecontrolserviceState, StageEntry};

/// How long a generated client certificate is valid for.
///
/// API Gateway's own documentation for the certificates it generates: "The generated
/// certificates are valid for 365 days" — so the record's `expirationDate` is its
/// `createdDate` plus this span, and the issued material bears the same window.
pub const CLIENT_CERTIFICATE_VALIDITY: ::std::time::Duration =
    ::std::time::Duration::from_secs(365 * 24 * 60 * 60);

/// The most client certificates this emulator stores for one account, at API Gateway's
/// published per-account-per-Region quota for client certificates (60), which is what
/// `GenerateClientCertificate` answers its modelled `LimitExceededException` past.
pub const MAX_CLIENT_CERTIFICATES: usize = 60;

/// When a client certificate created at `created_date` expires: that instant plus
/// [`CLIENT_CERTIFICATE_VALIDITY`], at the whole-second resolution this surface reports a
/// timestamp in (see [`super::now`]).
pub fn client_certificate_expiration(
    created_date: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    let validity = i64::try_from(CLIENT_CERTIFICATE_VALIDITY.as_secs()).unwrap_or(0);
    ::aws_smithy_types::DateTime::from_secs(created_date.secs().saturating_add(validity))
}

/// The certificate `GenerateClientCertificate` issues for the record `client_certificate_id`,
/// PEM-encoded, valid over the record's own window.
///
/// A REAL self-signed X.509 certificate, issued through the maintained generator this tree
/// already uses for certificate material, so what the member carries is a certificate a
/// client can parse and present rather than text in the shape of one. The record is named
/// in the subject's common name — an integration endpoint identifies the caller by the
/// certificate's subject — and no subject alternative name is set, because the certificate
/// authenticates an API Gateway stage rather than a host.
///
/// The private half is generated here and dropped: API Gateway keeps it ("the generated
/// certificate's private key is managed by API Gateway"), and the `ClientCertificate` shape
/// declares no member for it, so nothing this service stores or reports can present it.
///
/// `None` when the material does not issue, which a caller is told about rather than being
/// handed a record with no certificate in it.
pub fn mint_client_certificate_pem(
    client_certificate_id: &str,
    created_date: ::aws_smithy_types::DateTime,
    expiration_date: ::aws_smithy_types::DateTime,
) -> Option<String> {
    let (from_year, from_month, from_day) = certificate_day(created_date)?;
    let (to_year, to_month, to_day) = certificate_day(expiration_date)?;
    let key = ::rcgen::KeyPair::generate().ok()?;
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, client_certificate_id);
    params.distinguished_name = subject;
    params.not_before = ::rcgen::date_time_ymd(from_year, from_month, from_day);
    params.not_after = ::rcgen::date_time_ymd(to_year, to_month, to_day);
    Some(params.self_signed(&key).ok()?.pem())
}

/// What `GenerateClientCertificate` answers when [`mint_client_certificate_pem`] issues no
/// material.
///
/// Its own refusal rather than [`super::state_unavailable`]'s, which states a condition
/// that is not this one: the store is reachable, the generator is what did not produce a
/// certificate. Like that sibling it is the scaffold channel's non-retryable signal rather
/// than a 5xx server fault — this operation models no server error, and nothing a caller
/// could change about the request would make the next attempt issue one.
pub fn client_certificate_not_issued<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator could not generate a client certificate's material"
            .to_string(),
    })
}

/// The UTC day `at` falls on, as `(year, month, day)` — the form the certificate generator
/// takes a validity bound in.
///
/// The validity the material bears is stated in whole DAYS — the resolution an X.509
/// validity period is read at by the tools that inspect one — so the window runs from the
/// day the record was created to the day it expires.
fn certificate_day(at: ::aws_smithy_types::DateTime) -> Option<(i32, u8, u8)> {
    use ::chrono::Datelike;

    let day = ::chrono::DateTime::<::chrono::Utc>::from_timestamp(at.secs(), 0)?.date_naive();
    Some((
        day.year(),
        u8::try_from(day.month()).ok()?,
        u8::try_from(day.day()).ok()?,
    ))
}

/// The `BadRequestException` message for a client certificate a `Stage` still names in its
/// `clientCertificateId`, which is what `DeleteClientCertificate` refuses while the
/// reference stands (see
/// [`ComAmazonawsApigatewayBackplanecontrolserviceState::stage_referencing_client_certificate`]).
///
/// API Gateway words this condition as ONE sentence whose fixed text states that the
/// certificate cannot be deleted and that a stage references it — `Cannot delete client
/// certificate. Certificate ID '`…`', is referenced in stage `…`.`, the comma and the
/// closing period included — with the certificate's own id and the NAME of the referencing
/// stage as its only variable parts. The error class is the one this surface answers a
/// delete blocked by a stored reference with, as `DeleteVpcLink` and `DeleteStage` answer
/// their own standing references.
pub fn client_certificate_in_use(client_certificate_id: &str, stage_name: &str) -> String {
    format!(
        "Cannot delete client certificate. Certificate ID '{client_certificate_id}', is \
         referenced in stage {stage_name}."
    )
}

impl ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// The ids of every stored client certificate in creation order, the stable sequence
    /// [`super::page_of`] cuts a `GetClientCertificates` page out of.
    ///
    /// The rank is fixed when each certificate is generated, with the certificate's own id
    /// breaking a tie, so nothing a caller can change moves a certificate in the
    /// collection: describing one, or patching its description, leaves it where it was, and
    /// the `position` cursor a page handed back keeps naming the same point in the sequence.
    pub fn client_certificate_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&super::ClientCertificateEntry> =
            self.client_certificates.values().collect();
        // Compared by reference rather than through a cloned sort key: the order is the
        // same, and the ids are cloned once, for the answer.
        ordered.sort_by(|left, right| {
            (left.sequence, left.id.as_str()).cmp(&(right.sequence, right.id.as_str()))
        });
        ordered
            .into_iter()
            .map(|certificate| certificate.id.clone())
            .collect()
    }

    /// The NAME of the stage that still presents the client certificate
    /// `client_certificate_id`, or `None` when no stage does.
    ///
    /// A `Stage` names "the identifier of a client certificate for an API stage"
    /// (`Stage$clientCertificateId`), so a stage carrying that identifier is a REFERENCE to
    /// the certificate — the same kind of reference
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::integration_referencing_vpc_link`]
    /// finds for a VPC link. A client certificate belongs to the ACCOUNT rather than to one
    /// API, so the walk covers every stored API; both walks are in key order, so a refusal
    /// NAMES the same stage on every call for one set of records.
    pub fn stage_referencing_client_certificate(
        &self,
        client_certificate_id: &str,
    ) -> Option<String> {
        self.rest_apis.values().find_map(|api| {
            api.stages
                .values()
                .find(|stage: &&StageEntry| {
                    stage.client_certificate_id.as_deref() == Some(client_certificate_id)
                })
                .map(|stage| stage.stage_name.clone())
        })
    }
}
