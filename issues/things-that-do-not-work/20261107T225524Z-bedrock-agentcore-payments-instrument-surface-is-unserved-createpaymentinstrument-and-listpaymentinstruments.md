Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.payment-instrument-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.payment-instrument-numeric-precision-preserved|run#8|AssertionError|assert create_code == "AccessDeniedException", (
Issue-Commit: a25c82394d1a52e3f7a14fc6dffbcc94599db86a

# Payments instrument surface is unserved: CreatePaymentInstrument and ListPaymentInstruments answer 501 NotImplemented instead of AWS's 403 AccessDeniedException "Payment manager not found"

> _Note: this issue was identified against the code base at commit `a25c82394d1a52e3f7a14fc6dffbcc94599db86a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePaymentInstrument

## Description
Cause 1 of 1: the bedrock-agentcore (data plane) payment-instrument operations are scaffold stubs, so neither the success path nor the ownership refusal of a payment manager is ever evaluated. Failing check ids for this cause: bedrock-agentcore.payment-instrument-numeric-precision-preserved.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json) binds both operations to the bedrock-agentcore data plane — CreatePaymentInstrument POST /payments/createPaymentInstrument (success 201, required paymentManagerArn/paymentConnectorId/paymentInstrumentType/paymentInstrumentDetails) and ListPaymentInstruments POST /payments/listPaymentInstruments (success 200, required paymentManagerArn). Both model AccessDeniedException at HTTP 403 ("The exception that occurs when you do not have sufficient permissions to perform an action..."). Recorded behavior: a request naming a payment-manager ARN this caller does not own is refused on BOTH operations with HTTP 403, error code AccessDeniedException and the message "Payment manager not found" — the ownership of the manager is resolved before any instrument is created or listed, so the denial is scoped (an unrelated ListABTests in the same session still succeeds) and no partially-created instrument becomes visible. Message template: the whole sentence "Payment manager not found" is FIXED text for this error class — it carries no value from the request (notably it does NOT echo the paymentManagerArn or paymentConnectorId) and no per-run id.

Emulator: both handlers are the generated scaffold and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, so the call answers HTTP 501 with error code `NotImplemented` and the message "operation <Op> is scaffolded but not implemented in this emulator". Replay: `expected CreatePaymentInstrument against a payment manager this caller does not own to be denied AccessDeniedException, got 'NotImplemented'`. ListPaymentInstruments is blocked behind the same stub and is never reached by the check.

Why: a client that handles AccessDeniedException to detect "this payment manager is not mine" (and distinguishes it from a retryable/unsupported condition) sees a 501 NotImplemented instead. 501 is not a modeled error for these operations, so botocore surfaces it as an unmodeled failure: cross-account/cross-tenant denial cannot be exercised at all, and no payment instrument can be created or enumerated. Nothing in the payments data plane is testable against the emulator.

Fix: implement the two operation handlers — services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/create_payment_instrument.rs and .../src/ops/list_payment_instruments.rs — resolving paymentManagerArn against the caller's payment managers in the crate's state before doing any work, and returning the modeled AccessDeniedException (403) with the fixed message when the manager is not one the caller owns; leave the created/listed instrument state untouched on that path. Note the related open stub issue 20261029T212239Z-bedrock-agentcore-control-createpaymentmanager-stub.md covers the bedrock-agentcore-CONTROL CreatePaymentManager stub only — it does not cover these two data-plane operations, and no open issue in this listing names CreatePaymentInstrument or ListPaymentInstruments.

Traces: bedrock-agentcore.payment-instrument-numeric-precision-preserved (suite service heading `### bedrock-agentcore`). Handler citations: create_payment_instrument.rs line 198, list_payment_instruments.rs line 177 (the `not_implemented` returns), with the SCAFFOLDED marker at line 4 of each file.

## Evidence
Expected (AWS, recorded): CreatePaymentInstrument{paymentManagerArn=<arn of a payment-manager the caller does not own>, paymentConnectorId=..., paymentInstrumentType=EMBEDDED_CRYPTO_WALLET, paymentInstrumentDetails={embeddedCryptoWallet:{...}}} -> HTTP 403, Error.Code="AccessDeniedException", Error.Message="Payment manager not found". ListPaymentInstruments{paymentManagerArn=<same arn>} -> HTTP 403, Error.Code="AccessDeniedException", Error.Message="Payment manager not found". An unrelated ListABTests in the same session -> 200 (positive control, so the denial is scoped and not a blanket refusal).

Actual (emulator): HTTP 501, Error.Code="NotImplemented", Error.Message="operation CreatePaymentInstrument is scaffolded but not implemented in this emulator". Replay assertion: AssertionError: expected CreatePaymentInstrument against a payment manager this caller does not own to be denied AccessDeniedException, got 'NotImplemented'. ListPaymentInstruments never reached (same stub: .../src/ops/list_payment_instruments.rs returns Scaffolded::not_implemented("ListPaymentInstruments")).

Model evidence: both operations declare AccessDeniedException with httpError 403; CreatePaymentInstrument http {method POST, uri /payments/createPaymentInstrument, code 201}; ListPaymentInstruments http {method POST, uri /payments/listPaymentInstruments, code 200}. 501/NotImplemented is not a modeled error for either.

## Steps to Reproduce
1. Boot the emulator and point a botocore bedrock-agentcore (data plane) client at it.
2. Call ListABTests to confirm the endpoint and principal are healthy (expect 200).
3. Call create_payment_instrument(paymentManagerArn="arn:aws:bedrock-agentcore:<region>:<account>:payment-manager/<name>-<10-char suffix>" for a manager this caller does not own, paymentConnectorId="<connector>-<10-char suffix>", paymentInstrumentType="EMBEDDED_CRYPTO_WALLET", paymentInstrumentDetails={"embeddedCryptoWallet": {"network": "ETHEREUM", "linkedAccounts": [{"email": {"emailAddress": "<addr>"}}]}}).
   Expect: ClientError with HTTP 403, code AccessDeniedException, message "Payment manager not found". Observed: HTTP 501, code NotImplemented.
4. Call list_payment_instruments(paymentManagerArn=<same arn>). Expect the same 403 AccessDeniedException "Payment manager not found"; observed HTTP 501 NotImplemented.
5. Re-replay behavioral check bedrock-agentcore.payment-instrument-numeric-precision-preserved to confirm the fix.
