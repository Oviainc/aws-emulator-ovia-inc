Issue-Source: trace-sweep-divergence
Issue-Trace: securitylake-data-lake-empty-state-reads
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securitylake-data-lake-empty-state-reads
Issue-Shape-Id: com.amazonaws.securitylake#SecurityLake
Issue-Key: securitylake|ListSubscribers|4xx->2xx|$.message
Issue-Key: securitylake|ListSubscribers|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: securitylake|ListSubscribers|4xx->2xx|status
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# securitylake ListSubscribers returns 200 instead of 404 ResourceNotFoundException when Security Lake is not enabled

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscribers

## Description
Cause 1 of 1: ListSubscribers does not gate on data-lake enablement.
  AWS:      Smithy: ListSubscribers GET /v1/subscribers, success code 200, paginated (subscribers/nextToken), modeled client errors AccessDeniedException (403), BadRequestException (400), ConflictException (409), ResourceNotFoundException (404), ThrottlingException (429) (services/service-com-amazonaws-securitylake-securitylake/smithy-model/com-amazonaws-securitylake-securitylake.json). An account with Security Lake not enabled in any Region has no subscriber collection to read, so the read is answered with the modeled ResourceNotFoundException at HTTP 404 and x-amzn-errortype: ResourceNotFoundException; the recording shows that, with a body message of the form "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again." (example text — the requirement is the 404 + ResourceNotFoundException, the message is an illustration).
  Emulator: HTTP 200 with a success payload and no error at all — no $.message and no x-amzn-errortype header. It treats "not enabled" as "enabled with zero subscribers". Note the emulator already knows the enablement concept elsewhere (DeleteDataLake in the same service answers "Security Lake is not enabled in Region us-west-2"), so the enablement precondition is simply missing from this read path.
  Why:      The empty-state answer is indistinguishable from a configured-but-empty account. A client that lists subscribers to decide whether onboarding is needed concludes Security Lake is enabled and proceeds to subscriber work that cannot succeed, instead of taking the documented remedy (enable Security Lake). Error-handling branches keyed on ResourceNotFoundException never run, and pagination code is handed a success envelope for an account that has no collection.
  Fix:      The ListSubscribers handler (and the shared data-lake-enablement precondition it should consult in the service's state layer) must check enablement before serving the list and raise ResourceNotFoundException -> 404 when no Region has Security Lake enabled.
  Traces:   securitylake-data-lake-empty-state-reads#task#2.

This divergence carried no stub_path and no owned_by, and no prior securitylake issue exists (list_existing_issues query 'securitylake' returned nothing).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture securitylake-data-lake-empty-state-reads, step task#2, operation ListSubscribers, verdict=mismatch, 1-member cluster (affected: securitylake-data-lake-empty-state-reads#task#2). divergence_key: service=securitylake|ListSubscribers|status=404->200|keys=$.message,$headers.x-amzn-errortype,status.

Expected (recorded): HTTP 404; $headers.x-amzn-errortype = 'ResourceNotFoundException'; $.message = "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again."
Actual (emulator): HTTP 200; $.message missing (None); $headers.x-amzn-errortype missing (None).

Replay summary for this run: emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so the 200 is the emulator's own answer on a freshly reset, never-enabled data-lake state.

Model evidence: ResourceNotFoundException is a modeled error of ListSubscribers with httpError 404 in the service's Smithy model, so the 404/ResourceNotFoundException pairing is wire-accurate and not an unmodeled behavior.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Fixture securitylake-data-lake-empty-state-reads, step task#2: with no data lake ever created/enabled for the account, call ListSubscribers (GET /v1/subscribers).
4. Expected: HTTP 404, header x-amzn-errortype: ResourceNotFoundException, body carrying a "Security Lake isn't enabled for your account in any Regions" style message.
5. Actual: HTTP 200 with a success body, no error message and no x-amzn-errortype header.
6. After the fix, re-replay fixture securitylake-data-lake-empty-state-reads step task#2 and confirm it passes.
