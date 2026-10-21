Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the rbin (Amazon Recycle Bin) emulator

_Filed: 2026-10-21T21:28:08Z_
_Operation: service-com-amazonaws-rbin-amazonrecyclebin | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 10 operations in `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service.

Implement a FIRST SMALL SLICE: the retention-rule lifecycle, statefully, so a realistic client script works end to end. Scope = exactly these five operations (everything else stays 501 and will be filed as individual gap issues later):

1. **CreateRule** (`POST /rules` -> 201). Input: `RetentionPeriod` (required, struct of `RetentionPeriodValue` 1..3650 + `RetentionPeriodUnit` = `DAYS`), `ResourceType` (required, one of `EBS_SNAPSHOT|EBS_VOLUME|EC2_IMAGE`), optional `Description`, `Tags` (list of `Tag{Key,Value}`), `ResourceTags`/`ExcludeResourceTags` (list of `ResourceTag{ResourceTagKey,ResourceTagValue}`), `LockConfiguration`. Must MINT and PERSIST a new rule: generate an `Identifier` matching `^[0-9a-zA-Z]{11}$`, build `RuleArn` as `arn:aws:rbin:<region>:<account>:rule/<Identifier>`, set `Status` = `available`, set `LockState` = `locked` when `LockConfiguration` was supplied (otherwise leave it unset / never-locked), and echo back the stored `RetentionPeriod`, `Description`, `Tags`, `ResourceType`, `ResourceTags`, `ExcludeResourceTags` from what was stored — not from the request object.
2. **GetRule** (`GET /rules/{Identifier}` -> 200). Look the rule up by `Identifier` in the store and return the PERSISTED `Identifier`, `Description`, `ResourceType`, `RetentionPeriod`, `ResourceTags`, `ExcludeResourceTags`, `Status`, `LockConfiguration`, `LockState`, `LockEndTime`, `RuleArn`. Unknown identifier -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListRules** (`POST /list-rules` -> 200). Input: `ResourceType` (required), optional `ResourceTags`, `ExcludeResourceTags`, `LockState`, `MaxResults` (1..1000), `NextToken`. Return one `RuleSummary{Identifier, Description, RetentionPeriod, LockState, RuleArn}` per stored rule, FILTERED by `ResourceType` (and by `LockState` / `ResourceTags` when given). Honour `MaxResults` by truncating and emitting an opaque `NextToken` matching `^[A-Za-z0-9+/=]{1,2048}$` that resumes where the page stopped; omit `NextToken` on the last page. A rule created in step 1 MUST appear here.
4. **UpdateRule** (`PATCH /rules/{Identifier}` -> 200). Apply the supplied `RetentionPeriod`, `Description`, `ResourceTags`, `ExcludeResourceTags` to the STORED rule (fields absent from the request are left unchanged) and return the merged, persisted rule. A following GetRule must observe the update. Unknown identifier -> `ResourceNotFoundException` (404).
5. **DeleteRule** (`DELETE /rules/{Identifier}` -> 204). Remove the rule from the store. Afterwards GetRule must return `ResourceNotFoundException` (404) and ListRules must not include it. Unknown identifier -> 404.

Rules for all five: no canned, hardcoded or request-echoed responses, and no "generate an ID and forget it". Every response field must be read back out of the shared store reached via `crate::state::service_state(&*self.engine)`, so state survives across requests. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each implemented handler.

## State Model

In `src/state.rs` (`ComAmazonawsRbinAmazonrecyclebinState`):

- `rules: BTreeMap<String /* Identifier, 11 alnum chars */, Rule>` — ordered so ListRules paging is stable.
- `arn_index: HashMap<String /* RuleArn */, String /* Identifier */>` — so the ARN-addressed tag operations (later slice) and ARN lookups resolve without scanning.
- `struct Rule {`
  - `identifier: String`
  - `rule_arn: String` (`arn:aws:rbin:{region}:{account}:rule/{identifier}`)
  - `description: Option<String>`
  - `resource_type: ResourceType` (`EBS_SNAPSHOT|EBS_VOLUME|EC2_IMAGE`)
  - `retention_period: RetentionPeriod { value: i32 /* 1..3650 */, unit: RetentionPeriodUnit /* DAYS */ }`
  - `resource_tags: Vec<ResourceTag { key: String, value: Option<String> }>` (0..50)
  - `exclude_resource_tags: Vec<ResourceTag>` (0..5)
  - `tags: Vec<Tag { key: String, value: String }>` (0..200, the rule's own AWS tags)
  - `status: RuleStatus` (`available` | `pending`)
  - `lock_configuration: Option<LockConfiguration { unlock_delay: UnlockDelay }>`
  - `lock_state: Option<LockState>` (`locked` | `pending_unlock` | `unlocked`; `None` = never locked)
  - `lock_end_time: Option<SystemTime>`
  - `created_at: SystemTime`
- `next_identifier_seed: u64` — monotonic counter used to mint unique 11-character alphanumeric identifiers.

ListRules pagination: encode the `NextToken` as the base64 of the last returned `Identifier` and resume from the next key in the `BTreeMap`.

## Evidence

`services/service-com-amazonaws-rbin-amazonrecyclebin/OPERATIONS.md` lists all 10 operations with routes, input/output members and errors — CreateRule `POST /rules -> 201` with required `RetentionPeriod`+`ResourceType` and output `Identifier`/`RuleArn`/`Status`/`LockState`; GetRule `GET /rules/{Identifier} -> 200` with `ResourceNotFoundException (HTTP 404)`; ListRules `POST /list-rules -> 200` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=Rules)`; UpdateRule `PATCH /rules/{Identifier} -> 200`; DeleteRule `DELETE /rules/{Identifier} -> 204`. Shape details for `RetentionPeriod` (`RetentionPeriodValue` range 1..3650, `RetentionPeriodUnit` required), `RuleSummary` (`Identifier, Description, RetentionPeriod, LockState, RuleArn`), `ResourceTag`, `Tag` and `LockConfiguration{UnlockDelay}` come from the Smithy model at `services/service-com-amazonaws-rbin-amazonrecyclebin/smithy-model/com-amazonaws-rbin-amazonrecyclebin.json`. All 10 handler files under `src/ops/` still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501): create_rule.rs, get_rule.rs, list_rules.rs, update_rule.rs, delete_rule.rs, lock_rule.rs, unlock_rule.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `rbin` client at it.
2. `CreateRule` with `ResourceType=EBS_SNAPSHOT`, `RetentionPeriod={RetentionPeriodValue:7, RetentionPeriodUnit:DAYS}`, `Description='bootstrap rule'`, `Tags=[{Key:env,Value:dev}]` -> expect 201 with a fresh 11-character `Identifier`, a well-formed `RuleArn`, `Status=available`; actual: HTTP 501 NotImplemented.
3. `GetRule` with the returned `Identifier` -> expect 200 echoing the persisted `RetentionPeriod` (7/DAYS), `Description`, `ResourceType`, `RuleArn`; actual: HTTP 501.
4. `ListRules` with `ResourceType=EBS_SNAPSHOT` -> expect 200 whose `Rules` contains a `RuleSummary` for that identifier; also call with `MaxResults=1` after creating two rules and expect a `NextToken` that returns the remaining rule on the follow-up call; actual: HTTP 501.
5. `ListRules` with `ResourceType=EC2_IMAGE` -> expect 200 with an empty `Rules` list (filtering works).
6. `UpdateRule` on the identifier with `RetentionPeriod={RetentionPeriodValue:30, RetentionPeriodUnit:DAYS}` -> expect 200, then `GetRule` must report 30 days while `Description` is unchanged; actual: HTTP 501.
7. `DeleteRule` on the identifier -> expect 204, then `GetRule` on the same identifier must fail with `ResourceNotFoundException` (HTTP 404) and `ListRules` must no longer list it; actual: HTTP 501.
8. `GetRule` with a never-created identifier such as `aaaaaaaaaaa` -> expect `ResourceNotFoundException` (HTTP 404).
