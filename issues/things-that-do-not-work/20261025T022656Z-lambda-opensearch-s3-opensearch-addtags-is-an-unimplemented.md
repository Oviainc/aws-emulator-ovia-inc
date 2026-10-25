Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.s3-notification-filter-suffix-positive lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative
Issue-Key: lambda__opensearch__s3|AddTags|NotImplemented
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__opensearch__s3: OpenSearch AddTags is an unimplemented scaffold stub, blocking both S3-notification scenarios (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
Cause 1 of 1: es:AddTags answers NotImplemented, so the OpenSearch domain the two S3-notification checks need cannot be set up and neither the suffix-filter positive nor the write-failure negative ever runs.

AWS: Smithy (opensearch) AddTags attaches a TagList to a domain ARN and answers 200 with an empty body; its modeled refusals are client errors — BaseException/ValidationException, LimitExceededException, ResourceNotFoundException and InternalException.

Emulator: the call is refused with code 'NotImplemented' and message 'operation AddTags is scaffolded but not implemented in this emulator' (recorded message, an example).

Why: NotImplemented is not in the operation's modeled error set, so a client cannot tag a domain (the normal way to label and discover it) and any provisioning script that tags what it creates aborts at that step, taking unrelated downstream behaviour with it.

Fix: implement the AddTags handler in the opensearch service (domain ARN resolution + tag store), alongside ListTags/RemoveTags.

Traces: lambda__opensearch__s3.s3-notification-filter-suffix-positive, lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative.

## Evidence
Expected: AddTags(ARN=<domain arn>, TagList=[...]) answers 200.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddTags operation: operation AddTags is scaffolded but not implemented in this emulator

## Steps to Reproduce
1. CreateDomain in the opensearch service.
2. Call AddTags with the domain ARN and a tag list.
3. Observe NotImplemented.
