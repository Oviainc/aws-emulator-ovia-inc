Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-put-rest-api-warnings-transient-and-strict-overwrite-preserves-state
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-put-rest-api-warnings-transient-and-strict-overwrite-preserves-state|run#12|AssertionError|assert warnings == want_warnings, (
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# API Gateway: PutRestApi returns the wrong warning for a method whose security references no created authorizer

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRestApi

## Description
Cause 1 of 1: the `warnings` list PutRestApi returns for an imported method that declares `security` against a security definition the importer could not turn into an authorizer carries the wrong text.

AWS: Smithy models PutRestApi's output RestApi with a `warnings` member (ListOfString) — the importer's non-fatal diagnostics are part of the response, not an error message, so a client reads them directly. For a method that declares `security` while no custom authorizer was created and the x-amazon-apigateway-auth extension was not set, AWS's warning is the fixed sentence "Method 'GET' on resource '/thing' specified security, but no custom authorizers were created and the extension x-amazon-apigateway-auth was not properly set. This method will be not be secured." — the load-bearing clause being that the method WILL NOT BE SECURED. The sibling warning for the unsupported security-definition type matches already.

Emulator: emits "Method 'GET' on resource '/thing' specified security, but no custom authorizers were created and the extension x-amazon-apigateway-auth was not provided. Ignoring." — same position in the list, but it reports the outcome as "Ignoring" instead of telling the caller the method is left unsecured.

Why: `warnings` is a response data member, so a client (or a CI gate using failOnWarnings) that inspects or matches the importer's diagnostics sees a different outcome description for the one case where an import silently leaves a method open to unauthenticated callers.

Fix: the OpenAPI importer's warning construction in the PutRestApi/ImportRestApi handler — the security-without-authorizer branch.

Traces: apigateway.apigw-put-rest-api-warnings-transient-and-strict-overwrite-preserves-state.

## Evidence
apigateway.apigw-put-rest-api-warnings-transient-and-strict-overwrite-preserves-state
expected warnings:
  ["Unsupported security definition type 'basic' for 'myAuth'. Ignoring.",
   "Method 'GET' on resource '/thing' specified security, but no custom authorizers were created and the extension x-amazon-apigateway-auth was not properly set. This method will be not be secured."]
actual warnings:
  ["Unsupported security definition type 'basic' for 'myAuth'. Ignoring.",
   "Method 'GET' on resource '/thing' specified security, but no custom authorizers were created and the extension x-amazon-apigateway-auth was not provided. Ignoring."]

## Steps to Reproduce
1. CreateRestApi.
2. PutRestApi mode=overwrite with an OpenAPI document declaring a securityDefinitions entry 'myAuth' of type 'basic' and a GET /thing method whose `security` references it, with no x-amazon-apigateway-auth extension.
3. Read the response's `warnings` list: the second entry reports "Ignoring." instead of AWS's "...was not properly set. This method will be not be secured."
