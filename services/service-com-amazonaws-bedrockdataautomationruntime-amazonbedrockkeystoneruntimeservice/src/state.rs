// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice service.
//!
//! The service owns one resource, the asynchronous data-automation invocation
//! (`AutomationJobResource` in the model). `InvokeDataAutomationAsync` mints one and
//! persists it here; `GetDataAutomationStatus`, `TagResource`, `ListTagsForResource`
//! and `UntagResource` all resolve a caller-supplied ARN against
//! [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::invocations`]
//! and compute their answer from the stored record.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Blueprint, DataAutomationConfiguration, EncryptionConfiguration, NotificationConfiguration, Tag,
};

/// The region every ARN this service mints falls back to when the request carries no
/// SigV4 credential scope to read one from (an unsigned, host-routed request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `arnNamespace` (`aws.api#service`): every resource ARN is under
/// `arn:aws:bedrock:…`, which is also what the modelled `InvocationArn` pattern
/// requires.
const ARN_NAMESPACE: &str = "bedrock";

/// The resource portion prefix of an invocation ARN, as the modelled `InvocationArn` /
/// `TaggableResourceArn` pattern spells it
/// (`…:data-automation-invocation/[a-zA-Z0-9-_]+`).
const INVOCATION_RESOURCE_PREFIX: &str = "data-automation-invocation/";

/// How long a freshly submitted job stays `Created` before the clock owes it
/// `InProgress`.
///
/// AWS publishes no latency for this transition, so the span is CHOSEN: it sits inside
/// the emulator's 250–750 ms window for a clock-owed transition, so a caller that polls
/// immediately after `InvokeDataAutomationAsync` still observes the non-terminal
/// `Created` state rather than a job that was never pending.
pub const CREATED_FOR: Duration = Duration::from_millis(300);

/// How long a job stays `InProgress` before the clock owes it `Success`.
///
/// Chosen on the same basis as [`CREATED_FOR`]: AWS documents no duration for data
/// automation, and a real job's runtime depends on the asset, so this is the emulator's
/// own span inside the 250–750 ms window.
pub const IN_PROGRESS_FOR: Duration = Duration::from_millis(300);

/// The modelled `TagList` bound (`@length(min: 0, max: 200)`): the most tags one
/// resource can carry.
pub const MAX_TAGS: usize = 200;

/// The modelled `TagKey` pattern, verbatim (`smithy_query` shape `TagKey`).
///
/// Stated here because it has to be applied by hand: see [`tag_key_is_well_formed`].
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";

/// The tag-key prefix the modelled `TagKey` pattern excludes with its negative
/// lookahead — the namespace AWS reserves for its own tags. Stated for the refusal
/// documentation; the pattern itself is what [`tag_key_is_well_formed`] applies.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The characters the modelled [`TAG_KEY_PATTERN`]'s class lists literally, beside the
/// three Unicode general categories it names.
const TAG_KEY_PUNCTUATION: [char; 8] = ['_', '.', ':', '/', '=', '+', '-', '@'];

/// Whether `c` is admitted by the character-class half of [`TAG_KEY_PATTERN`]
/// (`[\p{L}\p{Z}\p{N}_.:/=+\-@]`), applied as the Unicode general categories the class
/// names:
///
/// * `\p{N}` is `char::is_numeric` exactly (`Nd`, `Nl`, `No`).
/// * `\p{Z}` is `char::is_whitespace` MINUS the control characters: `White_Space` is
///   `\p{Z}` plus `\t`, `\n`, `\u{b}`, `\u{c}`, `\r` and `\u{85}`, every one of which is
///   a control character, so subtracting `char::is_control` leaves the separators alone.
/// * `\p{L}` is `char::is_alphabetic` up to `Alphabetic`'s wider definition: that property
///   is `\p{L}` plus `Nl` — already admitted through `\p{N}` — plus `Other_Alphabetic`,
///   which is combining marks. A key carrying a combining mark is therefore accepted where
///   AWS's class would refuse it; erring toward accepting an exotic mark is the direction
///   the framework's own constraint walk takes for a pattern it cannot apply
///   (`vendor/smithy-schema-server/src/constraints.rs` treats such a pattern as matching
///   every value), and it keeps this check from refusing a key on a Unicode table
///   difference.
fn is_tag_key_char(c: char) -> bool {
    c.is_alphabetic()
        || c.is_numeric()
        || (c.is_whitespace() && !c.is_control())
        || TAG_KEY_PUNCTUATION.contains(&c)
}

/// Whether `key` satisfies the modelled [`TAG_KEY_PATTERN`].
///
/// This one modelled constraint cannot be left to the framework's boundary walk: the
/// pattern opens with the negative lookahead `(?!aws:)`, which the walk's regex engine
/// cannot compile, and a `@pattern` that fails to compile is treated as matching EVERY
/// value — which drops the character-class half of the pattern along with the lookahead
/// (`vendor/smithy-schema-server/src/constraints.rs`). So the pattern is applied here,
/// verbatim, by the three operations that accept a tag key (`InvokeDataAutomationAsync`
/// and `TagResource` through `tags`, `UntagResource` through `tagKeys`).
///
/// It is applied as the pattern's two independent halves rather than through a
/// look-around regex engine, because the pattern's ONLY look-around is its leading
/// `(?!aws:)`, which states exactly "the key does not begin with
/// [`RESERVED_TAG_KEY_PREFIX`]" — a prefix test. What remains is the plain character
/// class [`is_tag_key_char`] applies to every character, anchored at both ends by `*$`,
/// so the whole modelled constraint is enforced without a second regex engine in the
/// dependency graph.
pub fn tag_key_is_well_formed(key: &str) -> bool {
    !key.starts_with(RESERVED_TAG_KEY_PREFIX) && key.chars().all(is_tag_key_char)
}

/// The requirement clause this service states for a `@pattern` violation: the declared
/// pattern WITHOUT its outer `^`/`$` anchors, which a Smithy `@pattern` does not need
/// (the trait is a find, not a full match).
///
/// The anchor stripping itself is `service_common::validation::unanchored` — the staged,
/// shared home for that rendering, which drops the pair only when it wraps the WHOLE
/// expression and is not an escaped `$`, so an alternation that anchors each branch
/// (`^a$|^b$`) is left alone rather than mangled.
///
/// PENDING MIGRATION: the REQUIREMENT half below (the sentence per `ViolationKind`, with
/// [`violation_clause`], [`validation_envelope`] and `service::clause_for_violation`) is
/// this service's inlined copy of a service-independent validation-wording rule that
/// `services/common/src/validation.rs` stages but does not state yet — its "Still to
/// migrate here" section lists it as not yet there, so there is no shared helper to call
/// for it today. The fix that owns the migration (~24 crates carry the same copy) moves
/// those rules there and calls them from here. The note is left here, as that module
/// asks, because this change does not own that migration.
pub fn pattern_requirement(pattern: &str) -> String {
    let unanchored = service_common::validation::unanchored(pattern);
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// One clause of a refusal, as this service words it: the member it names and the
/// requirement that member failed, with the offending value NOT echoed.
///
/// Pending the migration noted on [`pattern_requirement`].
///
/// `path` is the framework's own input-path form (segments joined with `.`, with `member`
/// for the step from a list into its element).
pub fn violation_clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The whole `ValidationException` message around one or more [`violation_clause`]s, in
/// the AWS envelope (`1 validation error detected: …` / `N validation errors detected:
/// …; …`).
///
/// The envelope itself is a service dialect, but the clause halves it joins are not:
/// pending the migration noted on [`pattern_requirement`].
///
/// The ONE place this service's refusal envelope is written: the boundary walk's
/// refusals reach it through `service::override_refusal`, and a rule the walk cannot
/// express (the `TagKey` lookahead) reaches it through [`malformed_tag_key_message`], so
/// one logical failure cannot read two different ways depending on which side caught it.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The `ValidationException` message for a tag key outside [`TAG_KEY_PATTERN`], at input
/// path `path`.
pub fn malformed_tag_key_message(path: &str) -> String {
    validation_envelope(&[violation_clause(
        path,
        &pattern_requirement(TAG_KEY_PATTERN),
    )])
}

/// Why a submission that names neither a data-automation project nor an inline blueprint
/// is refused.
///
/// This service's own fixed string for that refusal: it carries NOTHING from the
/// request, so reproducing it is fidelity rather than fitting one recorded call. What a
/// client branches on is the HTTP 400 and the `ValidationException` code.
pub const MISSING_PROJECT_OR_BLUEPRINTS: &str =
    "At least one of project or inline blueprints must be provided in the request.";

/// Why a submission that names BOTH a data-automation project and an inline blueprint is
/// refused. A fixed string on the same footing as [`MISSING_PROJECT_OR_BLUEPRINTS`].
///
/// NO RECORDING covers this half of the rule: the corpus only records the "names
/// neither" refusal. It is stated because the two members describe two different
/// extraction targets and the service has no documented tie-break, but a later recording
/// showing AWS accepting both should settle it — and would change only this branch of
/// [`extraction_target_refusal`], not the "names neither" one the corpus pins.
pub const BOTH_PROJECT_AND_BLUEPRINTS: &str =
    "Only one of project or inline blueprints can be provided in the request.";

/// The refusal a submission's extraction members earn, or `None` when they are
/// acceptable.
///
/// A data-automation submission has to say WHAT to extract, and it says so in exactly
/// ONE of two places: a data-automation project
/// (`dataAutomationConfiguration.dataAutomationProjectArn`) or an inline `blueprints`
/// list. Naming neither describes no extraction; naming both describes two, and the
/// service does not choose between them. Both members are individually optional in the
/// model — no Smithy trait expresses "exactly one of" — so this cross-member rule is the
/// service's, and it is single-homed here because BOTH invoke operations
/// (`InvokeDataAutomation` and `InvokeDataAutomationAsync`) apply it identically.
///
/// `blueprints` carries `@length(min: 1)`, so an empty list is already refused by the
/// boundary walk; it counts here as "no inline blueprint" so the rule holds whatever
/// reaches a handler.
pub fn extraction_target_refusal(
    data_automation_configuration: Option<&DataAutomationConfiguration>,
    blueprints: Option<&[Blueprint]>,
) -> Option<&'static str> {
    let names_project = data_automation_configuration
        .is_some_and(|configuration| !configuration.data_automation_project_arn.is_empty());
    let names_blueprint = blueprints.is_some_and(|blueprints| !blueprints.is_empty());
    match (names_project, names_blueprint) {
        (false, false) => Some(MISSING_PROJECT_OR_BLUEPRINTS),
        (true, true) => Some(BOTH_PROJECT_AND_BLUEPRINTS),
        _ => None,
    }
}

/// What this service answers when a caller-supplied ARN names no retained invocation —
/// the one message behind `ResourceNotFoundException` for every operation here that
/// resolves an ARN (`GetDataAutomationStatus`, `TagResource`, `ListTagsForResource`,
/// `UntagResource`).
///
/// It is the service's own fixed string and carries NOTHING from the request: the ARN
/// the caller sent is not interpolated, because this service does not echo it back. Kept
/// in one place so the four readers cannot drift apart.
pub const INVOCATION_NOT_FOUND_MESSAGE: &str = "Data Automation Invocation ARN not found.";

/// The most invocation records this emulator retains at once.
///
/// Every `InvokeDataAutomationAsync` call persists one record, and one `clientToken`
/// entry beside it, on a request-reachable path, so without a ceiling the store would
/// grow with traffic forever. The capacity is the emulator's own (AWS publishes no such
/// quota), and it bounds BOTH maps: it is enforced by
/// [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::admit_invocation`],
/// the one admission path, which evicts finished jobs (and their tokens) to make room
/// and refuses the submission when it cannot.
pub const MAX_INVOCATIONS: usize = 1_000;

/// The `AutomationJobStatus` enum, as the model declares its values.
///
/// Held as an enum rather than a string so the clock-driven progression in
/// [`Invocation::advance_to`] cannot drift onto a value the model does not declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationJobStatus {
    /// The submission was accepted and the job has not started yet.
    Created,
    /// The job is running.
    InProgress,
    /// The job finished successfully.
    Success,
    /// The job failed for a reason attributable to the caller's request.
    ClientError,
    /// The job failed inside the service.
    ServiceError,
}

impl AutomationJobStatus {
    /// The wire value, exactly as `AUTOMATION_JOB_STATUS_VALUES` lists it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ClientError => "ClientError",
            Self::Created => "Created",
            Self::InProgress => "InProgress",
            Self::ServiceError => "ServiceError",
            Self::Success => "Success",
        }
    }
}

/// One asynchronous data-automation invocation: everything the submit call accepted,
/// plus the job's own lifecycle.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The ARN this invocation is addressed by — the key it is stored under.
    pub invocation_arn: String,
    /// The opaque id in the ARN's resource portion.
    pub invocation_id: String,
    /// `inputConfiguration.s3Uri` as submitted.
    pub input_s3_uri: String,
    /// `outputConfiguration.s3Uri` as submitted; what `GetDataAutomationStatus`
    /// reports as its `outputConfiguration`.
    pub output_s3_uri: String,
    /// `dataAutomationConfiguration.dataAutomationProjectArn`, when one was given.
    pub data_automation_project_arn: Option<String>,
    /// `dataAutomationConfiguration.stage` (a `DataAutomationStage` value).
    pub stage: Option<String>,
    /// `dataAutomationProfileArn` as submitted (required by the model).
    pub data_automation_profile_arn: String,
    /// `blueprints` as submitted; empty when the member was absent.
    pub blueprints: Vec<Blueprint>,
    /// `encryptionConfiguration` as submitted.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `notificationConfiguration` as submitted.
    pub notification_configuration: Option<NotificationConfiguration>,
    /// Where the job is in its lifecycle. Advanced only by [`Invocation::advance_to`].
    pub status: AutomationJobStatus,
    /// `errorType`, set only on a failure status.
    pub error_type: Option<String>,
    /// `errorMessage`, set only on a failure status.
    pub error_message: Option<String>,
    /// When the submission was accepted.
    pub job_submission_time: SystemTime,
    /// When the job reached a terminal status; `None` while it is still running.
    pub job_completion_time: Option<SystemTime>,
    /// The resource's tags, keyed by tag key — the map `TagResource` merges into and
    /// `UntagResource` removes from. A `BTreeMap` so `ListTagsForResource` answers in a
    /// stable order.
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` this job was submitted under, when the caller sent one.
    pub client_token: Option<String>,
}

impl Invocation {
    /// A job accepted at `job_submission_time`, in its initial `Created` status, with
    /// every optional member absent. The caller fills in the optional members it was
    /// given.
    pub fn new(
        invocation_arn: String,
        invocation_id: String,
        input_s3_uri: String,
        output_s3_uri: String,
        data_automation_profile_arn: String,
        job_submission_time: SystemTime,
    ) -> Self {
        Self {
            invocation_arn,
            invocation_id,
            input_s3_uri,
            output_s3_uri,
            data_automation_project_arn: None,
            stage: None,
            data_automation_profile_arn,
            blueprints: Vec::new(),
            encryption_configuration: None,
            notification_configuration: None,
            status: AutomationJobStatus::Created,
            error_type: None,
            error_message: None,
            job_submission_time,
            job_completion_time: None,
            tags: BTreeMap::new(),
            client_token: None,
        }
    }

    /// Materialise every lifecycle transition the clock already owes this job at `now`.
    ///
    /// The transitions are owed by the passage of time since the submission that
    /// started them, not by anyone reading the job, so applying them from a reporting
    /// operation is what keeps repeated reads consistent: the first read that finds a
    /// transition due records it, and every later read sees the same status and the same
    /// `job_completion_time`. The completion instant stored is the instant the
    /// transition became DUE, not the instant it was observed, so the derived
    /// `jobDurationInSeconds` does not depend on when a caller happened to poll.
    ///
    /// What `Success` CLAIMS here: the invocation RECORD reached its terminal state. It
    /// does not claim that analysis ran or that result objects exist under the submitted
    /// `outputConfiguration.s3Uri` — those objects live in a service this crate does not
    /// emulate, and nothing here writes them or synthesises analysis output (the
    /// synchronous `InvokeDataAutomation`, which would have to RETURN such output, stays
    /// a 501 for exactly that reason). This operation reports the lifecycle of the job
    /// record, which is the control-plane fact the emulator does own.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.status == AutomationJobStatus::Created
            && now >= self.job_submission_time + CREATED_FOR
        {
            self.status = AutomationJobStatus::InProgress;
        }
        if self.status == AutomationJobStatus::InProgress {
            let completion_due = self.job_submission_time + CREATED_FOR + IN_PROGRESS_FOR;
            if now >= completion_due {
                self.status = AutomationJobStatus::Success;
                self.job_completion_time = Some(completion_due);
            }
        }
    }

    /// Whether the job has reached a status it can never leave. Only a terminal job is
    /// a candidate for eviction: dropping one still running would lose a job a caller is
    /// waiting on.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            AutomationJobStatus::Success
                | AutomationJobStatus::ClientError
                | AutomationJobStatus::ServiceError
        )
    }

    /// `jobDurationInSeconds`: whole seconds between the two stored timestamps, and
    /// `None` while the job has no completion time. Derived on every read, never stored.
    ///
    /// The emulated lifecycle spans ([`CREATED_FOR`] + [`IN_PROGRESS_FOR`]) total under a
    /// second, so a successful job here reports `0`: the value is consistent with the two
    /// timestamps it is derived from, and is not a duration to assert a magnitude on.
    pub fn job_duration_in_seconds(&self) -> Option<i32> {
        let completion = self.job_completion_time?;
        let elapsed = completion.duration_since(self.job_submission_time).ok()?;
        i32::try_from(elapsed.as_secs()).ok()
    }

    /// The tag map as the modelled `list<Tag>`, in tag-key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice. A handler reaches it through [`service_state`].
///
/// BOTH maps are private, and the only way to add to either of them is
/// [`Self::admit_invocation`], which admits one invocation and its `clientToken`
/// TOGETHER and only after it has freed a slot. That is what bounds this store: no
/// request-reachable path can insert into one map without the other, so
/// `client_tokens` can never outgrow `invocations`, and `invocations` can never exceed
/// [`MAX_INVOCATIONS`]. A handler reads a record through [`Self::invocation`] /
/// [`Self::invocation_mut`], which cannot grow anything.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState {
    /// Every invocation this emulator has minted, keyed by its `invocationArn`. Bounded
    /// at [`MAX_INVOCATIONS`] by [`Self::admit_invocation`].
    invocations: HashMap<String, Invocation>,
    /// `clientToken` -> `invocationArn`, which is what makes
    /// `InvokeDataAutomationAsync` (`@idempotent`) replay instead of minting a second
    /// job for a token it has already seen.
    ///
    /// An entry is written ONLY by [`Self::admit_invocation`], together with the
    /// invocation it points at, and is dropped with that invocation by
    /// [`Self::forget_invocation`] — including when the invocation is evicted to free a
    /// slot. So this map is a subset of `invocations`' keys by construction and inherits
    /// its [`MAX_INVOCATIONS`] bound: a caller submitting a fresh token on every request
    /// cannot make it grow past the cap.
    client_tokens: HashMap<String, String>,
    /// How many invocations this store has ever minted an id for — the monotonic half of
    /// [`Self::mint_invocation_id`]. It counts mints, not retained records, so it keeps
    /// rising past an eviction and an id is never handed out twice.
    minted_invocations: u64,
}

impl ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState {
    /// Mint the opaque resource id for one new invocation.
    ///
    /// Built from this store's own monotonic mint count and the submission instant, and
    /// from NOTHING the request carries: two identical payloads submitted under
    /// different tokens therefore get different ids, and the count means an id is never
    /// reused even after the record it named was retired. Lower-case hex and one hyphen,
    /// so the id is inside the modelled `InvocationArn` pattern's `[a-zA-Z0-9-_]+`
    /// resource segment and the whole ARN stays well inside its `@length(1..128)`.
    pub fn mint_invocation_id(&mut self, now: SystemTime) -> String {
        self.minted_invocations = self.minted_invocations.wrapping_add(1);
        let since_epoch = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{since_epoch:016x}-{:016x}", self.minted_invocations)
    }

    /// The invocation `invocation_arn` names, for a reporting read.
    pub fn invocation(&self, invocation_arn: &str) -> Option<&Invocation> {
        self.invocations.get(invocation_arn)
    }

    /// The invocation `invocation_arn` names, for a caller that edits the record in
    /// place (a tag edit, or materialising a transition the clock owes). Reaching an
    /// existing record can never grow the store.
    pub fn invocation_mut(&mut self, invocation_arn: &str) -> Option<&mut Invocation> {
        self.invocations.get_mut(invocation_arn)
    }

    /// How many invocations are retained; at most [`MAX_INVOCATIONS`].
    pub fn invocation_count(&self) -> usize {
        self.invocations.len()
    }

    /// How many `clientToken` entries are retained; never more than
    /// [`Self::invocation_count`].
    pub fn client_token_count(&self) -> usize {
        self.client_tokens.len()
    }

    /// The `invocationArn` an already-accepted `clientToken` must replay, if any.
    ///
    /// The token ALONE identifies the submission: this model declares no
    /// `IdempotentParameterMismatch`-style error for the operation, so a reused token is
    /// answered with the job it already minted whatever else the request carries, rather
    /// than with an error the model has no shape for.
    ///
    /// A token whose invocation is no longer retained is NOT replayed: handing back an
    /// ARN no reader can resolve would turn an idempotent retry into a 404 from the very
    /// next `GetDataAutomationStatus`. Such a token is dropped here, so the caller mints
    /// a fresh job instead and the map does not keep dead entries.
    pub fn replay_for_client_token(&mut self, client_token: &str) -> Option<String> {
        let invocation_arn = self.client_tokens.get(client_token)?.clone();
        if self.invocations.contains_key(&invocation_arn) {
            return Some(invocation_arn);
        }
        self.client_tokens.remove(client_token);
        None
    }

    /// Admit one freshly minted invocation, with its `clientToken` (taken from the
    /// record itself, so the two can never be written apart).
    ///
    /// This is the ONLY way anything is added to this store. It first frees a slot,
    /// bounding `invocations` at [`MAX_INVOCATIONS`]; `false` means every retained job
    /// is still running, so the caller must refuse the submission with the modelled
    /// quota error rather than drop work somebody is waiting on — and nothing at all is
    /// inserted, so a refused submission grows neither map.
    pub fn admit_invocation(&mut self, invocation: Invocation, now: SystemTime) -> bool {
        if !self.reserve_invocation_slot(now) {
            return false;
        }
        let invocation_arn = invocation.invocation_arn.clone();
        if let Some(client_token) = invocation.client_token.clone() {
            self.client_tokens
                .insert(client_token, invocation_arn.clone());
        }
        self.invocations.insert(invocation_arn, invocation);
        debug_assert!(
            self.client_tokens.len() <= self.invocations.len()
                && self.invocations.len() <= MAX_INVOCATIONS,
            "both maps stay bounded by MAX_INVOCATIONS"
        );
        true
    }

    /// Make room for one more invocation, bounding the store at [`MAX_INVOCATIONS`].
    ///
    /// At capacity it materialises what the clock owes each stored job and then evicts
    /// the longest-submitted FINISHED one, repeating until there is room. `false` means
    /// every retained job is still running. Private: admission goes through
    /// [`Self::admit_invocation`], so freeing a slot and filling it cannot come apart.
    fn reserve_invocation_slot(&mut self, now: SystemTime) -> bool {
        while self.invocations.len() >= MAX_INVOCATIONS {
            let evictable = self
                .invocations
                .values_mut()
                .filter_map(|invocation| {
                    invocation.advance_to(now);
                    invocation
                        .is_terminal()
                        .then(|| (invocation.job_submission_time, invocation.invocation_arn.clone()))
                })
                .min_by_key(|(submitted, _)| *submitted);
            match evictable {
                Some((_, arn)) => self.forget_invocation(&arn),
                None => return false,
            }
        }
        true
    }

    /// Drop one invocation and every `clientToken` entry that named it, so no token is
    /// left pointing at a record that is gone — which is what keeps `client_tokens` a
    /// subset of `invocations` across eviction.
    pub fn forget_invocation(&mut self, invocation_arn: &str) {
        self.invocations.remove(invocation_arn);
        self.client_tokens
            .retain(|_, stored| stored != invocation_arn);
    }
}

/// The ARN of an invocation with resource id `invocation_id` in `region`.
///
/// Built through `emulator_server::emulator_arn`, so the account segment is the one
/// canonical emulator account id. The resource id is never request-supplied — the only
/// caller mints it with
/// [`ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::mint_invocation_id`],
/// whose hex-and-hyphen text is inside the modelled `InvocationArn` pattern's
/// `[a-zA-Z0-9-_]+` resource segment — and `region` has already been reduced to that
/// pattern's `[a-zA-Z0-9-]*` charset by [`request_region`], so the result satisfies the
/// modelled pattern for every input this service produces.
pub fn invocation_arn(region: &str, invocation_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{INVOCATION_RESOURCE_PREFIX}{invocation_id}"),
    )
}

/// The region to mint an ARN in for this request: the SigV4 credential scope's region,
/// else [`DEFAULT_REGION`].
///
/// The scope region is accepted only when it is non-empty and entirely within the
/// modelled `InvocationArn` pattern's `[a-zA-Z0-9-]*` region charset; anything else
/// falls back to [`DEFAULT_REGION`] rather than being spliced into an ARN. This is a
/// charset check, NOT a check that the label names a real AWS region: the emulator
/// serves whatever region a caller signs for, and the only property an ARN needs is
/// that it satisfies the modelled pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState>)
}
