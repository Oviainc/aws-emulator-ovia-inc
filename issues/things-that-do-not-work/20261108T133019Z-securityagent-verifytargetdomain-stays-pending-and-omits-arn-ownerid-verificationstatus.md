Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-target-domain-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-target-domain-lifecycle
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|VerifyTargetDomain|2xx->2xx|$.arn
Issue-Key: securityagent|VerifyTargetDomain|2xx->2xx|$.ownerId
Issue-Key: securityagent|VerifyTargetDomain|2xx->2xx|$.status
Issue-Key: securityagent|VerifyTargetDomain|2xx->2xx|$.verificationStatus
Issue-Key: securityagent|VerifyTargetDomain|2xx->2xx|$.verificationStatusReason
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# VerifyTargetDomain stays PENDING and omits arn, ownerId, verificationStatus

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
VerifyTargetDomain

## Description
One net-new root cause with two inseparable halves in a single response.

Cause 1 of 1 (fixture securityagent-target-domain-lifecycle, step task#5, VerifyTargetDomain): a verification attempt on a target domain that cannot be verified never reaches a terminal outcome, and the response omits the target domain's identity and verification-status fields altogether.

AWS: Smithy: VerifyTargetDomain (POST /VerifyTargetDomain, 200) takes targetDomainId (@required) and is documented as "Initiates verification of a target domain. This checks whether the domain ownership verification token has been properly configured." Its status member targets the TargetDomainStatus enum [PENDING, VERIFIED, FAILED, UNREACHABLE], so FAILED is a modeled terminal outcome of the check, and verificationStatusReason is documented as "The reason for the current target domain verification status". Recorded behavior only for the rest: the recorded response for a re-verify of a domain whose published proof cannot be read carries status FAILED, a verificationStatus field also FAILED, an arn of the form arn:aws:securityagent:<region>:<accountId>:target-domain/<targetDomainId>, an ownerId holding the owning account id, and a verificationStatusReason whose fixed text is 'Verification failed for ' + <the verification URL built from the domain name the request's record holds> + ': connection timed out while attempting to query endpoint'. arn, ownerId and verificationStatus are NOT members of the modeled VerifyTargetDomainOutput (which models only targetDomainId, domainName, createdAt, updatedAt, verifiedAt, status, verificationStatusReason), so the real service returns them beyond the model and the recording is the authority for them. The region, account id, domain name and generated target domain id in those values come from the environment and the request's stored record, not from the recording.

Emulator: HTTP 200, but status is still 'PENDING' rather than the terminal 'FAILED'; verificationStatus, arn and ownerId are absent entirely (all reported `missing (... got None)`); and verificationStatusReason is the emulator's standing pending sentence 'Target domain is pending ownership verification. Add the DNS TXT record to the DNS resolver associated with your domain before attempting verification' instead of the failure reason for the attempt that was just made. src/ops/verify_target_domain.rs builds the response from exactly the seven modeled members (lines 229-243) and takes status from record.attempt_verification(now), so a check that never settles to a failure keeps answering PENDING forever and the three extra wire fields can never be emitted.

Why: ownership verification is a state machine the client polls. A client that calls VerifyTargetDomain and re-reads the record is waiting for a terminal PENDING -> VERIFIED/FAILED/UNREACHABLE transition; against the emulator it polls PENDING indefinitely, never surfaces a failure to the operator, and never learns why (the reason it is handed describes the setup step, not the attempt's outcome). Separately, a client that keys its own records off the target domain's arn or checks ownerId to confirm the account that owns the domain gets nothing at all, and a client reading verificationStatus (the field the recorded API reports the outcome in) sees it missing and cannot distinguish "not yet verified" from "verification failed".

Fix: the VerifyTargetDomain handler and the verification state machine it drives — TargetDomainRecord::attempt_verification/settle_verification in crate::state must settle an unreachable/unverifiable attempt to a terminal status and set the attempt's reason, and the handler's output projection (plus the operation's output serializer, since the three fields are not modeled members) must emit arn, ownerId and verificationStatus alongside the modeled members, deriving arn and ownerId from the service's region/account context rather than hard-coding them.

Related open issue: 20261030T135909Z-securityagent-updateagentspace-responses-omit-agent-instance-identity-fields-and reports the same class of omission (arn/ownerId missing from a response) for UpdateAgentSpace; it does not cover this operation or this fixture step (the replay reports owned_by empty here).

Traces: securityagent-target-domain-lifecycle#task#5.

## Evidence
Fixture securityagent-target-domain-lifecycle, step task#5, operation VerifyTargetDomain, verdict mismatch, expected status 200 / actual status 200:
  $.status: value_mismatch — expected 'FAILED', got 'PENDING'
  $.verificationStatus: missing — expected 'FAILED', got None
  $.arn: missing — expected 'arn:aws:securityagent:us-west-2:000000000000:target-domain/<targetDomainId>', got None
  $.ownerId: missing — expected the owning account id, got None
  $.verificationStatusReason: value_mismatch — expected 'Verification failed for https://<domainName>/.well-known/aws/securityagent-domain-verification.json: connection timed out while attempting to query endpoint', got 'Target domain is pending ownership verification. Add the DNS TXT record to the DNS resolver associated with your domain before attempting verification'

(The region/account in the arn, the generated target domain id and the probe domain name are environment/request values filling the templates, not requirements. The requirements are: a terminal status value from TargetDomainStatus, the presence of verificationStatus/arn/ownerId, and the failure reason's fixed text 'Verification failed for ' ... ': connection timed out while attempting to query endpoint' around the verification URL derived from the stored domain name.)

Smithy: VerifyTargetDomain POST /VerifyTargetDomain, success 200, no modeled errors; output members targetDomainId, domainName, createdAt, updatedAt, verifiedAt, status (TargetDomainStatus enum PENDING|VERIFIED|FAILED|UNREACHABLE), verificationStatusReason. arn/ownerId/verificationStatus are unmodeled — recorded behavior only.

Citations: services/service-com-amazonaws-securityagent-securityagent/src/ops/verify_target_domain.rs:220-243 (status taken from record.attempt_verification(now); output built from the seven modeled members only); the module's own note at lines 70-81 documents that a check "does not settle inside the call that starts it"; services/service-com-amazonaws-securityagent-securityagent/src/state.rs (TargetDomainRecord::attempt_verification / settle_verification and settle_target_domain_verifications).

## Steps to Reproduce
1. Boot the emulator (fresh state).
2. sa = boto3.client("securityagent", endpoint_url=<emulator>)
3. d = sa.create_target_domain(domainName='<probe>.example.com', ...) -> targetDomainId (HTTP-route verification).
4. sa.verify_target_domain(targetDomainId=d) — the attempt that cannot read any published proof.
5. sa.verify_target_domain(targetDomainId=d) again (the fixture's task#5).
6. Observe HTTP 200 with status == 'PENDING', no verificationStatus / arn / ownerId members, and the standing pending reason. Real AWS reports a terminal status ('FAILED'), the matching verificationStatus, the target-domain arn and ownerId, and a verificationStatusReason describing the failed attempt.
7. Replay fixture securityagent-target-domain-lifecycle step task#5 to confirm.
