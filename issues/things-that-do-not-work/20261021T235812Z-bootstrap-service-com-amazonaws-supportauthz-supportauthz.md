Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: b135c4508c1fb26e9a8e4477f81bd51ec13345b2

# Bootstrap the SupportAuthZ emulator

_Filed: 2026-10-21T23:58:12Z_
_Operation: service-com-amazonaws-supportauthz-supportauthz | Class: missing_

> _Note: this issue was identified against the code base at commit `b135c4508c1fb26e9a8e4477f81bd51ec13345b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Nothing in this crate holds state: the six wired handlers under `src/ops/` (GetAction, ListActions, ListTagsForResource, RejectSupportPermitRequest, TagResource, UntagResource) all carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501, and the five operations of the service's PRIMARY resource (CreateSupportPermit, GetSupportPermit, ListSupportPermits, DeleteSupportPermit, ListSupportPermitRequests) have no handler file at all — `src/ops/SKIPPED_STUBS.md` records them as declined by the stub generator ("shape missing from model: smithy.api#Unit"), so they are answered by the framework's unroutable-operation path.

First slice = the support-permit lifecycle, implemented statefully and end to end. Implement exactly these FIVE operations (hand-author the four skipped ones in `src/ops/<op>.rs`, patterning them on the rendered `get_action.rs` sibling; protocol is `restJson1`):

1. **CreateSupportPermit** — `POST /support-permits`. Input: `name` (required, `^[a-zA-Z0-9]{1,256}$`), `permit` (required: Permit{actions, resources, conditions}), `signingKeyInfo` (required union, `kmsKey`), `description`, `supportCaseDisplayId`, `clientToken` (@idempotencyToken), `tags`. Must MINT and store an `arn` matching `^arn:[a-z0-9-]+:[a-z0-9-]+:[a-z0-9-]*:[0-9]{12}:.+$` (e.g. `arn:aws:supportauthz:<region>:<account>:support-permit/<name>`), set `status = ACTIVE`, set `createdAt` to the current time, and echo back the stored record — `name`, `arn`, `permit`, `status`, `signingKeyInfo`, `createdAt`, `description`, `supportCaseDisplayId`, `tags`. A second create with the same `name` (or a different body under the same `clientToken`) must be refused with `ConflictException` (409); a repeat with the SAME `clientToken` and the same body must return the original record rather than creating a second one.
2. **GetSupportPermit** — `GET /support-permits/{supportPermitIdentifier}`. Must look the permit up by ARN (and by name, since the label pattern `^[a-zA-Z0-9:/-]{1,512}$` admits a bare name) and return exactly the stored fields, including `tags`. Unknown identifier → `ResourceNotFoundException` (404), never a hollow 200.
3. **ListSupportPermits** — `GET /support-permits`. Must return one `SupportPermitSummary` per stored permit (`name`, `arn`, `permit`, `status`, `signingKeyInfo`, `createdAt`, `supportCaseDisplayId`), filtered by the `supportPermitStatuses` query list when present, honouring `maxResults` (1..100) with a real opaque `nextToken` (`^[a-zA-Z0-9/+=%_-]+$`) that resumes where the previous page stopped. Empty store → empty `supportPermits` list, no token.
4. **DeleteSupportPermit** — `DELETE /support-permits/{supportPermitIdentifier}` (@idempotent). Must remove the permit from the store and return its final record with `status` reflecting deletion (`DELETING`/`INACTIVE`). A subsequent GetSupportPermit must return 404, and the permit must disappear from ListSupportPermits. Deleting an unknown ARN → `ResourceNotFoundException` (404).
5. **TagResource / ListTagsForResource** (count as one tagging step) — `POST /tags/{resourceArn}` → 204 must merge `tags` onto the permit identified by `resourceArn`; `GET /tags/{resourceArn}` must return exactly the tags stored by CreateSupportPermit plus those merged by TagResource. Unknown `resourceArn` → `ResourceNotFoundException` (404). (`UntagResource` may follow in the same slice if cheap: remove `tagKeys` and nothing else.)

Every one of these must compute its response from the persisted store — NO canned bodies, NO echoing the input without storing it, NO random ARN that is never written down. A create followed by a get/list must return what was created; a delete must be observable by the next reader.

## State Model

In `src/state.rs`, on `ComAmazonawsSupportauthzSupportauthzState` (reached via `crate::state::service_state(&*self.engine)`, mutated through the lock guard):

- `support_permits: BTreeMap<String /* arn */, SupportPermitRecord>` — insertion/creation order preserved for stable list paging; plus `permit_names: BTreeMap<String /* name */, String /* arn */>` so lookup-by-name and the create-conflict check are O(log n).
- `SupportPermitRecord { name: String, arn: String, description: Option<String>, permit: Permit /* actions, resources, conditions */, status: SupportPermitStatus /* ACTIVE | DELETING | INACTIVE */, signing_key_info: SigningKeyInfo /* kmsKey */, created_at: DateTime, support_case_display_id: Option<String>, tags: BTreeMap<String, String> }`.
- `idempotency: HashMap<String /* clientToken */, String /* arn */>` for CreateSupportPermit's `@idempotencyToken` replay.
- `list_tokens: HashMap<String /* nextToken */, usize /* offset */>` (or an opaque base64 of the last-seen arn) shared by the paginated list operations.

All other operations (GetAction, ListActions, ListSupportPermitRequests, RejectSupportPermitRequest, UntagResource if not done above) stay at 501 and will be filed as individual gap issues later.

Done = a client script can create a permit, read it back, see it in a list, tag it, read the tags, delete it, and observe the 404 — with correct bodies, not just 2xx.

## Evidence

`services/service-com-amazonaws-supportauthz-supportauthz/OPERATIONS.md` lists all 11 operations with routes, members and errors: CreateSupportPermit `POST /support-permits` (required `permit`, `name`, `signingKeyInfo`; output required `name`/`arn`/`permit`/`status`/`signingKeyInfo`/`createdAt`), GetSupportPermit `GET /support-permits/{supportPermitIdentifier}`, ListSupportPermits `GET /support-permits` (@paginated inputToken=nextToken, items=supportPermits, maxResults 1..100), DeleteSupportPermit `DELETE /support-permits/{supportPermitIdentifier}` (@idempotent), TagResource `POST /tags/{resourceArn}` → 204, ListTagsForResource `GET /tags/{resourceArn}`. Errors are modeled: ConflictException 409 on CreateSupportPermit, ResourceNotFoundException 404 on Get/Delete/tagging. Shapes from the Smithy model: `Permit{actions, resources, conditions(max 2)}`, `SigningKeyInfo` union with `kmsKey`, `SupportPermitSummary{name, arn, permit, status, signingKeyInfo, createdAt, supportCaseDisplayId}`, `SupportPermitStatus` ∈ {ACTIVE, DELETING, INACTIVE}. Proof of all-stubs: `src/ops/get_action.rs:4` has `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 167 returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetAction"))` — the same for the other five wired files; `src/ops/SKIPPED_STUBS.md:21-25` shows CreateSupportPermit, DeleteSupportPermit, GetSupportPermit, ListSupportPermits and ListSupportPermitRequests have no handler file at all. No tests assert any of this behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. POST /support-permits with {"name":"DemoPermit","permit":{"actions":["ec2:DescribeInstances"],"resources":["*"]},"signingKeyInfo":{"kmsKey":"arn:aws:kms:us-east-1:123456789012:key/abc"},"supportCaseDisplayId":"case-1234","tags":{"team":"support"}} — expect 200 with a minted `arn`, `status=ACTIVE`, a real `createdAt`, and the submitted permit/tags echoed from the store; actual: unroutable/501 (no handler file).
2. GET /support-permits/<arn returned above> — expect 200 with the identical stored record including `tags`; actual: unroutable/501.
3. GET /support-permits?maxResults=1 — expect 200 with one SupportPermitSummary for DemoPermit (and a `nextToken` only if more permits exist); actual: unroutable/501.
4. POST /tags/<arn> with {"tags":{"env":"test"}} then GET /tags/<arn> — expect 204 then 200 with {"team":"support","env":"test"}; actual: both 501.
5. DELETE /support-permits/<arn> — expect 200 with the final record (status DELETING/INACTIVE); then GET /support-permits/<arn> must return 404 ResourceNotFoundException and GET /support-permits must no longer list it; actual: unroutable/501 and no state is ever retained.
6. POST /support-permits again with name "DemoPermit" while the first still exists — expect 409 ConflictException; actual: unroutable/501.
