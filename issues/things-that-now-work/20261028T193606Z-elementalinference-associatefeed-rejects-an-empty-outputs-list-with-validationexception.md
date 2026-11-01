Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.associate-feed-activates-feed elementalinference.associate-feed-second-resource-rejected elementalinference.associate-feed-concurrent-race-single-winner elementalinference.dictionary-reference-tracks-referencing-feed elementalinference.disassociate-feed-dry-run-does-not-persist elementalinference.disassociate-feed-never-associated-rejected elementalinference.disassociate-feed-wrong-resource-name-rejected
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference|AssociateFeed|ValidationException
Issue-Key: elementalinference.associate-feed-concurrent-race-single-winner|run#11|AssertionError|assert len(winners) == 1, f"expected exactly 1 winner, got {len(winners)}: {results}"
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# AssociateFeed rejects an empty outputs list with ValidationException "You must specify at least one output.", refusing a request AWS accepts

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateFeed

## Description
One root cause, Cause 1 of 1.

Cause 1 of 1: AssociateFeed applies a non-modeled minimum-cardinality check to the `outputs` member and refuses any request whose `outputs` list is empty, so the whole associate/disassociate lifecycle is unreachable.

AWS: Smithy model `com-amazonaws-elementalinference-elementalinference.json` — `AssociateFeed` POST /v1/feed/{id}/associate, input members `id` (@httpLabel, required, pattern ^[a-z0-9]{19}$), `associatedResourceName` (required, @length 1..2048), `outputs` (required, target `CreateOutputList`), `dryRun` (default false). `outputs` is `required` but its target list `CreateOutputList` carries NO `@length` constraint at all, so an empty list satisfies every modeled constraint; `AssociateFeed` is also `@idempotent`. Recorded behavior confirms the model: `associate_feed(id=<feed-id>, associatedResourceName=<name>, outputs=[])` is ACCEPTED and returns HTTP 200 with `{arn, id}` — the feed keeps the output set it was created with, and the feed transitions AVAILABLE -> ACTIVE with `association.associatedResourceName` set to the supplied name. `ValidationException` is modeled (client error, HTTP 400, single required `message`) but AWS does not raise it for this input; there is no modeled error whose documentation covers "no outputs supplied" on associate (recorded behavior only for the acceptance).

Emulator: every `AssociateFeed` call with `outputs=[]` is refused with HTTP 400, error code `ValidationException`, message `You must specify at least one output.` (this message appears to be wholly fixed text — it names no request value, no member name and no limit, so no part of it is interpolated from the request or state). The call never reaches the feed state machine, so the feed stays AVAILABLE and acquires no `association`.

Why: a client that associates a resource to a feed without restating the feed's outputs — the documented and recorded way to do it, since the outputs were already supplied at CreateFeed — gets a hard 400 instead of a 200 association. Nothing downstream of association can be driven at all: the feed never becomes ACTIVE, `association` is never populated, a dictionary referenced by an output can never be observed through an active feed, `DisassociateFeed` can never be exercised (neither its happy path, its `dryRun=true` no-op, its "never associated" rejection, nor its "wrong resource name" rejection), and the concurrent-association race has no winner at all (both racers are refused, so the emulator reports 0 winners where AWS has exactly 1 winner and 1 ValidationException loser). Because the refusal happens before any state change, the emulator looks like a service with no association feature rather than one with a stricter validator.

Additionally masked by this same cause and therefore NOT independently confirmed by this run (they must be re-checked once the refusal is lifted): the "already associated" rejection message `The feed is already associated with a resource.`, the never-associated rejection message `You can't disassociate this feed. It has status AVAILABLE which indicate that it's not currently associated with a resource.`, the wrong-resource rejection message `You can't disassociate this feed because it isn't associated with the resource you specified. Specify the correct resource.`, the `dryRun=true` no-op on DisassociateFeed, the ARCHIVED terminal transition, and the `Resource not found` message on ResourceNotFoundException after delete. Each of those is fixed text except where noted and should hold after the fix.

Fix: the request-validation layer of the AssociateFeed handler in the elementalinference crate — drop the minimum-cardinality rejection on `outputs` so a modeled-valid empty list is accepted, and let the handler fall through to the feed state transition (set `association.associatedResourceName`, move AVAILABLE -> ACTIVE, preserve the feed's existing outputs when the request supplies none). Validation of `outputs` should be limited to what the model states (presence of the member, and per-element `CreateOutput` constraints), not an invented non-empty requirement.

Traces: elementalinference.associate-feed-activates-feed, elementalinference.associate-feed-second-resource-rejected, elementalinference.associate-feed-concurrent-race-single-winner, elementalinference.dictionary-reference-tracks-referencing-feed, elementalinference.disassociate-feed-dry-run-does-not-persist, elementalinference.disassociate-feed-never-associated-rejected, elementalinference.disassociate-feed-wrong-resource-name-rejected.

## Evidence
Expected (AWS / Smithy model): POST /v1/feed/{id}/associate with body {"associatedResourceName": "<name>", "outputs": []} -> HTTP 200, body {"arn": "<feed arn>", "id": "<feed id>"}. `CreateOutputList` has no @length constraint, so [] is valid input. A subsequent GetFeed shows status ACTIVE and association.associatedResourceName == <name>, with the feed's original outputs intact.

Actual (emulator): HTTP 400, error code `ValidationException`, message `You must specify at least one output.` botocore surfaces it as: `botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the AssociateFeed operation: You must specify at least one output.` No state change occurs; GetFeed still reports status AVAILABLE with no association.

Consequence observed in the race check: both concurrent AssociateFeed calls were refused, so the result set was [('ValidationException', '<resource-a>'), ('ValidationException', '<resource-b>')] and the assertion `expected exactly 1 winner, got 0` fired. AWS yields exactly one `ok` and one `ValidationException`.

## Steps to Reproduce
1. CreateFeed with one output, e.g. outputs=[{"name": "out1", "outputConfig": {"cropping": {}}, "status": "ENABLED"}]; note the returned feed id.
2. Poll GetFeed until status is AVAILABLE and no `association` is present.
3. Call AssociateFeed(id=<feed id>, associatedResourceName=<any name matching ^[\w \-\.',@:;/]*$ of length 1..2048>, outputs=[]).
4. Observe HTTP 400 ValidationException "You must specify at least one output." instead of HTTP 200 {arn, id}.
5. GetFeed again: the feed is still AVAILABLE with no association, confirming no state transition occurred.
6. For the race variant, issue two AssociateFeed calls concurrently on the same feed with outputs=[] and different associatedResourceName values: both are refused, so there is no winner, where AWS admits exactly one and fails the other with ValidationException.
