Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# The template listing's NextToken is a positional offset, not the key position state.rs says it keeps

_Filed: 2026-10-29T13:36:30Z_
_Operation: ListServiceQuotaIncreaseRequestsInTemplate | Class: pagination_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::template_entries` states, as the reason the template is stored in a key-ordered `BTreeMap`, that "a pagination token keeps meaning the same position as entries are added and removed around it". The code under that comment does not do it: the handler builds the filtered `Vec` and hands it to `crate::state::paginate`, whose token is nothing but a base64-encoded OFFSET into the list that was just built. The template is keyed by `(ServiceCode, AwsRegion, QuotaCode)`, and `put_template_entry` / `remove_template_entry` insert and delete at the key's own position — so an entry added or removed BEFORE the offset shifts every later entry by one and the offset no longer names the position it was issued for.

Two consequences a client observes inside the emulator, both of them the opposite of the comment's claim:
* a `PutServiceQuotaIncreaseRequestIntoTemplate` for a `QuotaCode` that sorts ahead of the page boundary makes the last entry of page 1 be reported AGAIN as the first entry of page 2, while the newly added entry is never reported by the walk at all;
* a `DeleteServiceQuotaIncreaseRequestFromTemplate` of an entry ahead of the boundary pulls the listing up by one, so the entry that sat at the stored offset is skipped and appears on no page.

The same handler is the only consumer of `template_entries`, so the rule the comment states is the rule this operation's token is supposed to honour and nothing else does.

## Evidence

The rule the crate states for THIS listing — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:1840-1842:

    /// The order is the key's own — `(ServiceCode, AwsRegion, QuotaCode)` — so it is
    /// the same for two callers and a pagination token keeps meaning the same position
    /// as entries are added and removed around it.
    pub fn template_entries(

The token is a plain offset into the freshly built list — src/state.rs:2447-2463:

    let start = match next_token {
        Some(token) => decode_page_token(listing, token)?,
        None => 0,
    };
    let total = all.len();
    ...
    let items: Vec<T> = all.into_iter().skip(start).take(size).collect();
    let stopped_at = start + items.len();
    Ok(Page {
        items,
        next_token: (stopped_at < total).then(|| encode_page_token(listing, stopped_at)),
    })

...and src/state.rs:2467-2469:

    /// The opaque token for "resume `listing` at `offset`".
    fn encode_page_token(listing: &str, offset: usize) -> String {
        base64::engine::general_purpose::STANDARD.encode(format!("{listing}:{offset}"))
    }

The handler builds the list and pages it with that offset token — src/ops/list_service_quota_increase_requests_in_template.rs:174-190:

        let entries = state.lock().template_entries(
            input.service_code.as_deref(),
            input.aws_region.as_deref(),
        );
        ...
        let page = crate::state::paginate(
            &label,
            entries,
            input.max_results,
            input.next_token.as_deref(),
        )

The writers that shift those offsets between two pages — src/state.rs:1814-1820 and 1832-1834:

    pub fn put_template_entry(&mut self, key: TemplateKey, desired_value: f64) -> bool {
        let replaces = self.templates.entries.contains_key(&key);
        ...
        self.templates.entries.insert(key, desired_value);

    pub fn remove_template_entry(&mut self, key: &TemplateKey) -> bool {
        self.templates.entries.remove(key).is_some()
    }

Test coverage: tests/all/quota_request_template.rs walks the listing a page at a time through `walk_pages` (lines 604-636, called at line 568) and only checks that a full walk of a listing that is NEVER mutated mid-walk yields the same entries as one unpaged call; lines 576-599 pin only that another selection's token is refused. No test puts or deletes an entry between two pages, so nothing pins the claim at state.rs:1840-1842.

## Steps to Reproduce

1. Boot the emulator with the calling account in an AWS Organization with ALL features enabled, and make every call below at the template plane's home Region us-east-1 (crate::state::TEMPLATE_HOME_REGION).
2. POST / with X-Amz-Target: ServiceQuotasV20190624.PutServiceQuotaIncreaseRequestIntoTemplate and body {"ServiceCode":"vpc","QuotaCode":"L-A4707A72","AwsRegion":"us-east-1","DesiredValue":10} (Internet gateways per Region, default 5).
3. Repeat the Put with {"ServiceCode":"vpc","QuotaCode":"L-F678F1CE","AwsRegion":"us-east-1","DesiredValue":10} (VPCs per Region, default 5). The template now holds, in key order, L-A4707A72 then L-F678F1CE.
4. POST ListServiceQuotaIncreaseRequestsInTemplate with {"MaxResults":1} -> one entry (L-A4707A72) plus a NextToken that encodes offset 1. Keep the token.
5. POST Put with {"ServiceCode":"vpc","QuotaCode":"L-045E6E9C","AwsRegion":"us-east-1","DesiredValue":6000} (Network interfaces per Region, default 5000) — this QuotaCode sorts BEFORE both stored keys, so the listing becomes L-045E6E9C, L-A4707A72, L-F678F1CE.
6. Resume with {"MaxResults":1,"NextToken":"<token from step 4>"} -> the page holds L-A4707A72 again (already reported on page 1), and continuing the walk never reports L-045E6E9C at all.
7. For the skip variant: with the three entries stored, take a {"MaxResults":1} page and its token, then POST DeleteServiceQuotaIncreaseRequestFromTemplate for L-045E6E9C and resume with the token -> the entry that occupied offset 1 is reported on no page.
