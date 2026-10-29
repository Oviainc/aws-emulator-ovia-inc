Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-feed-association-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-feed-association-error-paths elementalinference-feed-lifecycle-associate-disassociate elementalinference-list-dictionaries-and-feeds-pagination elementalinference-feed-dictionary-subtitling-reference
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.outputs[*].outputConfig.cropping.audioInput
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.status
Issue-Key: elemental-inference|CreateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.inputs
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.status
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.cropping.audioInput
Issue-Key: elemental-inference|GetFeed|2xx->2xx|$.outputs[*].outputConfig.clipping.audioInput
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ElementalInference feed responses drop audioInput and inputs, and the create/update lifecycle status is inverted (4 causes)

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFeed

## Description
Four distinct root causes in the elementalinference crate's feed plane, all grounded in replayed wire fixture steps of the simparity-wire-aws-v1 corpus. Every one must be closed before submitting; they span six divergence clusters across CreateFeed, GetFeed and UpdateFeed. (The open issue 20261029T183811Z-elementalinference-archived-feed-association-refusal-searchfixtures-7-day covers the ARCHIVED-association refusal text, the SearchFixtures window message and the dead dataEndpoint; none of the four causes below is one of those.)

Cause 1 of 4 - every output's outputConfig loses its `audioInput` member, on create, on read and on update.
AWS: recorded behavior only. The on-disk Smithy model is older than the recorded API version: CroppingConfig, ClippingConfig and SubtitlingConfig declare no `audioInput` member (Smithy: OutputConfig is a union of cropping|clipping|subtitling|contextualMetadata; CroppingConfig{templateGroups}, ClippingConfig{callbackMetadata,dataSourceConfiguration}, SubtitlingConfig{language,aspectRatio,dictionary,profanityFilter}). The recordings nevertheless show that EVERY output config variant carries a string `audioInput` naming which of the feed's audio inputs that output consumes, and that the service reports it in the CreateFeed 202 body, in the GetFeed 200 body and in the UpdateFeed 200 body, for cropping, clipping and subtitling alike. Its value is the name of an audio input of the feed (in these recordings the feed's single default audio input - see Cause 2); the name itself is state, not a fixed constant.
Emulator: the member is absent from every output config it serializes (replay reports `$.outputs[0].outputConfig.cropping.audioInput: missing (expected 'default-audio', got None)`, likewise `...subtitling.audioInput` and `$.outputs[1].outputConfig.clipping.audioInput`). Because the generated types are built from the model, the member is not deserialized from the request, not stored on crate::state::Output and not serialized back.
Why: a client cannot tell which audio input an output transcribes/crops, and an SDK round-trip (GetFeed -> edit -> UpdateFeed, which is a PUT that replaces the whole output list) silently drops the audio binding of every output, repointing outputs at whatever the service defaults to instead of preserving the caller's wiring.
Fix: the output-config shapes in this crate's ops/types plus crate::state::Output (which must carry the member) and the three handlers that build GetOutput lists - create_feed, update_feed and get_feed - so the member is accepted, stored and reported.
Traces: elementalinference-feed-association-error-paths#task#1 (CreateFeed, cropping), elementalinference-feed-dictionary-subtitling-reference#task#3 (CreateFeed, subtitling), elementalinference-feed-dictionary-subtitling-reference#task#4 (GetFeed), elementalinference-feed-dictionary-subtitling-reference#task#6 (UpdateFeed), elementalinference-feed-lifecycle-associate-disassociate#task#2 and #task#5 (GetFeed, cropping + clipping).

Cause 2 of 4 - GetFeed never reports the feed's `inputs`.
AWS: recorded behavior only (GetFeedResponse in the on-disk model declares arn/name/id/dataEndpoints/outputs/accessRoleArn/status/association/tags and no `inputs`). The recorded GetFeed 200 body carries an additional `inputs` object describing the media inputs the feed exposes: an `audio` list of objects each with a `name`. A feed created with no explicit input has exactly one, the service's default audio input, and that is the name the outputs' `audioInput` (Cause 1) refers to. The member is NOT present in the CreateFeed 202 body of the same recordings - only the reads report it - so it is materialised as part of building the feed. Hypothesis: the default audio input is minted when the feed finishes creating, which is why it is readable only after the 202.
Emulator: the member is absent from every GetFeed body (`$.inputs: missing (expected {'audio': [{'name': 'default-audio'}]}, got None)`).
Why: the input names a feed exposes are the only way a client can learn what value to put in an output's `audioInput` or where to push media; with `inputs` missing, a client that reads a feed to discover its inputs sees none and cannot construct a valid output binding.
Fix: crate::state::Feed must hold the feed's input set (minted with the feed), and the GetFeed handler plus this crate's response types must report it.
Traces: elementalinference-feed-dictionary-subtitling-reference#task#4, elementalinference-feed-lifecycle-associate-disassociate#task#2, elementalinference-feed-lifecycle-associate-disassociate#task#5 (all GetFeed 200).

Cause 3 of 4 - CreateFeed reports the settled status instead of the accept-time CREATING.
AWS: Smithy: CreateFeed is POST /v1/feed with @http code 202; CreateFeedResponse$status is required and targets FeedStatus, whose values are CREATING, AVAILABLE, ACTIVE, UPDATING, DELETING, DELETED, ARCHIVED, and the member documents "After creation of the feed has succeeded, the status will be AVAILABLE". The recordings show what that means on the wire: the 202 answer reports `CREATING` (creation has been ACCEPTED, not completed), and a later GetFeed of the same feed reports AVAILABLE - the GetFeed steps in these same fixtures do not diverge on `status`, so the settled value the emulator already serves for reads is correct.
Emulator: the 202 answer reports `AVAILABLE` ($.status: expected 'CREATING', got 'AVAILABLE'). create_feed.rs stores the new record with crate::state::FeedStatus::Available and deliberately owes no transition, on the premise that a created feed is immediately usable.
Why: a client that waits for the documented asynchronous create - poll GetFeed until the feed leaves CREATING - is handed a terminal-looking status by the accept response, so a waiter keyed on observing CREATING first never starts, and any client asserting the 202 body's status disagrees with AWS. It also contradicts the 202 the operation is modelled with.
Fix: the CreateFeed handler and the initial status/transition the Feed record is minted with in crate::state - accept in CREATING with the move to AVAILABLE falling due on the store's transition clock, exactly as CreateDictionary already does for CREATING -> AVAILABLE.
Traces: elementalinference-feed-association-error-paths#task#1 (representative; same cluster also elementalinference-feed-lifecycle-associate-disassociate#task#1 and elementalinference-list-dictionaries-and-feeds-pagination#task#3) and elementalinference-feed-dictionary-subtitling-reference#task#3.

Cause 4 of 4 - UpdateFeed reports a transient UPDATING the recorded service never returns.
AWS: Smithy: UpdateFeed is PUT /v1/feed/{id} with @http code 200 (not 202) and UpdateFeedResponse$status is required, targeting FeedStatus. The recorded 200 answer reports the feed's SETTLED status - `AVAILABLE` for a feed with no resource associated - i.e. the update is complete when it answers; UPDATING is not what the call hands back.
Emulator: answers 200 with `UPDATING` ($.status: expected 'AVAILABLE', got 'UPDATING'). update_feed.rs calls Feed::begin_reconfiguration, which suspends the status into UPDATING for the store's transition span and reports that transient value in the response body.
Why: a client that updates a feed and then branches on the returned status sees an in-flight state where AWS reports a settled one, so "update done" logic waits for a transition that never had to happen; combined with Cause 3 the two accept paths are exactly inverted (async create reports settled, synchronous update reports in-flight).
Fix: the UpdateFeed handler - report the feed's status as stored after the write rather than the reconfiguration-suspended value; whether a reconfiguration window is modelled for READS is a separate matter, but the 200 body must carry the settled status.
Traces: elementalinference-feed-dictionary-subtitling-reference#task#6.

## Evidence
All six clusters come from replay_wire_corpus on trace set simparity-wire-aws-v1 (score 75.9%, 13 mismatched steps, emulator up, no step blocked or errored). None carries a stub_path or an owned_by.

Cluster 1 - CreateFeed, expected 202 / actual 202, fixture elementalinference-feed-association-error-paths step task#1 (also elementalinference-feed-lifecycle-associate-disassociate#task#1, elementalinference-list-dictionaries-and-feeds-pagination#task#3): "$.outputs[0].outputConfig.cropping.audioInput: missing (expected 'default-audio', got None)" and "$.status: value_mismatch (expected 'CREATING', got 'AVAILABLE')".

Cluster 2 - CreateFeed, 202/202, elementalinference-feed-dictionary-subtitling-reference#task#3: "$.outputs[0].outputConfig.subtitling.audioInput: missing (expected 'default-audio', got None)" and "$.status: value_mismatch (expected 'CREATING', got 'AVAILABLE')".

Cluster 3 - GetFeed, 200/200, elementalinference-feed-dictionary-subtitling-reference#task#4: "$.inputs: missing (expected {'audio': [{'name': 'default-audio'}]}, got None)" and "$.outputs[0].outputConfig.subtitling.audioInput: missing (expected 'default-audio', got None)".

Cluster 4 - UpdateFeed, 200/200, elementalinference-feed-dictionary-subtitling-reference#task#6: "$.outputs[0].outputConfig.subtitling.audioInput: missing (expected 'default-audio', got None)" and "$.status: value_mismatch (expected 'AVAILABLE', got 'UPDATING')".

Cluster 5 - GetFeed, 200/200, elementalinference-feed-lifecycle-associate-disassociate#task#2: "$.inputs: missing" and "$.outputs[0].outputConfig.cropping.audioInput: missing".

Cluster 6 - GetFeed, 200/200, elementalinference-feed-lifecycle-associate-disassociate#task#5 (also #task#7): "$.inputs: missing", "$.outputs[0].outputConfig.cropping.audioInput: missing", "$.outputs[1].outputConfig.clipping.audioInput: missing".

Status codes agree everywhere (202 for CreateFeed, 200 for GetFeed/UpdateFeed); only body members diverge. 'default-audio' is the input NAME held by these recordings' feeds - the requirement is that the member is present and names one of the feed's inputs, not that literal token. Emulator code backing causes 3 and 4: services/service-com-amazonaws-elementalinference-elementalinference/src/ops/create_feed.rs lines 395-402 (status: FeedStatus::Available, "no CREATING window to wait out") and src/ops/update_feed.rs lines 369-392 (feed.begin_reconfiguration(now) then status: feed.status.as_wire()).

## Steps to Reproduce
Replay the elemental-inference fixtures of trace set simparity-wire-aws-v1 (replay_wire_corpus), or by hand against the emulator:

Causes 1 and 3: POST /v1/feed with one cropping output (name + outputConfig.cropping.templateGroups) - inspect the 202 body: `status` is AVAILABLE where AWS reports CREATING, and the output's outputConfig.cropping has no `audioInput`. Repeat with a subtitling output (fixture elementalinference-feed-dictionary-subtitling-reference#task#3) for the subtitling variant.

Cause 2: create a feed as above, then GET /v1/feed/{id} - the 200 body has no `inputs` member at all (AWS reports inputs.audio[*].name) and the outputs' configs still lack `audioInput`.

Cause 4: create a feed, then PUT /v1/feed/{id} with the replacement name + outputs - the 200 body reports status UPDATING where AWS reports the settled AVAILABLE, and the replaced output's config has lost `audioInput`.

Cause 1 round-trip: create a feed with outputs whose configs carry `audioInput`, GET it, and compare - the member supplied in the request never appears in any response.
