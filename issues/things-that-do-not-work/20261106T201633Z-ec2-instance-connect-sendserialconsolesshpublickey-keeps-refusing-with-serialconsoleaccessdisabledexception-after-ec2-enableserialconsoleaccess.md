Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2-instance-connect.send-serial-console-ssh-public-key-access-disabled-rejected
Issue-Shape-Id: com.amazonaws.ec2instanceconnect#AWSEC2InstanceConnectService
Issue-Key: ec2-instance-connect.send-serial-console-ssh-public-key-access-disabled-rejected|run#17|AssertionError|assert code_after == "EC2InstanceNotFoundException", (
Issue-Commit: 09448fb928879ffb83c121d01adc49e65396319c

# SendSerialConsoleSSHPublicKey keeps refusing with SerialConsoleAccessDisabledException after EC2 EnableSerialConsoleAccess (account toggle not shared with ec2-instance-connect)

> _Note: this issue was identified against the code base at commit `09448fb928879ffb83c121d01adc49e65396319c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSerialConsoleSSHPublicKey

## Description
Cause 1 of 1: the account-level EC2 serial-console toggle is held twice — once in the EC2 service's state (written by EnableSerialConsoleAccess / DisableSerialConsoleAccess and read by GetSerialConsoleAccessStatus) and once, independently, in ec2-instance-connect's own state — and nothing propagates the EC2 write into the ec2-instance-connect copy. SendSerialConsoleSSHPublicKey therefore reads a flag that is always the service's default (disabled) and refuses every call for the lifetime of the process, even after the account has been granted access.

AWS: EC2 serial console access is ONE account-level setting. EnableSerialConsoleAccess grants it for the account, GetSerialConsoleAccessStatus reports it, and ec2-instance-connect's SendSerialConsoleSSHPublicKey gates on that same setting. Smithy (com.amazonaws.ec2instanceconnect#SendSerialConsoleSSHPublicKey): modeled errors include SerialConsoleAccessDisabledException (@httpError 403, awsQueryError code SerialConsoleAccessDisabled) — documented "Your account is not authorized to use the EC2 Serial Console. To authorize your account, run the EnableSerialConsoleAccess API." — and EC2InstanceNotFoundException (@httpError 404, code EC2InstanceNotFound) — "The specified instance was not found." The account gate is evaluated BEFORE the instance lookup, so while access is disabled an unknown instance id yields SerialConsoleAccessDisabledException, and once access is ENABLED the identical request must fall through to the instance lookup and yield EC2InstanceNotFoundException with the message "Instance not found." (fixed text, no variable parts). The disabled message's fixed text is "The account (" + <the caller's account id, taken from the request's credentials/emulator account> + ") is not authorized to use the EC2 Serial Console. To authorize the account, run the EnableSerialConsoleAccess API." — the account id is the only variable part.

Emulator: with access disabled, the call is correctly refused with SerialConsoleAccessDisabledException and the exact message above (that beat passes). The check then calls ec2.enable_serial_console_access() and polls ec2.get_serial_console_access_status() until it reports SerialConsoleAccessEnabled=true — i.e. the EC2 side DID flip. Replaying the SAME SendSerialConsoleSSHPublicKey request for the SAME well-formed-but-unknown instance id still returns error code 'SerialConsoleAccessDisabledException' instead of 'EC2InstanceNotFoundException' / "Instance not found." (replay: "expected EC2InstanceNotFoundException once access is enabled (same bogus instance id), got 'SerialConsoleAccessDisabledException'"). The gate never clears, so NO SendSerialConsoleSSHPublicKey call can ever succeed or reach any later decision beat (not-found, instance state, instance type, or the Success:true push).

Why: a client that follows AWS's documented remedy — run EnableSerialConsoleAccess, then push a serial-console key — is permanently stuck on a 403 SerialConsoleAccessDisabledException telling it to run the API it already ran successfully. The account-level grant is unobservable from ec2-instance-connect, so the whole serial-console data path (and every error class behind the gate) is unreachable, and tooling that branches on 403-vs-404 mis-diagnoses an unknown instance as an unauthorized account.

Fix: the account-level serial-console setting must be a single shared fact rather than a per-crate duplicate. Either ec2-instance-connect's SendSerialConsoleSSHPublicKey gate should read the EC2 service's account setting (the store EnableSerialConsoleAccess/DisableSerialConsoleAccess write and GetSerialConsoleAccessStatus reads), or the EC2 toggle handlers must publish the change into ec2-instance-connect's state so the two can never disagree; the gate itself belongs in the SendSerialConsoleSSHPublicKey handler's decision order (step 1), which is otherwise correct. Citations: services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/src/ops/send_serial_console_ssh_public_key.rs:231 (reads the local flag), .../src/state.rs:406 and :480 (the private flag and its default, whose own doc comment says "That API belongs to EC2, not to this service, so until the EC2 emulator implements it this field is where it would write" — EC2 now implements it), and services/service-com-amazonaws-ec2-amazonec2/src/ops/enable_serial_console_access.rs:155 (writes it into the EC2 store only).

Traces: ec2-instance-connect.send-serial-console-ssh-public-key-access-disabled-rejected.

## Evidence
Check ec2-instance-connect.send-serial-console-ssh-public-key-access-disabled-rejected [mismatch].

Sequence:
1. ec2.disable_serial_console_access(); poll get_serial_console_access_status() until SerialConsoleAccessEnabled=false.
2. eic.send_serial_console_ssh_public_key(InstanceId=<unknown well-formed i-...>, SerialPort=0, SSHPublicKey=<wire-valid ssh-rsa key>)
   Expected (AWS): SerialConsoleAccessDisabledException, message "The account (<account-id>) is not authorized to use the EC2 Serial Console. To authorize the account, run the EnableSerialConsoleAccess API."
   Actual (emulator): same — this beat PASSES.
3. ec2.enable_serial_console_access(); poll get_serial_console_access_status() until SerialConsoleAccessEnabled=true — reached, so EC2 reports the grant.
4. Identical send_serial_console_ssh_public_key call for the identical unknown instance id:
   Expected (AWS): HTTP 404, Error.Code 'EC2InstanceNotFoundException', Error.Message 'Instance not found.'
   Actual (emulator): Error.Code 'SerialConsoleAccessDisabledException' (403), still the disabled message.
   Assertion: "expected EC2InstanceNotFoundException once access is enabled (same bogus instance id), got 'SerialConsoleAccessDisabledException'"

The two beats together show the gate is readable but write-blind: ec2-instance-connect's serial_console_access_enabled starts false and is never set by EC2's EnableSerialConsoleAccess, which writes only the EC2 crate's own state (enable_serial_console_access.rs:155 -> set_serial_console_access_enabled(true)).

## Steps to Reproduce
1. Boot the emulator with both ec2 and ec2-instance-connect mounted.
2. ec2.enable_serial_console_access()
3. ec2.get_serial_console_access_status() -> SerialConsoleAccessEnabled is true.
4. ec2-instance-connect.send_serial_console_ssh_public_key(InstanceId="i-0a1b2c3d4e5f60718" (a seeded running Nitro instance) or any well-formed unknown id, SerialPort=0, SSHPublicKey=<80..4096-char ssh-rsa key>).
5. Observe SerialConsoleAccessDisabledException (403) instead of Success:true for the known instance / EC2InstanceNotFoundException "Instance not found." (404) for the unknown one. Repeat for DisableSerialConsoleAccess followed by the same call: the answer is identical, confirming the EIC gate ignores the account setting entirely.
