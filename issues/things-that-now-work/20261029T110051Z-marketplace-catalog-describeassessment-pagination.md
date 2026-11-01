Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: e14af6b59464e58f15b7f39517598ee7f904626e

# decode_page_token honours offset spellings encode_page_token never writes ("+N", leading zeros, uppercase hex), so a token this service never issued is served as a page

_Filed: 2026-10-29T11:00:51Z_
_Operation: DescribeAssessment | Class: pagination_

> _Note: this issue was identified against the code base at commit `e14af6b59464e58f15b7f39517598ee7f904626e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate (`DescribeAssessment`, `ListAssessments`, `ListChangeSets`, `ListEntities`) resolves its `NextToken` through the single shared decision `crate::state::page_offset` -> `decode_page_token`. That decision documents its own rule: only a token THIS SERVICE ISSUED for the list being called resumes a page, and "anything else ... names no page of this list and is the one `invalid_page_token` fault".

The one place a token is minted, `encode_page_token`, writes exactly `format!("{prefix}-{offset:x}")` — a bare lowercase hexadecimal offset with no sign and no leading zeros. The decoder, however, hands the remainder to `usize::from_str_radix(rest, 16)`, which accepts three further spellings the minter can never produce:

* a leading `+` (`CTRL-+1a` decodes to offset 26),
* leading zeros (`CTRL-0000001a` decodes to offset 26),
* uppercase hex digits (`CTRL-1A` decodes to offset 26, although `{:x}` only ever writes `1a`).

All three reach the handler: the modelled `NextToken` pattern `^[\w+=.:@\-\/]+$` admits `+` and the alphanumerics, so the boundary validator passes them through to `page_offset`. The result is that a value this service never issued is silently served as a page instead of being refused with the one page-token `ValidationException` the crate says it answers — an error turned into a success, and a page positioned by a client-fabricated offset.

The crate's sibling refusals in the same helper are strict about exactly this: `CTRL-0x10`, `CTRL-zz`, `CTRL-` and `CTRL_0` are all refused, so the decoder clearly intends to accept only its own spelling; the three spellings above slip through only because `from_str_radix` is more permissive than the format string.

## Evidence

The documented rule, `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:1441-1445` (doc on `page_offset`):
```
/// `NextToken` asks for the first page (offset zero), one carrying a token this
/// service issued for the list being called resumes where that token points, and
/// anything else — a token outside the modelled `NextToken` syntax, a well-formed
/// token naming no page, or a token another list of this service issued — names
/// no page of this list and is the one [`invalid_page_token`] fault.
```

The ONE minting site, `src/state.rs:2874-2876`:
```
pub fn encode_page_token(prefix: &str, offset: usize) -> String {
    format!("{prefix}-{offset:x}")
}
```

The decoder that contradicts it, `src/state.rs:2880-2883`:
```
pub fn decode_page_token(prefix: &str, token: &str) -> Option<usize> {
    let rest = token.strip_prefix(prefix)?.strip_prefix('-')?;
    usize::from_str_radix(rest, 16).ok()
}
```
`usize::from_str_radix` accepts an optional leading `+`, any number of leading zeros, and uppercase `A`-`F`; `{offset:x}` writes none of those.

The handler call site, `src/ops/describe_assessment.rs:222-230`:
```
        let offset = match crate::state::page_offset(
            crate::state::CONTROL_PAGE_PREFIX,
            input.next_token.as_deref(),
        ) {
            Ok(offset) => offset,
            Err(fault) => {
                return Err(DescribeAssessmentErrorModeled::ValidationException(fault).into())
            }
        };
```
(the same three lines appear in `list_assessments.rs:192-200`, `list_change_sets.rs:245-253` and `list_entities.rs:343-349`).

The modelled member that lets `+` through the boundary — `smithy_query(query="operation", operation="DescribeAssessment")`: `"NextToken": {"target": "NextToken", "length": {"min": 1, "max": 4096}, "pattern": "^[\\w+=.:@\\-\\/]+$"}`.

Test coverage: `tests/all/page_token_rule.rs:250-280` enumerates "the ways a `NextToken` can name no page" — `"not a valid token !!"`, `"0"`, `prefix`, `format!("{prefix}_0")`, `format!("{prefix}-zz")`, `format!("{prefix}-")`, `format!("{prefix}-0x10")`, `format!("{prefix}-{}", "f".repeat(40))` — and names NONE of `+N`, leading zeros or uppercase hex, so no test pins the current acceptance.

## Steps to Reproduce

1. Start a change set that produces an assessment: POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"AddDeliveryOptions","Entity":{"Type":"AmiProduct"},"Details":"{\"DeliveryOptions\":[{\"Details\":{\"AmiDeliveryOptionDetails\":{\"AmiSource\":{\"AmiId\":\"ami-1\",\"AccessRoleArn\":\"arn:aws:iam::000000000000:role/r\",\"UserName\":\"ec2-user\"},\"UsageInstructions\":\"run it\"}}}]}"}]} and wait CHANGE_SET_PREPARING + CHANGE_SET_APPLYING.
2. POST /ListAssessments {"Catalog":"AWSMarketplace"} and read the AssessmentId.
3. POST /DescribeAssessment {"Catalog":"AWSMarketplace","AssessmentIdentifier":"<id>","MaxResults":1} and note the issued NextToken, e.g. "CTRL-1".
4. Repeat with "NextToken":"CTRL-+1" — observe 200 and the second control result, instead of the 422 ValidationException "[Requested page token is invalid.]" the crate answers for every other token it did not issue.
5. Repeat with "NextToken":"CTRL-0000001" and with "NextToken":"CTRL-A" (for offset 10 on a longer list) — both are served as pages, although encode_page_token writes neither spelling.
6. Do the same against /ListChangeSets ("CS-+1"), /ListEntities ("ENT-+1") and /ListAssessments ("ASMT-+1"): all four paginated operations accept them, since they share decode_page_token.
