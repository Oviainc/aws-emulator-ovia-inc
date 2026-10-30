Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-experiment-list-operations
Issue-Step: teardown#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-experiment-list-operations
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|DeleteApplication|4xx->2xx|$
Issue-Key: appconfig|DeleteApplication|4xx->2xx|$headers.content-length
Issue-Key: appconfig|DeleteApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|DeleteApplication|4xx->2xx|status
Issue-Key: appconfig|DeleteExperimentDefinition|2xx->4xx|status
Issue-Key: appconfig|DeleteApplication|2xx->4xx|status
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# AppConfig DeleteApplication destroys an application that still has experiment definitions instead of refusing with BadRequestException

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplication

## Description
DeleteApplication has no guard for child experiment definitions, so the emulator silently cascade-deletes an application AWS refuses to delete - and because the application is then gone, the two deletes that should follow fail. 3 causes are bundled: the missing guard and the two downstream refusals it produces, all in one teardown sequence.

Cause 1 of 3: DeleteApplication succeeds where AWS refuses - fixture appconfig-experiment-list-operations, step teardown#13.
AWS: Smithy DeleteApplication (DELETE /applications/{ApplicationId}, success 204 with an empty body, errors BadRequestException 400 / InternalServerException 500 / ResourceNotFoundException 404). Recorded behavior: when the application still has experiment definitions under it, AWS refuses with HTTP 400 and error type BadRequestException, body {"Message": "Cannot delete application <applicationId>, because there are still experiment definitions existing under it."}, and sets the x-amzn-errortype and content-length response headers. The message's FIXED text is "Cannot delete application " ... ", because there are still experiment definitions existing under it."; the only VARIABLE part is the application id, taken from the request path (the recording's id is a per-run value and is not part of the requirement).
Emulator: answers HTTP 204 with an empty body and no x-amzn-errortype/content-length error headers - it deletes the application together with its experiment definitions.
Why: the refusal is AppConfig's protection against orphaning experiment state. A client relying on it to detect leftover children (the documented teardown order is children first) gets a success, loses the experiment definitions without ever being told, and cannot distinguish "nothing was there" from "children were destroyed". Worse, the emulator's success makes a correct AWS teardown script diverge from this point onward (see Causes 2 and 3).

Cause 2 of 3: DeleteExperimentDefinition then 404s - fixture appconfig-experiment-list-operations, step teardown#14.
AWS: DeleteExperimentDefinition returns 204 here, because the application and its experiment definition still exist (the preceding DeleteApplication was refused).
Emulator: 404 - the experiment definition went away with the application in Cause 1.
Why: a client following AWS's required teardown order (delete the experiment definition, then the application) gets a hard not-found on a resource that must still exist.

Cause 3 of 3: the real DeleteApplication then 404s - fixture appconfig-experiment-list-operations, step teardown#15.
AWS: 204 - the application is deleted now that its last experiment definition is gone.
Emulator: 404 - already deleted in Cause 1.
Why: the teardown cannot complete cleanly; the client sees a not-found for the application it is responsible for and must treat a correct sequence as an error.

Fix: the DeleteApplication handler and the appconfig service's parent/child state. DeleteApplication must count the experiment definitions under the application and, when any remain, reject with BadRequestException / HTTP 400 and the message template above (including the x-amzn-errortype header) WITHOUT mutating state; only when no child experiment definitions remain may it delete the application and return 204. Removing the cascade also restores Causes 2 and 3 to 204.

## Evidence
Cause 1 (appconfig-experiment-list-operations#teardown#13, DeleteApplication):
  status: expected 400, got 204
  $: empty_body (expected {'Message': 'Cannot delete application <applicationId>, because there are still experiment definitions existing under it.'}, got {})
  $headers.x-amzn-errortype: missing (expected present, i.e. BadRequestException)
  $headers.content-length: missing (expected present)
  Message decomposition - fixed text: "Cannot delete application " and ", because there are still experiment definitions existing under it."; variable: the application id from the request path (recorded value 'ho65pb1' is per-run).

Cause 2 (appconfig-experiment-list-operations#teardown#14, DeleteExperimentDefinition):
  status: expected 204, got 404

Cause 3 (appconfig-experiment-list-operations#teardown#15, DeleteApplication):
  status: expected 204, got 404

Causes 2 and 3 are the observable consequence of Cause 1: the emulator had already removed both resources, so the two deletes AWS answers 204 for are refused as not-found.

Smithy support: DeleteApplication DELETE /applications/{ApplicationId}, success code 204, output Unit, errors BadRequestException (client, httpError 400) / InternalServerException 500 / ResourceNotFoundException 404; BadRequestException members Message, Reason, Details.

## Steps to Reproduce
1. Replay fixture appconfig-experiment-list-operations and inspect steps teardown#13, teardown#14 and teardown#15.
2. CreateApplication, CreateEnvironment, a feature-flag ConfigurationProfile and then CreateExperimentDefinition under that application.
3. Call DeleteApplication(ApplicationId) while the experiment definition still exists.
   Expected: HTTP 400, x-amzn-errortype BadRequestException, Message "Cannot delete application <applicationId>, because there are still experiment definitions existing under it.", application and experiment definition untouched.
   Observed: HTTP 204, empty body, both destroyed.
4. Call DeleteExperimentDefinition for that definition. Expected 204; observed 404.
5. Call DeleteApplication again. Expected 204; observed 404.
6. Re-replay all 3 bundled fixture#step bindings listed under Traces to confirm the fix.
