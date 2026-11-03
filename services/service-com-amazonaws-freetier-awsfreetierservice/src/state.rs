// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-freetier-awsfreetierservice service.
//!
//! Free Tier has no "create" API: everything it reports is account-scoped data
//! the account already has, so this store holds three collections keyed by
//! account id and every handler reads (or, for `UpgradeAccountPlan`, mutates)
//! them:
//!
//! * `plans` — one [`AccountPlan`] singleton per account, reported by
//!   `GetAccountPlanState` and written by `UpgradeAccountPlan`. It starts EMPTY:
//!   an account has account-plan data only once the service's one plan-writing
//!   operation has established it, so `GetAccountPlanState` has a not-found
//!   answer to give (see
//!   [`ComAmazonawsFreetierAwsfreetierserviceState::settled_plan`]).
//! * `activities` — the activity catalog, insertion-ordered so paging it is
//!   stable. `ListAccountActivities` pages over it and `GetAccountActivity`
//!   looks ids up in the SAME rows, so the summary and the detail view of one
//!   activity always agree.
//! * `usages` — the Free Tier usage records `GetFreeTierUsage` pages and
//!   filters, also insertion-ordered.
//!
//! `activities` and `usages` are part of the store's initial value
//! ([`Default`]), so a reporting operation serves a read without writing
//! anything.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{ActivityReward, MonetaryAmount, LANGUAGE_CODE_VALUES};

/// `AccountPlanType` values, from the model's enum of the same name
/// (`shared_types::ACCOUNT_PLAN_TYPE_VALUES`).
pub const PLAN_TYPE_FREE: &str = "FREE";
/// See [`PLAN_TYPE_FREE`].
pub const PLAN_TYPE_PAID: &str = "PAID";

/// `AccountPlanStatus` values, from the model's enum of the same name
/// (`shared_types::ACCOUNT_PLAN_STATUS_VALUES`).
pub const PLAN_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`PLAN_STATUS_ACTIVE`].
pub const PLAN_STATUS_EXPIRED: &str = "EXPIRED";

/// `ActivityStatus` values, from the model's enum of the same name
/// (`shared_types::ACTIVITY_STATUS_VALUES`).
pub const ACTIVITY_STATUS_NOT_STARTED: &str = "NOT_STARTED";
/// See [`ACTIVITY_STATUS_NOT_STARTED`].
pub const ACTIVITY_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`ACTIVITY_STATUS_NOT_STARTED`].
pub const ACTIVITY_STATUS_COMPLETED: &str = "COMPLETED";
/// See [`ACTIVITY_STATUS_NOT_STARTED`].
pub const ACTIVITY_STATUS_EXPIRING: &str = "EXPIRING";

/// The only `CurrencyCode` the model declares
/// (`shared_types::CURRENCY_CODE_VALUES`).
pub const CURRENCY_USD: &str = "USD";

/// The page size the model declares for `maxResults` on both paginated
/// operations (`@default: 10`).
pub const DEFAULT_MAX_RESULTS: i32 = 10;

/// The language a request that names no `languageCode` is answered in, and the
/// language every catalog row holds its base text in.
///
/// Both activity readers model `languageCode` as OPTIONAL
/// (`ListAccountActivitiesRequest$languageCode`,
/// `GetAccountActivityRequest$languageCode`) and the model names no default, so
/// which language a request without one is answered in is this emulator's
/// choice; it is one of the modelled `LanguageCode` values
/// (`crate::ops::types::LANGUAGE_CODE_VALUES`).
pub const DEFAULT_LANGUAGE_CODE: &str = "en-US";

/// The primary language subtag of a `LanguageCode` — `en` of `en-GB` — which is
/// what makes a code with no stored translation resolve to another variant of
/// the same language rather than straight to the base text.
///
/// This crate's one statement of when two `LanguageCode` values name the SAME
/// language: [`LocalizedText::in_language`] resolves a requested code with it,
/// so both activity readers group codes by it, and the suites that pin that
/// grouping call it here instead of restating it.
pub fn primary_subtag(language_code: &str) -> &str {
    language_code
        .split_once('-')
        .map_or(language_code, |(primary, _)| primary)
}

/// A piece of catalog text the service returns translated: the base text in
/// [`DEFAULT_LANGUAGE_CODE`] plus the translations the catalog holds for other
/// `LanguageCode` values.
///
/// `languageCode` on the activity readers is documented as "the language code
/// used to return translated titles" / "translated title and description
/// fields", so the text a reader reports is a function of the row AND the
/// requested code. Storing the translations beside the row — rather than
/// deriving text from the request — is what makes both readers answer one
/// code with the same text, every time.
#[derive(Debug, Clone)]
pub struct LocalizedText {
    /// The text in [`DEFAULT_LANGUAGE_CODE`]; always present, so every request
    /// has an answer whatever it asks for.
    base: String,
    /// Language code -> the text in that language, ordered so the resolution
    /// below is deterministic.
    translations: BTreeMap<String, String>,
}

impl LocalizedText {
    /// Build the text from its base ([`DEFAULT_LANGUAGE_CODE`]) and the
    /// `(language code, text)` pairs the catalog translates it into.
    pub fn new(base: &str, translations: &[(&str, &str)]) -> Self {
        Self {
            base: base.to_string(),
            translations: translations
                .iter()
                .map(|(code, text)| ((*code).to_string(), (*text).to_string()))
                .collect(),
        }
    }

    /// The text in the language a request asked for.
    ///
    /// Resolution, nearest match first: the exact code; then any variant of the
    /// same language (`en-GB` is served by the `en-US` base, `zh-TW` by another
    /// `zh-*` entry when the catalog holds no Taiwanese text); then the base
    /// text. A request that names no code is answered in
    /// [`DEFAULT_LANGUAGE_CODE`].
    pub fn in_language(&self, language_code: Option<&str>) -> &str {
        let Some(code) = language_code else {
            return &self.base;
        };
        if let Some(text) = self.translations.get(code) {
            return text;
        }
        let wanted = primary_subtag(code);
        if wanted == primary_subtag(DEFAULT_LANGUAGE_CODE) {
            return &self.base;
        }
        self.translations
            .iter()
            .find(|(stored, _)| primary_subtag(stored) == wanted)
            .map_or(&self.base, |(_, text)| text.as_str())
    }
}

/// Seconds in a day, for the seeded timestamps below.
const DAY: i64 = 86_400;

/// How long after it is created the seeded free plan expires.
///
/// The Free Tier plan is a time-boxed window rather than a permanent state
/// ("the timestamp for when the current account plan expires",
/// `GetAccountPlanStateResponse$accountPlanExpirationDate`); the length of the
/// emulator's window is this crate's choice, not a value AWS publishes, and
/// nothing in the service's behaviour depends on its exact size — only on
/// whether the instant has passed.
const FREE_PLAN_WINDOW_DAYS: i64 = 180;

/// The credits the seeded free plan still holds, in [`CURRENCY_USD`]. A chosen
/// starting balance: no operation in this service consumes credits, so the
/// figure is reported, never recomputed.
const FREE_PLAN_CREDITS: f64 = 100.0;

/// The credit every seeded activity rewards, in [`CURRENCY_USD`]. Also chosen:
/// the catalog is this emulator's, because the emulator observes no real
/// account activity.
const ACTIVITY_CREDIT: f64 = 20.0;

/// An account's Free Tier plan: the singleton `GetAccountPlanState` reports and
/// `UpgradeAccountPlan` moves.
#[derive(Debug, Clone)]
pub struct AccountPlan {
    /// `^[0-9]{12}$` — the account the plan belongs to.
    pub account_id: String,
    /// `AccountPlanType`: `FREE` | `PAID`.
    pub plan_type: String,
    /// `AccountPlanStatus`: `ACTIVE` | `EXPIRED` | `NOT_STARTED`.
    pub plan_status: String,
    pub remaining_credits: Option<MonetaryAmount>,
    /// When a `FREE` plan expires. A `PAID` plan has no expiry, so this is
    /// `None` for one.
    pub expiration_date: Option<DateTime>,
}

impl AccountPlan {
    /// The plan an account is on before it has ever been upgraded: the free
    /// plan, active, with its credit balance and the instant it expires.
    pub fn free(account_id: &str, now: DateTime) -> Self {
        Self {
            account_id: account_id.to_string(),
            plan_type: PLAN_TYPE_FREE.to_string(),
            plan_status: PLAN_STATUS_ACTIVE.to_string(),
            remaining_credits: Some(MonetaryAmount {
                amount: FREE_PLAN_CREDITS,
                unit: CURRENCY_USD.to_string(),
            }),
            expiration_date: Some(DateTime::from_secs(
                now.secs() + FREE_PLAN_WINDOW_DAYS * DAY,
            )),
        }
    }

    /// Materialise the expiry the clock has already made due: a free plan whose
    /// `accountPlanExpirationDate` has passed is `EXPIRED`, whether or not
    /// anybody called. A paid plan does not expire.
    ///
    /// This is the only write a reporting operation performs, and it is owed at
    /// the same instant regardless of the call that observes it.
    pub fn settle(&mut self, now: DateTime) {
        if self.plan_type != PLAN_TYPE_FREE {
            return;
        }
        if self
            .expiration_date
            .is_some_and(|expires_at| expires_at.secs() <= now.secs())
        {
            self.plan_status = PLAN_STATUS_EXPIRED.to_string();
        }
    }

    /// Whether the account's free-plan window is still open — the plan it holds
    /// is the free plan and that plan's expiration instant has not passed yet.
    ///
    /// The free plan is a one-off, time-boxed window
    /// ("the timestamp for when the current account plan expires",
    /// `GetAccountPlanStateResponse$accountPlanExpirationDate`), so an account
    /// whose window is closed — because it left the free plan, or because the
    /// window elapsed — cannot be moved onto it again. Read the status after
    /// [`Self::settle`], which is what makes an elapsed window show here.
    pub fn free_window_open(&self) -> bool {
        self.plan_type == PLAN_TYPE_FREE && self.plan_status != PLAN_STATUS_EXPIRED
    }

    /// Move the plan onto `plan_type` as of `now`. The plan becomes `ACTIVE` on
    /// the type it now holds, and a paid plan drops the expiration date a free
    /// plan carries because it does not expire.
    ///
    /// The status this leaves stored is the status the record yields at `now`
    /// ([`Self::settle`]), so what the writing operation reports is what the
    /// next read of the same record reports: a stored plan whose expiry is
    /// already past is never left claiming to be `ACTIVE`.
    pub fn upgrade_to(&mut self, plan_type: &str, now: DateTime) {
        self.plan_type = plan_type.to_string();
        self.plan_status = PLAN_STATUS_ACTIVE.to_string();
        if plan_type == PLAN_TYPE_PAID {
            self.expiration_date = None;
        }
        self.settle(now);
    }
}

/// One activity in the account's catalog. It serves BOTH
/// `ListAccountActivities` (the summary members) and `GetAccountActivity` (all
/// of them), so one row is the single source of truth for an activity.
#[derive(Debug, Clone)]
pub struct Activity {
    /// `ActivityId`: exactly 32 characters of `[a-zA-Z0-9]`.
    pub activity_id: String,
    /// The short title, in every language the catalog holds it in: a reader
    /// reports the one its `languageCode` asks for.
    pub title: LocalizedText,
    /// The detailed description, likewise translated.
    pub description: LocalizedText,
    pub instructions_url: String,
    /// `ActivityStatus`: `COMPLETED` | `EXPIRING` | `IN_PROGRESS` |
    /// `NOT_STARTED`.
    pub status: String,
    /// The credit completing the activity rewards.
    pub reward: MonetaryAmount,
    pub estimated_time_to_complete_in_minutes: Option<i32>,
    pub expires_at: Option<DateTime>,
    pub started_at: Option<DateTime>,
    pub completed_at: Option<DateTime>,
}

impl Activity {
    /// The reward as the modelled `ActivityReward` union; `credit` is its only
    /// member.
    pub fn reward_union(&self) -> ActivityReward {
        ActivityReward::Credit(self.reward.clone())
    }
}

/// One Free Tier usage record: an offer's limit and what the account has used
/// of it.
///
/// `Default` gives the empty record [`dimension_kind`] asks which kind of value
/// a dimension takes; it is never stored.
#[derive(Debug, Clone, Default)]
pub struct UsageRecord {
    pub service: String,
    pub operation: String,
    pub usage_type: String,
    pub region: String,
    pub actual_usage_amount: f64,
    pub forecasted_usage_amount: f64,
    pub limit: f64,
    pub unit: String,
    pub description: String,
    pub free_tier_type: String,
}

impl UsageRecord {
    /// The share of the offer's limit the account has used, as a percentage.
    /// Zero when the offer carries no limit, because no share of nothing is
    /// defined.
    pub fn usage_percentage(&self) -> f64 {
        if self.limit == 0.0 {
            return 0.0;
        }
        self.actual_usage_amount / self.limit * 100.0
    }

    /// This record's value for one `Dimension`, or `None` when the dimension
    /// names nothing this record carries.
    pub fn dimension_value(&self, key: &str) -> Option<DimensionValue> {
        let text = match key {
            "SERVICE" => &self.service,
            "OPERATION" => &self.operation,
            "USAGE_TYPE" => &self.usage_type,
            "REGION" => &self.region,
            "FREE_TIER_TYPE" => &self.free_tier_type,
            "DESCRIPTION" => &self.description,
            "USAGE_PERCENTAGE" => return Some(DimensionValue::Number(self.usage_percentage())),
            _ => return None,
        };
        Some(DimensionValue::Text(text.clone()))
    }
}

/// The value a `Dimension` takes on a usage record: text for every dimension
/// but `USAGE_PERCENTAGE`, which is a number and so compares by magnitude
/// rather than by substring.
#[derive(Debug, Clone)]
pub enum DimensionValue {
    Text(String),
    Number(f64),
}

impl DimensionValue {
    /// Which kind of value this is, without the value itself.
    pub fn kind(&self) -> DimensionKind {
        match self {
            Self::Text(_) => DimensionKind::Text,
            Self::Number(_) => DimensionKind::Number,
        }
    }
}

/// Whether a `Dimension` compares text or magnitudes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionKind {
    Text,
    Number,
}

/// The kind of value a `Dimension` takes on a usage record, or `None` when the
/// key names no dimension a record carries.
///
/// A property of the dimension alone — every record answers the same dimension
/// with the same kind — so a filter's comparison can be checked before any
/// record is read. Answered by asking [`UsageRecord::dimension_value`] on an
/// empty record, so the kind reported here and the value a record answers with
/// cannot disagree.
pub fn dimension_kind(key: &str) -> Option<DimensionKind> {
    UsageRecord::default()
        .dimension_value(key)
        .map(|value| value.kind())
}

/// Per-service state for com-amazonaws-freetier-awsfreetierservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsFreetierAwsfreetierserviceState {
    /// Account id -> that account's plan singleton.
    pub plans: HashMap<String, AccountPlan>,
    /// Account id -> its activity catalog, in catalog order.
    pub activities: HashMap<String, Vec<Activity>>,
    /// Account id -> its Free Tier usage records, in report order.
    pub usages: HashMap<String, Vec<UsageRecord>>,
}

impl Default for ComAmazonawsFreetierAwsfreetierserviceState {
    /// The store's initial value: the activity catalog and the usage records the
    /// account can report, and NO account-plan record.
    ///
    /// An account holds account-plan data only once
    /// [`crate::ops::upgrade_account_plan`] — the service's one plan-writing
    /// operation — has established it; until then `GetAccountPlanState` reports
    /// the modelled not-found error rather than a plan nothing stored.
    ///
    /// Seeding the two catalogs here rather than on first read is what lets both
    /// reporting operations answer without writing: a read never changes what a
    /// later caller sees.
    fn default() -> Self {
        let account_id = caller_account_id().to_string();
        let now = now();
        Self {
            plans: HashMap::new(),
            activities: HashMap::from([(account_id.clone(), seed_activities(now))]),
            usages: HashMap::from([(account_id, seed_usages())]),
        }
    }
}

impl ComAmazonawsFreetierAwsfreetierserviceState {
    /// The account's plan as of `now`, with the expiry the clock has already made
    /// due materialised ([`AccountPlan::settle`]), or `None` when this store
    /// holds no plan for the account.
    ///
    /// The lookup a REPORTING operation uses: it answers what is stored and
    /// never brings a plan into being, so an account with no plan data stays
    /// that way however often it is read.
    pub fn settled_plan(&mut self, account_id: &str, now: DateTime) -> Option<AccountPlan> {
        let plan = self.plans.get_mut(account_id)?;
        plan.settle(now);
        Some(plan.clone())
    }

    /// The account's plan for writing, established on the free plan when this
    /// store holds none.
    ///
    /// The lookup the plan-WRITING operation uses: establishing the record is the
    /// mutation the caller asked for. A reporting operation must use
    /// [`Self::settled_plan`] instead.
    pub fn plan_mut(&mut self, account_id: &str) -> &mut AccountPlan {
        self.plans
            .entry(account_id.to_string())
            .or_insert_with(|| AccountPlan::free(account_id, now()))
    }

    /// The account's activity catalog, in catalog order; empty for an account
    /// this store holds no catalog for.
    pub fn activities_for(&self, account_id: &str) -> &[Activity] {
        self.activities
            .get(account_id)
            .map_or(&[][..], Vec::as_slice)
    }

    /// The account's usage records, in report order; empty for an account this
    /// store holds none for.
    pub fn usages_for(&self, account_id: &str) -> &[UsageRecord] {
        self.usages.get(account_id).map_or(&[][..], Vec::as_slice)
    }
}

/// The account whose Free Tier data a request addresses.
///
/// Every generated emulator presents the one canonical account id
/// ([`engine::emulator_account_id`]); it is never derived from the caller's
/// access key.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The current instant, as the modelled `Timestamp`.
pub fn now() -> DateTime {
    DateTime::from(std::time::SystemTime::now())
}

/// Version marker inside every pagination token this service issues, so a token
/// from an older encoding is refused rather than misread.
const PAGE_TOKEN_VERSION: &str = "ftv1";

/// Encode the position a page stopped at — the offset of the next unreported
/// row — into an opaque `nextToken`.
///
/// `kind` names the list the token belongs to, so a token issued by one
/// paginated operation is not accepted by the other.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{kind}:{offset}"))
}

/// The offset a `nextToken` carries, or `None` when the token does not even
/// have the FORM [`encode_page_token`] gives a token of this `kind`.
///
/// The form is only half of "was this token issued": the offset it carries has
/// to be one the issuer could have produced too, which is what
/// [`page_offset`] decides. Handlers go through that one, never through this.
fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}

/// The stored-collection offset a request resumes its listing at: `0` when it
/// carried no `nextToken`, the token's offset when the token is one this
/// service issued for `kind`, and `None` when it is not — which is what the
/// caller refuses with [`INVALID_NEXT_TOKEN`].
///
/// A token is one this service issued only when BOTH halves hold: its form is
/// the form [`encode_page_token`] gives a token of this listing, and the offset
/// it carries is one [`cut_page`] — the only issuer — could have encoded over
/// the `stored_rows` rows the offset indexes into. `cut_page` encodes
/// `position + 1` for a row it has just reported while a further selected row
/// is still unreported, so the offsets it can hand out are exactly
/// `1..stored_rows`: zero (a listing starts there with no token at all) and any
/// offset at or past the end of the stored collection are offsets no page ever
/// issued, and a token carrying one is as unissued as a token of the other
/// listing or of no listing at all. The bound is the STORED collection's, not
/// the filtered view's, because a token stays usable when the caller changes
/// the filter between pages.
pub fn page_offset(kind: &str, token: Option<&str>, stored_rows: usize) -> Option<usize> {
    let Some(token) = token else {
        return Some(0);
    };
    let offset = decode_page_token(kind, token)?;
    (1..stored_rows).contains(&offset).then_some(offset)
}

/// The page size one request asks for: its `maxResults`, or the modelled
/// default when the request carried none. The model declares `@range(1..1000)`,
/// which the framework refuses before a handler runs, so a non-positive value
/// here is only ever the absent member's zero.
pub fn page_size(max_results: i32) -> usize {
    let requested = if max_results <= 0 {
        DEFAULT_MAX_RESULTS
    } else {
        max_results
    };
    usize::try_from(requested).unwrap_or(DEFAULT_MAX_RESULTS as usize)
}

/// The member a page-size refusal names, as both listings' inputs spell it.
const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The largest page `ListAccountActivities` SERVES, which is smaller than the
/// maximum the model declares for the member it is asked through (`MaxResults`
/// is `@range(1..1000)` —
/// `smithy_query(query="shape",shape="MaxResults")`).
///
/// The live listing serves a page size of 100 and refuses 101, so the ceiling a
/// caller of this listing has to respect is this one and not the modelled 1000.
/// It is a property of the LISTING rather than of the member's shape — the usage
/// listing serves the modelled maximum — so each listing passes its own ceiling
/// to [`max_results_refusal`] instead of this being read off the member.
pub const ACTIVITIES_MAX_RESULTS_CEILING: i32 = 100;

/// Whether a request's `maxResults` is above `ceiling`, the largest page the
/// listing it was asked of serves, and the message refusing it when it is.
///
/// A listing that serves a smaller page than its member's `@range` declares
/// enforces the difference itself: the model's bound is checked by the
/// framework's constraint walk in FRONT of the handler, so a value that reaches
/// here is one the model admits and only the listing's own ceiling is left to
/// apply. It is applied before anything is paged, so the refusal cannot depend
/// on how many rows the account holds or on what else the request asked for.
///
/// `None` means the page size is one the listing serves — the ceiling itself
/// included, because a ceiling is a value that is still served. `Some(message)`
/// is the refusal's message, worded by [`crate::service::validation`], the one
/// place this service states a broken input bound.
pub fn max_results_refusal(ceiling: i32, max_results: i32) -> Option<String> {
    (max_results > ceiling)
        .then(|| crate::service::validation::above_ceiling(MAX_RESULTS_MEMBER, ceiling))
}

/// The rows one request selects out of a stored collection, each paired with
/// its position in the STORED order: starting at the offset the request's token
/// resumed at, every row `selects` keeps.
///
/// The step [`cut_page`] cuts, stated here ONCE beside it rather than
/// hand-written in each paginated operation, because it is what makes the token
/// `cut_page` issues an offset into the stored collection: the offset is skipped
/// over the STORED rows and the position paired with a kept row is its stored
/// one, so a page never resumes in the middle of the filtered view and changing
/// the filter between pages cannot make a page skip or repeat a row. `selects`
/// is the only part that differs between the listings — the statuses a request
/// lists, the filter expression it carries — and a request expressing no
/// narrowing selects every row.
pub fn select_rows<T>(
    stored: &[T],
    offset: usize,
    mut selects: impl FnMut(&T) -> bool,
) -> Vec<(usize, &T)> {
    stored
        .iter()
        .enumerate()
        .skip(offset)
        .filter(|&(_, row)| selects(row))
        .collect()
}

/// Cut the rows a request selected down to one page and issue the `nextToken`
/// that resumes the listing after it.
///
/// The part of the pagination rule whose other parts are
/// [`encode_page_token`]/[`page_offset`], [`page_size`] and [`select_rows`],
/// stated here ONCE so both paginated operations page by the same rule: a page
/// reports at most `page_size` of the selected rows, in the order they were
/// selected, and a token is issued only while a selected row is still
/// unreported — the `@paginated` contract is that an absent token ends the
/// iteration, so an exactly-filled last page carries none.
///
/// `rows` pairs each selected item with its position in the STORED order —
/// which is what [`select_rows`], the step before this one, is for — so the
/// token the page issues is an offset into the stored collection and not
/// into the filtered view: changing the filter between pages cannot make a page
/// resume in the middle of a row it already reported. `kind` names the listing
/// the token belongs to, so a token one paginated operation issued is not
/// accepted by the other.
pub fn cut_page<'rows, T>(
    kind: &str,
    rows: &'rows [(usize, T)],
    page_size: usize,
) -> (&'rows [(usize, T)], Option<String>) {
    let page = &rows[..page_size.min(rows.len())];
    let next_token = match page.last() {
        Some((position, _)) if rows.len() > page.len() => {
            Some(encode_page_token(kind, position + 1))
        }
        _ => None,
    };
    (page, next_token)
}

/// What `GetFreeTierUsage` states for a `nextToken` it did not issue — a token
/// of the other listing, of an older encoding, one carrying an offset no page
/// could have stopped at, or one that was never a token at all.
///
/// Stated through [`stated_validation`], the envelope that operation's own
/// refusals carry.
///
/// The CONDITION is one rule, the last part of the pagination rule beside
/// [`encode_page_token`]/[`page_offset`], [`page_size`], [`select_rows`] and
/// [`cut_page`]: "this is not a token of mine", the same whichever listing was
/// asked and whatever the token held, which is why neither sentence names the
/// token. The SENTENCE is not: see [`ACTIVITIES_INVALID_NEXT_TOKEN`].
pub const INVALID_NEXT_TOKEN: &str = "The next token is invalid.";

/// What `ListAccountActivities` states for that same condition, and it states it
/// VERBATIM — no envelope.
///
/// The two listings do not share one sentence: each states the one the live
/// service answers it with, so the wording is a property of the operation that
/// states it and not of the condition alone, and a client must not assume a
/// shared string in either direction. The sentences sit together here, beside
/// the one condition both reach, so that stays visible.
pub const ACTIVITIES_INVALID_NEXT_TOKEN: &str = "Invalid nextToken";

/// The `message` a `ValidationException` this service's OWN validation layer
/// raises carries: the sentence stating the condition, in the envelope that
/// layer states every one of them in.
///
/// The envelope is the layer's rather than any one condition's — a refusal of a
/// filter's root-operator arity, of a match-option count and of a pagination
/// token the service did not issue are all recorded carrying it — so it is
/// written ONCE here and every handler that refuses a request itself states its
/// sentence through this. It is NOT the envelope of the refusal the framework
/// renders in front of a handler for a constraint the model declares: that one
/// is the per-member validation envelope
/// (`crate::service::validation`), which this service's recordings show
/// unwrapped.
///
/// `sentence` states the condition and nothing of the request: a reason string
/// that varied with the input would make a client's branch on it depend on
/// something the rule does not distinguish.
pub fn stated_validation(sentence: &str) -> String {
    format!("[Message = {sentence}]")
}

/// The order this service STATES the `LanguageCode` value set in when it refuses
/// a value outside it.
///
/// A refusal of a value outside an enumerated type names the values that type
/// admits, and the order it names them in is the SERVICE's: the order a value
/// set reaches the refusal in is the order the MODEL declares that enum's
/// members in (`en-US, en-GB, id-ID, de-DE, es-ES, fr-FR, ja-JP, it-IT, pt-PT,
/// ko-KR, zh-CN, zh-TW, tr-TR` \u2014
/// `smithy_query(query="shape",shape="LanguageCode")`), while the service lists
/// the same set in this one. It is FIXED text: no member of the request and
/// nothing in the store varies it, and both activity readers state it alike.
///
/// It holds exactly the values the model declares for that enum, reordered and
/// nothing else, which the `activity_plane_refusals` suite pins against
/// `crate::ops::types::LANGUAGE_CODE_VALUES`.
pub const LANGUAGE_CODE_VALUE_SET: [&str; 13] = [
    "zh-TW", "en-US", "zh-CN", "tr-TR", "pt-PT", "it-IT", "fr-FR", "es-ES", "de-DE", "id-ID",
    "ko-KR", "ja-JP", "en-GB",
];

/// The order this service STATES the `ActivityStatus` value set in, which the
/// model declares `NOT_STARTED, IN_PROGRESS, COMPLETED, EXPIRING`
/// (`smithy_query(query="shape",shape="ActivityStatus")`).
///
/// The sibling of [`LANGUAGE_CODE_VALUE_SET`]; see it for why a published order
/// exists at all. This one is stated nested inside a clause about the LIST that
/// carries the enum (`filterActivityStatuses`), which is a property of where the
/// violation was found and not of the set, so the order is the same either way.
pub const ACTIVITY_STATUS_VALUE_SET: [&str; 4] =
    ["IN_PROGRESS", "COMPLETED", "NOT_STARTED", "EXPIRING"];

/// Every enum value set this service publishes an order for.
///
/// One row per enum whose stated order differs from its declaration order; a
/// value set absent from this table is stated as it was presented, because this
/// service has shown no other order for it.
const PUBLISHED_ENUM_VALUE_SETS: &[&[&str]] =
    &[&LANGUAGE_CODE_VALUE_SET, &ACTIVITY_STATUS_VALUE_SET];

/// `values` in the order this service states that enum's value set in: the
/// published order when `values` is a set it publishes one for
/// ([`PUBLISHED_ENUM_VALUE_SETS`]), and `values` unchanged for any other set.
///
/// An enum is recognised by its value SET rather than by the member that carried
/// the rejected value, so every member targeting one enum \u2014 of any operation, at
/// any depth, whether the clause is stated at the member itself or nested inside
/// one about the list that holds it \u2014 states that enum's set in the one order.
/// The match is on membership and size, never on position, so a model revision
/// that reordered its own declaration of the same enum would still be stated in
/// the published order.
pub fn stated_enum_value_set(values: &[String]) -> Vec<String> {
    match published_enum_value_set(values) {
        Some(published) => published.iter().map(|value| (*value).to_string()).collect(),
        None => values.to_vec(),
    }
}

/// The published order for `values`, or `None` when this service publishes none
/// for that set. The lookup half of [`stated_enum_value_set`].
fn published_enum_value_set(values: &[String]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_VALUE_SETS.iter().copied().find(|published| {
        published.len() == values.len()
            && published
                .iter()
                .all(|value| values.iter().any(|presented| presented == value))
    })
}

/// Whether a request's `languageCode` names a language an activity reader can
/// answer in, and the message refusing it when it does not.
///
/// The sibling of [`LocalizedText::in_language`], which resolves the codes this
/// accepts: both activity readers document `languageCode` as the code the
/// translated text is returned in, so a value outside the modelled
/// `LanguageCode` enum (`crate::ops::types::LANGUAGE_CODE_VALUES`) names no
/// language the catalog could be read in and is refused rather than silently
/// answered in another. An absent code is accepted: the member is optional and
/// [`DEFAULT_LANGUAGE_CODE`] answers it.
///
/// `None` means the code is one a reader can answer. `Some(message)` is the
/// refusal's message, which each reader raises inside its OWN modelled
/// `ValidationException`, so the two readers refuse in one wording.
pub fn language_code_refusal(language_code: Option<&str>) -> Option<String> {
    let code = language_code?;
    if LANGUAGE_CODE_VALUES.contains(&code) {
        return None;
    }
    Some(format!(
        "languageCode: {code} is not a language code; expected one of {}",
        LANGUAGE_CODE_VALUES.join(", ")
    ))
}

/// Whether `activity_id` satisfies the modelled `ActivityId` constraints:
/// exactly 32 characters, each of `[a-zA-Z0-9]`.
pub fn is_activity_id(activity_id: &str) -> bool {
    activity_id.len() == 32 && activity_id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The activity catalog this emulator reports for an account.
///
/// The emulator observes no real account activity, so the catalog is a fixed
/// set of rows rather than something derived from traffic — but every row is
/// stored, and both activity readers read these same rows. The statuses span
/// the whole `ActivityStatus` enum so a status filter has something to select,
/// and each status carries exactly the timestamps that status implies.
///
/// Each row's `title` and `description` are stored as [`LocalizedText`]: the
/// base text in [`DEFAULT_LANGUAGE_CODE`] plus a translation for every other
/// language the model's `LanguageCode` enum declares
/// (`crate::ops::types::LANGUAGE_CODE_VALUES`), so a reader's `languageCode`
/// selects which stored text is reported. `en-GB` holds no separate entry
/// because British English does not differ from the base text of these rows;
/// it resolves to the base through [`LocalizedText::in_language`]'s
/// same-language step. `instructionsUrl` is NOT localized: the model documents
/// the title and description as the translated fields.
fn seed_activities(now: DateTime) -> Vec<Activity> {
    let secs = now.secs();
    vec![
        Activity {
            activity_id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".to_string(),
            title: LocalizedText::new(
                "Launch an Amazon EC2 instance",
                &[
                    ("de-DE", "Eine Amazon EC2-Instance starten"),
                    ("es-ES", "Lanzar una instancia de Amazon EC2"),
                    ("fr-FR", "Lancer une instance Amazon EC2"),
                    ("id-ID", "Luncurkan instans Amazon EC2"),
                    ("it-IT", "Avvia un'istanza Amazon EC2"),
                    ("ja-JP", "Amazon EC2 インスタンスを起動する"),
                    ("ko-KR", "Amazon EC2 인스턴스 시작하기"),
                    ("pt-PT", "Iniciar uma instância do Amazon EC2"),
                    ("tr-TR", "Amazon EC2 örneği başlatın"),
                    ("zh-CN", "启动 Amazon EC2 实例"),
                    ("zh-TW", "啟動 Amazon EC2 執行個體"),
                ],
            ),
            description: LocalizedText::new(
                "Launch your first Amazon EC2 instance and keep it running for five \
                 minutes to earn Free Tier credits.",
                &[
                    (
                        "de-DE",
                        "Starten Sie Ihre erste Amazon EC2-Instance und lassen Sie sie fünf \
                         Minuten laufen, um Free-Tier-Guthaben zu erhalten.",
                    ),
                    (
                        "es-ES",
                        "Lanza tu primera instancia de Amazon EC2 y mantenla en ejecución \
                         durante cinco minutos para obtener créditos de la capa gratuita.",
                    ),
                    (
                        "fr-FR",
                        "Lancez votre première instance Amazon EC2 et laissez-la fonctionner \
                         pendant cinq minutes pour gagner des crédits de l'offre gratuite.",
                    ),
                    (
                        "id-ID",
                        "Luncurkan instans Amazon EC2 pertama Anda dan biarkan berjalan \
                         selama lima menit untuk mendapatkan kredit Tingkat Gratis.",
                    ),
                    (
                        "it-IT",
                        "Avvia la tua prima istanza Amazon EC2 e lasciala in esecuzione per \
                         cinque minuti per ottenere crediti del piano gratuito.",
                    ),
                    (
                        "ja-JP",
                        "最初の Amazon EC2 インスタンスを起動し、5 分間実行したままにして\
                         無料利用枠のクレジットを獲得します。",
                    ),
                    (
                        "ko-KR",
                        "첫 Amazon EC2 인스턴스를 시작하고 5분 동안 실행하여 프리 티어 \
                         크레딧을 받으세요.",
                    ),
                    (
                        "pt-PT",
                        "Inicie a sua primeira instância do Amazon EC2 e mantenha-a em \
                         execução durante cinco minutos para ganhar créditos do nível \
                         gratuito.",
                    ),
                    (
                        "tr-TR",
                        "İlk Amazon EC2 örneğinizi başlatın ve Ücretsiz Kullanım kredisi \
                         kazanmak için beş dakika çalışır durumda bırakın.",
                    ),
                    (
                        "zh-CN",
                        "启动您的第一个 Amazon EC2 实例并保持运行五分钟，即可获得免费套餐积分。",
                    ),
                    (
                        "zh-TW",
                        "啟動您的第一個 Amazon EC2 執行個體並保持執行五分鐘，即可獲得免費方案額度。",
                    ),
                ],
            ),
            instructions_url: "https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/EC2_GetStarted.html"
                .to_string(),
            status: ACTIVITY_STATUS_NOT_STARTED.to_string(),
            reward: credit(ACTIVITY_CREDIT),
            estimated_time_to_complete_in_minutes: Some(10),
            expires_at: None,
            started_at: None,
            completed_at: None,
        },
        Activity {
            activity_id: "0f1e2d3c4b5a69788796a5b4c3d2e1f0".to_string(),
            title: LocalizedText::new(
                "Create a budget in AWS Budgets",
                &[
                    ("de-DE", "Ein Budget in AWS Budgets erstellen"),
                    ("es-ES", "Crear un presupuesto en AWS Budgets"),
                    ("fr-FR", "Créer un budget dans AWS Budgets"),
                    ("id-ID", "Buat anggaran di AWS Budgets"),
                    ("it-IT", "Crea un budget in AWS Budgets"),
                    ("ja-JP", "AWS Budgets で予算を作成する"),
                    ("ko-KR", "AWS Budgets에서 예산 생성하기"),
                    ("pt-PT", "Criar um orçamento no AWS Budgets"),
                    ("tr-TR", "AWS Budgets'ta bütçe oluşturun"),
                    ("zh-CN", "在 AWS Budgets 中创建预算"),
                    ("zh-TW", "在 AWS Budgets 中建立預算"),
                ],
            ),
            description: LocalizedText::new(
                "Create a cost budget so you are alerted before your spend passes a \
                 threshold you choose.",
                &[
                    (
                        "de-DE",
                        "Erstellen Sie ein Kostenbudget, damit Sie benachrichtigt werden, \
                         bevor Ihre Ausgaben einen von Ihnen gewählten Schwellenwert \
                         überschreiten.",
                    ),
                    (
                        "es-ES",
                        "Crea un presupuesto de costes para recibir una alerta antes de que \
                         tu gasto supere el umbral que elijas.",
                    ),
                    (
                        "fr-FR",
                        "Créez un budget de coûts pour être alerté avant que vos dépenses ne \
                         dépassent le seuil que vous choisissez.",
                    ),
                    (
                        "id-ID",
                        "Buat anggaran biaya agar Anda mendapat peringatan sebelum \
                         pengeluaran Anda melewati batas yang Anda pilih.",
                    ),
                    (
                        "it-IT",
                        "Crea un budget dei costi per ricevere un avviso prima che la tua \
                         spesa superi la soglia che scegli.",
                    ),
                    (
                        "ja-JP",
                        "コスト予算を作成して、支出が選択したしきい値を超える前に通知を受け取ります。",
                    ),
                    (
                        "ko-KR",
                        "비용 예산을 만들어 지출이 선택한 임계값을 넘기 전에 알림을 받으세요.",
                    ),
                    (
                        "pt-PT",
                        "Crie um orçamento de custos para ser alertado antes de as suas \
                         despesas excederem o limite que escolher.",
                    ),
                    (
                        "tr-TR",
                        "Harcamanız seçtiğiniz eşiği aşmadan önce uyarı almak için bir \
                         maliyet bütçesi oluşturun.",
                    ),
                    (
                        "zh-CN",
                        "创建成本预算，以便在支出超过您选择的阈值之前收到提醒。",
                    ),
                    (
                        "zh-TW",
                        "建立成本預算，以便在支出超過您選擇的閾值之前收到警示。",
                    ),
                ],
            ),
            instructions_url: "https://docs.aws.amazon.com/cost-management/latest/userguide/budgets-create.html"
                .to_string(),
            status: ACTIVITY_STATUS_COMPLETED.to_string(),
            reward: credit(ACTIVITY_CREDIT),
            estimated_time_to_complete_in_minutes: Some(5),
            expires_at: None,
            started_at: Some(DateTime::from_secs(secs - 3 * DAY)),
            completed_at: Some(DateTime::from_secs(secs - 2 * DAY)),
        },
        Activity {
            activity_id: "1122334455667788990011223344aabb".to_string(),
            title: LocalizedText::new(
                "Launch an Amazon RDS database",
                &[
                    ("de-DE", "Eine Amazon RDS-Datenbank starten"),
                    ("es-ES", "Lanzar una base de datos de Amazon RDS"),
                    ("fr-FR", "Lancer une base de données Amazon RDS"),
                    ("id-ID", "Luncurkan basis data Amazon RDS"),
                    ("it-IT", "Avvia un database Amazon RDS"),
                    ("ja-JP", "Amazon RDS データベースを起動する"),
                    ("ko-KR", "Amazon RDS 데이터베이스 시작하기"),
                    ("pt-PT", "Iniciar uma base de dados do Amazon RDS"),
                    ("tr-TR", "Amazon RDS veritabanı başlatın"),
                    ("zh-CN", "启动 Amazon RDS 数据库"),
                    ("zh-TW", "啟動 Amazon RDS 資料庫"),
                ],
            ),
            description: LocalizedText::new(
                "Create a managed database instance with Amazon RDS and connect to it.",
                &[
                    (
                        "de-DE",
                        "Erstellen Sie eine verwaltete Datenbankinstanz mit Amazon RDS und \
                         verbinden Sie sich mit ihr.",
                    ),
                    (
                        "es-ES",
                        "Crea una instancia de base de datos administrada con Amazon RDS y \
                         conéctate a ella.",
                    ),
                    (
                        "fr-FR",
                        "Créez une instance de base de données gérée avec Amazon RDS et \
                         connectez-vous à celle-ci.",
                    ),
                    (
                        "id-ID",
                        "Buat instans basis data terkelola dengan Amazon RDS dan hubungkan \
                         ke instans tersebut.",
                    ),
                    (
                        "it-IT",
                        "Crea un'istanza di database gestita con Amazon RDS e connettiti a \
                         essa.",
                    ),
                    (
                        "ja-JP",
                        "Amazon RDS でマネージドデータベースインスタンスを作成し、接続します。",
                    ),
                    (
                        "ko-KR",
                        "Amazon RDS로 관리형 데이터베이스 인스턴스를 만들고 연결하세요.",
                    ),
                    (
                        "pt-PT",
                        "Crie uma instância de base de dados gerida com o Amazon RDS e \
                         ligue-se a ela.",
                    ),
                    (
                        "tr-TR",
                        "Amazon RDS ile yönetilen bir veritabanı örneği oluşturun ve ona \
                         bağlanın.",
                    ),
                    (
                        "zh-CN",
                        "使用 Amazon RDS 创建托管数据库实例并连接到它。",
                    ),
                    (
                        "zh-TW",
                        "使用 Amazon RDS 建立受管資料庫執行個體並連線到該執行個體。",
                    ),
                ],
            ),
            instructions_url: "https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/CHAP_GettingStarted.html"
                .to_string(),
            status: ACTIVITY_STATUS_IN_PROGRESS.to_string(),
            reward: credit(ACTIVITY_CREDIT),
            estimated_time_to_complete_in_minutes: Some(20),
            expires_at: None,
            started_at: Some(DateTime::from_secs(secs - DAY)),
            completed_at: None,
        },
        Activity {
            activity_id: "deadbeefcafef00d0123456789abcdef".to_string(),
            title: LocalizedText::new(
                "Invoke an AWS Lambda function",
                &[
                    ("de-DE", "Eine AWS Lambda-Funktion aufrufen"),
                    ("es-ES", "Invocar una función de AWS Lambda"),
                    ("fr-FR", "Invoquer une fonction AWS Lambda"),
                    ("id-ID", "Panggil fungsi AWS Lambda"),
                    ("it-IT", "Richiama una funzione AWS Lambda"),
                    ("ja-JP", "AWS Lambda 関数を呼び出す"),
                    ("ko-KR", "AWS Lambda 함수 호출하기"),
                    ("pt-PT", "Invocar uma função do AWS Lambda"),
                    ("tr-TR", "AWS Lambda işlevi çağırın"),
                    ("zh-CN", "调用 AWS Lambda 函数"),
                    ("zh-TW", "叫用 AWS Lambda 函數"),
                ],
            ),
            description: LocalizedText::new(
                "Create a Lambda function from a blueprint and invoke it once.",
                &[
                    (
                        "de-DE",
                        "Erstellen Sie eine Lambda-Funktion aus einer Blaupause und rufen \
                         Sie sie einmal auf.",
                    ),
                    (
                        "es-ES",
                        "Crea una función de Lambda a partir de un modelo e invócala una vez.",
                    ),
                    (
                        "fr-FR",
                        "Créez une fonction Lambda à partir d'un modèle et invoquez-la une \
                         fois.",
                    ),
                    (
                        "id-ID",
                        "Buat fungsi Lambda dari cetak biru dan panggil sekali.",
                    ),
                    (
                        "it-IT",
                        "Crea una funzione Lambda da un modello e richiamala una volta.",
                    ),
                    (
                        "ja-JP",
                        "設計図から Lambda 関数を作成し、1 回呼び出します。",
                    ),
                    (
                        "ko-KR",
                        "블루프린트로 Lambda 함수를 만들고 한 번 호출하세요.",
                    ),
                    (
                        "pt-PT",
                        "Crie uma função do Lambda a partir de um modelo e invoque-a uma vez.",
                    ),
                    (
                        "tr-TR",
                        "Bir şablondan Lambda işlevi oluşturun ve bir kez çağırın.",
                    ),
                    ("zh-CN", "从蓝图创建 Lambda 函数并调用一次。"),
                    ("zh-TW", "從藍圖建立 Lambda 函數並叫用一次。"),
                ],
            ),
            instructions_url: "https://docs.aws.amazon.com/lambda/latest/dg/getting-started.html"
                .to_string(),
            status: ACTIVITY_STATUS_NOT_STARTED.to_string(),
            reward: credit(ACTIVITY_CREDIT),
            estimated_time_to_complete_in_minutes: Some(15),
            expires_at: None,
            started_at: None,
            completed_at: None,
        },
        Activity {
            activity_id: "ABCDEF0123456789abcdef0123456789".to_string(),
            title: LocalizedText::new(
                "Run an inference with Amazon Bedrock",
                &[
                    ("de-DE", "Eine Inferenz mit Amazon Bedrock ausführen"),
                    ("es-ES", "Ejecutar una inferencia con Amazon Bedrock"),
                    ("fr-FR", "Exécuter une inférence avec Amazon Bedrock"),
                    ("id-ID", "Jalankan inferensi dengan Amazon Bedrock"),
                    ("it-IT", "Esegui un'inferenza con Amazon Bedrock"),
                    ("ja-JP", "Amazon Bedrock で推論を実行する"),
                    ("ko-KR", "Amazon Bedrock으로 추론 실행하기"),
                    ("pt-PT", "Executar uma inferência com o Amazon Bedrock"),
                    ("tr-TR", "Amazon Bedrock ile çıkarım çalıştırın"),
                    ("zh-CN", "使用 Amazon Bedrock 运行推理"),
                    ("zh-TW", "使用 Amazon Bedrock 執行推論"),
                ],
            ),
            description: LocalizedText::new(
                "Send a prompt to a foundation model in the Amazon Bedrock console.",
                &[
                    (
                        "de-DE",
                        "Senden Sie eine Eingabeaufforderung an ein Basismodell in der \
                         Amazon Bedrock-Konsole.",
                    ),
                    (
                        "es-ES",
                        "Envía una indicación a un modelo fundacional en la consola de \
                         Amazon Bedrock.",
                    ),
                    (
                        "fr-FR",
                        "Envoyez une invite à un modèle de fondation dans la console Amazon \
                         Bedrock.",
                    ),
                    (
                        "id-ID",
                        "Kirim prompt ke model dasar di konsol Amazon Bedrock.",
                    ),
                    (
                        "it-IT",
                        "Invia un prompt a un modello di base nella console Amazon Bedrock.",
                    ),
                    (
                        "ja-JP",
                        "Amazon Bedrock コンソールで基盤モデルにプロンプトを送信します。",
                    ),
                    (
                        "ko-KR",
                        "Amazon Bedrock 콘솔에서 파운데이션 모델에 프롬프트를 보내세요.",
                    ),
                    (
                        "pt-PT",
                        "Envie um pedido a um modelo fundacional na consola do Amazon \
                         Bedrock.",
                    ),
                    (
                        "tr-TR",
                        "Amazon Bedrock konsolunda bir temel modele istem gönderin.",
                    ),
                    (
                        "zh-CN",
                        "在 Amazon Bedrock 控制台中向基础模型发送提示。",
                    ),
                    (
                        "zh-TW",
                        "在 Amazon Bedrock 主控台中向基礎模型傳送提示。",
                    ),
                ],
            ),
            instructions_url: "https://docs.aws.amazon.com/bedrock/latest/userguide/getting-started.html"
                .to_string(),
            status: ACTIVITY_STATUS_EXPIRING.to_string(),
            reward: credit(ACTIVITY_CREDIT),
            estimated_time_to_complete_in_minutes: Some(10),
            expires_at: Some(DateTime::from_secs(secs + 7 * DAY)),
            // Stored, but `GetAccountActivity` reports `startedAt` only for an
            // IN_PROGRESS or COMPLETED activity, so this row's is withheld.
            started_at: Some(DateTime::from_secs(secs - 5 * DAY)),
            completed_at: None,
        },
    ]
}

/// A reward of `amount` credits in the only currency the model declares.
fn credit(amount: f64) -> MonetaryAmount {
    MonetaryAmount {
        amount,
        unit: CURRENCY_USD.to_string(),
    }
}

/// The Free Tier usage records this emulator reports for an account.
///
/// As with the activity catalog, the emulator meters no real usage, so the rows
/// are fixed and stored; `GetFreeTierUsage` filters and pages exactly these.
/// The rows span several services, regions, usage types and `freeTierType`s so
/// that every filter dimension has something to select on, and two rows share a
/// service so a filter can select more than one row.
fn seed_usages() -> Vec<UsageRecord> {
    vec![
        UsageRecord {
            service: "AmazonEC2".to_string(),
            operation: "RunInstances".to_string(),
            usage_type: "BoxUsage:t2.micro".to_string(),
            region: "us-east-1".to_string(),
            actual_usage_amount: 350.0,
            forecasted_usage_amount: 744.0,
            limit: 750.0,
            unit: "Hrs".to_string(),
            description: "750 Hrs of Amazon EC2 t2.micro instance usage free per month"
                .to_string(),
            free_tier_type: "12 Months Free".to_string(),
        },
        UsageRecord {
            service: "AmazonS3".to_string(),
            operation: "PutObject".to_string(),
            usage_type: "TimedStorage-ByteHrs".to_string(),
            region: "us-east-1".to_string(),
            actual_usage_amount: 2.5,
            forecasted_usage_amount: 5.0,
            limit: 5.0,
            unit: "GB-Mo".to_string(),
            description: "5 GB of Amazon S3 standard storage free per month".to_string(),
            free_tier_type: "12 Months Free".to_string(),
        },
        UsageRecord {
            service: "AWSLambda".to_string(),
            operation: "Invoke".to_string(),
            usage_type: "Request".to_string(),
            region: "us-east-1".to_string(),
            actual_usage_amount: 250_000.0,
            forecasted_usage_amount: 900_000.0,
            limit: 1_000_000.0,
            unit: "Requests".to_string(),
            description: "1 million free AWS Lambda requests per month".to_string(),
            free_tier_type: "Always Free".to_string(),
        },
        UsageRecord {
            service: "AWSLambda".to_string(),
            operation: "Invoke".to_string(),
            usage_type: "Lambda-GB-Second".to_string(),
            region: "us-west-2".to_string(),
            actual_usage_amount: 360_000.0,
            forecasted_usage_amount: 400_000.0,
            limit: 400_000.0,
            unit: "Second".to_string(),
            description: "400,000 GB-seconds of AWS Lambda compute time free per month"
                .to_string(),
            free_tier_type: "Always Free".to_string(),
        },
        UsageRecord {
            service: "AmazonDynamoDB".to_string(),
            operation: "PutItem".to_string(),
            usage_type: "TimedStorage-ByteHrs".to_string(),
            region: "us-west-2".to_string(),
            actual_usage_amount: 12.5,
            forecasted_usage_amount: 25.0,
            limit: 25.0,
            unit: "GB-Mo".to_string(),
            description: "25 GB of Amazon DynamoDB storage free per month".to_string(),
            free_tier_type: "Always Free".to_string(),
        },
        UsageRecord {
            service: "AmazonRDS".to_string(),
            operation: "CreateDBInstance".to_string(),
            usage_type: "InstanceUsage:db.t3.micro".to_string(),
            region: "eu-west-1".to_string(),
            actual_usage_amount: 100.0,
            forecasted_usage_amount: 750.0,
            limit: 750.0,
            unit: "Hrs".to_string(),
            description: "750 Hrs of Amazon RDS db.t3.micro instance usage free per month"
                .to_string(),
            free_tier_type: "12 Months Free".to_string(),
        },
    ]
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFreetierAwsfreetierserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFreetierAwsfreetierserviceState>)
}
