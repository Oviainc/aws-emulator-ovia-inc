Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-esm-sqs-fifo-batch-size-above-ten-rejected
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-esm-sqs-fifo-batch-size-above-ten-rejected lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker lambda-lsrno-traj-esm-role-gap-then-lookup-by-function-and-queue lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream lambda-rmq-traj-find-the-mapping-from-the-broker-side lambda-snx-traj-sns-trigger-is-not-an-event-source-mapping lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|ListEventSourceMappings|2xx->2xx|$.EventSourceMappings[*]
Issue-Key: lambda|ListEventSourceMappings|2xx->4xx|$.EventSourceMappings
Issue-Key: lambda|ListEventSourceMappings|2xx->4xx|status
Issue-Key: lambda|ListEventSourceMappings|4xx->2xx|$.Type
Issue-Key: lambda|ListEventSourceMappings|4xx->2xx|$.message
Issue-Key: lambda|ListEventSourceMappings|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListEventSourceMappings|4xx->2xx|status
Issue-Key: lambda|UpdateEventSourceMapping|4xx->2xx|$.Type
Issue-Key: lambda|UpdateEventSourceMapping|4xx->2xx|$.message
Issue-Key: lambda|UpdateEventSourceMapping|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateEventSourceMapping|4xx->2xx|status
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.BatchSize
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.BisectBatchOnFunctionError
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.DestinationConfig
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.EventSourceArn
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.EventSourceMappingArn
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.FilterCriteria
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.FunctionArn
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.FunctionResponseTypes
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.LastModified
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.LastProcessingResult
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.MaximumBatchingWindowInSeconds
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.MaximumRecordAgeInSeconds
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.MaximumRetryAttempts
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.ParallelizationFactor
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.StartingPosition
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.State
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.StateTransitionReason
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.TumblingWindowInSeconds
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|$.UUID
Issue-Key: lambda|UpdateEventSourceMapping|2xx->4xx|status
Issue-Key: lambda|CreateEventSourceMapping|4xx->4xx|$.Message
Issue-Related: 20261029T233239Z-lambda-event-source-mappings-are-created-for-sources.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda event source mapping validation and lookup refusals diverge

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventSourceMappings

## Description
The event-source-mapping plane accepts configurations AWS refuses, refuses calls AWS accepts, and answers the wrong error class for an unknown source. 6 causes, all in the Create/Update/ListEventSourceMappings validation and lookup paths.

Cause 1 of 6 - a BatchSize above the FIFO-queue ceiling is accepted, so the mapping exists.
AWS: Smithy: CreateEventSourceMapping input BatchSize has range 1..10000, but for an SQS FIFO queue source AWS caps the batch size at 10 and refuses a larger value with InvalidParameterValueException (HTTP 400), so no mapping is created and the subsequent ListEventSourceMappings returns an empty EventSourceMappings list.
Emulator: creates the mapping with BatchSize=11 and lists it.
Why: a client that relies on the refusal to catch an invalid FIFO batch size gets a mapping AWS would never create; its FIFO ordering guarantees then do not hold.
Fix: the CreateEventSourceMapping handler's source-type-aware BatchSize validation.
Traces: lambda-esm-sqs-fifo-batch-size-above-ten-rejected#task#9 (3 steps affected).

Cause 2 of 6 - ListEventSourceMappings answers 404 where AWS returns an empty list.
AWS: Smithy: ListEventSourceMappings output EventSourceMappings. A filter (by FunctionName or by EventSourceArn) that matches nothing is not an error: AWS answers HTTP 200 with EventSourceMappings=[].
Emulator: answers HTTP 404 with no EventSourceMappings member at all.
Why: a client that enumerates a function's or a queue's mappings to decide whether one exists treats the 404 as a failure and aborts, instead of reading the empty list and proceeding to create one.
Fix: the ListEventSourceMappings handler's empty-result path (and the resolver that currently 404s when the filter target has no mappings).
Traces: lambda-lsrno-traj-esm-role-gap-then-lookup-by-function-and-queue#task#15 (3 steps affected).

Cause 3 of 6 - an unrecognized EventSourceArn is accepted instead of refused.
AWS: ListEventSourceMappings declares InvalidParameterValueException (400). An EventSourceArn that is not a recognisable event-source form is refused with HTTP 400, Type 'User' and the message 'Unrecognized event source. Expected arn of format arn:aws:<service>:<region>:<account-id>:<resource-path>' - fixed text ending in the expected ARN template for the source service AWS inferred from the request (for a DynamoDB stream: arn:aws:dynamodb:<region>:<account-id>:table/TableName/stream/StreamId).
Emulator: answers HTTP 200 with a result set, treating the unusable ARN as a filter that simply matches nothing.
Why: a client that passes a table ARN where a stream ARN is required sees an empty, successful listing and concludes no mapping exists, rather than being told the ARN form is wrong.
Fix: the ListEventSourceMappings handler's EventSourceArn validation.
Traces: lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream#task#9 (3 steps affected).

Cause 4 of 6 - UpdateEventSourceMapping accepts an invalid filter pattern.
AWS: Smithy: UpdateEventSourceMapping input FilterCriteria; the operation declares InvalidParameterValueException (400). A filter whose Pattern is not a valid filter document is refused with HTTP 400, Type 'User', x-amzn-errortype InvalidParameterValueException and the message 'Invalid filter pattern definition.' (fixed text, no variable parts).
Emulator: answers HTTP 202 and stores the malformed pattern.
Why: a client gets a mapping whose filter cannot match, so the source is silently never consumed, instead of a refusal naming the bad pattern.
Fix: the UpdateEventSourceMapping handler's FilterCriteria validation.
Traces: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#8.

Cause 5 of 6 - the corrected UpdateEventSourceMapping is then refused 400.
AWS: the valid update is accepted with HTTP 202 and returns the full EventSourceMappingConfiguration (UUID, StartingPosition, BatchSize, FilterCriteria with the new pattern, EventSourceArn, FunctionArn, EventSourceMappingArn, State='Updating', StateTransitionReason='User action' and the rest).
Emulator: answers HTTP 400 with none of the modelled output members.
Why: after the malformed filter of cause 4 was accepted, the mapping cannot be corrected at all - the repair path is closed.
Fix: the UpdateEventSourceMapping handler's FilterCriteria handling (the stored malformed pattern must not make later valid updates unparseable).
Traces: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#9.

Cause 6 of 6 - an unknown broker event source is answered 409 instead of 404.
AWS: Smithy: CreateEventSourceMapping declares both ResourceNotFoundException (404) and ResourceConflictException (409). An EventSourceArn naming a broker that does not exist is a not-found condition: HTTP 404 with the message 'The broker resource: eventSourceArn you requested does not exist.' (fixed text, naming the request member).
Emulator: answers HTTP 409 with x-amzn-errortype ResourceConflictException and no message.
Why: a client distinguishing 'the source does not exist, create it first' from 'a conflicting mapping already exists, delete it first' takes the wrong remediation branch.
Fix: the CreateEventSourceMapping handler's event-source resolution, which must classify an unknown broker as not-found before the conflict check.
Traces: lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker#task#7.

## Evidence
Cause 1: lambda-esm-sqs-fifo-batch-size-above-ten-rejected#task#9 (200 vs 200) - $.EventSourceMappings[0] expected absent, got a mapping with BatchSize=11 on a .fifo queue (3 steps affected).
Cause 2: lambda-lsrno-traj-esm-role-gap-then-lookup-by-function-and-queue#task#15 - expected 200 with $.EventSourceMappings=[]; got 404 with the member absent (3 steps affected).
Cause 3: lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream#task#9 - expected 400 + $.Type='User' + $.message='Unrecognized event source. Expected arn of format arn:aws:dynamodb:<region>:<account>:table/TableName/stream/StreamId'; got 200 (3 steps affected).
Cause 4: lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping#task#8 - expected 400 + $.Type='User' + $.message='Invalid filter pattern definition.' + x-amzn-errortype; got 202.
Cause 5: #task#9 - expected 202 with the full EventSourceMappingConfiguration (19 members asserted); got 400 with all of them absent.
Cause 6: lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker#task#7 - expected 404 + $.Message='The broker resource: eventSourceArn you requested does not exist.'; got 409 ResourceConflictException with no message.

## Steps to Reproduce
1. Replay lambda-esm-sqs-fifo-batch-size-above-ten-rejected and check task#9's listing after the BatchSize=11 create against a .fifo queue.
2. Replay lambda-lsrno-traj-esm-role-gap-then-lookup-by-function-and-queue task#15 (ListEventSourceMappings filtered to a function/queue with no mappings).
3. Replay lambda-onyx-traj-stream-mapping-discovery-by-function-and-by-stream task#9 (ListEventSourceMappings with a table ARN instead of a stream ARN).
4. Replay lambda-onyx-traj-filter-criteria-fix-attempt-on-the-stream-mapping task#8 then task#9 (malformed filter, then the corrected one).
5. Replay lambda-rmq-traj-recreate-mapping-role-cannot-describe-broker task#7 (CreateEventSourceMapping against a non-existent broker).
