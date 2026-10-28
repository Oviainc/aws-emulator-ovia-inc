Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 8aa1bc3970afbc278ab25e0eb153e57974fffad6

# GetPersonalizedRanking skips the crate's own empty-string rule for campaignArn, so a request naming no campaign is told a campaign "does not exist"

_Filed: 2026-10-28T18:18:25Z_
_Operation: GetPersonalizedRanking | Class: error-response_

> _Note: this issue was identified against the code base at commit `8aa1bc3970afbc278ab25e0eb153e57974fffad6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in one place, how every handler must read an empty string in a member whose modelled `@length` starts at 0: the boundary admits it, and the handler treats it as ABSENT. `crate::state::present` exists for exactly that, and it names `campaignArn` explicitly. Both sibling handlers run `campaignArn` through it (`GetRecommendations` and `GetActionRecommendations`), and `GetPersonalizedRanking` itself applies the same rule to `userId` (`input.user_id.is_empty()`), but it passes `campaignArn` straight into the registry lookup without `present`.

The consequence: `campaignArn: ""` (which the modelled `length 0..256` on `Arn` admits, so it reaches the handler) is answered as an UNRESOLVABLE ARN rather than as a request that named no campaign. `Denied::unresolvable("")` formats `"{arn} does not exist or is not active yet."` with an empty `arn`, so the client receives the message `" does not exist or is not active yet."` — a sentence that names no member, shows no value, and asserts that a campaign the request never named either was never created or is not yet ACTIVE (a retryable condition). The two sibling operations answer the same request by reporting that no campaign was given (`"Either campaignArn or recommenderArn is required."` / `"The given campaign ARN is invalid: "`). The HTTP status and error shape are right (400 `InvalidInputException`); what is wrong is that one of the three handlers drops the crate-wide rule for this member, so the refusal describes the wrong fault.

## Evidence

The rule, stated once for the whole crate — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:686-693`:

    /// A request member that is present and not empty, as a `&str`.
    ///
    /// The modelled length bounds on `campaignArn`, `userId` and `itemId` all start at 0,
    /// so the boundary validator accepts an empty string for each; an empty string names
    /// no resource and identifies no user, so every handler treats it as absent.
    pub fn present(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.is_empty())
    }

The handler that does not — `src/ops/get_personalized_ranking.rs:210`:

    let campaign = guard.campaign(&input.campaign_arn).map_err(deny)?.clone();

(no `registry::present(...)`, although the SAME handler applies the rule to `userId` eight lines earlier, `src/ops/get_personalized_ranking.rs:202-207`:

    if input.user_id.is_empty() {
        return Err(invalid(
            "userId is required: it is the user the campaign provides a personalized ranking \
             for.",
        ));
    }
)

Both siblings do apply it to `campaignArn` — `src/ops/get_recommendations.rs:221-225`:

    let resolved = resolve(
        &guard,
        registry::present(input.campaign_arn.as_deref()),
        registry::present(input.recommender_arn.as_deref()),
    )?;

and `src/ops/get_action_recommendations.rs:175-177`:

    let Some(campaign_arn) = registry::present(input.campaign_arn.as_deref()) else {
        return Err(invalid_campaign_arn(input.campaign_arn.as_deref()));
    };

What the skipped rule produces — `src/state.rs:449-452` and `src/state.rs:520-522`:

    pub fn campaign(&self, arn: &str) -> Result<&Campaign, Denied> {
        self.campaigns
            .get(arn)
            .ok_or_else(|| Denied::unresolvable(arn))
    }
...
    pub fn unresolvable(arn: &str) -> Self {
        Self::invalid(format!("{arn} does not exist or is not active yet."))
    }

An empty ARN can never be a registered campaign, so the lookup always misses — `src/state.rs:734-736`:

    if campaign.arn.is_empty() {
        return false;
    }

Test coverage: the empty-string form of a member IS the pinned form elsewhere in this crate — `tests/all/request_only_refusal_order.rs:274-280` sends `{"userId": ""}` to GetPersonalizedRanking and expects the member-naming refusal, and `tests/all/optional_input_refusal_order.rs:100-105` drives "the member sent empty" (`Some(Value::from(""))`) for the siblings' optional members. No row anywhere sends `campaignArn: ""` to GetPersonalizedRanking: `tests/all/unresolved_resource_arns.rs:118-144` varies only well-formed ARNs, so this path is untested.

## Steps to Reproduce

1. Start an emulator with this service mounted and register nothing (an empty registry is enough — an empty ARN can never be registered, see state.rs:734-736).
2. POST /personalize-ranking with body {"campaignArn": "", "userId": "user-1", "inputList": ["item-1"]} (the modelled Arn length is 0..256, so the boundary admits the empty string and the handler runs).
3. Observe 400 InvalidInputException whose message is " does not exist or is not active yet." — it names no member, echoes no value, and claims a campaign the request never named is missing or not yet ACTIVE.
4. POST /recommendations with {"campaignArn": "", "userId": "user-1"} and POST /action-recommendations with {"campaignArn": "", "userId": "user-1"} for comparison: both sibling handlers run the member through registry::present and report that no campaign was given ("Either campaignArn or recommenderArn is required." and "The given campaign ARN is invalid: " respectively).
5. Compare with state.rs:686-693, which states that an empty string "names no resource" and that "every handler treats it as absent".
