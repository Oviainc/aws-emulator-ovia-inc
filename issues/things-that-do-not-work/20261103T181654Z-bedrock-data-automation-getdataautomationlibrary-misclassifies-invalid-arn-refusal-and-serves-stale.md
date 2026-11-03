Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-library-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-library-error-paths bedrock-data-automation-library-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->4xx|status
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|2xx->2xx|$.library.entityTypes[*]
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|2xx->2xx|$.library.libraryDescription
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->2xx|$
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->2xx|$.message
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|GetDataAutomationLibrary|4xx->2xx|status
Issue-Commit: 693e56bfaa4159f6333ea367977c7471ac0df2f0

# GetDataAutomationLibrary misclassifies invalid-ARN refusal and serves stale library state

> _Note: this issue was identified against the code base at commit `693e56bfaa4159f6333ea367977c7471ac0df2f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAutomationLibrary

## Description
Four net-new causes on the bedrock-data-automation Data Automation Library read path. Every cause is observed through GetDataAutomationLibrary: one is a misclassified refusal, the other three are reads that do not reflect the library's actual lifecycle state (entity types, updated description, deletion). All four must be closed before this issue is submitted.

Cause 1 of 4: GetDataAutomationLibrary answers a region-invalid library ARN with 404 ResourceNotFoundException instead of 400 ValidationException.
AWS: Smithy: GetDataAutomationLibrary.libraryArn is an @httpLabel required member targeting DataAutomationLibraryArn with @length max 128 and @pattern ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$; the operation models both ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). An ARN that is well formed but does not belong to the endpoint's region is an input-validation failure, not a lookup miss: the recorded response is HTTP 400 with header x-amzn-errortype: 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/' and body message 'The provided ARN is invalid for the service region'. That message is entirely fixed text for this error class - it interpolates no request or state value.
Emulator: HTTP 404 with x-amzn-errortype: 'ResourceNotFoundException' and message 'Library Arn not found.' - a lookup miss, with emulator prose rather than AWS wording.
Why: a client that distinguishes a wrong-region/ malformed ARN (non-retryable, fix the input) from an absent resource (create it, or treat as deleted) takes the wrong branch. SDK error-matching on ValidationException never fires; code that treats ResourceNotFoundException as "already gone" silently swallows a misaddressed request.
Fix: the ARN admission check in GetDataAutomationLibrary's input validation - region (and partition/account) conformance must be decided before the state lookup and must raise ValidationException with the message above and the coral-qualified x-amzn-errortype, instead of falling through to the not-found path in the handler's state lookup.
Traces: bedrock-data-automation-library-error-paths#task#3.

Cause 2 of 4: a library read omits the library's entityTypes entirely.
AWS: recorded behavior only - the Smithy model types DataAutomationLibrary.entityTypes as a list, but states nothing about when it is populated. The recording shows that after the fixture's library is populated, GetDataAutomationLibrary returns library.entityTypes with at least one element whose entityType is 'VOCABULARY' and which carries an entityMetadata value (the metadata contents are shape-asserted only, not value-asserted). The element count and the metadata contents are state-derived, not fixed.
Emulator: returns library.entityTypes as an empty list, so $.library.entityTypes[0] is missing; HTTP status is 200 and the rest of the library envelope matches.
Why: a client enumerating what a library can resolve against sees a library with no entity types and concludes it is unusable or still empty, skipping ingestion-dependent work that AWS would have allowed. Hypothesis: the entity types are never recorded into library state because the ingestion path that would add them is not implemented - see open issues '20261103T163536Z-bedrock-data-automation-invokedataautomationlibraryingestionjob-stub.md' and '20261103T163536Z-bedrock-data-automation-listdataautomationlibraryingestionjobs-stub.md', neither of which carries this replay key; this read-back divergence is not covered by them.
Fix: library state must persist the entity types a library has acquired, and GetDataAutomationLibrary's serializer must project them onto the response's library.entityTypes member.
Traces: bedrock-data-automation-library-lifecycle#task#2.

Cause 3 of 4: a library description update is not visible to a subsequent read.
AWS: Smithy: UpdateDataAutomationLibrary (PUT /data-automation-libraries/{libraryArn}/, idempotent) accepts libraryDescription targeting DataAutomationLibraryDescription (@length 0..300, @sensitive) and returns libraryArn + status; GetDataAutomationLibrary then returns the library whose libraryDescription is the value last written. In the recording the read-back after the update returns the updated description value supplied by that update request.
Emulator: the read-back still returns the description the library was created with - the value from the create request, unchanged - at HTTP 200. In the same response entityTypes is also still empty (Cause 2).
Why: read-after-write is broken for library metadata: a client that updates a description and then confirms it sees its write silently lost, and any reconcile loop re-issues the update forever. Hypothesis: the update never reaches state because the operation is an unimplemented scaffold - see open issue '20261103T163536Z-bedrock-data-automation-updatedataautomationlibrary-stub.md', which does not carry this replay key; the stale read is this issue's.
Fix: UpdateDataAutomationLibrary's handler must write libraryDescription into library state so the GetDataAutomationLibrary read path returns the newest value.
Traces: bedrock-data-automation-library-lifecycle#task#4.

Cause 4 of 4: a deleted library keeps answering 200 ACTIVE instead of 404 ResourceNotFoundException.
AWS: Smithy: DeleteDataAutomationLibrary (DELETE /data-automation-libraries/{libraryArn}/) returns libraryArn + status, and GetDataAutomationLibrary models ResourceNotFoundException with httpError 404. Once the delete has settled, the read returns HTTP 404 ResourceNotFoundException whose body message is 'The specified library Arn not found.' - fixed text for this error class, with no interpolated id - and an x-amzn-errortype naming ResourceNotFoundException. The recording polls for that terminal state rather than asserting it on the first read, so eventual settling is allowed; never settling is not.
Emulator: HTTP 200 with the full library envelope still present and status 'ACTIVE', for all 16 poll attempts - no error code is ever produced, so the library is never observably deleted.
Why: deletion is unobservable. Teardown and idempotent-delete logic that waits for a 404 hangs until it times out, and a client re-creating under the same name believes the old library is still live. Hypothesis: the delete never mutates state - see open issue '20261103T163536Z-bedrock-data-automation-deletedataautomationlibrary-stub.md', which does not carry this replay key.
Fix: DeleteDataAutomationLibrary's handler must remove (or tombstone) the library in state, and GetDataAutomationLibrary's lookup must then raise ResourceNotFoundException with the message above.
Traces: bedrock-data-automation-library-lifecycle#teardown#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 70 steps, 47 passed, 23 mismatched, score 67.1%). The four net-new clusters, none carrying stub_path or owned_by:

Cause 1 - bedrock-data-automation-library-error-paths#task#3, GetDataAutomationLibrary:
  status: expected 400, got 404
  $.message: expected 'The provided ARN is invalid for the service region', got 'Library Arn not found.'
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/', got 'ResourceNotFoundException'

Cause 2 - bedrock-data-automation-library-lifecycle#task#2, GetDataAutomationLibrary (status 200 on both sides):
  $.library.entityTypes[0]: missing (expected {'entityMetadata': '<shaped-not-asserted>', 'entityType': 'VOCABULARY'}, got None)

Cause 3 - bedrock-data-automation-library-lifecycle#task#4, GetDataAutomationLibrary (status 200 on both sides):
  $.library.entityTypes[0]: missing (expected {'entityMetadata': '<shaped-not-asserted>', 'entityType': 'VOCABULARY'}, got None)
  $.library.libraryDescription: value_mismatch (expected 'Updated description', got 'Initial description')

Cause 4 - bedrock-data-automation-library-lifecycle#teardown#6, GetDataAutomationLibrary:
  status: expected 404, got 200
  $.message: missing (expected 'The specified library Arn not found.', got None)
  $headers.x-amzn-errortype: missing
  $: poll: error code None not in ['ResourceNotFoundException'] after 16 attempt(s); emulator still served {'library': {'libraryArn': 'arn:aws:bedrock:<region>:<account>:data-automation-library/<id>', 'libraryName': '<name>', 'libraryDescription': 'Initial description', 'status': 'ACTIVE', 'entityTypes': []}}

Model evidence (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json): GetDataAutomationLibrary GET-shaped read at POST /data-automation-libraries/{libraryArn}/, errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; libraryArn @pattern ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$. UpdateDataAutomationLibrary.libraryDescription @length 0..300. DeleteDataAutomationLibrary returns libraryArn + status.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture bedrock-data-automation-library-error-paths, step task#3 - call GetDataAutomationLibrary with a well-formed data-automation-library ARN whose region is not the endpoint's region. Expect 400 ValidationException / 'The provided ARN is invalid for the service region'; observe 404 ResourceNotFoundException / 'Library Arn not found.'.
4. Cause 2: replay fixture bedrock-data-automation-library-lifecycle, step task#2 - create a library, let the fixture populate it, then GetDataAutomationLibrary. Expect library.entityTypes to contain an element with entityType 'VOCABULARY' and an entityMetadata value; observe an empty entityTypes list.
5. Cause 3: same fixture, step task#4 - UpdateDataAutomationLibrary with a new libraryDescription, then GetDataAutomationLibrary. Expect the updated description; observe the description from the create call.
6. Cause 4: same fixture, step teardown#6 - DeleteDataAutomationLibrary, then poll GetDataAutomationLibrary. Expect it to settle on 404 ResourceNotFoundException / 'The specified library Arn not found.'; observe 200 with status 'ACTIVE' for all 16 polls.
