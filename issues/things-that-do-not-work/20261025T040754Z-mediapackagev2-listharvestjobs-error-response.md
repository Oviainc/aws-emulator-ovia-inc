Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# ListHarvestJobs words its invalid-pagination-token refusal differently from every other paged list in the service

_Filed: 2026-10-25T04:07:54Z_
_Operation: ListHarvestJobs | Class: error-response_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paged lists in this crate refuse a token the service did not issue with the same modeled 400 and the `INVALID_PAGINATION_TOKEN` discriminator, but three of them build the body through the shared pair `crate::state::validation_failed("nextToken", crate::state::INVALID_PAGINATION_TOKEN_DETAIL, "INVALID_PAGINATION_TOKEN")` while ListHarvestJobs hand-rolls a different sentence with no member path.

The shared constant exists precisely to prevent that: its doc says the detail is "The same sentence whichever list is being paged", and `validation_message`'s doc says every `ValidationException` the service raises goes through it so that "the whole surface reports a refusal in one form" and explicitly names "a pagination token it did not issue" as one of the self-enforced rules that must read like a declared-constraint refusal. ListHarvestJobs instead answers `"The pagination token is not one this service issued"`, so a client paging harvest jobs sees a body shaped unlike the one it sees paging groups, channels or endpoints, with no `request.nextToken` member path to key on.

Fix: call the same helper the three sibling lists call, so the four bodies cannot drift.

## Evidence

The three siblings — src/ops/list_channel_groups.rs:193-200:
```
                    return Err(ListChannelGroupsErrorModeled::ValidationException(
                        crate::state::validation_failed(
                            "nextToken",
                            crate::state::INVALID_PAGINATION_TOKEN_DETAIL,
                            "INVALID_PAGINATION_TOKEN",
                        ),
```
(identical at src/ops/list_channels.rs:213-219 and src/ops/list_origin_endpoints.rs:261-267).

The outlier — src/ops/list_harvest_jobs.rs:245-257:
```
                    return Err(ListHarvestJobsErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "The pagination token is not one this service issued"
                                    .to_string(),
                            ),
                            validation_exception_type: Some(
                                crate::state::VALIDATION_INVALID_PAGINATION_TOKEN.to_string(),
                            ),
                        },
```

The rule it breaks — src/state.rs:2684-2688:
```
/// Why a pagination token this service did not issue is refused — the detail
/// [`validation_message`] states after the member path. The same sentence whichever list
/// is being paged, and about the token rather than about the caller's data.
pub const INVALID_PAGINATION_TOKEN_DETAIL: &str = "You've provided an invalid pagination \
     token in your request. Check your token and then try again";
```
and src/state.rs:2212-2219:
```
/// The modeled 400 for a member this service refuses, in the envelope every
/// mediapackagev2 validation message uses ([`validation_message`]) ...
/// A rule the service enforces itself (a policy document it cannot parse, a pagination
/// token it did not issue) is therefore stated exactly like a constraint the model
/// declares, so a caller reads one form for both.
pub fn validation_failed(
```

Test coverage: tests/all/control_plane_validation.rs:22-24 states the rule for "every paged list" ("A pagination token the service did not issue is a 400 `ValidationException` / `INVALID_PAGINATION_TOKEN` on every paged list") but the test at line 640 only drives `ListChannelGroups` and `ListOriginEndpoints` (lines 656-670), so ListHarvestJobs' body is never asserted; `assert_validation` (line 64, line 80) would fail it, since it requires `message(body).starts_with("Validation error encountered at request")`.

## Steps to Reproduce

1. Create a channel group `grp`.
2. GET `/channelGroup/grp/harvestJob?nextToken=zzzz` (a token this service did not issue).
3. Observe the 400 body: `ValidationExceptionType` is `INVALID_PAGINATION_TOKEN` (correct) but `Message` is `"The pagination token is not one this service issued"`.
4. GET `/channelGroup?nextToken=zzzz` and `/channelGroup/grp/channel/chan/originEndpoint?nextToken=zzzz` and observe `"Validation error encountered at request.nextToken: You've provided an invalid pagination token in your request. Check your token and then try again"` — the same class refused in two different forms by one service.
