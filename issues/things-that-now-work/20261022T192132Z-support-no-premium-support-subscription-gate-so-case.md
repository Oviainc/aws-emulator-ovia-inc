Issue-Source: trace-sweep-divergence
Issue-Trace: support-subscription-required-all-operations
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: support-subscription-required-all-operations
Issue-Shape-Id: com.amazonaws.support#AWSSupport_20130415
Issue-Key: support|AddCommunicationToCase|4xx->4xx|$.__type
Issue-Key: support|AddCommunicationToCase|4xx->4xx|$.message
Issue-Key: support|CreateCase|4xx->2xx|$.__type
Issue-Key: support|CreateCase|4xx->2xx|$.message
Issue-Key: support|CreateCase|4xx->2xx|status
Issue-Key: support|DescribeCases|4xx->2xx|$.__type
Issue-Key: support|DescribeCases|4xx->2xx|$.message
Issue-Key: support|DescribeCases|4xx->2xx|status
Issue-Key: support|DescribeCommunications|4xx->4xx|$.__type
Issue-Key: support|DescribeCommunications|4xx->4xx|$.message
Issue-Key: support|ResolveCase|4xx->4xx|$.__type
Issue-Key: support|ResolveCase|4xx->4xx|$.message
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# support: no Premium Support subscription gate, so case operations succeed or return CaseIdNotFound instead of SubscriptionRequiredException

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddCommunicationToCase

## Description
The emulator's AWS Support implementation has no notion of the account-level Premium Support subscription check that fronts every Support API operation. Because that gate is missing, the Support case operations fall straight through to their normal handlers, so the caller sees either an ordinary success or an unrelated resource-lookup error where AWS refuses the call outright. This is ONE shared fault with FIVE causes, one per operation exercised by the fixture; all five must be closed.

AWS (shared): the Support service documentation states that "If you call the Amazon Web Services Support API from an account that doesn't have an Amazon Web Services Business Support+, Amazon Web Services Enterprise Support, or Amazon Web Services Unified Operations plan, the SubscriptionRequiredException error message appears" (Smithy: service AWSSupport_20130415 documentation, sdkId Support, protocol awsJson1_1). SubscriptionRequiredException is NOT a modeled shape in the on-disk model (smithy_query shape=SubscriptionRequiredException -> not found) and it is not in any operation's errors list nor in common_errors (only InternalServerError), so its wire form is recorded behavior only: the recording shows HTTP 400 with awsJson1_1 body field $.__type = 'SubscriptionRequiredException'. The status and the __type code are the requirement; the recorded message text 'Amazon Web Services Premium Support Subscription is required to use this service.' is an example of the message, not a requirement. The gate precedes request/resource validation, so it fires even for a caseId that does not exist.

Cause 1 of 5: AddCommunicationToCase (fixture support-subscription-required-all-operations, step task#2).
  AWS: 400 with $.__type 'SubscriptionRequiredException'.
  Emulator: 400, but $.__type is 'CaseIdNotFound' and $.message is 'The requested CaseId could not be located.' (CaseIdNotFound IS a modeled error for this op, httpError 400 defaulted) - i.e. it reached case lookup instead of refusing the call.
  Why: a client distinguishing "my account cannot use this API" from "that case does not exist" branches the wrong way - it will retry with other case ids forever instead of surfacing a support-plan problem.
  Fix: the subscription gate belongs ahead of the AddCommunicationToCase handler's case lookup, in the service-level request pre-check / dispatch path for support.

Cause 2 of 5: CreateCase (fixture support-subscription-required-all-operations, step task#4).
  AWS: 400 with $.__type 'SubscriptionRequiredException'; no case is created.
  Emulator: HTTP 200 with a normal CreateCase success body; $.__type and $.message absent.
  Why: the client believes a support case was opened when AWS would have created nothing, so the case id it stores refers to a case that does not exist in the real service.
  Fix: the CreateCase handler (or the shared support pre-dispatch check) must refuse before mutating case state.

Cause 3 of 5: DescribeCases (fixture support-subscription-required-all-operations, step task#7).
  AWS: 400 with $.__type 'SubscriptionRequiredException'.
  Emulator: HTTP 200 with a normal DescribeCases body; $.__type and $.message absent.
  Why: a caller probing whether the Support API is usable concludes it is, and silently reports an empty/synthetic case list instead of the subscription failure.
  Fix: same shared support pre-dispatch check, applied to the read path too.

Cause 4 of 5: DescribeCommunications (fixture support-subscription-required-all-operations, step task#8).
  AWS: 400 with $.__type 'SubscriptionRequiredException'.
  Emulator: 400 with $.__type 'CaseIdNotFound' and message 'The requested CaseId could not be located.'.
  Why: same mis-branching as Cause 1 - an account-level refusal is reported as a per-case not-found.
  Fix: gate must run before the DescribeCommunications handler resolves caseId.

Cause 5 of 5: ResolveCase (fixture support-subscription-required-all-operations, step task#15).
  AWS: 400 with $.__type 'SubscriptionRequiredException'.
  Emulator: 400 with $.__type 'CaseIdNotFound' and message 'The requested CaseId could not be located.'.
  Why: a client cleaning up cases treats the failure as "already gone" and proceeds, losing the fact that it has no access to the Support API at all.
  Fix: gate must run before the ResolveCase handler resolves caseId.

Fix (shared): add an account/subscription state flag to the support service state and a single pre-handler check in the support dispatch path that, when the account has no qualifying plan, serializes an awsJson1_1 error with HTTP 400 and __type 'SubscriptionRequiredException' (the exception shape also has to be added, since it is absent from the model's error set). The check must fire for every bound operation, ahead of input and resource validation, which is exactly the shape of the recorded fixture (it covers the whole operation surface).

Related open issues: none of the four currently-open issues concern support (they cover textract, build-support glob re-exports, and an emr stub), so none of them covers these causes.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture support-subscription-required-all-operations (15 steps, 0 passed, 15 mismatched, 5 divergence clusters, score 0.0%, state_reset_failures 0, bootstrap_signal false, emulator_up true). No cluster carried a stub_path or owned_by, so none is already filed.

AddCommunicationToCase - step task#2: expected status 400 / actual 400; $.__type expected 'SubscriptionRequiredException', got 'CaseIdNotFound'; $.message expected 'Amazon Web Services Premium Support Subscription is required to use this service.', got 'The requested CaseId could not be located.'
CreateCase - step task#4: status expected 400, got 200; $.__type expected 'SubscriptionRequiredException', got None; $.message expected 'Amazon Web Services Premium Support Subscription is required to use this service.', got None
DescribeCases - step task#7: status expected 400, got 200; $.__type expected 'SubscriptionRequiredException', got None; $.message expected 'Amazon Web Services Premium Support Subscription is required to use this service.', got None
DescribeCommunications - step task#8: expected status 400 / actual 400; $.__type expected 'SubscriptionRequiredException', got 'CaseIdNotFound'; $.message expected 'Amazon Web Services Premium Support Subscription is required to use this service.', got 'The requested CaseId could not be located.'
ResolveCase - step task#15: expected status 400 / actual 400; $.__type expected 'SubscriptionRequiredException', got 'CaseIdNotFound'; $.message expected 'Amazon Web Services Premium Support Subscription is required to use this service.', got 'The requested CaseId could not be located.'

Model evidence: smithy_query service_info -> sdkId Support, endpointPrefix support, protocol awsJson1_1, common_errors [InternalServerError]; smithy_query shape=SubscriptionRequiredException -> 'shape not found'; CaseIdNotFound -> client error, httpError 400 (defaulted), member message. Service documentation (smithy_query doc=AWSSupport_20130415) states SubscriptionRequiredException is returned for accounts without a qualifying Support plan.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay wire fixture support-subscription-required-all-operations from trace set simparity-wire-aws-v1 against it (awsJson1_1, X-Amz-Target AWSSupport_20130415.<Operation>, host/endpointPrefix support), with the account configured as having NO Premium Support subscription, as the fixture records.
3. Step task#2 (AddCommunicationToCase) and steps task#8 (DescribeCommunications) / task#15 (ResolveCase): observe HTTP 400 with __type 'CaseIdNotFound' instead of __type 'SubscriptionRequiredException'.
4. Steps task#4 (CreateCase) and task#7 (DescribeCases): observe HTTP 200 with a normal success body instead of HTTP 400 with __type 'SubscriptionRequiredException'.
5. Expected for all five steps: HTTP 400, awsJson1_1 error body with $.__type = 'SubscriptionRequiredException' (message per AWS's subscription-required text, e.g. 'Amazon Web Services Premium Support Subscription is required to use this service.'), raised before any caseId lookup or state mutation.
