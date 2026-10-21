// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2 service.
//!
//! The bot lifecycle slice (`CreateBot`, `DescribeBot`, `ListBots`, `UpdateBot`,
//! `DeleteBot`) is served entirely out of the `bots` map below: every response those
//! handlers build is read back from a record stored here, and the ids they hand out are
//! minted here and kept.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{BotMember, BotSummary, DataPrivacy, ErrorLogSettings};

/// `BotStatus` members this crate names. The full modelled set is
/// `smithy_query(query="shape",shape="BotStatus")`:
/// Creating | Available | Inactive | Deleting | Failed | Versioning | Importing | Updating.
pub const BOT_STATUS_CREATING: &str = "Creating";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_AVAILABLE: &str = "Available";
/// See [`BOT_STATUS_CREATING`].
pub const BOT_STATUS_DELETING: &str = "Deleting";

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

/// How long after `CreateBot` the `Creating` -> `Available` transition falls due.
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
        let now = Instant::now();
        if let Some((next, due)) = self.pending_status.take() {
            if now >= due {
                self.bot_status = next;
            } else {
                self.pending_status = Some((next, due));
            }
        }
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

/// Per-service state for com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State {
    /// Every bot this emulator holds, keyed by the `botId` it minted.
    pub bots: HashMap<String, Bot>,
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
