// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions service.
//!
//! The service holds exactly one kind of record: a *recommended action* that AWS
//! Billing and Cost Management computed for an account. Its model binds a single
//! operation, `ListRecommendedActions`, which only reports — there is no public
//! create, update or delete — so the store STARTS with the inventory this account
//! has ([`computed_recommendations`], the stand-in for the real service's own
//! recommendation generator) and is re-filled through the seeding seam below
//! ([`RecommendedActionsStore::seed`]).
//!
//! Records are held in INSERTION ORDER: `ListRecommendedActions` is `@paginated`,
//! and a stable order is what makes its `nextToken` resume exactly where the
//! previous page stopped without duplicating or skipping a record.

use std::collections::BTreeMap;

/// Declare a Smithy `enum` shape as a Rust enum plus its wire spelling.
///
/// Every member list below is copied from this service's Smithy model; see the
/// doc comment on each enum for the `smithy_query` call that produced it. Keeping
/// them as Rust enums means a record in the store cannot hold a value the model
/// does not declare, and `from_wire` is the single membership test the request
/// members the model types as enums are validated against.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident => $wire:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every member the model declares, in model order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// This member's wire value.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }

            /// The member `wire` spells, or `None` when the enum has no such member.
            pub fn from_wire(wire: &str) -> Option<Self> {
                match wire { $($wire => Some(Self::$variant),)+ _ => None }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#ActionType` — the action a customer can
    /// take by adopting the recommendation.
    ///
    /// Members from `smithy_query(query="shape", shape="ActionType")`.
    ActionType {
        AddAlternateBillingContact => "ADD_ALTERNATE_BILLING_CONTACT",
        CreateAnomalyMonitor => "CREATE_ANOMALY_MONITOR",
        CreateBudget => "CREATE_BUDGET",
        EnableCostOptimizationHub => "ENABLE_COST_OPTIMIZATION_HUB",
        MigrateToGranularPermissions => "MIGRATE_TO_GRANULAR_PERMISSIONS",
        PaymentsDue => "PAYMENTS_DUE",
        PaymentsPastDue => "PAYMENTS_PAST_DUE",
        ReviewAnomalies => "REVIEW_ANOMALIES",
        ReviewBudgetAlerts => "REVIEW_BUDGET_ALERTS",
        ReviewBudgetsExceeded => "REVIEW_BUDGETS_EXCEEDED",
        ReviewExpiringRi => "REVIEW_EXPIRING_RI",
        ReviewExpiringSp => "REVIEW_EXPIRING_SP",
        ReviewFreetierUsageAlerts => "REVIEW_FREETIER_USAGE_ALERTS",
        ReviewFreetierCreditsRemaining => "REVIEW_FREETIER_CREDITS_REMAINING",
        ReviewFreetierDaysRemaining => "REVIEW_FREETIER_DAYS_REMAINING",
        ReviewSavingsOpportunityRecommendations => "REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS",
        UpdateExpiredPaymentMethod => "UPDATE_EXPIRED_PAYMENT_METHOD",
        UpdateInvalidPaymentMethod => "UPDATE_INVALID_PAYMENT_METHOD",
        UpdateTaxExemptionCertificate => "UPDATE_TAX_EXEMPTION_CERTIFICATE",
        UpdateTaxRegistrationNumber => "UPDATE_TAX_REGISTRATION_NUMBER",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#Severity` — how urgent the action is.
    ///
    /// Members from `smithy_query(query="shape", shape="Severity")`.
    Severity {
        Info => "INFO",
        Warning => "WARNING",
        Critical => "CRITICAL",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#Feature` — the Billing and Cost
    /// Management feature the action belongs to.
    ///
    /// Members from `smithy_query(query="shape", shape="Feature")`.
    Feature {
        Account => "ACCOUNT",
        Budgets => "BUDGETS",
        CostAnomalyDetection => "COST_ANOMALY_DETECTION",
        CostOptimizationHub => "COST_OPTIMIZATION_HUB",
        FreeTier => "FREE_TIER",
        Iam => "IAM",
        Payments => "PAYMENTS",
        Reservations => "RESERVATIONS",
        SavingsPlans => "SAVINGS_PLANS",
        TaxSettings => "TAX_SETTINGS",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#FilterName` — the record field an
    /// `ActionFilter` filters on.
    ///
    /// Members from `smithy_query(query="shape", shape="FilterName")`.
    FilterName {
        Feature => "FEATURE",
        Severity => "SEVERITY",
        Type => "TYPE",
    }
}

wire_enum! {
    /// `com.amazonaws.bcmrecommendedactions#MatchOption` — whether an
    /// `ActionFilter` keeps or excludes the records it matches.
    ///
    /// Members from `smithy_query(query="shape", shape="MatchOption")`.
    MatchOption {
        Equals => "EQUALS",
        NotEquals => "NOT_EQUALS",
    }
}

/// The wire members of an enum, as the value set a refusal states them — the one
/// mapping behind every value set this service names in a `ValidationException`.
///
/// Only the request members the model TYPES as enums are stated this way: a member
/// whose target is a plain string closes no value set, so what a caller sends in it
/// is compared against the stored records rather than checked against a list.
pub fn member_strings<E: std::fmt::Display>(all: &[E]) -> Vec<String> {
    all.iter().map(std::string::ToString::to_string).collect()
}

/// One recommended action held for an account: every member of the wire
/// `RecommendedAction` shape, persisted.
///
/// `context` is a `BTreeMap` so the map a response carries is rendered in a
/// stable order, and `last_updated_timestamp` is the ISO-8601 instant the action's
/// status last changed (the model types `RecommendedAction$lastUpdatedTimeStamp`
/// as a plain string, so it is stored and returned verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAction {
    /// `RecommendedAction$id`.
    pub id: String,
    /// `RecommendedAction$type`.
    pub action_type: ActionType,
    /// `RecommendedAction$accountId` — 12 digits (`^[0-9]{12}$`).
    pub account_id: String,
    /// `RecommendedAction$severity`.
    pub severity: Severity,
    /// `RecommendedAction$feature`.
    pub feature: Feature,
    /// `RecommendedAction$context`.
    pub context: BTreeMap<String, String>,
    /// `RecommendedAction$nextSteps`.
    pub next_steps: Vec<String>,
    /// `RecommendedAction$lastUpdatedTimeStamp`.
    pub last_updated_timestamp: String,
}

/// The instant a record was last recomputed, as `RecommendedAction$lastUpdatedTimeStamp`
/// carries it: the member is modelled as a plain `String`, and AWS renders these
/// instants ISO-8601 in UTC.
fn iso8601(at: std::time::SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One record of the inventory the service computed for this account, built from
/// the action it recommends.
fn computed(
    id: &str,
    action_type: ActionType,
    severity: Severity,
    feature: Feature,
    next_step: &str,
    computed_at: &str,
) -> StoredAction {
    StoredAction {
        id: id.to_string(),
        action_type,
        account_id: ::engine::emulator_account_id().to_string(),
        severity,
        feature,
        context: BTreeMap::from([(
            "feature".to_string(),
            feature.as_str().to_string(),
        )]),
        next_steps: vec![next_step.to_string()],
        last_updated_timestamp: computed_at.to_string(),
    }
}

/// The recommended actions this emulator holds when it starts.
///
/// `ListRecommendedActions` only REPORTS, and the service models no create
/// operation because AWS itself computes the recommendations — so an inventory
/// filled ONLY through [`RecommendedActionsStore::seed`] reports nothing at all to
/// a caller that never seeded it, and an exclusion-style query (`NOT_EQUALS`) over
/// it can never return the complement it exists to return. The store therefore
/// starts with the set this account has, the way the real service's generator
/// produces one; `seed` and `clear` stay the seam that installs another.
///
/// Every record names a modelled `ActionType`, `Severity` and `Feature`, and the
/// account the emulator presents ([`engine::emulator_account_id`]). The three
/// severities differ, so a filter on any one of them keeps some records and excludes
/// others. The inventory is fixed rather than derived from traffic: this emulator
/// observes no billing usage to derive one from.
fn computed_recommendations() -> Vec<StoredAction> {
    let computed_at = iso8601(std::time::SystemTime::now());
    vec![
        computed(
            "ra-savings-opportunities",
            ActionType::ReviewSavingsOpportunityRecommendations,
            Severity::Info,
            Feature::CostOptimizationHub,
            "Review the savings opportunities in Cost Optimization Hub.",
            &computed_at,
        ),
        computed(
            "ra-expiring-reserved-instances",
            ActionType::ReviewExpiringRi,
            Severity::Warning,
            Feature::Reservations,
            "Review the Reserved Instances that expire soon.",
            &computed_at,
        ),
        computed(
            "ra-expiring-savings-plans",
            ActionType::ReviewExpiringSp,
            Severity::Critical,
            Feature::SavingsPlans,
            "Review the Savings Plans that expire soon.",
            &computed_at,
        ),
    ]
}

/// The recommended actions this emulator holds, in insertion order.
#[derive(Debug)]
pub struct RecommendedActionsStore {
    actions: Vec<StoredAction>,
}

impl Default for RecommendedActionsStore {
    /// The inventory the service computed for this account — see
    /// [`computed_recommendations`]. Part of the store's INITIAL value rather than
    /// something a reader installs on first use, so `ListRecommendedActions` stays
    /// a pure read: two callers see the same records whichever of them called first.
    fn default() -> Self {
        Self {
            actions: computed_recommendations(),
        }
    }
}

impl RecommendedActionsStore {
    /// Install recommended actions, keeping insertion order.
    ///
    /// THE SEEDING SEAM. The service models no create operation — AWS itself
    /// computes these records — so this is how the emulator's inventory is
    /// populated. An action whose `id` is already held is REPLACED in place, so
    /// re-seeding an updated record neither duplicates it nor moves it to the end
    /// of the page order.
    pub fn seed<I: IntoIterator<Item = StoredAction>>(&mut self, actions: I) {
        for action in actions {
            match self.actions.iter_mut().find(|held| held.id == action.id) {
                Some(held) => *held = action,
                None => self.actions.push(action),
            }
        }
    }

    /// Drop every held action — the seam a caller uses to model the service having
    /// recomputed its recommendations down to none.
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// The held actions, in insertion order.
    pub fn actions(&self) -> &[StoredAction] {
        &self.actions
    }

    /// How many actions are held.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether no action is held.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Per-service state for com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState {
    /// The recommended actions `ListRecommendedActions` reports.
    pub recommended_actions: RecommendedActionsStore,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>)
}
