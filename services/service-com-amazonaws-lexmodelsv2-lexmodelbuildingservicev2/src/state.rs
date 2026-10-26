// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2 service.
//!
//! The bot lifecycle slice (`CreateBot`, `DescribeBot`, `ListBots`, `UpdateBot`,
//! `DeleteBot`) is served entirely out of the `bots` map below: every response those
//! handlers build is read back from a record stored here, and the ids they hand out are
//! minted here and kept.
//!
//! The locale slice (`CreateBotLocale`, `DescribeBotLocale`) is served the same way out
//! of `bot_locales`, keyed by the bot, bot version and locale the request names.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AudioFillerSettings, BotMember, BotSummary, DataPrivacy, ErrorLogSettings,
    GenerativeAISettings, SpeakerDiarizationSettings, SpeechRecognitionSettings,
    UnifiedSpeechSettings, VoiceSettings,
};

/// `BotStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotStatus")`:
/// Creating | Available | Inactive | Deleting | Failed | Versioning | Importing | Updating.
pub const BOT_STATUS_CREATING: &str = "Creating";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_AVAILABLE: &str = "Available";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_DELETING: &str = "Deleting";

/// `BotLocaleStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotLocaleStatus")`:
/// Creating | Building | Built | ReadyExpressTesting | Failed | Deleting | NotBuilt |
/// Importing | Processing.
pub const BOT_LOCALE_STATUS_CREATING: &str = "Creating";
/// See [`BOT_LOCALE_STATUS_CREATING`]. AWS documents this as the status of a locale
/// that "was saved but not built", which is where a freshly created locale lands.
pub const BOT_LOCALE_STATUS_NOT_BUILT: &str = "NotBuilt";

/// Every language and locale Amazon Lex V2 accepts, each with the locale name the API
/// reports for it.
///
/// The modelled `LocaleId` is a plain string
/// (`smithy_query(query="shape",shape="LocaleId")`) whose doc says only "The string
/// must match one of the supported locales", so the accepted set and both columns here
/// come from the published Amazon Lex V2 supported languages and locales table — the
/// same source that table's `localeName` column has.
pub const SUPPORTED_LOCALES: [(&str, &str); 27] = [
    ("ar_AE", "Arabic (Gulf)"),
    ("ca_ES", "Catalan (Spain)"),
    ("de_AT", "German (Austria)"),
    ("de_DE", "German (Germany)"),
    ("en_AU", "English (Australia)"),
    ("en_GB", "English (UK)"),
    ("en_IN", "English (India)"),
    ("en_NZ", "English (New Zealand)"),
    ("en_US", "English (US)"),
    ("en_ZA", "English (South Africa)"),
    ("es_419", "Spanish (Latin America)"),
    ("es_ES", "Spanish (Spain)"),
    ("es_US", "Spanish (US)"),
    ("fi_FI", "Finnish (Finland)"),
    ("fr_CA", "French (Canada)"),
    ("fr_FR", "French (France)"),
    ("hi_IN", "Hindi (India)"),
    ("it_IT", "Italian (Italy)"),
    ("ja_JP", "Japanese (Japan)"),
    ("ko_KR", "Korean (Korea)"),
    ("nb_NO", "Norwegian (Norway)"),
    ("nl_NL", "Dutch (Netherlands)"),
    ("pl_PL", "Polish (Poland)"),
    ("pt_BR", "Portuguese (Brazil)"),
    ("pt_PT", "Portuguese (Portugal)"),
    ("sv_SE", "Swedish (Sweden)"),
    ("zh_CN", "Chinese (Simplified)"),
];

/// The locale name Amazon Lex reports for `locale_id`, or `None` when the service
/// supports no such locale.
pub fn locale_name(locale_id: &str) -> Option<&'static str> {
    SUPPORTED_LOCALES
        .iter()
        .find(|(id, _)| *id == locale_id)
        .map(|(_, name)| *name)
}

/// Every supported locale id, in the order [`SUPPORTED_LOCALES`] lists them: what a
/// refusal tells a caller it may send instead.
pub fn supported_locale_ids() -> Vec<&'static str> {
    SUPPORTED_LOCALES.iter().map(|(id, _)| *id).collect()
}

/// Every member of the modelled `BotType` enum
/// (`smithy_query(query="shape",shape="BotType")`).
pub const BOT_TYPES: [&str; 2] = ["Bot", "BotNetwork"];

/// Every member of the modelled `BotSortAttribute` enum
/// (`smithy_query(query="shape",shape="BotSortAttribute")`).
pub const BOT_SORT_ATTRIBUTES: [&str; 1] = ["BotName"];

/// Every member of the modelled `SortOrder` enum
/// (`smithy_query(query="shape",shape="SortOrder")`).
pub const SORT_ORDERS: [&str; 2] = ["Ascending", "Descending"];

/// Every member of the modelled `BotFilterName` enum
/// (`smithy_query(query="shape",shape="BotFilterName")`).
pub const BOT_FILTER_NAMES: [&str; 2] = ["BotName", "BotType"];

/// Every member of the modelled `BotFilterOperator` enum
/// (`smithy_query(query="shape",shape="BotFilterOperator")`).
pub const BOT_FILTER_OPERATORS: [&str; 3] = ["CO", "EQ", "NE"];

/// How long after a create the `Creating` -> settled transition falls due: `Available`
/// for a bot, `NotBuilt` for a bot locale.
///
/// AWS documents the transition but not its duration ("The bot is first in the
/// `Creating` status. Once the bot is ready for use, it changes to the `Available`
/// status"), so the span is CHOSEN: non-zero, so a caller that reads the bot straight
/// back still observes `Creating`, and short enough that a client script moves on
/// quickly. Nothing sleeps for it — the due instant is stored and the status is
/// materialised by the next read, which is what the clock already owed.
pub const CREATION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// How many bots one account may hold: the published AWS Lex V2 service quota "Bots
/// per account per Region" (default 100, adjustable on request).
///
/// `CreateBot` answers the modelled `ServiceQuotaExceededException` ("You have reached
/// a quota for your bot", HTTP 402) once the store holds this many, which is also what
/// bounds how large this service's state — and therefore any listing built from it —
/// can grow.
pub const BOTS_PER_ACCOUNT_QUOTA: usize = 100;

/// The largest page `ListBots` returns, which is the modelled `MaxResults` maximum
/// (`smithy_query(query="operation",operation="ListBots")`: range 1..1000). It also
/// caps a listing that asks for no page size, so no single response is unbounded; the
/// remainder stays reachable through `nextToken`.
pub const MAX_LIST_PAGE: usize = 1000;

/// A bot, as `CreateBot` stored it and `DescribeBot` / `ListBots` / `UpdateBot` read it
/// back.
#[derive(Debug, Clone)]
pub struct Bot {
    pub bot_id: String,
    pub bot_name: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub data_privacy: DataPrivacy,
    pub idle_session_ttl_in_seconds: i32,
    pub bot_status: String,
    pub bot_type: Option<String>,
    pub bot_members: Vec<BotMember>,
    pub error_log_settings: Option<ErrorLogSettings>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    pub bot_tags: HashMap<String, String>,
    pub test_bot_alias_tags: HashMap<String, String>,
    /// The status the clock owes this bot, and the instant it falls due. `None` once
    /// the bot has reached a status nothing further is owed from. Set it through
    /// [`Bot::schedule_status`] rather than by hand.
    pub pending_status: Option<(String, Instant)>,
}

impl Bot {
    /// Apply the status transition the clock has already made due, if any.
    ///
    /// A read calls this before answering: the change was owed at that instant whether
    /// or not anybody called, so materialising it is not a mutation a reader invented.
    pub fn settle(&mut self) {
        settle_status(&mut self.bot_status, &mut self.pending_status);
    }

    /// Schedule `status` to become this bot's status once `after` has elapsed.
    pub fn schedule_status(&mut self, status: &str, after: Duration) {
        self.pending_status = Some((status.to_string(), Instant::now() + after));
    }

    /// This bot as a `ListBots` summary entry.
    ///
    /// `latestBotVersion` stays absent until a numbered version exists: a freshly
    /// created bot has only the `DRAFT` version, which is not a numerical version.
    pub fn summary(&self) -> BotSummary {
        BotSummary {
            bot_id: Some(self.bot_id.clone()),
            bot_name: Some(self.bot_name.clone()),
            description: self.description.clone(),
            bot_status: Some(self.bot_status.clone()),
            latest_bot_version: None,
            last_updated_date_time: Some(to_date_time(self.last_updated_date_time)),
            bot_type: self.bot_type.clone(),
        }
    }

    /// The stored `botMembers`, as the wire shape renders them: absent rather than an
    /// empty list when the bot is in no network.
    pub fn wire_bot_members(&self) -> Option<Vec<BotMember>> {
        (!self.bot_members.is_empty()).then(|| self.bot_members.clone())
    }

    /// The stored `botTags`, absent rather than empty when the bot carries none.
    pub fn wire_bot_tags(&self) -> Option<HashMap<String, String>> {
        (!self.bot_tags.is_empty()).then(|| self.bot_tags.clone())
    }

    /// The stored `testBotAliasTags`, absent rather than empty when there are none.
    pub fn wire_test_bot_alias_tags(&self) -> Option<HashMap<String, String>> {
        (!self.test_bot_alias_tags.is_empty()).then(|| self.test_bot_alias_tags.clone())
    }
}

/// Move `status` on to the transition the clock has already made due, if any.
///
/// Shared by every record that carries a scheduled status, so one rule decides when a
/// pending transition is owed.
fn settle_status(status: &mut String, pending: &mut Option<(String, Instant)>) {
    let now = Instant::now();
    if let Some((next, due)) = pending.take() {
        if now >= due {
            *status = next;
        } else {
            *pending = Some((next, due));
        }
    }
}

/// A bot locale, as `CreateBotLocale` stored it and `DescribeBotLocale` reads it back.
#[derive(Debug, Clone)]
pub struct BotLocale {
    pub bot_id: String,
    pub bot_version: String,
    pub locale_id: String,
    pub locale_name: String,
    pub description: Option<String>,
    pub nlu_intent_confidence_threshold: f64,
    pub voice_settings: Option<VoiceSettings>,
    pub unified_speech_settings: Option<UnifiedSpeechSettings>,
    pub audio_filler_settings: Option<AudioFillerSettings>,
    pub speech_recognition_settings: Option<SpeechRecognitionSettings>,
    pub generative_ai_settings: Option<GenerativeAISettings>,
    pub speech_detection_sensitivity: Option<String>,
    pub speaker_diarization_settings: Option<SpeakerDiarizationSettings>,
    pub bot_locale_status: String,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// When the locale was last submitted for building; `None` until a build is asked
    /// for.
    pub last_build_submitted_date_time: Option<SystemTime>,
    /// The status the clock owes this locale, and the instant it falls due. Set it
    /// through [`BotLocale::schedule_status`] rather than by hand.
    pub pending_status: Option<(String, Instant)>,
}

impl BotLocale {
    /// Apply the status transition the clock has already made due, if any — the same
    /// contract as [`Bot::settle`].
    pub fn settle(&mut self) {
        settle_status(&mut self.bot_locale_status, &mut self.pending_status);
    }

    /// Schedule `status` to become this locale's status once `after` has elapsed.
    pub fn schedule_status(&mut self, status: &str, after: Duration) {
        self.pending_status = Some((status.to_string(), Instant::now() + after));
    }
}

/// How a bot locale is keyed: the bot, the bot version and the locale id, which is the
/// same triple every locale operation's URI carries.
pub type BotLocaleKey = (String, String, String);

/// The [`BotLocaleKey`] for one locale of one bot version.
pub fn bot_locale_key(bot_id: &str, bot_version: &str, locale_id: &str) -> BotLocaleKey {
    (
        bot_id.to_string(),
        bot_version.to_string(),
        locale_id.to_string(),
    )
}

/// Per-service state for com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Every bot this emulator holds, keyed by the `botId` it minted.
    pub bots: HashMap<String, Bot>,
    /// Every bot locale this emulator holds, keyed by [`bot_locale_key`]. A locale
    /// belongs to one version of one bot, so this map is bounded by the bots the store
    /// may hold ([`BOTS_PER_ACCOUNT_QUOTA`]) times the locale ids a bot version can
    /// carry ([`SUPPORTED_LOCALES`]): a caller cannot grow it past that.
    pub bot_locales: HashMap<BotLocaleKey, BotLocale>,
    /// The monotonic source the minted ids are derived from.
    bot_id_seq: u64,
}

impl ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Mint an unused `botId`: exactly 10 characters from `[0-9a-zA-Z]`, as the
    /// modelled `Id` shape requires (len 10..10, pattern `^[0-9a-zA-Z]+$`).
    ///
    /// The id is derived from a monotonic counter, so two calls never collide, and the
    /// result is checked against the stored bots before it is handed out.
    pub fn mint_bot_id(&mut self) -> String {
        loop {
            self.bot_id_seq += 1;
            let candidate = render_id(self.bot_id_seq);
            if !self.bots.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Is `name` already the name of a stored bot? `CreateBot` refuses a duplicate:
    /// "The bot name must be unique in the account that creates the bot."
    pub fn bot_name_taken(&self, name: &str) -> bool {
        self.bots.values().any(|bot| bot.bot_name == name)
    }

    /// The stored bot `bot_id` names, with any due status transition applied first.
    pub fn settled_bot(&mut self, bot_id: &str) -> Option<&Bot> {
        let bot = self.bots.get_mut(bot_id)?;
        bot.settle();
        Some(bot)
    }

    /// The stored bot `bot_id` names, mutably, with any due status transition applied.
    pub fn settled_bot_mut(&mut self, bot_id: &str) -> Option<&mut Bot> {
        let bot = self.bots.get_mut(bot_id)?;
        bot.settle();
        Some(bot)
    }

    /// Apply every due status transition, so a listing reports the same status a
    /// per-bot read would.
    pub fn settle_all(&mut self) {
        for bot in self.bots.values_mut() {
            bot.settle();
        }
    }

    /// Is this locale already defined for that bot version? `CreateBotLocale` refuses a
    /// second locale with the same id on the same bot version.
    pub fn bot_locale_exists(&self, bot_id: &str, bot_version: &str, locale_id: &str) -> bool {
        self.bot_locales
            .contains_key(&bot_locale_key(bot_id, bot_version, locale_id))
    }

    /// The stored locale the triple names, with any due status transition applied
    /// first.
    pub fn settled_bot_locale(
        &mut self,
        bot_id: &str,
        bot_version: &str,
        locale_id: &str,
    ) -> Option<&BotLocale> {
        let locale = self
            .bot_locales
            .get_mut(&bot_locale_key(bot_id, bot_version, locale_id))?;
        locale.settle();
        Some(locale)
    }

    /// Drop every locale of `bot_id`, whatever version holds it, and answer how many
    /// went: a locale cannot outlive the bot it is defined in.
    pub fn remove_bot_locales(&mut self, bot_id: &str) -> usize {
        let before = self.bot_locales.len();
        self.bot_locales.retain(|(bot, _, _), _| bot != bot_id);
        before - self.bot_locales.len()
    }
}

/// A stored instant as the wire timestamp the modelled `Timestamp` members carry.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The opaque `nextToken` that resumes a listing at the entry whose sort key is `at`.
///
/// Hex-encoded, so the token is opaque to the caller and carries no internal key in
/// clear text — the same codec `services/service-com-amazonaws-proton-awsproton20200720`
/// uses for its listings.
pub fn encode_page_token(at: &str) -> String {
    hex::encode(at)
}

/// The sort key an opaque `nextToken` resumes at, or `None` when the token was not
/// minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Render `seq` into the `[0-9a-zA-Z]{10}` space the modelled `Id` shape allows.
///
/// The counter is mixed first (SplitMix64's finalizer) so consecutive ids do not read
/// as consecutive numbers, the way a real Lex bot id does not; the mix is a bijection
/// on `u64`, and the result is reduced into `62^10` values, so distinct counters can
/// only collide after that reduction — which [`mint_bot_id`]'s re-draw covers.
fn render_id(seq: u64) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    const LEN: usize = 10;

    let mut z = seq.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;

    let space = (ALPHABET.len() as u64).pow(LEN as u32);
    let mut value = z % space;
    let mut digits = [b'0'; LEN];
    for slot in digits.iter_mut().rev() {
        *slot = ALPHABET[(value % ALPHABET.len() as u64) as usize];
        value /= ALPHABET.len() as u64;
    }
    String::from_utf8(digits.to_vec()).expect("the id alphabet is ASCII")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State>)
}
