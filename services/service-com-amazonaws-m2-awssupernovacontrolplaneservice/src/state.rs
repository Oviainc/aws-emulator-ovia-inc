// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-m2-awssupernovacontrolplaneservice service.
//!
//! The application lifecycle (m2's primary resource) is persisted here: every
//! operation that reports on an application computes its answer from these records,
//! and nothing is recomputed from the request.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// How long a just-created application (and application version) is still reported in
/// its non-terminal `Creating` status before the clock owes it the next one.
///
/// m2 returns a freshly created application in `Creating` and settles it without any
/// further call, so the transition is owed by the clock rather than by a caller. The
/// span is the emulator's own choice — it is never zero, so a caller that reads the
/// resource straight after creating it still observes the transient status.
pub const CREATION_SETTLES_AFTER: Duration = Duration::from_millis(400);

/// `ApplicationLifecycle` value a created application is reported in until
/// [`CREATION_SETTLES_AFTER`] has passed. Source: the model's
/// `crate::ops::types::APPLICATION_LIFECYCLE_VALUES`.
pub const APPLICATION_CREATING: &str = "Creating";

/// `ApplicationLifecycle` value an application that has settled, but has not been
/// deployed to a runtime environment, is reported in. Source: the model's
/// `crate::ops::types::APPLICATION_LIFECYCLE_VALUES`.
pub const APPLICATION_CREATED: &str = "Created";

/// `ApplicationVersionLifecycle` value a new version is reported in until
/// [`CREATION_SETTLES_AFTER`] has passed. Source: the model's
/// `crate::ops::types::APPLICATION_VERSION_LIFECYCLE_VALUES`.
pub const VERSION_CREATING: &str = "Creating";

/// `ApplicationVersionLifecycle` value a settled version is reported in. Source: the
/// model's `crate::ops::types::APPLICATION_VERSION_LIFECYCLE_VALUES`.
pub const VERSION_AVAILABLE: &str = "Available";

/// The most applications this emulator keeps, after which `CreateApplication` is refused
/// with the modelled `ServiceQuotaExceededException`.
///
/// The emulator's store is process memory with no eviction, so the resource count needs a
/// ceiling: without one a caller looping create-with-a-fresh-name grows the process
/// without limit. The value is the emulator's own choice — real m2 publishes a per-Region
/// application quota, and a caller that reaches this one sees the same modelled fault it
/// would see there.
pub const MAX_APPLICATIONS: usize = 200;

/// The most versions one application keeps, after which `UpdateApplication` is refused.
///
/// Each version holds a full copy of the caller-supplied definition text and nothing
/// short of `DeleteApplication` drops one, so the version history needs a ceiling too.
pub const MAX_VERSIONS_PER_APPLICATION: usize = 100;

/// The page `ListApplications` returns when the caller names no `maxResults`.
///
/// A listing is never unbounded: the page is clamped and a `nextToken` carries the rest,
/// so an unqualified list can neither serialise the whole store nor lose an item. The
/// model's own `maxResults` ceiling is 2000, and a caller may still ask for any value in
/// that range.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// This service's ARN namespace, which is also its SigV4 signing name and endpoint
/// prefix: `smithy_query(query="service_info")` -> `arnNamespace: m2`.
pub const ARN_SERVICE: &str = "m2";

/// The region a minted value carries when the request names none the emulator can
/// read; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, when the SigV4 credential scope names one
/// shaped like the region segment of the modelled `Arn` pattern, else
/// [`DEFAULT_REGION`].
///
/// The ONE region read in this crate: an ARN is minted from it (and persisted, so a
/// resource's ARN never changes) and so is the single-sign-on URL
/// [`signed_bluinsights_url`] builds, and neither may carry a value that is not
/// shaped like a region.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` matches the region segment of the m2 `Arn` pattern,
/// `[a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]` (from `smithy_query`'s
/// `Arn` pattern), so a minted ARN satisfies the shape the model declares.
pub(crate) fn is_arn_region(region: &str) -> bool {
    let Some((prefix, mut rest)) = region.split_once('-') else {
        return false;
    };
    if prefix.len() != 2 || !prefix.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    // The optional partition segment: `gov-`, `iso-`, or `iso<letter>-`.
    if let Some(stripped) = rest.strip_prefix("gov-") {
        rest = stripped;
    } else if let Some(after_iso) = rest.strip_prefix("iso") {
        if let Some(stripped) = after_iso.strip_prefix('-') {
            rest = stripped;
        } else if after_iso
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        {
            if let Some(stripped) = after_iso[1..].strip_prefix('-') {
                rest = stripped;
            }
        }
    }
    let Some((name, number)) = rest.rsplit_once('-') else {
        return false;
    };
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_lowercase())
        && number.len() == 1
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// The base URL the AWS Blu Insights single-sign-on URL is built on: the emulator's
/// OWN endpoint — the authority this request arrived on — falling back to this
/// service's regional endpoint when the request carries no authority at all.
///
/// It names the emulator rather than `bluinsights.aws`, the host the real web
/// application is published under, because a URL this emulator computes has to be
/// honest about what it addresses: the emulator serves no Blu Insights application, so
/// a URL pointing at AWS would send a caller out of the emulator entirely. Same rule,
/// and same reading of the request, as `kinesisvideo`'s `request_endpoint`.
///
/// The authority is taken only when it IS one ([`is_authority`]), so a `Host` header
/// carrying a URL delimiter cannot change how a client reads the URL this is
/// interpolated into.
pub fn request_endpoint(parts: &::http::request::Parts, region: &str) -> String {
    let authority = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| is_authority(host))
        .map(ToString::to_string)
        .or_else(|| {
            parts
                .uri
                .authority()
                .map(ToString::to_string)
                .filter(|authority| is_authority(authority))
        });
    match authority {
        Some(authority) => format!("https://{authority}"),
        None => format!("https://{ARN_SERVICE}.{region}.amazonaws.com"),
    }
}

/// Whether `value` is an HTTP authority — `host[:port]`.
///
/// Decided by `http::uri::Authority`, the maintained parse this crate already depends
/// on, rather than by a charset test of our own; the two extra conditions are that the
/// parse consumed the WHOLE value (`Authority` stops at a delimiter, so a value
/// carrying `/`, `?` or `#` parses as its prefix) and that it carries no `user@`
/// userinfo, which has no place in a URL this emulator hands to a caller.
fn is_authority(value: &str) -> bool {
    ::http::uri::Authority::try_from(value)
        .is_ok_and(|authority| authority.as_str() == value && !value.contains('@'))
}

/// The path the single-sign-on URL carries under that endpoint, named once so the URL
/// this emulator computes has one spelling.
pub const BLUINSIGHTS_SSO_PATH: &str = "/bluinsights/sso";

/// The single-sign-on AWS Blu Insights URL for the account and region a request was
/// signed for, under the endpoint it arrived on.
///
/// Computed, never stored: the URL names the account and the region the caller signed
/// for, plus a federation token derived from exactly those two values, so two reads in
/// one region answer the same URL (the operation is `@readonly` — nothing about it may
/// change what a later caller sees) and a caller signing for another region gets
/// another URL. The token is [`derived_token`] over this service's namespace, the
/// account and the region: it carries no request data and no secret of this emulator,
/// and nothing may authenticate on it. `account` is this emulator's own fixed 12-digit
/// id (`engine::emulator_account_id`) and `region` is shape-checked by
/// [`request_region`], so neither can carry a URL delimiter.
pub fn signed_bluinsights_url(endpoint: &str, account: &str, region: &str) -> String {
    let token = derived_token(&[ARN_SERVICE, "bluinsights", account, region]);
    format!("{endpoint}{BLUINSIGHTS_SSO_PATH}?account={account}&region={region}&token={token}")
}

/// A stable, opaque token derived from `parts`: the first 16 bytes of the SHA-256 digest
/// of the parts, each NUL-terminated, rendered as lowercase hexadecimal.
///
/// The crate's ONE derivation for a value that must be the same every time the same
/// inputs are reported — the same `sha2`/`hex` pair [`ComAmazonawsM2Awssupernovacontrolplaneservice
/// State::mint_identifier`](ComAmazonawsM2AwssupernovacontrolplaneserviceState::mint_identifier)
/// already mints ids with, differing only in carrying no counter and no clock, which is
/// what makes it stable. The NUL terminator is what keeps two different splittings of
/// the same text (`["ab", "c"]` and `["a", "bc"]`) from deriving one token.
fn derived_token(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(&hasher.finalize()[..16])
}

/// One recorded version of an application: the definition text Create/Update
/// persisted, and when that version was recorded.
#[derive(Debug, Clone)]
pub struct ApplicationVersion {
    /// The version number, 1 for the version `CreateApplication` records.
    pub application_version: i32,
    /// The definition text persisted for this version, returned verbatim by
    /// `GetApplicationVersion`.
    pub definition_content: String,
    /// The description recorded with this version.
    pub description: Option<String>,
    /// When this version was recorded.
    pub creation_time: SystemTime,
    /// The instant the clock owes this version its `Available` status; see
    /// [`CREATION_SETTLES_AFTER`].
    pub settles_at: SystemTime,
    /// Why the version is in its current status, when the service has a reason.
    pub status_reason: Option<String>,
}

impl ApplicationVersion {
    /// This version's `ApplicationVersionLifecycle` status as of `now`: a version is
    /// reported `Creating` until the clock owes it `Available`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.settles_at {
            VERSION_CREATING
        } else {
            VERSION_AVAILABLE
        }
    }
}

/// A persisted m2 application.
#[derive(Debug, Clone)]
pub struct Application {
    /// The minted, opaque application id (the `Identifier` every operation takes).
    pub application_id: String,
    /// The ARN minted at creation: `arn:aws:m2:<region>:<account>:app/<applicationId>`.
    pub application_arn: String,
    /// The caller-chosen name, unique per account.
    pub name: String,
    /// The current description, as last supplied by Create/Update.
    pub description: Option<String>,
    /// `bluage` or `microfocus`.
    pub engine_type: String,
    /// The customer managed key id supplied at creation, if any.
    pub kms_key_id: Option<String>,
    /// The role ARN supplied at creation, if any.
    pub role_arn: Option<String>,
    /// When the application was created.
    pub creation_time: SystemTime,
    /// The instant the clock owes this application its settled status; see
    /// [`CREATION_SETTLES_AFTER`].
    pub settles_at: SystemTime,
    /// When the application was last started, once a start operation records one.
    pub last_start_time: Option<SystemTime>,
    /// The runtime environment this application is deployed to, once a deployment
    /// records one.
    pub environment_id: Option<String>,
    /// The highest recorded version number.
    pub latest_version: i32,
    /// Every recorded version, in ascending version order.
    pub versions: Vec<ApplicationVersion>,
}

impl Application {
    /// This application's `ApplicationLifecycle` status as of `now`: an application is
    /// reported `Creating` until the clock owes it `Created`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.settles_at {
            APPLICATION_CREATING
        } else {
            APPLICATION_CREATED
        }
    }

    /// The recorded version `version`, or `None` when this application has no such
    /// version.
    pub fn version(&self, version: i32) -> Option<&ApplicationVersion> {
        self.versions
            .iter()
            .find(|candidate| candidate.application_version == version)
    }

    /// The highest recorded version. An application always carries the version
    /// `CreateApplication` recorded, so this is only `None` for a record built by hand.
    pub fn latest(&self) -> Option<&ApplicationVersion> {
        self.version(self.latest_version)
    }
}

/// Per-service state for com-amazonaws-m2-awssupernovacontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsM2AwssupernovacontrolplaneserviceState {
    /// Every application, keyed by its minted application id.
    pub applications: HashMap<String, Application>,
    /// Application id by name — the index behind the duplicate-name conflict.
    pub app_names: HashMap<String, String>,
    /// Application id by `@idempotencyToken` client token, for replaying a retried
    /// `CreateApplication`.
    pub client_tokens: HashMap<String, String>,
    /// The ONE tag store, keyed by resource ARN, so the application operations and the
    /// Tag/Untag/ListTagsForResource family read and write the same tags rather than two
    /// copies of them.
    pub tags_by_arn: HashMap<String, HashMap<String, String>>,
    /// Counter folded into every minted identifier, so two resources created in the same
    /// clock tick still get distinct ids.
    pub minted: u64,
}

impl ComAmazonawsM2AwssupernovacontrolplaneserviceState {
    /// Mint an opaque `Identifier` (`^\S{1,80}$`): 32 lowercase hex characters derived
    /// from a per-state counter, the caller-visible `seed` and the wall clock, so ids are
    /// unique within a run and carry no caller data a client could depend on.
    pub fn mint_identifier(&mut self, seed: &str) -> String {
        self.minted += 1;
        let mut hasher = Sha256::new();
        hasher.update(self.minted.to_be_bytes());
        hasher.update(seed.as_bytes());
        hasher.update(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes(),
        );
        hex::encode(&hasher.finalize()[..16])
    }

    /// The tags recorded for `arn`, or an empty map when it carries none.
    pub fn tags_for(&self, arn: &str) -> HashMap<String, String> {
        self.tags_by_arn.get(arn).cloned().unwrap_or_default()
    }
}

/// The definition text to persist for a `Definition`, or `None` when the definition is
/// an Amazon S3 location.
///
/// An inline definition is persisted verbatim and is what `GetApplicationVersion`
/// reports as `definitionContent`. The emulator cannot read an object out of Amazon S3,
/// so a definition given as `s3Location` is refused with the operation's modelled
/// `ValidationException` rather than answered with invented content.
pub fn definition_content(definition: &crate::ops::types::Definition) -> Option<&str> {
    match definition {
        crate::ops::types::Definition::Content(content) => Some(content.as_str()),
        crate::ops::types::Definition::s3Location(_) => None,
    }
}

/// How a definition the emulator cannot read is refused, named once so Create and Update
/// refuse it identically.
pub const S3_DEFINITION_UNSUPPORTED: &str =
    "definition.s3Location is not supported by this emulator: it cannot read an \
     application definition out of Amazon S3. Supply the definition inline as \
     definition.content.";

/// Encode an opaque `NextToken` (`^\S{1,2000}$`) for the last item of a page.
///
/// The token names the sort key the next page resumes AFTER — the item's creation
/// instant and its id — rather than an offset into the list, so a resource created or
/// deleted between two pages can neither repeat nor skip an item the caller has not
/// seen. It is hex so that it carries no whitespace, as `NextToken`'s pattern requires.
pub fn encode_page_token(creation_time: SystemTime, id: &str) -> String {
    let nanos = creation_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hex::encode(format!("{nanos}:{id}"))
}

/// Decode a token built by [`encode_page_token`], or `None` when it was not.
pub fn decode_page_token(token: &str) -> Option<(u128, String)> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (nanos, id) = text.split_once(':')?;
    Some((nanos.parse().ok()?, id.to_string()))
}

/// The sort key a paginated listing orders by: creation instant, then id, so the order
/// is total and stable across calls.
pub fn page_key(creation_time: SystemTime, id: &str) -> (u128, String) {
    let nanos = creation_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (nanos, id.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsM2AwssupernovacontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsM2AwssupernovacontrolplaneserviceState>)
}
