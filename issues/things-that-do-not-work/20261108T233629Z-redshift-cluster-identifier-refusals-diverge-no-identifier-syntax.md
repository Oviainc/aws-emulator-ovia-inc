Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-rshealth-describe-identifier-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-rshealth-describe-identifier-mistakes redshift-cluster-lifecycle-op-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeLoggingStatus|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|DescribeLoggingStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeLoggingStatus|4xx->4xx|status
Issue-Key: redshift|FailoverPrimaryCompute|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# Redshift cluster-identifier refusals diverge: no identifier-syntax validation and emulator-authored ClusterNotFound wording

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoggingStatus

## Description
This bundle carries TWO root causes in how Redshift operations refuse a ClusterIdentifier they cannot serve: the syntactic validation that must run BEFORE any lookup is missing, and the not-found message the lookup then emits is the emulator's own sentence. Both must be closed before this issue is submitted.

Cause 1 of 2: a syntactically invalid ClusterIdentifier is answered with a not-found fault instead of AWS's InvalidParameterValue validation refusal (fixture redshift-rshealth-describe-identifier-mistakes, step task#6, operation DescribeLoggingStatus).
AWS: a Redshift cluster identifier must begin with a letter, contain only ASCII letters, digits and hyphens, not end with a hyphen and not contain two consecutive hyphens; it is also lower-cased. An identifier violating that grammar is refused by REQUEST VALIDATION, before any cluster lookup happens, so the answer is the service-common InvalidParameterValue refusal at HTTP 400, not the operation's ClusterNotFoundFault. Smithy: DescribeLoggingStatus input ClusterIdentifier (required, target String) carries no pattern trait and the operation models only ClusterNotFoundFault (404/ClusterNotFound) and UnsupportedOperationFault (400), so the grammar and its refusal are recorded behavior only: the recorded response is HTTP 400 with Error.Code `InvalidParameterValue` and Error.Message "The parameter ClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." That message is ENTIRELY fixed text except the member name "ClusterIdentifier", which is the offending input member from the request; nothing in it comes from the request's value.
Emulator: the request is routed straight to a cluster lookup and refused as a missing cluster - HTTP 404, Error.Code `ClusterNotFound`, Error.Message "Cluster <the raw identifier> not found." (replay: "status: expected 400, got 404"; Code "expected 'InvalidParameterValue', got 'ClusterNotFound'"; Message "expected 'The parameter ClusterIdentifier is not a valid identifier. ...', got 'Cluster Sim2real_Probe_RSHealth_b76fbc not found.'"). The identifier in that recording contains uppercase letters and underscores, i.e. it can never name a cluster.
Why: the status, the error code and the taxonomy are all wrong, so a client cannot tell a malformed request from a missing resource. SDK retry/handling logic keyed on ClusterNotFound treats a permanent client-side input bug as a resource-existence condition (and 404 vs 400 flows differently through gateways and client error classifiers); validation tests that assert InvalidParameterValue fail against the emulator. Conversely, code that creates-on-not-found will attempt a create with an identifier AWS also refuses.
Fix: the shared ClusterIdentifier request validation for the cluster-scoped operations - it must run in the handler's input-validation stage, ahead of the state lookup, and raise the common InvalidParameterValue refusal with AWS's fixed sentence for the offending member; the lookup path is reached only for a syntactically valid identifier. Open issue 20261103T040731Z-redshift-describe-path-request-validation-answers-the-wrong.md describes the same family of describe-path validation faults but carries no replay key for this step, so this step is not covered by it. (A separate stub issue, 20261108T121521Z-redshift-describeloggingstatus-stub.md, covers DescribeLoggingStatus's unimplemented success path - a different defect: here the operation does answer, with the wrong refusal.)

Cause 2 of 2: the ClusterNotFound message is the emulator's own wording (fixture redshift-cluster-lifecycle-op-preconditions, step task#10, operation FailoverPrimaryCompute).
AWS: naming a cluster that does not exist is refused with the operation's modeled ClusterNotFoundFault - Smithy: FailoverPrimaryCompute errors include ClusterNotFoundFault (errorKind client, httpError 404, awsQueryError code `ClusterNotFound`), documented "The ClusterIdentifier parameter does not refer to an existing cluster." The message text is recorded behavior only: the recorded response is HTTP 404, Error.Code ClusterNotFound, Error.Message "Cluster not found: <ClusterIdentifier>" - FIXED text "Cluster not found: " followed by the identifier echoed from the request (the recording's `sim2real-probe-cluster-missing` is a request value, not part of the template), with NO trailing period.
Emulator: the status (404) and code match, but the message is inverted into the emulator's own sentence "Cluster <ClusterIdentifier> not found." - different fixed text, different word order, and a trailing period AWS does not emit.
Why: a message that differs from AWS's wording breaks every consumer that reads refusal text rather than the code - operator-facing error surfaces, log assertions, and wrappers that parse the sentence to recover the cluster name from the prefix. The same wrong template is what Cause 1's lookup emits, so fixing it once corrects both the misclassified and the genuine not-found paths.
Fix: the shared cluster-lookup not-found mapping that produces ClusterNotFoundFault for the cluster-scoped handlers (FailoverPrimaryCompute included) - the message must be rendered as AWS's fixed "Cluster not found: " prefix plus the requested identifier, with no trailing period.

Traces: redshift-rshealth-describe-identifier-mistakes#task#6 (Cause 1, representative), redshift-cluster-lifecycle-op-preconditions#task#10 (Cause 2).

## Evidence
Cause 1 - DescribeLoggingStatus with a ClusterIdentifier that violates the Redshift identifier grammar (fixture redshift-rshealth-describe-identifier-mistakes, step task#6; the recording uses `Sim2real_Probe_RSHealth_b76fbc`: uppercase letters and underscores):
  Expected (recorded AWS): HTTP 400; Error.Code = InvalidParameterValue; Error.Message = "The parameter ClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." (all fixed text; the only variable part is the member name ClusterIdentifier).
  Actual (emulator): HTTP 404; Error.Code = ClusterNotFound; Error.Message = "Cluster Sim2real_Probe_RSHealth_b76fbc not found." Replay keys: status, $.ErrorResponse.Error.Code, $.ErrorResponse.Error.Message.

Cause 2 - FailoverPrimaryCompute naming a cluster that does not exist (fixture redshift-cluster-lifecycle-op-preconditions, step task#10):
  Expected (recorded AWS): HTTP 404, Error.Code ClusterNotFound, Error.Message "Cluster not found: <ClusterIdentifier>" (recording: "Cluster not found: sim2real-probe-cluster-missing"), no trailing period.
  Actual (emulator): HTTP 404, Error.Code ClusterNotFound, Error.Message "Cluster sim2real-probe-cluster-missing not found." Only $.ErrorResponse.Error.Message diverges.

Model basis: Smithy DescribeLoggingStatus input ClusterIdentifier required (String, no pattern), errors {ClusterNotFoundFault -> 404/ClusterNotFound, UnsupportedOperationFault -> 400/UnsupportedOperation}; FailoverPrimaryCompute input ClusterIdentifier required, errors {ClusterNotFoundFault -> 404/ClusterNotFound, InvalidClusterStateFault, UnauthorizedOperation, UnsupportedOperationFault}; ClusterNotFoundFault documented "The ClusterIdentifier parameter does not refer to an existing cluster."

## Steps to Reproduce
1. Boot the emulator and point a redshift client at its data listener.
2. Cause 1: call DescribeLoggingStatus with a ClusterIdentifier that cannot be a legal Redshift identifier - e.g. one containing underscores and uppercase letters, or one ending in a hyphen, or one with two consecutive hyphens. Observe HTTP 404 with Error.Code ClusterNotFound and Message "Cluster <value> not found."; AWS answers HTTP 400 with Error.Code InvalidParameterValue and the fixed identifier-grammar sentence naming the member ClusterIdentifier.
3. Cause 2: call FailoverPrimaryCompute with a syntactically VALID ClusterIdentifier that names no existing cluster (e.g. sim2real-probe-cluster-missing). Observe HTTP 404 ClusterNotFound with Message "Cluster <id> not found."; AWS answers "Cluster not found: <id>" with no trailing period.
4. Replay the bundled fixture steps: redshift-rshealth-describe-identifier-mistakes#task#6 and redshift-cluster-lifecycle-op-preconditions#task#10.
