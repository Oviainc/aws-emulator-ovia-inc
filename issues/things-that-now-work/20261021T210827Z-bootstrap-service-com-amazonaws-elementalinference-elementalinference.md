Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the ElementalInference emulator

_Filed: 2026-10-21T21:08:27Z_
_Operation: service-com-amazonaws-elementalinference-elementalinference | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the feed lifecycle, which is the service's primary resource (`restJson1`, sigv4 signing name `elemental-inference`).

Happy path to implement, in order:

1. **CreateFeed** — `POST /v1/feed -> 202`. Input: `name` (required, pattern `^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$`), `outputs` (required, list<CreateOutput>{name, outputConfig, status, description}), `accessRoleArn` (optional, pattern `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/.+$`), `tags`. Must MINT and persist a new feed: `id` matching `^[a-z0-9]{19}$`, `arn` matching the modeled feed ARN form (`arn:aws:elemental-inference:<region>:<account>:feed/<id>`), `dataEndpoints` (required list), `outputs` returned as list<GetOutput> (same name/outputConfig/status/description plus `fromAssociation=false` since they came from CreateFeed), `status` from the `FeedStatus` enum, and the `tags` as supplied. Response must be built from the stored record, not echoed from the request alone.
2. **GetFeed** — `GET /v1/feed/{id} -> 200`. Must return exactly the stored feed created in step 1: `arn`, `name`, `id`, `dataEndpoints`, `outputs` (list<GetOutput>), `accessRoleArn`, `status`, `association`, `tags`. Unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **ListFeeds** — `GET /v1/feeds -> 200`. Must enumerate the persisted feeds as `feeds` (list<FeedSummary>{arn, id, name, association, status}) — never an empty or canned list after a create. Honour `maxResults`/`nextToken` (`@paginated(inputToken=nextToken, outputToken=nextToken, items=feeds)`): return at most `maxResults` items plus an opaque `nextToken` when more remain, and resume from that token on the next call.
4. **UpdateFeed** — `PUT /v1/feed/{id} -> 200`. Input: `id` (@httpLabel), `name` (required), `outputs` (required list<UpdateOutput>), `accessRoleArn`. Must MUTATE the stored feed so a subsequent GetFeed/ListFeeds shows the new name and outputs. Unknown id -> `ResourceNotFoundException` (404).
5. **DeleteFeed** — `DELETE /v1/feed/{id} -> 202`. Output: `arn`, `id`, `status` (a terminal `FeedStatus`, e.g. DELETING/DELETED). Must remove the feed from state so GetFeed afterwards is a 404 and ListFeeds no longer lists it. Deleting an unknown id -> `ResourceNotFoundException` (404).

NEVER STUBS: each of these five must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)` — no hardcoded bodies, no echoing input without storing it, no minting an id that is never persisted. Required output members must be populated with real values (an id/arn that satisfies the modeled pattern).

## State Model

In `src/state.rs`, on the service state struct:

- `feeds: BTreeMap<String /* feed id, ^[a-z0-9]{19}$ */, Feed>`
- `struct Feed { id: String, name: String, arn: String, data_endpoints: Vec<String>, outputs: Vec<Output>, access_role_arn: Option<String>, status: FeedStatus, association: Option<FeedAssociation>, tags: BTreeMap<String, String>, created_at: SystemTime }`
- `struct Output { name: String, output_config: OutputConfig, status: OutputStatus, description: Option<String>, from_association: bool }`
- `next_feed_seq: u64` (or any deterministic generator) used to mint 19-char lowercase-alphanumeric feed ids, so ids are unique and stable within a run.
- Pagination: `nextToken` encodes the last-returned feed id (e.g. base64 of the key) and ListFeeds resumes from the next key in the BTreeMap, giving stable ordering.

Out of scope for this first slice (stay 501 and get filed as individual gap issues later): dictionaries (CreateDictionary, GetDictionary, UpdateDictionary, DeleteDictionary, ListDictionaries, ExportDictionaryEntries), feed policies (PutFeedPolicy, GetFeedPolicy, DeleteFeedPolicy), association (AssociateFeed, DisassociateFeed), tagging (TagResource, UntagResource, ListTagsForResource), and fixtures (GetFixture, SearchFixtures).

Done = a client script that creates a feed, gets it back, lists it, updates it, deletes it and confirms it is gone passes end-to-end with correct bodies, not just 2xx statuses.

## Evidence

`services/service-com-amazonaws-elementalinference-elementalinference/OPERATIONS.md` documents all 21 operations, their routes/success codes and members: CreateFeed (`POST /v1/feed -> 202`, lines 112-118), GetFeed (`GET /v1/feed/{id} -> 200`, lines 120-126), UpdateFeed (`PUT /v1/feed/{id} -> 200`, lines 128-134), DeleteFeed (`DELETE /v1/feed/{id} -> 202`, lines 136-142), ListFeeds (`GET /v1/feeds -> 200`, @paginated, lines 144-150). `smithy_query` on CreateOutput/GetOutput/UpdateOutput/FeedSummary gives the output member sets quoted above. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` in `src/ops/` matches all 21 handler files (create_feed.rs, get_feed.rs, list_feeds.rs, update_feed.rs, delete_feed.rs among them), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. No handler touches state; no tests assert any feed behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Call CreateFeed with name="my-feed" and one output {name:"out1", outputConfig:<any modeled config>, status:<OutputStatus>} — expect HTTP 202 with a minted 19-char `id`, a feed `arn`, `dataEndpoints`, and `outputs` echoing the created output with fromAssociation=false; actual: HTTP 501 NotImplemented.
2. Call GetFeed with the returned id — expect HTTP 200 returning exactly the created feed (name, outputs, status, tags); actual: HTTP 501.
3. Call ListFeeds — expect HTTP 200 with `feeds` containing a FeedSummary for the created feed (arn, id, name, status); actual: HTTP 501.
4. Call ListFeeds with maxResults=1 after creating two feeds — expect one feed plus a `nextToken`, and the second feed on the follow-up call with that token.
5. Call UpdateFeed on the id with name="my-feed-2" and a changed outputs list — expect HTTP 200, then GetFeed must report the new name and outputs; actual: HTTP 501.
6. Call DeleteFeed on the id — expect HTTP 202 with arn/id/status, then GetFeed on that id must return ResourceNotFoundException (HTTP 404) and ListFeeds must no longer include it; actual: HTTP 501.
7. Call GetFeed with an id that was never created — expect ResourceNotFoundException (HTTP 404), not 501.
