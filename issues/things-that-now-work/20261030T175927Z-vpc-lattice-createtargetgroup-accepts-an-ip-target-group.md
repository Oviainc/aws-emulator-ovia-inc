Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-target-group-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-target-group-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->2xx|$.fieldList
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->2xx|$.message
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->2xx|$.reason
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->2xx|status
Issue-Commit: aa6eac2cd77e70e1001710733620f31a870d8297

# vpc-lattice CreateTargetGroup accepts an IP target group missing required config fields

> _Note: this issue was identified against the code base at commit `aa6eac2cd77e70e1001710733620f31a870d8297`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
ONE CAUSE.

Cause 1 of 1 — CreateTargetGroup with type IP and an incomplete config, fixture vpclattice-target-group-lifecycle step task#4 (expected HTTP 400, actual HTTP 201).
AWS: CreateTargetGroup (Smithy: POST /targetgroups, success 201; input `name` required, `type` -> TargetGroupType one of IP|LAMBDA|INSTANCE|ALB required, `config` -> TargetGroupConfig optional) refuses a target group of type IP whose config does not supply port, protocol and vpcIdentifier. TargetGroupConfig documents these as type-dependent (`port`: "The port on which the targets are listening... Not supported if the target group type is LAMBDA"; `protocol`: "The protocol to use for routing traffic to the targets... Not supported if the target group type is LAMBDA"; `vpcIdentifier`: "The ID of the VPC. Not supported if the target group type is LAMBDA"), but the model carries no required/presence trait tying them to the IP type, so the refusal itself is RECORDED BEHAVIOR. The recording shows HTTP 400 ValidationException (Smithy: ValidationException{message required, reason -> ValidationExceptionReason required, fieldList -> ValidationExceptionFieldList}, errorKind client, httpError 400) with the x-amzn-errortype header naming it, reason "fieldValidationFailed", message "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group", and fieldList carrying one ValidationExceptionField whose `name` is the offending member path "config" and whose `message` repeats that sentence. In that message the whole sentence is fixed text for this error class; its variable parts are the pipe-joined list of missing config member names and the target-group type token ("IP") taken from the request.
Emulator: answers HTTP 201 and CREATES the target group — the 400, the error code, `reason`, `message`, `fieldList` and the x-amzn-errortype header are all absent (replay: "status: expected 400, got 201"; "$.reason: missing (expected 'fieldValidationFailed')"; "$.message: missing"; "$.fieldList: missing"; "$headers.x-amzn-errortype: missing"). The handler validates the name, tag count, the per-type SUPPORT of config members and health-check ranges, but never checks that the members the IP type REQUIRES are present — and when `config` is absent altogether the whole config check is skipped, so the record is persisted with an empty/defaulted config.
Why: a client's negative path is inverted. Code that relies on AWS refusing an incomplete IP target group proceeds as though the group exists, then fails later against a group that has no port, protocol or VPC; the invalid record also occupies the account-unique target-group name and is handed to callers by GetTargetGroup/ListTargetGroups as if it were valid. A conformance or negative test asserting the 400 fails outright.
Fix: CreateTargetGroup's input validation — extend the per-type config check (`crate::state::check_target_group_config`) with the presence rule for each target-group type, run it even when `config` is absent, and report the refusal through the existing service-rule/validation construction so the response carries reason "fieldValidationFailed", the message above and a fieldList entry named for the offending member path ("config").
Traces: vpclattice-target-group-lifecycle#task#4; code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_target_group.rs:247-259 (the config check runs only `if let Some(config) = input.config.as_ref()` and only checks support, not presence) and lines 264-265/348-363 (the record is normalised and persisted regardless).

No open issue covers this: the open vpc-lattice issues are the id-alphabet minting issue (20261030T043524Z-vpc-lattice-minted-resource-ids-use-a-base36.md), the GetRule stale-state issue (20261029T192412Z-...) and the BatchUpdateRule/PutAuthPolicy/PutResourcePolicy/TagResource stub issues; none touches CreateTargetGroup's validation. The cluster is reported with stub_path empty and owned_by empty.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; 110 steps, 53 passed, 57 mismatched, 0 errored, 0 blocked; score 48.2%; 22 divergence clusters). One net-new vpc-lattice mismatch cluster (divergence_key service=vpc-lattice|CreateTargetGroup|status=400->201|keys=$.fieldList,$.message,$.reason,$headers.x-amzn-errortype,status), fixture vpclattice-target-group-lifecycle step task#4, affected_count 1, stub_path empty, owned_by empty.

Detail, verbatim:
  status: expected 400, got 201
  $.fieldList: missing (expected [{'message': 'Config fields port|protocol|vpcIdentifier must not be null for IP Target Group', 'name': 'config'}], got None)
  $.message: missing (expected 'Config fields port|protocol|vpcIdentifier must not be null for IP Target Group', got None)
  $.reason: missing (expected 'fieldValidationFailed', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

Expected (AWS, from the recording): HTTP 400 ValidationException, x-amzn-errortype naming ValidationException, reason "fieldValidationFailed", message "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group", fieldList = [{name: "config", message: <the same sentence>}].
Actual (emulator): HTTP 201 with a created target group; none of those members and no error header.

Model facts used: CreateTargetGroup http {POST /targetgroups, code 201}, input {name required, type -> TargetGroupType(IP|LAMBDA|INSTANCE|ALB) required, config -> TargetGroupConfig}, errors include ValidationException (httpError 400); ValidationException{message required, reason required, fieldList}; ValidationExceptionField{name required, message required}; TargetGroupConfig.port/protocol/vpcIdentifier documented as "Not supported if the target group type is LAMBDA" with no modeled presence requirement for the IP type — so the presence rule and the exact sentence are recorded behavior, while the 400 and the ValidationException member set are modeled.
Code: src/ops/create_target_group.rs:247-259 validates config only when present and only for per-type SUPPORT (crate::state::check_target_group_config) plus health-check ranges; there is no presence check for the members the IP type requires, and lines 264-265 and 348-372 normalise and persist the record unconditionally.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture vpclattice-target-group-lifecycle (replay_wire_corpus); the diverging step is task#4.
2. Equivalent by hand: POST /targetgroups with {"name":"<a fresh valid name>","type":"IP"} — i.e. type IP with no config, or a config that omits any of port/protocol/vpcIdentifier.
3. Observe HTTP 201 and a created target group. AWS answers HTTP 400 ValidationException with the x-amzn-errortype header, reason "fieldValidationFailed", message "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group" and fieldList [{"name":"config","message":<the same sentence>}].
4. GET /targetgroups/{id} afterwards and note the emulator persisted a target group AWS would never have created.
5. Confirm the fix by re-replaying vpclattice-target-group-lifecycle#task#4.
