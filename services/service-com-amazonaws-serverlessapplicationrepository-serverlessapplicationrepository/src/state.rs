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
//! A *prepared CloudFormation template* of one version lives in its own map keyed by the
//! `TemplateId` that minted it ([`CloudFormationTemplate`]), because it is a resource with
//! a life of its own: it is prepared from a published version and expires
//! [`TEMPLATE_LIFETIME`] later, and both template operations report it out of that one
//! record.
//!
//! The bodies a publisher supplies INLINE are retained on those same records, and the
//! locator this service announces for one ([`minted_artifact_url`]) is an address on the
//! emulator's own endpoint that `crate::service` serves them at — a `TemplateUrl` is a
//! link a CloudFormation client deploys from, so it has to be fetchable.
//!
//! An application also carries its permission policy — the `policy_statements` list
//! `PutApplicationPolicy` replaces and `GetApplicationPolicy` reports — so a policy is
//! part of the application record and goes away with it. Room is still deliberately left
//! for the operations later issues own: `UnshareApplication` and
//! `ListApplicationDependencies` declare no state of their own here yet, and they still
//! answer 501. `CreateCloudFormationChangeSet` needs none either: it reads the published
//! version's own template out of this store (or the prepared template a `TemplateId`
//! names), and the change set itself lives in AWS CloudFormation, which it calls in
//! process.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ApplicationPolicyStatement, ApplicationSummary, BadRequestException, ConflictException,
    InternalServerErrorException, NotFoundException, ParameterDefinition, Version, VersionSummary,
};

/// Reads an inline `TemplateBody` — JSON or YAML — into the one tree everything below
/// derives a template's members from.
mod template_document;

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
    /// Every version of this application, keyed by its semantic version so a lookup by
    /// the `SemanticVersion` label is exact and the map's own order is stable. The order
    /// a version COLLECTION is reported in is semantic-version precedence, which the
    /// text of a version does not give — see [`Application::versions_by_precedence`].
    pub versions: BTreeMap<String, AppVersion>,
    /// The semantic version with the highest precedence of those stored, which
    /// `GetApplication` reports when the caller names none.
    pub latest_version: Option<String>,
    /// This application's permission policy, in the order the last
    /// `PutApplicationPolicy` listed its statements. Empty for an application whose
    /// policy was never set, or whose last put carried no statements: a policy is
    /// REPLACED by each put, never merged into.
    pub policy_statements: Vec<PolicyStatement>,
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

    /// This application's semantic versions in ASCENDING semantic-version precedence,
    /// which is the order a version collection is reported in.
    ///
    /// Precedence is semver 2.0.0 §11 ([`precedence_key`]) — the same ordering that
    /// decides which stored version is [`Application::latest`], so one application cannot
    /// report a walk whose last entry is not the version it calls its latest. The text of
    /// a semantic version does not give that order: `1.10.0` sorts below `1.9.0` as text
    /// and above it as a version, and a prerelease sorts above the release it precedes as
    /// text and below it as a version.
    ///
    /// The key is computed once per version rather than once per comparison, so the walk
    /// costs one parse of each stored version.
    pub fn versions_by_precedence(&self) -> Vec<String> {
        let mut semantic_versions: Vec<String> = self.versions.keys().cloned().collect();
        semantic_versions.sort_by_cached_key(|semantic_version| precedence_key(semantic_version));
        semantic_versions
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

    /// This application's policy projected into the modelled statements both
    /// `PutApplicationPolicy` and `GetApplicationPolicy` answer with, in the stored
    /// order — so the two operations cannot render one stored policy differently.
    pub fn policy(&self) -> Vec<ApplicationPolicyStatement> {
        self.policy_statements
            .iter()
            .map(PolicyStatement::statement)
            .collect()
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
    /// The capabilities a deployment of this version must acknowledge, derived from the
    /// resources its template declares when the version is PUBLISHED (see
    /// [`required_capabilities_of`]) and stored with it: a later publish never rewrites an
    /// already-published version's list, and a read does not re-derive it.
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

/// One statement of an application's permission policy: the record
/// `PutApplicationPolicy` writes and `GetApplicationPolicy` reads back.
///
/// Stored rather than kept as the modelled shape because a statement always HAS a
/// `StatementId` once it is stored — the model documents it as "A unique ID for the
/// statement" — while the request's member is optional, so the id is resolved once, on
/// the way in ([`validate_policy_statements`]), and every read answers with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyStatement {
    /// The statement's unique id: the caller's own `StatementId`, or the one minted for
    /// a statement that named none.
    pub statement_id: String,
    /// The application permissions this statement grants; see [`APPLICATION_PERMISSIONS`].
    pub actions: Vec<String>,
    /// The AWS account ids this statement grants them to, or `*`.
    pub principals: Vec<String>,
    /// The AWS organization ids this statement is conditioned on, empty when it names
    /// none.
    pub principal_org_ids: Vec<String>,
}

impl PolicyStatement {
    /// This record projected into the modelled `ApplicationPolicyStatement`.
    ///
    /// `PrincipalOrgIDs` is rendered as the empty LIST for a statement that names no
    /// organization, the same way [`Application::summary`] renders a label-less
    /// application's `Labels`: an empty list is the member's value, not its absence, so a
    /// client reading `statement["principalOrgIDs"]` never has to guard for the member
    /// being missing.
    pub fn statement(&self) -> ApplicationPolicyStatement {
        ApplicationPolicyStatement {
            actions: self.actions.clone(),
            principal_org_i_ds: Some(self.principal_org_ids.clone()),
            principals: self.principals.clone(),
            statement_id: Some(self.statement_id.clone()),
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

    /// The file name the minted locator ends in, and the name the document plane reads
    /// back out of it ([`Artifact::from_file_name`]).
    const fn file_name(self) -> &'static str {
        match self {
            Self::Template => "template.yaml",
            Self::License => "license.txt",
            Self::Readme => "readme.md",
        }
    }

    /// Which artefact the last segment of an announced locator names, or `None` for a
    /// name this service never minted. The inverse of [`Artifact::file_name`], so the
    /// address announced and the address served cannot drift apart.
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        [Self::Template, Self::License, Self::Readme]
            .into_iter()
            .find(|artifact| artifact.file_name() == file_name)
    }

    /// The media type the document plane serves `body` — the publisher's own retained
    /// bytes — as.
    ///
    /// A template is published as a packaged CloudFormation/SAM document, which may be
    /// either JSON or YAML, and this emulator retains exactly the bytes it was handed:
    /// the type is therefore read off those bytes rather than assumed, and the JSON a
    /// publisher sent is labelled JSON (which a YAML reader parses too, YAML being a
    /// superset). A license is plain text and a readme is Markdown, the forms the two
    /// members document. The parse is bounded by [`MAX_TEMPLATE_BYTES`], which the
    /// publish already held the body to.
    pub fn content_type(self, body: &str) -> &'static str {
        match self {
            Self::Template => {
                if ::serde_json::from_str::<::serde_json::Value>(body).is_ok() {
                    "application/json"
                } else {
                    "application/x-yaml"
                }
            }
            Self::License => "text/plain; charset=utf-8",
            Self::Readme => "text/markdown; charset=utf-8",
        }
    }
}

// ---------------------------------------------------------------------------
// The artifact document plane.
//
// `Version$TemplateUrl` is "A link to the template that can be used to deploy the
// application using AWS CloudFormation", and both CloudFormation template operations
// report that same link for the template they prepare: the member is a PROMISE that a
// caller can FOLLOW it, because fetching it and deploying the bytes is what a
// CloudFormation client does with it. The locator this service mints for a body a
// publisher supplied inline is therefore an address on the emulator's OWN endpoint,
// under a path space that names no modelled operation, and `crate::service` serves the
// retained body there — so the address announced and the address served are one address.
// ---------------------------------------------------------------------------

/// The path space the artifact document plane is served under.
///
/// The prefix names this service and this kind of object, so it cannot collide with
/// another service's modelled URI space, and it is not in this service's own model —
/// which is what routes a fetch of it to [`crate::service`]'s document seam.
pub const ARTIFACT_PATH_PREFIX: &str = "/serverlessrepo-artifacts";

/// The link reported for content the caller supplied INLINE: a PRESIGNED address on the
/// emulator's own endpoint that
/// [`ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState::artifact_body`]
/// serves the retained bytes at.
///
/// The real service copies an inline body into an S3 object it owns and reports a
/// presigned download URL for that object — a link whose query string carries the
/// signature, so whoever holds the link can fetch the document with no credential of their
/// own, until it expires. This emulator holds the body in its own state and serves it
/// itself, so the locator carries the same four facts an S3 key would — the region and the
/// account that published it, the application, and the version for an artefact a version
/// published — under `host`, the endpoint the publishing request arrived on, and the same
/// query-string signature, minted over that address by [`presigned_artifact_url`] and
/// honoured by the document plane in `crate::service`. A literal AWS hostname would send
/// the next client out of the emulator entirely, so it is never announced here.
pub fn minted_artifact_url(
    host: &str,
    region: &str,
    application_name: &str,
    semantic_version: Option<&str>,
    artifact: Artifact,
) -> String {
    let address = artifact_address(host, region, application_name, semantic_version, artifact);
    presigned_artifact_url(&address, region, SystemTime::now(), ARTIFACT_URL_LIFETIME)
}

/// The bare address of one stored artefact, without the signature
/// [`minted_artifact_url`] announces it with: the path
/// [`parse_artifact_path`] reads back.
fn artifact_address(
    host: &str,
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
        "https://{host}{ARTIFACT_PATH_PREFIX}/{region}/{account}/{path}/{}",
        artifact.file_name()
    )
}

/// Which stored artefact a request to the document plane addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactTarget {
    /// The region the addressed application is catalogued in.
    pub region: String,
    /// The account that published it, which is the one account this emulator serves.
    pub account_id: String,
    pub application_name: String,
    /// The version that published the body, for an artefact a VERSION publishes (a
    /// template); `None` for one the application carries (a license, a readme).
    pub semantic_version: Option<String>,
    pub artifact: Artifact,
}

/// The artefact a document-plane request path addresses, or `None` when the path is not
/// one [`minted_artifact_url`] announces.
///
/// Every segment is read back out of the announced locator, so a fetch resolves by what
/// the URL NAMES rather than by string equality with a stored field: the record is found by
/// the region, account, application and version the address carries, whatever endpoint of
/// this emulator the locator was announced on.
pub fn parse_artifact_path(path: &str) -> Option<ArtifactTarget> {
    let rest = path.strip_prefix(ARTIFACT_PATH_PREFIX)?.strip_prefix('/')?;
    let segments: Vec<&str> = rest.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let (region, account_id, application_name, semantic_version, file_name) =
        match segments.as_slice() {
            [region, account_id, name, file_name] => (*region, *account_id, *name, None, *file_name),
            [region, account_id, name, semantic_version, file_name] => (
                *region,
                *account_id,
                *name,
                Some((*semantic_version).to_string()),
                *file_name,
            ),
            _ => return None,
        };
    Some(ArtifactTarget {
        region: region.to_string(),
        account_id: account_id.to_string(),
        application_name: application_name.to_string(),
        semantic_version,
        artifact: Artifact::from_file_name(file_name)?,
    })
}

/// Whether `path` addresses the document plane: [`ARTIFACT_PATH_PREFIX`] followed by a
/// path separator, or by nothing at all.
///
/// The separator is required so that a sibling path space whose name merely STARTS with the
/// prefix is not claimed by this service.
pub fn addresses_artifact_plane(path: &str) -> bool {
    match path.strip_prefix(ARTIFACT_PATH_PREFIX) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// The region a document-plane path is addressed in: the first segment under
/// [`ARTIFACT_PATH_PREFIX`], which every locator carries and whose scope the locator's
/// signature was minted under. `None` for a path that carries no segment at all.
pub fn artifact_path_region(path: &str) -> Option<&str> {
    path.strip_prefix(ARTIFACT_PATH_PREFIX)?
        .strip_prefix('/')?
        .split('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

/// The link stored for a `*Body`/`*Url` pair: the caller's URL, or a minted locator
/// for a body they supplied inline, or `None` when they supplied neither.
pub fn resolve_artifact_url(
    body: Option<&String>,
    url: Option<&String>,
    host: &str,
    region: &str,
    application_name: &str,
    semantic_version: Option<&str>,
    artifact: Artifact,
) -> Option<String> {
    match (body, url) {
        (_, Some(url)) => Some(url.clone()),
        (Some(_), None) => Some(minted_artifact_url(
            host,
            region,
            application_name,
            semantic_version,
            artifact,
        )),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------------------
// The signature a published locator carries.
//
// Every `*Url` this service announces for a body it STORES is a presigned download
// address: AWS reports a presigned S3 URL for one, whose query string carries the
// signature that makes the document fetchable with no credential of the holder's own,
// until it expires. The signature here is computed by the maintained `aws-sigv4` crate
// over the very address being announced, and `crate::service`'s document plane verifies
// it before serving a byte — so the link this service hands out is a link that is
// honoured because it was signed, and a tampered or expired one is refused.
//
// The framework's own request verifier is deliberately NOT the verifier here: it checks a
// CALLER's SigV4 against the credentials the emulator minted and holds in its credential
// store, while a locator below is signed with this document plane's own ephemeral key,
// which that store neither holds nor should. What the two share is the computation, and
// that is the `aws-sigv4` crate both use rather than a signing routine written here.
//
// A locator is honoured as it was ANNOUNCED, field for field: a client that re-encodes the
// query it was handed presents a different canonical request, which is a different
// signature, exactly as it would be to the object store this plane stands in for.
// ---------------------------------------------------------------------------

/// How long a locator this service announces stays followable.
///
/// One week, the longest lifetime SigV4 query-string signing allows ("the maximum
/// expiration for a presigned URL is one week from the time of creation"), so a locator a
/// publish announced outlives every read of the record that carries it rather than
/// expiring under a client that was handed it.
pub const ARTIFACT_URL_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// `address` with the query-string signature that makes it a download link anyone holding
/// it can follow: `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`,
/// `X-Amz-SignedHeaders` and `X-Amz-Signature`, as `aws-sigv4` computes them for a `GET`
/// of that address over the host it names.
///
/// `signed_at` and `lifetime` are parameters rather than read from the clock so that the
/// instant a locator was signed at is the caller's: [`minted_artifact_url`] signs at the
/// publishing instant, and [`artifact_signature`] re-signs at the instant the presented
/// locator states, which is what lets one computation both mint and verify.
///
/// An address that is not an absolute URL cannot be signed; it is announced unsigned, and
/// the document plane then refuses it rather than serving a document from a link whose
/// signature nothing computed.
pub fn presigned_artifact_url(
    address: &str,
    region: &str,
    signed_at: SystemTime,
    lifetime: Duration,
) -> String {
    presign(address, region, signed_at, lifetime).unwrap_or_else(|| {
        ::tracing::warn!(%address, "a published artifact address could not be signed");
        address.to_string()
    })
}

/// What a document-plane request's own locator is: the verdict `crate::service` serves or
/// refuses on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactSignature {
    /// Signed by this service, over this address, and not expired yet.
    Honoured,
    /// No query-string signature at all.
    Absent,
    /// Signed by this service, but the expiry the locator states has passed.
    Expired,
    /// A signature that does not verify over the address it was presented on: a field was
    /// rewritten, the address was, or it was signed by something else.
    Invalid,
}

/// Whether the locator a document-plane request presents is one this service signed and
/// still honours.
///
/// `target` is the request target as it arrived (path and query), `host` the endpoint it
/// arrived on and `region` the one the addressed application is catalogued in — the facts
/// the signature covers. The locator is honoured when it is FIELD FOR FIELD the one this
/// service would mint over that address, at the instant and for the lifetime the locator
/// itself states: the signing fields are stripped, the rest of the query travels into the
/// address being re-signed, and what comes back is compared with what was presented. So a
/// rewritten scope, algorithm, signed-header list, timestamp, lifetime or signature, a
/// field of the client's own, another endpoint host and another document's address are each
/// a locator this service did not mint. A lifetime longer than [`ARTIFACT_URL_LIFETIME`] is
/// not one it ever mints, and neither is a target carrying more than
/// [`MAX_LOCATOR_FIELDS`] query fields — the count is capped before any per-field work, so
/// what this check costs does not depend on how many fields a caller sends.
///
/// The locator is signed for the download of a document, so a `HEAD` of it — which reports
/// what a `GET` of the same address would answer ABOUT that document — is checked against
/// the same signature.
pub fn artifact_signature(
    host: &str,
    target: &str,
    region: &str,
    now: SystemTime,
) -> ArtifactSignature {
    let Some((path, query)) = target.split_once('?') else {
        return ArtifactSignature::Absent;
    };
    // The field count is the one part of this check a CALLER chooses, so it is capped
    // before the fields are even collected: past the cap the target is one this service
    // never announced, whatever the fields say.
    if query.split('&').count() > MAX_LOCATOR_FIELDS {
        return ArtifactSignature::Invalid;
    }
    let presented = query_fields(query);
    if !presented
        .iter()
        .any(|(name, _value)| *name == "X-Amz-Signature")
    {
        return ArtifactSignature::Absent;
    }
    let stated = |name: &str| {
        presented
            .iter()
            .find_map(|(field, value)| (*field == name).then_some(*value))
    };
    let (Some(signed_at), Some(expires)) = (stated("X-Amz-Date"), stated("X-Amz-Expires")) else {
        return ArtifactSignature::Invalid;
    };
    let (Some(signed_at), Ok(lifetime)) = (signing_instant(signed_at), expires.parse::<u64>())
    else {
        return ArtifactSignature::Invalid;
    };
    let lifetime = Duration::from_secs(lifetime);
    if lifetime > ARTIFACT_URL_LIFETIME {
        return ArtifactSignature::Invalid;
    }
    let Some(expires_at) = signed_at.checked_add(lifetime) else {
        return ArtifactSignature::Invalid;
    };
    // Every field the signing adds back is dropped; everything else is part of the address
    // the signature covers, so a field a client appended of its own travels into it.
    let carried: Vec<String> = presented
        .iter()
        .filter(|(name, _value)| !name.starts_with("X-Amz-"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let address = if carried.is_empty() {
        format!("https://{host}{path}")
    } else {
        format!("https://{host}{path}?{}", carried.join("&"))
    };
    let Some(minted) = presign(&address, region, signed_at, lifetime) else {
        return ArtifactSignature::Invalid;
    };
    let Some((_address, minted_query)) = minted.split_once('?') else {
        return ArtifactSignature::Invalid;
    };
    if !same_locator(&query_fields(minted_query), &presented) {
        return ArtifactSignature::Invalid;
    }
    // Expiry is answered only for a locator that IS this service's: one it never minted is
    // not expired, it is invalid.
    if now >= expires_at {
        ArtifactSignature::Expired
    } else {
        ArtifactSignature::Honoured
    }
}

/// The most query fields a document-plane request target may carry.
///
/// A locator this service mints carries exactly the signing fields and none of a client's
/// own, so this is those plus a little room: a target carrying more is one this service did
/// not announce, and refusing it on the count alone is what keeps the verification's work
/// independent of how many fields a caller sends.
const MAX_LOCATOR_FIELDS: usize = 8;

/// The query fields of `query`, in the order they arrived, as (name, value) pairs.
fn query_fields(query: &str) -> Vec<(&str, &str)> {
    query
        .split('&')
        .filter(|field| !field.is_empty())
        .map(|field| field.split_once('=').unwrap_or((field, "")))
        .collect()
}

/// Whether `presented` is the locator `minted` field for field: the same fields, each with
/// the same value, in any order.
///
/// The presented fields are indexed once and each minted field is looked up in that index,
/// so the comparison costs one pass over each side rather than one pass per field.
fn same_locator(minted: &[(&str, &str)], presented: &[(&str, &str)]) -> bool {
    if minted.len() != presented.len() {
        return false;
    }
    let presented: BTreeMap<&str, &str> = presented.iter().copied().collect();
    minted.iter().all(|(name, value)| {
        presented.get(name).is_some_and(|presented| {
            // The signature is the field derived from this service's own key, so how long
            // its comparison takes must not depend on how much of a presented one was
            // right.
            if *name == "X-Amz-Signature" {
                same_signature(value, presented)
            } else {
                presented == value
            }
        })
    })
}

/// The signing of one download address, shared by the mint and the verification so there
/// is one computation and no second copy of what a locator's signature covers.
///
/// `None` when the address is not an absolute URL, or when the parameters the crate needs
/// cannot be built from it.
fn presign(
    address: &str,
    region: &str,
    signed_at: SystemTime,
    lifetime: Duration,
) -> Option<String> {
    use ::aws_sigv4::http_request::{
        sign, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation,
        SigningSettings,
    };

    let authority = url_authority(address)?;
    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name(ARN_NAMESPACE)
            .access_key_id(document_plane_key_id())
            .secret_access_key(document_plane_key())
            .build()
            .into();
    let mut settings = SigningSettings::default();
    // The signature travels in the query string, which is what makes the locator
    // followable by a client that sends no `Authorization` header at all.
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(lifetime);
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(ARN_NAMESPACE)
        .settings(settings)
        .time(signed_at)
        .build()
        .ok()?;
    let signable = SignableRequest::new(
        "GET",
        address,
        ::std::iter::once(("host", authority)),
        SignableBody::UnsignedPayload,
    )
    .ok()?;
    let (instructions, _signature) = sign(signable, &params.into()).ok()?.into_parts();
    let mut request = ::http::Request::builder()
        .method("GET")
        .uri(address)
        .body(())
        .ok()?;
    instructions.apply_to_request_http1x(&mut request);
    Some(request.uri().to_string())
}

/// The key this service's document plane signs a published locator with.
///
/// Drawn once per process from [`process_seed`] and digested with `sha2`: it is the
/// plane's own signing key, not a credential of any account and not the emulator's own —
/// nothing returns it, nothing else accepts it, and it is gone when the process is, so a
/// locator one run announced is not followable against another.
fn document_plane_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| digest_of("artifact-download-signing-key"))
        .as_str()
}

/// The id of that key, as the locator's `X-Amz-Credential` names the signer: 20 uppercase
/// characters, the shape of an access key id, derived from the process seed under its own
/// domain so it identifies the signing key without revealing it.
fn document_plane_key_id() -> &'static str {
    static KEY_ID: OnceLock<String> = OnceLock::new();
    KEY_ID
        .get_or_init(|| digest_of("artifact-download-key-id")[..20].to_ascii_uppercase())
        .as_str()
}

/// This process's seed digested under `domain`, hex-rendered: one derivation, so two
/// values of this module are never the same bytes under two names.
fn digest_of(domain: &str) -> String {
    use ::sha2::Digest as _;

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(domain.as_bytes());
    ::hex::encode(digest.finalize())
}

/// The instant a locator's `X-Amz-Date` names: `YYYYMMDDTHHMMSSZ`, the basic form SigV4
/// states it in.
///
/// The fixed-width fields are re-rendered as the extended form and read by
/// `aws_smithy_types`' own date-time parser (the one [`iso8601`] renders with), so the
/// calendar arithmetic is the maintained crate's rather than this module's.
fn signing_instant(stamp: &str) -> Option<SystemTime> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    if !bytes[..8]
        .iter()
        .chain(&bytes[9..15])
        .all(u8::is_ascii_digit)
    {
        return None;
    }
    let extended = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &stamp[0..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    );
    let at = ::aws_smithy_types::DateTime::from_str(
        &extended,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(at).ok()
}

/// The authority of an absolute URL: the host, with its port when it names one.
fn url_authority(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://")?.1;
    Some(match after_scheme.find(['/', '?', '#']) {
        Some(end) => &after_scheme[..end],
        None => after_scheme,
    })
}

/// `expected == presented`, without stopping at the first byte that differs.
///
/// A signature is derived from a key this service holds, so how long the comparison takes
/// must not depend on how much of a presented one was right.
fn same_signature(expected: &str, presented: &str) -> bool {
    let (expected, presented) = (expected.as_bytes(), presented.as_bytes());
    expected.len() == presented.len()
        && expected
            .iter()
            .zip(presented)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
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

/// The endpoint this emulator was reached on for this request: its `Host` header, or the
/// `:authority` an HTTP/2 request carries on the URI instead of one.
///
/// An address a caller must be able to FOLLOW — the locator of a published artefact, see
/// [`minted_artifact_url`] — has to be built from this, never from a literal AWS
/// hostname, which would send the next client out of the emulator (and out of a sandboxed
/// run) entirely. A request that names no host at all leaves nothing of the emulator to
/// announce, and then the real service's own endpoint for `region` is reported: the only
/// honest answer, and one whose first label is this service's signing name, so a client
/// that does send it back here is still routed to this service.
pub fn request_host(ctx: &smithy_server_runtime::OperationContext<'_>, region: &str) -> String {
    let parts = ctx.parts();
    // Each source is tested on its own, in that order, and the FIRST one carrying a value
    // wins: a `Host` header that names nothing (an empty value) leaves the authority the
    // URI carries still to be read, rather than standing in for it.
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|host| host.to_str().ok()),
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|host| !host.is_empty())
    .map_or_else(
        || format!("{ARN_NAMESPACE}.{region}.amazonaws.com"),
        str::to_string,
    )
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
    /// The application carries no prepared template with the requested id (404).
    TemplateNotFound,
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
            Self::TemplateNotFound => "TemplateNotFoundException",
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
            Self::ApplicationNotFound | Self::VersionNotFound | Self::TemplateNotFound => 404,
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

/// The wording a 404 carries for a semantic version the named application does not hold,
/// naming both members the caller asked with. Stated once here because every operation
/// that resolves a version — GetApplication and both CloudFormation template operations —
/// answers the same condition: the subject is the application, and what is reported about
/// it is that it carries no such version. The fixed text is "Application " … " does not
/// have a version " and it ends with the version string, with no closing punctuation; the
/// two variable parts are the `ApplicationId` and the `SemanticVersion` the request named.
pub fn version_not_found_message(application_id: &str, semantic_version: &str) -> String {
    format!("Application {application_id} does not have a version {semantic_version}")
}

/// The wording a 404 carries for a prepared template the store does not hold, naming the
/// `TemplateId` the caller asked for and nothing else.
///
/// Stated once here because every operation that resolves a prepared template —
/// GetCloudFormationTemplate and CreateCloudFormationChangeSet — answers the same
/// condition about the same subject: the template the request named. The fixed text is
/// "Template with id " … " can not be found.", and the `TemplateId` the request carried in
/// its URI or body is its ONLY variable part: no other resource is named, because no other
/// resource is the one the caller got wrong.
pub fn template_not_found_message(template_id: &str) -> String {
    format!("Template with id {template_id} can not be found.")
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

// ---------------------------------------------------------------------------
// The application PERMISSION POLICY, validated and stored in ONE place.
//
// `PutApplicationPolicy` is the only operation that writes a policy and
// `GetApplicationPolicy` the only one that reads it, so the rule each statement must
// satisfy, and the record a statement is kept as, live here once rather than in two
// handlers that can drift.
// ---------------------------------------------------------------------------

/// The application permissions a policy statement may grant, as the API spells them.
///
/// The catalogue is the "Application permissions" list the `Actions` member's own
/// documentation points at
/// (`smithy_query(query="doc", shape="ApplicationPolicyStatement", member="Actions")` →
/// `https://docs.aws.amazon.com/serverlessrepo/latest/devguide/access-control-resource-based.html#application-permissions`):
/// the per-application actions a publisher grants to another account or an organization,
/// plus `Deploy`, the grant that covers the set a deployment needs.
///
/// An action outside the catalogue is not a permission this service can grant, so a put
/// naming one is refused rather than stored — a stored statement a client reads back must
/// mean something.
pub const APPLICATION_PERMISSIONS: &[&str] = &[
    "CreateCloudFormationChangeSet",
    "CreateCloudFormationTemplate",
    "Deploy",
    "GetApplication",
    "ListApplicationDependencies",
    "ListApplicationVersions",
    "SearchApplications",
    "UnshareApplication",
];

/// The prefix an action carries when it is spelled as the IAM action it is.
///
/// The same permission is written bare in this API's own examples (`Actions=Deploy`) and
/// `serverlessrepo:Deploy` in an IAM policy document, so both spellings name the same
/// entry of [`APPLICATION_PERMISSIONS`] and both are accepted; the statement is stored as
/// the caller spelled it.
const ACTION_PREFIX: &str = "serverlessrepo:";

/// How many statements one application's policy retains, and how many entries one of a
/// statement's lists retains.
///
/// A policy is caller-supplied content the store holds for the life of the application,
/// so without a ceiling one call can drive the emulator's heap until the host is out of
/// memory. AWS publishes no quota for either, so these are the emulator's own bounds:
/// far above any real sharing policy, finite so the growth is not caller-controlled. A
/// put past a ceiling is refused with the modelled `BadRequestException` and leaves the
/// stored policy exactly as it was.
pub const MAX_POLICY_STATEMENTS: usize = 100;

/// See [`MAX_POLICY_STATEMENTS`].
pub const MAX_POLICY_LIST_ENTRIES: usize = 100;

/// See [`MAX_POLICY_STATEMENTS`]; the bound on a caller-supplied `StatementId`.
pub const MAX_STATEMENT_ID_LENGTH: usize = 128;

/// The statements of `statements` as the records a policy is stored as, or the refusal
/// wording the calling operation renders as its modelled invalid-parameter error.
///
/// What is enforced, and why each rule is a rule:
///
/// * `Actions` and `Principals` are `@required` members of the statement
///   (`smithy_query(query="operation", operation="PutApplicationPolicy")`), so a
///   statement granting no action, or granting to nobody, is not a statement;
/// * an action must be an application permission ([`APPLICATION_PERMISSIONS`]);
/// * a principal is "an array of AWS account IDs, or * to make the application public"
///   (`smithy_query(query="doc", shape="ApplicationPolicyStatement", member="Principals")`),
///   so it is a 12-digit account id or `*`, and never the account that OWNS the
///   application: a resource policy grants access to OTHER principals, and the owner's
///   own access does not come from it;
/// * a `PrincipalOrgIDs` entry "corresponds to AWS IAM aws:PrincipalOrgID" (the same
///   query on that member), whose value is an organization id, `o-` and 10 to 32
///   lowercase alphanumerics;
/// * a `StatementId` is "A unique ID for the statement", so two statements of one policy
///   cannot carry the same one, and a statement that names none is minted one.
///
/// Nothing is written until every statement passes, so a refused put leaves the stored
/// policy untouched.
///
/// Every one of these refusals is carried by one modelled shape, `BadRequestException`,
/// under one `errorCode`, so the MESSAGE is the only thing that tells the conditions
/// apart — which makes its wording part of the contract. A value outside its member's
/// vocabulary is reported as `<Member> <value> is invalid`, and a principal that is the
/// application's own owner as [`PRINCIPAL_IS_OWNER_MESSAGE`].
pub fn validate_policy_statements(
    statements: &[ApplicationPolicyStatement],
    owner_account: &str,
) -> Result<Vec<PolicyStatement>, String> {
    if statements.len() > MAX_POLICY_STATEMENTS {
        return Err(format!(
            "Statements must have at most {MAX_POLICY_STATEMENTS} entries, got {}",
            statements.len()
        ));
    }

    let mut stored: Vec<PolicyStatement> = Vec::with_capacity(statements.len());
    for statement in statements {
        bounded_entries("Actions", statement.actions.len())?;
        for action in &statement.actions {
            let permission = action.strip_prefix(ACTION_PREFIX).unwrap_or(action);
            if !APPLICATION_PERMISSIONS.contains(&permission) {
                return Err(format!("Action {action} is invalid"));
            }
        }

        bounded_entries("Principals", statement.principals.len())?;
        for principal in &statement.principals {
            if !is_principal(principal) {
                return Err(format!("Principal {principal} is invalid"));
            }
            if principal == owner_account {
                return Err(PRINCIPAL_IS_OWNER_MESSAGE.to_string());
            }
        }

        let principal_org_ids = statement.principal_org_i_ds.clone().unwrap_or_default();
        if !principal_org_ids.is_empty() {
            bounded_entries("PrincipalOrgIDs", principal_org_ids.len())?;
            for organization_id in &principal_org_ids {
                if !is_organization_id(organization_id) {
                    return Err(format!("PrincipalOrgID {organization_id} is invalid"));
                }
            }
        }

        let statement_id = match statement.statement_id.as_deref() {
            Some(statement_id) => {
                bounded_length("StatementId", statement_id, 1, MAX_STATEMENT_ID_LENGTH)?;
                statement_id.to_string()
            }
            None => new_uuid(),
        };
        if stored.iter().any(|kept| kept.statement_id == statement_id) {
            return Err(format!(
                "StatementId must be unique within the policy, got {statement_id} on two \
                 statements"
            ));
        }

        stored.push(PolicyStatement {
            statement_id,
            actions: statement.actions.clone(),
            principals: statement.principals.clone(),
            principal_org_ids,
        });
    }

    Ok(stored)
}

/// The wording a statement granting to the application's own owner is refused with.
///
/// A resource policy grants access to OTHER principals; the account that owns the
/// application already has that access and cannot be a principal of its own
/// application's policy. This is the service's own wording for the condition, and it is
/// the only thing that separates it from the other refusals of this operation, which all
/// share `BadRequestException` and one `errorCode`.
pub const PRINCIPAL_IS_OWNER_MESSAGE: &str = "You cannot set principal to resource owner";

/// The account that owns the resource an ARN names: an ARN's fifth colon-separated field
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// A list member of a policy statement carries at least one entry and at most
/// [`MAX_POLICY_LIST_ENTRIES`].
fn bounded_entries(member: &str, entries: usize) -> Result<(), String> {
    if entries == 0 {
        return Err(format!("{member} must have at least one entry"));
    }
    if entries > MAX_POLICY_LIST_ENTRIES {
        return Err(format!(
            "{member} must have at most {MAX_POLICY_LIST_ENTRIES} entries, got {entries}"
        ));
    }
    Ok(())
}

/// Whether `principal` is "an AWS account ID, or * to make the application public"
/// (`smithy_query(query="doc", shape="ApplicationPolicyStatement", member="Principals")`):
/// an AWS account id is twelve digits.
fn is_principal(principal: &str) -> bool {
    principal == "*"
        || (principal.len() == 12 && principal.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether `organization_id` is an AWS organization id, the value of the
/// `aws:PrincipalOrgID` condition key `PrincipalOrgIDs` corresponds to: `o-` followed by
/// 10 to 32 lowercase letters or digits.
fn is_organization_id(organization_id: &str) -> bool {
    let Some(suffix) = organization_id.strip_prefix("o-") else {
        return false;
    };
    (10..=32).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// How many ids this process has minted: the part of an id that makes it UNIQUE.
///
/// A counter cannot repeat within a process, so no two ids this service hands out can
/// collide however fast they are minted — which a clock reading alone cannot promise.
static UUIDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A per-process random value, the part of a minted id that makes it UNGUESSABLE.
///
/// `RandomState` seeds each of its hashers with keys the standard library draws from the
/// OS once per process, so hashing a fixed value through one yields a value that differs
/// between runs and is not derivable from the request.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(&RandomState::new());
        ::std::hash::Hash::hash(&"com.amazonaws.serverlessapplicationrepository", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// A fresh lowercase, hyphenated uuid in the version-4 layout (version nibble `4`,
/// variant bits `10`): the `StatementId` a policy statement that named none is stored
/// under, and the `TemplateId` a prepared CloudFormation template is read back by. One
/// minter for both, so the crate has a single id-minting path.
///
/// The 16 bytes are a SHA-256 digest — from the maintained `sha2` crate, not written here
/// — of this process's random seed, the number of ids minted before this one and the
/// current clock reading, rendered by `hex`. The counter is what makes the id unique and
/// the seed is what makes it unguessable; digesting them is what spreads both across all
/// 16 bytes, so an id reveals neither. It is deliberately NOT derived from the request:
/// two identical calls mint two different ids.
pub fn new_uuid() -> String {
    use ::sha2::Digest as _;

    let minted = UUIDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let mut bytes = <[u8; 16]>::try_from(&digest.finalize()[..16]).expect("16 of 32 digest bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
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
    iso8601(SystemTime::now())
}

/// `at`, as every time-valued `__string` member of this service carries it: ISO-8601 in
/// UTC. One renderer, so a record's stored instant and the time members read off it cannot
/// be formatted two ways.
pub fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
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

/// Why a supplied template was refused.
///
/// Every bound it names is a documented AWS maximum, so the refusal states the rule
/// rather than an emulator detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateRefusal {
    /// Bigger than [`MAX_TEMPLATE_BYTES`].
    TemplateTooLarge,
    /// More than [`MAX_TEMPLATE_PARAMETERS`] entries in `Parameters`.
    TooManyParameters,
    /// More than [`MAX_TEMPLATE_RESOURCES`] entries in `Resources`.
    TooManyResources,
    /// A template that declares no `Transform` field, so it is not a packaged AWS SAM
    /// template; see [`declares_transform`]. Carries the template as the refusal quotes
    /// it back ([`quoted_template`]), because the refusal AWS gives names the document it
    /// rejected.
    MissingTransform(String),
}

impl TemplateRefusal {
    /// The refusal wording: for a bound, the member and the bound it exceeded; for a
    /// template that is not a SAM template, the resource it is invalid as and the field
    /// it must declare, both built from the request.
    pub fn message(&self) -> String {
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
            Self::MissingTransform(template) => format!(
                "Resource with name [{TEMPLATE_RESOURCE}] is invalid. Resource must have \
                 Transform field: {template}"
            ),
        }
    }
}

/// The name the refusal gives the template artefact of a publish — the "resource" an
/// invalid template makes invalid, as AWS's own wording names it.
const TEMPLATE_RESOURCE: &str = "template";

/// How much of the refused template the 400 quotes back.
///
/// The refusal names the document it rejected, which is what makes it actionable, but the
/// document is caller-supplied and bounded only by [`MAX_TEMPLATE_BYTES`] — so the quote
/// is cut here instead of letting one request be answered with a megabyte of its own
/// input. The rule the message states (the missing `Transform` field) is unaffected.
const MAX_QUOTED_TEMPLATE_BYTES: usize = 512;

/// `template` as the refusal quotes it: the parsed document in the form AWS's own refusal
/// echoes it ([`template_document::echoed`]), cut at [`MAX_QUOTED_TEMPLATE_BYTES`] on a
/// character boundary and marked as cut.
fn quoted_template(template: &::serde_json::Value) -> String {
    let rendered = template_document::echoed(template);
    if rendered.len() <= MAX_QUOTED_TEMPLATE_BYTES {
        return rendered;
    }
    let mut cut = MAX_QUOTED_TEMPLATE_BYTES;
    while cut > 0 && !rendered.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &rendered[..cut])
}

/// The field a packaged AWS SAM template declares, which makes it a SAM template rather
/// than plain CloudFormation.
///
/// SAM is a CloudFormation *transform*: a SAM template is a CloudFormation template whose
/// top-level `Transform` field names the SAM macro (`AWS::Serverless-2016-10-31`), and
/// CloudFormation lets that field be one name or a list of them. A template that declares
/// none is not the "raw packaged AWS SAM template"
/// (`smithy_query(query="doc", shape="CreateApplicationVersionRequest",
/// member="TemplateBody")`) a publish takes, so it is refused rather than published.
pub fn declares_transform(template: &::serde_json::Value) -> bool {
    match template.get("Transform") {
        Some(::serde_json::Value::String(name)) => !name.trim().is_empty(),
        Some(::serde_json::Value::Array(names)) => names
            .iter()
            .any(|name| name.as_str().is_some_and(|name| !name.trim().is_empty())),
        _ => false,
    }
}

/// A supplied template, read: refused when it is not a publishable AWS SAM template, and
/// otherwise the parameters it declares, as modelled `ParameterDefinition`s.
///
/// A publish takes "the raw packaged AWS SAM template of your application"
/// (`smithy_query(query="doc", shape="CreateApplicationVersionRequest",
/// member="TemplateBody")`), so a template that declares no `Transform` field is not one
/// and is refused with [`TemplateRefusal::MissingTransform`] rather than published; see
/// [`declares_transform`].
///
/// CloudFormation's template anatomy — which AWS SAM extends — puts the parameters in the
/// top-level `Parameters` object: one entry per parameter name, carrying `Type`,
/// `Default`, `Description`, `AllowedValues`, `AllowedPattern`, `MinLength`/`MaxLength`,
/// `MinValue`/`MaxValue`, `ConstraintDescription` and `NoEcho`. A parameter's
/// `ReferencedByResources` is the set of resources in the template's `Resources` object
/// that reference it, which a template states as `{"Ref": "<name>"}` or as `${name}`
/// inside an `Fn::Sub`.
///
/// Only a template the caller supplied INLINE is read, in either of the two forms a
/// packaged SAM template is written in — JSON or YAML, read by
/// [`template_document::read`], which spells a short-form intrinsic (`!Ref`, `!Sub`) as
/// the long form a JSON template carries. An emulator makes no outbound connections, so a
/// template named only by `TemplateUrl` is never fetched, and a body that is not a
/// template document at all is neither judged nor guessed at: it is stored as supplied and
/// reports no parameters.
///
/// # The work is bounded before it is done
///
/// The body is caller-supplied, so its size and shape are refused against the
/// documented maxima ([`MAX_TEMPLATE_BYTES`], [`MAX_TEMPLATE_PARAMETERS`],
/// [`MAX_TEMPLATE_RESOURCES`]) BEFORE anything is walked, and `Resources` is walked
/// exactly ONCE — building an identifier -> referencing-resources index that every
/// parameter then looks itself up in. The cost is linear in the template
/// (`O(resources + parameters)`), not the product of the two, so no accepted template
/// can occupy a worker for longer than its own size warrants. No walk over the parsed
/// document RECURSES: every one below carries an explicit worklist, so a document's
/// nesting costs heap (bounded by the node count the shared reader caps) rather than the
/// process stack, whichever of the two forms the body arrived in.
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
    let Some(template) = template_document::read(body) else {
        return Ok(Vec::new());
    };
    // The shape bounds come first: counting two maps is the cheapest thing this function
    // does, and a template past them is refused before any of the rest of the work —
    // including building a refusal that quotes it back.
    let parameters = template.get("Parameters").and_then(|p| p.as_object());
    if parameters.is_some_and(|parameters| parameters.len() > MAX_TEMPLATE_PARAMETERS) {
        return Err(TemplateRefusal::TooManyParameters);
    }
    let resources = template.get("Resources").and_then(|r| r.as_object());
    if resources.is_some_and(|resources| resources.len() > MAX_TEMPLATE_RESOURCES) {
        return Err(TemplateRefusal::TooManyResources);
    }
    // A template this emulator CAN read is held to the SAM transform: plain
    // CloudFormation is not the packaged SAM template a publish takes. The refusal quotes
    // the template as read, so the caller sees which document was rejected.
    if !declares_transform(&template) {
        return Err(TemplateRefusal::MissingTransform(quoted_template(
            &template,
        )));
    }
    let Some(parameters) = parameters else {
        return Ok(Vec::new());
    };

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

// ---------------------------------------------------------------------------
// The capabilities a deployment of a published version must acknowledge.
//
// "Some applications might include resources that can affect permissions in your AWS
// account … you must explicitly acknowledge their capabilities by specifying this
// parameter. … If you don't specify this parameter for an application that requires
// capabilities, the call will fail."
// (`smithy_query(query="doc", shape="CreateCloudFormationChangeSetRequest",
// member="Capabilities")`)
//
// The requirement is therefore a function of the resources the version's own template
// declares, which is why it is derived here — once — for both the operation that
// REPORTS it (`Version$RequiredCapabilities`) and the one that HOLDS a caller to it
// (`CreateCloudFormationChangeSet`).
// ---------------------------------------------------------------------------

/// Acknowledges the IAM resources a template creates.
pub const CAPABILITY_IAM: &str = "CAPABILITY_IAM";

/// Acknowledges IAM resources the template gives CUSTOM NAMES to.
pub const CAPABILITY_NAMED_IAM: &str = "CAPABILITY_NAMED_IAM";

/// Acknowledges the resource policies a template creates.
pub const CAPABILITY_RESOURCE_POLICY: &str = "CAPABILITY_RESOURCE_POLICY";

/// Acknowledges the nested applications a template contains.
pub const CAPABILITY_AUTO_EXPAND: &str = "CAPABILITY_AUTO_EXPAND";

/// "The following resources require you to specify CAPABILITY_IAM or
/// CAPABILITY_NAMED_IAM: AWS::IAM::Group, AWS::IAM::InstanceProfile, AWS::IAM::Policy,
/// and AWS::IAM::Role" — the `Capabilities` member's own documentation.
const IAM_RESOURCE_TYPES: &[&str] = &[
    "AWS::IAM::Group",
    "AWS::IAM::InstanceProfile",
    "AWS::IAM::Policy",
    "AWS::IAM::Role",
];

/// "The following resources require you to specify CAPABILITY_RESOURCE_POLICY:
/// AWS::Lambda::Permission, AWS::IAM:Policy, AWS::ApplicationAutoScaling::ScalingPolicy,
/// AWS::S3::BucketPolicy, AWS::SQS::QueuePolicy, and AWS::SNS:TopicPolicy" — the same
/// member's documentation, with the two type names it abbreviates (`AWS::IAM:Policy`,
/// `AWS::SNS:TopicPolicy`) spelled as the resource-type reference spells them.
const RESOURCE_POLICY_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApplicationAutoScaling::ScalingPolicy",
    "AWS::IAM::Policy",
    "AWS::Lambda::Permission",
    "AWS::S3::BucketPolicy",
    "AWS::SNS::TopicPolicy",
    "AWS::SQS::QueuePolicy",
];

/// "Applications that contain one or more nested applications require you to specify
/// CAPABILITY_AUTO_EXPAND": a nested application is an `AWS::Serverless::Application`
/// resource, the AWS SAM resource type for one.
const NESTED_APPLICATION_RESOURCE_TYPE: &str = "AWS::Serverless::Application";

/// The property that gives an IAM resource a CUSTOM NAME, per resource type: the `Name`
/// property that type's reference publishes as OPTIONAL. `AWS::IAM::Policy` is absent
/// deliberately — its `PolicyName` is a REQUIRED property of the type, so every policy
/// carries one and it is not a name the template chose to pin.
const IAM_NAME_PROPERTIES: &[(&str, &str)] = &[
    ("AWS::IAM::Group", "GroupName"),
    ("AWS::IAM::InstanceProfile", "InstanceProfileName"),
    ("AWS::IAM::Role", "RoleName"),
];

/// The capabilities a deployment of `template_body` must acknowledge, sorted.
///
/// Derived from the `Resources` the template declares, against the catalogue the
/// `Capabilities` member documents (see the note above). `CAPABILITY_NAMED_IAM` replaces
/// `CAPABILITY_IAM` when any IAM resource carries a custom name, because it is the
/// stronger acknowledgement of the same resources: "If the application contains IAM
/// resources, you can specify either CAPABILITY_IAM or CAPABILITY_NAMED_IAM. If the
/// application contains IAM resources with custom names, you must specify
/// CAPABILITY_NAMED_IAM."
///
/// Derived from the template as [`template_document::read`] reads it, so a YAML template's
/// resources are judged exactly as the same resources in JSON are. A template this
/// emulator never sees — one the caller named only by `TemplateUrl` — or a body that is not
/// a template document declares nothing it can judge, so it requires nothing: the same
/// policy [`parse_template`] applies to the parameters it reports.
pub fn required_capabilities_of(template_body: Option<&String>) -> Vec<String> {
    let Some(body) = template_body else {
        return Vec::new();
    };
    let Some(template) = template_document::read(body) else {
        return Vec::new();
    };
    let Some(resources) = template.get("Resources").and_then(|r| r.as_object()) else {
        return Vec::new();
    };

    let mut required: BTreeSet<&'static str> = BTreeSet::new();
    for spec in resources.values() {
        let Some(resource_type) = spec.get("Type").and_then(|t| t.as_str()) else {
            continue;
        };
        if IAM_RESOURCE_TYPES.contains(&resource_type) {
            if carries_a_custom_name(resource_type, spec) {
                required.insert(CAPABILITY_NAMED_IAM);
            } else {
                required.insert(CAPABILITY_IAM);
            }
        }
        if RESOURCE_POLICY_RESOURCE_TYPES.contains(&resource_type) {
            required.insert(CAPABILITY_RESOURCE_POLICY);
        }
        if resource_type == NESTED_APPLICATION_RESOURCE_TYPE {
            required.insert(CAPABILITY_AUTO_EXPAND);
        }
    }
    // One template with a named and an unnamed IAM resource requires the named
    // capability only: it already acknowledges both.
    if required.contains(CAPABILITY_NAMED_IAM) {
        required.remove(CAPABILITY_IAM);
    }
    required.into_iter().map(str::to_string).collect()
}

/// Whether this resource pins its own name, whatever the property's value is — a literal
/// or an intrinsic function both name the resource.
fn carries_a_custom_name(resource_type: &str, spec: &::serde_json::Value) -> bool {
    IAM_NAME_PROPERTIES
        .iter()
        .filter(|(named_type, _)| *named_type == resource_type)
        .any(|(_, property)| {
            spec.get("Properties")
                .and_then(|properties| properties.get(property))
                .is_some_and(|name| !name.is_null())
        })
}

/// The entries of `required` that `acknowledged` does not cover, in `required`'s order.
///
/// `CAPABILITY_NAMED_IAM` covers a `CAPABILITY_IAM` requirement (it is the stronger
/// acknowledgement of the same resources); nothing else covers anything but itself.
pub fn unacknowledged_capabilities(required: &[String], acknowledged: &[String]) -> Vec<String> {
    required
        .iter()
        .filter(|capability| {
            let covered = acknowledged.iter().any(|given| given == *capability)
                || (capability.as_str() == CAPABILITY_IAM
                    && acknowledged.iter().any(|given| given == CAPABILITY_NAMED_IAM));
            !covered
        })
        .cloned()
        .collect()
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
///
/// The walk is ITERATIVE, over an explicit worklist: the document is caller-supplied, so
/// recursing once per level it nests would spend the process stack on a depth the caller
/// chose — and a stack overflow is not a catchable panic. The worklist cannot outgrow the
/// document's node count, which the shared reader bounds before this runs.
fn collect_references(value: &::serde_json::Value, out: &mut BTreeSet<String>) {
    let mut pending: Vec<&::serde_json::Value> = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            ::serde_json::Value::Object(fields) => {
                if let Some(name) = fields.get("Ref").and_then(|r| r.as_str()) {
                    out.insert(name.to_string());
                }
                if let Some(substitution) = fields.get("Fn::Sub") {
                    collect_substitutions(substitution, out);
                }
                pending.extend(fields.values());
            }
            ::serde_json::Value::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
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

// ---------------------------------------------------------------------------
// Prepared CloudFormation templates.
//
// `CreateCloudFormationTemplate` prepares the deployable template of ONE published
// version and hands back the id it is read by; `GetCloudFormationTemplate` reports that
// same record. Both the status and the expiry are functions of the stored instants and
// the clock, so neither read moves anything.
// ---------------------------------------------------------------------------

/// How long a prepared template lives: SIX hours from the instant it was created.
///
/// Recorded behavior: an `ACTIVE` prepared template reports an `ExpirationTime` exactly
/// six hours after its `CreationTime`. The model's doc sentence on the member — "The date
/// and time this template expires. Templates expire 1 hour after creation."
/// (`smithy_query(query="doc", shape="CreateCloudFormationTemplateResponse",
/// member="ExpirationTime")`) — is contradicted by the service itself, and the window a
/// client observes is the one it depends on: a client that caches the prepared artifact,
/// or schedules a re-prepare from the reported instant, must not treat it as dead five
/// hours early. The window is the service's own published duration, not a span this
/// emulator chose, so it is this constant alone that every `expires_at` and every
/// `status_at` is derived from.
pub const TEMPLATE_LIFETIME: Duration = Duration::from_secs(6 * 60 * 60);

/// How long a prepared template reports [`TEMPLATE_PREPARING`] before the clock owes it
/// [`TEMPLATE_ACTIVE`].
///
/// `Status` is the "Status of the template creation workflow"
/// (`smithy_query(query="doc", shape="CreateCloudFormationTemplateResponse",
/// member="Status")`), whose first state is `PREPARING`: the move to `ACTIVE` is owed by
/// the clock at a fixed instant, whether or not anybody calls, never performed by a read.
/// AWS publishes no duration for it, so this span is the emulator's own choice: non-zero,
/// so a caller reading straight after the create still sees `PREPARING`, and short enough
/// that the next poll of a client sees `ACTIVE`.
pub const TEMPLATE_PREPARATION_SPAN: Duration = Duration::from_millis(500);

/// The three states of the template creation workflow, as the `Status` enum declares them
/// (`smithy_query(query="shape", shape="Status")`: PREPARING | ACTIVE | EXPIRED).
pub const TEMPLATE_PREPARING: &str = "PREPARING";
/// See [`TEMPLATE_PREPARING`].
pub const TEMPLATE_ACTIVE: &str = "ACTIVE";
/// See [`TEMPLATE_PREPARING`].
pub const TEMPLATE_EXPIRED: &str = "EXPIRED";

/// How many prepared templates this emulator retains.
///
/// The map grows once per `CreateCloudFormationTemplate` and no operation deletes an
/// entry, so without a ceiling one caller can drive the emulator's heap until the host is
/// out of memory. AWS publishes no quota for prepared templates, so this is the emulator's
/// own bound; see
/// [`ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState::make_room_for_template`]
/// for what reaching it does.
pub const MAX_STORED_TEMPLATES: usize = 1_000;

/// One prepared CloudFormation template: the record `CreateCloudFormationTemplate` writes
/// and `GetCloudFormationTemplate` reports.
#[derive(Debug, Clone)]
pub struct CloudFormationTemplate {
    /// The UUID this template is read back by, and its key in
    /// [`ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState::templates`].
    pub template_id: String,
    /// The `ApplicationId` of the application whose version was prepared. A template is
    /// reached only through its own application, so this is checked on every read.
    pub application_id: String,
    /// The semantic version that was prepared: the one the request named, or the
    /// application's latest when it named none.
    pub semantic_version: String,
    /// "A link to the template that can be used to deploy the application using AWS
    /// CloudFormation" — the location of the template body published with that version,
    /// which is what this prepared template serves.
    pub template_url: String,
    /// When this template was prepared.
    pub created_at: SystemTime,
    /// The instant the clock owes this template its move from `PREPARING` to `ACTIVE`:
    /// `created_at + `[`TEMPLATE_PREPARATION_SPAN`].
    pub active_at: SystemTime,
    /// The instant this template expires: `created_at + `[`TEMPLATE_LIFETIME`].
    pub expires_at: SystemTime,
}

impl CloudFormationTemplate {
    /// The record for a template prepared at `now` from the version `template_url` was
    /// published with. Both due instants are derived here, so no caller sets them.
    pub fn prepared(
        template_id: String,
        application_id: String,
        semantic_version: String,
        template_url: String,
        now: SystemTime,
    ) -> Self {
        Self {
            template_id,
            application_id,
            semantic_version,
            template_url,
            created_at: now,
            active_at: now + TEMPLATE_PREPARATION_SPAN,
            expires_at: now + TEMPLATE_LIFETIME,
        }
    }

    /// The status this template reports at `now`, a pure function of the stored instants:
    /// `PREPARING` until the preparation span is up, `ACTIVE` until it expires, `EXPIRED`
    /// after that. Nothing is written, so two reads at one instant answer identically.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now >= self.expires_at {
            TEMPLATE_EXPIRED
        } else if now >= self.active_at {
            TEMPLATE_ACTIVE
        } else {
            TEMPLATE_PREPARING
        }
    }

    /// This record projected into the seven members both template operations report, so
    /// `CreateCloudFormationTemplate` and `GetCloudFormationTemplate` cannot render one
    /// stored template two ways.
    pub fn report(&self, now: SystemTime) -> TemplateReport {
        TemplateReport {
            application_id: self.application_id.clone(),
            creation_time: iso8601(self.created_at),
            expiration_time: iso8601(self.expires_at),
            semantic_version: self.semantic_version.clone(),
            status: self.status_at(now).to_string(),
            template_id: self.template_id.clone(),
            template_url: self.template_url.clone(),
        }
    }
}

/// A prepared template as both template operations report it: the members of
/// `CreateCloudFormationTemplateResponse`, which `GetCloudFormationTemplateResponse`
/// declares identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateReport {
    pub application_id: String,
    pub creation_time: String,
    pub expiration_time: String,
    pub semantic_version: String,
    pub status: String,
    pub template_id: String,
    pub template_url: String,
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
    /// Every prepared CloudFormation template, keyed by its `TemplateId`. Bounded by
    /// [`MAX_STORED_TEMPLATES`].
    pub templates: BTreeMap<String, CloudFormationTemplate>,
}

impl ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState {
    /// Make room for one more prepared template at `now`, answering whether there is any.
    ///
    /// No operation deletes a prepared template, so the only record whose life has already
    /// ended is one the clock has expired: releasing the template that expired longest ago
    /// materialises a removal that was due at its own `expires_at` whether or not anybody
    /// called. A store at the ceiling holding nothing expired makes no room at all — the
    /// create is refused rather than served by evicting a template another caller can
    /// still deploy.
    pub fn make_room_for_template(&mut self, now: SystemTime) -> bool {
        if self.templates.len() < MAX_STORED_TEMPLATES {
            return true;
        }
        let expired = self
            .templates
            .values()
            .filter(|template| template.status_at(now) == TEMPLATE_EXPIRED)
            .min_by_key(|template| template.expires_at)
            .map(|template| template.template_id.clone());
        match expired {
            Some(template_id) => self.templates.remove(&template_id).is_some(),
            None => false,
        }
    }

    /// The prepared template `template_id` of the application `application_id`, or `None`
    /// when the store holds no such template OR holds one prepared from a DIFFERENT
    /// application: a template is a sub-resource of its application
    /// (`/applications/{ApplicationId}/templates/{TemplateId}`), so one read through
    /// another application's id is as absent as one that was never prepared.
    pub fn template_of(
        &self,
        application_id: &str,
        template_id: &str,
    ) -> Option<&CloudFormationTemplate> {
        self.templates
            .get(template_id)
            .filter(|template| template.application_id == application_id)
    }

    /// The retained bytes the artefact `target` addresses, or `None` when this store holds
    /// nothing published there.
    ///
    /// This is the READ behind the document plane (`crate::service`): every segment of the
    /// announced locator has to agree with the stored record — the account that published
    /// it, the region and name the application is catalogued under, and, for an artefact a
    /// version publishes, the version that published the body. A locator whose application
    /// has since been deleted, whose version was never published, or whose body was named
    /// by a `*Url` the publisher supplied instead of being supplied inline, addresses
    /// nothing. Nothing is written, so following a locator twice answers the same bytes.
    pub fn artifact_body(&self, target: &ArtifactTarget) -> Option<&String> {
        if target.account_id != ::engine::emulator_account_id() {
            return None;
        }
        let application_id = self
            .name_index
            .get(&name_key(&target.region, &target.application_name))?;
        let application = self.applications.get(application_id)?;
        match (target.artifact, target.semantic_version.as_deref()) {
            (Artifact::License, None) => application.license_body.as_ref(),
            (Artifact::Readme, None) => application.readme_body.as_ref(),
            (Artifact::Template, Some(semantic_version)) => application
                .version_named(semantic_version)
                .and_then(|version| version.template_body.as_ref()),
            // A template's locator is minted version-scoped and a license's or a readme's
            // application-scoped ([`minted_artifact_url`]), so a crossed pair is a path
            // this service never announced and addresses nothing.
            _ => None,
        }
    }
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
