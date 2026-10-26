Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# Endpoint.createdAt goes onto the wire as an epoch number: the managed-endpoint shape was left on the derive that drops @timestampFormat

_Filed: 2026-10-26T01:51:15Z_
_Operation: ListManagedEndpoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every timestamp member of this service targets `com.amazonaws.emrcontainers#Date`, which carries `@timestampFormat("date-time")`, so on the restJson1 wire it must be an RFC 3339 STRING. This crate knows the vendored derive cannot emit that format and compensates for it by hand in `src/ops/types/date_time_wire.rs`, applying `emrc_schema_and_codec!` to `JobRun`, `JobTemplate`, `SecurityConfiguration`, `VirtualCluster` and `CreateJobTemplateResponse`.

`Endpoint` — the shape `ListManagedEndpoints` (and `DescribeManagedEndpoint`) answers with — was left on the plain `#[smithy_macros::smithy(shape = ...)]` derive. Its `createdAt` member schema therefore reports NO timestamp format, and the JSON serializer's documented fallback is epoch seconds, so `GET /virtualclusters/{id}/endpoints` renders `endpoints[].createdAt` as a bare JSON NUMBER while every other read path of the same service renders `createdAt` as a date-time string. An SDK deserializing `Endpoint$createdAt` against the model's `date-time` format gets a number where a string is modelled, and the one service answers two different wire forms for the same modelled type. The root cause is the one missing `emrc_schema_and_codec!` block, so `DescribeManagedEndpoint` is wrong in exactly the same way (its own existing issue is about the absent `serverUrl`/`certificateAuthority`/`securityGroup`/`subnetIds` members, a different defect).

## Evidence

The rule, stated by this crate's own compensation module, src/ops/types/date_time_wire.rs:17-24:
```
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is
//! hand-written service code). So a macro-built `date-time` member schema reports NO
//! format and every such member went onto the wire as a bare epoch number — not even the
//! modelled JSON type, so an SDK deserializing the member rejects or mis-parses it.
```

The sibling shapes that get the fix, src/ops/types/shared_types.rs:380-383 and 397-406:
```
/// `createdAt` targets `Date` (`@timestampFormat("date-time")`), so this shape's schema
/// comes from `emrc_schema_and_codec!` — see `types/date_time_wire.rs`.
...
emrc_schema_and_codec!(
    JobTemplate,
    JOB_TEMPLATE_SCHEMA,
...
    (M_CREATED_AT, created_at, "createdAt", 3, opt, date_time),
```
(the same for `JobRun` at shared_types.rs:253-330, `SecurityConfiguration` at 459-494 and `VirtualCluster` at 515-534).

The shape that does NOT, src/ops/types/shared_types.rs:428-444:
```
#[smithy_macros::smithy(shape = "com.amazonaws.emrcontainers#Endpoint")]
#[derive(Debug, Clone, Default)]
pub struct Endpoint {
...
    pub created_at: Option<::aws_smithy_types::DateTime>,
```
No `emrc_schema_and_codec!(Endpoint, ...)` exists anywhere in the crate (grep for `emrc_schema_and_codec!` matches only shared_types.rs:279/397/485/534 and create_job_template_types.rs:36), and `vendor/smithy-macros` contains no `with_timestamp_format`/`timestampFormat` call at all (grep: "No matches.").

The model says the member is date-time (smithy_query shape=Endpoint): `"createdAt": {"target": "Date", "timestampFormat": "date-time", "type": "timestamp", "doc": "<p>The date and time when the endpoint was created.</p>"}`.

The fallback that then applies, vendor/smithy-schema-server/src/json.rs:44-50 and 516-521:
```
fn timestamp_format(schema_format: Option<TimestampFormat>) -> Format {
    match schema_format {
        Some(TimestampFormat::DateTime) => Format::DateTime,
        Some(TimestampFormat::HttpDate) => Format::HttpDate,
        Some(TimestampFormat::EpochSeconds) | None => Format::EpochSeconds,
```
```
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        let format = timestamp_format(schema.timestamp_format().map(|t| t.format()));
```

The affected handler, src/ops/list_managed_endpoints.rs:302-305:
```
        let endpoints: Vec<Endpoint> = matching[start..end]
            .iter()
            .map(|record| record.to_endpoint())
```
(and src/ops/describe_managed_endpoint.rs:175-177 returns the same rendering).

Test coverage: tests/all/created_metadata_wire_form.rs:15-21 states the rule — "Every member this service's model targets at `Date` … goes onto the restJson1 wire as an RFC 3339 date-time STRING, never as an epoch number, on every path that reports one" — but its table at lines 240-280 has 9 rows covering only `CreateJobTemplateResponse`, `JobTemplate`, `SecurityConfiguration`, `VirtualCluster` and `JobRun`; no row reads an endpoint. tests/all/managed_endpoint_listing.rs:243 and 721 read `/createdAt` through `instant_millis`, whose doc (tests/all/virtual_cluster_job_run_loop.rs:264-281) says it "accepts whichever the wire used, so the assertion is about the INSTANT and not about the framework's rendering of it" — so no test pins the epoch form, and no comment justifies it.

## Steps to Reproduce

1. POST /virtualclusters with {"name":"vc","clientToken":"t1","containerProvider":{"id":"eks-a","type":"EKS","info":{"eksInfo":{"namespace":"ns"}}}} → note the cluster id.
2. POST /virtualclusters/{clusterId}/endpoints with {"name":"jeg","clientToken":"t2","type":"JUPYTER_ENTERPRISE_GATEWAY","releaseLabel":"emr-6.10.0-latest","executionRoleArn":"arn:aws:iam::000000000000:role/EMRJobRole","sessionIdleTimeoutInMinutes":30} → 200.
3. GET /virtualclusters/{clusterId}/endpoints → the response body carries `endpoints[0].createdAt` as a JSON NUMBER (epoch seconds), e.g. 1793000000.123.
4. GET /virtualclusters/{clusterId} → the same instant on the cluster is reported as `virtualCluster.createdAt` = "2026-10-26T12:33:20.123Z", an RFC 3339 string; so does /jobtemplates/{id}, /securityconfigurations/{id} and every job-run read.
5. GET /virtualclusters/{clusterId}/endpoints/{endpointId} → `endpoint.createdAt` is the same epoch number, from the identical rendering.
