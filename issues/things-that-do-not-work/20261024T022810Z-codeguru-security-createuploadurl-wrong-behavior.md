Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# s3Url points at a deliberately non-resolving .invalid host, so the documented upload step cannot be performed

_Filed: 2026-10-24T02:28:10Z_
_Operation: CreateUploadUrl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: CreateUploadUrlResponse$s3Url is "A pre-signed S3 URL. You can upload your code to this URL" — a client PUTs its zipped source to that URL with the returned requestHeaders, and the bytes are then what CreateScan analyses.

Actual: the handler synthesises `https://codeguru-security-upload.{region}.emulator.invalid/{account}/{scanName}/{codeArtifactId}` (src/ops/create_upload_url.rs lines 181-185). `.invalid` is reserved and never resolves, so any client following the documented workflow fails at the upload step with a DNS error, and nothing in the emulator can receive the artifact. Because no bytes are ever stored, the scan has nothing to analyse, which is the root cause of the scan-always-Failed and findings-always-empty gaps. The emulator ships an S3 emulator (services/service-com-amazonaws-s3-amazons3), so the minted target could instead address the emulated S3 data plane and make the upload -> scan -> findings workflow completable end to end.

## Evidence

Smithy model: CreateUploadUrlResponse$s3Url documented as a pre-signed S3 URL for uploading the resource, with requestHeaders as "the required headers when uploading your resource". Handler: src/ops/create_upload_url.rs lines 174-185 explicitly chooses a non-resolving host ("Deliberately NOT a real s3...amazonaws.com host"). No test PUTs to the URL; tests only assert the artifact record exists (tests/all/scan_lifecycle.rs:466+).

## Steps to Reproduce

1. POST /uploadUrl with {"scanName":"demo"}
2. Take s3Url from the response and PUT a zip archive to it with the returned requestHeaders
3. The request fails (host under .invalid never resolves); against AWS the PUT succeeds and the uploaded archive is what the subsequent CreateScan analyses
