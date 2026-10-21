Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Lex Models V2 (lexmodelsv2) emulator

_Filed: 2026-10-21T21:08:31Z_
_Operation: service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate is a fresh scaffold: every one of the 107 operations under `src/ops/` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and `src/state.rs` declares `ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State` with no fields at all (`// TODO: add service-specific state fields`). Nothing is persisted, so no client workflow can run.

Implement a FIRST SMALL SLICE: the bot lifecycle of the primary resource (Bot), statefully, so a realistic client script works end to end. Protocol is `restJson1` (sigv4 signing name `lex`, endpoint prefix `models-v2-lex`).

Happy path, in order (fields from OPERATIONS.md):

1. **CreateBot** — `PUT /bots -> 202`. Input: `botName` (required, len 1..100, pattern `^([0-9a-zA-Z][_-]?){1,100}$`), `roleArn` (required, pattern `^arn:aws:iam::[0-9]{12}:role/.*$`), `dataPrivacy` (required), `idleSessionTTLInSeconds` (required, 60..86400), plus `description`, `botTags`, `testBotAliasTags`, `botType` (Bot|BotNetwork), `botMembers`, `errorLogSettings`. Must MINT and store a new `botId` matching the modeled shape (len exactly 10, `^[0-9a-zA-Z]+$`), record `creationDateTime`, and return the stored record with `botStatus` (e.g. `Available`, or `Creating` then observable as `Available`). The id must be stored — a random id with no backing record is a failure.
2. **DescribeBot** — `GET /bots/{botId} -> 200`. Must read the persisted record and echo back exactly what CreateBot stored: `botId`, `botName`, `description`, `roleArn`, `dataPrivacy`, `idleSessionTTLInSeconds`, `botStatus`, `creationDateTime`, `lastUpdatedDateTime`, `botType`, `botMembers`, `errorLogSettings`. Unknown `botId` -> `ResourceNotFoundException` (HTTP 404).
3. **ListBots** — `POST /bots -> 200`. Returns `botSummaries` built from the stored map (one entry per created bot, with `botId`/`botName`/`botStatus`/`description`/`lastUpdatedDateTime`), honouring `maxResults` (1..1000) and round-tripping `nextToken` for the remainder. Never a canned empty list once a bot exists.
4. **UpdateBot** — `PUT /bots/{botId} -> 202`. Mutates the stored record (`botName`, `description`, `roleArn`, `dataPrivacy`, `idleSessionTTLInSeconds`, `botType`, `botMembers`, `errorLogSettings`), sets `lastUpdatedDateTime`, and returns the NEW stored values; a following DescribeBot must show the update. Unknown `botId` -> 404-class error.
5. **DeleteBot** — `DELETE /bots/{botId} -> 202`. Removes the record from state and returns `botId` + `botStatus` (`Deleting`). A subsequent DescribeBot must fail with `ResourceNotFoundException`, and ListBots must no longer include it. Unknown `botId` -> `ResourceNotFoundException`.

Optional sixth step if cheap: **CreateBotLocale** (`PUT /bots/{botId}/botversions/{botVersion}/botlocales -> 202`, requires `localeId` and `nluIntentConfidenceThreshold` 0..1) stored under the bot and readable via **DescribeBotLocale** — include it only if the five above are done.

Every one of these must compute its response from persisted state. No canned responses, no echoing the input without storing it, no random ids without a record.

## State Model

In `ComAmazonawsLexmodelsv2Lexmodelbuildingservicev2State`:

- `bots: HashMap<String /* botId (10 chars, ^[0-9a-zA-Z]+$) */, Bot>`
- `Bot { bot_id: String, bot_name: String, description: Option<String>, role_arn: String, data_privacy: DataPrivacy, idle_session_ttl_in_seconds: i32, bot_status: BotStatus, bot_type: Option<BotType>, bot_members: Vec<BotMember>, error_log_settings: Option<ErrorLogSettings>, creation_date_time: DateTime, last_updated_date_time: DateTime, bot_tags: HashMap<String,String>, test_bot_alias_tags: HashMap<String,String>, locales: HashMap<String /* "{botVersion}:{localeId}" */, BotLocale> }`
- `BotLocale { bot_id, bot_version, locale_id, description, nlu_intent_confidence_threshold, bot_locale_status, creation_date_time }` (only if the optional step is taken)
- a monotonic id source for minting 10-character bot ids (e.g. a counter rendered into the `[0-9a-zA-Z]{10}` space), so ids are unique and collision-checked against `bots`.

All of it lives in the single shared engine store reached via `crate::state::service_state(&*self.engine)` — never a freshly constructed engine inside a handler.

Operations outside this slice (aliases, versions, intents, slots, slot types, exports/imports, analytics, test sets, replicas, ...) stay at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) lines 8-14 (restJson1, SDK id `Lex Models V2`, sigv4 `lex`), 54-59 (CreateBot: `PUT /bots -> 202`, required botName/roleArn/dataPrivacy/idleSessionTTLInSeconds, output botId/botStatus/creationDateTime), 145-150 (DeleteBot `DELETE /bots/{botId} -> 202`, output botId/botStatus), 258-263 (DescribeBot `GET /bots/{botId} -> 200`, declares `ResourceNotFoundException (HTTP 404)`), 469-475 (ListBots `POST /bots -> 200`, @paginated with nextToken/maxResults, output botSummaries), 738-743 (UpdateBot `PUT /bots/{botId} -> 202`, output includes lastUpdatedDateTime), 68-73 (CreateBotLocale). Scaffold proof: `src/ops/create_bot.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and `src/state.rs` lines 6-9 define the state struct with only a `// TODO: add service-specific state fields` comment. No tests assert any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `lexv2-models` client at it.
2. `CreateBot` with botName=DemoBot, roleArn=arn:aws:iam::123456789012:role/lex-demo, dataPrivacy={childDirected:false}, idleSessionTTLInSeconds=300 — expect 202 with a 10-character botId and a botStatus; today: HTTP 501 NotImplemented.
3. `DescribeBot` with that botId — expect 200 echoing the stored botName/roleArn/dataPrivacy/idleSessionTTLInSeconds/creationDateTime.
4. `ListBots` — expect botSummaries to contain exactly the created bot; repeat with maxResults=1 and follow nextToken to confirm pagination round-trips.
5. `UpdateBot` on the botId with description='updated' and idleSessionTTLInSeconds=600 — expect 202, then `DescribeBot` must return the new description/TTL and a bumped lastUpdatedDateTime.
6. `DeleteBot` with that botId — expect 202 with botStatus=Deleting; then `DescribeBot` on the same id must fail with ResourceNotFoundException (HTTP 404) and `ListBots` must no longer list it.
7. `DescribeBot` on a never-created id (e.g. 0000000000) — expect ResourceNotFoundException (HTTP 404), not 501 and not a hollow 200.
