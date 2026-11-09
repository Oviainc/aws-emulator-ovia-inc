// SPDX-License-Identifier: CC0-1.0

//! What a certificate template says the certificate issued under it carries.
//!
//! `IssueCertificate`'s `TemplateArn` "Specifies a custom configuration template to use
//! when issuing a certificate. If this parameter is not provided, Amazon Web Services
//! Private CA defaults to the `EndEntityCertificate/V1` template. For CA certificates, you
//! should choose the shortest path length that meets your needs. The path length is
//! indicated by the PathLenN portion of the ARN, where N is the CA depth."
//! (`IssueCertificateRequest$TemplateArn` docs.)
//!
//! A template is a named set of X.509 extensions, and the one every published template
//! definition turns on is `basicConstraints` (RFC 5280 section 4.2.1.9): it is what makes a
//! CA certificate usable as an issuer, what bounds the hierarchy below it, and what tells a
//! CA certificate apart from an end-entity one. The template's NAME says which certificate
//! it defines, so the definition is read off that name:
//!
//! * `RootCACertificate` — a CA certificate: `cA` TRUE with no `pathLenConstraint`, so the
//!   depth under a root is bounded by its subordinates' own constraints;
//! * `SubordinateCACertificate_PathLenN` — a CA certificate whose `pathLenConstraint` is
//!   the CA depth N the name publishes;
//! * every other kind (`EndEntityCertificate`, `EndEntityClientAuthCertificate`,
//!   `EndEntityServerAuthCertificate`, `CodeSigningCertificate`, `OCSPSigningCertificate`)
//!   — an end-entity certificate: `cA` FALSE, which is what forbids issuing under it;
//! * the `Blank…` kinds define no extensions of their own, which is what makes them blank:
//!   an `ApiPassthrough` is what fills such a certificate in. The one exception is the
//!   `…CriticalBasicConstraints…` variants, whose name says the single extension they do
//!   define — which is why the plain blank kinds cannot be defining it too.
//!
//! The extension is critical, as the basic constraints of a CA certificate must be (RFC
//! 5280 section 4.2.1.9) and as the published definitions state for the end-entity kinds.
//! A name's `_APIPassthrough` / `_APICSRPassthrough` suffix selects where the REST of the
//! certificate's content is taken from rather than what the template defines, so it changes
//! none of the above; a template name outside these kinds defines nothing this service
//! knows, and the certificate issued under it carries no extension of the template's.
//!
//! Beside the extensions a template DEFINES sits the one it passes through: the subject
//! alternative name the certification request asks for ([`requested_subject_alternative_name`]).

use super::*;

use ::x509_cert::der::oid::AssociatedOid;
use ::x509_cert::ext::pkix::BasicConstraints;

/// The extensions a certificate issued under one template carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CertificateTemplate {
    /// The basic constraints the template defines, or `None` for a template that defines
    /// no extensions of its own.
    pub basic_constraints: Option<BasicConstraintsDefinition>,
}

/// A template's `basicConstraints` definition: whether the certificate issued under it is a
/// CA certificate, and how deep a hierarchy may sit below that CA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicConstraintsDefinition {
    /// The `cA` flag: TRUE for the CA-certificate templates.
    pub ca: bool,
    /// The `pathLenConstraint` the template's `PathLenN` portion publishes, when it
    /// publishes one.
    pub path_length: Option<u8>,
}

/// The template name a request that names none is answered under: "If this parameter is not
/// provided, Amazon Web Services Private CA defaults to the `EndEntityCertificate/V1`
/// template." (`IssueCertificateRequest$TemplateArn` docs.)
pub const DEFAULT_TEMPLATE_NAME: &str = "EndEntityCertificate";

/// The template kind that defines a ROOT CA's own certificate.
const ROOT_CA_TEMPLATE_KIND: &str = "RootCACertificate";

/// The template kind that defines a SUBORDINATE CA's certificate, the one whose name
/// publishes the CA depth below it.
const SUBORDINATE_CA_TEMPLATE_KIND: &str = "SubordinateCACertificate";

/// The template kinds that define an END-ENTITY certificate, from the `TemplateArn` values
/// Amazon Web Services Private CA publishes. A name carrying none of these and no CA marker
/// is a kind this service does not know, and defines nothing (the module docs).
const END_ENTITY_TEMPLATE_KINDS: &[&str] = &[
    "EndEntityCertificate",
    "EndEntityClientAuthCertificate",
    "EndEntityServerAuthCertificate",
    "CodeSigningCertificate",
    "OCSPSigningCertificate",
];

/// The portion of a template name that publishes the CA depth of the certificate it
/// defines.
const PATH_LENGTH_SEGMENT: &str = "PathLen";

/// The prefix of the template kinds that define no extensions of their own.
const BLANK_TEMPLATE_PREFIX: &str = "Blank";

/// The portion of a blank template's name that says it defines critical basic constraints
/// after all.
const CRITICAL_BASIC_CONSTRAINTS_SEGMENT: &str = "CriticalBasicConstraints";

/// The template definition a request's `TemplateArn` selects, and the default template's
/// when the request named none.
pub fn certificate_template(template_arn: Option<&str>) -> CertificateTemplate {
    let name = template_arn
        .and_then(template_name)
        .unwrap_or(DEFAULT_TEMPLATE_NAME);
    CertificateTemplate {
        basic_constraints: basic_constraints_of(name),
    }
}

/// The template name the ARN `arn:<partition>:acm-pca:::template/<name>/<version>` carries,
/// or `None` for an ARN that names no template.
fn template_name(template_arn: &str) -> Option<&str> {
    let (_, after) = template_arn.split_once(":template/")?;
    let name = after.split('/').next().unwrap_or(after);
    (!name.is_empty()).then_some(name)
}

/// The basic constraints the template called `name` defines (the module docs), or `None`
/// when it defines none — a blank kind, or a kind this service does not know.
fn basic_constraints_of(name: &str) -> Option<BasicConstraintsDefinition> {
    if name.starts_with(BLANK_TEMPLATE_PREFIX)
        && !name.contains(CRITICAL_BASIC_CONSTRAINTS_SEGMENT)
    {
        return None;
    }
    // A CA kind's name is what publishes the depth below it, so the depth is read only for
    // a kind this service knows: a name outside the published kinds defines nothing,
    // whatever portions it carries.
    if name.contains(ROOT_CA_TEMPLATE_KIND) || name.contains(SUBORDINATE_CA_TEMPLATE_KIND) {
        return Some(BasicConstraintsDefinition {
            ca: true,
            path_length: path_length_in(name),
        });
    }
    END_ENTITY_TEMPLATE_KINDS
        .iter()
        .any(|kind| name.contains(kind))
        .then_some(BasicConstraintsDefinition {
            ca: false,
            path_length: None,
        })
}

/// The CA depth the `PathLenN` portion of `name` publishes, when the name carries one.
fn path_length_in(name: &str) -> Option<u8> {
    let (_, after) = name.split_once(PATH_LENGTH_SEGMENT)?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The DER extensions a certificate issued under `template` for `request` carries: the ones
/// the template defines, plus the subject alternative name the certification request asks
/// for ([`requested_subject_alternative_name`]).
pub fn certificate_extensions(
    template: &CertificateTemplate,
    request: &CertificationRequestInfo,
) -> Option<Extensions> {
    let mut extensions: Extensions = template_extensions(template).unwrap_or_default();
    if let Some(alternative_name) = requested_subject_alternative_name(request) {
        extensions.push(alternative_name);
    }
    (!extensions.is_empty()).then_some(extensions)
}

/// The DER extensions a certificate issued under `template` carries, or `None` for a
/// template that defines none.
pub fn template_extensions(template: &CertificateTemplate) -> Option<Extensions> {
    let constraints = template.basic_constraints?;
    basic_constraints_extension(constraints).map(|extension| vec![extension])
}

/// The `subjectAltName` extension (RFC 5280 section 4.2.1.6) the certification request asks
/// for, when it asks for one.
///
/// Every published template definition carries subject alternative name as a passthrough
/// from the certification request: the names a certificate is to be valid for are the
/// requester's to state and a relying party matches a certificate against them, so the
/// extension the request asks for is copied into the certificate as it stands rather than
/// being defined by the template. A PKCS#10 request states the extensions it asks for in its
/// `extensionRequest` attribute (RFC 2985 section 5.4.2).
pub fn requested_subject_alternative_name(
    request: &CertificationRequestInfo,
) -> Option<Extension> {
    requested_extensions(request)
        .into_iter()
        .find(|extension| extension.extn_id == super::oid::SUBJECT_ALT_NAME)
}

/// Every extension the `extensionRequest` attributes of `request` ask for, in the order they
/// are stated; none for a request that asks for no extension this service can read.
fn requested_extensions(request: &CertificationRequestInfo) -> Vec<Extension> {
    request
        .attributes
        .iter()
        .filter(|attribute| attribute.oid == super::oid::EXTENSION_REQUEST)
        .flat_map(|attribute| attribute.values.iter())
        .filter_map(|value| {
            let der = value.to_der().ok()?;
            Extensions::from_der(&der).ok()
        })
        .flatten()
        .collect()
}

/// The `basicConstraints` extension (RFC 5280 section 4.2.1.9) a template defines, marked
/// critical.
fn basic_constraints_extension(constraints: BasicConstraintsDefinition) -> Option<Extension> {
    let value = BasicConstraints {
        ca: constraints.ca,
        path_len_constraint: constraints.path_length,
    };
    Some(Extension {
        extn_id: BasicConstraints::OID,
        critical: true,
        extn_value: OctetString::new(value.to_der().ok()?).ok()?,
    })
}
