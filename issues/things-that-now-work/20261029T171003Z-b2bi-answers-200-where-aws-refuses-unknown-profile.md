Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-list-operations-and-pagination-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-list-operations-and-pagination-errors b2bi-transformer-job-error-paths b2bi-transformer-lifecycle
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|ListPartnerships|4xx->2xx|$.__type
Issue-Key: b2bi|ListPartnerships|4xx->2xx|$.message
Issue-Key: b2bi|ListPartnerships|4xx->2xx|status
Issue-Key: b2bi|StartTransformerJob|4xx->2xx|$.Message
Issue-Key: b2bi|StartTransformerJob|4xx->2xx|$.__type
Issue-Key: b2bi|StartTransformerJob|4xx->2xx|status
Issue-Key: b2bi|GetTransformer|4xx->2xx|$
Issue-Key: b2bi|GetTransformer|4xx->2xx|$.__type
Issue-Key: b2bi|GetTransformer|4xx->2xx|$.message
Issue-Key: b2bi|GetTransformer|4xx->2xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# b2bi answers 200 where AWS refuses: unknown profile, cross-region bucket and deleted transformer

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPartnerships

## Description
Three net-new causes, one shared fault: three b2bi calls that AWS refuses with a modeled client error are answered HTTP 200 with a success body, so a client sees a request as having succeeded where AWS rejected it.

Shared AWS behavior (Smithy: services/service-com-amazonaws-b2bi-b2bi, protocol awsJson1_0; ResourceNotFoundException and the common ValidationException are both client errors, recorded on this service with HTTP status 400 and a fully-qualified `__type` in the com.amazonaws.visby namespace; ValidationException's message member is `Message`, ResourceNotFoundException's is `message`).

Shared emulator behavior: no refusal at all — HTTP 200 and a normal success payload, with no `__type` and no message member.

Why: these are the three ways a client learns that its reference is wrong. A pagination/listing caller that passes a stale profileId gets an empty-but-successful page instead of ResourceNotFoundException and concludes the profile has no partnerships; a job submitter whose bucket is in the wrong region gets a job accepted that AWS would never start; and a delete-then-verify teardown sees the deleted transformer still readable, so nothing can distinguish a live resource from a deleted one.

Fix: the named operations' validation/lookup ordering and, for cause 3, the delete path in the transformer state store.

Cause 1 of 3: ListPartnerships with a profileId that no profile holds. AWS: 400, __type com.amazonaws.visby#ResourceNotFoundException, message "Profile with profileId <profileId> not found." — fixed text "Profile with profileId " + the profileId from the request + " not found.". Emulator: 200 with a partnerships page and no error members. Fix: ListPartnerships' handler must resolve the profileId filter before it builds a page (ResourceNotFoundException is modeled on this operation). Trace: b2bi-list-operations-and-pagination-errors#task#4.

Cause 2 of 3: StartTransformerJob whose inputFile bucket is not in the Region the call is made in. AWS: 400, __type com.amazonaws.visby#ValidationException, Message "Input Bucket <bucketName> is not in current region <region>" — fixed text "Input Bucket ", the request's bucket name, " is not in current region ", the request's signing Region. Emulator: 200, the job is accepted. Fix: StartTransformerJob's input validation must compare the input bucket's Region with the request Region before it starts a job. Trace: b2bi-transformer-job-error-paths#task#3.

Cause 3 of 3: GetTransformer for a transformer that DeleteTransformer already removed. AWS: 400, __type com.amazonaws.visby#ResourceNotFoundException, message "Transformer not found with ID <transformerId>" — fixed text "Transformer not found with ID " + the requested transformerId. Emulator: 200, returning the full stored transformer (id, ARN, name, status active, timestamps, mapping, outputConversion) indefinitely, so the fixture's poll for the refusal never settles. Fix: the transformer state store's delete must remove the record (or mark it unreadable) and GetTransformer must raise ResourceNotFoundException for it. Trace: b2bi-transformer-lifecycle#teardown#7.

## Evidence
Replay of simparity-wire-aws-v1. b2bi-list-operations-and-pagination-errors#task#4 ListPartnerships: status expected 400 got 200; $.__type missing (expected 'com.amazonaws.visby#ResourceNotFoundException'); $.message missing (expected 'Profile with profileId 00000000000000000000 not found.'). b2bi-transformer-job-error-paths#task#3 StartTransformerJob: status expected 400 got 200; $.__type missing (expected 'com.amazonaws.visby#ValidationException'); $.Message missing (expected 'Input Bucket <bucket> is not in current region us-west-2'). b2bi-transformer-lifecycle#teardown#7 GetTransformer: status expected 400 got 200; $.__type and $.message missing; poll reported 'error code None not in [ResourceNotFoundException] after 29 attempt(s)', the actual body being the still-present transformer record.

## Steps to Reproduce
1. Boot the emulator from mainline. 2. Replay simparity-wire-aws-v1. 3. Replay b2bi-list-operations-and-pagination-errors#task#4, b2bi-transformer-job-error-paths#task#3 and b2bi-transformer-lifecycle#teardown#7. 4. Observe HTTP 200 with a success body where the recording has HTTP 400 with the modeled ResourceNotFoundException / ValidationException.
