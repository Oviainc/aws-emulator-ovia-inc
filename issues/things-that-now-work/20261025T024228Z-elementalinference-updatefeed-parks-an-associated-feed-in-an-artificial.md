Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.dictionary-reference-tracks-referencing-feed
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference|DisassociateFeed|ValidationException
Issue-Commit: ef659389d11311dc27a7ee58cb593d22d3f663cf

# UpdateFeed parks an associated feed in an artificial UPDATING window, so the next DisassociateFeed is refused 400 ValidationException instead of releasing the resource

> _Note: this issue was identified against the code base at commit `ef659389d11311dc27a7ee58cb593d22d3f663cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateFeed

## Description
REOPENS 20261024T152949Z-elementalinference-updatefeed-wrong-behavior.md: its fix left the call refused.

Cause 1 of 1: an UpdateFeed on an ACTIVE (associated) feed suspends the feed in a transient UPDATING status for an emulator-invented span, and DisassociateFeed refuses any feed whose status is not ACTIVE — so the DisassociateFeed that follows the update is refused. Failing check: elementalinference.dictionary-reference-tracks-referencing-feed

AWS:      Smithy: `UpdateFeed` = `PUT /v1/feed/{id}` -> 200 (`@idempotent`); `DisassociateFeed` = `POST /v1/feed/{id}/disassociate` -> 200 (`@idempotent`, `associatedResourceName` is the `@idempotencyToken`), modelling `ValidationException` (httpError 400) and `ConflictException` (409). The model does NOT state that an output-list overwrite makes a feed temporarily unreleasable, and it does not state a status precondition for DisassociateFeed — recorded behavior only: in the recording the sequence CreateDictionary -> CreateFeed -> AssociateFeed -> UpdateFeed (dropping the dictionary reference) -> DisassociateFeed runs back-to-back with no wait, UpdateFeed answers 200, the immediately following DisassociateFeed answers 200, and the feed then reports ARCHIVED. An update therefore does not revoke the feed's association, and the release succeeds on a feed that was just reconfigured.
Emulator: UpdateFeed calls `Feed::begin_reconfiguration(now)`, which puts the feed into `FeedStatus::Updating` with a due instant `now + TRANSITION_SPAN` and remembers the suspended settled status. DisassociateFeed then hits its `feed.status != FeedStatus::Active` precondition and is refused HTTP 400 `ValidationException`: "You can't disassociate this feed. It has status UPDATING which indicate that it's not currently associated with a resource." The feed is in fact still associated — the message asserts the opposite of the stored state.
Why:      the normal control-plane sequence "reconfigure the feed, then release its source media" fails for a client that issues the two calls back-to-back, as the recording does. The refusal is a 400 ValidationException, which an SDK does not retry, so the client concludes its request was malformed and that the feed holds no association — while the association is still held and the resource is never released. It also makes the whole multi-resource lifecycle (feed/output/dictionary teardown) unrunnable against the emulator.
Fix:      UpdateFeed's handler and the feed state machine it drives (`begin_reconfiguration`/`TRANSITION_SPAN`): an output-list overwrite must leave an associated feed releasable rather than parking it in a status its own operations refuse. The DisassociateFeed status precondition is the second half of the same cause — it must not read a transient reconfiguration as "not currently associated with a resource" when an association is stored. This is the UPDATING counterpart of the artificial-CREATING-window divergence already fixed under 20261022T192643Z-elementalinference-4-fidelity-divergences.
Traces:   elementalinference.dictionary-reference-tracks-referencing-feed (errors at the DisassociateFeed call that directly follows UpdateFeed). Code citations: `src/ops/update_feed.rs` line ~349 (`feed.begin_reconfiguration(now)`); `src/state.rs` `Feed::begin_reconfiguration` ~line 726 and `TRANSITION_SPAN` ~line 118; `src/ops/disassociate_feed.rs` lines ~248-255 (the `!= Active` refusal).

## Evidence
Check elementalinference.dictionary-reference-tracks-referencing-feed (verdict: error).
Expected (recorded AWS): after `UpdateFeed` (PUT, dictionary reference dropped) answers 200, `DisassociateFeed(id, associatedResourceName)` answers HTTP 200 with the feed's arn/id and the feed then settles to ARCHIVED.
Actual (emulator): `botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the DisassociateFeed operation: You can't disassociate this feed. It has status UPDATING which indicate that it's not currently associated with a resource.` — HTTP 400, Error.Code `ValidationException`. The status and code are the requirement being diverged; the quoted sentence is the emulator's recorded example text.
The earlier legs of the check all pass: the dictionary flips to REFERENCED and lists the feed, AssociateFeed drives the feed ACTIVE, UpdateFeed drops the dictionary field and frees the dictionary back to AVAILABLE. Only the release is refused, and it is refused solely on the status the preceding UpdateFeed installed.
Spec used: smithy_query(query="operation", operation="UpdateFeed,DisassociateFeed") — UpdateFeed PUT /v1/feed/{id} 200, DisassociateFeed POST /v1/feed/{id}/disassociate 200 @idempotent, errors ValidationException(400)/ConflictException(409)/ResourceNotFoundException(404); no modelled status precondition.

## Steps to Reproduce
1. POST /v1/dictionary {"name":"d1","language":"eng","entries":"[{\"content\": \"birdie\"}]"} -> note dictionary id.
2. POST /v1/feed with one output `{"name":"sub1","outputConfig":{"subtitling":{"language":"eng","dictionary":"<dictId>"}},"status":"ENABLED"}` -> note feed id.
3. POST /v1/feed/{feedId}/associate with {"associatedResourceName":"src-1","outputs":[]} -> 200; poll GET /v1/feed/{feedId} until ACTIVE.
4. PUT /v1/feed/{feedId} with the same name and outputs `[{"name":"sub1","outputConfig":{"subtitling":{"language":"eng"}},"status":"ENABLED"}]` -> 200.
5. IMMEDIATELY POST /v1/feed/{feedId}/disassociate with {"associatedResourceName":"src-1","dryRun":false} -> observe HTTP 400 ValidationException reporting status UPDATING; AWS answers HTTP 200 and the feed becomes ARCHIVED.
