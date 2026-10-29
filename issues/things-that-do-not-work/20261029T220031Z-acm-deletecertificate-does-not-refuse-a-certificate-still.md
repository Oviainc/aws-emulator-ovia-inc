Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-create-routing-rule-missing-condition-rejected
Issue-Step: teardown#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-create-routing-rule-missing-condition-rejected apigatewayv2-put-routing-rule-nonexistent-not-found apigatewayv2-routing-rule-priority-numeric-precision
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: acm|DeleteCertificate|4xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# acm DeleteCertificate does not refuse a certificate still in use

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCertificate

## Description
Cause 1 of 1: DeleteCertificate on a certificate that is still associated with an in-use resource.
AWS: refuses the delete with HTTP 400 and the modeled `ResourceInUseException`. Smithy: DeleteCertificate binds errors AccessDeniedException(403), ConflictException(400), InvalidArnException(400), ResourceInUseException(400), ResourceNotFoundException(400), ThrottlingException(400), ValidationException(400); ResourceInUseException is documented as the refusal when the certificate is in use by another AWS service. The recorded teardown deletes an ACM certificate that an ApiGatewayV2 custom domain name still references, and AWS answers 400.
Emulator: 200 - the delete succeeds and the certificate disappears while a resource still references it.
Why: a client relying on ACM to protect a certificate that is still attached loses it; a dependent custom domain name is then backed by a certificate that no longer exists, and the client never sees the ResourceInUseException it is written to handle (typically by detaching first, then retrying).
Fix: the DeleteCertificate handler's in-use check - it must consult the certificate's association/usage state before deleting and refuse with ResourceInUseException (400) when any association remains.
Traces: apigatewayv2-create-routing-rule-missing-condition-rejected#teardown#16 (the cluster also covers apigatewayv2-put-routing-rule-nonexistent-not-found#teardown#18 and apigatewayv2-routing-rule-priority-numeric-precision#teardown#14).

## Evidence
Replay verdict `mismatch`, divergence_key `service=acm|DeleteCertificate|status=400->200|keys=status`, detail `status: expected 400, got 200`, expected_status 400, actual_status 200, 3 affected recorded steps.

## Steps to Reproduce
Replay the corpus. In fixture apigatewayv2-create-routing-rule-missing-condition-rejected, step teardown#16 runs ACM DeleteCertificate for the certificate the fixture's ApiGatewayV2 custom domain name was created with, while that domain name association still exists. The emulator answers 200; AWS answers 400 ResourceInUseException.
