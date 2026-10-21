Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MediaStore emulator

_Filed: 2026-10-21T21:48:13Z_
_Operation: service-com-amazonaws-mediastore-mediastore-20170901 | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in `services/service-com-amazonaws-mediastore-mediastore-20170901/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. No client workflow can run.

Implement a FIRST, SMALL, stateful slice: the container lifecycle plus one policy round-trip. Scope is exactly these six operations, in this order (wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: MediaStore_20170901.<OperationName>`):

1. **CreateContainer** — Input: `ContainerName` (required, len 1..255, `^[\w-]+$`), `Tags` (list<Tag>, len 1..200). Output: `Container` (required). Must INSERT a new container into state and return the stored record: `Name` = the requested name, a generated `ARN` matching `^arn:aws:mediastore:[a-z]+-[a-z]+-\d:\d{12}:container/[\w-]{1,255}$` (use the request's region and account), a generated `Endpoint` (e.g. `https://<hash>.data.mediastore.<region>.amazonaws.com`), `CreationTime` = now, `Status` = `CREATING` or `ACTIVE`, `AccessLoggingEnabled` = false. Re-creating an existing name must fail with `ContainerInUseException` (HTTP 400), not overwrite.
2. **DescribeContainer** — Input: `ContainerName` (optional in the model). Output: `Container`. Must look the container up in state and return the SAME `ARN`, `Endpoint`, `CreationTime`, `Name`, `Status` and `AccessLoggingEnabled` that CreateContainer stored. Unknown name → `ContainerNotFoundException` (HTTP 400).
3. **ListContainers** — Input: `NextToken` (len 1..1024, `^[0-9A-Za-z=/+]+$`), `MaxResults` (1..100). Output: `Containers` (list<Container>, required), `NextToken`. Must enumerate the containers actually in state in a stable order, honour `MaxResults` as a page size, and emit `NextToken` only when more remain; passing that token must continue where the previous page stopped and must not repeat or skip entries.
4. **PutContainerPolicy** — Input: `ContainerName` (required), `Policy` (required, len 1..8192, `^[\x00-\x7F]+$`). Output: no members. Must STORE the policy string on the named container. Unknown container → `ContainerNotFoundException`.
5. **GetContainerPolicy** — Input: `ContainerName` (required). Output: `Policy` (required). Must return the byte-identical policy stored by PutContainerPolicy. If the container exists but no policy was ever put (or it was deleted) → `PolicyNotFoundException` (HTTP 400). Unknown container → `ContainerNotFoundException`.
6. **DeleteContainer** — Input: `ContainerName` (required). Output: no members. Must REMOVE the container (and its attached policy/tags) from state. Unknown name → `ContainerNotFoundException`. After deletion, DescribeContainer must fail with `ContainerNotFoundException` and ListContainers must no longer include it.

NEVER STUBS: each of these six must compute its response from persisted state. A hardcoded `Container`, an echoed `ContainerName` without storage, or a generated ARN/endpoint that is not remembered for the next DescribeContainer call is a failure of this issue. Each handler must reach the shared store via `crate::state::service_state(&*self.engine)` (see README.md "State") — never construct a fresh engine.

Validation in scope for these six: enforce the modeled `@required` members, the `ContainerName` length 1..255 and `^[\w-]+$` pattern, the `Policy` length/charset, and `MaxResults` range 1..100, returning a 400 validation error rather than accepting garbage.

## State Model

In `src/state.rs`, on the service state struct:

- `containers: BTreeMap<String /* ContainerName */, Container>` — ordered by name so ListContainers paging is stable.
- `struct Container { name: String, arn: String, endpoint: String, creation_time: SystemTime, status: ContainerStatus /* Creating | Active | Deleting */, access_logging_enabled: bool, policy: Option<String>, cors_policy: Option<Vec<CorsRule>>, lifecycle_policy: Option<String>, metric_policy: Option<MetricPolicy>, tags: Vec<Tag /* { key: String, value: Option<String> } */> }`

Only `name`, `arn`, `endpoint`, `creation_time`, `status`, `access_logging_enabled`, `policy` and `tags` are exercised by this first slice; the other fields are declared now so the later policy operations (CORS, lifecycle, metric) and tag operations attach to the same record without reshaping state. ListContainers pagination: derive the opaque `NextToken` from the last returned container name (e.g. base64 of the name, matching `^[0-9A-Za-z=/+]+$`) and resume the `BTreeMap` range after it.

The remaining 15 operations (DeleteContainerPolicy, Put/Get/DeleteCorsPolicy, Put/Get/DeleteLifecyclePolicy, Put/Get/DeleteMetricPolicy, Start/StopAccessLogging, TagResource, UntagResource, ListTagsForResource) stay scaffolded at 501 and will be filed as individual gap issues later.

Done = a client script can create a container, describe it and see the same ARN/endpoint/creation time back, see it in ListContainers, put and get a policy verbatim, delete the container, and then observe `ContainerNotFoundException` — with all state surviving across separate HTTP requests.

## Evidence

`services/service-com-amazonaws-mediastore-mediastore-20170901/OPERATIONS.md` lists all 21 operations with their input/output members and errors: CreateContainer (`ContainerName` required + `Tags`; output `Container` required; errors ContainerInUseException/LimitExceededException/InternalServerError), DescribeContainer (output `Container`; ContainerNotFoundException), ListContainers (@paginated inputToken=NextToken outputToken=NextToken, `MaxResults` range 1..100, output `Containers` required), PutContainerPolicy / GetContainerPolicy (`Policy` len 1..8192 pattern `^[\x00-\x7F]+$`, PolicyNotFoundException on get), DeleteContainer (ContainerNotFoundException). The `Container` shape in the Smithy model (queried via smithy_query) has members `Endpoint`, `CreationTime`, `ARN`, `Name`, `Status` (enum CREATING/ACTIVE/DELETING), `AccessLoggingEnabled` — these are the fields that must be persisted and returned. `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 21 operation files, so no operation has any behavior today; README.md documents the awsJson1_1 `X-Amz-Target` routing and the `crate::state::service_state(&*self.engine)` store access pattern.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI MediaStore client at it.
2. CreateContainer with ContainerName="demo-container" and Tags=[{Key:"env",Value:"test"}] — expect 200 with Container{Name:"demo-container", ARN matching arn:aws:mediastore:<region>:<acct>:container/demo-container, non-empty Endpoint, CreationTime, Status}. Today: HTTP 501 NotImplemented.
3. CreateContainer again with the same name — expect ContainerInUseException (HTTP 400).
4. DescribeContainer with ContainerName="demo-container" — expect the identical ARN, Endpoint and CreationTime returned by step 2.
5. ListContainers with MaxResults=1 — expect Containers to include demo-container; create a second container and verify paging via NextToken returns each container exactly once.
6. PutContainerPolicy with ContainerName="demo-container" and a JSON policy document — expect 200.
7. GetContainerPolicy with ContainerName="demo-container" — expect the byte-identical policy string from step 6.
8. DeleteContainer with ContainerName="demo-container" — expect 200.
9. DescribeContainer with ContainerName="demo-container" — expect ContainerNotFoundException (HTTP 400), and ListContainers no longer lists it.
10. GetContainerPolicy on a container that exists but has no policy — expect PolicyNotFoundException (HTTP 400).
