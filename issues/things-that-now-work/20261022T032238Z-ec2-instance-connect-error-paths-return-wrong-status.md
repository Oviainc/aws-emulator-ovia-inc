Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-instance-connect-serial-console-error-access-disabled
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-instance-connect-serial-console-error-access-disabled ec2-instance-connect-ssh-key-error-invalid-args ec2-instance-connect-ssh-key-error-not-found
Issue-Shape-Id: com.amazonaws.ec2instanceconnect#AWSEC2InstanceConnectService
Issue-Key: ec2-instance-connect|SendSerialConsoleSSHPublicKey|4xx->4xx|$.Message
Issue-Key: ec2-instance-connect|SendSerialConsoleSSHPublicKey|4xx->4xx|$.__type
Issue-Key: ec2-instance-connect|SendSerialConsoleSSHPublicKey|4xx->4xx|status
Issue-Key: ec2-instance-connect|SendSSHPublicKey|4xx->4xx|$.Message
Issue-Key: ec2-instance-connect|SendSSHPublicKey|4xx->4xx|$.__type
Issue-Key: ec2-instance-connect|SendSSHPublicKey|4xx->4xx|status
Issue-Commit: 665638329bed0451c93a41d0e56b2a57ab18cb60

# EC2 Instance Connect error paths return wrong status and error type

> _Note: this issue was identified against the code base at commit `665638329bed0451c93a41d0e56b2a57ab18cb60`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSSHPublicKey

## Description
All three recorded EC2 Instance Connect error fixtures diverge in the same way: every failing request is answered from a single instance-existence check that is evaluated before anything else and is serialized with HTTP 404, so the emulator never produces the modeled access-control or validation errors and never produces the recorded status. 3 causes.

Cause 1 of 3 - SendSerialConsoleSSHPublicKey, serial-console access disabled (fixture ec2-instance-connect-serial-console-error-access-disabled, step task#1).
AWS: SendSerialConsoleSSHPublicKey models SerialConsoleAccessDisabledException (Smithy: com.amazonaws.ec2instanceconnect#SerialConsoleAccessDisabledException, bound to SendSerialConsoleSSHPublicKey, doc "Your account is not authorized to use the EC2 Serial Console. To authorize your account, run the EnableSerialConsoleAccess API"; protocol awsJson1_1). The recorded response for an account with serial-console access disabled is HTTP 400 with __type SerialConsoleAccessDisabledException (note the model carries @httpError 403 on that shape, but the recording - which is the wire truth for this fixture - shows 400, so 400 is the status to serve). Message example: "The account (000000000000) is not authorized to use the EC2 Serial Console. To authorize the account, run the EnableSerialConsoleAccess API."
Emulator: answers HTTP 404 with __type EC2InstanceNotFoundException and Message "Instance i-0fedcba987654321f not found".
Why: a client that enables serial console on demand branches on SerialConsoleAccessDisabledException to call EnableSerialConsoleAccess; it instead sees a not-found error and concludes the instance does not exist, so the recovery path is never taken and the error is misattributed to the wrong resource.
Fix: SendSerialConsoleSSHPublicKey's handler - the serial-console authorization check must be modelled and evaluated, and its error mapped to SerialConsoleAccessDisabledException at the recorded status, instead of falling through to the instance-existence check.

Cause 2 of 3 - SendSSHPublicKey, invalid InstanceOSUser (fixture ec2-instance-connect-ssh-key-error-invalid-args, step task#1).
AWS: Smithy: SendSSHPublicKey.InstanceOSUser is @required with @length min 1 max 32 and @pattern "^(^[A-Za-z_][A-Za-z0-9\\@\\._-]{0,30}[A-Za-z0-9\\$_-]?$)|(^(?=.{2,32}$)[0-9]{1,32}[A-Za-z\\@\\._-][A-Za-z0-9\\@\\._-]*[A-Za-z0-9\\$_-]?$)$"; the modeled error for a constraint violation is InvalidArgsException ("One of the parameters is not valid.", @httpError 400). The recorded response is HTTP 400 with __type InvalidArgsException, and the message is built from the request - it names the failing member and the constraint it violated (example: "1 validation error detected: Value 'not a valid user!!' at 'instanceOSUser' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."), so that construction rule, not the literal text, is the requirement.
Emulator: answers HTTP 404 with __type EC2InstanceNotFoundException and Message "Instance i-0123456789abcdef0 not found" - the member pattern is never enforced.
Why: input validation is skipped entirely, so a client sending a malformed OS user gets a resource-level 404 instead of a 400 client-validation error; retry/backoff logic treats the two classes differently and the caller gets no indication which member was wrong.
Fix: SendSSHPublicKey's input validation - enforce the modeled @length/@pattern constraints on InstanceId, InstanceOSUser and SSHPublicKey and raise InvalidArgsException (HTTP 400) BEFORE any instance lookup.

Cause 3 of 3 - SendSSHPublicKey, instance not found (fixture ec2-instance-connect-ssh-key-error-not-found, step task#1).
AWS: the recorded response for an unknown instance id is HTTP 400 with __type EC2InstanceNotFoundException (Smithy: com.amazonaws.ec2instanceconnect#EC2InstanceNotFoundException, "The specified instance was not found."; the shape carries @httpError 404 but the recording shows the service answering 400, and the recording is the wire truth here). Recorded message "Instance not found." is an example only - no documented rule produces it - so the message difference alone is not the defect.
Emulator: selects the right error type but serializes it with HTTP 404.
Why: clients and SDK retry classifiers keyed on the HTTP status see a 404 where AWS returns a 400, so status-based error handling (and any proxy or gateway in between that treats 404 specially) behaves differently against the emulator than against AWS.
Fix: the error serializer / status mapping for this service - EC2InstanceNotFoundException must be emitted at the status the recordings show (400), not the raw @httpError value.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 3 steps, 0 passed, score 0.0%; bootstrap_signal false, state_reset_failures 0):

1) ec2-instance-connect-serial-console-error-access-disabled#task#1, SendSerialConsoleSSHPublicKey:
   status: expected 400, got 404
   $.__type: expected 'SerialConsoleAccessDisabledException', got 'EC2InstanceNotFoundException'
   $.Message: expected 'The account (000000000000) is not authorized to use the EC2 Serial Console. To authorize the account, run the EnableSerialConsoleAccess API.', got 'Instance i-0fedcba987654321f not found'

2) ec2-instance-connect-ssh-key-error-invalid-args#task#1, SendSSHPublicKey:
   status: expected 400, got 404
   $.__type: expected 'InvalidArgsException', got 'EC2InstanceNotFoundException'
   $.Message: expected "1 validation error detected: Value 'not a valid user!!' at 'instanceOSUser' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[A-Za-z_][A-Za-z0-9\\@\\._-]{0,30}[A-Za-z0-9\\$_-]?$)|(^(?=.{2,32}$)[0-9]{1,32}[A-Za-z\\@\\._-][A-Za-z0-9\\@\\._-]*[A-Za-z0-9\\$_-]?$)", got 'Instance i-0123456789abcdef0 not found'

3) ec2-instance-connect-ssh-key-error-not-found#task#1, SendSSHPublicKey:
   status: expected 400, got 404
   $.Message: expected 'Instance not found.', got 'Instance i-0123456789abcdef0 not found' (error type matched)

Model consulted: services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/smithy-model/com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice.json - sdkId "EC2 Instance Connect", protocol awsJson1_1, 2 operations; common errors include EC2InstanceNotFoundException, InvalidArgsException, AuthException; SendSerialConsoleSSHPublicKey additionally binds SerialConsoleAccessDisabledException.

No open issue covers these fixtures (list_existing_issues for 'ec2-instance-connect' / 'ec2instanceconnect' returned nothing); no cluster carried stub_path or owned_by.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) - all 3 ec2-instance-connect steps mismatch.
3. Cause 1: replay fixture ec2-instance-connect-serial-console-error-access-disabled step task#1 - POST awsJson1_1 SendSerialConsoleSSHPublicKey for an account whose serial-console access is disabled; expect HTTP 400 __type SerialConsoleAccessDisabledException, observe HTTP 404 __type EC2InstanceNotFoundException.
4. Cause 2: replay fixture ec2-instance-connect-ssh-key-error-invalid-args step task#1 - SendSSHPublicKey with InstanceOSUser = "not a valid user!!" (violates the member's @pattern); expect HTTP 400 __type InvalidArgsException naming instanceOSUser and the pattern, observe HTTP 404 __type EC2InstanceNotFoundException.
5. Cause 3: replay fixture ec2-instance-connect-ssh-key-error-not-found step task#1 - SendSSHPublicKey with an unknown instance id i-0123456789abcdef0; expect HTTP 400 __type EC2InstanceNotFoundException, observe HTTP 404.
6. All three must replay green before closing: the two fixtures listed in trace_ids are part of this issue, not separate findings.
