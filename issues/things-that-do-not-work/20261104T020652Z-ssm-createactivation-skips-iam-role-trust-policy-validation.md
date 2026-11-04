Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-activation-and-managed-instance-registration
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-activation-and-managed-instance-registration
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateActivation|4xx->2xx|$.__type
Issue-Key: ssm|CreateActivation|4xx->2xx|$.message
Issue-Key: ssm|CreateActivation|4xx->2xx|status
Issue-Commit: 490c1cbe5b0cc74feaea217ae97fc7bca73b7940

# SSM CreateActivation skips IAM role trust-policy validation and returns 200 for a nonexistent role

> _Note: this issue was identified against the code base at commit `490c1cbe5b0cc74feaea217ae97fc7bca73b7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateActivation

## Description
Cause 1 of 1: CreateActivation does not validate the IamRole it is handed, so a request naming a role that does not exist (or one whose trust policy does not permit the ssm service principal) is accepted and an activation is created.

AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm): CreateActivation.IamRole is @required, target IamRole, @length min 0 max 64; the operation models InvalidParameters (400) and InternalServerError (500). The model does not state the trust-policy rule, so for that rule this is recorded behavior only: the recording shows AWS rejecting the call with HTTP 400 and a JSON body whose __type is 'ValidationException' and whose message is "Nonexistent role or missing ssm service principal in trust policy: <role-arn>". In that message the fixed text is "Nonexistent role or missing ssm service principal in trust policy: "; the only variable part is the role ARN, which AWS renders from the caller's account id and the IamRole value supplied in the request (arn:aws:iam::<account-id>:role/<IamRole>). The recorded run's particular account id and role name are just the template filled in for that run and are not themselves requirements.

Emulator: answers HTTP 200 with a successful CreateActivationResult (ActivationId/ActivationCode). No __type and no message are emitted at all: the replay reports 'status: expected 400, got 200', '$.__type: missing (expected ValidationException, got None)' and '$.message: missing'. The role referenced in the request is never checked against the emulator's IAM state or against the ssm service principal requirement.

Why: a client that relies on AWS's behavior treats a 200 from CreateActivation as proof that the managed-instance registration role is usable. Against the emulator it receives an activation id and activation code bound to a role that does not exist, so the misconfiguration surfaces later (or never), and negative-path code that expects to catch ValidationException for a bad or untrusted role never runs. Tooling that probes role validity through CreateActivation gets the opposite answer from the emulator than from AWS.

Fix: the CreateActivation handler's input validation — after the modeled IamRole length/required checks, resolve the role against the emulator's IAM role state and verify its trust policy admits the ssm service principal, and on failure return HTTP 400 with __type 'ValidationException' and the message above, with the role ARN composed from the request's IamRole and the account id. No other open issue covers this operation: the 51 other SSM divergences in this replay are already owned by existing open issues (parameter-store error classes, document payloads, ValidationException text rendering, DescribeInstanceInformation/Properties filter validation, public parameter hierarchy), none of which touches activations.

Traces: ssm-activation-and-managed-instance-registration#task#2 (CreateActivation, expected 400 ValidationException, got 200).

## Evidence
Fixture ssm-activation-and-managed-instance-registration, step task#2, operation CreateActivation.
Expected (AWS recording): HTTP 400, body __type = 'ValidationException', message = 'Nonexistent role or missing ssm service principal in trust policy: arn:aws:iam::<account-id>:role/<IamRole>' (recorded instance: arn:aws:iam::000000000000:role/sim2real-probe-ssm-ssm-absent-role-76b72919).
Actual (emulator): HTTP 200, successful CreateActivationResult; $.__type missing, $.message missing.
Replay detail verbatim: "status: expected 400, got 200 / $.__type: missing (expected 'ValidationException', got None) / $.message: missing".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture ssm-activation-and-managed-instance-registration, step task#2: call ssm CreateActivation with an IamRole naming a role that does not exist in the emulator's IAM state (and therefore has no trust policy granting ssm.amazonaws.com).
3. Observe HTTP 200 with an ActivationId/ActivationCode instead of HTTP 400 with __type 'ValidationException' and message 'Nonexistent role or missing ssm service principal in trust policy: <role ARN built from the request's IamRole>'.
