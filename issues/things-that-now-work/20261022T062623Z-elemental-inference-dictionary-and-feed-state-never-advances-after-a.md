Issue-Source: trace-sweep-divergence
Issue-Trace: elementalinference-dictionary-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elementalinference-dictionary-lifecycle elementalinference-feed-dictionary-subtitling-reference
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elemental-inference|GetDictionary|4xx->2xx|$
Issue-Key: elemental-inference|GetDictionary|4xx->2xx|$.message
Issue-Key: elemental-inference|GetDictionary|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: elemental-inference|GetDictionary|4xx->2xx|status
Issue-Key: elemental-inference|GetDictionary|2xx->2xx|$.name
Issue-Key: elemental-inference|GetDictionary|2xx->2xx|$
Issue-Key: elemental-inference|GetDictionary|2xx->2xx|$.references[*]
Issue-Key: elemental-inference|GetDictionary|2xx->2xx|$.status
Issue-Key: elemental-inference|GetDictionary|2xx->2xx|$.tags
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.dataEndpoints[*]
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.aspectRatio
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.outputs[*].outputConfig.subtitling.audioInput
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.status
Issue-Key: elemental-inference|UpdateFeed|2xx->2xx|$.tags
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Dictionary and feed state never advances after a mutation

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDictionary

## Description
Mutating operations on ElementalInference resources return success but do not change the stored resource: a delete, a rename, a feed->dictionary reference and a feed update are all invisible to the following read. Four causes, all in the handlers' state transitions rather than in serialization.

Cause 1 of 4 - DeleteDictionary does not remove the dictionary, so GetDictionary keeps answering 200.
AWS: Smithy: DeleteDictionary (DELETE /v1/dictionary/{id}, 202) removes the dictionary; GetDictionary models ResourceNotFoundException with httpError 404 and its doc states a dictionary moves DELETING -> DELETED. After the delete settles, GetDictionary on that id must answer 404 ResourceNotFoundException (recorded body {"message": "Resource not found"} with an x-amzn-errortype header; the exact message text is an example).
Emulator: GetDictionary answers 200 with the full dictionary (status AVAILABLE, references []) indefinitely - the replay polled for 60s / 31 attempts and never saw anything but 200, and no `message` body or x-amzn-errortype header was produced.
Why: a client cannot tell whether a delete took effect, cleanup loops that poll for 404 never terminate, and a recreate-after-delete workflow sees the stale resource.
Fix: the DeleteDictionary handler and dictionary state - the delete must transition the record to DELETING/DELETED and the id must stop resolving; GetDictionary must raise ResourceNotFoundException (404).
Traces: elementalinference-dictionary-lifecycle#teardown#7.

Cause 2 of 4 - UpdateDictionary does not persist the new name.
AWS: Smithy: UpdateDictionary (PATCH /v1/dictionary/{id}) takes an optional `name` (ResourceName, @pattern ^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$) and returns the updated resource; a subsequent GetDictionary returns the updated `name` (required member).
Emulator: after a rename, GetDictionary still returns the pre-update name - `$.name: value_mismatch (expected '<name>v2', got '<name>')`.
Why: a client that renames a dictionary and then reads it back sees the old name, so rename is a silent no-op and name-based reconciliation loops repeat the update forever.
Fix: the UpdateDictionary handler - write the supplied members back into dictionary state instead of only echoing them.
Traces: elementalinference-dictionary-lifecycle#task#4.

Cause 3 of 4 - a dictionary referenced by a feed is not marked REFERENCED and its references list stays empty.
AWS: Smithy: GetDictionaryResponse.status targets DictionaryStatus, whose value set includes REFERENCED, and GetDictionaryResponse.references targets FeedReferences (a list of FeedId). DeleteDictionary's doc states you cannot delete a dictionary that is referenced by a feed. So when a feed's subtitling output names a dictionary, that dictionary becomes status REFERENCED with the feed's id in `references`.
Emulator: the dictionary stays status AVAILABLE with references [] - `$.status: value_mismatch (expected 'REFERENCED', got 'AVAILABLE')` and `$.references[0]: missing (expected the feed id, got None)`; the replay polled 13 attempts for REFERENCED and never saw it.
Why: the reference back-link is how a client discovers which feeds consume a dictionary and why a delete is refused; with it missing, clients believe an in-use dictionary is free to delete and the delete-refusal guard can never fire.
Fix: the CreateFeed/UpdateFeed/AssociateFeed handlers and dictionary state - register the feed reference on the dictionary and derive its status from it.
Traces: elementalinference-feed-dictionary-subtitling-reference#task#5.

Cause 4 of 4 - UpdateFeed leaves the feed in UPDATING instead of settling to AVAILABLE.
AWS: Smithy: UpdateFeed (PUT /v1/feed/{id}, 200) returns the feed with a required `status` of FeedStatus, whose value set is CREATING/AVAILABLE/ACTIVE/UPDATING/DELETING/DELETED/ARCHIVED. Recorded behavior: the UpdateFeed response for an unassociated feed reports status AVAILABLE.
Emulator: it reports UPDATING - `$.status: value_mismatch (expected 'AVAILABLE', got 'UPDATING')`.
Why: a client that waits for a feed to leave the transient UPDATING state before sending media (or before issuing the next update, which AWS would refuse with ConflictException) blocks on a state the emulator never exits.
Fix: the UpdateFeed handler's status transition in feed state.
Traces: elementalinference-feed-dictionary-subtitling-reference#task#6 (this step also shows the dataEndpoints/tags/output-default divergences bundled in the response-serialization issue).

## Evidence
Replay of simparity-wire-aws-v1. Representative step elementalinference-dictionary-lifecycle#teardown#7 (GetDictionary after DeleteDictionary): `status: expected 404, got 200`; `$.message: missing (expected 'Resource not found', got None)`; `$headers.x-amzn-errortype: missing`; `$: poll: status 200 not in [404] after 31 attempt(s)` returning {'name': ..., 'status': 'AVAILABLE', 'references': [], 'tags': {'env': 'probe'}}. elementalinference-dictionary-lifecycle#task#4 (GetDictionary, 200/200): `$.name: value_mismatch (expected 'sim2real-probe-elementalinference-dict-b344569fv2', got 'sim2real-probe-elementalinference-dict-b344569f')`. elementalinference-feed-dictionary-subtitling-reference#task#5 (GetDictionary, 200/200): `$.status: value_mismatch (expected 'REFERENCED', got 'AVAILABLE')`, `$.references[0]: missing (expected '53prtya0vie7788nfh5', got None)`, poll not in ['REFERENCED'] after 13 attempts. elementalinference-feed-dictionary-subtitling-reference#task#6 (UpdateFeed, 200/200): `$.status: value_mismatch (expected 'AVAILABLE', got 'UPDATING')`.

## Steps to Reproduce
1. Replay elementalinference-dictionary-lifecycle: create a dictionary, DELETE /v1/dictionary/{id}, then GET /v1/dictionary/{id} (teardown#7) - it answers 200 with the live dictionary instead of 404 ResourceNotFoundException, forever.
2. In the same fixture, PATCH /v1/dictionary/{id} with a new name, then GET it (task#4) - the old name comes back.
3. Replay elementalinference-feed-dictionary-subtitling-reference: create a feed whose subtitling output references the dictionary, then GET /v1/dictionary/{id} (task#5) - status AVAILABLE, references [].
4. Then PUT /v1/feed/{id} (task#6) - the response status is UPDATING, not AVAILABLE.
