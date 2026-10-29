Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.invalid-parameter-daemon-taskdef-and-namespace
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.invalid-parameter-daemon-taskdef-and-namespace|run#21|AssertionError|assert error == expected, f"ListDaemonTaskDefinitions: expected {expected!r}, got {error!r}"
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# ECS ListDaemonTaskDefinitions: maxResults below the documented range is refused with "Max results can not be lesser than 1" instead of AWS's "MaxResults must be a positive number between 1-100."

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDaemonTaskDefinitions

## Description
One root cause (1 of 1).

Cause 1 of 1: the page-size refusal for ListDaemonTaskDefinitions uses a different sentence at the LOW end of the documented range than at the high end, and the low-end sentence is not the one AWS answers with.

AWS: Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json) models ListDaemonTaskDefinitions.maxResults as a BoxedInteger with NO @range trait, and models InvalidParameterException (HTTP 400, "The specified parameter isn't valid. Review the available parameters for the API request.") among the operation's errors; the member's own documentation states the bound in prose: "This value can be between 1 and 100." Because the model carries no range, the message itself is recorded behavior only: the recording shows AWS refusing maxResults=0 with HTTP 400, error code InvalidParameterException and message "MaxResults must be a positive number between 1-100." — i.e. AWS answers the SAME range sentence for a value on either side of the documented 1..=100 range, and it is the range sentence, not a below-the-minimum one. Message template: the whole sentence is fixed text except the two range bounds, which are the operation's documented minimum and maximum page size (1 and 100 for this operation); it echoes neither the value the request carried nor the member's wire spelling ("maxResults").

Emulator: for maxResults=0 the emulator returns the right status (400) and the right code (InvalidParameterException) but the message "Max results can not be lesser than 1". It only uses the AWS sentence when maxResults is ABOVE the maximum. The split is in the shared page-size helper: a value below the minimum takes a below-the-minimum sentence built from a member subject and the bound, while a value above the maximum takes the range sentence. So the two ends of one documented range are refused with two different sentences and the low end diverges from AWS.

Why: a client that keys off the error MESSAGE — error-message assertions in deployment tooling and CDK/Terraform-style retry and diagnostic paths, support runbooks, and log-based alerting that matches ECS's documented refusal text — sees wording ECS never emits for an out-of-range page size. Because the code (InvalidParameterException) is identical for both ends, the message is the only thing distinguishing the refusal, so a consumer that parses it mis-reports the reason, or fails to recognise the refusal at all, while a consumer that only inspects the code is unaffected. It also makes the emulator self-inconsistent: maxResults=0 and maxResults=101 are the same documented violation but produce different text.

Fix: the operation's validation, in the shared page-size validation helper the ECS crate uses — `page_size_violation` (and the `below_minimum_message` sentence it reaches for on the low side) in the ECS service state module, called from the ListDaemonTaskDefinitions handler's maxResults guard. A page size outside the documented inclusive range should answer the range sentence `page_size_range_message` already produces, at BOTH ends, with the operation's own documented minimum and maximum substituted. Note the same helper is shared by every paging operation in this crate, so the sentence a below-minimum page size earns changes for all of them; only ListDaemonTaskDefinitions is evidenced by this check, so the other lists' low-end wording should be confirmed against their recordings rather than assumed.

Related open issues (neither covers this): 20261027T094105Z-ecs-continueservicedeployment-does-not-validate-the-hookid-format.md concerns ContinueServiceDeployment hookId wording, and 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md concerns validations that are MISSING entirely — here the validation fires with the correct code and only the message diverges.

Traces: ecs.invalid-parameter-daemon-taskdef-and-namespace. The check's other assertions in the same run passed (RegisterDaemonTaskDefinition invalid cpu, DescribeDaemonTaskDefinition malformed identifier, DeleteDaemonTaskDefinition malformed identifier), so this is the only divergence it exposes.

## Evidence
Check ecs.invalid-parameter-daemon-taskdef-and-namespace, verdict: mismatch.

Call: ListDaemonTaskDefinitions with maxResults=0.

Expected (real AWS, from the recording):
  HTTP 400
  code:    InvalidParameterException
  message: "MaxResults must be a positive number between 1-100."

Actual (emulator):
  HTTP 400
  code:    InvalidParameterException
  message: "Max results can not be lesser than 1"

Replay assertion text:
  AssertionError: ListDaemonTaskDefinitions: expected ('InvalidParameterException', 'MaxResults must be a positive number between 1-100.'), got ('InvalidParameterException', 'Max results can not be lesser than 1')

Message anatomy (AWS form): fixed text "MaxResults must be a positive number between " + <min> + "-" + <max> + ".", where <min>/<max> are the operation's documented page-size bounds (1 and 100 per the maxResults doc "This value can be between 1 and 100."). No part of it comes from the request value.

Emulator source of the diverging text:
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs:792-800 — page_size_violation() returns below_minimum_message(member_subject("maxResults"), min) when max_results < min, and only page_size_range_message(min, max) when max_results > max.
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs:781-783 — below_minimum_message() formats "{subject} can not be lesser than {min}".
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs:770-772 — page_size_range_message() formats the correct AWS sentence "MaxResults must be a positive number between {min}-{max}.".
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_daemon_task_definitions.rs:184-200 — the handler's maxResults guard calling that helper with MIN_LIST_PAGE_SIZE=1 / MAX_LIST_PAGE_SIZE=100.

Model grounding: smithy_query(query="operation", operation="ListDaemonTaskDefinitions") shows input member maxResults -> BoxedInteger (no @range) and errors AccessDeniedException(403), ClientException(400), InvalidParameterException(400), ServerException(500). The range therefore lives only in the member's documentation, and the refusal wording is recorded behavior only.

## Steps to Reproduce
1. git_clone the repo and boot the emulator.
2. Register a daemon task definition so the listing has a positive control:
   aws ecs register-daemon-task-definition --family sim2real-daemon-td-1 \
     --container-definitions '[{"name":"app","image":"public.ecr.aws/amazonlinux/amazonlinux:latest","cpu":128,"memory":128,"essential":true}]'
3. Call ListDaemonTaskDefinitions with a page size below the documented minimum:
   aws ecs list-daemon-task-definitions --max-results 0
4. Observe the refusal: HTTP 400, code InvalidParameterException, message "Max results can not be lesser than 1".
   Real AWS answers HTTP 400, code InvalidParameterException, message "MaxResults must be a positive number between 1-100.".
5. For contrast, call it with a page size above the maximum:
   aws ecs list-daemon-task-definitions --max-results 101
   The emulator answers the correct AWS sentence here, showing the two ends of one documented range are refused with two different sentences.
6. Re-replay check ecs.invalid-parameter-daemon-taskdef-and-namespace to confirm a fix.
