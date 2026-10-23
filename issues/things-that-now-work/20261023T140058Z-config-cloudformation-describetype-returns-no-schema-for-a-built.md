Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.put-resource-config-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.put-resource-config-numeric-precision-preserved|run#5|AssertionError|assert native_type.get("Schema"), (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudFormation DescribeType returns no Schema for a built-in resource type (AWS::S3::Bucket), reached through a Config check

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeType

## Description
ONE CAUSE must be closed. The failing check id is a Config check (config.put-resource-config-numeric-precision-preserved), which is why this issue is filed under the config service token, but the handler to fix is CloudFormation's.

Cause 1 of 1: DescribeType for a public AWS-provided resource type answers without the type's schema.
AWS: recorded behavior only — cloudformation DescribeType with Type=RESOURCE and TypeName=AWS::S3::Bucket returns the type's metadata including a non-empty Schema (the JSON resource schema), which is what Config's PutResourceConfig contract is validated against (a resource type must be registered in the CloudFormation registry and its schema is the authority for the Configuration payload).
Emulator: the response carries no Schema (falsy), so the positive control of the Config check cannot be established and nothing downstream can be attributed.
Why: a client that reads the registry to discover a resource type's schema — the documented way to learn which properties a type accepts, and the precondition Config's PutResourceConfig validation depends on — gets an answer with the schema missing, so it cannot distinguish "type not registered" from "registered but schema unavailable".
Fix: the CloudFormation DescribeType handler and the built-in resource-type registry it reads, which must return the Schema for public AWS::* types (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_type.rs and the registry/state it serves).
Traces: config.put-resource-config-numeric-precision-preserved.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

config.put-resource-config-numeric-precision-preserved:
  AssertionError: cloudformation:DescribeType returned no schema for AWS::S3::Bucket -- the positive control failed, so the rejection below cannot be attributed
  (at `assert native_type.get("Schema")` on the response of cloudformation describe-type for Type=RESOURCE, TypeName=AWS::S3::Bucket)

Expected: a 200 response whose Schema member carries the resource schema for AWS::S3::Bucket.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. cloudformation describe-type --type RESOURCE --type-name AWS::S3::Bucket — the response has no (or an empty) Schema member.
3. Replay simparity-behavioral-aws-v1 and re-replay the check id in trace_ids.
