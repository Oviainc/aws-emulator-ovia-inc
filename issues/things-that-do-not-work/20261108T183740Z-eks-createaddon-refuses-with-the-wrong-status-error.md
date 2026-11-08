Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-request-shape-mistakes-without-cluster
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-request-shape-mistakes-without-cluster eks-mbes-traj-create-addon-request-mistakes eks-efsfips-traj-addon-name-and-version-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateAddon|4xx->4xx|$.clusterName
Issue-Key: eks|CreateAddon|4xx->4xx|$.message
Issue-Key: eks|CreateAddon|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateAddon|4xx->4xx|status
Issue-Key: eks|CreateAddon|4xx->4xx|$.addonName
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# eks CreateAddon refuses with the wrong status, error type and message for five request-shape mistakes

> _Note: this issue was identified against the code base at commit `99cd58a9732d3abbd4ae376bbbd8ca320e363114`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddon

## Description
Five net-new root causes share one fault on the CreateAddon surface: the handler resolves the cluster from state before (or instead of) validating the request's own shape and the caller's authorization, so a malformed or unauthorized request is refused with the wrong HTTP status, the wrong x-amzn-errortype and an emulator-authored or generic-framework message; and when it does refuse at 400 it populates the error body's clusterName/addonName members with each other's value.

The already-open issue 20261102T083656Z-eks-add-on-request-validation-runs-in-the.md describes the same family of faults, but its replay keys cover only ListAddons (Issue-Key eks|ListAddons|4xx->4xx|...), so none of the five CreateAddon divergences below are gated by it and the replay report reports them unowned. They are filed here so a fixer actually re-replays them.

Cause 1 of 5: an invalid clusterName path label is never checked, and a body member's required-check fires first.
  AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) models CreateAddon.clusterName -> ClusterName as @httpLabel @required with @length 1..100 and @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ on POST /clusters/{clusterName}/addons; modeled errors include InvalidParameterException (400). When the label violates that pattern (e.g. a full cluster ARN passed where the bare name belongs) AWS refuses with HTTP 400 and the message 'The parameter clusterName contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' — every word of that sentence is fixed text except the member name ('clusterName'), which comes from the request member that failed; AWS also echoes the offending value in the body's clusterName member. The label check runs ahead of body-member presence checks.
  Emulator: HTTP 400 but with the generic framework sentence "1 validation error detected: Value null at 'addonName' failed to satisfy constraint: Member must not be null", and no clusterName member in the body. The httpLabel pattern is never enforced.
  Why: a client that passed a cluster ARN where a cluster name belongs is told a different member (addonName) is missing, so it corrects the wrong field and the request keeps failing; it never learns the name format is the blocker.
  Fix: CreateAddon's input validation — enforce the clusterName @pattern/@length on the path label and order it ahead of body-member presence checks — plus the error serializer that echoes clusterName.
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#1.

Cause 2 of 5: a cross-account pass-role refusal is reported as a missing cluster.
  AWS: CreateAddon.serviceAccountRoleArn -> RoleArn (@length 1..255). When the role belongs to another account, AWS refuses the call as an authorization failure before resolving the cluster: HTTP 403, x-amzn-errortype 'AccessDeniedException', message 'Cross-account pass role is not allowed.' — entirely fixed text, no variable part — and no clusterName member in the body (recorded behavior; AccessDeniedException is not in CreateAddon's modeled error list, so the status/code are taken from the recorded response).
  Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', message 'No cluster found for name: <clusterName>.' (fixed text 'No cluster found for name: ' plus the cluster name from the request and a period), plus an extra clusterName member.
  Why: 403 is an IAM problem the caller must fix on the role; 404 tells the caller to create the cluster. The client takes a remediation that cannot succeed.
  Fix: CreateAddon's handler — the pass-role/account authorization check must run ahead of cluster resolution and raise AccessDeniedException (403).
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#2.

Cause 3 of 5: a malformed serviceAccountRoleArn is reported as a missing cluster.
  AWS: a serviceAccountRoleArn that is not a well-formed role ARN is refused with HTTP 400, x-amzn-errortype 'InvalidParameterException' (modeled on CreateAddon, httpError 400), message 'The provided addonServiceAccountRoleArn is invalid.' — fixed text with no variable part — and no clusterName member in the body.
  Emulator: HTTP 404 'ResourceNotFoundException' with 'No cluster found for name: <clusterName>.' and an unmodeled clusterName member; the ARN is never inspected.
  Why: the caller is pointed at the cluster instead of the role ARN it actually got wrong, so the loop of retries never converges.
  Fix: CreateAddon's input validation — ARN shape check on serviceAccountRoleArn, classified InvalidParameterException, ordered ahead of cluster resolution.
  Traces: eks-efsfips-traj-request-shape-mistakes-without-cluster#task#3 (2 affected steps, including eks-mbes-traj-create-addon-request-mistakes#task#3).

Cause 4 of 5: a clusterName that violates the label pattern is looked up in state instead of being rejected.
  AWS: a clusterName containing characters outside ^[0-9A-Za-z][A-Za-z0-9\-_]*$ (e.g. spaces or punctuation) is refused with HTTP 400 and the clusterName-invalid-characters message quoted in Cause 1 — fixed text apart from the member name — not with a not-found.
  Emulator: HTTP 404, x-amzn-errortype 'ResourceNotFoundException', message 'No cluster found for name: <the malformed name>.' — the malformed value is used as a state key rather than rejected.
  Why: 404 invites the client to create a cluster under a name EKS will never accept, so an automated create-then-retry loop cannot terminate.
  Fix: CreateAddon's input validation — enforce the clusterName pattern on the path label before any state lookup.
  Traces: eks-mbes-traj-create-addon-request-mistakes#task#4.

Cause 5 of 5: the CreateAddon refusal body transposes clusterName and addonName.
  AWS: in an add-on refusal body, the clusterName member carries the cluster from the request's path label and addonName carries the add-on from the request body; each is the value of its own request parameter.
  Emulator: the two are swapped — at HTTP 400 it returns the add-on name in clusterName and the cluster name in addonName.
  Why: a client that logs or branches on the error body's clusterName attributes the failure to the wrong resource, and any automated remediation keyed on it targets a resource that does not exist.
  Fix: CreateAddon's error serializer, where those two members are populated.
  Traces: eks-efsfips-traj-addon-name-and-version-mistakes#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 63.4%, 31 divergence clusters). All five clusters below report stub_path empty and owned_by empty.

Cause 1 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#1, CreateAddon, status 400->400: expected $.clusterName 'arn:aws:eks:us-west-2:000000000000:cluster/<cluster>' (got None); expected $.message 'The parameter clusterName contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.', got "1 validation error detected: Value null at 'addonName' failed to satisfy constraint: Member must not be null".

Cause 2 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#2, CreateAddon: expected status 403, x-amzn-errortype 'AccessDeniedException', $.message 'Cross-account pass role is not allowed.', $.clusterName absent; got status 404, x-amzn-errortype 'ResourceNotFoundException', $.message 'No cluster found for name: <cluster>.', $.clusterName populated.

Cause 3 — eks-efsfips-traj-request-shape-mistakes-without-cluster#task#3, CreateAddon: expected status 400, x-amzn-errortype 'InvalidParameterException', $.message 'The provided addonServiceAccountRoleArn is invalid.'; got status 404, 'ResourceNotFoundException', 'No cluster found for name: <cluster>.' — 2 affected steps (also eks-mbes-traj-create-addon-request-mistakes#task#3).

Cause 4 — eks-mbes-traj-create-addon-request-mistakes#task#4, CreateAddon: expected status 400 with the clusterName-invalid-characters message; got status 404, 'ResourceNotFoundException', 'No cluster found for name: sim2real probe bad!.'

Cause 5 — eks-efsfips-traj-addon-name-and-version-mistakes#task#8, CreateAddon, status 400->400: $.clusterName value_mismatch (expected the cluster-position value, got the add-on name) and $.addonName value_mismatch (expected the add-on-position value, got the cluster name) — i.e. the two members are returned transposed.

Smithy consulted: CreateAddon.clusterName (ClusterName, httpLabel, required, length 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$), CreateAddon.serviceAccountRoleArn (RoleArn, length 1..255), CreateAddon errors (ClientException 400, InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404, ServerException 500), http POST /clusters/{clusterName}/addons.

## Steps to Reproduce
1. git clone the project repo; the emulator is built from that checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-efsfips-traj-request-shape-mistakes-without-cluster, step task#1. The other four causes are at the fixture#step pairs listed per cause (and in Issue-Trace-Ids).
4. Manually: POST /clusters/arn%3Aaws%3Aeks%3Aus-west-2%3A000000000000%3Acluster%2F<name>/addons with an empty body — the emulator reports addonName null instead of the clusterName pattern violation. POST /clusters/<absent-cluster>/addons with a serviceAccountRoleArn in another account — expect 403 AccessDeniedException 'Cross-account pass role is not allowed.', the emulator answers 404. Same call with a malformed role ARN — expect 400 InvalidParameterException 'The provided addonServiceAccountRoleArn is invalid.'. POST /clusters/'sim2real probe bad!'/addons — expect 400 invalid-characters, the emulator answers 404. Any CreateAddon 400 refusal body — clusterName and addonName come back swapped.
