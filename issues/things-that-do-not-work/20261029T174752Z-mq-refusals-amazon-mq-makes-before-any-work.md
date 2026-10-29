Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.create-configuration-invalid-name-rejected mq.create-tags-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.create-configuration-invalid-name-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: mq.create-tags-nonexistent-resource-rejected|run#6|AssertionError|assert code == "NotFoundException", code
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# mq: refusals Amazon MQ makes before any work answer 501 NotImplemented — CreateConfiguration name-charset validation and CreateTags on a nonexistent resource ARN

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
Two net-new root causes in the Amazon MQ emulator (service-com-amazonaws-mq-mq). Both are REFUSALS the real service makes on input/state alone — they do not depend on any behaviour that is still unwritten — yet the emulator answers the scaffold's HTTP 501 `NotImplemented` for them. Both handlers are still marked `SCAFFOLDED, NOT IMPLEMENTED`; the open scaffold issue for `CreateConfiguration` covers only the happy path of minting a configuration, it does not cover either refusal below, and `CreateTags` is not named by any open issue at all. Note the precedent already in the repo: `create_configuration.rs` deliberately performs the modelled enum boundary refusal (`engineType`, `authenticationStrategy`) ahead of its 501 for exactly this reason; the two refusals below belong in the same place.

Cause 1 of 2: CreateConfiguration does not refuse a Name outside the allowed character set.
Failing check: mq.create-configuration-invalid-name-rejected
AWS: Smithy `CreateConfiguration` (POST /v1/configurations) models `Name` (jsonName `name`) as required `__string` and documents it as "This value can contain only alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~). This value must be 1-150 characters long." The operation models `BadRequestException` (HTTP 400, members `errorAttribute`/`message`/`resourceShareErrors`). Real AWS refuses a Name containing a space or '!' with HTTP 400, error code `BadRequestException` and the message `Make sure your Configuration name only contains valid characters.` — that message is entirely FIXED text for this error class (it interpolates no request value); the recorded response supplies the status and the code. No configuration is created as a side effect, so a following ListConfigurations must not contain the rejected name.
Emulator: the call is answered with the scaffold's HTTP 501 and error code `NotImplemented`. The check asserts the code and fails: "AssertionError: expected BadRequestException, got NotImplemented".
Why: a client that validates names by round-tripping them, or that distinguishes "your input is bad, fix it" (400, non-retryable, surfaced to the user) from "the service cannot do this" (501), takes the wrong branch. A 501 also tells a caller the name might have been acceptable, so test suites and provisioning tools treat an illegal name as an environment outage rather than a caller error, and the no-side-effect guarantee is never exercised.
Fix: input validation of the CreateConfiguration handler — alongside the enum refusals already performed there, before the `not_implemented` return. Lift the failure through the existing `bad_request` helper into the modelled `BadRequestException`. Traces: services/service-com-amazonaws-mq-mq/src/ops/create_configuration.rs lines 186-203 (enum refusals, then the 501).

Cause 2 of 2: CreateTags against a syntactically valid but nonexistent resource ARN is not refused NotFoundException.
Failing check: mq.create-tags-nonexistent-resource-rejected
AWS: Smithy `CreateTags` (POST /v1/tags/{ResourceArn}, success 204, empty output) takes required `ResourceArn` as an httpLabel and models `NotFoundException` (HTTP 404, members `errorAttribute`/`message`/`resourceShareErrors`). Real AWS refuses a well-formed ARN naming a resource that does not exist (e.g. `arn:aws:mq:<region>:<account>:configuration:<unknown-id>`) with HTTP 404, error code `NotFoundException` and message `Resource was not found` — fixed text for this error class, with no request value interpolated; region, account and configuration id in the ARN come from the request and are not part of the message.
Emulator: the call is answered HTTP 501 with error code `NotImplemented`; the check fails on "AssertionError: NotImplemented" where `NotFoundException` was expected. The whole operation is a scaffold stub, so the existence check against service state is absent and the success path (204, then ListTags returning the tags) is absent too.
Why: a client cannot tell a mistyped or deleted resource ARN from an unimplemented service. Tag-reconciliation loops that treat 404 as "resource gone, stop" and 5xx as "retry" will spin on a permanent condition, and nothing refuses tagging a resource that was never created, so later reads of those tags silently disagree with AWS.
Fix: the CreateTags handler — resolve `ResourceArn` against this service's state (crate::state) and return the modelled `NotFoundException` when it resolves to nothing; otherwise record the tags and answer the modelled 204. Traces: services/service-com-amazonaws-mq-mq/src/ops/create_tags.rs lines 160-178 (handler returns `Scaffolded::not_implemented("CreateTags")`).

## Evidence
Check mq.create-configuration-invalid-name-rejected:
  Request: CreateConfiguration{EngineType="ACTIVEMQ", Name="<unique prefix> mqcfg!"} (name contains a space and '!')
  Expected (AWS): HTTP 400, Error.Code="BadRequestException", Error.Message="Make sure your Configuration name only contains valid characters."; no configuration with that name appears in ListConfigurations.
  Actual (emulator): HTTP 501, Error.Code="NotImplemented".
  Suite output: `AssertionError: expected BadRequestException, got NotImplemented` (check_mq_create_configuration_invalid_name_rejected.py line 30).

Check mq.create-tags-nonexistent-resource-rejected:
  Request: CreateTags{ResourceArn="arn:aws:mq:<region>:<account-id>:configuration:<nonexistent-id>", Tags={"k":"v"}}
  Expected (AWS): HTTP 404, Error.Code="NotFoundException", Error.Message="Resource was not found".
  Actual (emulator): HTTP 501, Error.Code="NotImplemented".
  Suite output: `AssertionError: NotImplemented` on `assert code == "NotFoundException"` (check_mq_create_tags_nonexistent_resource_rejected.py line 32).

Model citations: Smithy com.amazonaws.mq#CreateConfiguration errors = BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500); Name documented as alphanumerics, dashes, periods, underscores, tildes, length 1-150. Smithy com.amazonaws.mq#CreateTags errors = BadRequestException(400), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404); success status 204.

Source evidence: both handlers still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and end in `Err(crate::scaffolded::Scaffolded::not_implemented(..))`.

## Steps to Reproduce
1. Boot the emulator and point an mq client at it (region/credentials as the suite uses).
2. Cause 1: call `mq create-configuration --engine-type ACTIVEMQ --name 'sim2real mqcfg!'`. Observe HTTP 501 / `NotImplemented`; AWS answers HTTP 400 `BadRequestException` with message "Make sure your Configuration name only contains valid characters." Then call `mq list-configurations` and confirm no configuration by that name exists (the no-side-effect half of the check).
3. Cause 2: call `sts get-caller-identity` for the account id, then `mq create-tags --resource-arn arn:aws:mq:<region>:<account>:configuration:c-nonexistent-<suffix> --tags k=v`. Observe HTTP 501 / `NotImplemented`; AWS answers HTTP 404 `NotFoundException` with message "Resource was not found".
4. Re-replay to confirm a fix: mq.create-configuration-invalid-name-rejected and mq.create-tags-nonexistent-resource-rejected (each also exercises a positive control that needs the CreateConfiguration/CreateTags/ListTags success paths).
