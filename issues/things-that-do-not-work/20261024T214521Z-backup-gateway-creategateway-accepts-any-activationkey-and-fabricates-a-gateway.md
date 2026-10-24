Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway.create-gateway-rejects-unissued-activation-key
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway.create-gateway-rejects-unissued-activation-key|_client_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# CreateGateway accepts any ActivationKey and fabricates a Gateway: no activation-key redemption or pattern validation

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Cause 1 of 1: backup-gateway CreateGateway performs no validation of the ActivationKey member at all. It treats every supplied string as a redeemable activation and unconditionally creates and returns a Gateway, so the operation has no client-error path whatsoever.

AWS: Smithy model (com-amazonaws-backupgateway-backuponpremises-v20210101) CreateGateway: input ActivationKey is @required with @length(min:1,max:50) and @pattern ^[0-9a-zA-Z\-]+$; GatewayDisplayName @required @length(min:1,max:100) @pattern ^[a-zA-Z0-9-]*$; GatewayType @required. The operation's error union is exactly {InternalServerException (500, server), ThrottlingException (429, client), ValidationException (400, client)} -- there is no ResourceNotFoundException, ConflictException or AccessDeniedException bound to it. An ActivationKey is only issued by a real on-premises Backup gateway appliance completing an activation handshake with AWS, so a key that no appliance ever issued has no activation record to redeem and the only client-error shape available to report that is ValidationException (HTTP 400). Likewise, a key that violates the modeled @pattern can only be reported as ValidationException (400); AWS builds that message from the request in its standard constraint-violation form naming the member and the violated constraint, e.g. "1 validation error detected: Value 'not a valid key!!' at 'activationKey' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z\-]+" -- that construction rule is the requirement, not the literal text. For the unissued-but-pattern-valid case the recorded error message is "The specified activation key was not found." (example text; the requirement is the 400/ValidationException pair). In both cases the rejected call is atomic: no Gateway is created, and the display name never appears in ListGateways.

Emulator: CreateGateway returns HTTP 200 for a pattern-valid string that was never issued, with a fabricated gateway ARN (shape 'arn:aws:backup-gateway:<region>:<account>:gateway/bgw-<id>'), instead of 400 ValidationException. The replay shows the check's _client_error helper raising because the call SUCCEEDED and returned a GatewayArn. Because no ActivationKey validation exists on this path, the modeled @pattern is not enforced either, so a syntactically invalid key such as "not a valid key!!" is also accepted rather than rejected as 400 ValidationException. Every such accepted call also leaks state: the fabricated gateway is persisted and becomes visible in ListGateways, so the rejection is not atomic because there is no rejection.

Why: a client that relies on AWS refusing unredeemable activation keys gets silent false success. Code that treats CreateGateway as the confirmation that a gateway appliance really completed its activation handshake will conclude a gateway exists and is activated when nothing was ever activated, then proceed to hypervisor import, metadata sync and backup-job wiring against a phantom gateway. Negative-path tests that assert CreateGateway raises ValidationException for a bad or stale key pass against AWS and fail here, and SDK retry/error-handling branches for ValidationException are unreachable. Input-validation regressions in callers go undetected because the modeled ActivationKey pattern is never enforced. The fabricated gateways also pollute ListGateways, so inventory reconciliation sees resources that do not exist in AWS.

Fix: belongs in the CreateGateway handler and its request validation, plus the gateway state store. Add ActivationKey validation ahead of any state mutation: enforce the modeled @length(1,50) and @pattern ^[0-9a-zA-Z\-]+$ and return 400 ValidationException with the standard constraint-violation message built from the member name and violated constraint; then require the key to match an activation record the emulator actually issued, returning 400 ValidationException when no such record exists. Ensure the handler returns before persisting, so a rejected CreateGateway creates no gateway and the display name never becomes visible to ListGateways. Keep the responses inside the modeled error union -- do not introduce ResourceNotFoundException or ConflictException, which are not bound to this operation.

Traces: backup-gateway.create-gateway-rejects-unissued-activation-key (mismatch; group native-protocol, axis:rejection, concern:negative-classes, plane:control).

## Evidence
Check: backup-gateway.create-gateway-rejects-unissued-activation-key -- verdict: mismatch.

Expected (AWS, per Smithy model + recorded behavior):
  CreateGateway(ActivationKey=<pattern-valid, never issued>, GatewayDisplayName=<name>, GatewayType="BACKUP_VM")
    -> HTTP 400, Error.Code = "ValidationException"
       Error.Message (example) = "The specified activation key was not found."
    -> and ListGateways must NOT list <name>.
  CreateGateway(ActivationKey="not a valid key!!", ...)
    -> HTTP 400, Error.Code = "ValidationException"
       Message built from the request, e.g. "1 validation error detected: Value 'not a valid key!!' at 'activationKey' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z\-]+"
    -> and ListGateways must NOT list that display name.

Actual (emulator): the first call SUCCEEDED.
  HTTPStatusCode: 200
  content-type: application/x-amz-json-1.0
  body: {"GatewayArn": "arn:aws:backup-gateway:<region>:<account>:gateway/bgw-<id>"}
  (recorded instance: 'arn:aws:backup-gateway:us-west-2:000000000000:gateway/bgw-0000000000000000'; region/account/id are environment-specific and are not the requirement)

Harness output:
  File "check_backup_gateway_create_gateway_rejects_unissued_activation_key.py", line 31, in run
    got = _client_error(lambda: bg.create_gateway(ActivationKey=fake_key, GatewayDisplayName=display_name, GatewayType="BACKUP_VM"))
  File "check_backup_gateway_create_gateway_rejects_unissued_activation_key.py", line 74, in _client_error
    raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'GatewayArn': '...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Divergence: expected status 400 with code ValidationException, got status 200 with a created resource. The check aborted at the first assertion, so the pattern-invalid key assertion and both ListGateways no-leak assertions were never reached -- they share this one root cause (no ActivationKey validation on the CreateGateway path) and must all pass after the fix.

Model evidence (authoritative, from smithy_query):
  CreateGateway.input.ActivationKey: required=true, length={min:1,max:50}, pattern="^[0-9a-zA-Z\\-]+$"
  CreateGateway.input.GatewayDisplayName: required=true, length={min:1,max:100}, pattern="^[a-zA-Z0-9-]*$"
  CreateGateway.errors: InternalServerException(500, server), ThrottlingException(429, client), ValidationException(400, client)
  -> ValidationException is the ONLY client-error shape available for an unredeemable or malformed ActivationKey.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator (run_behavioral_suite builds and starts it).
2. Point a botocore/boto3 'backup-gateway' client at the emulator endpoint.
3. Build a pattern-valid ActivationKey that no gateway appliance ever issued -- any string matching ^[0-9a-zA-Z\-]+$ of length 1-50, e.g. "sim2real-bgw-unissued-0001". Pick a unique GatewayDisplayName matching ^[a-zA-Z0-9-]*$, e.g. "sim2real-bgw-gw-0001".
4. Call:
     bg.create_gateway(ActivationKey="sim2real-bgw-unissued-0001",
                       GatewayDisplayName="sim2real-bgw-gw-0001",
                       GatewayType="BACKUP_VM")
   Observe: returns HTTP 200 with a GatewayArn.
   Expected: botocore.exceptions.ClientError with HTTP 400, Error.Code == "ValidationException".
5. Call bg.list_gateways() (paging via NextToken until exhausted) and collect GatewayDisplayName values.
   Observe: "sim2real-bgw-gw-0001" IS listed -- the refused call leaked a gateway.
   Expected: the name is absent, because a rejected CreateGateway creates nothing.
6. Call create_gateway with an ActivationKey that violates the modeled pattern:
     bg.create_gateway(ActivationKey="not a valid key!!",
                       GatewayDisplayName="sim2real-bgw-gw2-0001",
                       GatewayType="BACKUP_VM")
   Expected: HTTP 400 ValidationException whose message is built from the request, naming member 'activationKey' and the violated regular-expression constraint; and the display name must not appear in list_gateways().
7. Replay the whole check for the authoritative verdict:
     run_behavioral_suite()  ->  backup-gateway.create-gateway-rejects-unissued-activation-key
