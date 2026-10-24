Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# CreateSchema accepts a SchemaName outside the service's name constraints

_Filed: 2026-10-24T05:42:24Z_
_Operation: CreateSchema | Class: validation_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SchemaName` is taken from the URI label and stored unchecked. Real EventBridge Schemas validates the schema name (letters, digits, '.', '-', '_', '@'; at most 385 characters) and refuses anything else with a 400 BadRequestException before the schema is created — the same parameter-pattern refusal class the service uses for its other URI labels.

Expected: CreateSchema with a name containing e.g. a space, '/', '#', '$' or longer than 385 characters -> HTTP 400 BadRequestException and no schema stored.
Actual: HTTP 201, the schema and its first version are persisted, and DescribeSchema/ListSchemas/SearchSchemas subsequently report a schema whose name AWS would have rejected (its minted SchemaArn embeds the invalid name).

## Evidence

Spec: `smithy_query(query="shape", shape="CreateSchemaRequest")` — `SchemaName` targets the unconstrained `__string` (@httpLabel, @required), so nothing is enforced at the boundary; the constraint is the service's. The crate itself documents that this service pattern-validates URI-label parameters and answers 400 ('Parameter CodeBindingLanguage doesn't match pattern ...', src/state.rs:2160-2185). Well-established constraint: schema names are at most 385 characters of [A-Za-z0-9._@-] (the constraint CloudFormation AWS::EventSchemas::Schema and the Terraform aws_schemas_schema resource publish for `name`, and the shape of every AWS-published name, e.g. 'aws.ec2@EC2InstanceStateChangeNotification').
Handler: services/service-com-amazonaws-schemas-schemas/src/ops/create_schema.rs:157-273 validates only `Content` (JSON object) plus registry existence, managed-registry ownership, duplicate name and store caps — the name itself is never checked before `registry.schemas.insert(input.schema_name.clone(), schema)`.
Tests: tests/all/registry_schema_lifecycle.rs and tests/all/malformed_content_and_policy_deletion.rs cover content refusals and duplicates only; no test sends a malformed schema name.

## Steps to Reproduce

1. POST /v1/registries/name/reg (CreateRegistry) -> 201
2. POST /v1/registries/name/reg/schemas/name/bad%20name%23 with {"Content":"{}","Type":"OpenApi3"} -> observe 201 instead of 400 BadRequestException
3. GET /v1/registries/name/reg/schemas/name/bad%20name%23 -> observe 200 with SchemaArn 'arn:aws:schemas:us-east-1:<acct>:schema/reg/bad name#'
4. Repeat with a 400-character schema name -> observe 201 instead of a 400 length refusal
