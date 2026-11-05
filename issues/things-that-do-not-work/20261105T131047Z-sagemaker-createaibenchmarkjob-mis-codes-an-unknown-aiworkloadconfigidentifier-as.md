Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-ai-benchmark-recommendation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-ai-benchmark-recommendation-errors
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->4xx|$.Message
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->4xx|$.__type
Issue-Commit: 034b8904e79b91183249d6d72900938b66cbdc8c

# SageMaker CreateAIBenchmarkJob mis-codes an unknown AIWorkloadConfigIdentifier as ValidationException with no message

> _Note: this issue was identified against the code base at commit `034b8904e79b91183249d6d72900938b66cbdc8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAIBenchmarkJob

## Description
Cause 1 of 1: CreateAIBenchmarkJob refuses a request whose AIWorkloadConfigIdentifier names a workload config that does not exist in the account, but it refuses it with the wrong error shape and with no message member at all.

AWS: Smithy model services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json — CreateAIBenchmarkJob binds exactly three errors: ResourceInUse, ResourceLimitExceeded and ResourceNotFound (all errorKind=client, httpError 400). ValidationException is NOT bound to this operation. AIWorkloadConfigIdentifier is a required member targeting AIResourceIdentifier (@length 1..256, @pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:[a-z\-]*/)?([a-zA-Z0-9]([a-zA-Z0-9\-]){0,62})(?<!-)$); a well-formed identifier that satisfies that pattern is NOT a constraint violation, so a value that merely resolves to nothing is a referenced-resource lookup failure, not a request-shape failure. ResourceNotFound carries one member, Message (target FailureReason, @length 0..1024), documented "Resource being access is not found." The recorded response for this step is HTTP 400 with __type 'ResourceNotFound' and Message "AIWorkloadConfig '<identifier>' not found for this account." — fixed text "AIWorkloadConfig '" ... "' not found for this account." with the single-quoted identifier taken verbatim from the request's AIWorkloadConfigIdentifier member (in the recording, 'sim2real-probe-sagemaker-missing-ai-workload-config-dd57e8e6'). The recording is the authority for the wording; the model is the authority for the error shape and the member name.

Emulator: answers HTTP 400 (status matches) but with __type 'ValidationException' and NO Message member at all ($.Message missing, got None). So the body carries neither the modeled error code nor any human-readable reason.

Why: a client that branches on the error code sees a request-validation failure where AWS reports a missing referenced resource. SDK error types differ (ResourceNotFound vs ValidationException), so retry/catch logic keyed on ResourceNotFound never fires and provisioning code that would create the missing AIWorkloadConfig and retry instead reports the caller's request as malformed. The absent Message also means the identifier that failed to resolve is never surfaced, so neither a human nor a log scrape can tell WHICH workload config was missing — and the message member the model declares is simply not serialized for this path.

Fix: the CreateAIBenchmarkJob handler's referenced-resource resolution step (the AIWorkloadConfigIdentifier lookup) must, on a miss, raise the modeled ResourceNotFound error — not fall through to the generic ValidationException path used for request-shape violations — and must populate ResourceNotFound.Message with the "AIWorkloadConfig '<AIWorkloadConfigIdentifier>' not found for this account." template, interpolating the identifier from the request. The error serializer must emit __type 'ResourceNotFound' and the Message member for that error.

Related but NOT covering: open issue 20261029T222441Z-sagemaker-resourcenotfound-validationexception-refusal-texts-are-emulator-authored.md covers emulator-authored refusal texts for ten other resource families, and 20261024T160943Z-sagemaker-control-plane-gaps-createuserprofile-and-deleteaibenchmarkjob-are.md covers DeleteAIBenchmarkJob being a 501 stub. Neither carries this step's replay key, and the replay reports this cluster unowned.

Traces: sagemaker-ai-benchmark-recommendation-errors#task#1.

## Evidence
Fixture sagemaker-ai-benchmark-recommendation-errors, step task#1, operation CreateAIBenchmarkJob, service sagemaker.
Expected status 400, actual status 400 (status agrees).
Body divergences reported by replay:
  $.__type: value_mismatch (expected 'ResourceNotFound', got 'ValidationException')
  $.Message: missing (expected "AIWorkloadConfig 'sim2real-probe-sagemaker-missing-ai-workload-config-dd57e8e6' not found for this account.", got None)

Expected (AWS):
  HTTP 400
  {"__type": "ResourceNotFound", "Message": "AIWorkloadConfig '<AIWorkloadConfigIdentifier>' not found for this account."}
Actual (emulator):
  HTTP 400
  {"__type": "ValidationException"}   (no Message member)

Message anatomy: fixed text = "AIWorkloadConfig '" + "' not found for this account."; variable part = the request's AIWorkloadConfigIdentifier value, single-quoted. The recording's 'sim2real-probe-sagemaker-missing-ai-workload-config-dd57e8e6' is one instantiation of the template, not part of it.

Model evidence: smithy_query operation=CreateAIBenchmarkJob — errors = [ResourceInUse, ResourceLimitExceeded, ResourceNotFound], each httpError 400, errorKind client; ValidationException is not bound. smithy_query shape=ResourceNotFound — structure, errorKind client, httpError 400, members {Message: FailureReason @length 0..1024}, doc "Resource being access is not found."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture sagemaker-ai-benchmark-recommendation-errors, step task#1: POST a SageMaker CreateAIBenchmarkJob call (JSON 1.1, X-Amz-Target SageMaker.CreateAIBenchmarkJob) with a valid AIBenchmarkJobName, BenchmarkTarget, OutputConfig and RoleArn, and an AIWorkloadConfigIdentifier that satisfies the AIResourceIdentifier pattern but names no existing AIWorkloadConfig in the account (e.g. a probe-generated name).
3. Observe HTTP 400 with body {"__type": "ValidationException"} and no Message member.
4. Expected: HTTP 400 with {"__type": "ResourceNotFound", "Message": "AIWorkloadConfig '<the identifier sent>' not found for this account."}.
