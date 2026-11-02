Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: macie2.allow-list-status-derivation-missing-object
Issue-Shape-Id: com.amazonaws.macie2#Macie2
Issue-Key: macie2.allow-list-status-derivation-missing-object|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 4f40ee75c1ecaa151e81cdd2de43099afcfa6986

# macie2 CreateAllowList never validates an s3WordsList criteria or derives allow-list status: answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `4f40ee75c1ecaa151e81cdd2de43099afcfa6986`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAllowList

## Description
Cause 1 of 1: CreateAllowList performs no synchronous validation of an `s3WordsList` criteria and never derives an allow list's status, because the operation is not served at all — it refuses every call with HTTP 501 / `NotImplemented`. The whole validate-reject-then-accept-and-derive-status behavior the check exercises is therefore absent.

AWS: `CreateAllowList` (Smithy: POST /allow-lists, input `CreateAllowListRequest` with required `name`, `clientToken`, `criteria`; `criteria` targets `AllowListCriteria`, documented "The criteria must specify a regular expression (regex) or an S3 object (s3WordsList). It can't specify both"; `S3WordsList.bucketName` and `S3WordsList.objectKey` are both required). `ValidationException` is a modeled error of the operation (`com.amazonaws.macie2#ValidationException`, errorKind client, httpError 400, single `message` member). Recorded behavior: when `criteria.s3WordsList` names an object that does not exist in the bucket, AWS checks the object up front and rejects the call with HTTP 400 `ValidationException` and message `The S3 object doesn't exist: <bucketName>, <objectKey>` — fixed text `The S3 object doesn't exist: `, then the request's `criteria.s3WordsList.bucketName`, then `, `, then the request's `criteria.s3WordsList.objectKey`; nothing is persisted, so the rejected `name` must NOT appear in `ListAllowLists`. When the object does exist, the call is accepted and `GetAllowList` reports `status.code` = `OK` (Smithy: `AllowListStatus.code` is required and targets `AllowListStatusCode`, whose values are OK, S3_OBJECT_NOT_FOUND, S3_USER_ACCESS_DENIED, S3_OBJECT_ACCESS_DENIED, S3_THROTTLED, S3_OBJECT_OVERSIZE, S3_OBJECT_EMPTY, UNKNOWN_ERROR), and the created list is listed exactly once under the requested name.

Emulator: every `CreateAllowList` call is refused with HTTP 501 and error code `NotImplemented`, message `operation CreateAllowList is scaffolded but not implemented in this emulator`. The replayed check asserted the error code of the missing-object call and got `'NotImplemented'` where `ValidationException` is required (AssertionError: expected ValidationException, got 'NotImplemented'); the rest of the behavior — absence of the rejected list, acceptance of a valid s3WordsList criteria, and `status.code` = `OK` derivation on `GetAllowList` — is consequently unreachable.

Why: a client that creates allow lists from S3 word lists gets a 501 for a call AWS answers, so it cannot create an allow list at all. A client that relies on the up-front rejection to surface a typo'd object key sees an unmodeled server-side `NotImplemented` instead of the modeled 400 `ValidationException`, which SDK retry/error handling treats differently (a 501 is not a client input error), and a client that polls `GetAllowList(...).status.code` to decide whether Macie can read its word list has no status to read.

Fix: the CreateAllowList handler (`services/service-com-amazonaws-macie2-macie2/src/ops/create_allow_list.rs`) plus its input validation (existence check of `criteria.s3WordsList` against S3 state, emitting the modeled `ValidationException`), its state (persist nothing on the rejected path; record the list and its derived `AllowListStatus` on the accepted path) and the GetAllowList/ListAllowLists serializers that must surface `status.code`. Note: the open issue `20261101T224706Z-macie2-createallowlist-stub.md` asks for the stub to be implemented, but its `Issue-Trace-Ids` lists only `macie2.create-allow-list-mutually-exclusive-criteria-rejected` and `macie2.delete-allow-list-in-use-rejected`; it does not carry this check, and the s3WordsList object-existence validation, the exact rejection message and the status-code derivation described here are additional requirements that implementing the stub alone does not guarantee.

Traces: macie2.allow-list-status-derivation-missing-object

## Evidence
Check `macie2.allow-list-status-derivation-missing-object` (verdict: mismatch).

Expected (AWS): CreateAllowList with `criteria={"s3WordsList": {"bucketName": <scratch bucket>, "objectKey": "does-not-exist.txt"}}` -> HTTP 400, error code `ValidationException`, message `The S3 object doesn't exist: <bucketName>, does-not-exist.txt` (fixed text `The S3 object doesn't exist: ` + request bucketName + `, ` + request objectKey); the rejected name is absent from ListAllowLists; the same call against an existing object returns an `id`, the list appears exactly once in ListAllowLists, and GetAllowList reports `status.code` = `OK`.

Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation CreateAllowList is scaffolded but not implemented in this emulator`.

Replay output:
  File "check_macie2_allow_list_status_derivation_missing_object.py", line 36, in run
    assert code == "ValidationException", f"expected ValidationException, got {code!r}"
  AssertionError: expected ValidationException, got 'NotImplemented'

## Steps to Reproduce
1. Boot the macie2 emulator and ensure a Macie session exists (GetMacieSession, else EnableMacie).
2. Create a scratch S3 bucket via the emulator's s3 endpoint.
3. Call macie2 CreateAllowList with a unique `name`, a `clientToken`, and `criteria={"s3WordsList": {"bucketName": <bucket>, "objectKey": "does-not-exist.txt"}}`. Observe HTTP 501 `NotImplemented` instead of HTTP 400 `ValidationException` with message `The S3 object doesn't exist: <bucket>, does-not-exist.txt`.
4. Call ListAllowLists and confirm the rejected name was not persisted.
5. PutObject `wordlist.txt` into the bucket, then call CreateAllowList with `criteria={"s3WordsList": {"bucketName": <bucket>, "objectKey": "wordlist.txt"}}`. Observe it is refused rather than returning an `id`.
6. Call GetAllowList on the returned id and observe that no `status.code` of `OK` is ever derived.
7. Equivalent: replay check `macie2.allow-list-status-derivation-missing-object`.
