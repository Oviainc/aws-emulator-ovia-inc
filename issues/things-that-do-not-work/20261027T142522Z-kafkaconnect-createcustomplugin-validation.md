Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# CreateCustomPlugin accepts a bucketArn that is not a bucket ARN and silently skips the plugin-file resolution

_Filed: 2026-10-27T14:25:22Z_
_Operation: CreateCustomPlugin | Class: validation_

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCustomPlugin` guards `location.s3Location.bucketArn` with `crate::state::is_arn_for(.., S3_ARN_SERVICE)`, which only checks that the value is a well-formed ARN whose VENDOR field is `s3`. Any `s3` ARN passes, including one that names an object (`arn:aws:s3:::my-bucket/plugin.zip`) or an access point (`arn:aws:s3:us-east-1:000000000000:accesspoint/ap`), neither of which is "the Amazon Resource Name (ARN) of an S3 bucket" the member documents.

The consequence is not just a missing refusal on the member: it disables the whole file-resolution rule for that create. `take_on_custom_plugin_file` resolves the bucket with the stricter `crate::state::s3_bucket_name`, which rejects exactly those two forms, and when it returns `None` the function answers `CustomPluginFileVerdict::Unknown` on the explicit premise that the handler has ALREADY refused such a value. That premise is false, so the request never reaches Amazon S3, the `BadRequestException` carrying `CUSTOM_PLUGIN_FILE_UNREADABLE` can never fire for it, and the plugin is minted and stored (state `CREATING`, then `ACTIVE`) with `file: None` — so `DescribeCustomPlugin`/`ListCustomPlugins` report a `latestRevision` with NO `fileDescription` while echoing back the unusable location. The create answers 200 for an address this crate itself says it cannot address a bucket by, and not even the warn-and-accept log of the unknown branch is emitted.

The two helpers in one crate disagree about what a bucket ARN is, and the handler uses the lax one while the code downstream assumes the strict one was used.

## Evidence

Model (the rule): `S3Location$bucketArn` — "<p>The Amazon Resource Name (ARN) of an S3 bucket.</p>" (smithy_query shape=S3Location; `__string`, `required`, no `@pattern`, so the framework cannot refuse it at the boundary and the handler must).

Handler (the lax check), services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/create_custom_plugin.rs:198-213:
```
// The plugin's file is named by an S3 bucket ARN plus a key; a
// `bucketArn` that is not an ARN at all names no bucket, so the request
// is refused rather than stored. ...
if !crate::state::is_arn_for(
    &input.location.s3_location.bucket_arn,
    crate::state::S3_ARN_SERVICE,
) {
```
what that check actually is, src/state.rs:1916-1918 -> services/common/src/arn.rs:155-162:
```
pub fn is_arn_for(value: &str, service: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, service)
}
```
```
/// Whether `value` is a well-formed resource ARN whose vendor field is `expected`.
pub fn is_resource_arn_of(value: &str, expected: &str) -> bool {
    vendor(value) == Some(expected)
}
```
(`parse`, services/common/src/arn.rs:101, requires only a non-empty partition, vendor and resource — so `arn:aws:s3:::my-bucket/plugin.zip` is accepted.)

The strict sibling helper in the SAME crate, src/state.rs:1925-1931:
```
/// ... A value naming an object
/// (`…:::bucket/key`) or an access point (`…:accesspoint/name`) is not a bucket
/// ARN this emulator addresses a bucket by, so neither is reported.
pub fn s3_bucket_name(value: &str) -> Option<&str> {
    let bucket = ::service_common::arn::parse_of(value, S3_ARN_SERVICE)?.resource;
    (!bucket.is_empty() && !bucket.contains(['/', ':'])).then_some(bucket)
}
```

The false premise that turns this into a skipped verification, src/state.rs:2078-2082:
```
// The caller has already refused a `bucketArn` that is not a bucket ARN with
// its own message, so a value that names no bucket is not re-judged here.
let Some(bucket) = s3_bucket_name(&location.bucket_arn) else {
    return CustomPluginFileVerdict::Unknown;
};
```
and the create's own comment on what `Unknown` means, src/ops/create_custom_plugin.rs:255-257: "Nothing was learned about the object, so the revision reports no file description rather than one this service did not compute."

Test coverage: tests/all/custom_plugin_file_location.rs builds every row's location as `"bucketArn": format!("arn:aws:s3:::{bucket}")` (line 237) — a bare bucket name only; no row passes an object or access-point ARN. The only pinned no-`fileDescription` case is `with_no_object_store_to_ask_a_revision_reports_no_file_description` (tests/all/custom_plugin_file_description.rs:566), i.e. no S3 mounted — a different cause, so nothing existing pins the behaviour described here.

## Steps to Reproduce

1. Mount the kafkaconnect emulator beside the S3 emulator on one engine (as tests/all/custom_plugin_file_location.rs does) and create bucket `kafkaconnect-plugin-artifacts` with object `plugin.zip`.
2. POST /v1/custom-plugins with {"name":"p1","contentType":"ZIP","location":{"s3Location":{"bucketArn":"arn:aws:s3:::kafkaconnect-plugin-artifacts/plugin.zip","fileKey":"plugin.zip"}}} — an object ARN instead of a bucket ARN.
3. Observe HTTP 200 with a customPluginArn: `is_arn_for` accepted the value, `s3_bucket_name` then returned None, so `take_on_custom_plugin_file` answered `Unknown` without ever calling S3 — neither the `Invalid parameter bucketArn: Invalid ARN.` refusal nor the `Invalid parameter s3Location: Could not access the specified custom plugin file…` refusal fires.
4. GET /v1/custom-plugins/<arn> after the transition span: the plugin is ACTIVE and `latestRevision` carries no `fileDescription` at all, while `latestRevision.location.s3Location.bucketArn` echoes the unusable ARN.
5. Repeat with {"bucketArn":"arn:aws:s3:us-east-1:000000000000:accesspoint/ap"} for the same 200.
