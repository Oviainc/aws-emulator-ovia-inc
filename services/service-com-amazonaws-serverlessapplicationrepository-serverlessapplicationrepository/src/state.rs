// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository service.
//!
//! The Serverless Application Repository is a catalogue: an *application* is
//! identified by an ARN minted from its name, and carries an ordered set of
//! *versions*, each keyed by its semantic version. Everything the handlers in
//! `src/ops/` answer with is read back out of [`ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState`],
//! so a create is observable by the next Get/List and a delete is observable as an
//! absence.
//!
//! Beside the two maps this module also holds the projections from the stored record
//! into the modelled shapes (`Version`, `ApplicationSummary`, `VersionSummary`) and the
//! paging primitives both list operations share, so the per-operation files cannot
//! drift from each other in how they render the same stored record.
//!
//! Room is deliberately left for the operations later issues own: an
//! `Application` grows a `policy_statements` list for
//! PutApplicationPolicy/GetApplicationPolicy, and the state struct grows a template
//! map for the CloudFormation template/changeset operations. Neither is declared yet —
//! the operations that would read them still answer 501.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ops::types::{
    ApplicationSummary, BadRequestException, ConflictException, InternalServerErrorException,
    NotFoundException, ParameterDefinition, Version, VersionSummary,
};

/// The region an ARN carries when the request names none.
///
/// A request's region is its SigV4 credential scope (see [`request_region`]); an
/// unsigned request has no scope, and every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `serverlessrepo` ARN namespace this service's model declares, used for every
/// `ApplicationId`.
const ARN_NAMESPACE: &str = "serverlessrepo";

/// The largest page either list operation serves, and the page size a request that
/// names none gets.
///
/// It is the maximum of the `MaxItems` shape's `@range` (`range 1..100`,
/// `smithy_query(query="shape", shape="MaxItems")`). The model declares no default, so
/// a request without `maxItems` is served the modelled maximum per page.
pub const MAX_ITEMS_LIMIT: i32 = 100;

/// How many applications this emulator retains, and how many versions one application
/// retains.
///
/// Both stores grow once per create and are released only by a caller's own delete, so
/// without a ceiling one caller can drive the emulator's heap until the host is out of
/// memory. AWS publishes no account quota for either, so these are the emulator's own
/// bounds: generous enough that no realistic client script reaches them, finite so the
/// growth is not caller-controlled. A create past a ceiling is refused with the
/// modelled `BadRequestException`, never served by evicting somebody else's record.
pub const MAX_STORED_APPLICATIONS: usize = 1_000;

/// See [`MAX_STORED_APPLICATIONS`].
pub const MAX_VERSIONS_PER_APPLICATION: usize = 200;

/// The largest inline `LicenseBody`/`ReadmeBody` the store retains: "Maximum size 5 MB"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="LicenseBody")`,
/// and the same sentence on `ReadmeBody`).
pub const MAX_ARTIFACT_BYTES: usize = 5 * 1024 * 1024;

/// The largest inline `TemplateBody` the store retains and parses.
///
/// A packaged AWS SAM template is a CloudFormation template, whose documented maximum
/// size is 1 MB (1,048,576 bytes) for a template in an S3 object — the form SAR stores.
pub const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;

/// The most `Parameters`, and the most `Resources`, a template may declare: 200 and 500,
/// CloudFormation's documented per-template maxima. Bounding both bounds the work
/// [`parse_template`] does on caller-supplied content.
pub const MAX_TEMPLATE_PARAMETERS: usize = 200;

/// See [`MAX_TEMPLATE_PARAMETERS`].
pub const MAX_TEMPLATE_RESOURCES: usize = 500;

/// One published application: the record `CreateApplication` writes and every other
/// operation reads.
#[derive(Debug, Clone, Default)]
pub struct Application {
    /// The application ARN, and this record's key in [`ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState::applications`].
    pub application_id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub home_page_url: Option<String>,
    pub labels: Vec<String>,
    /// The license as a link, either the caller's `LicenseUrl` or the locator minted
    /// for a `LicenseBody` the caller supplied inline.
    pub license_url: Option<String>,
    /// The license text, when the caller supplied `LicenseBody` rather than a URL.
    pub license_body: Option<String>,
    /// The readme as a link; see [`Application::license_url`].
    pub readme_url: Option<String>,
    /// The readme text, when the caller supplied `ReadmeBody` rather than a URL.
    pub readme_body: Option<String>,
    pub spdx_license_id: Option<String>,
    /// Whether AWS has verified this application's author. Author verification is a
    /// manual AWS review that no API call performs, so an application published here
    /// is never a verified author's.
    pub is_verified_author: bool,
    /// The verified author's public profile, which only a verified author has.
    pub verified_author_url: Option<String>,
    /// ISO-8601, as the model's `CreationTime` (a `__string`, not a timestamp) carries it.
    pub creation_time: String,
    /// The region this application was published in — the one its ARN names, and the one
    /// its name is unique within.
    pub region: String,
    /// Every version of this application, ordered by semantic version so a paged walk
    /// is stable.
    pub versions: BTreeMap<String, AppVersion>,
    /// The semantic version with the highest precedence of those stored, which
    /// `GetApplication` reports when the caller names none.
    pub latest_version: Option<String>,
}

impl Application {
    /// The version this application reports when a request names none: the one with the
    /// highest semantic-version precedence of those stored.
    pub fn latest(&self) -> Option<&AppVersion> {
        self.latest_version
            .as_deref()
            .and_then(|latest| self.versions.get(latest))
    }

    /// The version a request names, or `None` when this application has no such version
    /// — which is as absent as an application that was never created.
    pub fn version_named(&self, semantic_version: &str) -> Option<&AppVersion> {
        self.versions.get(semantic_version)
    }

    /// Store `version` and keep [`Application::latest_version`] pointing at the stored
    /// version with the highest precedence, so "latest" does not depend on the order the
    /// versions happened to be created in.
    pub fn insert_version(&mut self, version: AppVersion) {
        let semantic_version = version.semantic_version.clone();
        if self
            .latest_version
            .as_deref()
            .is_none_or(|current| takes_precedence_over(&semantic_version, current))
        {
            self.latest_version = Some(semantic_version.clone());
        }
        self.versions.insert(semantic_version, version);
    }

    /// This record projected into the modelled `ApplicationSummary` that both
    /// `ListApplications` returns and `GetApplication` reads its scalar members from.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_id: self.application_id.clone(),
            author: self.author.clone(),
            creation_time: Some(self.creation_time.clone()),
            description: self.description.clone(),
            home_page_url: self.home_page_url.clone(),
            // An empty list is the member's VALUE, not its absence: the service reports
            // a label-less application as `"labels": []`, the same way it reports a
            // version-less application as `"versions": []`. A client reading
            // `response["labels"]` must not have to guard for the member being missing.
            labels: Some(self.labels.clone()),
            name: self.name.clone(),
            spdx_license_id: self.spdx_license_id.clone(),
        }
    }
}

/// One version of one application.
#[derive(Debug, Clone, Default)]
pub struct AppVersion {
    pub semantic_version: String,
    /// ISO-8601; see [`Application::creation_time`].
    pub creation_time: String,
    /// The packaged template as a link: the caller's `TemplateUrl`, or the locator
    /// minted for a `TemplateBody` supplied inline. The model makes
    /// `Version$TemplateUrl` `@required`, so a version always has one.
    pub template_url: String,
    /// The template text, when the caller supplied `TemplateBody` rather than a URL.
    pub template_body: Option<String>,
    pub source_code_url: Option<String>,
    pub source_code_archive_url: Option<String>,
    /// The parameters the stored template declares; see [`parse_template`].
    pub parameter_definitions: Vec<ParameterDefinition>,
    /// The capabilities a deployment of this version must acknowledge. A template does
    /// not state them — they are derived from the resources it creates, which the
    /// CloudFormation changeset operations own — so this stays empty here.
    pub required_capabilities: Vec<String>,
    /// Whether every resource in the version is supported in the region it is read
    /// from. This emulator serves one account's catalogue from the caller's own region,
    /// so a stored version is always supported where it is read.
    pub resources_supported: bool,
}

impl AppVersion {
    /// This record projected into the modelled `Version`.
    pub fn version(&self, application_id: &str) -> Version {
        Version {
            application_id: application_id.to_string(),
            creation_time: self.creation_time.clone(),
            parameter_definitions: self.parameter_definitions.clone(),
            required_capabilities: self.required_capabilities.clone(),
            resources_supported: self.resources_supported,
            semantic_version: self.semantic_version.clone(),
            source_code_archive_url: self.source_code_archive_url.clone(),
            source_code_url: self.source_code_url.clone(),
            template_url: self.template_url.clone(),
        }
    }

    /// This record projected into the modelled `VersionSummary` `ListApplicationVersions`
    /// returns.
    pub fn summary(&self, application_id: &str) -> VersionSummary {
        VersionSummary {
            application_id: application_id.to_string(),
            creation_time: self.creation_time.clone(),
            semantic_version: self.semantic_version.clone(),
            source_code_url: self.source_code_url.clone(),
        }
    }
}

/// The template, license and readme a caller may supply either inline or as a link.
///
/// Every one of them is modelled as a `*Body`/`*Url` pair of which the caller may
/// "specify only one … otherwise an error results"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="TemplateBody")`),
/// so the pair is resolved once, here, into the single link the stored record keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    Template,
    License,
    Readme,
}

impl Artifact {
    /// The member names this pair carries in the model, for a refusal that names them.
    pub const fn members(self) -> (&'static str, &'static str) {
        match self {
            Self::Template => ("TemplateBody", "TemplateUrl"),
            Self::License => ("LicenseBody", "LicenseUrl"),
            Self::Readme => ("ReadmeBody", "ReadmeUrl"),
        }
    }

    /// The file name the minted locator ends in.
    const fn file_name(self) -> &'static str {
        match self {
            Self::Template => "template.yaml",
            Self::License => "license.txt",
            Self::Readme => "readme.md",
        }
    }
}

/// The link reported for content the caller supplied INLINE.
///
/// The real service copies an inline body into an S3 object it owns and reports that
/// object's URL; this emulator holds the body in its own state, so it reports the
/// same-shaped locator derived from the stored application, its version and the
/// artefact. Like an ARN it is an identifier of stored state, not a fetchable
/// endpoint: an emulator makes no outbound connections and serves no object.
pub fn minted_artifact_url(
    region: &str,
    application_name: &str,
    semantic_version: Option<&str>,
    artifact: Artifact,
) -> String {
    let account = ::engine::emulator_account_id();
    let path = match semantic_version {
        Some(semantic_version) => format!("{application_name}/{semantic_version}"),
        None => application_name.to_string(),
    };
    format!(
        "https://awsserverlessrepo-{region}.s3.{region}.amazonaws.com/{account}/{path}/{}",
        artifact.file_name()
    )
}

/// The link stored for a `*Body`/`*Url` pair: the caller's URL, or a minted locator
/// for a body they supplied inline, or `None` when they supplied neither.
pub fn resolve_artifact_url(
    body: Option<&String>,
    url: Option<&String>,
    region: &str,
    application_name: &str,
    semantic_version: Option<&str>,
    artifact: Artifact,
) -> Option<String> {
    match (body, url) {
        (_, Some(url)) => Some(url.clone()),
        (Some(_), None) => Some(minted_artifact_url(
            region,
            application_name,
            semantic_version,
            artifact,
        )),
        (None, None) => None,
    }
}

/// The application ARN minted for `name`:
/// `arn:aws:serverlessrepo:<region>:<account>:applications/<name>`.
///
/// Built through `engine`'s `emulator_arn` so the account segment is the one canonical
/// emulator account.
pub fn application_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("applications/{name}"))
}

/// The region this request is for: its SigV4 credential scope's, else [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The key an application's name is unique under.
///
/// Region-scoped, because the `ApplicationId` minted from the name is: two regions are
/// two catalogues, and the same name in each is two applications with two ARNs.
pub fn name_key(region: &str, name: &str) -> String {
    format!("{region}/{name}")
}

/// The condition an error body names in its `errorCode` member.
///
/// Every error this service models declares `ErrorCode` (jsonName `errorCode`) beside
/// `Message` (`smithy_query(query="shape", shape="ConflictException")`, and the same for
/// `NotFoundException`, `BadRequestException`, `ForbiddenException`,
/// `InternalServerErrorException` and `TooManyRequestsException`). The HTTP status
/// already travels in the status line and in the `x-amzn-errortype` header, so what the
/// body's `errorCode` adds is the NAME of the condition that was raised: it is the only
/// field that separates two conditions sharing one modelled shape and one status — a
/// duplicate application and a duplicate version are both 409 `ConflictException` — so a
/// client branching on `errorCode` can tell them apart. The member's own documentation is
/// just the bare status number, which is the shape's `httpError`, not a value to echo.
///
/// Each name is the `<Subject><Condition>Exception` form AWS reports for the condition;
/// a condition no recording covers takes the same form over the subject the handler
/// checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The catalogue already holds an application with the requested name (409).
    ApplicationAlreadyExists,
    /// The catalogue holds no application with the requested id (404).
    ApplicationNotFound,
    /// The application already carries the requested semantic version (409).
    VersionAlreadyExists,
    /// The application carries no such semantic version (404).
    VersionNotFound,
    /// A member of the request is outside the contract (400).
    InvalidRequest,
    /// The service could not serve the request (500).
    InternalServerError,
}

impl ErrorCode {
    /// The name this condition reports in `errorCode`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ApplicationAlreadyExists => "ApplicationAlreadyExistsException",
            Self::ApplicationNotFound => "ApplicationNotFoundException",
            Self::VersionAlreadyExists => "VersionAlreadyExistsException",
            Self::VersionNotFound => "VersionNotFoundException",
            Self::InvalidRequest => "InvalidRequestException",
            Self::InternalServerError => "InternalServerErrorException",
        }
    }

    /// The status of the modelled shape this condition is carried by — the `httpError`
    /// of `ConflictException` (409), `NotFoundException` (404),
    /// `BadRequestException` (400) and `InternalServerErrorException` (500). Used only
    /// to keep a condition and the shape it is raised through in step.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::ApplicationAlreadyExists | Self::VersionAlreadyExists => 409,
            Self::ApplicationNotFound | Self::VersionNotFound => 404,
            Self::InvalidRequest => 400,
            Self::InternalServerError => 500,
        }
    }
}

/// The modelled 409 for `code`: "The resource already exists."
pub fn conflict(code: ErrorCode, message: impl Into<String>) -> ConflictException {
    debug_assert_eq!(code.http_status(), 409, "a ConflictException is 409");
    ConflictException {
        error_code: Some(code.name().to_string()),
        message: Some(message.into()),
    }
}

/// The modelled 404 for `code`: the resource named in the request does not exist.
pub fn not_found(code: ErrorCode, message: impl Into<String>) -> NotFoundException {
    debug_assert_eq!(code.http_status(), 404, "a NotFoundException is 404");
    NotFoundException {
        error_code: Some(code.name().to_string()),
        message: Some(message.into()),
    }
}

/// The modelled 400: "One of the parameters in the request is invalid." Every request
/// this service refuses as invalid raises the one condition for it,
/// [`ErrorCode::InvalidRequest`], and says in `message` which member is wrong and why.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        error_code: Some(ErrorCode::InvalidRequest.name().to_string()),
        message: Some(message.into()),
    }
}

/// The modelled 500, for a request the emulator could not serve at all.
pub fn internal_server_error(message: impl Into<String>) -> InternalServerErrorException {
    InternalServerErrorException {
        error_code: Some(ErrorCode::InternalServerError.name().to_string()),
        message: Some(message.into()),
    }
}

/// The wording a 404 carries for an application the catalogue does not hold, naming the
/// `ApplicationId` the caller asked for.
pub fn application_not_found_message(application_id: &str) -> String {
    format!("Application with id {application_id} could not be found.")
}

/// The documented ceiling on `Labels`: "Maximum number of labels: 10"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="Labels")`).
pub const MAX_LABELS: usize = 10;

// ---------------------------------------------------------------------------
// The application metadata members, validated in ONE place.
//
// `Name`, `Author`, `Description` and `Labels` are members of BOTH
// `CreateApplication` and `UpdateApplication` (`Name` only of the create), so the
// bounds each one carries are stated here once and both handlers refuse by the same
// rule rather than keeping two copies that can drift.
//
// WHAT IS ENFORCED: the model constrains none of these members — every one targets
// `__string`/`__listOf__string` with no `@length` and no `@pattern`
// (`smithy_query(query="operation", operation="CreateApplication")`) — so the only
// bounds there are to apply are the ones the member's own documentation states, and of
// those a CHARACTER PATTERN is applied only where the value is an identifier the
// service builds other identifiers from. `Name` is such a value: it is the resource
// segment of the `ApplicationId` ARN ([`application_arn`]) and the catalogue's
// uniqueness key ([`name_key`]), so its documented character class is enforced.
// `Labels` are the discovery tokens a search matches on, and their documented class is
// the token alphabet, so it is enforced too. `Author` and `Description` are free-form
// human text that is only stored and echoed: the service accepts any text within the
// documented length — an `Author` carrying a space, say — so the lowercase-and-hyphen
// pattern that appears in the member's prose is NOT a refusal here.
// ---------------------------------------------------------------------------

/// `Name`: "Minimum length=1. Maximum length=140 Pattern: \"[a-zA-Z0-9\\-]+\""
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="Name")`).
///
/// Returns the refusal message the calling operation renders as its own modelled
/// invalid-parameter error.
pub fn validate_name(name: &str) -> Result<(), String> {
    bounded_length("Name", name, 1, 140)?;
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("Name must satisfy pattern [a-zA-Z0-9\\-]+: {name}"));
    }
    Ok(())
}

/// `Author`: "Minimum length=1. Maximum length=127"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="Author")`).
///
/// Length only: the author is attribution text the service stores and echoes, and the
/// model declares no constraint on it. See the note above this group.
pub fn validate_author(author: &str) -> Result<(), String> {
    bounded_length("Author", author, 1, 127)
}

/// `Description`: "Minimum length=1. Maximum length=256"
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="Description")`).
pub fn validate_description(description: &str) -> Result<(), String> {
    bounded_length("Description", description, 1, 256)
}

/// `Labels`: "Minimum length=1. Maximum length=127. Maximum number of labels: 10
/// Pattern: \"^[a-zA-Z0-9+\\-_:\\/@]+$\""
/// (`smithy_query(query="doc", shape="CreateApplicationRequest", member="Labels")`).
pub fn validate_labels(labels: &[String]) -> Result<(), String> {
    if labels.len() > MAX_LABELS {
        return Err(format!(
            "Labels must have at most {MAX_LABELS} entries, got {}",
            labels.len()
        ));
    }
    for label in labels {
        bounded_length("Labels", label, 1, 127)?;
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | ':' | '/' | '@'))
        {
            return Err(format!(
                "Labels must satisfy pattern ^[a-zA-Z0-9+\\-_:\\/@]+$: {label}"
            ));
        }
    }
    Ok(())
}

/// The documented minimum/maximum length of one member, in characters.
pub fn bounded_length(member: &str, value: &str, min: usize, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length < min || length > max {
        return Err(format!(
            "{member} must have length between {min} and {max}, got {length}"
        ));
    }
    Ok(())
}

/// The documented maximum size of an inline body, in bytes — checked before the body is
/// retained, because the store holds it for the life of the record.
pub fn bounded_bytes(member: &str, value: Option<&String>, max: usize) -> Result<(), String> {
    if let Some(value) = value {
        if value.len() > max {
            return Err(format!(
                "{member} must be at most {max} bytes, got {}",
                value.len()
            ));
        }
    }
    Ok(())
}

/// Whether `semantic_version` is a semantic version, which is what the model says the
/// member carries: "The semantic version of the application: <https://semver.org/>"
/// (`smithy_query(query="doc", shape="Version", member="SemanticVersion")`).
///
/// The grammar is semver 2.0.0 §2 and §9: three dot-separated numeric identifiers with
/// no leading zeros, then an optional `-prerelease` and an optional `+build`, each a
/// dot-separated list of non-empty identifiers of `[0-9A-Za-z-]` (a numeric prerelease
/// identifier also carries no leading zero).
pub fn is_semantic_version(semantic_version: &str) -> bool {
    let (core, rest) = match semantic_version.find(['-', '+']) {
        Some(at) => (&semantic_version[..at], &semantic_version[at..]),
        None => (semantic_version, ""),
    };
    let core: Vec<&str> = core.split('.').collect();
    if core.len() != 3 || !core.iter().all(|field| is_numeric_identifier(field)) {
        return false;
    }

    let (prerelease, build) = match rest.strip_prefix('-') {
        Some(after) => match after.find('+') {
            Some(at) => (Some(&after[..at]), Some(&after[at + 1..])),
            None => (Some(after), None),
        },
        None => (None, rest.strip_prefix('+')),
    };
    if let Some(prerelease) = prerelease {
        let identifiers: Vec<&str> = prerelease.split('.').collect();
        if !identifiers.iter().all(|identifier| {
            is_alphanumeric_identifier(identifier)
                && (!identifier.chars().all(|c| c.is_ascii_digit())
                    || is_numeric_identifier(identifier))
        }) {
            return false;
        }
    }
    if let Some(build) = build {
        if !build.split('.').all(is_alphanumeric_identifier) {
            return false;
        }
    }
    true
}

/// A numeric identifier: digits only, and no leading zero unless it IS zero.
fn is_numeric_identifier(field: &str) -> bool {
    !field.is_empty()
        && field.chars().all(|c| c.is_ascii_digit())
        && (field == "0" || !field.starts_with('0'))
}

/// A prerelease or build identifier: non-empty, `[0-9A-Za-z-]` only.
fn is_alphanumeric_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether `candidate` has higher semantic-version precedence than `current`.
///
/// Precedence is semver 2.0.0 §11: compare major, minor then patch numerically; a
/// version WITH a prerelease has lower precedence than the same version without one;
/// otherwise compare prerelease identifiers left to right, numerically where both are
/// numeric and lexically otherwise, with a longer identifier list winning a tie. Build
/// metadata is ignored. A value that is not a semantic version compares by its own text,
/// below every version that is one, so an unparseable stored version can never be
/// reported as the latest ahead of a real one.
fn takes_precedence_over(candidate: &str, current: &str) -> bool {
    precedence_key(candidate) > precedence_key(current)
}

/// A total, orderable key for one version's precedence.
fn precedence_key(semantic_version: &str) -> (bool, [u64; 3], bool, Vec<PrereleaseField>, String) {
    let well_formed = is_semantic_version(semantic_version);
    let (core, rest) = match semantic_version.find(['-', '+']) {
        Some(at) => (&semantic_version[..at], &semantic_version[at..]),
        None => (semantic_version, ""),
    };
    let mut numbers = [0u64; 3];
    for (slot, field) in numbers.iter_mut().zip(core.split('.')) {
        *slot = field.parse().unwrap_or(0);
    }
    let prerelease = rest
        .strip_prefix('-')
        .map(|after| after.split('+').next().unwrap_or(after));
    let fields: Vec<PrereleaseField> = prerelease
        .map(|prerelease| prerelease.split('.').map(PrereleaseField::of).collect())
        .unwrap_or_default();
    (
        well_formed,
        numbers,
        // No prerelease outranks any prerelease, so `true` (absent) sorts above.
        prerelease.is_none(),
        fields,
        semantic_version.to_string(),
    )
}

/// One prerelease identifier, ordered the way semver §11 orders it: a numeric identifier
/// below an alphanumeric one, numerics compared as numbers and the rest as text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PrereleaseField {
    Numeric(u64),
    Alphanumeric(String),
}

impl PrereleaseField {
    fn of(identifier: &str) -> Self {
        match identifier.parse::<u64>() {
            Ok(number) if is_numeric_identifier(identifier) => Self::Numeric(number),
            _ => Self::Alphanumeric(identifier.to_string()),
        }
    }
}

/// Now, as the model's `CreationTime` (`__string`) carries it: ISO-8601 in UTC.
pub fn now_iso8601() -> String {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Why a `nextToken` could not be used. Both answer HTTP 400: a token is either this
/// service's own or nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenError {
    /// Not a token this service minted.
    Malformed,
    /// A well-formed token naming an item the collection no longer holds.
    Unknown,
}

impl PageTokenError {
    /// The refusal wording, naming the `nextToken` member the caller must fix.
    pub fn message(self) -> &'static str {
        match self {
            Self::Malformed => "The value of nextToken is not a valid pagination token.",
            Self::Unknown => {
                "The item named by nextToken no longer exists. Restart the walk without a \
                 nextToken."
            }
        }
    }
}

/// One page of an ordered collection, plus the token for the page after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The keys on this page, in the collection's own order.
    pub keys: Vec<String>,
    /// The token that starts the next page, absent on the final page.
    pub next_token: Option<String>,
}

/// The page of `items` a (`max_items`, `next_token`) request selects.
///
/// The walk is defined over the map's own key order, and a token names the first key of
/// the page it starts, so consecutive pages visit every item exactly once and the final
/// page carries no token.
pub fn page_of(
    keys: impl Iterator<Item = String>,
    max_items: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page, PageTokenError> {
    let limit = max_items
        .unwrap_or(MAX_ITEMS_LIMIT)
        .clamp(1, MAX_ITEMS_LIMIT) as usize;
    let all: Vec<String> = keys.collect();

    let start = match next_token {
        None => 0,
        Some(token) => {
            let key = decode_page_token(token).ok_or(PageTokenError::Malformed)?;
            all.iter()
                .position(|candidate| *candidate == key)
                .ok_or(PageTokenError::Unknown)?
        }
    };

    let end = start.saturating_add(limit).min(all.len());
    Ok(Page {
        keys: all[start..end].to_vec(),
        next_token: all.get(end).map(|key| encode_page_token(key)),
    })
}

/// The opaque token naming `key`. Opaque because a caller must not build one: it is
/// hex so the whole token is URL-safe whatever the key contains.
fn encode_page_token(key: &str) -> String {
    ::hex::encode(key)
}

/// The key a token names, or `None` when the token is not one this service minted.
fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Why a supplied template, or an inline artefact body, was refused.
///
/// Every bound it names is a documented AWS maximum, so the refusal states the rule
/// rather than an emulator detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateRefusal {
    /// Bigger than [`MAX_TEMPLATE_BYTES`].
    TemplateTooLarge,
    /// More than [`MAX_TEMPLATE_PARAMETERS`] entries in `Parameters`.
    TooManyParameters,
    /// More than [`MAX_TEMPLATE_RESOURCES`] entries in `Resources`.
    TooManyResources,
}

impl TemplateRefusal {
    /// The refusal wording, naming the member and the bound it exceeded.
    pub fn message(self) -> String {
        match self {
            Self::TemplateTooLarge => format!(
                "TemplateBody must be at most {MAX_TEMPLATE_BYTES} bytes, the maximum size of a \
                 packaged AWS CloudFormation template."
            ),
            Self::TooManyParameters => format!(
                "TemplateBody must declare at most {MAX_TEMPLATE_PARAMETERS} parameters, the \
                 maximum an AWS CloudFormation template may declare."
            ),
            Self::TooManyResources => format!(
                "TemplateBody must declare at most {MAX_TEMPLATE_RESOURCES} resources, the \
                 maximum an AWS CloudFormation template may declare."
            ),
        }
    }
}

/// The parameters a supplied template declares, as modelled `ParameterDefinition`s.
///
/// CloudFormation's template anatomy — which AWS SAM extends — puts them in the
/// top-level `Parameters` object: one entry per parameter name, carrying `Type`,
/// `Default`, `Description`, `AllowedValues`, `AllowedPattern`, `MinLength`/`MaxLength`,
/// `MinValue`/`MaxValue`, `ConstraintDescription` and `NoEcho`. A parameter's
/// `ReferencedByResources` is the set of resources in the template's `Resources` object
/// that reference it, which a template states as `{"Ref": "<name>"}` or as `${name}`
/// inside an `Fn::Sub`.
///
/// Only a template the caller supplied INLINE and in JSON form is read: an emulator
/// makes no outbound connections, so a template named only by `TemplateUrl` is never
/// fetched, and a template in YAML is not parsed (no YAML reader is available here).
/// Either way the answer is an empty list — never a guess.
///
/// # The work is bounded before it is done
///
/// The body is caller-supplied, so its size and shape are refused against the
/// documented maxima ([`MAX_TEMPLATE_BYTES`], [`MAX_TEMPLATE_PARAMETERS`],
/// [`MAX_TEMPLATE_RESOURCES`]) BEFORE anything is walked, and `Resources` is walked
/// exactly ONCE — building an identifier -> referencing-resources index that every
/// parameter then looks itself up in. The cost is linear in the template
/// (`O(resources + parameters)`), not the product of the two, so no accepted template
/// can occupy a worker for longer than its own size warrants. Recursion depth is
/// bounded by `serde_json`'s own nesting limit, which rejects the body before this runs.
///
/// Both the parameters and each one's referencing resources come back sorted by name, so
/// two reads of one stored template answer identically.
pub fn parse_template(
    template_body: Option<&String>,
) -> Result<Vec<ParameterDefinition>, TemplateRefusal> {
    let Some(body) = template_body else {
        return Ok(Vec::new());
    };
    if body.len() > MAX_TEMPLATE_BYTES {
        return Err(TemplateRefusal::TemplateTooLarge);
    }
    let Ok(template) = ::serde_json::from_str::<::serde_json::Value>(body) else {
        return Ok(Vec::new());
    };
    let Some(parameters) = template.get("Parameters").and_then(|p| p.as_object()) else {
        return Ok(Vec::new());
    };
    if parameters.len() > MAX_TEMPLATE_PARAMETERS {
        return Err(TemplateRefusal::TooManyParameters);
    }
    let resources = template.get("Resources").and_then(|r| r.as_object());
    if resources.is_some_and(|resources| resources.len() > MAX_TEMPLATE_RESOURCES) {
        return Err(TemplateRefusal::TooManyResources);
    }

    let referencing = reference_index(resources);
    let mut definitions: Vec<ParameterDefinition> = parameters
        .iter()
        .map(|(name, spec)| ParameterDefinition {
            allowed_pattern: string_field(spec, "AllowedPattern"),
            allowed_values: string_list_field(spec, "AllowedValues"),
            constraint_description: string_field(spec, "ConstraintDescription"),
            default_value: scalar_field(spec, "Default"),
            description: string_field(spec, "Description"),
            max_length: integer_field(spec, "MaxLength"),
            max_value: integer_field(spec, "MaxValue"),
            min_length: integer_field(spec, "MinLength"),
            min_value: integer_field(spec, "MinValue"),
            name: name.clone(),
            no_echo: spec.get("NoEcho").and_then(boolean_of),
            referenced_by_resources: referencing.get(name).cloned().unwrap_or_default(),
            r#type: string_field(spec, "Type"),
        })
        .collect();
    definitions.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(definitions)
}

/// One pass over `Resources`: for each identifier a resource references, the names of
/// the resources that reference it, sorted.
fn reference_index(
    resources: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> BTreeMap<String, Vec<String>> {
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(resources) = resources else {
        return index;
    };
    for (resource, body) in resources {
        let mut referenced: BTreeSet<String> = BTreeSet::new();
        collect_references(body, &mut referenced);
        for identifier in referenced {
            index.entry(identifier).or_default().push(resource.clone());
        }
    }
    for referencing in index.values_mut() {
        referencing.sort();
    }
    index
}

/// Every identifier `value` references, as `{"Ref": "<name>"}` or as `${name}` inside an
/// `Fn::Sub`.
///
/// An identifier that names something other than a parameter (a resource, a
/// pseudo-parameter) is collected too and simply matches no parameter, which keeps this
/// one walk independent of the parameter list.
fn collect_references(value: &::serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        ::serde_json::Value::Object(fields) => {
            if let Some(name) = fields.get("Ref").and_then(|r| r.as_str()) {
                out.insert(name.to_string());
            }
            if let Some(substitution) = fields.get("Fn::Sub") {
                collect_substitutions(substitution, out);
            }
            for nested in fields.values() {
                collect_references(nested, out);
            }
        }
        ::serde_json::Value::Array(elements) => {
            for nested in elements {
                collect_references(nested, out);
            }
        }
        _ => {}
    }
}

/// The identifiers an `Fn::Sub` — a string, or `[string, {bindings}]` — interpolates as
/// `${name}`.
fn collect_substitutions(substitution: &::serde_json::Value, out: &mut BTreeSet<String>) {
    let texts: Vec<&str> = match substitution {
        ::serde_json::Value::String(text) => vec![text.as_str()],
        ::serde_json::Value::Array(elements) => {
            elements.iter().filter_map(|e| e.as_str()).collect()
        }
        _ => Vec::new(),
    };
    for text in texts {
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                break;
            };
            // `${!Literal}` is the documented escape for a literal `${`, and names
            // nothing.
            let identifier = after[..end].trim();
            if !identifier.starts_with('!') {
                out.insert(identifier.to_string());
            }
            rest = &after[end + 1..];
        }
    }
}

fn string_field(spec: &::serde_json::Value, field: &str) -> Option<String> {
    spec.get(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// A template's list-valued field (`AllowedValues`), as the strings the modelled
/// member carries; a non-string element is rendered the way a scalar field is.
fn string_list_field(spec: &::serde_json::Value, field: &str) -> Option<Vec<String>> {
    let elements = spec.get(field)?.as_array()?;
    Some(
        elements
            .iter()
            .filter_map(|element| match element {
                ::serde_json::Value::String(text) => Some(text.clone()),
                ::serde_json::Value::Number(number) => Some(number.to_string()),
                ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
                _ => None,
            })
            .collect(),
    )
}

/// A parameter's `Default` is of the parameter's own type, so a number or a boolean is
/// rendered as the string the modelled `DefaultValue` carries.
fn scalar_field(spec: &::serde_json::Value, field: &str) -> Option<String> {
    match spec.get(field)? {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn integer_field(spec: &::serde_json::Value, field: &str) -> Option<i32> {
    spec.get(field)
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok())
}

/// CloudFormation accepts a template's booleans as JSON booleans and as the strings
/// `"true"`/`"false"`.
fn boolean_of(value: &::serde_json::Value) -> Option<bool> {
    match value {
        ::serde_json::Value::Bool(flag) => Some(*flag),
        ::serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// Per-service state for com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState {
    /// Every application, keyed by its `ApplicationId` ARN and ordered by it so list
    /// paging is stable. Bounded by [`MAX_STORED_APPLICATIONS`].
    pub applications: BTreeMap<String, Application>,
    /// [`name_key`] -> `ApplicationId`, which is what makes a second
    /// `CreateApplication` with a name already in that region's catalogue a 409.
    pub name_index: HashMap<String, String>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState>)
}
