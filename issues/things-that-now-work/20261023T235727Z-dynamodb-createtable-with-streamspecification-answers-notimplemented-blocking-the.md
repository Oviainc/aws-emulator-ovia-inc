Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target
Issue-Key: dynamodb__lambda__pipes__sqs|CreateTable|NotImplemented
Issue-Commit: cfd0674e2ddfe499cbab1f11b70f1bc18d331a71

# DynamoDB CreateTable with StreamSpecification answers NotImplemented, blocking the two Pipes-enrichment DynamoDB-stream checks that no filed issue carries

> _Note: this issue was identified against the code base at commit `cfd0674e2ddfe499cbab1f11b70f1bc18d331a71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
This bundle has 1 cause.

Cause 1 of 1: CreateTable refuses any request carrying StreamSpecification with a non-modeled NotImplemented error, so a DynamoDB-stream source for an EventBridge Pipes enrichment topology cannot be created.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTableInput has an optional StreamSpecification member (structure with required StreamEnabled and StreamViewType, one of KEYS_ONLY | NEW_IMAGE | OLD_IMAGE | NEW_AND_OLD_IMAGES); CreateTable's modeled errors are ResourceInUseException, LimitExceededException, InternalServerError and InvalidEndpointException — NotImplemented is not among them. A successful CreateTable returns 200 with a TableDescription echoing StreamSpecification and, once the stream is active, LatestStreamArn/LatestStreamLabel, which is what a pipe's DynamoDB stream source resolves.
Emulator: every CreateTable that includes StreamSpecification fails with botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator. CreateTable without StreamSpecification is served normally, so this is a per-member refusal inside an otherwise-implemented operation.
Why: the two checks below never reach their first assertion, so nothing about pipes enrichment over a DynamoDB stream (stream-filter dropping MODIFY/REMOVE before enrichment, and NewImage being computed at the target) can be observed; more generally no change-data-capture topology over DynamoDB can be built.
Fix: the DynamoDB CreateTable handler and the table state behind it — accept StreamSpecification, persist it, echo it in TableDescription, mint LatestStreamArn/LatestStreamLabel and register the stream so dynamodbstreams and a pipes DynamoDB-stream source can resolve it.
Traces: dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment, dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target.
Relationship to what is already filed: the open issue 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md describes this identical root cause (its Issue-Key is `dynamodb|CreateTable|NotImplemented`) and should be fixed once, not twice — but its replay keys cover the suite directories dynamodb, dynamodb__kinesis__lambda, dynamodb__lambda, dynamodb__lambda__s3, dynamodb__pipes__sqs and dynamodbstreams, and NOT dynamodb__lambda__pipes__sqs. That is why the suite marked the two dynamodb__pipes__sqs checks of this run OWNED by it and left these two unmarked. This issue exists to carry those two check ids so they are re-replayed when the fix lands; close it together with that issue.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 12/107 passed (11.2%); dynamodb__lambda__pipes__sqs 0/2 (0%), both failed. The emulator was serving (no bootstrap signal; CreateTable without StreamSpecification, SQS and Lambda calls answered normally elsewhere in this run).

Both checks fail on the first CreateTable call of the check, before any pipes assertion:
  dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator
  dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target — identical error.
  Expected: HTTP 200 with TableDescription echoing StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES} and a LatestStreamArn once the stream is active.
  Actual: error code NotImplemented, which CreateTable does not model.
  Neither check was marked ALREADY FILED or OWNED in this run's report, while the two dynamodb__pipes__sqs checks were marked OWNED by 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md.

Table names, stream ARNs and labels are generated per run and are examples only; the requirement is that CreateTable with StreamSpecification succeeds and the stream becomes resolvable.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with AttributeDefinitions [{pk, S}], KeySchema [{pk, HASH}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}. Observe the call fail with error code NotImplemented instead of 200 with a TableDescription.
3. Re-replay dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-filter-drops-modify-and-remove-before-enrichment and dynamodb__lambda__pipes__sqs.events-pipes-enrichment-dynamodb-stream-newimage-computed-at-target; both abort on that first CreateTable.
4. After the stream-enablement fix lands (see 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md), re-replay these two check ids as well — they additionally need the pipes DynamoDB-stream source and enrichment runtime filed separately in this run.
