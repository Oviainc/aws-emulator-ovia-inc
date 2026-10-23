// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resiliencehub-awsresiliencehub service.

use crate::ops::types::{App, AppComponent, ResiliencyPolicy};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The region every minted ARN falls back to when the request carries no usable
/// SigV4 credential scope (an unsigned, in-process call, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `appVersion` of an application's DRAFT version — the one version every
/// application has, and the only one the `CreateAppVersion*` / `UpdateAppVersion*`
/// operations write to (`PublishAppVersion` is what mints a numbered, frozen
/// version from it).
pub const DRAFT_APP_VERSION: &str = "draft";

/// The most applications this emulator stores.
///
/// A MEMORY BOUND, not an AWS quota: `CreateApp` is reachable by any caller and
/// stores a caller-sized record (description, tags, event subscriptions) per call,
/// so without a ceiling a request loop grows the process until the host is out of
/// memory. Past it `CreateApp` answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when an
/// application cannot be created because a limit is reached.
pub const MAX_APPS: usize = 200;

/// The most resiliency policies this emulator stores, for the same reason as
/// [`MAX_APPS`]. `CreateResiliencyPolicy` answers `ServiceQuotaExceededException`
/// past it.
pub const MAX_RESILIENCY_POLICIES: usize = 200;

/// The most PUBLISHED app versions this emulator stores for ONE application.
///
/// A MEMORY BOUND, not an AWS quota: `PublishAppVersion` is reachable by any
/// caller and freezes a COPY of the draft's contents per call, so without a
/// ceiling a publish loop grows the process until the host is out of memory.
/// `PublishAppVersion` models no `ServiceQuotaExceededException`, so past the
/// ceiling it answers its own modelled `ValidationException` (HTTP 400) — the
/// channel its model offers for a request this service will not carry out —
/// naming the ceiling.
pub const MAX_PUBLISHED_APP_VERSIONS_PER_APP: usize = 200;

/// The most Application Components this emulator stores in ONE app version.
///
/// A MEMORY BOUND, not an AWS quota, for the same reason as [`MAX_APPS`]:
/// `CreateAppVersionAppComponent` is reachable by any caller and stores a
/// caller-sized record per call. Past it the operation answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when a
/// resource cannot be created because a limit is reached.
pub const MAX_APP_COMPONENTS_PER_APP_VERSION: usize = 200;

/// The modelled `@length` of `AdditionalInfoValueList` — the value list of every
/// `AdditionalInfoMap` entry — transcribed from the model (`smithy_query
/// query="shape" shape="AdditionalInfoMap"`: `value` targets
/// `AdditionalInfoValueList`, `length` 1..10). Enforced by
/// [`additional_info_violation`].
const ADDITIONAL_INFO_VALUES_MIN: usize = 1;

/// The upper half of that bound; see [`ADDITIONAL_INFO_VALUES_MIN`].
const ADDITIONAL_INFO_VALUES_MAX: usize = 10;

/// Hard ceiling on remembered idempotency tokens, oldest evicted first.
///
/// A BACKSTOP. One token is recorded per successfully created resource and
/// `DeleteApp` forgets the tokens of the app it removes, so the map is already
/// bounded by [`MAX_APPS`] + [`MAX_RESILIENCY_POLICIES`] while every creating
/// handler releases what it allocated. The ceiling is here so that invariant does
/// not depend on a future handler remembering to, and is enforced in
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::record_idempotency_token`],
/// the only writer.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_APPS + MAX_RESILIENCY_POLICIES;

/// What one app version of one application holds.
///
/// The modelled shapes ARE the stored records, exactly as for `App` and
/// `ResiliencyPolicy`: a Describe is a clone of the stored value, so a read can
/// never disagree with what the last write persisted.
#[derive(Debug, Default, Clone)]
pub struct AppVersionContents {
    /// The version's Application Components, keyed by their `id`. A `BTreeMap`
    /// because a reader (`ListAppVersionAppComponents`, and any paginated read
    /// built on it) needs one stable order across calls. At most
    /// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] entries.
    pub app_components: BTreeMap<String, AppComponent>,
    /// The `versionName` the `PublishAppVersion` call that minted this version
    /// carried, when it named one — the modelled `AppVersionSummary$versionName`.
    /// Always `None` for the draft, which nobody names.
    pub version_name: Option<String>,
    /// The modelled `AppVersionSummary$identifier` of this version: the publish
    /// sequence number a published version was minted with. `None` for the draft,
    /// which is not a published version and so has no identifier.
    pub identifier: Option<i64>,
    /// When this version was published — the modelled
    /// `AppVersionSummary$creationTime`. `None` for the draft, which comes into
    /// being with the application and whose creation time is the application's own
    /// `creationTime`.
    pub creation_time: Option<::aws_smithy_types::DateTime>,
}

impl AppVersionContents {
    /// Whether this app version holds nothing — the question
    /// `PublishAppVersion` asks of a draft before freezing it.
    ///
    /// Every member a draft writer persists is counted here, so adding one (a
    /// resource, an app template body) means adding it to this test rather than
    /// to each caller of it.
    pub fn is_empty(&self) -> bool {
        self.app_components.is_empty()
    }
}

/// Per-service state for com-amazonaws-resiliencehub-awsresiliencehub. A handler reaches it through [`service_state`].
///
/// The modelled `App` and `ResiliencyPolicy` shapes ARE the stored records: a
/// Describe is a clone of the stored value, so a read can never disagree with what
/// the last write persisted.
///
/// Every map here is caller-driven, so every one is bounded: `apps` by
/// [`MAX_APPS`], `app_name_index` by `apps` (one entry per stored app),
/// `policies` by [`MAX_RESILIENCY_POLICIES`], `app_versions` by `apps` (an entry
/// exists only for a stored application, and `DeleteApp` drops it) times
/// [`MAX_PUBLISHED_APP_VERSIONS_PER_APP`] plus the draft, times
/// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] per version, and the idempotency store
/// by [`MAX_IDEMPOTENCY_TOKENS`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Every stored application, keyed by its minted `appArn`. At most
    /// [`MAX_APPS`] entries.
    pub apps: HashMap<String, App>,
    /// `name` -> `appArn`. Application names are unique within an account, so this
    /// is both the name filter's index and the duplicate-name conflict detector.
    /// Exactly one entry per entry of `apps`.
    pub app_name_index: HashMap<String, String>,
    /// Every stored resiliency policy, keyed by its minted `policyArn`. At most
    /// [`MAX_RESILIENCY_POLICIES`] entries.
    pub policies: HashMap<String, ResiliencyPolicy>,
    /// What each app version of each application holds: `appArn` ->
    /// `appVersion` -> the version's contents. An entry is created only for a
    /// stored application and dropped by `DeleteApp`, so this map never outlives
    /// `apps`.
    ///
    /// The DRAFT version of a stored application exists whether or not it has an
    /// entry here — [`Self::app_version_exists`] is what answers that — so an
    /// application nobody has added anything to costs nothing.
    pub app_versions: HashMap<String, HashMap<String, AppVersionContents>>,
    /// `<operation>/<clientToken>` -> the ARN that token's FIRST call created, so a
    /// replayed `@idempotencyToken` returns the first result instead of creating a
    /// second resource. Keyed by operation as well as token because the same token
    /// may legitimately be replayed to two different operations.
    ///
    /// PRIVATE, with `record_idempotency_token` / `created_by_token` /
    /// `forget_idempotency_tokens_for` as the only access: the entry-count ceiling
    /// cannot be bypassed by a handler inserting directly.
    idempotency_tokens: HashMap<String, String>,
    /// The keys of `idempotency_tokens` in the order they were first recorded, so
    /// the ceiling evicts the oldest.
    idempotency_order: VecDeque<String>,
}

impl ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Whether `app_version` of `app_arn` exists.
    ///
    /// The draft version exists for every stored application — it is created with
    /// the application and cannot be removed — while any other version exists only
    /// once something recorded it.
    pub fn app_version_exists(&self, app_arn: &str, app_version: &str) -> bool {
        if app_version == DRAFT_APP_VERSION {
            return self.apps.contains_key(app_arn);
        }
        self.app_versions
            .get(app_arn)
            .is_some_and(|versions| versions.contains_key(app_version))
    }

    /// What `app_version` of `app_arn` holds, when anything has been stored in it.
    ///
    /// `None` covers both "no such version" and "an empty draft": a reader that has
    /// to tell them apart asks [`Self::app_version_exists`] first.
    pub fn app_version(&self, app_arn: &str, app_version: &str) -> Option<&AppVersionContents> {
        self.app_versions
            .get(app_arn)
            .and_then(|versions| versions.get(app_version))
    }

    /// The draft version of `app_arn`, for writing — materialised empty on the
    /// first write to it. The caller has already established that `app_arn` names
    /// a stored application.
    pub fn draft_app_version_mut(&mut self, app_arn: &str) -> &mut AppVersionContents {
        self.app_versions
            .entry(app_arn.to_string())
            .or_default()
            .entry(DRAFT_APP_VERSION.to_string())
            .or_default()
    }

    /// How many PUBLISHED versions `app_arn` has: every stored version except the
    /// draft, which is not published.
    pub fn published_app_version_count(&self, app_arn: &str) -> usize {
        self.app_versions
            .get(app_arn)
            .map(|versions| {
                versions
                    .keys()
                    .filter(|version| version.as_str() != DRAFT_APP_VERSION)
                    .count()
            })
            .unwrap_or_default()
    }

    /// The publish sequence number the next published version of `app_arn` takes:
    /// one past the highest it already has, so the first publish is 1 and the
    /// numbers never repeat for one application.
    ///
    /// Derived from the stored versions rather than from a counter, so the number
    /// a version was minted with is the number a reader finds it under.
    pub fn next_published_app_version(&self, app_arn: &str) -> i64 {
        let highest = self
            .app_versions
            .get(app_arn)
            .and_then(|versions| {
                versions
                    .values()
                    .filter_map(|contents| contents.identifier)
                    .max()
            })
            .unwrap_or(0);
        highest.saturating_add(1)
    }

    /// Store `contents` as version `app_version` of `app_arn`. The caller has
    /// already established that `app_arn` names a stored application and that
    /// `app_version` is not in use.
    pub fn insert_app_version(
        &mut self,
        app_arn: &str,
        app_version: &str,
        contents: AppVersionContents,
    ) {
        self.app_versions
            .entry(app_arn.to_string())
            .or_default()
            .insert(app_version.to_string(), contents);
    }

    /// Forget every app version of `app_arn`, so deleting an application releases
    /// what its versions allocated.
    pub fn forget_app_versions(&mut self, app_arn: &str) {
        self.app_versions.remove(app_arn);
    }

    /// The ARN `operation`'s `client_token` created on its first call, if it is
    /// still remembered.
    pub fn created_by_token(&self, operation: &str, client_token: &str) -> Option<&String> {
        self.idempotency_tokens
            .get(&idempotency_key(operation, client_token))
    }

    /// Remember that `operation`'s `client_token` created `arn`, evicting the
    /// oldest remembered token once [`MAX_IDEMPOTENCY_TOKENS`] is reached. Re-recording
    /// a key already present replaces its ARN and keeps its original position, so a
    /// replay cannot be used to push other callers' tokens out.
    pub fn record_idempotency_token(&mut self, operation: &str, client_token: &str, arn: &str) {
        let key = idempotency_key(operation, client_token);
        if self
            .idempotency_tokens
            .insert(key.clone(), arn.to_string())
            .is_some()
        {
            return;
        }
        self.idempotency_order.push_back(key);
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// Forget every token that created `arn` or anything UNDER it (a sub-resource,
    /// recorded as `<arn>/<kind>/<id>`), so deleting a resource releases what
    /// creating it and its children allocated.
    pub fn forget_idempotency_tokens_for(&mut self, arn: &str) {
        let sub_resource_prefix = format!("{arn}/");
        self.idempotency_tokens
            .retain(|_, created| created != arn && !created.starts_with(&sub_resource_prefix));
        self.idempotency_order
            .retain(|key| self.idempotency_tokens.contains_key(key));
    }

    /// How many idempotency tokens are remembered. For tests and diagnostics.
    pub fn remembered_token_count(&self) -> usize {
        self.idempotency_tokens.len()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResiliencehubAwsresiliencehubState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResiliencehubAwsresiliencehubState>)
}

/// The idempotency-map key for `operation`'s `client_token`.
fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// Whether `region` satisfies the REGION component of the `Arn` shape's `@pattern`,
/// transcribed from the model (`smithy_query query="shape" shape="Arn"` on
/// `smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json`, whose region
/// group is `([a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9])`): two lowercase
/// letters, a hyphen, an OPTIONAL `iso`/`iso<letter>`/`gov` infix, one or more
/// lowercase letters, a hyphen and one digit.
///
/// Every ARN this service hands back must satisfy the model's pattern, because it is
/// the same pattern the boundary validator applies when that ARN comes back in as
/// `appArn` or `policyArn` — and the region is the only part of a minted ARN the
/// caller influences.
///
/// The optional group is tried BOTH ways, exactly as the pattern's alternation does:
/// a region whose third segment merely starts with `iso` (`us-isolated-1`) matches
/// through the empty branch, so committing to the `iso` branch would reject a region
/// the pattern admits.
pub fn region_matches_modeled_arn(region: &str) -> bool {
    let Some((prefix, rest)) = region.split_once('-') else {
        return false;
    };
    if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return false;
    }

    // `((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]`: the empty branch, the `gov-`
    // branch and the `iso`/`iso<letter>-` branch.
    matches_region_tail(rest)
        || rest.strip_prefix("gov-").is_some_and(matches_region_tail)
        || iso_infix_tail(rest).is_some_and(matches_region_tail)
}

/// Whether `tail` is the pattern's `[a-z]+-[0-9]` ending.
fn matches_region_tail(tail: &str) -> bool {
    let Some((letters, digit)) = tail.split_once('-') else {
        return false;
    };
    !letters.is_empty()
        && letters.bytes().all(|byte| byte.is_ascii_lowercase())
        && digit.len() == 1
        && digit.bytes().all(|byte| byte.is_ascii_digit())
}

/// What follows the pattern's `iso[a-z]{0,1}-` infix, when `rest` opens with it.
fn iso_infix_tail(rest: &str) -> Option<&str> {
    let after_iso = rest.strip_prefix("iso")?;
    if let Some(tail) = after_iso.strip_prefix('-') {
        return Some(tail);
    }
    let mut characters = after_iso.chars();
    let letter = characters.next()?;
    if !letter.is_ascii_lowercase() {
        return None;
    }
    characters.as_str().strip_prefix('-')
}

/// The region to mint this request's ARNs in: the region of the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned or scoped
/// to a region that would make the minted ARN fail the model's own `Arn` pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_matches_modeled_arn(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The next server-generated resource id, as the canonical 8-4-4-4-12 form AWS gives
/// a Resilience Hub application or resiliency policy.
///
/// Unique within this emulator: the low half is a monotonic counter, so two ids
/// minted in the same nanosecond still differ, and the high half is the minting
/// instant, so ids do not repeat across a restart. Server-generated, never derived
/// from the request.
fn mint_resource_id() -> String {
    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let sequence = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let minted_at = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (minted_at >> 32) as u32,
        (minted_at >> 16) & 0xffff,
        minted_at & 0xffff,
        (sequence >> 48) & 0xffff,
        sequence & 0xffff_ffff_ffff,
    )
}

/// The refusal message for an `additionalInfo` map whose `member` holds a value
/// list outside `AdditionalInfoValueList`'s modelled `@length` — `None` when every
/// entry is within it.
///
/// A collection's own `@length` is checked against the COLLECTION MEMBER's schema,
/// and a `map<string, list<string>>` member such as `AdditionalInfoMap` has no
/// separate schema for the inner list (the framework's own
/// `outer_element_count_bound_is_not_applied_to_a_nested_map` pins that), so the
/// boundary validator cannot reach this bound and a handler taking an
/// `AdditionalInfoMap` is where it is enforced.
///
/// The sentence is NOT written here: it is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, exactly as for
/// [`enum_violation`], so a handler refusal and a boundary refusal read
/// identically — including the `<n> items` form the framework displays a
/// collection's length with.
pub fn additional_info_violation(
    member: &str,
    additional_info: &HashMap<String, Vec<String>>,
) -> Option<String> {
    // Sorted, because a `HashMap` iterates in an arbitrary order and the same
    // request must be refused with the same sentence every time.
    let mut entries: Vec<(&String, &Vec<String>)> = additional_info.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));

    let violations: Vec<::smithy_server_runtime::Violation> = entries
        .into_iter()
        .filter_map(|(key, values)| {
            let kind = if values.len() < ADDITIONAL_INFO_VALUES_MIN {
                ::smithy_server_runtime::ViolationKind::MinLength(ADDITIONAL_INFO_VALUES_MIN as i64)
            } else if values.len() > ADDITIONAL_INFO_VALUES_MAX {
                ::smithy_server_runtime::ViolationKind::MaxLength(ADDITIONAL_INFO_VALUES_MAX as i64)
            } else {
                return None;
            };
            Some(::smithy_server_runtime::Violation {
                // A map ENTRY's path embeds the caller's key, so it is elided for
                // the same reason the value is.
                path: elided(&format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::entry(key)
                )),
                kind,
                value: Some(format!("{} items", values.len())),
            })
        })
        .collect();

    ::aws_smithy_schema_server::constraints::envelope(&violations)
}

/// Mint a fresh `appArn`: `arn:aws:resiliencehub:<region>:<account>:app/<id>`.
pub fn mint_app_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("app/{}", mint_resource_id()),
    )
}

/// Mint a fresh Application Component `id`, for a `CreateAppVersionAppComponent`
/// that named none.
///
/// Server-generated and never derived from the request, and shaped to satisfy the
/// `EntityName255` pattern `AppComponent$id` targets
/// (`^[A-Za-z0-9][A-Za-z0-9_\-]{0,254}$`, from `smithy_query query="shape"
/// shape="AppComponent"`): an alphanumeric head, then hyphens and hex digits.
pub fn mint_app_component_id() -> String {
    format!("appcomponent-{}", mint_resource_id())
}

/// Mint a fresh `policyArn`:
/// `arn:aws:resiliencehub:<region>:<account>:resiliency-policy/<id>`.
pub fn mint_policy_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("resiliency-policy/{}", mint_resource_id()),
    )
}

/// The refusal message for `value`, when the model's enum for `member` does not
/// list it — `None` when it does.
///
/// The boundary constraint validator already walks every enum MEMBER of an input
/// shape, so a handler never has to re-check one. What it does not walk is a
/// map's KEY: `DisruptionPolicy`'s key targets the `DisruptionType` enum, and an
/// unknown key reaches the handler. The sentence is NOT written here — it is built
/// from the framework's own [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, the one place that wording
/// lives, so a handler refusal and a boundary refusal read identically.
pub fn enum_violation(member: &str, value: &str, allowed: &[&str]) -> Option<String> {
    if allowed.contains(&value) {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        // Elided like the value: for a map KEY the path embeds caller-supplied
        // text, so it is the same amplification vector.
        path: elided(member),
        kind: ::smithy_server_runtime::ViolationKind::EnumValue(
            allowed.iter().map(|allowed| (*allowed).to_string()).collect(),
        ),
        // `Violation::value` must arrive ALREADY elided: the message interpolates
        // it verbatim, and this one is a caller-supplied string of any length.
        value: Some(elided(value)),
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
}

/// The longest caller-supplied text a refusal message echoes, pinned to the
/// boundary validator's own ceiling (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`).
const MAX_ECHOED_VALUE: usize = 256;

/// `text` as a refusal message may carry it: itself when short, a
/// character-boundary-safe prefix with an elision when not, so a huge request
/// member cannot be amplified into a huge response.
///
/// The framework's equivalent (`constraints::echoed`) is PRIVATE and lives under
/// the agent-immutable `vendor/` tree, so it cannot be called or widened from
/// here; the ceiling is therefore pinned to the same constant above.
fn elided(text: &str) -> String {
    if text.len() <= MAX_ECHOED_VALUE {
        return text.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &text[..end], text.len())
}

/// "Now", as the wire timestamp type every creation/evaluation time uses.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}
