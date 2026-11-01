Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# RegisterApplication accepts reserved aws:-prefixed tag keys and never resolves the EC2 instances / database it is given

_Filed: 2026-10-24T15:17:25Z_
_Operation: RegisterApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two refusals the real service makes are missing, both leaving unusable state behind:

1. Reserved tag keys. The model gives TagKey the pattern ^(?!aws:)[a-zA-Z+-=._:/]+$ — i.e. a key may not start with the reserved aws: prefix. That pattern contains a negative lookahead, which the boundary validator cannot compile, and the framework treats an uncompilable @pattern as MATCHING (vendor/smithy-schema-server/src/constraints.rs: `let Ok(re) = regex::Regex::new(regex) else { return true; }`). Nothing in the handler re-states the rule, so `Tags: {"aws:anything": "x"}` is stored verbatim and then reported by GetApplication/ListApplications, where real AWS refuses the registration with ValidationException. TagResource is still a scaffold, so RegisterApplication is the only tag entry point and the only place this can be caught.

2. Unresolved EC2 / database references. Instances (InstanceId, ^i-...$), ComponentsInfo[].Ec2InstanceId and DatabaseArn are stored exactly as given, with no check that the instance or database exists. The documented requirement for the call is that the SAP application is running on an Amazon EC2 instance with SSM Agent configured; registering against an instance id that names no instance answers 200 and persists an application pointing at nothing, so the ResourceNotFoundException the operation models (HTTP 404) is unreachable for every input.

## Evidence

Smithy model: TagMap key -> TagKey with pattern ^(?!aws:)[a-zA-Z+-=._:/]+$; RegisterApplication input Instances (InstanceList of InstanceId, pattern ^i-[\w\d]{8}$|^i-[\w\d]{17}$), ComponentsInfo[].Ec2InstanceId (InstanceId, required), DatabaseArn; RegisterApplication declares ResourceNotFoundException (404) and ValidationException (400). Handler: src/ops/register_application.rs validates only ApplicationType and the type-conditional members, then `region_state.tags_by_arn.insert(arn, tags)` and stores `instances: input.instances` / `database_arn: input.database_arn` with no resolution; src/state.rs has no tag-key or instance validation. Framework: vendor/smithy-schema-server/src/constraints.rs pattern_matches returns true for a pattern that fails to compile (the lookahead case). Tests: tests/all/application_lifecycle.rs and tests/all/register_application_conditional_requirements.rs register with made-up instance ids and ordinary tag keys; no test asserts a refusal for an aws: key or for an instance/database that does not exist.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"TAGTEST","ApplicationType":"SAP_ABAP","Instances":["i-00000000000000001"],"Tags":{"aws:cloudformation:stack-name":"x"}}
2. Observe 200 and then POST /get-application {"ApplicationId":"TAGTEST"} reporting the reserved aws: tag, where real AWS refuses the register with ValidationException (reserved tag-key prefix)
3. POST /register-application with {"ApplicationId":"NOINST","ApplicationType":"HANA","Sid":"HDB","SapInstanceNumber":"00","Instances":["i-0deadbeefdeadbeef"],"Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"sap/hdb/admin"}],"DatabaseArn":"arn:aws:ssm-sap:us-east-1:000000000000:database/NOPE/NOPE"}
4. Observe 200 with a fully registered application although neither the EC2 instance nor the database exists; the modeled ResourceNotFoundException is never produced
