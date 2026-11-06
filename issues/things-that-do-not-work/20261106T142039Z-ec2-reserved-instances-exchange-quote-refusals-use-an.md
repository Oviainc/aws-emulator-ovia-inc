Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-capacity-reservation-and-ri-exchange-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-capacity-reservation-and-ri-exchange-errors ec2-reserved-instances-exchange-quote-errors
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AcceptReservedInstancesExchangeQuote|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AcceptReservedInstancesExchangeQuote|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetReservedInstancesExchangeQuote|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|GetReservedInstancesExchangeQuote|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetReservedInstancesExchangeQuote|4xx->4xx|status
Issue-Commit: dabd605678ea57cce68275edff8c08147ffd283a

# EC2 reserved-instances exchange-quote refusals use an emulator error code and ignore DryRun

> _Note: this issue was identified against the code base at commit `dabd605678ea57cce68275edff8c08147ffd283a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptReservedInstancesExchangeQuote

## Description
Three net-new wire divergences share one root cause: the reserved-instances exchange-quote pair (AcceptReservedInstancesExchangeQuote and GetReservedInstancesExchangeQuote) rejects a bad ReservedInstanceIds value with an emulator-invented error code and an emulator-authored message instead of EC2's refusal, and the same validation runs ahead of the DryRun precondition so a DryRun request never gets EC2's 412 answer. This issue bundles 3 causes; all 3 must be closed. The open issue '20261023T102725Z-ec2-getreservedinstancesexchangequote-stub.md' concerns the same operation as a scaffold stub but does not carry these replay keys (the operation now answers 400, not 501), and '20261101T121132Z-ec2-3-net-new-refusal-fidelity-root-causes.md' describes EC2 refusal fidelity on other steps without these keys.

Cause 1 of 3: AcceptReservedInstancesExchangeQuote refuses a malformed reserved-instances id with the wrong error code and a non-sentence message.
AWS: Smithy: AcceptReservedInstancesExchangeQuote input ReservedInstanceIds -> ReservedInstanceIdSet (clientOptional, required, xmlName ReservedInstanceId), DryRun -> Boolean; the ec2 model binds no operation-level errors, so the refusal itself is recorded behavior only. The recording shows HTTP 400 with Response.Errors.Error.Code 'InvalidReservedInstancesID.NotFound' and Message "The reserved-instances ID '<id>' is malformed" - the fixed text is "The reserved-instances ID '" ... "' is malformed", and <id> is the offending value taken verbatim from the request's ReservedInstanceId member.
Emulator: HTTP 400 (status matches) with Code 'InvalidReservedInstanceId' - a code EC2 does not define - and Message '[<id>]', i.e. the rejected id list rendered as a debug-formatted collection with no sentence at all.
Why: a client that branches on the EC2 error code (retry/report logic keyed on 'InvalidReservedInstancesID.NotFound', or an SDK error-shape match) sees an unknown code and falls through to generic handling, and any user-facing surfacing of the message prints a bracketed list instead of EC2's sentence.
Fix: the AcceptReservedInstancesExchangeQuote handler's input validation and its error serializer - the id-format refusal must emit EC2's code and sentence template.
Traces: ec2-accept-capacity-reservation-and-ri-exchange-errors#task#2.

Cause 2 of 3: GetReservedInstancesExchangeQuote has the identical defect on the read side of the pair.
AWS: Smithy: GetReservedInstancesExchangeQuote input ReservedInstanceIds -> ReservedInstanceIdSet (clientOptional, required), DryRun -> Boolean; no modeled errors, so recorded behavior only. The recording shows HTTP 400, Code 'InvalidReservedInstancesID.NotFound', Message "The reserved-instances ID '<id>' is malformed" with the same fixed text and the same request-supplied id.
Emulator: HTTP 400 with Code 'InvalidReservedInstanceId' and Message '[<id>]'.
Why: as cause 1; both halves of the exchange-quote flow are unusable for error-driven clients, and a caller cannot distinguish a malformed id from any other rejection.
Fix: the GetReservedInstancesExchangeQuote handler's input validation / error serializer, sharing the same reserved-instances-id refusal helper as cause 1.
Traces: ec2-reserved-instances-exchange-quote-errors#task#1.

Cause 3 of 3: a DryRun request to GetReservedInstancesExchangeQuote is answered with the id-validation refusal instead of EC2's DryRun precondition result.
AWS: Smithy: the operation's DryRun input member is the standard EC2 dry-run flag. Recorded behavior: with DryRun set the call answers HTTP 412 with Code 'DryRunOperation' and Message 'Request would have succeeded, but DryRun flag is set.' - that message is entirely fixed text with no variable part - and the dry-run verdict is reported ahead of the reserved-instances-id rejection the same request would otherwise draw.
Emulator: HTTP 400 with Code 'InvalidReservedInstanceId' and Message '[<id>]'; the DryRun flag is never evaluated, so no 412/DryRunOperation answer is ever produced for this operation.
Why: dry-run is a permission/feasibility probe - clients treat 412 DryRunOperation as success and any other status as failure. Getting a 400 makes a dry run look like a real rejection, so provisioning tools abort a flow AWS would have allowed.
Fix: the GetReservedInstancesExchangeQuote handler - evaluate the DryRun precondition in the operation's request path (before the id-format rejection) and return the 412 DryRunOperation refusal.
Traces: ec2-reserved-instances-exchange-quote-errors#task#2.

## Evidence
Cause 1 (ec2-accept-capacity-reservation-and-ri-exchange-errors#task#2, AcceptReservedInstancesExchangeQuote, status 400 expected and 400 observed): $.Response.Errors.Error.Code value_mismatch (expected 'InvalidReservedInstancesID.NotFound', got 'InvalidReservedInstanceId'); $.Response.Errors.Error.Message value_mismatch (expected "The reserved-instances ID 'not-a-valid-reserved-instance-id' is malformed", got '[not-a-valid-reserved-instance-id]').
Cause 2 (ec2-reserved-instances-exchange-quote-errors#task#1, GetReservedInstancesExchangeQuote, status 400 -> 400): $.Response.Errors.Error.Code value_mismatch (expected 'InvalidReservedInstancesID.NotFound', got 'InvalidReservedInstanceId'); $.Response.Errors.Error.Message value_mismatch (expected "The reserved-instances ID 'ri-000000000000000000' is malformed", got '[ri-000000000000000000]').
Cause 3 (ec2-reserved-instances-exchange-quote-errors#task#2, GetReservedInstancesExchangeQuote): status expected 412, got 400; $.Response.Errors.Error.Code value_mismatch (expected 'DryRunOperation', got 'InvalidReservedInstanceId'); $.Response.Errors.Error.Message value_mismatch (expected 'Request would have succeeded, but DryRun flag is set.', got '[ri-000000000000000000]').
In each recorded message the id inside the quotes is the value the request supplied; the surrounding sentence is the fixed template.

## Steps to Reproduce
1. Boot the emulator and replay the simparity-wire-aws-v1 wire corpus.
2. Inspect ec2-accept-capacity-reservation-and-ri-exchange-errors#task#2: AcceptReservedInstancesExchangeQuote with a syntactically invalid ReservedInstanceId; compare Response.Errors.Error.Code/Message against the recorded 400 'InvalidReservedInstancesID.NotFound' / "The reserved-instances ID '<id>' is malformed".
3. Inspect ec2-reserved-instances-exchange-quote-errors#task#1 for the same call shape on GetReservedInstancesExchangeQuote.
4. Inspect ec2-reserved-instances-exchange-quote-errors#task#2: the same GetReservedInstancesExchangeQuote request with DryRun set must answer HTTP 412 / 'DryRunOperation' / 'Request would have succeeded, but DryRun flag is set.'; the emulator answers 400 with its id error instead.
