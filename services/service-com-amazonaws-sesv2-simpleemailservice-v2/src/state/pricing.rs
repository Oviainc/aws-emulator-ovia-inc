// SPDX-License-Identifier: CC0-1.0

//! The account's pricing plane: how the pricing plans rank against each other, and
//! where a requested plan lands on the account's pricing attributes.
//!
//! Split out of the state module root so this one concern lives in its own file;
//! every item is re-exported from [`crate::state`], so a caller still reaches it as
//! `crate::state::…`.

use super::AccountState;

/// The pricing plans from the cheapest to the richest.
///
/// The ORDER is the plan ladder, which is what decides whether a requested plan is
/// an upgrade of the active one: `PutAccountPricingAttributesRequest$Plan`
/// (`smithy_query(query="doc",shape="PutAccountPricingAttributesRequest",member="Plan")`)
/// lists the plans it accepts in exactly this order, as does the `PricingPlan` shape
/// itself, and that is the order of the tiers SES prices — `NONE` being the account
/// on no paid plan ([`super::PRICING_PLAN_NONE`]).
///
/// The value SET is the same one `crate::ops::types::PRICING_PLAN_VALUES` holds; that
/// catalog is sorted, so it states no ladder and cannot answer this question.
pub const PRICING_PLAN_LADDER: [&str; 4] = ["NONE", "ESSENTIALS", "PRO", "ENTERPRISE"];

/// Where `plan` sits on [`PRICING_PLAN_LADDER`], and `None` for a value that is not
/// a plan at all.
///
/// Two plans are compared by this rank alone, so nothing else in the service has to
/// know how many plans there are or which of them is the richest.
pub fn pricing_plan_rank(plan: &str) -> Option<usize> {
    PRICING_PLAN_LADDER
        .iter()
        .position(|known| *known == plan)
}

impl AccountState {
    /// Record a request for the account to be on `plan`, and report whether that
    /// plan became active at once.
    ///
    /// A plan ABOVE the one currently active is a move up the ladder, which the
    /// account starts paying for immediately: it becomes `CurrentPlan` there and
    /// then. Any other plan is the plan the account is asking to be on from the next
    /// billing cycle, so the active plan is left alone and the requested one is
    /// staged as `CurrentPlan`'s companion `NextPlan` — "the pricing plan that will
    /// become active at the start of the next monthly cycle, if a scheduled change
    /// has been requested".
    ///
    /// Either way the account is left with ONE outstanding request, the latest: a
    /// request for the plan already active schedules nothing, and `NextPlan` "is
    /// empty when no scheduled change is pending", so whatever an earlier request
    /// staged is dropped the moment a request leaves nothing to activate.
    pub fn request_pricing_plan(&mut self, plan: &str) -> bool {
        let activates_now = match (pricing_plan_rank(&self.pricing_plan), pricing_plan_rank(plan)) {
            (Some(active), Some(requested)) => requested > active,
            // A value off the ladder cannot be placed against the active plan, so it
            // is not an upgrade of it.
            _ => false,
        };
        if activates_now {
            self.pricing_plan = plan.to_string();
            self.next_pricing_plan = None;
        } else if plan == self.pricing_plan {
            self.next_pricing_plan = None;
        } else {
            self.next_pricing_plan = Some(plan.to_string());
        }
        activates_now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder holds every plan the wire accepts, exactly once: a plan missing
    /// from it would be unrankable and so could never be an upgrade.
    #[test]
    fn the_ladder_holds_every_modelled_plan() {
        for plan in crate::ops::types::PRICING_PLAN_VALUES {
            assert_eq!(
                PRICING_PLAN_LADDER
                    .iter()
                    .filter(|known| *known == plan)
                    .count(),
                1,
                "case {plan:?}: expected the plan once on the ladder, got {PRICING_PLAN_LADDER:?}"
            );
        }
        assert_eq!(
            PRICING_PLAN_LADDER.len(),
            crate::ops::types::PRICING_PLAN_VALUES.len(),
            "case ladder: expected the ladder to hold only modelled plans, got \
             {PRICING_PLAN_LADDER:?}"
        );
        assert_eq!(
            pricing_plan_rank(super::super::PRICING_PLAN_NONE),
            Some(0),
            "case none: expected the unpaid plan at the foot of the ladder, got {:?}",
            pricing_plan_rank(super::super::PRICING_PLAN_NONE)
        );
    }

    /// Every ordered pair of plans: a move up activates at once, and anything else
    /// is staged for the next cycle with the active plan untouched.
    #[test]
    fn a_move_up_activates_at_once_and_everything_else_is_staged() {
        for active in PRICING_PLAN_LADDER {
            for requested in PRICING_PLAN_LADDER {
                let case = format!("{active} -> {requested}");
                let mut account = AccountState {
                    pricing_plan: active.to_string(),
                    // An earlier request's staging, so each row also pins what
                    // becomes of a change already scheduled.
                    next_pricing_plan: Some("PRO".to_string()),
                    ..AccountState::default()
                };
                let activated = account.request_pricing_plan(requested);
                let up = pricing_plan_rank(requested) > pricing_plan_rank(active);
                assert_eq!(
                    activated, up,
                    "case {case:?}: expected immediate activation {up}, got {activated}"
                );
                let expected = if up {
                    (requested.to_string(), None)
                } else if requested == active {
                    (active.to_string(), None)
                } else {
                    (active.to_string(), Some(requested.to_string()))
                };
                assert_eq!(
                    (account.pricing_plan.clone(), account.next_pricing_plan.clone()),
                    expected,
                    "case {case:?}: expected the plans {expected:?}, got {:?}",
                    (account.pricing_plan, account.next_pricing_plan)
                );
            }
        }
    }
}
