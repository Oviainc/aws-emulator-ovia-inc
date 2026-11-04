Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-parameter-labels-and-policy-validation
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-parameter-labels-and-policy-validation ssm-parameter-resource-policy-lifecycle ssm-resource-policy-error-paths ssm-resource-policy-lifecycle ssm-resource-policy-delete-error-paths ssm-resource-policy-errors ssm-resource-policy-put-error-paths
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetResourcePolicies|4xx->2xx|$.Message
Issue-Key: ssm|GetResourcePolicies|4xx->2xx|$.__type
Issue-Key: ssm|GetResourcePolicies|4xx->2xx|status
Issue-Key: ssm|GetResourcePolicies|2xx->2xx|$.Policies[*]
Issue-Key: ssm|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: ssm|DeleteResourcePolicy|4xx->4xx|$.Message
Issue-Key: ssm|DeleteResourcePolicy|4xx->4xx|$.__type
Issue-Key: ssm|PutResourcePolicy|4xx->4xx|$.Message
Issue-Key: ssm|PutResourcePolicy|4xx->2xx|$.__type
Issue-Key: ssm|PutResourcePolicy|4xx->2xx|$.message
Issue-Key: ssm|PutResourcePolicy|4xx->2xx|status
Issue-Key: ssm|PutResourcePolicy|4xx->4xx|$.message
Issue-Commit: 208501bc18e68434d3edfe944c65f90258a97b7e

# ssm Parameter Store resource-policy operations skip resource and tier gating and author their own refusal wording (7 causes)

> _Note: this issue was identified against the code base at commit `208501bc18e68434d3edfe944c65f90258a97b7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
The emulator's SSM resource-policy trio (PutResourcePolicy / GetResourcePolicies / DeleteResourcePolicy) diverges from AWS in one shared way: it does not gate a call on the TARGET RESOURCE (existence, ownership, parameter tier) before it looks at the policy store, it re-serializes the stored policy document instead of returning the text it was given, and where it does refuse it invents its own error class, its own message member name and its own message text instead of AWS's. 7 distinct causes, ALL of which must be closed before this issue is submitted.

No open issue covers this surface. The nearest open issues are for OTHER services' resource-policy planes (e.g. 20261029T202053Z-kinesis-resource-policy-operations-skip-stream-state-gating.md, 20261029T233239Z-lambda-resource-policy-documents-are-re-serialized-with.md, 20261030T060227Z-dynamodb-resource-policy-refusals-name-the-wrong-resource.md, 20261104T045922Z-redshift-resource-policy-and-namespace-registration-semantics-diverge.md); none of them touches ssm. The open ssm issues (20261103T173026Z-ssm-parameter-store-error-classes-and-messages-are-emulator.md and siblings) own the Parameter Store get/put/document paths, not the resource-policy operations below.

--- Cause 1 of 7: GetResourcePolicies on a resource that does not exist answers 200 instead of ResourceNotFoundException ---
AWS: GetResourcePolicies is modeled with ResourceNotFoundException (Smithy: GetResourcePolicies errors = InternalServerError, ResourceNotFoundException, ResourcePolicyInvalidParameterException; ResourceNotFoundException httpError 400, member `Message`). When the ResourceArn names a resource that is absent (or not owned by the caller), AWS refuses with HTTP 400, `__type` = "ResourceNotFoundException" and `Message` = "Resource with ARN <resource-arn> does not exist or you are not the owner." — the fixed text is "Resource with ARN " ... " does not exist or you are not the owner." and the only variable part is the ARN echoed from the request's ResourceArn (in the recording, a document ARN).
Emulator: answers HTTP 200 with a policy list and no `__type`/`Message` at all — it treats "no policies stored for this ARN" as an empty, successful read and never checks whether the ARN resolves to a resource.
Why: a client probing whether a resource is shared cannot distinguish "resource absent / not mine" from "resource exists with no policy". Code that uses GetResourcePolicies as an existence or ownership check silently takes the success branch and proceeds against a resource that is not there.
Fix: GetResourcePolicies' handler — resolve ResourceArn against the resource store (parameter/document/OpsItemGroup) and raise ResourceNotFoundException before reading the policy store.
Traces: ssm-parameter-labels-and-policy-validation#task#6 (expected 400 ResourceNotFoundException, got 200).

--- Cause 2 of 7: GetResourcePolicies returns a re-serialized, pretty-printed policy document instead of the text that was put ---
AWS: GetResourcePoliciesResponseEntry.Policy is a plain string (Smithy: GetResourcePoliciesResponseEntry members PolicyId/PolicyHash/Policy, Policy pattern ^(?!\s*$).+$ — no structural normalization is modeled). AWS stores and returns the policy document as supplied: the string read back is byte-for-byte the string passed to PutResourcePolicy, with the same key order and no inserted whitespace or newlines.
Emulator: round-trips the document through a JSON object and re-emits it with inserted newlines and spaces around separators ("\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {\n ..."), so the returned Policy differs from the one stored even though PolicyId and PolicyHash match.
Why: a client that compares the read-back policy with the document it submitted (idempotent reconciliation, drift detection, config checks) sees a permanent difference and re-puts the policy forever; any client that hashes or diffs the document text gets a value AWS would never return.
Fix: GetResourcePolicies' serializer/state — persist the Policy string verbatim on PutResourcePolicy and emit the stored string unchanged, rather than parsing and re-printing it.
Traces: ssm-parameter-resource-policy-lifecycle#task#4 (representative; same cause at ssm-parameter-resource-policy-lifecycle#task#6, ssm-resource-policy-error-paths#task#8, ssm-resource-policy-lifecycle#task#3, ssm-resource-policy-lifecycle#task#5).

--- Cause 3 of 7: DeleteResourcePolicy's policy-not-found refusal carries no lowercase `message` member ---
AWS: DeleteResourcePolicy is modeled with ResourcePolicyNotFoundException (Smithy: DeleteResourcePolicy errors include ResourcePolicyNotFoundException, httpError 400). When the resource EXISTS but the supplied PolicyId/PolicyHash pair matches no stored policy, AWS returns HTTP 400 carrying the diagnostic in a `message` member whose text is "Resource policy for resource <resource-arn> with policy ID <policy-id> and policy hash <policy-hash> does not exist" — fixed text "Resource policy for resource " / " with policy ID " / " and policy hash " / " does not exist", with the ARN, policy id and policy hash taken verbatim from the request.
Emulator: returns the right status and error code but no `message` member at all (the replay reports `$.message: missing`), so the body has no readable diagnostic on that key.
Why: SDK error handling reads the message key AWS populates; with it absent the client surfaces an empty error string, and a caller cannot tell which of the three request values (ARN, id, hash) was wrong.
Fix: DeleteResourcePolicy's error serializer — populate the same message member AWS uses for this error class, with the request's ResourceArn, PolicyId and PolicyHash interpolated into the template above.
Traces: ssm-resource-policy-delete-error-paths#task#2 (representative; same cause at ssm-resource-policy-error-paths#task#6, ssm-resource-policy-errors#task#4).

--- Cause 4 of 7: DeleteResourcePolicy on an absent resource reports ResourcePolicyNotFoundException instead of ResourceNotFoundException ---
AWS: DeleteResourcePolicy models BOTH ResourceNotFoundException and ResourcePolicyNotFoundException (Smithy: both httpError 400, both with member `Message`), and they are not interchangeable: the resource is checked FIRST. When the ResourceArn names a parameter that does not exist, AWS answers `__type` = "ResourceNotFoundException" with `Message` = "Parameter <resource-arn> not found" — fixed text "Parameter " ... " not found", the ARN taken from the request.
Emulator: answers `__type` = "ResourcePolicyNotFoundException" with `Message` = "No policies with the specified policy ID and hash could be found." (the model's documentation sentence for that shape), i.e. it only ever consults the policy store and never discovers that the resource itself is missing.
Why: the two classes drive different client behavior — ResourceNotFoundException means "create the resource", ResourcePolicyNotFoundException means "the delete is already satisfied". Retry/cleanup logic that treats policy-not-found as success will mark a delete done for a resource that was never provisioned.
Fix: DeleteResourcePolicy's handler — resolve and validate ResourceArn before the PolicyId/PolicyHash lookup, and raise ResourceNotFoundException with the "Parameter <arn> not found" wording for an unresolvable ARN.
Traces: ssm-resource-policy-delete-error-paths#task#3.

--- Cause 5 of 7: PutResourcePolicy's resource-not-found refusal uses the shape's doc sentence instead of AWS's message ---
AWS: PutResourcePolicy models ResourceNotFoundException (Smithy: httpError 400, member `Message`). For an unresolvable ResourceArn AWS returns `Message` = "Parameter <resource-arn> not found" — fixed text "Parameter " ... " not found" with the ARN echoed from the request, the same wording as the delete path.
Emulator: returns the correct status and error code but the message "The specified parameter to be shared could not be found.", which is the Smithy documentation string for ResourceNotFoundException, not the message AWS puts on the wire. It names no ARN at all.
Why: the message is part of the error contract. A client (or an operator reading a log) gets no indication of WHICH ARN failed, and any assertion or log-matching rule written against AWS's wording does not fire.
Fix: PutResourcePolicy's error serializer — emit "Parameter <ResourceArn> not found" for ResourceNotFoundException instead of the shape's documentation text.
Traces: ssm-resource-policy-error-paths#task#2 (representative; same cause at ssm-resource-policy-put-error-paths#task#3).

--- Cause 6 of 7: PutResourcePolicy accepts a policy on a Standard-tier parameter that AWS refuses ---
AWS: only Advanced-tier parameters may carry a resource policy. PutResourcePolicy against a Standard-tier parameter is refused with HTTP 400, `__type` = "ValidationException" (Smithy: ssm#ValidationException, httpError 400) and the message "Only advanced parameters can be shared" — the whole string is fixed text with no variable part.
Emulator: answers HTTP 200 and stores the policy, with no `__type` or message, so a Standard parameter ends up carrying a resource policy that cannot exist in AWS.
Why: a client that relies on the refusal to discover it must first upgrade the parameter's tier never learns that, and then believes a cross-account share is in place on a parameter AWS would never share — a silent, permissive divergence on an access-control path.
Fix: PutResourcePolicy's validation — read the target parameter's tier from the parameter store and raise ValidationException with the fixed "Only advanced parameters can be shared" message unless the tier is Advanced.
Traces: ssm-resource-policy-error-paths#task#3 (expected 400 ValidationException, got 200).

--- Cause 7 of 7: PutResourcePolicy's malformed-document refusal carries no lowercase `message` member ---
AWS: PutResourcePolicy models MalformedResourcePolicyDocumentException (Smithy: httpError 400). When the Policy string is not a parseable policy document AWS returns HTTP 400 carrying the diagnostic in a `message` member whose text is "Policy is malformed and cannot be parsed" — entirely fixed text, no variable part.
Emulator: returns the right status and error code but no `message` member at all (`$.message: missing`), so the refusal arrives with an empty diagnostic.
Why: the client sees an error with no reason; nothing distinguishes a syntactically broken document from any other 400 on this operation, so a caller cannot tell the user to fix the policy JSON.
Fix: PutResourcePolicy's error serializer — populate the message member AWS uses for MalformedResourcePolicyDocumentException with the fixed text above.
Traces: ssm-resource-policy-errors#task#2 (representative; same cause at ssm-resource-policy-put-error-paths#task#2).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 815 steps, 152 passed). 7 net-new divergence clusters, all service=ssm, none stub-blocked and none owned by an open issue:

Cause 1 — ssm-parameter-labels-and-policy-validation#task#6, GetResourcePolicies:
  status: expected 400, got 200
  $.__type: missing (expected 'ResourceNotFoundException', got None)
  $.Message: missing (expected 'Resource with ARN arn:aws:ssm:us-west-2:000000000000:document/probe-absent-document does not exist or you are not the owner.', got None)
  (ARN is the recording's value filling the template; the requirement is the fixed text plus the request's ResourceArn.)

Cause 2 — ssm-parameter-resource-policy-lifecycle#task#4, GetResourcePolicies (5 steps affected):
  $.Policies[0]: expected Policy '{"Version":"2012-10-17","Statement":[{"Sid":"AllowCrossAccountGet",...}]}' (the exact text that was put), got '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "AllowCrossAccountGet",\n ... } ]\n}'. PolicyId and PolicyHash matched; only the document text was rewritten.

Cause 3 — ssm-resource-policy-delete-error-paths#task#2, DeleteResourcePolicy (3 steps affected), status 400 both sides:
  $.message: missing (expected 'Resource policy for resource arn:aws:ssm:us-west-2:000000000000:parameter/sim2real-probe-ssm-resourcepolicy-delete-param-1ccf1005 with policy ID fabricated-policy-id and policy hash fabricated-policy-hash does not exist', got None)

Cause 4 — ssm-resource-policy-delete-error-paths#task#3, DeleteResourcePolicy, status 400 both sides:
  $.__type: value_mismatch (expected 'ResourceNotFoundException', got 'ResourcePolicyNotFoundException')
  $.Message: value_mismatch (expected 'Parameter arn:aws:ssm:us-west-2:000000000000:parameter/sim2real-probe-ssm-resourcepolicy-delete-missing-1ccf1005 not found', got 'No policies with the specified policy ID and hash could be found.')

Cause 5 — ssm-resource-policy-error-paths#task#2, PutResourcePolicy (2 steps affected), status 400 both sides:
  $.Message: value_mismatch (expected 'Parameter arn:aws:ssm:us-west-2:000000000000:parameter/sim2real-probe-ssm-ghost-param-a4725a3a not found', got 'The specified parameter to be shared could not be found.')

Cause 6 — ssm-resource-policy-error-paths#task#3, PutResourcePolicy:
  status: expected 400, got 200
  $.__type: missing (expected 'ValidationException', got None)
  $.message: missing (expected 'Only advanced parameters can be shared', got None)

Cause 7 — ssm-resource-policy-errors#task#2, PutResourcePolicy (2 steps affected), status 400 both sides:
  $.message: missing (expected 'Policy is malformed and cannot be parsed', got None)

Model evidence (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): PutResourcePolicy errors = InternalServerError, MalformedResourcePolicyDocumentException, ResourceNotFoundException, ResourcePolicyConflictException, ResourcePolicyInvalidParameterException, ResourcePolicyLimitExceededException, ResourcePolicyNotFoundException; GetResourcePolicies errors = InternalServerError, ResourceNotFoundException, ResourcePolicyInvalidParameterException; DeleteResourcePolicy errors include ResourceNotFoundException and ResourcePolicyNotFoundException. All are httpError 400 client errors with a `Message` member; ssm#ValidationException is httpError 400 with Message + ReasonCode. GetResourcePoliciesResponseEntry.Policy is a string (pattern ^(?!\s*$).+$) with no normalization modeled. The "Only advanced parameters can be shared" tier rule and the exact not-found wordings are recorded behavior from the fixtures above.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and look at the ssm clusters whose operation is PutResourcePolicy, GetResourcePolicies or DeleteResourcePolicy.
3. Cause 1: call GetResourcePolicies with a ResourceArn naming a document that was never created -> emulator answers 200 with a policies list; AWS answers 400 ResourceNotFoundException / 'Resource with ARN <arn> does not exist or you are not the owner.' Replay ssm-parameter-labels-and-policy-validation#task#6.
4. Cause 2: PutResourcePolicy a compact single-line policy document on an Advanced parameter, then GetResourcePolicies on the same ARN -> the returned Policies[0].Policy comes back pretty-printed with newlines and spaces instead of the text submitted. Replay ssm-parameter-resource-policy-lifecycle#task#4.
5. Cause 3: on a parameter that EXISTS and has a policy, call DeleteResourcePolicy with a PolicyId/PolicyHash pair that matches nothing -> 400 with no `message` member. Replay ssm-resource-policy-delete-error-paths#task#2.
6. Cause 4: call DeleteResourcePolicy with a ResourceArn for a parameter that does not exist -> emulator reports ResourcePolicyNotFoundException / 'No policies with the specified policy ID and hash could be found.'; AWS reports ResourceNotFoundException / 'Parameter <arn> not found'. Replay ssm-resource-policy-delete-error-paths#task#3.
7. Cause 5: call PutResourcePolicy with a ResourceArn for a parameter that does not exist -> message is the shape's doc sentence 'The specified parameter to be shared could not be found.' instead of 'Parameter <arn> not found'. Replay ssm-resource-policy-error-paths#task#2.
8. Cause 6: PutParameter a Standard-tier parameter, then PutResourcePolicy a valid policy on its ARN -> emulator answers 200 and stores it; AWS answers 400 ValidationException / 'Only advanced parameters can be shared'. Replay ssm-resource-policy-error-paths#task#3.
9. Cause 7: call PutResourcePolicy with a Policy string that is not parseable JSON -> 400 arrives with no `message` member; AWS sends 'Policy is malformed and cannot be parsed'. Replay ssm-resource-policy-errors#task#2.
10. Re-replay every fixture#step in Issue-Trace/Issue-Step and trace_ids to confirm all 7 causes are closed.
