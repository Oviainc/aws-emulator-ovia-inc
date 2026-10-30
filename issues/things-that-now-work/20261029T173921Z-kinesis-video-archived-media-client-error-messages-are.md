Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideoarchivedmedia-get-media-for-fragment-list-validation
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideoarchivedmedia-get-media-for-fragment-list-validation kinesisvideoarchivedmedia-list-fragments-addressing-and-validation
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media|GetMediaForFragmentList|4xx->4xx|$.message
Issue-Key: kinesis-video-archived-media|ListFragments|4xx->4xx|$.Message
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# Kinesis Video Archived Media client-error messages are emulator-authored instead of AWS wording (3 causes)

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFragments

## Description
Shared fault: on the KVAM paths where the emulator does reach the right HTTP status and error class, the Message/message text it serializes is its own prose rather than the fixed wording AWS uses for that error class. The status and code are right, so clients that branch on the code work; clients and operators that read, log or assert on the text see something AWS never sends. Three distinct refusal templates are wrong; a fixer must close all 3.

Cause 1 of 3 — GetMediaForFragmentList Smithy constraint violation is reported per-element instead of per-member.
AWS: Smithy GetMediaForFragmentList input member Fragments targets FragmentNumberList (length 1..1000) whose member FragmentNumberString has @length min 1 max 128 and @pattern ^[0-9]+$. AWS reports the aggregate member constraint and does NOT echo the offending value or its index. The 400 body carries message "<n> validation error detected: Value at 'fragments' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[0-9]+$]" — fixed text apart from the leading error count and the member path 'fragments', which is the request member's wire name.
Emulator: returns the right 400 but with message "1 validation error detected: Value 'not-a-fragment-number' at 'fragments.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$" — it echoes the rejected value, addresses an indexed element path ('fragments.1.member') and lists only the one constraint that failed.
Why: callers and test suites that match AWS's constraint sentence fail to match, and the echoed value leaks request content into a message AWS keeps value-free.
Fix: GetMediaForFragmentList's input validation / constraint-message serializer — collapse list-member constraint failures to the member path and emit the full bracketed constraint set without the value.
Traces: kinesisvideoarchivedmedia-get-media-for-fragment-list-validation#task#5.

Cause 2 of 3 — ListFragments stream-addressing refusal is reworded.
AWS: Smithy ListFragments takes StreamName and StreamARN, both optional, and exactly one must be supplied. Supplying both (or neither) is refused 400 InvalidArgumentException with message 'One of StreamName or StreamARN must be provided, but not both.' — entirely fixed text, no variable part.
Emulator: returns 400 with Message 'StreamName and StreamARN are mutually exclusive: specify exactly one'.
Why: the refusal is semantically right but the text is not AWS's, so clients keying on the message (and users reading it) see wording no AWS region produces; it also fails to state the 'neither' half of the rule.
Fix: ListFragments' validation — use AWS's fixed addressing sentence for both the both-supplied and neither-supplied cases.
Traces: kinesisvideoarchivedmedia-list-fragments-addressing-and-validation#task#7 (also #task#8).

Cause 3 of 3 — ListFragments not-found refusal is reworded.
AWS: Smithy ListFragments models ResourceNotFoundException, errorKind client, httpError 404. For a stream that does not exist or is not ACTIVE the message is 'The requested stream is not found or not active.' — entirely fixed text; it deliberately does not name the stream.
Emulator: returns the correct 404 ResourceNotFoundException but with Message 'No such stream: StreamName <streamName>' (fixed prefix plus the StreamName from the request).
Why: the message asserts a different fact (absent) than AWS's (absent OR not active) and injects request content AWS omits, so a client that treats the text as diagnostic misreports an inactive stream as nonexistent.
Fix: ListFragments' handler/validation — author AWS's fixed not-found sentence for the stream-resolution failure.
Traces: kinesisvideoarchivedmedia-list-fragments-addressing-and-validation#task#9 (also #task#11, #task#12).

## Evidence
Cause 1: status 400 as expected; $.message expected "1 validation error detected: Value at 'fragments' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[0-9]+$]", got "1 validation error detected: Value 'not-a-fragment-number' at 'fragments.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$". Cause 2: status 400 as expected; $.Message expected 'One of StreamName or StreamARN must be provided, but not both.', got 'StreamName and StreamARN are mutually exclusive: specify exactly one'. Cause 3: status 404 as expected; $.Message expected 'The requested stream is not found or not active.', got 'No such stream: StreamName sim2real-probe-kvam-lf-missing-941dd60a'. The probe stream name is per-run and not part of the requirement; the fixed message text is.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator. 2. Fixture kinesisvideoarchivedmedia-get-media-for-fragment-list-validation, step task#5: call GetMediaForFragmentList with a non-numeric fragment number and compare $.message to AWS's aggregate constraint sentence. 3. Fixture kinesisvideoarchivedmedia-list-fragments-addressing-and-validation, step task#7 (and task#8): call ListFragments with both StreamName and StreamARN and compare $.Message. 4. Same fixture, step task#9 (and task#11, task#12): call ListFragments naming a stream that was never created and compare $.Message.
