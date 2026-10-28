Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 41cd94e3f39433730586ca0c36d19074975940e2

# ListRules accepts a NextToken it never issued whenever the token decodes to an identifier-shaped string, answering 200 with a silently truncated (often empty) page

_Filed: 2026-10-28T14:54:54Z_
_Operation: ListRules | Class: pagination_

> _Note: this issue was identified against the code base at commit `41cd94e3f39433730586ca0c36d19074975940e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRules` resolves its cursor through `crate::state::decode_page_token`, which accepts ANY token that is base64 of 11 ASCII-alphanumeric bytes — it only checks the SHAPE of the decoded value (`is_rule_identifier`), never that the value is a cursor this service handed out, nor that it names a rule the Region this request is signed for holds. The decoded string is then used directly as `Bound::Excluded(..)` over the whole rule map.

So a token the service never issued — base64 of any 11-character alphanumeric string a client invents — is treated as a valid cursor and the walk resumes "after" it. Because every minted identifier is rendered left-padded with `'0'` (`base62_identifier`), a forged token such as base64("ZZZZZZZZZZZ") = `WlpaWlpaWlpaWlo=` sorts after every stored identifier, so the request is answered HTTP 200 with `"Rules": []` and no `NextToken` — the caller concludes the Region holds no retention rules of that type. A forged token in the middle of the alphabet silently drops every rule that sorts before it.

That is the exact outcome the handler's own comment and `decode_page_token`'s own doc say must not happen (the service must answer the modelled `ValidationException` with `Reason=INVALID_PAGE_TOKEN`, which the crate already composes and already returns for tokens that fail the base64/shape check). A refusal the crate implements is turned into a success carrying wrong data.

Second symptom of the same root cause: the token carries nothing but an identifier, so a cursor minted by one Region's endpoint is accepted by another Region's endpoint, contradicting `page_from`'s doc claim that "a cursor it issued names a rule of that Region" — the foreign cursor simply skips every rule of the answering Region that sorts at or before it, again with HTTP 200.

## Evidence

The rule the crate states for a token it did not issue — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:1209-1213`:
```
/// Decode a page cursor back to the identifier the previous page stopped at.
///
/// `None` for any token this service did not emit — it is base64 of something that
/// is not a rule identifier — so the caller can refuse it as an invalid page token
/// rather than silently returning the first page.
```
and `src/ops/list_rules.rs:169-170`:
```
        // A `NextToken` this service did not mint resolves to no cursor at all;
        // answering the first page instead would silently restart the walk.
```

The code that breaks it — `src/state.rs:1214-1218`:
```
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::aws_smithy_types::base64::decode(token).ok()?;
    let identifier = String::from_utf8(bytes).ok()?;
    is_rule_identifier(&identifier).then_some(identifier)
}
```
where the only test applied is a SHAPE test, `src/state.rs:1188-1190`:
```
/// Whether `candidate` is shaped like a `RuleIdentifier` (`^[0-9a-zA-Z]{11}$`).
pub fn is_rule_identifier(candidate: &str) -> bool {
    candidate.len() == IDENTIFIER_LEN && candidate.bytes().all(|b| b.is_ascii_alphanumeric())
```
and the decoded value is used as the cursor with no further check, `src/state.rs:1155-1161`:
```
        let lower = match after {
            Some(identifier) => Bound::Excluded(identifier.to_string()),
            None => Bound::Unbounded,
        };
        self.rules
            .range((lower, Bound::Unbounded))
            .filter(move |(_, rule)| rule.region == region)
```
against that same function's doc, `src/state.rs:1148-1149`:
```
    /// Region-scoped like every other resolver: a regional reader pages through the
    /// rules its own Region holds, and a cursor it issued names a rule of that Region.
```

The refusal path that exists but is never reached for such a token — `src/ops/list_rules.rs:171-191` (`decode_page_token` -> `None` => `ValidationException { reason: INVALID_PAGE_TOKEN }`).

Test coverage: both suites that pin this fault class claim to cover "a token this service did not issue" but only exercise tokens that fail the base64/shape check, so the accepted-forgery case is untested.
`tests/all/retention_rule_lifecycle.rs:626-635`:
```
async fn list_rules_refuses_a_next_token_it_never_minted() {
    let router = router();
    // Not base64, base64 of something that is not an identifier, and base64 of an
    // identifier-shaped value of the wrong length.
    let rows = [
        ("not base64", "****"),
        ("base64 of a non-identifier", "bm90LWFuLWlk"),
        ("base64 of a short identifier", "YWFh"),
    ];
```
and `tests/all/error_response_bodies.rs:823-834`:
```
async fn a_page_token_this_service_did_not_issue_is_the_page_token_fault_class() {
...
    for token in ["AAAA", "Zm9vYmFy", "abc=", "0123456789abcdef"] {
```
None of the eight rows is base64 of an 11-character alphanumeric value, which is exactly the class that slips through.

## Steps to Reproduce

1. POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — HTTP 201, a rule now exists in us-east-1.
2. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT"} — HTTP 200, the rule is reported, no NextToken.
3. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","NextToken":"WlpaWlpaWlpaWlo="} (base64 of "ZZZZZZZZZZZ", a token the service never minted, whose syntax satisfies NextToken's @pattern). Expected per src/ops/list_rules.rs:169-170 and src/state.rs:1211-1213: HTTP 400 ValidationException with Reason=INVALID_PAGE_TOKEN, as for "Zm9vYmFy". Actual: HTTP 200 with "Rules": [] and no NextToken — the forged cursor was accepted and sorts after every minted identifier, so the caller is told the Region holds no EBS_SNAPSHOT rules.
4. Create several rules, take the NextToken of a real first page, and replay it against another Region's endpoint (sign the request for us-west-2, which holds its own rules): the foreign cursor is accepted and that Region's rules sorting at or before it are silently omitted, although src/state.rs:1148-1149 states a cursor names a rule of the answering Region.
