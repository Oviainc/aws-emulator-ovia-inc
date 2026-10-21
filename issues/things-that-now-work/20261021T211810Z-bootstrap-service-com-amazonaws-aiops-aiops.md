Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the aiops (AIOps / CloudWatch investigations) emulator

_Filed: 2026-10-21T21:18:10Z_
_Operation: service-com-amazonaws-aiops-aiops | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 11 operations in `services/service-com-amazonaws-aiops-aiops/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns HTTP 501 `NotImplemented`, holding zero state. Implement a FIRST stateful slice around the service's primary resource, the investigation group (restJson1, sigv4 name `aiops`).

## Happy path to implement (in order)

1. **CreateInvestigationGroup** — `POST /investigationGroups` -> 201. Input: `name` (required, len 1..512, pattern `^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$`), `roleArn` (required, len 20..2048, `^arn:.*$`), optional `encryptionConfiguration`, `retentionInDays` (7..90), `tags`, `tagKeyBoundaries`, `chatbotNotificationChannel`, `isCloudTrailEventHistoryEnabled`, `crossAccountConfigurations` (0..25). Output: `arn` only, and it MUST match the modeled pattern `^arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:<region>:<12-digit-account>:investigation-group/[A-Za-z0-9]{16}$` — i.e. generate a fresh 16-char alphanumeric id, store the whole group under it, and return the ARN built from the request's region/account.
2. **GetInvestigationGroup** — `GET /investigationGroups/{identifier}` -> 200. `identifier` accepts EITHER the bare name/id (`[\-_A-Za-z0-9]{1,512}`) or the full ARN, so resolution must accept both forms. Output must be read back from stored state: `arn`, `name`, `roleArn`, `encryptionConfiguration`, `retentionInDays`, `chatbotNotificationChannel`, `tagKeyBoundaries`, `isCloudTrailEventHistoryEnabled`, `crossAccountConfigurations`, plus `createdBy`/`createdAt`/`lastModifiedBy`/`lastModifiedAt` (epoch-seconds longs, `createdAt` set at create time, `lastModifiedAt` bumped by updates). Unknown identifier -> `ResourceNotFoundException` (404), never 501 and never a hollow 200.
3. **ListInvestigationGroups** — `GET /investigationGroups` -> 200. Returns `investigationGroups` (list of `ListInvestigationGroupsModel`) built from the store — only groups actually created, and gone after deletion. Honour `maxResults` and the opaque `nextToken` (len 0..2048) per the `@paginated(inputToken=nextToken, outputToken=nextToken, items=investigationGroups)` trait: truncate the page, emit `nextToken` only when more remain, and omit it on the last page.
4. **UpdateInvestigationGroup** — `PATCH /investigationGroups/{identifier}` -> 200, empty body. Mutates the stored group's `roleArn`, `encryptionConfiguration`, `tagKeyBoundaries`, `chatbotNotificationChannel`, `isCloudTrailEventHistoryEnabled`, `crossAccountConfigurations` (absent members left unchanged) and refreshes `lastModifiedAt`/`lastModifiedBy`. A following GetInvestigationGroup must show the new values. Unknown identifier -> 404.
5. **DeleteInvestigationGroup** — `DELETE /investigationGroups/{identifier}` -> 200, empty body. Removes the group (and its policy/tags) from the store; a following Get -> `ResourceNotFoundException` (404) and the group disappears from List. Deleting an unknown identifier -> 404.
6. **PutInvestigationGroupPolicy / GetInvestigationGroupPolicy** — `POST|GET /investigationGroups/{identifier}/policy` -> 200. Put stores `policy` (len 1..32768) on the resolved group and returns `investigationGroupArn`; Get returns the exact `policy` string previously stored plus `investigationGroupArn`. Get on a group with no policy, or on an unknown group, -> `ResourceNotFoundException` (404).

All six must compute their responses from persisted state — no canned bodies, no echoing the input back, no random ARN that is never stored. Required-member and constraint violations (missing `name`/`roleArn`, `retentionInDays` outside 7..90, name/ARN pattern mismatch) must surface as `ValidationException` (400).

## State Model

In `src/state.rs` (`ComAmazonawsAiopsAiopsState`, reached via `crate::state::service_state(&*self.engine)`):

- `investigation_groups: BTreeMap<String /* 16-char group id */, InvestigationGroup>` — BTreeMap so List has a stable, deterministic order for pagination tokens.
- `name_index: HashMap<String /* name */, String /* group id */>` — lets `{identifier}` resolve by name as well as by id/ARN, and lets create detect a duplicate name (`ConflictException`, 409).
- `InvestigationGroup { id: String, arn: String, name: String, role_arn: String, encryption_configuration: Option<EncryptionConfiguration>, retention_in_days: Option<i64>, tag_key_boundaries: Vec<String>, chatbot_notification_channel: BTreeMap<String, Vec<String>>, is_cloud_trail_event_history_enabled: Option<bool>, cross_account_configurations: Vec<CrossAccountConfiguration>, policy: Option<String>, tags: BTreeMap<String, String>, created_by: String, created_at: i64, last_modified_by: String, last_modified_at: i64 }`.

Tags live on the group record so the later TagResource/UntagResource/ListTagsForResource slice can key off `resourceArn` without a schema change.

The remaining operations (`TagResource`, `UntagResource`, `ListTagsForResource`, `DeleteInvestigationGroupPolicy`) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-aiops-aiops/OPERATIONS.md` lists all 11 operations with their routes, input/output members and errors (CreateInvestigationGroup lines 49-55, GetInvestigationGroup 57-63, UpdateInvestigationGroup 65-71, DeleteInvestigationGroup 73-79, ListInvestigationGroups 81-87 incl. the `@paginated` trait, Put/GetInvestigationGroupPolicy 89-103) and states the wire protocol is restJson1 with sigv4 name `aiops`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-aiops-aiops/src/ops/` matches all 11 handler files (create_investigation_group.rs, get_investigation_group.rs, update_investigation_group.rs, delete_investigation_group.rs, list_investigation_groups.rs, put_investigation_group_policy.rs, get_investigation_group_policy.rs, delete_investigation_group_policy.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. The ARN shape and the 16-char group-id segment are taken from the modeled output pattern on `CreateInvestigationGroup.arn`; ResourceNotFoundException/ValidationException/ConflictException are modeled errors on these operations.

## Steps to Reproduce

1. Call CreateInvestigationGroup with name="my-group" and roleArn="arn:aws:iam::123456789012:role/AIOpsRole", retentionInDays=30 — expect 201 and an `arn` ending in `investigation-group/<16 alphanumerics>` (today: 501).
2. Call GetInvestigationGroup with identifier="my-group" (the bare name) — expect 200 with name/roleArn/retentionInDays exactly as created plus createdAt/lastModifiedAt timestamps.
3. Call GetInvestigationGroup again with the full ARN returned by step 1 — expect the identical body.
4. Call ListInvestigationGroups with maxResults=1 after creating two groups — expect one item plus a nextToken, and the second page returns the other item with no nextToken.
5. Call UpdateInvestigationGroup on the ARN with isCloudTrailEventHistoryEnabled=true, then GetInvestigationGroup — expect the new flag and a bumped lastModifiedAt, with name/roleArn untouched.
6. Call PutInvestigationGroupPolicy with a policy document, then GetInvestigationGroupPolicy — expect the byte-identical policy string and the group's ARN.
7. Call DeleteInvestigationGroup on the ARN, then GetInvestigationGroup — expect ResourceNotFoundException (HTTP 404) and the group absent from ListInvestigationGroups.
8. Call GetInvestigationGroup with identifier="never-created" — expect ResourceNotFoundException (404), and CreateInvestigationGroup with retentionInDays=1 — expect ValidationException (400).
