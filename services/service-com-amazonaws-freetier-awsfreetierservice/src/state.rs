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

use std::collections::HashMap;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{ActivityReward, MonetaryAmount};

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

    /// Move the plan onto `plan_type`. The plan becomes `ACTIVE` on the type it
    /// now holds, and a paid plan drops the expiration date a free plan carries
    /// because it does not expire.
    pub fn upgrade_to(&mut self, plan_type: &str) {
        self.plan_type = plan_type.to_string();
        self.plan_status = PLAN_STATUS_ACTIVE.to_string();
        if plan_type == PLAN_TYPE_PAID {
            self.expiration_date = None;
        }
    }
}

/// One activity in the account's catalog. It serves BOTH
/// `ListAccountActivities` (the summary members) and `GetAccountActivity` (all
/// of them), so one row is the single source of truth for an activity.
#[derive(Debug, Clone)]
pub struct Activity {
    /// `ActivityId`: exactly 32 characters of `[a-zA-Z0-9]`.
    pub activity_id: String,
    pub title: String,
    pub description: String,
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

/// The offset a `nextToken` resumes at, or `None` when the token was not issued
/// by [`encode_page_token`] for this `kind`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
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
fn seed_activities(now: DateTime) -> Vec<Activity> {
    let secs = now.secs();
    vec![
        Activity {
            activity_id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".to_string(),
            title: "Launch an Amazon EC2 instance".to_string(),
            description: "Launch your first Amazon EC2 instance and keep it running for \
                          five minutes to earn Free Tier credits."
                .to_string(),
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
            title: "Create a budget in AWS Budgets".to_string(),
            description: "Create a cost budget so you are alerted before your spend \
                          passes a threshold you choose."
                .to_string(),
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
            title: "Launch an Amazon RDS database".to_string(),
            description: "Create a managed database instance with Amazon RDS and connect \
                          to it."
                .to_string(),
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
            title: "Invoke an AWS Lambda function".to_string(),
            description: "Create a Lambda function from a blueprint and invoke it once."
                .to_string(),
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
            title: "Run an inference with Amazon Bedrock".to_string(),
            description: "Send a prompt to a foundation model in the Amazon Bedrock \
                          console."
                .to_string(),
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
