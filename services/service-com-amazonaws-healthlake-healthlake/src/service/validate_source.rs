// SPDX-License-Identifier: CC0-1.0

//! The `POST /validate-source?sourceFormat=...` surface of the data-transformation
//! endpoint: the pre-flight check a caller runs over a source document before it
//! starts a data-transformation job over the same bytes.
//!
//! # Why this is not a generated operation
//!
//! The shipped model DOES declare the operation — `ValidateSource`, `POST
//! /validate-source`, with `sourceFormat` as an `@httpQuery` member and the document
//! as a required `@httpPayload` blob — but under the
//! `com.amazon.awshealthlakedatatransformationfrontendservice` namespace, OUTSIDE the
//! closure of the service shape this crate serves (`com.amazonaws.healthlake#HealthLake`,
//! whose 26 bound operations `smithy_query(query="digest")` lists and which does not
//! include it). So no module is generated for it, nothing is in the routing table, and
//! this crate's awsJson1_0 routing key — the `X-Amz-Target` header a REST call like
//! this one does not carry — cannot name it.
//!
//! It is therefore reached through
//! [`crate::service::EmulatedService::override_operation`], the one seam a service has
//! for a request the model's routing key cannot name. What it ANSWERS is taken from
//! those model shapes rather than invented: the report carries the four members
//! `ValidateSourceResponse` requires (`Valid`, `SourceFormat`, `Issues`, `Summary`),
//! each issue the members `SourceValidationIssue` declares (`Severity` and `Message`
//! required, plus the optional `Code`, `Field`, `Line`, `Column`, `Xpath` and
//! `Remediation`), and `Summary` both counts `ValidationSummary` requires. The
//! operation's modelled errors give the refusals their shape too: `ValidationException`
//! at HTTP 400, `InternalServerException` at 500.
//!
//! # The contract
//!
//! Two failure classes, and the whole point of the endpoint is that they are
//! answered differently:
//!
//! * a REQUEST-level problem — no `sourceFormat`, a `sourceFormat` outside the
//!   modelled enum or one this endpoint does not validate, an absent document, or one
//!   past the modelled payload bound — is refused with the modelled
//!   `ValidationException` at HTTP 400;
//! * a CONTENT-level problem — a document that is incomplete, or not a C-CDA
//!   document at all — is NOT a refusal. It is the answer: HTTP 200 carrying a
//!   validation report that says the document is invalid and names what is wrong
//!   with it. A caller distinguishes "my document has structural issues" from "my
//!   request was rejected" by the status, so a content problem that answered 4xx
//!   would collapse the two.
//!
//! The report is computed from the submitted bytes by a real XML parser (the
//! maintained `roxmltree` crate) walked against the elements CDA declares as
//! required — including each finding's line, column and XPath, read off the parse
//! tree; nothing about it is keyed to a particular document.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, Service, Violation, ViolationKind,
};

use super::non_rpc::{bounded, modelled_error};

/// The path this endpoint publishes, as the first (and only) segment of the request
/// URI, from the modelled `@http(uri: "/validate-source")`. The
/// `datatransformation.` host prefix the client injects addresses the endpoint and
/// does not change the path.
pub const VALIDATE_SOURCE_PATH_SEGMENT: &str = "validate-source";

/// The query parameter naming the format of the submitted document, as
/// `ValidateSourceRequest$SourceFormat`'s `@httpQuery` spells it.
const SOURCE_FORMAT_PARAM: &str = "sourceFormat";

/// The violation path of the `SourceFormat` member — its model name in the lowerCamel
/// spelling a violation path takes.
const SOURCE_FORMAT_MEMBER: &str = "sourceFormat";

/// The violation path of the `SourceFile` member, the request's `@httpPayload`.
const SOURCE_FILE_MEMBER: &str = "sourceFile";

/// Every value the modelled `SourceFormat` enum declares
/// (`smithy_query(query="shape",shape="SourceFormat")`), which is what the request
/// member accepts at all.
const MODELLED_SOURCE_FORMATS: &[&str] = &["CCDA", "CSV"];

/// The subset of those formats this endpoint validates a document OF.
///
/// `CCDA` names a document format with a structure to walk; the recorded behaviour of
/// this endpoint refuses `CSV` (HTTP 400) even though the member accepts it, so the
/// set that reaches the validator is the narrower one.
const VALIDATED_SOURCE_FORMATS: &[&str] = &["CCDA"];

/// The largest document this endpoint accepts, from the modelled `SourceFileBlob`
/// `@length(max: 10485760)` the `@httpPayload` member targets.
const MAX_SOURCE_FILE_BYTES: usize = 10_485_760;

/// The media type the validation report is returned as. The report is not a FHIR
/// resource, so it carries the service's ordinary JSON media type.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The `ValidationSeverity` every finding this validator produces carries.
///
/// The modelled enum declares `ERROR` and `WARNING`, and its documentation draws the
/// line: an error "prevent[s] successful conversion", a warning does not. Every rule
/// checked here is the PRESENCE of an element CDA requires, whose absence blocks
/// conversion, so nothing here is a warning and `Summary.WarningCount` is 0.
const ERROR_SEVERITY: &str = "ERROR";

/// The `Code` — "the validation rule code that identified this issue" — every
/// structural finding carries.
///
/// It is the one code the recorded behaviour of this endpoint evidences, and this
/// module does not invent a code vocabulary beside it: a finding is told from another
/// by its `Message`, `Field` and `Xpath`, all computed from the submitted document.
const CRITICAL_ERROR: &str = "CRITICAL_ERROR";

/// The XML namespace a CDA document declares (HL7 CDA Release 2).
const CDA_NAMESPACE: &str = "urn:hl7-org:v3";

/// The root element of a CDA document.
const CLINICAL_DOCUMENT: &str = "ClinicalDocument";

/// The children CDA Release 2 requires of `ClinicalDocument` — every element the
/// CDA R2 schema declares on it with a minimum cardinality of 1, which C-CDA R2.1
/// inherits unchanged (`typeId`, `id`, `code`, `effectiveTime`,
/// `confidentialityCode`, `recordTarget`, `author`, `custodian`, `component`).
///
/// Source: HL7 CDA Release 2, the `POCD_MT000040.ClinicalDocument` class. Everything
/// else CDA declares on `ClinicalDocument` (`realmCode`, `templateId`, `title`,
/// `languageCode`, `setId`, `versionNumber`, `componentOf`, the participations …) is
/// optional and so is not required here.
const REQUIRED_CLINICAL_DOCUMENT_CHILDREN: &[&str] = &[
    "typeId",
    "id",
    "code",
    "effectiveTime",
    "confidentialityCode",
    "recordTarget",
    "author",
    "custodian",
    "component",
];

/// The two forms CDA allows a document body to take, inside `component`.
const BODY_FORMS: &[&str] = &["structuredBody", "nonXMLBody"];

/// The `/validate-source` endpoint, as the one [`ErasedOperation`] the service holds
/// for it.
///
/// Stateless: a validation report is a function of the submitted bytes alone, so
/// nothing is read from or written to the service's store and one value serves every
/// request.
#[derive(Debug, Default)]
pub struct ValidateSource;

impl ValidateSource {
    /// The one value a service holds.
    pub const fn new() -> Self {
        Self
    }
}

impl ErasedOperation for ValidateSource {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let source_format = match requested_source_format(req.uri()) {
                Ok(source_format) => source_format,
                Err(refused) => return validation_exception(&refused),
            };
            // The router buffers every request before dispatch, so a body it cannot
            // hand back in memory is a server-side failure, never a finding about the
            // document: reporting `Valid: false` here would state a structural problem
            // with bytes nothing read.
            let Some(body) = req.body().bytes() else {
                return unreadable_body();
            };
            if let Err(refused) = submitted_document(body) {
                return validation_exception(&refused);
            }
            report_response(source_format, &validate_ccda(body))
        })
    }
}

/// The `sourceFormat` the request asks for, or the sentence its refusal states.
///
/// Three request-level refusals, each worded the way this service words that class:
/// an absent `@required` member and a value outside the modelled enum go through
/// [`crate::service::validation_envelope`], so they read exactly like a refusal of any
/// other HealthLake member; a value the member DOES accept but this endpoint does not
/// validate is this endpoint's own rule, so it states that rule instead of claiming a
/// constraint violation the model does not declare.
fn requested_source_format(uri: &str) -> Result<&'static str, String> {
    let Some(requested) = super::non_rpc::query_value(uri, SOURCE_FORMAT_PARAM) else {
        return Err(super::validation_envelope(&[Violation {
            path: SOURCE_FORMAT_MEMBER.to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }]));
    };
    if let Some(validated) = VALIDATED_SOURCE_FORMATS
        .iter()
        .find(|format| **format == requested)
    {
        return Ok(validated);
    }
    if MODELLED_SOURCE_FORMATS.contains(&requested.as_str()) {
        return Err(format!(
            "Source validation is available for {} source files only",
            VALIDATED_SOURCE_FORMATS.join(", ")
        ));
    }
    Err(super::validation_envelope(&[Violation {
        path: SOURCE_FORMAT_MEMBER.to_string(),
        kind: ViolationKind::EnumValue(
            MODELLED_SOURCE_FORMATS
                .iter()
                .map(|format| (*format).to_string())
                .collect(),
        ),
        // Deliberately absent, as everywhere else this service states an enum
        // violation: a value outside the set never became a value of the type, so the
        // refusal quotes none (see `crate::service::clause_for`).
        value: None,
        sensitive: false,
    }]))
}

/// `Ok` when the request carries a document this endpoint will read, else the sentence
/// its refusal states.
///
/// Both rules are the model's own, on the `@httpPayload` member: it is `@required`, so
/// a request with no body carries no document to validate, and its target declares
/// `@length(max: 10485760)`, so a larger one is outside the member's bound. Neither is
/// a finding ABOUT a document — there is no document — so neither is reported at 200.
fn submitted_document(body: &[u8]) -> Result<(), String> {
    if body.is_empty() {
        return Err(super::validation_envelope(&[Violation {
            path: SOURCE_FILE_MEMBER.to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }]));
    }
    if body.len() > MAX_SOURCE_FILE_BYTES {
        return Err(super::validation_envelope(&[Violation {
            path: SOURCE_FILE_MEMBER.to_string(),
            kind: ViolationKind::MaxLength(
                i64::try_from(MAX_SOURCE_FILE_BYTES).unwrap_or(i64::MAX),
            ),
            // The value is the document itself: a sensitive blob, which no message
            // echoes.
            value: None,
            sensitive: true,
        }]));
    }
    Ok(())
}

/// One finding about a document, in the members `SourceValidationIssue` declares.
///
/// `Severity` and `Code` are the same for every finding this validator produces (see
/// their constants); everything here is what differs, and every field is read off the
/// parse tree rather than written per case.
#[derive(Debug)]
struct Finding {
    /// `Message`: what is wrong, the one member besides `Severity` the model requires.
    message: String,
    /// `Field`: the element the rule is about, when the rule is about one.
    field: Option<String>,
    /// `Xpath`: where that element belongs. The model populates this "only for C-CDA
    /// source files", which is the only format validated here.
    xpath: Option<String>,
    /// `Line` and `Column`: where in the source file the issue was detected.
    line: Option<u32>,
    column: Option<u32>,
    /// `Remediation`: the suggested fix.
    remediation: Option<String>,
}

impl Finding {
    /// A finding with only the message the model requires.
    fn new(message: String) -> Self {
        Self {
            message,
            field: None,
            xpath: None,
            line: None,
            column: None,
            remediation: None,
        }
    }

    /// The element this finding is about, and where it belongs.
    fn at(mut self, field: &str, xpath: String) -> Self {
        self.field = Some(field.to_string());
        self.xpath = Some(xpath);
        self
    }

    /// Where in the source file the issue was detected.
    fn positioned(mut self, at: ::roxmltree::TextPos) -> Self {
        self.line = Some(at.row);
        self.column = Some(at.col);
        self
    }

    /// The suggested fix.
    fn remediated(mut self, remediation: String) -> Self {
        self.remediation = Some(remediation);
        self
    }

    /// This finding as the modelled `SourceValidationIssue`. An optional member the
    /// finding does not carry is left out rather than sent as a null.
    fn as_issue(&self) -> ::serde_json::Value {
        let mut issue = ::serde_json::Map::new();
        issue.insert("Severity".to_string(), ERROR_SEVERITY.into());
        issue.insert("Message".to_string(), self.message.clone().into());
        issue.insert("Code".to_string(), CRITICAL_ERROR.into());
        if let Some(field) = &self.field {
            issue.insert("Field".to_string(), field.clone().into());
        }
        if let Some(xpath) = &self.xpath {
            issue.insert("Xpath".to_string(), xpath.clone().into());
        }
        if let Some(line) = self.line {
            issue.insert("Line".to_string(), line.into());
        }
        if let Some(column) = self.column {
            issue.insert("Column".to_string(), column.into());
        }
        if let Some(remediation) = &self.remediation {
            issue.insert("Remediation".to_string(), remediation.clone().into());
        }
        ::serde_json::Value::Object(issue)
    }
}

/// The validation report for `source_format`, as HTTP 200.
///
/// Every member the modelled `ValidateSourceResponse` requires is present, and `Valid`
/// and `Summary.ErrorCount` are read off the findings, so a report cannot claim a
/// document is valid while listing issues with it.
fn report_response(source_format: &str, findings: &[Finding]) -> RtResponse {
    let report = ::serde_json::json!({
        "Valid": findings.is_empty(),
        "SourceFormat": source_format,
        "Issues": findings.iter().map(Finding::as_issue).collect::<Vec<_>>(),
        "Summary": {
            "ErrorCount": findings.len(),
            // Every rule this validator applies is a required element's presence, so
            // nothing it finds is a warning; see `ERROR_SEVERITY`.
            "WarningCount": 0,
        },
    });
    let mut response = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::from(report.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// The modelled `ValidationException` (HTTP 400), in this service's awsJson1_0 error
/// envelope.
///
/// The body carries the shape's own `Message` member, not the framework's lowercase
/// `message`: this service answers a refusal from its MODELLED error shape (see the
/// "Why this file validates input" note in [`crate::service`]) so an SDK surfaces the
/// sentence instead of an empty message. `ValidateSource` has no generated module in
/// this crate to carry that shape, so the envelope comes from
/// [`super::non_rpc::modelled_error`], which every non-RPC surface answers through.
fn validation_exception(message: &str) -> RtResponse {
    modelled_error(400, "ValidationException", message)
}

/// The answer when the request body cannot be read in memory at all: the modelled
/// server fault.
///
/// A defensive arm — the router buffers every request before dispatch — and it answers
/// the shape the operation declares for a failure of the service itself
/// (`InternalServerException`, `@httpError(500)`), rather than any report about a
/// document.
fn unreadable_body() -> RtResponse {
    modelled_error(
        500,
        "InternalServerException",
        "The server could not read the submitted document from the request body",
    )
}

/// Everything structurally wrong with `body` as a C-CDA document, in document order.
///
/// Empty means the document satisfies every element CDA requires, which is what a
/// report's `Valid: true` states. A document that cannot be read as XML at all yields
/// exactly one finding — the reads after the parse failure would be about a document
/// the parser never built.
fn validate_ccda(body: &[u8]) -> Vec<Finding> {
    let Ok(text) = ::std::str::from_utf8(body) else {
        return vec![Finding::new(
            "The source document is not UTF-8 text, so it cannot be read as a C-CDA XML \
             document"
                .to_string(),
        )
        .remediated("Submit the document as UTF-8 encoded XML".to_string())];
    };
    // A leading byte-order mark and leading whitespace are framing a file may carry
    // ahead of its XML declaration; neither says anything about the document's
    // structure, so they are stripped rather than reported.
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let document = match ::roxmltree::Document::parse(text) {
        Ok(document) => document,
        Err(error) => {
            return vec![Finding::new(format!(
                "The source document is not well-formed XML: {}",
                bounded(&error.to_string())
            ))
            .positioned(error.pos())
            .remediated("Correct the XML so the document is well-formed".to_string())];
        }
    };

    let root = document.root_element();
    let root_at = document.text_pos_at(root.range().start);
    let root_name = root.tag_name().name();
    if root_name != CLINICAL_DOCUMENT {
        return vec![Finding::new(format!(
            "The source document's root element is '{}', not the CDA '{CLINICAL_DOCUMENT}'",
            bounded(root_name)
        ))
        .at(root_name, format!("/{}", bounded(root_name)))
        .positioned(root_at)
        .remediated(format!(
            "Submit a C-CDA document whose root element is {CLINICAL_DOCUMENT}"
        ))];
    }
    // The namespace is required to be CDA's own only when the document declares one at
    // all: a document that declares none is still read for its structure rather than
    // refused on a single attribute.
    if let Some(namespace) = root.tag_name().namespace() {
        if namespace != CDA_NAMESPACE {
            return vec![Finding::new(format!(
                "The source document's root element is in namespace '{}', not the CDA \
                 '{CDA_NAMESPACE}'",
                bounded(namespace)
            ))
            .at(root_name, format!("/{CLINICAL_DOCUMENT}"))
            .positioned(root_at)
            .remediated(format!(
                "Declare the CDA namespace {CDA_NAMESPACE} on {CLINICAL_DOCUMENT}"
            ))];
        }
    }

    let mut findings = Vec::new();
    for required in REQUIRED_CLINICAL_DOCUMENT_CHILDREN {
        if child(root, required).is_none() {
            findings.push(
                Finding::new(format!(
                    "{CLINICAL_DOCUMENT} is missing the required element '{required}'"
                ))
                .at(required, format!("/{CLINICAL_DOCUMENT}/{required}"))
                .positioned(root_at)
                .remediated(format!(
                    "Add the required {required} element to {CLINICAL_DOCUMENT}"
                )),
            );
        }
    }
    if let Some(component) = child(root, "component") {
        findings.extend(body_findings(&document, component));
    }
    findings
}

/// Everything structurally wrong with a `ClinicalDocument/component`: CDA requires it
/// to carry one body, either a `structuredBody` or a `nonXMLBody`, and a
/// `structuredBody` to carry at least one `component/section`.
fn body_findings(
    document: &::roxmltree::Document<'_>,
    component: ::roxmltree::Node<'_, '_>,
) -> Vec<Finding> {
    let at = document.text_pos_at(component.range().start);
    let path = format!("/{CLINICAL_DOCUMENT}/component");
    let Some(form) = BODY_FORMS.iter().find_map(|form| child(component, form)) else {
        return vec![Finding::new(format!(
            "{CLINICAL_DOCUMENT}/component carries neither of the required document bodies \
             ({})",
            BODY_FORMS.join(" or ")
        ))
        .at("component", path)
        .positioned(at)
        .remediated(format!(
            "Add a {} element to {CLINICAL_DOCUMENT}/component",
            BODY_FORMS.join(" or a ")
        ))];
    };
    let form_name = form.tag_name().name();
    if form_name != "structuredBody" {
        return Vec::new();
    }
    let sections = form
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "component")
        .filter(|node| child(*node, "section").is_some())
        .count();
    if sections == 0 {
        return vec![Finding::new(format!(
            "{CLINICAL_DOCUMENT}/component/{form_name} carries no component/section"
        ))
        .at(form_name, format!("{path}/{form_name}"))
        .positioned(document.text_pos_at(form.range().start))
        .remediated(format!(
            "Add at least one component/section to {CLINICAL_DOCUMENT}/component/{form_name}"
        ))];
    }
    Vec::new()
}

/// The first child element of `node` with local name `name`.
///
/// The local name alone decides: every element of a CDA document inherits the
/// document's default namespace, so a namespace-qualified comparison would add
/// nothing the root element's own check has not already settled.
fn child<'a>(node: ::roxmltree::Node<'a, 'a>, name: &str) -> Option<::roxmltree::Node<'a, 'a>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}
