Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the Lex Model Building Service emulator

_Filed: 2026-10-21T23:27:56Z_
_Operation: service-com-amazonaws-lexmodelbuildingservice-awsdeepsensemodelbuildingservice | Class: stub_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 42 handlers under `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST small, stateful slice covering the core bot-authoring loop (restJson1, sigv4 name `lex`, endpoint prefix `models.lex`):

1. **PutIntent** — `PUT /intents/{name}/versions/$LATEST -> 200`. Input: `name` (httpLabel, 1..100, pattern `^([A-Za-z]_?)+$`), `description`, `sampleUtterances`, `slots`, `fulfillmentActivity`, `checksum`, `createVersion`. Store an Intent under `$LATEST` and return the stored record plus server-computed `version` (`$LATEST`), `checksum`, `createdDate`, `lastUpdatedDate`. Re-PUT of an existing intent requires a matching `checksum`; mismatch => `PreconditionFailedException` (412).
2. **PutBot** — `PUT /bots/{name}/versions/$LATEST -> 200`. Input: `name` (httpLabel, 2..50), required `locale` and `childDirected`, plus `description`, `intents` (list of `{intentName, intentVersion}`), `clarificationPrompt`, `abortStatement`, `idleSessionTTLInSeconds` (60..86400), `voiceId`, `processBehavior` (BUILD|SAVE), `checksum`, `createVersion`, `tags`. Persist the bot and return the stored record with computed `version` = `$LATEST`, `checksum`, `createdDate`, `lastUpdatedDate`, and a `status` derived from `processBehavior` (`SAVE` => `NOT_BUILT`; `BUILD` => `READY` once referenced intents exist). Checksum mismatch on update => 412.
3. **GetBot** — `GET /bots/{name}/versions/{versionOrAlias} -> 200`. Must return exactly what PutBot persisted (description, intents, locale, childDirected, idleSessionTTLInSeconds, status, checksum, version, createdDate/lastUpdatedDate). Unknown bot name or version => `NotFoundException` (404).
4. **GetBots** — `GET /bots -> 200`. Returns `bots` (list<BotMetadata>: `name`, `description`, `status`, `lastUpdatedDate`, `createdDate`, `version`) built from stored bots, honoring `nameContains` filtering and `maxResults` (1..50) with a real `nextToken` round-trip.
5. **GetIntent** — `GET /intents/{name}/versions/{version} -> 200`. Returns the persisted intent; unknown name/version => 404.
6. **DeleteBot** — `DELETE /bots/{name} -> 204` (empty body). Removes the bot from state; deleting an unknown bot => `NotFoundException` (404); a subsequent GetBot => 404 and GetBots no longer lists it.

Every one of these must compute its response from persisted state — no canned responses, no echoing the request back without storing it, no random IDs/checksums that are not saved. A PutBot followed by GetBot/GetBots must return what was written, and DeleteBot must be observable by later reads.

## State Model

In `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`), hold:

- `bots: HashMap<String /*bot name*/, BotEntry>` where `BotEntry { versions: BTreeMap<String /*"$LATEST" or numeric*/, Bot>, next_version: u64, tags: Vec<Tag> }`
- `Bot { name, description, intents: Vec<IntentRef{intent_name, intent_version}>, clarification_prompt, abort_statement, status: Status, failure_reason, idle_session_ttl_in_seconds: i32 (default 300), voice_id, locale, child_directed: bool, detect_sentiment: bool, enable_model_improvements, nlu_intent_confidence_threshold, version: String, checksum: String, created_date, last_updated_date }`
- `intents: HashMap<String /*intent name*/, IntentEntry>` where `IntentEntry { versions: BTreeMap<String, Intent>, next_version: u64 }`
- `Intent { name, description, slots, sample_utterances, confirmation_prompt, rejection_statement, follow_up_prompt, conclusion_statement, dialog_code_hook, fulfillment_activity, parent_intent_signature, version, checksum, created_date, last_updated_date }`
- (for later slices, same shape) `slot_types: HashMap<String, SlotTypeEntry>`, `bot_aliases: HashMap<(bot_name, alias_name), BotAlias>`

Checksums are generated server-side per write (e.g. a UUID or content hash) and stored on the record so conditional updates can compare them. Pagination tokens encode an offset/last-key into the sorted name list so `nextToken` is honored rather than ignored.

All other operations (CreateBotVersion, PutBotAlias/GetBotAlias/GetBotAliases, PutSlotType/GetSlotType/..., StartImport/GetImport, GetExport, StartMigration/GetMigration(s), GetUtterancesView, Tag/Untag/ListTagsForResource, channel associations, builtins) stay at 501 for now and will be filed as individual gap issues.

Done = an integration test doing PutIntent -> PutBot -> GetBot -> GetBots -> DeleteBot -> GetBot(404) passes with correct, stateful response bodies, not just 2xx.

## Evidence

OPERATIONS.md (crate root) lists all 42 operations with routes and members: PutBot `PUT /bots/{name}/versions/$LATEST -> 200` with required `locale`/`childDirected` and outputs `status`/`checksum`/`version`/`createdDate`; GetBot `GET /bots/{name}/versions/{versionOrAlias} -> 200` returning the same fields with `NotFoundException (HTTP 404)`; GetBots `GET /bots -> 200` @paginated(inputToken=nextToken, outputToken=nextToken) with `maxResults` 1..50 and `nameContains`; PutIntent `PUT /intents/{name}/versions/$LATEST -> 200` and GetIntent `GET /intents/{name}/versions/{version} -> 200`; DeleteBot `DELETE /bots/{name} -> 204` with `NotFoundException`/`ResourceInUseException`. Protocol section: restJson1, sdkId `Lex Model Building Service`, signing name `lex`. src/ops/put_bot.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and every other file in src/ops/ is the same generated stub, so the service holds zero state today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `lex-models` client at it.
2. PutIntent(name="OrderFlowers", sampleUtterances=["I want flowers"], fulfillmentActivity={type:"ReturnIntent"}) — expect 200 with version "$LATEST" and a checksum; today returns 501.
3. PutBot(name="FlowerBot", locale="en-US", childDirected=false, intents=[{intentName:"OrderFlowers", intentVersion:"$LATEST"}], processBehavior="SAVE", idleSessionTTLInSeconds=300) — expect 200 with version "$LATEST", status NOT_BUILT, a checksum and createdDate.
4. GetBot(name="FlowerBot", versionOrAlias="$LATEST") — expect 200 echoing the persisted description, intents, locale, idleSessionTTLInSeconds and the same checksum returned by PutBot.
5. GetBots(maxResults=10) — expect "FlowerBot" present in `bots` with its stored status and version; GetBots(nameContains="Nope") returns an empty list.
6. GetIntent(name="OrderFlowers", version="$LATEST") — expect the stored sampleUtterances back.
7. DeleteBot(name="FlowerBot") — expect 204 with an empty body.
8. GetBot(name="FlowerBot", versionOrAlias="$LATEST") — expect NotFoundException with HTTP 404, and GetBots no longer lists the bot.
