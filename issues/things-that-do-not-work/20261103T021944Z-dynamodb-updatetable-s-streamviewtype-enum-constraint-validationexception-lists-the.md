Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.update-table-misspelled-stream-view-type-rejected-and-enables-no-stream
Issue-Key: dynamodb.update-table-misspelled-stream-view-type-rejected-and-enables-no-stream|run#8|AssertionError|assert got == ("ValidationException",
Issue-Commit: bc4d78eb61f619d05d76afe3f781275ae6918fe7

# UpdateTable's StreamViewType enum-constraint ValidationException lists the enum value set in Smithy declaration order, not the order AWS serves (1 cause)

> _Note: this issue was identified against the code base at commit `bc4d78eb61f619d05d76afe3f781275ae6918fe7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Cause 1 of 1: when UpdateTable is called with a StreamSpecification whose StreamViewType is not a member of the enum, the emulator refuses it with the right status and error code but renders the "enum value set" clause of the message in the order the values are declared in the Smithy model instead of the order DynamoDB serves.

The failing check is marked neither ALREADY FILED nor OWNED by the replay report. The open issues 20261029T180026Z-dynamodb-createtable-updatetable-putitem-validation-10-missing-or.md and 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md cover neighbouring DynamoDB validation wording but carry other checks' replay keys and do not cover this member.

AWS: Smithy — UpdateTable.StreamSpecification.StreamViewType targets com.amazonaws.dynamodb#StreamViewType, an enum whose declared values are [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY]; a value outside the set is refused ValidationException (HTTP 400). Recorded behavior for the message: the fixed text is `1 validation error detected: Value '<value>' at '<member-path>' failed to satisfy constraint: Member must satisfy enum value set: [OLD_IMAGE, KEYS_ONLY, NEW_AND_OLD_IMAGES, NEW_IMAGE]`. The variable parts are only `<value>` (the rejected value copied from the request, here the misspelling `NEW_AND_OLD_IMAGE`) and `<member-path>` (`streamSpecification.streamViewType`); the bracketed value set is fixed text for this member and AWS serves it in the order OLD_IMAGE, KEYS_ONLY, NEW_AND_OLD_IMAGES, NEW_IMAGE.
Emulator: same status and code, but the bracketed set comes out in model-declaration order — the response message is `1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'streamSpecification.streamViewType' failed to satisfy constraint: Member must satisfy enum value set: [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY]`, i.e. the enum members are emitted in the order they appear in the Smithy enum rather than AWS's order.
Why: the message is part of the refusal contract. A client (or a test/assertion, a log matcher, a UI that surfaces the service's sentence verbatim) that compares the served message to AWS's sees a mismatch for every enum-constraint refusal on this member, and the divergence is systematic: any emulator refusal whose message embeds an enum value set is rendered in declaration order, so the defect generalises beyond StreamViewType.
Why (scope): because the refusal message is the check's FIRST assertion, everything it would assert afterwards is left unverified by this run — that the refused UpdateTable creates no stream (DescribeTable must carry neither StreamSpecification nor LatestStreamArn and DynamoDB Streams ListStreams must report none for the table), and that the corrected UpdateTable with NEW_AND_OLD_IMAGES then enables exactly one stream whose DescribeStream reports StreamViewType NEW_AND_OLD_IMAGES. Those requirements must hold once the message is fixed.
Fix: the shared enum-constraint message formatter used by the dynamodb service's input validation (the component that renders `Member must satisfy enum value set: [...]`) must emit the value set in AWS's order for the member rather than iterating the Smithy enum's declared values; UpdateTable's StreamSpecification validation is the call site this check exercises.
Traces: dynamodb.update-table-misspelled-stream-view-type-rejected-and-enables-no-stream.

## Evidence
Check dynamodb.update-table-misspelled-stream-view-type-rejected-and-enables-no-stream (verdict: mismatch):
AssertionError: enabling a stream with the misspelled view type gave ('ValidationException', "1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'streamSpecification.streamViewType' failed to satisfy constraint: Member must satisfy enum value set: [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY]")

Expected (AWS), code and message:
  code:    ValidationException (HTTP 400)
  message: 1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'streamSpecification.streamViewType' failed to satisfy constraint: Member must satisfy enum value set: [OLD_IMAGE, KEYS_ONLY, NEW_AND_OLD_IMAGES, NEW_IMAGE]

Actual (emulator):
  code:    ValidationException (matches)
  message: 1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'streamSpecification.streamViewType' failed to satisfy constraint: Member must satisfy enum value set: [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY]

The only divergence is the order of the bracketed enum value set: the emulator emits the Smithy declaration order (confirmed by the on-disk model: com.amazonaws.dynamodb#StreamViewType values = [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY]), while AWS serves [OLD_IMAGE, KEYS_ONLY, NEW_AND_OLD_IMAGES, NEW_IMAGE]. Everything else in the sentence matches, including the echoed rejected value and the lower-camel member path.

## Steps to Reproduce
1. CreateTable with BillingMode=PAY_PER_REQUEST, KeySchema id(HASH,S), no StreamSpecification; poll DescribeTable until TableStatus=ACTIVE.
2. Call UpdateTable(TableName=<name>, StreamSpecification={StreamEnabled: true, StreamViewType: "NEW_AND_OLD_IMAGE"}) — a misspelling of NEW_AND_OLD_IMAGES.
3. Expected: ValidationException (HTTP 400) with message `1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'streamSpecification.streamViewType' failed to satisfy constraint: Member must satisfy enum value set: [OLD_IMAGE, KEYS_ONLY, NEW_AND_OLD_IMAGES, NEW_IMAGE]`. Observe the same code but the value set emitted as [NEW_IMAGE, OLD_IMAGE, NEW_AND_OLD_IMAGES, KEYS_ONLY].
4. (Blocked by step 3, must also hold) DescribeTable must carry neither StreamSpecification nor LatestStreamArn, and dynamodbstreams ListStreams(TableName=<name>) must return []. Then UpdateTable with StreamViewType=NEW_AND_OLD_IMAGES, wait ACTIVE: DescribeTable must report StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES} plus a LatestStreamArn, ListStreams must report exactly that one stream, and DescribeStream must report StreamViewType NEW_AND_OLD_IMAGES.
