// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexmodelbuildingservice-awsdeepsensemodelbuildingservice service.
//!
//! This holds the bot-authoring records the Lex Model Building control plane persists:
//! every `Bot` and every `Intent`, each keyed by name and then by version (`$LATEST`
//! plus the numbered versions published from it). Every handler in `src/ops/` computes
//! its response from these records; nothing is synthesised per request.
//!
//! Leaf values that the model already declares (`Intent`, `Prompt`, `Statement`, `Slot`,
//! `Tag`, …) are stored as the modelled shapes from [`crate::ops::types`] rather than as
//! a second hand-written mirror of the same members: two copies of one shape is how two
//! operations end up reading two different records.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    CodeHook, FollowUpPrompt, FulfillmentActivity, InputContext, Intent, KendraConfiguration,
    OutputContext, Prompt, Slot, Statement, Tag,
};

/// The version every write targets, and the one a numbered version is published from.
/// Both `PutBot` and `PutIntent` are modelled as `PUT /…/versions/$LATEST`.
pub const LATEST_VERSION: &str = "$LATEST";

// The `Status` values this slice can report, from the model's `Status` enum
// (`STATUS_VALUES` in `ops::types::shared_types`).

/// `processBehavior=SAVE`: "When you set processBehavior to SAVE, Amazon Lex sets the
/// status code to NOT BUILT." (`PutBotResponse$status` documentation.)
pub const STATUS_NOT_BUILT: &str = "NOT_BUILT";

/// `processBehavior=BUILD`: "When you send a request to create a bot with
/// processBehavior set to BUILD, Amazon Lex sets the status response element to
/// BUILDING." (`PutBotResponse$status` documentation.)
pub const STATUS_BUILDING: &str = "BUILDING";

/// The status a finished build lands in: "When the bot is in the READY state you can
/// test and publish the bot." (`PutBotResponse$status` documentation.)
pub const STATUS_READY: &str = "READY";

/// The status a build that cannot succeed lands in: "If Amazon Lex can't build the bot,
/// Amazon Lex sets status to FAILED. Amazon Lex returns the reason for the failure in
/// the failureReason response element." (`PutBotResponse$status` documentation.)
pub const STATUS_FAILED: &str = "FAILED";

/// `ProcessBehavior$BUILD`, and the value `processBehavior` defaults to: "If you don't
/// specify this value, the default value is BUILD." (`PutBotRequest$processBehavior`.)
pub const PROCESS_BEHAVIOR_BUILD: &str = "BUILD";

/// `ProcessBehavior$SAVE`.
pub const PROCESS_BEHAVIOR_SAVE: &str = "SAVE";

/// The `idleSessionTTLInSeconds` a request that omits it is stored with. From the Amazon
/// Lex `PutBot` documentation for that member: "If you don't include the
/// idleSessionTTLInSeconds element in a PutBot operation request, Amazon Lex uses the
/// default value. This is also true if the request replaces an existing bot. The default
/// is 300 seconds (5 minutes)."
pub const DEFAULT_IDLE_SESSION_TTL_SECONDS: i32 = 300;

/// `detectSentiment` for a request that omits it: "If you don't specify detectSentiment,
/// the default is false." (`PutBotRequest$detectSentiment`.)
pub const DEFAULT_DETECT_SENTIMENT: bool = false;

/// How long a `processBehavior=BUILD` build takes in this emulator before the bot moves
/// from `BUILDING` to `READY`.
///
/// AWS publishes no duration for a Lex build, so this is CHOSEN: the emulator's span for
/// a transition the clock owes. It is deliberately non-zero, so a caller that reads the
/// bot straight back still observes the transient `BUILDING` status the real service
/// reports.
pub const BUILD_DURATION: Duration = Duration::from_millis(500);

/// `GetBots`' page size when the request omits `maxResults`: "The maximum number of bots
/// to return in the response that the request will return. The default is 10."
/// (`GetBotsRequest$maxResults` documentation.)
pub const DEFAULT_PAGE_SIZE: i32 = 10;

/// The largest `GetBots` page, from `GetBotsRequest$maxResults`' modelled `range`
/// (1..=50). The boundary validator already refuses anything outside it; the clamp here
/// is what keeps the page arithmetic in range regardless.
pub const MAX_PAGE_SIZE: i32 = 50;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete, so
// each growth site carries a cap and nothing caller-sized reaches the store unchecked.
// The model already bounds most caller-sized members (`slots` 0..100, `sampleUtterances`
// 0..1500, `tags` 0..200, `description` 0..200, every `messages` list 1..15); the caps
// here cover the ones it leaves open, and each says whether it is an AWS quota or this
// emulator's own resource guard.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT a published AWS quota: the most bots this emulator
/// stores for one account.
pub const MAX_BOTS: usize = 100;

/// Emulator resource guard, NOT a published AWS quota: the most intents this emulator
/// stores for one account.
pub const MAX_INTENTS: usize = 1000;

/// Emulator resource guard, NOT a published AWS quota: the most numbered versions this
/// emulator publishes for one bot or intent, beside its `$LATEST`.
pub const MAX_VERSIONS: usize = 100;

/// Emulator resource guard, NOT a published AWS quota: the most `intents` entries a bot
/// may carry. `IntentList` carries no modelled `length`.
pub const MAX_INTENTS_PER_BOT: usize = 100;

/// Emulator resource guard, NOT a published AWS quota: the longest caller-supplied string
/// this emulator stores for a member the model declares no `length` on (`voiceId`,
/// `checksum`, `parentIntentSignature`, `nextToken`).
pub const MAX_STRING_CHARS: usize = 512;

/// Refuse a caller-sized collection whose entry count would grow the store past `max`.
///
/// `Err` carries the message, naming the member and the bound it broke, for the caller to
/// fix the request from.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Per-service state for com-amazonaws-lexmodelbuildingservice-awsdeepsensemodelbuildingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState {
    /// Every stored bot, keyed by bot name. A `BTreeMap`, so `GetBots` can page a stable
    /// name-sorted sequence without sorting the whole store per request.
    pub bots: BTreeMap<String, BotEntry>,
    /// Every stored intent, keyed by intent name.
    pub intents: BTreeMap<String, IntentEntry>,
    /// Drives [`ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState::mint_checksum`]: the number of
    /// revisions this store has written, so each write mints a distinct checksum.
    pub revision_counter: u64,
}

/// One stored bot: its versions, the next numbered version to publish, and the tags that
/// were attached when it was created.
#[derive(Debug, Clone, Default)]
pub struct BotEntry {
    /// This bot's versions, keyed by `$LATEST` or by a numeric version.
    pub versions: BTreeMap<String, BotRecord>,
    /// The number the next published version gets; the first published version is `1`.
    pub next_version: u64,
    /// The bot's tags. On the entry rather than on a version because `PutBot` documents
    /// "You can only add tags when you create a bot, you can't use the PutBot operation
    /// to update the tags on a bot. To update tags, use the TagResource operation."
    pub tags: Vec<Tag>,
}

/// One version of one stored bot. Every member a `GetBot` or `PutBot` body reports is
/// read from here.
#[derive(Debug, Clone)]
pub struct BotRecord {
    pub name: String,
    pub description: Option<String>,
    pub intents: Vec<Intent>,
    pub enable_model_improvements: Option<bool>,
    pub nlu_intent_confidence_threshold: Option<f64>,
    pub clarification_prompt: Option<Prompt>,
    pub abort_statement: Option<Statement>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub idle_session_ttl_in_seconds: i32,
    pub voice_id: Option<String>,
    pub locale: String,
    pub child_directed: bool,
    pub detect_sentiment: bool,
    pub version: String,
    pub checksum: String,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
    /// When this bot's build is due to finish, for a record whose status is
    /// [`STATUS_BUILDING`]; `None` in every other status. Read by
    /// [`ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState::materialise_due_builds`].
    pub build_due_at: Option<SystemTime>,
}

/// One stored intent: its versions and the next numbered version to publish.
#[derive(Debug, Clone, Default)]
pub struct IntentEntry {
    pub versions: BTreeMap<String, IntentRecord>,
    pub next_version: u64,
}

/// One version of one stored intent.
#[derive(Debug, Clone)]
pub struct IntentRecord {
    pub name: String,
    pub description: Option<String>,
    pub slots: Vec<Slot>,
    pub sample_utterances: Vec<String>,
    pub confirmation_prompt: Option<Prompt>,
    pub rejection_statement: Option<Statement>,
    pub follow_up_prompt: Option<FollowUpPrompt>,
    pub conclusion_statement: Option<Statement>,
    pub dialog_code_hook: Option<CodeHook>,
    pub fulfillment_activity: Option<FulfillmentActivity>,
    pub parent_intent_signature: Option<String>,
    pub kendra_configuration: Option<KendraConfiguration>,
    pub input_contexts: Vec<InputContext>,
    pub output_contexts: Vec<OutputContext>,
    pub version: String,
    pub checksum: String,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
}

/// One page of a name-sorted sequence, as `GetBots` reports it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The names on this page, in the sequence's own order.
    pub names: Vec<String>,
    /// The `nextToken` the NEXT page starts at; absent on the last page.
    pub next_token: Option<String>,
}

/// Cut the page a caller asked for out of `ordered`, a name-sorted sequence.
///
/// `next_token` is the name the page starts at — the cursor a previous page handed back.
/// Because the sequence is sorted, the page starts at the first name that is not ordered
/// BEFORE the cursor, so a token stays usable even when the record it named was deleted
/// between the two calls. `max_results` defaults to [`DEFAULT_PAGE_SIZE`] and is clamped
/// into the modelled `1..=`[`MAX_PAGE_SIZE`] range.
pub fn page_of(ordered: &[String], next_token: Option<&str>, max_results: Option<i32>) -> Page {
    let start = match next_token {
        None => 0,
        Some(cursor) => ordered.partition_point(|name| name.as_str() < cursor),
    };
    let size = max_results
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let size = usize::try_from(size).unwrap_or(1);
    let end = start.saturating_add(size).min(ordered.len());
    Page {
        names: ordered[start..end].to_vec(),
        next_token: ordered.get(end).cloned(),
    }
}

impl ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState {
    /// Mint the checksum of a fresh revision: "Identifies a specific revision of the
    /// $LATEST version" (`PutBotRequest$checksum`).
    ///
    /// One revision counter drives every write in the store, so no two revisions of any
    /// resource can share a checksum; the counter is rendered through SHA-256 as the
    /// opaque UUID-shaped token the service hands back, and the caller echoes it on the
    /// next conditional write. The value is STORED on the record it stamps — a checksum
    /// that is not saved could never be compared against.
    pub fn mint_checksum(&mut self, kind: &str, name: &str, version: &str) -> String {
        self.revision_counter = self.revision_counter.wrapping_add(1);
        let digest = Sha256::digest(
            format!("{kind}\u{1f}{name}\u{1f}{version}\u{1f}{}", self.revision_counter).as_bytes(),
        );
        let rendered = hex::encode(&digest[..16]);
        format!(
            "{}-{}-{}-{}-{}",
            &rendered[0..8],
            &rendered[8..12],
            &rendered[12..16],
            &rendered[16..20],
            &rendered[20..32],
        )
    }

    /// Finish every build the CLOCK has already made due: a bot left in
    /// [`STATUS_BUILDING`] whose [`BotRecord::build_due_at`] has passed is now
    /// [`STATUS_READY`].
    ///
    /// Called by the reporting operations before they read, which is the one write a
    /// report may make: the transition was owed at this instant whether or not anybody
    /// called. Nothing else about the record moves.
    pub fn materialise_due_builds(&mut self, now: SystemTime) {
        for entry in self.bots.values_mut() {
            for record in entry.versions.values_mut() {
                let due = record
                    .build_due_at
                    .is_some_and(|due| now >= due && record.status == STATUS_BUILDING);
                if due {
                    record.status = STATUS_READY.to_string();
                    record.build_due_at = None;
                }
            }
        }
    }

    /// Whether `version` of the intent named `name` is stored — what a bot's `intents`
    /// references are resolved against when the bot is built.
    pub fn intent_version_exists(&self, name: &str, version: &str) -> bool {
        self.intents
            .get(name)
            .is_some_and(|entry| entry.versions.contains_key(version))
    }

    /// The names of the stored bots a `GetBots` request selects, in name order: every
    /// bot, or the ones whose name contains `name_contains`.
    ///
    /// `nameContains` is a plain substring test — "A bot will be returned if any part of
    /// its name matches the substring" (`GetBotsRequest$nameContains`) — matched
    /// case-sensitively, the way `DeleteBot` documents a bot name is compared ("The name
    /// is case sensitive").
    pub fn bot_names_matching(&self, name_contains: Option<&str>) -> Vec<String> {
        self.bots
            .keys()
            .filter(|name| name_contains.is_none_or(|needle| name.contains(needle)))
            .cloned()
            .collect()
    }
}

/// The `NotFoundException` message for a bot name (and version or alias) that names no
/// stored record. Shared so every operation that resolves a bot says the same thing
/// about the same failure.
pub fn no_such_bot(name: &str, version_or_alias: &str) -> String {
    format!("Not Found: the bot {name} has no version or alias {version_or_alias}")
}

/// The `NotFoundException` message for a bot name with no stored bot at all.
pub fn no_such_bot_name(name: &str) -> String {
    format!("Not Found: there is no bot named {name}")
}

/// The `NotFoundException` message for an intent name and version that name no stored
/// record.
pub fn no_such_intent(name: &str, version: &str) -> String {
    format!("Not Found: the intent {name} has no version {version}")
}

/// The `BadRequestException` message for a create that supplied a `checksum`: "When you
/// create a new bot, leave the checksum field blank. If you specify a checksum you get a
/// BadRequestException exception." (`PutBotRequest$checksum`.)
pub fn checksum_on_create(resource: &str, name: &str) -> String {
    format!(
        "Bad Request: leave the checksum field blank when creating the {resource} {name}; \
         it has no $LATEST revision to match"
    )
}

/// The `PreconditionFailedException` message for an update whose `checksum` does not
/// identify the stored revision: "If you don't specify the checksum field, or if the
/// checksum does not match the $LATEST version, you get a PreconditionFailedException
/// exception." (`PutBotRequest$checksum`.)
pub fn checksum_mismatch(resource: &str, name: &str, supplied: Option<&str>) -> String {
    match supplied {
        Some(supplied) => format!(
            "Precondition Failed: the checksum {supplied} does not match the current \
             revision of the $LATEST version of the {resource} {name}"
        ),
        None => format!(
            "Precondition Failed: updating the $LATEST version of the {resource} {name} \
             requires the checksum of its current revision"
        ),
    }
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the com-amazonaws-lexmodelbuildingservice-awsdeepsensemodelbuildingservice emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLexmodelbuildingserviceAwsdeepsensemodelbuildingserviceState>)
}
