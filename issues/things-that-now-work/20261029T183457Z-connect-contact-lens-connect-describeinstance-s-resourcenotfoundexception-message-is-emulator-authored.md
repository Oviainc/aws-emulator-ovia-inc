Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connect-contact-lens.deleted-instance-lookup-rejected
Issue-Shape-Id: com.amazonaws.connectcontactlens#AmazonConnectContactLens
Issue-Key: connect-contact-lens.deleted-instance-lookup-rejected|run#17|AssertionError|assert describe_error["Message"] == f"Instance {instance_id} not found", (
Issue-Commit: b2028dd6393016a9896c0e35ff905bdd884f0327

# Connect DescribeInstance's ResourceNotFoundException message is emulator-authored ("Instance not found: <id>") instead of AWS's "Instance <id> not found"

> _Note: this issue was identified against the code base at commit `b2028dd6393016a9896c0e35ff905bdd884f0327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstance

## Description
One cause (Cause 1 of 1).

AWS: Smithy: Connect's DescribeInstance models ResourceNotFoundException with httpError 404, documented "The specified resource was not found." The model does not fix the message string, so this is recorded behavior only: for an InstanceId that names no instance (including one that existed, was ACTIVE, and has since been deleted — deletion permanently retires the identifier), the service answers 404 ResourceNotFoundException with the message
  Instance <InstanceId> not found
Its fixed text is the word "Instance " followed by the identifier followed by " not found"; the only variable part is the InstanceId, taken verbatim from the request. No other member is populated.
Emulator: services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_instance.rs builds the refusal as format!("Instance not found: {}", input.instance_id), so the client receives message "Instance not found: <InstanceId>" — the same two pieces of information in the wrong word order, with a colon AWS does not emit. The code (ResourceNotFoundException) and status (404) are correct; only the message text diverges.
Why: the error message is part of the refusal contract. Log scrapers, runbooks, support tooling and tests that match on AWS's sentence ("Instance <id> not found") fail to match, and a message in a shape AWS never produces makes emulator-captured diagnostics non-comparable with production ones. It also makes this handler inconsistent with the service's other not-found texts.
Fix: the DescribeInstance handler's not-found construction in services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_instance.rs — emit the message as the identifier embedded in AWS's sentence rather than appended after a colon; prefer a single shared constructor for this service's instance-not-found refusal so no caller can grow another spelling.
Traces: connect-contact-lens.deleted-instance-lookup-rejected (the check's id carries the connect-contact-lens service token; the defective handler is the Connect service's DescribeInstance, reached as that check's deletion control).

## Evidence
Failing check (verdict [mismatch]): connect-contact-lens.deleted-instance-lookup-rejected
  check_connect_contact_lens_deleted_instance_lookup_rejected.py line 82:
    assert describe_error["Message"] == f"Instance {instance_id} not found"
  AssertionError: unexpected DescribeInstance message 'Instance not found: 4cd1aa0d-5ae8-497f-901e-e5a6892bcee8'

Expected (AWS): Code "ResourceNotFoundException", HTTP 404, Message "Instance <InstanceId> not found".
Actual (emulator): Code "ResourceNotFoundException", HTTP 404, Message "Instance not found: <InstanceId>".
(The uuid above is one run's value and is not part of the requirement; the requirement is the sentence template with the request's InstanceId substituted.)

Source: services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_instance.rs lines 159-163 — format!("Instance not found: {}", input.instance_id).
Smithy model: services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json, DescribeInstance errors InternalServiceException(500), InvalidRequestException(400), ResourceNotFoundException(404).

Note: the rest of this check's contract is satisfied — the ResourceNotFoundException code for the deleted instance and the contact-lens refusal preceding it matched; only the DescribeInstance message text diverges.

## Steps to Reproduce
1. connect.create_instance(IdentityManagementType="CONNECT_MANAGED", InstanceAlias=<unique>, InboundCallsEnabled=True, OutboundCallsEnabled=True); poll connect.describe_instance(InstanceId=<id>) until InstanceStatus == "ACTIVE" (confirms the instance really exists).
2. connect.delete_instance(InstanceId=<id>).
3. connect.describe_instance(InstanceId=<id>) and read the ClientError: Error.Code is ResourceNotFoundException and HTTP status 404 (both correct), but Error.Message is "Instance not found: <id>" where AWS returns "Instance <id> not found".
