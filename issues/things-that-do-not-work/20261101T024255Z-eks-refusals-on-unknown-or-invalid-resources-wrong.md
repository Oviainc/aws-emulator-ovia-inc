Issue-Source: trace-sweep-divergence
Issue-Trace: eks-capability-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-capability-error-paths eks-eks-anywhere-subscription-errors eks-eks-anywhere-subscription-not-found-errors eks-insights-and-identity-provider-operations-on-missing-cluster
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCapability|4xx->4xx|$.clusterName
Issue-Key: eks|CreateCapability|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCapability|4xx->4xx|status
Issue-Key: eks|DescribeEksAnywhereSubscription|4xx->4xx|$.message
Issue-Key: eks|DescribeEksAnywhereSubscription|4xx->4xx|$.subscriptionId
Issue-Key: eks|DisassociateIdentityProviderConfig|4xx->4xx|$.clusterName
Issue-Key: eks|DisassociateIdentityProviderConfig|4xx->4xx|$.message
Issue-Related: 20261029T185333Z-eks-refusal-messages-diverge-from-aws-wording-across.md
Issue-Commit: 1b9f432602d5f7155b187927ba6fddf5156de930

# eks refusals on unknown or invalid resources: wrong refusal selected and wrong exception identity members (3 causes)

> _Note: this issue was identified against the code base at commit `1b9f432602d5f7155b187927ba6fddf5156de930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapability

## Description
THREE net-new root causes in the eks emulator's refusal payloads for requests that name a resource the account/Region does not carry, or a parameter value the API does not define. In every one the emulator picks a refusal AWS does not pick for that request, and/or populates members of the exception shape that AWS leaves absent. All three must be closed before this issue is resubmitted.

None of them is covered by an open issue. The nearest neighbours are '20261029T185333Z-eks-refusal-messages-diverge-from-aws-wording-across.md' (which owns the *message-wording-only* divergences on CreateCapability, DescribeAddonConfiguration, the three tag operations, RegisterCluster and CreateCluster — its CreateCapability cause is keyed 'eks|CreateCapability|4xx->4xx|$.message', i.e. a 400-to-400 message difference, whereas cause 1 below is now a 400-to-404 status-and-code difference on the same step) and '20261031T171831Z-eks-2-net-new-root-causes-in-the--r2.md' (a BEHAVIORAL issue that owns the DescribeEksAnywhereSubscription message wording from the behavioral corpus but carries no wire replay key and does not mention the spurious subscriptionId member of cause 2).

=== Cause 1 of 3: CreateCapability resolves clusterName BEFORE judging the `type` enum, so an undefined capability type against a cluster that does not exist is answered 404 ResourceNotFoundException instead of 400 InvalidParameterException / 'Invalid capability type' ===
AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): CreateCapability = POST /clusters/{clusterName}/capabilities; `clusterName` is an @httpLabel required String; `type` is required and targets CapabilityType, an enum whose values are exactly [ACK, KRO, ARGOCD]. The operation's modeled error union is AccessDeniedException 403 / InvalidParameterException 400 / InvalidRequestException 400 / ResourceInUseException 409 / ResourceLimitExceededException 400 / ServerException 500 / ThrottlingException 429 — ResourceNotFoundException is NOT a modeled error of CreateCapability at all. The recorded behavior for a request that carries BOTH an undefined `type` and a clusterName no cluster holds is HTTP 400 with Error.Code = InvalidParameterException and Error.Message = 'Invalid capability type': the capability type is judged first, so the undefined enum value is reported and the missing parent cluster is never reached. That message is fixed text in its entirety — it has no variable part and does not echo the rejected value. The recorded payload also carries NO `clusterName` member (InvalidParameterException$clusterName is optional and AWS leaves it absent here).
Emulator: answers HTTP 404 with x-amzn-errortype = ResourceNotFoundException, Error.Message = 'No cluster found for name: <clusterName>.' and a populated `clusterName` member. The handler resolves the @httpLabel cluster before any body member is judged (the module's documented 'ORDER' rule and the `extra_errors = ["ResourceNotFoundException", ...]` widening), so the enum violation is shadowed by the parent lookup — and the error shape it answers with is one the operation does not model.
Why: a client gets the wrong error class for the wrong reason. Its SDK dispatches on CreateCapability's modeled error union, finds no ResourceNotFoundException there, and falls through to a generic/unknown-error path instead of its InvalidParameterException branch; the status moves 400 -> 404, which retry and classification logic treats differently; and the caller is told to create a cluster when the real defect is a capability type its own request made up. Fixing only the message (the pre-existing wording issue) cannot fix this: the wrong exception is being raised.
Fix: CreateCapability's input validation in the eks service handler — the `type` membership check against [ACK, KRO, ARGOCD] must run BEFORE the clusterName lookup and raise InvalidParameterException (400) with the fixed sentence 'Invalid capability type', and this operation's InvalidParameterException payload must not set the `clusterName` member for that refusal.
Traces: eks-capability-error-paths#task#4.

=== Cause 2 of 3: DescribeEksAnywhereSubscription's 404 carries the service-wide generic message and echoes the requested id into ResourceNotFoundException.subscriptionId, which AWS leaves absent ===
AWS: Smithy: DescribeEksAnywhereSubscription = GET /eks-anywhere-subscriptions/{id}, input {id @httpLabel required}, errors ClientException 400 / ResourceNotFoundException 404 / ServerException 500 / ServiceUnavailableException 503. ResourceNotFoundException's members (clusterName, nodegroupName, fargateProfileName, addonName, subscriptionId, message) are all optional. Recorded behavior only: for an id the Region carries no subscription for, AWS answers HTTP 404, Error.Code = ResourceNotFoundException, Error.Message = 'subscription not found.' — fixed text in its entirety, with no variable part — and the payload carries NO `subscriptionId` member.
Emulator: answers 404 ResourceNotFoundException but with Error.Message = 'The requested resource does not exist.' (the crate-wide state::RESOURCE_NOT_FOUND_MESSAGE constant) and sets `subscriptionId` to the id taken verbatim from the request path.
Why: a client that branches on, logs or asserts the refusal text for a missing EKS Anywhere subscription sees a sentence AWS never emits for this class, and a client that reads `subscriptionId` out of the payload to decide whether the service identified a concrete subscription sees a member populated where AWS reports none — so 'the service named this subscription' and 'the service named nothing' become indistinguishable.
Fix: DescribeEksAnywhereSubscription's handler in the eks service — emit the subscription-specific fixed message instead of reusing the crate-wide not-found constant, and stop populating ResourceNotFoundException$subscriptionId on this path.
Traces: eks-eks-anywhere-subscription-errors#task#1 (same cluster also reached from eks-eks-anywhere-subscription-not-found-errors#task#1).

=== Cause 3 of 3: DisassociateIdentityProviderConfig's unknown-cluster 404 substitutes the cluster-specific sentence and populates ResourceNotFoundException.clusterName, where AWS gives the generic sentence and no identity member ===
AWS: Smithy: DisassociateIdentityProviderConfig = POST /clusters/{clusterName}/identity-provider-configs/disassociate, input {clusterName @httpLabel required, identityProviderConfig required, clientRequestToken @idempotencyToken}; models ResourceNotFoundException with httpError 404, documented as 'The specified resource could not be found. You can view your available clusters with ListClusters...'. Recorded behavior: for a clusterName the Region holds no cluster for, AWS answers HTTP 404, Error.Code = ResourceNotFoundException, Error.Message = 'The requested resource does not exist.' — fixed text with no variable part — and the payload carries NO `clusterName` member; this operation does not name the cluster it failed to find.
Emulator: answers 404 ResourceNotFoundException with Error.Message = 'No cluster found for name: <clusterName>.' (the shared state::cluster_not_found helper, which interpolates the requested name) and a populated `clusterName` member.
Why: the emulator applies one service-wide cluster-not-found shape to an operation that AWS answers with the generic refusal, so a caller asserting or parsing this operation's refusal text mismatches, and it is handed a `clusterName` echo AWS does not disclose on this path. The inversion also runs the opposite way from cause 2 — the same two sentences are swapped between the two operations — so the underlying defect is that the refusal sentence and identity members are chosen crate-wide rather than per operation.
Fix: DisassociateIdentityProviderConfig's unknown-cluster path in the eks service handler (its local `not_found` closure, which calls the shared cluster-not-found helper) — raise ResourceNotFoundException with the generic fixed sentence and no `clusterName` member for this operation.
Traces: eks-insights-and-identity-provider-operations-on-missing-cluster#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 40 fixtures, 232 steps, 125 passed, 107 mismatched, score 53.9%, emulator_up = true (/__admin/health answered 200), bootstrap_signal = false. Each cause below is one divergence cluster with stub_path empty and owned_by empty.

Cause 1 — CreateCapability, fixture eks-capability-error-paths step task#4 (divergence_key eks|CreateCapability|status=400->404|keys=$.clusterName,$.message,$headers.x-amzn-errortype,status):
  Expected: HTTP 400, x-amzn-errortype = InvalidParameterException, $.message = 'Invalid capability type' (fixed text, no variable part), $.clusterName absent.
  Actual:   HTTP 404, x-amzn-errortype = ResourceNotFoundException, $.message = 'No cluster found for name: <clusterName from the request path>.', $.clusterName = the requested cluster name.
  Smithy: CreateCapability models InvalidParameterException (400) and does NOT model ResourceNotFoundException; `type` targets enum CapabilityType with values [ACK, KRO, ARGOCD].
  Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_capability.rs:34-39 (the documented 'ORDER' rule that resolves clusterName before any body member), :159-162 (extra_errors widening that admits ResourceNotFoundException), :349-354 (the 404 returned before the `type` value is ever judged), :294-302 (the InvalidParameterException closure that always sets cluster_name).

Cause 2 — DescribeEksAnywhereSubscription, fixture eks-eks-anywhere-subscription-errors step task#1, 1 further affected step (eks-eks-anywhere-subscription-not-found-errors#task#1) (divergence_key eks|DescribeEksAnywhereSubscription|status=404->404|keys=$.message,$.subscriptionId):
  Expected: HTTP 404, Error.Code = ResourceNotFoundException, $.message = 'subscription not found.' (fixed text), $.subscriptionId absent.
  Actual:   HTTP 404, Error.Code = ResourceNotFoundException, $.message = 'The requested resource does not exist.', $.subscriptionId = the id taken from the request path.
  Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_eks_anywhere_subscription.rs:157-166 — ResourceNotFoundException { subscription_id: Some(input.id.clone()), message: Some(state::RESOURCE_NOT_FOUND_MESSAGE.to_string()), .. }.

Cause 3 — DisassociateIdentityProviderConfig, fixture eks-insights-and-identity-provider-operations-on-missing-cluster step task#8 (divergence_key eks|DisassociateIdentityProviderConfig|status=404->404|keys=$.clusterName,$.message):
  Expected: HTTP 404, Error.Code = ResourceNotFoundException, $.message = 'The requested resource does not exist.' (fixed text), $.clusterName absent.
  Actual:   HTTP 404, Error.Code = ResourceNotFoundException, $.message = 'No cluster found for name: <clusterName from the request path>.', $.clusterName = the requested cluster name.
  Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/disassociate_identity_provider_config.rs:187-193 (the `not_found` closure calling state::cluster_not_found) and :226-228 (the unknown-cluster return).

The statuses and codes above are taken from the recorded responses; the generated cluster/subscription names and ids in the recordings are per-run values and are not part of the requirement — only the fixed text of each message, the absence of the identity members, and the status/code are.

## Steps to Reproduce
1. Clone this checkout, boot the emulator, and replay trace set simparity-wire-aws-v1 (emulator_up must be true; bootstrap_signal false).
2. Cause 1 — fixture eks-capability-error-paths, step task#4: POST /clusters/<a cluster name no cluster exists for>/capabilities with `type` set to a value outside [ACK, KRO, ARGOCD] (plus the required capabilityName, roleArn, deletePropagationPolicy). Observe HTTP 404 / x-amzn-errortype ResourceNotFoundException / message 'No cluster found for name: <name>.' with a clusterName member; AWS answers HTTP 400 / InvalidParameterException / 'Invalid capability type' with no clusterName member.
3. Cause 2 — fixture eks-eks-anywhere-subscription-errors, step task#1: GET /eks-anywhere-subscriptions/<an id no subscription carries> (boto3: eks.describe_eks_anywhere_subscription(id=...)). Observe Error.Message 'The requested resource does not exist.' and a populated subscriptionId; AWS answers 'subscription not found.' with subscriptionId absent.
4. Cause 3 — fixture eks-insights-and-identity-provider-operations-on-missing-cluster, step task#8: POST /clusters/<a cluster name no cluster exists for>/identity-provider-configs/disassociate with identityProviderConfig = {type: 'oidc', name: <any>}. Observe Error.Message 'No cluster found for name: <name>.' and a populated clusterName; AWS answers 'The requested resource does not exist.' with clusterName absent.
5. Replay to confirm the fix — all three bundled causes: eks-capability-error-paths#task#4, eks-eks-anywhere-subscription-errors#task#1 and eks-insights-and-identity-provider-operations-on-missing-cluster#task#8.
