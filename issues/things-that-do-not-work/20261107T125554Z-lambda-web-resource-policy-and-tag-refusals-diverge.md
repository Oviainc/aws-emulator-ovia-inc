Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-web-put-resource-policy-validates-before-existence-check
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-web-put-resource-policy-validates-before-existence-check lambda-web-resource-policy-and-tag-lifecycle lambda-web-resource-policy-and-tags-isolated-across-functions
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: lambda-web|PutResourcePolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-web|PutResourcePolicy|4xx->4xx|status
Issue-Key: lambda-web|PutResourcePolicy|4xx->4xx|$.resourceId
Issue-Key: lambda-web|PutResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: lambda-web|PutResourcePolicy|4xx->2xx|$.message
Issue-Key: lambda-web|PutResourcePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda-web|PutResourcePolicy|4xx->2xx|status
Issue-Key: lambda-web|DeleteResourcePolicy|4xx->4xx|$.resourceId
Issue-Key: lambda-web|DeleteResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: lambda-web|GetResourcePolicy|4xx->4xx|$.resourceId
Issue-Key: lambda-web|GetResourcePolicy|4xx->4xx|$.resourceType
Issue-Key: lambda-web|TagResource|4xx->4xx|$.message
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# lambda-web resource-policy and tag refusals diverge: validation order, an accepted bad policy, wrong wording and extra error members

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
SIX distinct, net-new root causes in the lambda-web emulator's resource-policy and tagging error contract, all reached by the replayed wire corpus. They are numbered below, each with its own fixture and step; ALL SIX must be closed before this issue is submitted.

--------------------------------------------------------------------
Cause 1 of 6: PutResourcePolicy checks that the web function exists BEFORE it validates the policy document, so a malformed policy aimed at an absent function is reported as a 404 not-found instead of AWS's 400 validation refusal.
Fixture/step: lambda-web-put-resource-policy-validates-before-existence-check#task#1

AWS: Smithy (services/service-com-amazonaws-lambdaweb-lambdaweb/smithy-model/com-amazonaws-lambdaweb-lambdaweb.json), service LambdaWeb, restJson1. PutResourcePolicy = PUT /2025-03-07/resource-policy/{resourceArn}, @idempotent, success 200; input resourceArn (ResourceArn, @required, @httpLabel, @length 1..256, @pattern ^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$), policy (ResourcePolicy, @required, @length 1..20480, documented as "The JSON-formatted resource-based policy to attach to the web function."), revisionId (@pattern ^[0-9a-f-]{36}$). It models BOTH ValidationException (client, httpError 400, @required message, doc "The request failed validation because a parameter was missing, malformed, or outside the allowed range.") and ResourceNotFoundException (client, httpError 404). The model does not fix the ORDER, so the order is recorded behavior only: for a request whose policy document is not a well-formed policy AND whose web function does not exist, AWS answers HTTP 400, error code ValidationException, message `Policy has syntax errors.` — the whole message is fixed text with NO request- or state-derived parts. Request validation is therefore performed before the resource is resolved, and no not-found is reported for a request that never passed validation.

Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and message `The web function <resourceArn> doesn't exist.` — the existence lookup is executed first and returns before the policy check is ever reached.

Why: a client that submits a bad policy against a mistyped/absent function ARN is told the function is missing rather than that its policy is malformed, so it retries resource discovery instead of fixing the document; and the error class a caller branches on (400 retry-never vs 404 create-the-resource) is inverted. SDK/Terraform-style error mapping and any test that pins AWS's validate-then-resolve ordering take the wrong branch.

Fix: the PutResourcePolicy handler's ordering — the policy-document validation must run before the function lookup, i.e. the validation branch has to precede the store lookup in that operation's handler (see Traces for the current ordering).

--------------------------------------------------------------------
Cause 2 of 6: PutResourcePolicy accepts a policy document AWS refuses as syntactically invalid, answering 200 and attaching it with a fresh revisionId.
Fixture/step: lambda-web-resource-policy-and-tag-lifecycle#task#7

AWS: Smithy, as in Cause 1: `policy` is documented as a JSON-formatted RESOURCE-BASED POLICY, and the only modelled constraint is @length 1..20480 (no @pattern), so the document's well-formedness is service-applied and the refusal text is recorded behavior only. For a policy document that is not a well-formed resource-based policy, AWS refuses the whole call with HTTP 400, error code ValidationException and the single fixed sentence `Policy has syntax errors.` (entirely fixed text; it names neither the document, nor a member path, nor an offset). The refusal is total: nothing is attached and the stored policy/revision are unchanged.

Emulator: answers HTTP 200 with a PutResourcePolicyResponse (Policy echoed, a freshly minted RevisionId) and stores the document. Its only acceptance rule is "the member parses as JSON and the top-level value is an object", so any JSON object passes regardless of whether it is a policy at all. The replay shows the step expecting 400 / `Policy has syntax errors.` / a ValidationException errortype header and getting a 200 with no error body at all.

Why: the emulator silently accepts documents AWS rejects, so a client's malformed policy appears to have been applied; later GetResourcePolicy reports a policy real AWS would never have stored, and negative tests that assert the 400 refusal (and that the previous generation survived) pass only against AWS. This is the most severe cause here: an accept where AWS refuses cannot be detected by the caller.

Fix: the policy-document acceptance rule used by the PutResourcePolicy handler's validation step (the resource-policy slice of the service state that defines it) must hold the document to a well-formed resource-based policy, not merely to "is a JSON object", and report the modelled ValidationException with AWS's fixed sentence. Hypothesis: the fixture's document parses as a JSON object but is not a usable policy (e.g. it lacks the Statement/Version elements or carries a malformed statement element), which is exactly the gap between the two rules.

--------------------------------------------------------------------
Cause 3 of 6: the ResourceNotFoundException message for a web function that is not stored uses the emulator's own wording instead of AWS's.
Fixture/step: lambda-web-put-resource-policy-validates-before-existence-check#task#2

AWS: Smithy, ResourceNotFoundException is a client error at httpError 404 with @required `message`, shape doc "The specified web function, revision, endpoint, or resource-based policy doesn't exist. Verify the resource identifier and try again." The model does not fix the text, so the text is recorded behavior only: addressed at a web-function ARN whose function does not exist, AWS answers HTTP 404, code ResourceNotFoundException, message `The function <resourceArn> does not exist.` FIXED text: "The function " and " does not exist."; the only VARIABLE part is the web-function ARN, taken verbatim from the request's resourceArn URI label (the recording's particular ARN is only an instance of that variable and is not part of the requirement).

Emulator: right status (404) and right code, wrong sentence: `The web function <resourceArn> doesn't exist.` — fixed text "The web function " / " doesn't exist.", same ARN variable. The same literal is spelled in several handlers (PutResourcePolicy, GetResourcePolicy, DeleteResourcePolicy and the tag handlers), so every reader diverges identically.

Why: message is the only way to distinguish the modelled "function gone" case from the "function exists but carries no policy" case, since both are the same 404/ResourceNotFoundException; a client branching on AWS's sentence mis-classifies, and golden-output assertions fail.

Fix: the message construction in the not-found branch of the resource-policy and tag handlers, ideally hoisted to one constant in the function slice of the service state so one sentence serves every reader. Do not change the status or the code, which are correct.

Related but NOT covering this: open issue 20261107T082505Z-lambda-web-resource-not-found-and-reserved-tag.md (behavioral corpus) describes the same wording for GetResourcePolicy/ListTags but carries no wire replay key, so this wire step is filed here.

--------------------------------------------------------------------
Cause 4 of 6: every resource-policy refusal populates the optional resourceId/resourceType members of the error body, which AWS leaves unset.
Fixtures/steps: lambda-web-put-resource-policy-validates-before-existence-check#task#2 (PutResourcePolicy 404), lambda-web-resource-policy-and-tag-lifecycle#task#10 (PutResourcePolicy 409), lambda-web-resource-policy-and-tag-lifecycle#task#11 (DeleteResourcePolicy 409), lambda-web-resource-policy-and-tag-lifecycle#task#13 (GetResourcePolicy 404; the same cluster also covers lambda-web-resource-policy-and-tags-isolated-across-functions#task#18)

AWS: Smithy, ResourceNotFoundException members = message (@required), resourceId (optional), resourceType (optional); ConflictException members = message (@required), resourceId (optional), resourceType (optional). Both extra members are OPTIONAL, and whether they are emitted is recorded behavior only: in the recordings AWS's resource-policy refusals carry ONLY `message` — resourceId and resourceType are absent from the JSON body for the 404 (function absent, and function present but carrying no policy) and for the 409 (stale revisionId) alike.

Emulator: emits both members on every one of those refusals — resourceId set to the request's resourceArn and resourceType set to "WebFunction" (function-absent branches) or "ResourcePolicy" (policy-absent and stale-revision branches). The replay reports `$.resourceId: type_mismatch (expected None, got ...)` and `$.resourceType: type_mismatch (expected None, got ...)` on all four steps.

Why: clients and assertion harnesses that compare the error document against AWS's shape see two unexpected members; tooling that keys off a populated resourceType (treating its presence as "the service told me the resource class") behaves differently against the emulator than against AWS, and an emulator-invented resourceType ("ResourcePolicy") is not a value AWS ever publishes for these errors.

Fix: the error construction in the resource-policy handlers (and the shared state constant that supplies the resourceType string) — build these refusals with `message` only, leaving resourceId/resourceType unset so the serializer omits them.

--------------------------------------------------------------------
Cause 5 of 6: the stale-revision ConflictException message from PutResourcePolicy is not AWS's wording.
Fixture/step: lambda-web-resource-policy-and-tag-lifecycle#task#10

AWS: Smithy, PutResourcePolicy models ConflictException (client, httpError 409, @required message) and documents revisionId as "The revision ID of the existing policy. Use this to prevent conflicts when updating a policy concurrently..."; the ConflictException doc covers "...or the policy revision ID that you specified doesn't match the current policy. Resolve the conflict and try again." The text is recorded behavior only: for a put carrying a revisionId that is not the stored policy's current generation, AWS answers HTTP 409, code ConflictException, message `The policy was updated or deleted by another request. Try again with the latest policy revisionId.` — entirely FIXED text, with NO variable parts (no revision id, no ARN, no function name).

Emulator: 409 and ConflictException are right; the message is `The policy was updated by another request. Try again.` — it drops "or deleted" and the "Try again with the latest policy revisionId." instruction. The same constant backs DeleteResourcePolicy's stale-revision refusal, so that path carries the same wrong sentence.

Why: a client that matches the documented stale-revision sentence to decide "re-read the policy and retry with the latest revisionId" versus "surface an unexpected conflict" takes the wrong branch, and the message is part of the error contract even when status and code are right.

Fix: the shared stale-revision message constant in the resource-policy slice of the service state that both mutating resource-policy handlers read.

Related but NOT covering this: open issue 20261106T141524Z-lambda-web-the-stale-revision-conflictexception-message-from.md states the same wording from the behavioral corpus but carries no wire replay key for this fixture step.

--------------------------------------------------------------------
Cause 6 of 6: TagResource's refusal of a reserved `aws:`-prefixed tag key reports the framework's per-violation constraint sentence instead of AWS's tag-validation sentence.
Fixture/step: lambda-web-resource-policy-and-tag-lifecycle#task#15

AWS: Smithy, TagResource = POST /2025-03-07/tags/{resource}, @idempotent, success 204; input resource (ResourceArn, @required, @httpLabel) and tags (Tags, @required, @length 0..200); ValidationException is a client error at httpError 400 with @required message. TagKey admits a colon (@length 1..128, @pattern ^[a-zA-Z0-9 _\.:/=+\-@]+$), so the reserved-namespace rule is NOT a modelled constraint and its text is recorded behavior only: AWS refuses the whole call with HTTP 400, code ValidationException and the single sentence `One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.` That message has NO variable parts — it names neither the offending key nor a member path — and it covers the three rules it lists (reserved prefix, empty key, duplicate keys) with one text.

Emulator: the 400, the ValidationException code and the all-or-nothing refusal are right, but the message is the framework's per-violation clause inside the shared count envelope: `<n> validation error detected: Value '<key>' at 'tags.<key>' failed to satisfy constraint: Member must not begin with the reserved prefix aws:` — its variable parts are the violation count and the submitted key (twice: as the value and inside the internal `tags.<key>` member path), none of which appear in AWS's sentence. The same wrong wording is served from the shared requirement constant for the UntagResource key-list path.

Why: a client or provider that asserts on / branches over AWS's documented tag-validation message sees a different sentence and a leaked internal member path, so a reserved-prefix rejection is indistinguishable from an ordinary modelled-constraint rejection; the text is also over-specific, echoing the caller's key, which AWS does not.

Fix: this service's tag validation — the tag slice of the service state that builds the reserved-key refusal text and the TagResource/UntagResource handlers that return it. Emit AWS's single fixed sentence without the count envelope and without the member path; keep the existing 400/ValidationException classification and the whole-call refusal.

Related but NOT covering this: open issue 20261107T082505Z-lambda-web-resource-not-found-and-reserved-tag.md (Cause 2) states the same requirement from the behavioral corpus; it carries no wire replay key for this fixture step, so the wire cause is filed here.

--------------------------------------------------------------------
Traces (failing fixture steps, as examples; file/line citations are against this checkout and may have drifted):
- Cause 1: lambda-web-put-resource-policy-validates-before-existence-check#task#1 — services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/put_resource_policy.rs:190-199 (existence lookup) precedes :204-211 (policy validation).
- Cause 2: lambda-web-resource-policy-and-tag-lifecycle#task#7 — src/state/resource_policies.rs:73-76 (`is_policy_document`: "parses as JSON and is an object") and :49 (POLICY_SYNTAX_ERROR_MESSAGE), called from src/ops/put_resource_policy.rs:204.
- Cause 3: lambda-web-put-resource-policy-validates-before-existence-check#task#2 — src/ops/put_resource_policy.rs:193, src/ops/get_resource_policy.rs:172, src/ops/delete_resource_policy.rs:197 (all `format!("The web function {} doesn't exist.", ...)`), with name/ARN resolution in src/state.rs (`resolve_function_name`).
- Cause 4: lambda-web-put-resource-policy-validates-before-existence-check#task#2, lambda-web-resource-policy-and-tag-lifecycle#task#10, #task#11, #task#13 (and lambda-web-resource-policy-and-tags-isolated-across-functions#task#18 in that cluster) — src/ops/put_resource_policy.rs:194-195 and :223-226, src/ops/get_resource_policy.rs:173-174 and :184-185, src/ops/delete_resource_policy.rs:198-199, :209-210 and :223-224; resourceType constant at src/state/resource_policies.rs:26.
- Cause 5: lambda-web-resource-policy-and-tag-lifecycle#task#10 — src/state/resource_policies.rs:61-62 (STALE_POLICY_REVISION_MESSAGE), raised at src/ops/put_resource_policy.rs:220-230 and src/ops/delete_resource_policy.rs:220-228.
- Cause 6: lambda-web-resource-policy-and-tag-lifecycle#task#15 — src/state/tags.rs (TAG_KEY_REQUIREMENT / `stated_requirement` wrapping it in service_common::validation_message::envelope_over / `tag_map_violation` / `tag_key_list_violation`), called from src/ops/tag_resource.rs.

## Evidence
Replay of trace set simparity-wire-aws-v1: 48 steps, 40 passed, 8 mismatched (score 83.3%), 7 divergence clusters — all in service lambda-web, none blocked by a stub and none owned by an open issue.

Cause 1 — lambda-web-put-resource-policy-validates-before-existence-check#task#1, PutResourcePolicy (malformed policy, absent function):
  Expected (AWS): HTTP 400, x-amzn-errortype ValidationException, $.message = "Policy has syntax errors." (entirely fixed text).
  Actual (emulator): HTTP 404, x-amzn-errortype ResourceNotFoundException, $.message = "The web function arn:aws:lambda:<region>:<account>:web-function/<name> doesn't exist."
  Replay detail: `status: expected 400, got 404` / `$.message: value_mismatch (expected 'Policy has syntax errors.', got "The web function ... doesn't exist.")` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')`.

Cause 2 — lambda-web-resource-policy-and-tag-lifecycle#task#7, PutResourcePolicy (policy AWS refuses, function present):
  Expected (AWS): HTTP 400, x-amzn-errortype ValidationException, $.message = "Policy has syntax errors."; nothing attached.
  Actual (emulator): HTTP 200 with a success body — no error at all.
  Replay detail: `status: expected 400, got 200` / `$.message: missing (expected 'Policy has syntax errors.', got None)` / `$headers.x-amzn-errortype: missing`.

Cause 3 — lambda-web-put-resource-policy-validates-before-existence-check#task#2, PutResourcePolicy (valid policy, absent function):
  Expected (AWS): HTTP 404, ResourceNotFoundException, $.message = "The function <resourceArn> does not exist." (fixed: "The function " / " does not exist."; variable: the request's web-function ARN).
  Actual (emulator): HTTP 404, ResourceNotFoundException (both correct), $.message = "The web function <resourceArn> doesn't exist."
  Replay detail: `$.message: value_mismatch (expected 'The function arn:aws:lambda:us-west-2:000000000000:web-function/sim2real-probe-lambdaweb-missing-cc341b77 does not exist.', got "The web function ... doesn't exist.")` — that ARN is this run's instance of the variable part.

Cause 4 — extra error members, four steps:
  Expected (AWS): the error body carries `message` only; $.resourceId and $.resourceType are ABSENT.
  Actual (emulator): `$.resourceId: type_mismatch (expected None, got 'arn:aws:lambda:<region>:<account>:web-function/<name>')` and `$.resourceType: type_mismatch (expected None, got 'WebFunction')` on lambda-web-put-resource-policy-validates-before-existence-check#task#2 (404); the same two mismatches with resourceType 'ResourcePolicy' on lambda-web-resource-policy-and-tag-lifecycle#task#10 (PutResourcePolicy 409), #task#11 (DeleteResourcePolicy 409) and #task#13 (GetResourcePolicy 404, cluster also covering lambda-web-resource-policy-and-tags-isolated-across-functions#task#18). On #task#11 and #task#13 these two members are the ONLY divergence: status, code and message already match.
  Model evidence: ResourceNotFoundException / ConflictException declare resourceId and resourceType as OPTIONAL members; the recordings leave both unset.

Cause 5 — lambda-web-resource-policy-and-tag-lifecycle#task#10, PutResourcePolicy with a stale revisionId:
  Expected (AWS): HTTP 409, ConflictException, $.message = "The policy was updated or deleted by another request. Try again with the latest policy revisionId." (all fixed text, no request/state values).
  Actual (emulator): HTTP 409, ConflictException (correct), $.message = "The policy was updated by another request. Try again."
  Replay detail: `$.message: value_mismatch (expected 'The policy was updated or deleted by another request. Try again with the latest policy revisionId.', got 'The policy was updated by another request. Try again.')`.

Cause 6 — lambda-web-resource-policy-and-tag-lifecycle#task#15, TagResource with a reserved aws: key:
  Expected (AWS): HTTP 400, ValidationException, $.message = 'One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.' (entirely fixed text).
  Actual (emulator): HTTP 400, ValidationException (correct), $.message = "1 validation error detected: Value 'aws:Reserved' at 'tags.aws:Reserved' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" — the count and the submitted key (as value and inside the internal member path) are interpolated; 'aws:Reserved' is this run's instance of that variable.
  Model evidence: TagResource http {POST /2025-03-07/tags/{resource}, 204}; ValidationException httpError 400; TagKey @pattern admits ':' so the reserved-prefix rule is service-applied.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore `lambda-web` client at the data listener (region us-west-2, any credentials).
2. Create an execution role (iam) and a versioned code bucket (s3) with code.zip, then CreateWebFunction with revisionConfig{buildConfig{codeConfig.s3Object, runtimeConfig.runtime=nodejs24.x}, serviceConfig.executionRoleArn} and endpointConfig{endpointName=ep, endpointType=HomeRegion, authType=IamAuth}; keep the returned functionArn. Also form an ARN for a web function that was never created (same ARN shape, unused name).

Cause 1: PutResourcePolicy(resourceArn=<never-created ARN>, policy=<a document AWS refuses as syntactically invalid>). The emulator answers 404 ResourceNotFoundException ("The web function ... doesn't exist.") instead of 400 ValidationException "Policy has syntax errors." — the existence check ran first.

Cause 2: PutResourcePolicy(resourceArn=<functionArn>, policy=<the same refused document>). The emulator answers 200 and attaches it (GetResourcePolicy then reports it) instead of 400 ValidationException "Policy has syntax errors." with nothing attached.

Cause 3: PutResourcePolicy(resourceArn=<never-created ARN>, policy=<a valid policy>). Status 404 and code ResourceNotFoundException are right, but the message is "The web function <arn> doesn't exist." instead of "The function <arn> does not exist."

Cause 4: inspect the JSON error body of each refusal below — each carries resourceId and resourceType, which AWS omits:
   a. the Cause 3 call (404, resourceType "WebFunction");
   b. rev1 = PutResourcePolicy(functionArn, policy=p1)["revisionId"]; rev2 = PutResourcePolicy(functionArn, policy=p2, revisionId=rev1)["revisionId"]; then PutResourcePolicy(functionArn, policy=p3, revisionId=rev1) -> 409 with resourceType "ResourcePolicy";
   c. DeleteResourcePolicy(functionArn, RevisionId=rev1) -> 409 with resourceType "ResourcePolicy";
   d. DeleteResourcePolicy(functionArn, RevisionId=<current>) to remove it, then GetResourcePolicy(functionArn) -> 404 with resourceType "ResourcePolicy".

Cause 5: in step 4b the 409 message is "The policy was updated by another request. Try again." instead of "The policy was updated or deleted by another request. Try again with the latest policy revisionId." (DeleteResourcePolicy with a stale revisionId shares the constant).

Cause 6: TagResource(resource=functionArn, tags={"ok":"v1", "aws:Reserved":"x"}). The call is correctly refused whole with 400 ValidationException and no tag applied, but the message is "1 validation error detected: Value 'aws:Reserved' at 'tags.aws:Reserved' failed to satisfy constraint: Member must not begin with the reserved prefix aws:" instead of AWS's 'One or more of your tags are invalid. Please ensure none of your tag keys start with "aws:", none are empty, and there are no duplicate tag keys.' UntagResource(resource=functionArn, tagKeys=["aws:Reserved"]) shows the same wrong wording from the shared constant.

Re-replay the wire corpus to confirm: fixtures lambda-web-put-resource-policy-validates-before-existence-check (task#1, task#2), lambda-web-resource-policy-and-tag-lifecycle (task#7, task#10, task#11, task#13, task#15) and lambda-web-resource-policy-and-tags-isolated-across-functions (task#18).
