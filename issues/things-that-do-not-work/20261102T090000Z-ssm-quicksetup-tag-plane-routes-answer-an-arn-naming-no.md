Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.tag-resource-list-tags-roundtrip
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup.tag-resource-list-tags-roundtrip|run#24|AssertionError|assert tag_err["Code"] == "ResourceNotFoundException", (
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# Tag-plane routes answer an ARN naming no manager with 401 UnauthorizedException, and their modeled 404 carries the wrong not-found sentence

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
2 causes; both must be closed before this issue is submitted. Related open issue 20261101T001539Z-ssm-quicksetup-tag-plane-routes-refuse-an-arn-naming-no.md describes cause 1 but the replay did NOT mark this check as owned by it (the check's assertion drifted: the refusal is now read at check_ssm_quicksetup_tag_resource_list_tags_roundtrip.py line 75 via tag_err["Code"], not the `tag_code` assertion that issue's Issue-Key pins), and that issue states the WRONG required message for this operation, which cause 2 corrects. The defect is still present at this checkout.

Cause 1 of 2 — resource non-existence is converted into an authorization refusal on /tags/{ResourceArn} (failing check: ssm-quicksetup.tag-resource-list-tags-roundtrip).
AWS: Smithy (services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json) binds ListTagsForResource (GET /tags/{ResourceArn}), TagResource (PUT /tags/{ResourceArn}) and UntagResource (DELETE /tags/{ResourceArn}) to exactly six errors: AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). No 401 and no UnauthorizedException shape exists anywhere in this service's model. An ARN that names no configuration manager in the Region — including the ARN of a manager just deleted — is answered HTTP 404 with error code ResourceNotFoundException.
Emulator: the request never reaches the operation handler. src/service.rs fronts ListTagsForResource, TagResource, UntagResource (and UpdateConfigurationDefinition) with a locally invented TargetAuthorization stage whose `serves()` is purely a store lookup (`store.manager_in(&region, arn).is_some()`); on a miss it answers HTTP 401 (UNAUTHORIZED_STATUS) with wire error code UnauthorizedException (UNAUTHORIZED_CODE, also x-amzn-errortype) and body `{"message": "Unauthorized"}` (UNAUTHORIZED_MESSAGE, lowercase `message`, unlike this service's modeled `Message`). Replay: after the manager is deleted and GetConfigurationManager on that ARN correctly reports ResourceNotFoundException (that assertion passes), ListTagsForResource on the same ARN reports UnauthorizedException.
Why: a client cannot distinguish a deleted/absent resource from a credential or permission problem. SDK callers branch on ResourceNotFoundException for idempotent cleanup, delete-then-verify loops and tag reconciliation; a 401 with an unmodeled code is not in the generated error union, so it surfaces as a generic ClientError no not-found branch matches, and 401 additionally drives SDK credential-refresh paths. The same wrong class is served for TagResource and UntagResource on a dead ARN, so post-delete mutations also report the wrong refusal, and the lowercase `message` member breaks clients reading Error.Message.
Fix: the request-authorization stage in the ssm-quicksetup service crate (src/service.rs: TargetAuthorization::serves/refusal, `unauthorized`, target_authorization_table) must not turn resource non-existence into an authorization refusal; target-ARN resolution belongs to each operation's handler, which already has a modeled 404 branch.

Cause 2 of 2 — the modeled 404 the handlers would serve states the wrong sentence (failing check: ssm-quicksetup.tag-resource-list-tags-roundtrip).
AWS: recorded behavior only (the model documents ResourceNotFoundException only as "The resource couldn't be found. Check the ID or name and try again."). The recording shows each tag-plane operation has its OWN not-found sentence, and neither is GetConfigurationManager's: ListTagsForResource on an ARN naming no manager answers Message `Configuration with requested id does not exist.` and TagResource on the same ARN answers Message `Resource arn not found.` — both are WHOLLY FIXED text, naming no id, ARN, member or value, so nothing in them is taken from the request or from state. GetConfigurationManager's sentence is different (`Quick setup manager with arn: ` + the ARN from the request path + ` could not be found.`, where only the ARN is variable) and must not be reused here.
Emulator: src/ops/list_tags_for_resource.rs:174-183 builds its ResourceNotFoundException with `crate::state::manager_not_found_message(&input.resource_arn)` — the GetConfigurationManager template, with an interpolated ARN — instead of the fixed ListTagsForResource sentence; src/ops/tag_resource.rs's not-found branch does the same rather than `Resource arn not found.`. This is masked today by cause 1 (the 401 preempts the branch), so fixing cause 1 alone leaves the check failing on the message assertions that follow it.
Why: a client matching the service's documented not-found text (log scraping, error classification, assertion in user tests) sees a sentence AWS never sends, and one that leaks the request ARN into a message AWS keeps fixed.
Fix: the per-operation not-found message rule in the tag-plane handlers' validation/error construction (src/ops/list_tags_for_resource.rs, src/ops/tag_resource.rs, src/ops/untag_resource.rs) — each needs its own fixed sentence rather than the shared manager_not_found_message helper.

Traces: ssm-quicksetup.tag-resource-list-tags-roundtrip.

## Evidence
Replay verdict — [mismatch] ssm-quicksetup.tag-resource-list-tags-roundtrip:
```
File "check_ssm_quicksetup_tag_resource_list_tags_roundtrip.py", line 75, in run
  assert tag_err["Code"] == "ResourceNotFoundException", (
AssertionError: expected ListTagsForResource on deleted manager A to raise ResourceNotFoundException, got UnauthorizedException
```
The check proves the manager is really gone first: it polls GetConfigurationManager until it answers ResourceNotFoundException with Message "Quick setup manager with arn: <arn> could not be found." (both assertions pass), then calls ListTagsForResource on that same ARN.

Cause 1 expected vs actual:
- Expected: HTTP 404, Error.Code = ResourceNotFoundException.
- Actual: HTTP 401, Error.Code / x-amzn-errortype = UnauthorizedException, body member lowercase `message` = "Unauthorized" (wholly fixed text, naming no member, value or resource).
Source — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/service.rs:97-112 declares the unmodeled refusal:
```
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";
pub const UNAUTHORIZED_STATUS: u16 = 401;
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";
```
with a doc comment conceding it is "Not a shape this service's model declares"; the stage's `serves()` decides it purely on `store.manager_in(&region, arn).is_some()`, and the tag operations plus UpdateConfigurationDefinition are enrolled in its table.
Model — smithy_query(query="operation", operation="ListTagsForResource,TagResource,UntagResource") on services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json: all three bind AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). No 401, no UnauthorizedException.

Cause 2 expected vs actual (the assertions immediately after the one that fails, lines 76-83 of the check):
- ListTagsForResource on the absent ARN — Expected Message: "Configuration with requested id does not exist." (fixed text, no variable part). Actual, from the handler that would serve it: "Quick setup manager with arn: <ARN from the request path> could not be found." — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/list_tags_for_resource.rs:174-183 calls `crate::state::manager_not_found_message(&input.resource_arn)`.
- TagResource on the absent ARN — Expected Code ResourceNotFoundException with Message: "Resource arn not found." (fixed text). Actual: 401 UnauthorizedException today, and the same manager_not_found_message sentence once cause 1 is fixed (src/ops/tag_resource.rs).

## Steps to Reproduce
1. Build and boot the emulator from this checkout; sign requests for ssm-quicksetup.
2. Create a configuration manager: POST /configurationManager with a valid Name and one AWSQuickSetupType-SSMHostMgmt definition; keep the returned ManagerArn.
3. Tag it (PUT /tags/{ManagerArn} with {"Tags": {"owner": "manager-a"}}) and read it back (GET /tags/{ManagerArn}); both succeed today.
4. Delete it (DELETE /configurationManager/{ManagerArn}) and poll GET /configurationManager/{ManagerArn} until it answers 404 ResourceNotFoundException — the resource is now gone.
5. Cause 1: call GET /tags/{ManagerArn} on that now-deleted ARN. Observe HTTP 401, x-amzn-errortype: UnauthorizedException, body {"message": "Unauthorized"} instead of HTTP 404 ResourceNotFoundException. Same wrong refusal for PUT /tags/{ManagerArn} (TagResource) and DELETE /tags/{ManagerArn}?tagKeys=owner (UntagResource). Also reproducible with no deletion at all: GET /tags/arn:aws:ssm-quicksetup:<region>:<account>:configuration-manager/00000000-0000-0000-0000-000000000000. Contrast GET /configurationManager/{same ARN}, which is not behind the stage and correctly answers the modeled 404.
6. Cause 2: with the authorization stage bypassed (or after fixing cause 1), repeat step 5's GET /tags/{deleted ARN}: the 404's Message is "Quick setup manager with arn: <that ARN> could not be found." instead of the fixed "Configuration with requested id does not exist."; and PUT /tags/{deleted ARN} with {"Tags": {"owner": "post-delete"}} reports that same sentence instead of the fixed "Resource arn not found."
7. Positive control that must keep working: a second manager (AWSQuickSetupType-Distributor) tagged {"owner": "manager-b"} still reports its tags from GET /tags/{its ARN} after manager A's deletion.
