Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis.ddbkds-enable-without-precision-leaves-the-precision-key-absent dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records
Issue-Key: dynamodb__kinesis.ddbkds-enable-without-precision-leaves-the-precision-key-absent|run#9|AssertionError|assert resp.get("EnableKinesisStreamingConfiguration") == {}, (
Issue-Key: dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records|_pump#14|Timeout|time.sleep(2.0)
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# dynamodb__kinesis: EnableKinesisStreamingDestination omits EnableKinesisStreamingConfiguration from its response, and an ACTIVE Kinesis streaming destination delivers no item-change records to the stream (2 causes)

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableKinesisStreamingDestination

## Description
Two net-new root causes on the DynamoDB-to-Kinesis streaming-destination surface, both grounded in failing behavioral checks the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (164/458 checks pass, bootstrap_signal false), so these are fidelity divergences. Both causes must be closed before this issue is submitted. Model citations are from services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

=== Cause 1 of 2: EnableKinesisStreamingDestination's response omits the EnableKinesisStreamingConfiguration member entirely ===
AWS: Smithy: KinesisStreamingDestinationOutput models TableName, StreamArn, DestinationStatus and EnableKinesisStreamingConfiguration (a structure whose only member is the ApproximateCreationDateTimePrecision enum, MILLISECOND or MICROSECOND). Recorded behavior: an Enable that names NO configuration still echoes the member, as an EMPTY structure (`{}`), and the resulting ACTIVE destination then describes no ApproximateCreationDateTimePrecision key at all; an Enable that DOES name a precision echoes it. The requirement is the presence/absence rule, not any one precision value: the member is always present on the response, and the precision key inside it is present only when the request named one.
Emulator: the response omits EnableKinesisStreamingConfiguration — the member reads back as absent (None) where an empty structure was owed.
Why: a client cannot distinguish 'no precision configured' (empty structure) from 'the service did not report its configuration', and code that reads response['EnableKinesisStreamingConfiguration'] to learn the effective precision raises a KeyError against the emulator and works against AWS.
Fix: the EnableKinesisStreamingDestination output serializer must always emit the EnableKinesisStreamingConfiguration structure, empty when the request carried no configuration (the EnableKinesisStreamingDestination handler and the destination record in the dynamodb service's table state). Note: the OPEN issue 20261025T024002Z-dynamodb-enablekinesisstreamingdestination-stub.md lists this check id but is keyed to the operation's 501/NotImplemented refusal; the operation now answers with real behavior, so this response-shape defect is net-new work layered on that stub.
Traces: dynamodb__kinesis.ddbkds-enable-without-precision-leaves-the-precision-key-absent.

=== Cause 2 of 2: an ACTIVE Kinesis streaming destination never delivers an item-change record to the Kinesis stream ===
AWS: the purpose of the streaming destination is that every item create/update/delete on the table is published as a record on the configured Kinesis data stream, carrying the item's primary key and the before/after images together with an approximate creation timestamp (DynamoDB Developer Guide, Kinesis Data Streams for DynamoDB; the guide describes the record CONTENT rather than the JSON envelope, and Kinesis-for-DynamoDB is explicitly unordered and may duplicate, so delivery is asserted as a de-duplicated SET of changes, never a sequence). Recorded behavior: after the destination reaches ACTIVE, three mutations of one item all arrive on the stream within the check's delivery budget and are readable via GetShardIterator/GetRecords.
Emulator: nothing is ever delivered — the check does not fail an assertion on a record's shape, it hangs until the harness cuts it off ('Error: Check timed out after 60.0s') while waiting for the first record to appear on the stream. The destination reports ACTIVE while the data plane behind it does not exist.
Why: this is the entire point of the feature. A client wires a table to a Kinesis stream, sees ACTIVE, writes items, and reads an empty stream forever; every downstream consumer (analytics, fan-out, replication) is silently starved, and the DISABLED-stops-delivery half of the contract cannot be observed either because nothing is delivered in the first place.
Fix: the item-write path must publish a change record to the configured Kinesis stream for every create/update/delete while the destination is ACTIVE, and stop once it is DISABLED — the dynamodb write handlers plus the streaming-destination publisher that writes into the kinesis service's stream state.
Traces: dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 164/458 passed (35.8%), dynamodb__kinesis 1/5. bootstrap_signal was false. Both failing checks were listed WITHOUT an 'ALREADY FILED' or 'OWNED by' marker.

Cause 1 — dynamodb__kinesis.ddbkds-enable-without-precision-leaves-the-precision-key-absent [mismatch]:
  AssertionError: an Enable with no configuration must echo an empty configuration, got None.
  Expected: EnableKinesisStreamingDestination response carries EnableKinesisStreamingConfiguration == {}; actual: the member is absent from the response.

Cause 2 — dynamodb__kinesis.kinesis-streaming-destination-delivers-item-change-records [mismatch]:
  Error: Check timed out after 60.0s. The check creates the table and stream, enables the destination, waits for ACTIVE, mutates one item three times and polls the Kinesis shard for the resulting records; it never reaches any record assertion because no record is ever delivered. Expected: the de-duplicated set of delivered change events covers the three mutations, with the item's key and the before/after images; actual: an empty stream for the whole 60 s budget.

## Steps to Reproduce
1. Build and boot the emulator, then replay the two check ids in trace_ids.
2. Cause 1: CreateStream in kinesis, CreateTable in dynamodb, then EnableKinesisStreamingDestination(TableName, StreamArn) with NO EnableKinesisStreamingConfiguration in the request -> inspect the response for the EnableKinesisStreamingConfiguration member (owed: present and empty; actual: absent). Repeat with ApproximateCreationDateTimePrecision set and confirm the echo, then DescribeKinesisStreamingDestination.
3. Cause 2: with the destination polled to DestinationStatus ACTIVE, PutItem / UpdateItem / DeleteItem one item, then GetShardIterator(LATEST or TRIM_HORIZON) + GetRecords on the stream's shard -> observe no records are ever returned. Then DisableKinesisStreamingDestination, wait for DISABLED, write again and confirm the (already empty) delivery.
