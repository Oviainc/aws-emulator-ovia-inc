// SPDX-License-Identifier: CC0-1.0

//! The SERVER CERTIFICATE a DB instance is created with: which certificate authority it
//! is issued by when the request names none, and how long the certificate it is issued is
//! valid for.
//!
//! The published CA catalog those identifiers are resolved against — what
//! `DescribeCertificates` reports — is [`super::certificates`]; this module is only what a
//! create assigns. Split out of [`super`] so that module keeps the store, the records and
//! their projections; every name here is re-exported from it, so a handler still reads
//! them as `crate::state::<name>`.

use super::*;

/// The CA certificate an instance's server certificate is issued under when the
/// request names none, and so the system default `DescribeCertificates` reports as
/// `DefaultCertificateForNewLaunches` where no override is set.
///
/// `CACertificateIdentifier` documents no default, so the value comes from what AWS
/// publishes: `rds-ca-rsa2048-g1` is the default certificate authority for a new DB
/// instance ("Using SSL/TLS to encrypt a connection to a DB instance", Amazon RDS User
/// Guide). It is a value AWS allocated — nothing computes it — so a table of one is
/// the only honest source; extend it when the published default moves.
///
/// An account can pin a different one per Region with `ModifyCertificates`, which is
/// what [`ComAmazonawsRdsAmazonrdsv19State::default_ca_certificate`] answers from: reach
/// for that where a request is being served, and for this constant only as the default
/// it falls back to.
pub const DEFAULT_CA_CERTIFICATE_IDENTIFIER: &str = "rds-ca-rsa2048-g1";

/// How long the server certificate issued at creation stays valid, which is what
/// `CertificateDetails$ValidTill` ("The expiration date of the DB instance's server
/// certificate") reports.
///
/// Neither the model nor the RDS API documents a lifetime for the issued certificate,
/// so this span is this emulator's choice: one year from the instant the instance was
/// created. It is a function of the stored creation time, so it never moves between
/// two reads of the same instance.
pub const SERVER_CERTIFICATE_VALIDITY: Duration = Duration::from_secs(365 * 24 * 60 * 60);
