Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-service-role-association
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-service-role-association
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|AssociateServiceRoleToAccount|4xx->2xx|$.Message
Issue-Key: greengrass|AssociateServiceRoleToAccount|4xx->2xx|status
Issue-Key: greengrass|GetServiceRoleForAccount|4xx->2xx|$
Issue-Key: greengrass|GetServiceRoleForAccount|4xx->2xx|status
Issue-Commit: 8f521f25a30f45aed4bf452c3a7727cefd2e1709

# greengrass /greengrass/servicerole paths never refuse: invalid role ARN and absent association both answer HTTP 200

> _Note: this issue was identified against the code base at commit `8f521f25a30f45aed4bf452c3a7727cefd2e1709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateServiceRoleToAccount

## Description
2 causes to close. Both live on the account-singleton service-role path (`/greengrass/servicerole`, shared by the greengrass and greengrassv2 shapes behind the `greengrass` endpoint prefix): the emulator's handlers unconditionally answer a 200 success, so neither of the two refusals AWS serves on that path exists. The shared fault is a MISSING refusal branch, not wrong wording.

Cause 1 of 2 (AssociateServiceRoleToAccount, fixture greengrassv2-service-role-association step task#4): a roleArn that is not an ARN at all is accepted and persisted.
AWS:      Smithy: AssociateServiceRoleToAccount, PUT /greengrass/servicerole, success code 200; input member `roleArn` (String, required, jsonName `RoleArn`); modeled errors ValidationException (HTTP 400) / InternalServerException (HTTP 500) in com.amazonaws.greengrassv2, BadRequestException (HTTP 400, members `Message` + `ErrorDetails`) / InternalServerErrorException (HTTP 500) in com.amazonaws.greengrass. The model places NO @pattern or @length constraint on `roleArn`, so the ARN-shape check is a server-side rule the model does not express — for the exact wire form this is recorded behavior only. The recorded response is HTTP 400 with a body whose only member is `Message` = "That role ARN is invalid." That message is ENTIRELY FIXED TEXT: it has no variable part — AWS does not echo the offending value, the member name, the account id or a pattern. No `x-amzn-errortype` header and no error code appear in the recorded refusal, so an SDK surfaces Error.Code as the HTTP status string "400". AWS validates only the ARN's SHAPE here; a well-formed IAM role ARN whose trust policy does not name Greengrass is still accepted, so the fix must not over-reject. The association must be left untouched by the refusal.
Emulator: answers HTTP 200 with the normal success body and `$.Message` absent (replay detail: "status: expected 400, got 200" / "$.Message: missing (expected 'That role ARN is invalid.', got None)"). The non-ARN string is written into the account's single service-role slot, so it is then readable as the account's service role.
Why:      a client that relies on AWS rejecting a malformed role ARN is told the association succeeded, and the account-level singleton ends up holding a value no service can ever assume. The failure surfaces much later (at core-device/connectivity verification) instead of at the call that caused it, and client code branching on the 400 / "That role ARN is invalid." refusal never runs.
Fix:      the request validation of AssociateServiceRoleToAccount's handler — check `roleArn` is a well-formed IAM role ARN BEFORE mutating state, and on failure answer HTTP 400 with a body carrying only the fixed `Message` text and no error code, leaving the stored association unchanged.
Traces:   greengrassv2-service-role-association#task#4.

Cause 2 of 2 (GetServiceRoleForAccount, fixture greengrassv2-service-role-association step task#6): reading the association when none exists answers an empty 200 instead of the not-attached refusal.
AWS:      Smithy: GetServiceRoleForAccount, GET /greengrass/servicerole, success code 200, output members `associatedAt`/`roleArn` (jsonName `AssociatedAt`/`RoleArn`); the only modeled error is InternalServerException/InternalServerErrorException (HTTP 500), so the not-attached refusal is recorded behavior only. The recording shows that when the account has no service role associated (none ever set, or the association removed by DisassociateServiceRoleFromAccount), the call fails with HTTP 404 and a body whose only member is `Message` = "You need to attach an IAM role to this AWS account." — again entirely fixed text with no variable parts, and no error code or `x-amzn-errortype` header, so an SDK reports Error.Code as "404". When an association DOES exist the 200 must carry `RoleArn` and `AssociatedAt`.
Emulator: answers HTTP 200 with an empty JSON object (replay detail: "status: expected 404, got 200" / "$: empty_body (expected {'Message': 'You need to attach an IAM role to this AWS account.'}, got {})"). The absent-association state is not distinguished from a present one.
Why:      "no service role attached" is indistinguishable from "attached" to a client: instead of a ClientError it gets a success whose required `RoleArn` is simply missing, so setup code that probes this call to decide whether it must associate a role concludes a role is present and skips the association, and any code reading `RoleArn` off the response hits a missing-key error rather than the modeled 404.
Fix:      GetServiceRoleForAccount's handler — branch on the account's service-role slot being empty and answer HTTP 404 with a body carrying only the fixed `Message` text above (no error code), serving the 200 with `RoleArn`/`AssociatedAt` only when an association is actually stored.
Traces:   greengrassv2-service-role-association#task#6.

Related but NOT covering these: the open behavioral issue 20261109T003358Z-greengrassv2-associateserviceroletoaccount-accepts-a-non-arn-rolearn-with.md describes the same AssociateServiceRoleToAccount gap but is grounded in a behavioral check and carries NO wire replay key, so this wire divergence is not re-runnable from it; 20261103T065241Z-greengrassv2-getserviceroleforaccount-stub.md is stale (the operation now answers 200 rather than 501) and says nothing about the not-attached refusal; 20261103T145648Z-greengrass-greengrassv2-wire-refusal-paths-diverge-wrong-status-error.md and 20261103T065241Z-greengrassv2-refusal-responses-diverge-from-aws-10-net.md cover component/deployment/tag refusal paths, not the service-role singleton.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture greengrassv2-service-role-association (service token `greengrass`).

Cause 1 — step task#4, AssociateServiceRoleToAccount (PUT /greengrass/servicerole) with a roleArn that is not an ARN:
  expected: HTTP 400, body {"Message": "That role ARN is invalid."} (fixed text, no variable parts), no error code / errortype header, association unchanged.
  actual:   HTTP 400 -> 200. Replay detail: `status: expected 400, got 200` and `$.Message: missing (expected 'That role ARN is invalid.', got None)`. Divergence key service=greengrass|AssociateServiceRoleToAccount|status=400->200|keys=$.Message,status.

Cause 2 — step task#6, GetServiceRoleForAccount (GET /greengrass/servicerole) with no association in effect:
  expected: HTTP 404, body {"Message": "You need to attach an IAM role to this AWS account."} (fixed text, no variable parts), no error code / errortype header.
  actual:   HTTP 404 -> 200 with an empty body. Replay detail: `status: expected 404, got 200` and `$: empty_body (expected {'Message': 'You need to attach an IAM role to this AWS account.'}, got {})`. Divergence key service=greengrass|GetServiceRoleForAccount|status=404->200|keys=$,status.

Neither cluster was marked stub_path or owned_by, so neither is covered by an existing wire-keyed issue. Timestamps and generated role-name suffixes in the recording are per-run values, not requirements.

## Steps to Reproduce
Boot the emulator and replay fixture `greengrassv2-service-role-association` from trace set simparity-wire-aws-v1; steps task#4 and task#6 diverge. By hand, against the `greengrass` endpoint prefix:

Cause 1:
1. PUT /greengrass/servicerole with body {"RoleArn": "not-an-arn"} (SDK: associate_service_role_to_account(roleArn="not-an-arn")).
   Expect: HTTP 400, body {"Message": "That role ARN is invalid."}, and the previously stored association unchanged.
   Observe: HTTP 200 with an `AssociatedAt` body, and the bogus value persisted.
2. Regression guard: repeat with a syntactically valid IAM role ARN (even one whose trust policy names only ec2.amazonaws.com) — this must still succeed with HTTP 200.

Cause 2:
3. Starting from an account with no service role attached (fresh state, or after DisassociateServiceRoleFromAccount), GET /greengrass/servicerole (SDK: get_service_role_for_account()).
   Expect: HTTP 404, body {"Message": "You need to attach an IAM role to this AWS account."}.
   Observe: HTTP 200 with body {}.
4. Associate a valid role ARN and repeat step 3 — the 200 must then carry both `RoleArn` and `AssociatedAt`.
