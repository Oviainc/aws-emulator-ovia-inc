Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1341f3b9d31b65a2fd61172106a4f6e6c4a020d0

# AssociateFeed refuses an append that adds no output, so a claim carrying an empty outputs array never takes the association

_Filed: 2026-10-29T09:00:00Z_
_Operation: AssociateFeed | Class: wrong-behavior_

> _Note: filed by the issue fixer of `20261028T175133Z-elementalinference-createfeed-maintainability--r2` from the merge gate's own behavioural evidence (see Evidence), because the defect blocks every change to this crate and had no report of its own. It is resolved in the same change, which is why this report sits under `things-that-now-work`._

## Description

`AssociateFeed` held its request's `outputs` list to the cardinality rule that belongs to the operations which SET a feed's whole output list, so a claim carrying `"outputs": []` was refused with HTTP 400 `ValidationException` ("You must specify at least one output.") and the association was never taken. The recorded service SERVES that call: the claim is accepted, the feed reports `ACTIVE` with its `associatedResourceName`, and the feed keeps exactly the outputs it already held.

The rule the real service follows: a feed always holds at least one output, so an operation whose list becomes the feed's WHOLE list (`CreateFeed`, which mints it; `UpdateFeed`, whose list is the one the feed is left holding) refuses a list that would leave it holding none — but an operation that APPENDS to the stored list refuses no list for its cardinality, because the feed keeps the outputs it already holds: a request that adds none takes none away, and the call does the rest of what the request says.

`AssociateFeed` is documented as a PATCH — "AssociateFeed is a PATCH operation, which means that you can include only parameters that you want to change. Parameters that you don't include will not be affected" — and its outputs "supplement any outputs that you specified when you created or updated the feed". `smithy_query(query="operation",operation="AssociateFeed")` gives `outputs` the target `CreateOutputList` with no `@length`, so an empty array satisfies every modelled constraint and the boundary validator passes it to the handler. The member's prose "an array of one or more outputs" describes what a caller supplies when it adds outputs; it is not a constraint the service enforces, as the recorded responses below show.

This supersedes the conclusion of `issues/things-that-now-work/20261027T233521Z-elementalinference-associatefeed-validation--r2.md`, which asked for exactly this refusal to be added. That report argued from the member's prose alone and cited no recorded response — and it records that the suite until then relied on the empty append being served (`tests/all/feed_association.rs:701`, `tests/all/feed_output_validation.rs:268-272`). Where an issue's account of the service and the recorded response disagree, the recording decides.

## Evidence

The merge gate's `crate_fidelity_regression` reviewer, on a candidate of this crate: "7 unit(s) of the touched crates that passed on release `c7f7d31100e4` no longer pass". All seven diverge on the same call, `associate_feed(id=<feed>, associatedResourceName=<name>, outputs=[])`, with "botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the AssociateFeed operation: You must specify at least one output." where the recording is served:

* `elementalinference.associate-feed-activates-feed`
* `elementalinference.associate-feed-concurrent-race-single-winner` ("expected exactly 1 winner, got 0")
* `elementalinference.associate-feed-second-resource-rejected`
* `elementalinference.dictionary-reference-tracks-referencing-feed`
* `elementalinference.disassociate-feed-dry-run-does-not-persist`
* `elementalinference.disassociate-feed-never-associated-rejected`
* `elementalinference.disassociate-feed-wrong-resource-name-rejected`

The refusal in the handler — `src/ops/associate_feed.rs`, before the fix: the appended list was run through `crate::state::supplied_output_list_refusal`, whose first clause (`OutputListRefusal::MissingOutputs`) is a rule about the list the feed is LEFT holding, and whose message is `crate::state::MISSING_OUTPUTS_MESSAGE`.

The operations that legitimately refuse an empty list, with the documentation that gives them the rule: `CreateFeedRequest$outputs` ("You must specify at least one output"), and `UpdateFeed`, whose list replaces the stored one.

## Steps to Reproduce

1. `POST /v1/feed` with `{"name":"empty-append","outputs":[{"name":"crop1","outputConfig":{"cropping":{"templateGroups":[{"name":"group1","templateUris":["s3://templates/wide.psd"]}]}},"status":"ENABLED"}]}` and note the feed id F
2. `POST /v1/feed/F/associate` with `{"associatedResourceName":"source-media","outputs":[],"dryRun":false}`
3. Observed: HTTP 400 `ValidationException` "You must specify at least one output.", and `GET /v1/feed/F` still reports `AVAILABLE` with no association
4. Expected (as recorded): HTTP 200 answering the feed's `arn` and `id`, `GET /v1/feed/F` reporting `ACTIVE` with `association.associatedResourceName` = `source-media`, and the feed still holding exactly the output it was created with
5. For contrast, `POST /v1/feed` and `PUT /v1/feed/F` with `"outputs": []` are still refused with that same 400: their list is the one the feed would be left holding
