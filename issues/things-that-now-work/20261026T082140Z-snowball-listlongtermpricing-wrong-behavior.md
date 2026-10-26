Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A long-term pricing contract created with IsLongTermPricingAutoRenew=true is reported EXPIRED at its end date instead of being renewed

_Filed: 2026-10-26T08:21:40Z_
_Operation: ListLongTermPricing | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLongTermPricing` persists the submitted `IsLongTermPricingAutoRenew` flag and `ListLongTermPricing` echoes it back, but the flag has no effect on anything the service computes. The contract's reported `LongTermPricingStatus` is derived purely from the end date: `status_at` answers the stored status while `now < long_term_pricing_end_date` and `EXPIRED` from that instant on, with no branch on the auto-renew flag and no advance of `long_term_pricing_end_date`.

The model documents the flag as a behaviour, not a label: an auto-renew contract 'should be automatically renewed before the long-term pricing contract expires'. The emulator instead expires it, so `ListLongTermPricing` reports `LongTermPricingStatus=EXPIRED` with an end date in the past for a contract the model says never reaches that point, and `CurrentActiveJob`/`JobIds` keep pointing at jobs billed to a contract the service claims has lapsed.

This is the same clock-driven derivation the crate uses elsewhere; it simply ignores the one stored field that decides which way the clock's due change goes.

## Evidence

The modelled sentence that states the behaviour (smithy_query(query="doc", shape="LongTermPricingListEntry", member="IsLongTermPricingAutoRenew")):
"If set to <code>true</code>, specifies that the current long-term pricing type for the device should be automatically renewed before the long-term pricing contract expires."

The handler-side computation that contradicts it, services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/state.rs:854-860:
```
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.long_term_pricing_end_date {
            &self.long_term_pricing_status
        } else {
            LONG_TERM_PRICING_STATUS_EXPIRED
        }
    }
```
`self.is_long_term_pricing_auto_renew` is never read here, and `long_term_pricing_end_date` is never moved forward (it is written once, at create time: src/ops/create_long_term_pricing.rs:238 `let Some(end_date) = plus_calendar_months(start_date, *term_months)`).

The crate nonetheless stores the flag on purpose, src/ops/create_long_term_pricing.rs:264-267:
```
            // "Specifies whether the current long-term pricing type for the device should
            // be renewed": a create that does not ask for renewal does not get it, so the
            // stored contract answers the question either way.
            is_long_term_pricing_auto_renew: input.is_long_term_pricing_auto_renew.unwrap_or(false),
```
and state.rs:892 feeds `status_at` straight into the response: `long_term_pricing_status: Some(self.status_at(now).to_string())`.

Test coverage: tests/all/long_term_pricing_listing.rs:450-454 only pins that a contract INSIDE its term reads ACTIVE ("expected a contract inside its term to be reported active"); a grep of the suite for `EXPIRED`/`status_at` returns no test at all, so no existing test pins the expiry-vs-auto-renew behaviour.

## Steps to Reproduce

1. CreateLongTermPricing with LongTermPricingType=OneMonth, SnowballType=EDGE, IsLongTermPricingAutoRenew=true.
2. ListLongTermPricing: the entry reports LongTermPricingStatus=ACTIVE and an end date one calendar month out, with IsLongTermPricingAutoRenew=true.
3. Advance past long_term_pricing_end_date (or seed the stored contract's start date more than one month in the past, as the crate's own test helpers do).
4. ListLongTermPricing again: the entry reports LongTermPricingStatus=EXPIRED with an unchanged, now-past LongTermPricingEndDate, although the stored IsLongTermPricingAutoRenew=true says the contract "should be automatically renewed before the long-term pricing contract expires".
