Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# CreateResourceSet accepts a ResourceSetType the API does not support

_Filed: 2026-10-24T15:51:31Z_
_Operation: CreateResourceSet | Class: validation_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `ResourceSetType` member documents an exhaustive value list (the 20 types the readiness feature can audit: AWS::ApiGateway::Stage, AWS::DynamoDB::Table, AWS::SQS::Queue, ... AWS::Route53RecoveryReadiness::DNSTargetResource). The handler validates only the modelled `@pattern` `^AWS::[A-Za-z0-9]+::[A-Za-z0-9]+$` (applied by the boundary validator) plus the name/resource/tag count ceilings, so any pattern-shaped-but-unsupported value — e.g. `AWS::S3::Bucket`, `AWS::Foo::Bar` — is accepted with HTTP 200 and persisted.

Expected: HTTP 400 ValidationException refusing the unknown resource type, the same refusal the service's own readiness-rule surface gives (the emulator's `ListRules` already answers 400 with the constant `Unknown resource type.` for exactly these values).

Actual: 200, the record is stored, and a later `GetResourceSet` / `ListResourceSets` reports a resource set whose type names no type the service has readiness rules for — the emulator is internally inconsistent, since `GET /rules?resourceType=AWS::S3::Bucket` refuses the identical value with 400.

## Evidence

Spec: `smithy_query(query="doc", shape="CreateResourceSetRequest", member="ResourceSetType")` enumerates the supported values ("Enter one of the following values for resource type: ..."); the same closed set is what `ListRules` reports rules for.

Handler: `src/ops/create_resource_set.rs` lines 175-192 — validates `ResourceSetName` (`state::validate_name`), `Resources` and `Tags` counts, then stores `resource_set_type: input.resource_set_type` (line 244) with no type check. The crate already owns the check: `state::SUPPORTED_RESOURCE_TYPES` / `state::validate_resource_type` + `state::UNKNOWN_RESOURCE_TYPE_MESSAGE` (src/state.rs:149-202), used only from `src/ops/list_rules.rs:909`.

Tests: `tests/all/readiness_check_lifecycle.rs:678` asserts all 20 documented types are accepted, and `:709` asserts only values that break the `@pattern` (`aws::dynamodb::table`, `DynamoDB::Table`, `AWS::DynamoDB`, `AWS::Dynamo_DB::Table`, ``) are refused. No test covers a pattern-valid but unsupported type, so this gap is uncovered.

## Steps to Reproduce

1. POST /resourcesets with body {"resourceSetName":"unsupported_type","resourceSetType":"AWS::S3::Bucket","resources":[{"resourceArn":"arn:aws:s3:::bucket-1"}]} (with ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE=1 so the account gate allows the create)
2. Observe HTTP 200 instead of 400 ValidationException
3. GET /resourcesets/unsupported_type — the record is persisted and reported with resourceSetType "AWS::S3::Bucket"
4. GET /rules?resourceType=AWS::S3::Bucket — the same value is refused with 400 ValidationException ("Unknown resource type."), showing the two surfaces disagree about whether the type exists
