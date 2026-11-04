Issue-Source: trace-sweep-divergence
Issue-Trace: eks-ekscc-traj-create-request-shape-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-ekscc-traj-create-request-shape-mistakes eks-ekscc-traj-vpc-config-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCluster|4xx->4xx|$.message
Issue-Related: 20261102T083656Z-eks-cluster-scope-request-validation-missing-checks-wrong.md
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# EKS CreateCluster refuses with generic framework wording instead of the EKS refusal

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Three net-new root causes share one fault: CreateCluster does not implement EKS's own request-refusal contract. Where EKS refuses with its own sentence (and, for an authorization failure, its own status and error class), the emulator emits the generic Smithy-constraint report produced by its validation framework, or a raw JSON-deserializer diagnostic dressed up as a modeled fault. The emulator's error body also omits the clusterName member EKS returns alongside these refusals.

The already-open issue 20261102T083656Z-eks-cluster-scope-request-validation-missing-checks-wrong.md describes the same family of faults across the cluster-scope operations, but the replay reports these three clusters as unowned: that issue's replay keys cover the clusterName/x-amzn-errortype/status facets of CreateCluster, not the $.message facets diverging here, and it names no cause at all for the missing-required-member refusal (Cause 1 below). These are filed separately so each is replayed and closed on its own evidence.

Cause 1 of 3: a missing required `name` is refused with the framework's null-constraint sentence instead of EKS's.
  AWS: CreateCluster.name is @required (Smithy: CreateCluster.name -> ClusterName, required, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$). A request that omits it is refused with HTTP 400 and the message 'The parameter name is required.' — fixed text 'The parameter ' ... ' is required.' with the omitted member's name ('name') as the only variable part. CreateCluster's modeled client errors are ClientException, InvalidParameterException, ResourceInUseException, ResourceLimitExceededException and UnsupportedAvailabilityZoneException; there is no ValidationException.
  Emulator: HTTP 400 with "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null" — a generic framework report, not an AWS message.
  Why: a client (or an operator reading the console/CLI output) that matches or surfaces EKS's documented sentence sees an unrecognised string, so a retry loop keyed on EKS's required-parameter wording never fires and the caller cannot tell which EKS contract it violated.
  Fix: CreateCluster's input validation — emit the EKS-worded required-member refusal rather than delegating to the generic Smithy-constraint reporter.
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#4 (cluster also covers #task#5, #task#8, #task#9 and eks-ekscc-traj-vpc-config-mistakes#task#3, #task#4 — 6 steps).

Cause 2 of 3: an invalid cluster name is refused with the framework's pattern sentence, and the error body omits clusterName.
  AWS: a `name` that violates the modeled pattern (Smithy: ClusterName @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) is refused with HTTP 400 and the message 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' — entirely fixed text apart from the member name 'name'. The error body additionally carries a clusterName member echoing the rejected name from the request.
  Emulator: HTTP 400 with "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\\-_]*$" and no clusterName member at all.
  Why: the emulator's sentence is a different error contract from the one EKS publishes — it exposes an escaped regex rather than EKS's explanation of the naming rule, and automation that reads clusterName out of the error body to report which cluster name was rejected finds nothing there.
  Fix: CreateCluster's validation must emit the EKS-worded pattern refusal, and its error serializer must populate clusterName from the request.
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#6 (cluster also covers #task#7 and eks-ekscc-traj-vpc-config-mistakes#task#5 — 3 steps).

Cause 3 of 3: authorization is evaluated after body deserialization, so an unauthorized call returns a parser diagnostic as a ValidationException.
  AWS: for this request EKS refuses with HTTP 403 and 'User: <principalArn> is not authorized to perform: eks:CreateCluster' — fixed text 'User: ' ... ' is not authorized to perform: eks:CreateCluster', with the caller's principal ARN as the only variable part. The refusal is produced before the request body is bound to the input shapes, so a body-shape problem never masks it (recorded behavior: the recorded response is 403 with that message; the model does not state the ordering).
  Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and message 'expected array start, got string' — an internal JSON-deserializer diagnostic that is not an AWS message, names no member, and whose error class is not modeled on CreateCluster at all (Smithy: CreateCluster errors listed above contain no ValidationException).
  Why: the client is told its request is malformed in deserializer terms when AWS would tell it the principal lacks eks:CreateCluster. It retries or 'fixes' the body forever instead of fixing the IAM policy, and a 400 is treated as a non-retryable client bug rather than an access-control failure, so the authorization gap never surfaces.
  Fix: CreateCluster's handler — evaluate authorization before deserializing the body, and the request deserializer must never surface a raw parser diagnostic as a modeled fault.
  Traces: eks-ekscc-traj-create-request-shape-mistakes#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 57.5%, 41 divergence clusters; these three were reported unowned and unblocked).

Cause 1 — fixture eks-ekscc-traj-create-request-shape-mistakes, step task#4, CreateCluster, 400->400, divergence_key service=eks|CreateCluster|status=400->400|keys=$.message, 6 affected steps:
  expected $.message 'The parameter name is required.'
  actual   $.message "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null"
  Fixed text: 'The parameter ' / ' is required.'; variable part: the omitted member name, here 'name'.

Cause 2 — fixture eks-ekscc-traj-create-request-shape-mistakes, step task#6, CreateCluster, 400->400, divergence_key service=eks|CreateCluster|status=400->400|keys=$.clusterName,$.message, 3 affected steps:
  expected $.message 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' and $.clusterName echoing the rejected name from the request (recorded run: 'sim2real probe ekscc e3d7c8ef')
  actual   $.message "1 validation error detected: Value 'sim2real probe ekscc e3d7c8ef' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\\-_]*$", $.clusterName absent (None)
  The whole expected sentence is fixed text except the member name 'name'; the only request-derived value is the clusterName member.

Cause 3 — fixture eks-ekscc-traj-create-request-shape-mistakes, step task#10, CreateCluster, 403->400, divergence_key service=eks|CreateCluster|status=403->400|keys=$.message,$headers.x-amzn-errortype,status:
  expected status 403 and $.message 'User: <principalArn> is not authorized to perform: eks:CreateCluster' (recorded run: principal arn:aws:iam::000000000000:user/emulator)
  actual   status 400, x-amzn-errortype 'ValidationException', $.message 'expected array start, got string'
  Fixed text: 'User: ' / ' is not authorized to perform: eks:CreateCluster'; variable part: the caller's principal ARN. The account id and user name in the recorded ARN are environment values, not requirements.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateCluster http POST /clusters (success 200); input member name -> ClusterName, required, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$; errors ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, UnsupportedAvailabilityZoneException 400, ServerException 500, ServiceUnavailableException 503 (no ValidationException).

## Steps to Reproduce
1. git clone the project repository and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-ekscc-traj-create-request-shape-mistakes, step task#4. The other two causes are at eks-ekscc-traj-create-request-shape-mistakes#task#6 and eks-ekscc-traj-create-request-shape-mistakes#task#10; re-replay all three (plus the clustered steps eks-ekscc-traj-create-request-shape-mistakes#task#5, #task#7, #task#8, #task#9 and eks-ekscc-traj-vpc-config-mistakes#task#3, #task#4, #task#5) to confirm a fix.
4. Manually, against the emulator's EKS endpoint:
   a. POST /clusters with roleArn and resourcesVpcConfig set but `name` omitted -> emulator answers 400 with the generic "1 validation error detected: Value null at 'name' ..." instead of 400 'The parameter name is required.'
   b. POST /clusters with name containing spaces (e.g. 'my probe cluster') -> emulator answers 400 with the escaped-regex framework sentence and no clusterName member, instead of EKS's 'The parameter name contains invalid characters. ...' with clusterName echoing the rejected name.
   c. POST /clusters as a principal with no eks:CreateCluster permission and a body whose resourcesVpcConfig list member is a string where a list is expected -> emulator answers 400 ValidationException 'expected array start, got string' instead of 403 'User: <principalArn> is not authorized to perform: eks:CreateCluster'.
