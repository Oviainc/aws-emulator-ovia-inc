Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 9d7fd75a9416876f17d684b80aaadc94961fb271

# PutProcurementPortalPreference drops its ClientToken, so a retry of one request is counted as a second write (Version/LastUpdateDate advance)

_Filed: 2026-10-28T17:52:34Z_
_Operation: PutProcurementPortalPreference | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `9d7fd75a9416876f17d684b80aaadc94961fb271`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutProcurementPortalPreference` is the only `@idempotent` operation of this crate that never reads its `ClientToken`. The member is accepted, validated by the boundary and then silently dropped: the handler mentions it only in a comment explaining that nothing is suppressed for it. Every other `@idempotent` operation of this service threads the token through the crate's own replay ledger (`crate::state::replayed_result` / `record_result`), so a retry answers with what the first call answered and performs its work ONCE.

Because the token is dropped, a client that retries a Put it did not see the answer to — which is exactly what an idempotency token exists for — has its retry applied as a fresh write: `Version` is incremented again and `LastUpdateDate` is re-stamped. Both are `@required` members of the stored `ProcurementPortalPreference` that `GetProcurementPortalPreference` and `ListProcurementPortalPreferences` report, so the retry is observably not a replay: three identical Puts under ONE `ClientToken` leave the resource reporting `Version` 4 where one call leaves it reporting 2.

The handler's own justification — "applying it twice leaves the resource with exactly the configuration it states — which is what makes the operation idempotent" — holds for the configuration members only; it does not hold for `Version` and `LastUpdateDate`, which the crate's sibling partial-write handler explicitly treats as part of what a replay must not move ("the resource's `Version` counts the writes it took", which is why that handler consults the ledger). The fix is the ledger call the other eight writes already make — key the replay on (operation, preference ARN, token) and answer the recorded ARN without touching `Version`/`LastUpdateDate` — not a new mechanism.

## Evidence

Smithy, `smithy_query(query="doc", shape="PutProcurementPortalPreferenceRequest", member="ClientToken")`:
  "A unique, case-sensitive identifier that you provide to ensure idempotency of the request."
(the operation also carries `@idempotent` — `smithy_query(query="operation", operation="PutProcurementPortalPreference")`), and the handler restates the sentence in its own module doc — services/service-com-amazonaws-invoicing-invoicing/src/ops/put_procurement_portal_preference.rs:69:
```
//! * `ClientToken` (BasicStringWithoutSpace, len 0..1024, pattern ^\S+$, @idempotencyToken) —
```

The member is then never read. `grep -n 'client_token\|ClientToken' src/ops/put_procurement_portal_preference.rs` matches exactly two lines — the doc line above and this comment, put_procurement_portal_preference.rs:210-216:
```
        // `@idempotent`, and nothing is suppressed for a `ClientToken`: this request
        // STATES the configuration it wants (every configuration member a caller sets is
        // a member of this input), so applying it twice leaves the resource with exactly
        // the configuration it states — which is what makes the operation idempotent. The
        // token is what lets a client retry a write it did not see the answer to; it is
        // not a record of a resource a retry must not create a second time, as it is for
        // `CreateProcurementPortalPreference`.
```
and the write it guards nothing against — put_procurement_portal_preference.rs:304-311:
```
            // `Version` is "the version number of the procurement portal preference
            // configuration" ... and `LastUpdateDate` is the instant it
            // changed at. `CreateDate` is not touched ...
            preference.version = preference.version.saturating_add(1);
            preference.last_update_date = updated_at;
```
Both members are `@required` on the shape a reader gets back — `smithy_query(query="shape", shape="ProcurementPortalPreference")`: `"Version": {"target": "Long", "required": true, "doc": "The version number of the procurement portal preference configuration."}`, `"LastUpdateDate": {"target": "Timestamp", "required": true}` — and `GetProcurementPortalPreference` returns the stored structure verbatim (src/ops/get_procurement_portal_preference.rs:180-182).

The sibling partial write of the SAME resource does consult the ledger, for the reason this handler denies — src/ops/update_procurement_portal_preference_status.rs:340-352:
```
        // `@idempotent`: a replay (the same `ClientToken` against the same preference)
        // answers with the ARN the first call returned and applies the update once — this
        // request states only the planes it moves, so re-applying it is not the same as
        // applying it once, and the resource's `Version` counts the writes it took.
        if let Some(arn) = store.replayed_result(
            Self::NAME,
            &input.procurement_portal_preference_arn,
            client_token,
        ) {
            return Ok(UpdateProcurementPortalPreferenceStatusOutput {
                procurement_portal_preference_arn: arn,
            });
        }
```

The ledger exists and is reached by EIGHT of this crate's nine `@idempotent` operations — src/state.rs:446 `pub fn replayed_result(` / src/state.rs:465 `pub fn record_result(`, called from create_invoice_unit.rs:171, update_invoice_unit.rs:174, delete_invoice_unit.rs:165, create_procurement_portal_preference.rs:257, delete_procurement_portal_preference.rs:183, update_procurement_portal_preference_status.rs:344, send_procurement_portal_validation.rs:215, verify_procurement_portal_validation.rs:236. `put_procurement_portal_preference.rs` is the only `@idempotent` handler absent from that list.

Disclosure — one test pins today's behaviour, with a reason drawn from the configuration members alone: tests/all/procurement_portal_preference_lifecycle.rs:788-791:
```
/// RULE: an update is idempotent in its EFFECT — repeating the same request leaves the
/// resource with exactly the configuration that request states, whether or not it carries
/// a `ClientToken` — while the version counter counts the WRITES the resource took, so it
/// advances once per successful call.
```
and :837-853, which sends the SAME body three times (with the token set at :834-836) and asserts `Version == 1 + round` each round. That reason is what the modelled sentence above contradicts: with a `ClientToken`, the repeat is the same REQUEST, so it is not another write for the counter to count. Fixing this changes that one table's expectation for the token-carrying rows (the `None` row keeps `1 + round`); no other test asserts a Put's `Version` under a repeated token.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"ZIP","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"AN02","EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"Contacts":[{"Name":"Ops","Email":"ops@example.com"}]}`; note the returned `ProcurementPortalPreferenceArn` (the create leaves `Version` 1).
3. POST / with `X-Amz-Target: Invoicing.PutProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>","Contacts":[{"Name":"Ops","Email":"ops@example.com"}],"EinvoiceDeliveryEnabled":true,"PurchaseOrderRetrievalEnabled":true,"ClientToken":"retry-me"}`.
4. Send the BYTE-IDENTICAL request again (the retry a client makes when it did not see the first answer), same `ClientToken`.
5. POST / with `X-Amz-Target: Invoicing.GetProcurementPortalPreference` and body `{"ProcurementPortalPreferenceArn":"<arn>"}`.
6. Observe `Version: 3` and a `LastUpdateDate` stamped by the retry. One request was made (one `ClientToken`), so a reader must see `Version: 2` and the first call's `LastUpdateDate` — compare `UpdateProcurementPortalPreferenceStatus`, where the same retry under one token is replayed and advances `Version` once.
