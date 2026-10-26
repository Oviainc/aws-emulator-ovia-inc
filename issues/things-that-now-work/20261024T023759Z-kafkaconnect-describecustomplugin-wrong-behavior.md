Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# Populate latestRevision.fileDescription (fileMd5/fileSize) for a custom plugin

_Filed: 2026-10-24T02:37:59Z_
_Operation: DescribeCustomPlugin | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeCustomPlugin returns `latestRevision` with `fileDescription` always absent. The real service computes the plugin artifact's checksum and size when it takes the file on at create time and reports them on every read of the plugin revision, so a client that validates the uploaded artifact (the documented purpose of both members: "You can use it to validate the file") sees nothing.

Expected: for a plugin whose file was taken on successfully, `latestRevision.fileDescription.fileMd5` (hex-encoded MD5 of the artifact) and `latestRevision.fileDescription.fileSize` (size in bytes) are present.

Actual: `CustomPluginRecord::latest_revision()` hardcodes `file_description: None`, so the member is omitted from DescribeCustomPlugin and from the summaries ListCustomPlugins answers with (both go through the same helper). The emulator already performs an in-process S3 `HeadObject` on the artifact during CreateCustomPlugin (`custom_plugin_file_is_unreadable`), so the size/ETag it needs is available at the moment the record is minted but is discarded.

## Evidence

Spec: Smithy shapes `CustomPluginRevisionSummary$fileDescription` -> `CustomPluginFileDescription` with `fileMd5` ("The hex-encoded MD5 checksum of the custom plugin file. You can use it to validate the file.") and `fileSize` ("The size in bytes of the custom plugin file..."); DescribeCustomPlugin output carries `latestRevision`.
Handler: src/state.rs `CustomPluginRecord::latest_revision()` — `file_description: None` with the comment "left unset: it reports the md5 and size of the plugin file ... which this emulator never reads"; consumed by src/ops/describe_custom_plugin.rs (latest_revision) and src/ops/list_custom_plugins.rs (summary()).
Tests: no test asserts `fileDescription` (grep over tests/ finds no occurrence); tests/all/custom_plugin_file_location.rs only asserts the location round-trips.

## Steps to Reproduce

1. PUT an object into the emulator's S3 so the artifact resolves.
2. POST /v1/custom-plugins with a `location.s3Location` naming that object; wait for the plugin to become ACTIVE.
3. GET /v1/custom-plugins/{customPluginArn} and inspect `latestRevision` — `fileDescription` is absent; the real service reports `fileDescription.fileMd5` and `fileDescription.fileSize` for the uploaded artifact.
4. GET /v1/custom-plugins and inspect the summary for the same plugin — `latestRevision.fileDescription` is likewise absent.
