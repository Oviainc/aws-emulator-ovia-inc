// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexruntimeservice-awsdeepsenseruntimeservice service.
//!
//! The Lex runtime is a conversation store keyed by (bot name, bot alias, user id):
//! `PutSession` creates or modifies the session, `PostText`/`PostContent` advance it a
//! turn, `GetSession` reports it and `DeleteSession` removes it. Everything the four
//! implemented handlers answer with is computed from the [`LexSession`] held here, so a
//! write by one request is what the next request reads.
//!
//! What this emulator does NOT have is a bot definition: Amazon Lex's natural-language
//! understanding resolves an utterance against the intents, slot types and prompts of a
//! bot built through the (separate) Lex *model-building* service, which this emulator does
//! not host. No intent recognition is therefore performed or invented here — the dialog a
//! session is in is the one the client set through `PutSession`'s `dialogAction`, and
//! `PostText` reports that persisted dialog rather than fabricating a recognised intent,
//! a confidence score or a sentiment. The parts of the runtime that do not need a bot
//! definition are real: session identity, session attributes, the recent-intent summary
//! view, and active contexts with their documented time- and turn-based expiry.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The bot version a runtime session reports when no model-building service has published
/// one. `$LATEST` is the draft version every Lex bot has, and it satisfies `BotVersion`'s
/// modeled pattern `^[0-9]+|\$LATEST$`.
pub const DEFAULT_BOT_VERSION: &str = "$LATEST";

/// `IntentSummaryList`'s modeled maximum length (`len 0..3`), which bounds both
/// `PutSession`'s input view and `GetSession`'s output view.
pub const MAX_RECENT_INTENTS: usize = 3;

/// `ActiveContextsList`'s modeled maximum length (`len 0..20`).
pub const MAX_ACTIVE_CONTEXTS: usize = 20;

/// The `Accept` values Amazon Lex documents for `PutSession`/`PostContent`:
/// "The following are the accepted values: audio/mpeg audio/ogg audio/pcm audio/*
/// (defaults to mpeg) text/plain; charset=utf-8".
pub const ACCEPTED_ACCEPT_VALUES: &[&str] = &[
    "audio/mpeg",
    "audio/ogg",
    "audio/pcm",
    "audio/*",
    "text/plain; charset=utf-8",
];

/// The `Accept` assumed when a request sends none: the one documented value that asks for
/// a text response, which is the only form this emulator can produce (it has no speech
/// synthesis dependency).
pub const DEFAULT_ACCEPT: &str = "text/plain; charset=utf-8";

/// The most conversations this store holds at once.
///
/// Every session is created by a request, keyed by values the caller chooses, so the store
/// needs a declared ceiling or a caller varying `userId` grows it without bound. At the
/// cap a new session makes room by giving up the least recently updated one — a
/// conversation nobody has touched for longest — which keeps the store bounded without
/// refusing a live caller. A chosen emulator ceiling, far above any test or demo workload;
/// AWS publishes no such limit.
pub const MAX_SESSIONS: usize = 1024;

/// The most turns one session keeps.
///
/// `turns` is the recent conversation history, and every turn holds the caller's
/// `inputText` (modeled `len 1..1024`), so it is a rolling window rather than a log that
/// grows for as long as the conversation lasts: past this many, the oldest turn is
/// dropped. A chosen emulator bound, not an AWS limit.
pub const MAX_TURNS_PER_SESSION: usize = 100;

/// The most session attributes one session holds.
///
/// `sessionAttributes` is a modeled `map<string,string>` with no modeled bound, and
/// `PostText` merges the request's entries into the stored map, so distinct keys would
/// otherwise accumulate for as long as the session lives. A chosen emulator bound, not an
/// AWS limit.
pub const MAX_SESSION_ATTRIBUTES: usize = 64;

/// The most bytes one session's attributes hold in total, counting keys and values.
/// A chosen emulator bound, not an AWS limit.
pub const MAX_SESSION_ATTRIBUTES_BYTES: usize = 12 * 1024;

/// Whether `attributes` fits the bounds this store declares for one session's
/// `sessionAttributes`, or why it does not — for a handler to answer with the modeled
/// `LimitExceededException`.
pub fn check_session_attributes(attributes: &HashMap<String, String>) -> Result<(), String> {
    if attributes.len() > MAX_SESSION_ATTRIBUTES {
        return Err(format!(
            "a session holds at most {MAX_SESSION_ATTRIBUTES} session attributes in this \
             emulator; the request would leave it with {}",
            attributes.len()
        ));
    }
    let bytes: usize = attributes
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum();
    if bytes > MAX_SESSION_ATTRIBUTES_BYTES {
        return Err(format!(
            "a session's session attributes hold at most {MAX_SESSION_ATTRIBUTES_BYTES} bytes \
             in this emulator; the request would leave it with {bytes}"
        ));
    }
    Ok(())
}

/// The identity of one conversation: Amazon Lex keys session state by bot name, bot alias
/// and user id, so the same user talking to two aliases has two sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub bot_name: String,
    pub bot_alias: String,
    pub user_id: String,
}

impl SessionKey {
    pub fn new(bot_name: &str, bot_alias: &str, user_id: &str) -> Self {
        Self {
            bot_name: bot_name.to_string(),
            bot_alias: bot_alias.to_string(),
            user_id: user_id.to_string(),
        }
    }
}

/// `DialogActionType`: the next action the bot should take, as the client sets it on
/// `PutSession`'s `dialogAction` and as `IntentSummary` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialogActionType {
    Close,
    ConfirmIntent,
    Delegate,
    #[default]
    ElicitIntent,
    ElicitSlot,
}

impl DialogActionType {
    /// The wire value, exactly as the model's enum declares it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::ConfirmIntent => "ConfirmIntent",
            Self::Delegate => "Delegate",
            Self::ElicitIntent => "ElicitIntent",
            Self::ElicitSlot => "ElicitSlot",
        }
    }

    /// Parse a wire value. `None` for anything the model does not declare — which the
    /// framework's enum-constraint validation has already refused before a handler runs,
    /// so a handler reaching `None` treats it as the default action.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Close" => Some(Self::Close),
            "ConfirmIntent" => Some(Self::ConfirmIntent),
            "Delegate" => Some(Self::Delegate),
            "ElicitIntent" => Some(Self::ElicitIntent),
            "ElicitSlot" => Some(Self::ElicitSlot),
            _ => None,
        }
    }
}

/// `FulfillmentState`: how the intent's fulfillment ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FulfillmentState {
    Failed,
    Fulfilled,
    ReadyForFulfillment,
}

impl FulfillmentState {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Fulfilled => "Fulfilled",
            Self::ReadyForFulfillment => "ReadyForFulfillment",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Failed" => Some(Self::Failed),
            "Fulfilled" => Some(Self::Fulfilled),
            "ReadyForFulfillment" => Some(Self::ReadyForFulfillment),
            _ => None,
        }
    }
}

/// `DialogState`: what the runtime reports the conversation is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialogState {
    ConfirmIntent,
    #[default]
    ElicitIntent,
    ElicitSlot,
    Failed,
    Fulfilled,
    ReadyForFulfillment,
}

impl DialogState {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::ConfirmIntent => "ConfirmIntent",
            Self::ElicitIntent => "ElicitIntent",
            Self::ElicitSlot => "ElicitSlot",
            Self::Failed => "Failed",
            Self::Fulfilled => "Fulfilled",
            Self::ReadyForFulfillment => "ReadyForFulfillment",
        }
    }
}

/// The dialog a session is in, as the client last set it.
#[derive(Debug, Clone, Default)]
pub struct DialogStateRecord {
    /// The state reported as `dialogState`, derived by [`derive_dialog_state`].
    pub dialog_state: DialogState,
    /// The action type the client set, reported back as `dialogAction.type`.
    pub dialog_action_type: DialogActionType,
    pub intent_name: Option<String>,
    pub slots: HashMap<String, String>,
    pub slot_to_elicit: Option<String>,
    pub fulfillment_state: Option<FulfillmentState>,
    pub message: Option<String>,
    pub message_format: Option<String>,
}

/// The `dialogState` a `dialogAction` puts the conversation in.
///
/// `ElicitIntent`, `ElicitSlot` and `ConfirmIntent` name the state directly. `Close`
/// "indicates that there will not be a response from the user", so the conversation ends
/// in the action's `fulfillmentState` — whose three values (`Failed`, `Fulfilled`,
/// `ReadyForFulfillment`) are the three terminal `DialogState` values. `Delegate` leaves
/// "the next action determined by Amazon Lex", which needs the bot's dialog definition
/// this emulator does not host, so the conversation stays in the state it was already in.
pub fn derive_dialog_state(
    action: DialogActionType,
    fulfillment_state: Option<FulfillmentState>,
    previous: DialogState,
) -> DialogState {
    match action {
        DialogActionType::ElicitIntent => DialogState::ElicitIntent,
        DialogActionType::ElicitSlot => DialogState::ElicitSlot,
        DialogActionType::ConfirmIntent => DialogState::ConfirmIntent,
        DialogActionType::Close => match fulfillment_state {
            Some(FulfillmentState::Failed) => DialogState::Failed,
            Some(FulfillmentState::ReadyForFulfillment) => DialogState::ReadyForFulfillment,
            // `Close` is refused without a `fulfillmentState`, so `None` cannot reach
            // here from a request; `Fulfilled` is the state a closed intent is in.
            Some(FulfillmentState::Fulfilled) | None => DialogState::Fulfilled,
        },
        DialogActionType::Delegate => previous,
    }
}

/// One entry of `recentIntentSummaryView`.
#[derive(Debug, Clone)]
pub struct IntentSummaryRecord {
    pub intent_name: Option<String>,
    pub checkpoint_label: Option<String>,
    pub slots: HashMap<String, String>,
    pub confirmation_status: Option<String>,
    pub dialog_action_type: DialogActionType,
    pub fulfillment_state: Option<FulfillmentState>,
    pub slot_to_elicit: Option<String>,
}

/// One active context, plus the bookkeeping its documented expiry needs.
///
/// `timeToLiveInSeconds` is "the number of seconds that the context should be active after
/// it is first sent" and `turnsToLive` "the number of conversation turns that the context
/// should be active. A conversation turn is one PostContent or PostText request and the
/// corresponding response" — so both values are kept exactly as the caller set them and
/// reported back unchanged, while `set_at` and `turns_remaining` track when the context
/// falls due for removal.
#[derive(Debug, Clone)]
pub struct ActiveContextRecord {
    pub name: String,
    pub time_to_live_in_seconds: Option<i32>,
    pub turns_to_live: Option<i32>,
    pub parameters: HashMap<String, String>,
    /// When this context was last set by a request.
    pub set_at: SystemTime,
    /// Turns the context still has to live; `None` when the caller set no `turnsToLive`.
    pub turns_remaining: Option<i32>,
}

impl ActiveContextRecord {
    /// Whether the context is still active at `now`: a `turnsToLive` budget that has run
    /// out, or a `timeToLiveInSeconds` that has elapsed, ends it.
    pub fn is_live(&self, now: SystemTime) -> bool {
        if self.turns_remaining.is_some_and(|left| left <= 0) {
            return false;
        }
        match self.time_to_live_in_seconds {
            Some(seconds) if seconds >= 0 => {
                let deadline = self.set_at + Duration::from_secs(seconds.unsigned_abs().into());
                now < deadline
            }
            _ => true,
        }
    }
}

/// One conversation turn: a `PostText`/`PostContent` request and its response.
#[derive(Debug, Clone)]
pub struct Turn {
    pub input_text: String,
    pub intent_name: Option<String>,
    pub dialog_state: DialogState,
    pub at: SystemTime,
}

/// The persisted conversation for one [`SessionKey`].
#[derive(Debug, Clone)]
pub struct LexSession {
    /// Minted once, when the session is created, and stable for the session's life: the id
    /// `PutSession` returns is the id the following `PostText` and `GetSession` return.
    pub session_id: String,
    pub bot_name: String,
    pub bot_alias: String,
    pub user_id: String,
    pub bot_version: String,
    pub session_attributes: HashMap<String, String>,
    /// Per-turn, not carried across turns: `requestAttributes` are documented as
    /// request-scoped, so each turn replaces them.
    pub request_attributes: HashMap<String, String>,
    pub dialog: DialogStateRecord,
    /// Newest first, capped at [`MAX_RECENT_INTENTS`].
    pub recent_intents: Vec<IntentSummaryRecord>,
    /// Capped at [`MAX_ACTIVE_CONTEXTS`].
    pub active_contexts: Vec<ActiveContextRecord>,
    pub turns: Vec<Turn>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
}

impl LexSession {
    /// A brand-new conversation. A session with no `dialogAction` set has no recognised
    /// intent, so it starts in `ElicitIntent` — the state in which "Amazon Lex wants to
    /// elicit the user's intent".
    pub fn new(session_id: String, key: &SessionKey, now: SystemTime) -> Self {
        Self {
            session_id,
            bot_name: key.bot_name.clone(),
            bot_alias: key.bot_alias.clone(),
            user_id: key.user_id.clone(),
            bot_version: DEFAULT_BOT_VERSION.to_string(),
            session_attributes: HashMap::new(),
            request_attributes: HashMap::new(),
            dialog: DialogStateRecord::default(),
            recent_intents: Vec::new(),
            active_contexts: Vec::new(),
            turns: Vec::new(),
            created_at: now,
            last_updated_at: now,
        }
    }

    /// Drop every context the clock has already ended. This materialises an expiry that
    /// was due at `now` whether or not anybody called, so a reporting operation may run it.
    pub fn expire_contexts(&mut self, now: SystemTime) {
        self.active_contexts.retain(|context| context.is_live(now));
    }

    /// Replace the active contexts, honouring the documented rule for the request member:
    /// "If you don't specify a list of contexts, Amazon Lex will use the current list of
    /// contexts for the session. If you specify an empty list, all contexts for the
    /// session are cleared."
    pub fn set_active_contexts(&mut self, contexts: Option<Vec<ActiveContextRecord>>) {
        if let Some(mut contexts) = contexts {
            contexts.truncate(MAX_ACTIVE_CONTEXTS);
            self.active_contexts = contexts;
        }
    }

    /// End the turn: spend one unit of every context's `turnsToLive` budget, then drop the
    /// contexts whose budget or clock has run out.
    pub fn consume_turn(&mut self, now: SystemTime) {
        for context in &mut self.active_contexts {
            if let Some(left) = context.turns_remaining.as_mut() {
                *left -= 1;
            }
        }
        self.expire_contexts(now);
    }

    /// Record the turn's utterance against the dialog it was answered in, keeping at most
    /// [`MAX_TURNS_PER_SESSION`] turns: the oldest is dropped to make room.
    pub fn record_turn(&mut self, input_text: &str, now: SystemTime) {
        while self.turns.len() >= MAX_TURNS_PER_SESSION {
            self.turns.remove(0);
        }
        self.turns.push(Turn {
            input_text: input_text.to_string(),
            intent_name: self.dialog.intent_name.clone(),
            dialog_state: self.dialog.dialog_state,
            at: now,
        });
    }

    /// Move the summary of the dialog's current intent to the front of the recent-intent
    /// view, creating it when the view does not carry it yet and keeping the
    /// `checkpointLabel` an earlier `PutSession` attached to that intent.
    pub fn touch_current_intent_summary(&mut self) {
        let Some(intent_name) = self.dialog.intent_name.clone() else {
            return;
        };
        let existing = self
            .recent_intents
            .iter()
            .position(|summary| summary.intent_name.as_deref() == Some(intent_name.as_str()))
            .map(|at| self.recent_intents.remove(at));
        self.recent_intents.insert(
            0,
            IntentSummaryRecord {
                intent_name: Some(intent_name),
                checkpoint_label: existing.as_ref().and_then(|s| s.checkpoint_label.clone()),
                slots: self.dialog.slots.clone(),
                confirmation_status: existing.and_then(|s| s.confirmation_status),
                dialog_action_type: self.dialog.dialog_action_type,
                fulfillment_state: self.dialog.fulfillment_state,
                slot_to_elicit: self.dialog.slot_to_elicit.clone(),
            },
        );
        self.recent_intents.truncate(MAX_RECENT_INTENTS);
    }
}

/// Per-service state for com-amazonaws-lexruntimeservice-awsdeepsenseruntimeservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLexruntimeserviceAwsdeepsenseruntimeserviceState {
    /// Every live conversation, keyed by bot name + alias + user id. At most
    /// [`MAX_SESSIONS`] entries; within one session `recent_intents` is bounded by
    /// [`MAX_RECENT_INTENTS`], `active_contexts` by [`MAX_ACTIVE_CONTEXTS`], `turns` by
    /// [`MAX_TURNS_PER_SESSION`] and `session_attributes` by [`MAX_SESSION_ATTRIBUTES`] /
    /// [`MAX_SESSION_ATTRIBUTES_BYTES`], so no caller-driven field grows without bound.
    pub sessions: HashMap<SessionKey, LexSession>,
    /// How many session ids this store has minted; folded into the id so two sessions
    /// created in the same clock tick still get different ids.
    session_seq: u64,
}

impl ComAmazonawsLexruntimeserviceAwsdeepsenseruntimeserviceState {
    /// Mint a session id: the creation instant, as Amazon Lex's own ids carry it, plus a
    /// suffix derived from that instant and this store's mint counter, so the id is unique
    /// and is not a guessable function of the request.
    pub fn mint_session_id(&mut self, now: SystemTime) -> String {
        self.session_seq += 1;
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut hasher = Sha256::new();
        hasher.update(nanos.to_be_bytes());
        hasher.update(self.session_seq.to_be_bytes());
        let suffix = hex::encode(hasher.finalize());
        let stamp = aws_smithy_types::DateTime::from(now)
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        format!("{stamp}-{}", &suffix[..16])
    }

    /// The session for `key`, with every context the clock has already ended removed.
    pub fn session(&mut self, key: &SessionKey, now: SystemTime) -> Option<&mut LexSession> {
        let session = self.sessions.get_mut(key)?;
        session.expire_contexts(now);
        Some(session)
    }

    /// The session for `key`, started if this store does not hold it yet. Call it only
    /// once a request has been fully validated: creating a session is a mutation, so a
    /// request that is going to be refused must never reach here.
    pub fn session_or_create(
        &mut self,
        key: &SessionKey,
        now: SystemTime,
    ) -> Option<&mut LexSession> {
        if !self.sessions.contains_key(key) {
            self.make_room_for_session();
            let session_id = self.mint_session_id(now);
            self.sessions
                .insert(key.clone(), LexSession::new(session_id, key, now));
        }
        self.session(key, now)
    }

    /// Make room for one more conversation: while the store is at [`MAX_SESSIONS`], give
    /// up the least recently updated session.
    fn make_room_for_session(&mut self) {
        while self.sessions.len() >= MAX_SESSIONS {
            let stalest = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.last_updated_at)
                .map(|(key, _)| key.clone());
            match stalest {
                Some(key) => {
                    self.sessions.remove(&key);
                }
                None => break,
            }
        }
    }
}

/// Conversions between the persisted records above and the modeled wire shapes, plus the
/// JSON renderings `PutSession`'s `SynthesizedJsonString` headers carry.
///
/// They live beside the records rather than in any one operation's file because all four
/// implemented operations read the same session: `GetSession` rebuilds a `dialogAction`
/// from what `PutSession` stored, and both of them plus `PostText` report the same
/// contexts.
pub mod wire {
    use super::{
        ActiveContextRecord, DialogActionType, DialogStateRecord, FulfillmentState,
        IntentSummaryRecord, MAX_ACTIVE_CONTEXTS,
    };
    use crate::ops::types::{ActiveContext, ActiveContextTimeToLive, DialogAction, IntentSummary};
    use std::collections::HashMap;
    use std::time::SystemTime;

    /// Build the stored form of the `activeContexts` a request carried. `None` in means
    /// "leave the session's contexts alone"; an empty list in means "clear them".
    pub fn active_contexts_from_wire(
        contexts: Option<Vec<ActiveContext>>,
        now: SystemTime,
    ) -> Option<Vec<ActiveContextRecord>> {
        let contexts = contexts?;
        let mut records: Vec<ActiveContextRecord> = contexts
            .into_iter()
            .map(|context| ActiveContextRecord {
                name: context.name,
                time_to_live_in_seconds: context.time_to_live.time_to_live_in_seconds,
                turns_to_live: context.time_to_live.turns_to_live,
                parameters: context.parameters,
                set_at: now,
                turns_remaining: context.time_to_live.turns_to_live,
            })
            .collect();
        records.truncate(MAX_ACTIVE_CONTEXTS);
        Some(records)
    }

    /// The modeled `activeContexts` list for a response, reporting each context's
    /// configured `timeToLive` unchanged.
    pub fn active_contexts_to_wire(records: &[ActiveContextRecord]) -> Vec<ActiveContext> {
        records
            .iter()
            .map(|record| ActiveContext {
                name: record.name.clone(),
                time_to_live: ActiveContextTimeToLive {
                    time_to_live_in_seconds: record.time_to_live_in_seconds,
                    turns_to_live: record.turns_to_live,
                },
                parameters: record.parameters.clone(),
            })
            .collect()
    }

    /// The stored form of a `recentIntentSummaryView`.
    pub fn intent_summaries_from_wire(summaries: Vec<IntentSummary>) -> Vec<IntentSummaryRecord> {
        summaries
            .into_iter()
            .map(|summary| IntentSummaryRecord {
                intent_name: summary.intent_name,
                checkpoint_label: summary.checkpoint_label,
                slots: summary.slots.unwrap_or_default(),
                confirmation_status: summary.confirmation_status,
                dialog_action_type: DialogActionType::from_wire(&summary.dialog_action_type)
                    .unwrap_or_default(),
                fulfillment_state: summary
                    .fulfillment_state
                    .as_deref()
                    .and_then(FulfillmentState::from_wire),
                slot_to_elicit: summary.slot_to_elicit,
            })
            .collect()
    }

    /// The modeled `recentIntentSummaryView` for a response.
    pub fn intent_summaries_to_wire(records: &[IntentSummaryRecord]) -> Vec<IntentSummary> {
        records
            .iter()
            .map(|record| IntentSummary {
                intent_name: record.intent_name.clone(),
                checkpoint_label: record.checkpoint_label.clone(),
                slots: Some(record.slots.clone()),
                confirmation_status: record.confirmation_status.clone(),
                dialog_action_type: record.dialog_action_type.as_wire().to_string(),
                fulfillment_state: record.fulfillment_state.map(|s| s.as_wire().to_string()),
                slot_to_elicit: record.slot_to_elicit.clone(),
            })
            .collect()
    }

    /// Rebuild the `dialogAction` a `GetSession` reports from the dialog `PutSession`
    /// stored.
    pub fn dialog_action_to_wire(dialog: &DialogStateRecord) -> DialogAction {
        DialogAction {
            r#type: dialog.dialog_action_type.as_wire().to_string(),
            intent_name: dialog.intent_name.clone(),
            slots: Some(dialog.slots.clone()),
            slot_to_elicit: dialog.slot_to_elicit.clone(),
            fulfillment_state: dialog.fulfillment_state.map(|s| s.as_wire().to_string()),
            message: dialog.message.clone(),
            message_format: dialog.message_format.clone(),
        }
    }

    /// A `SynthesizedJsonString` header value for a string map: the map as a JSON object.
    pub fn json_string_map(map: &HashMap<String, String>) -> String {
        serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string())
    }

    /// A `SynthesizedJsonActiveContextsString` header value: the contexts as a JSON array
    /// of the modeled `ActiveContext` shape.
    pub fn json_active_contexts(records: &[ActiveContextRecord]) -> String {
        let contexts: Vec<serde_json::Value> = records
            .iter()
            .map(|record| {
                let mut ttl = serde_json::Map::new();
                if let Some(seconds) = record.time_to_live_in_seconds {
                    ttl.insert("timeToLiveInSeconds".to_string(), seconds.into());
                }
                if let Some(turns) = record.turns_to_live {
                    ttl.insert("turnsToLive".to_string(), turns.into());
                }
                serde_json::json!({
                    "name": record.name,
                    "timeToLive": serde_json::Value::Object(ttl),
                    "parameters": record.parameters,
                })
            })
            .collect();
        serde_json::to_string(&contexts).unwrap_or_else(|_| "[]".to_string())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLexruntimeserviceAwsdeepsenseruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLexruntimeserviceAwsdeepsenseruntimeserviceState>)
}
