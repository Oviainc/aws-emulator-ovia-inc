Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.list-finding-aggregations-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.list-finding-aggregations-type-mismatch-rejected|run#6|AssertionError|assert msg == (
Issue-Commit: 46ea0afdef4b8d7aa8ecb7f663be31c677c6bd6d

# inspector2 ListFindingAggregations enum ValidationException message interpolates the rejected value and uses the wrong enum value-set ordering

> _Note: this issue was identified against the code base at commit `46ea0afdef4b8d7aa8ecb7f663be31c677c6bd6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFindingAggregations

## Description
SAME SHARED BUILDER as the already-open issue 20261029T202306Z-inspector2-enum-validationexception-messages-interpolate-the-rejected-value.md (and the wire issue it references, 20261029T171819Z-inspector2-constraint-violation-validationexception-messages-add-the-rejected.md). Neither carries this check's replay key and the replay report did not mark this check OWNED by them, so it is filed here with its own behavioral check id; a fixer correcting the scalar-enum branch of the shared constraint-violation message builder should close it together with those.

Cause 1 of 1: ListFindingAggregations with an unmodelled aggregationType value.
AWS: Smithy: ListFindingAggregations.aggregationType (required) targets the AggregationType enum; modeled error ValidationException (httpError 400); http POST /findings/aggregation/list. Recorded behavior: HTTP 400, error code ValidationException, message "1 validation error detected: Value at 'aggregationType' failed to satisfy constraint: Member must satisfy enum value set: [AWS_EC2_INSTANCE, PACKAGE, REPOSITORY, ACCOUNT, IMAGE_LAYER, AWS_LAMBDA_FUNCTION, FINDING_TYPE, AWS_ECR_CONTAINER, TITLE, LAMBDA_LAYER, SERVERLESS_FUNCTION, CONTAINER_IMAGE, VM_INSTANCE, AMI]". The fixed text of that sentence is the requirement: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". The variable parts are the member path (here the top-level required member name, taken from the request shape, 'aggregationType') and the enum value set (taken from the service's enum, in AWS's ordering). The rejected request value is NOT part of the message at all for this constraint class.
Emulator: answers HTTP 400 with error code ValidationException — status and code agree — but message "1 validation error detected: Value 'NOT_A_REAL_AGGREGATION_TYPE' at 'aggregationType' failed to satisfy constraint: Member must satisfy enum value set: [FINDING_TYPE, PACKAGE, TITLE, REPOSITORY, AMI, AWS_EC2_INSTANCE, AWS_ECR_CONTAINER, IMAGE_LAYER, ACCOUNT, AWS_LAMBDA_FUNCTION, LAMBDA_LAYER, CODE_REPOSITORY, VM_INSTANCE, CONTAINER_IMAGE, SERVERLESS_FUNCTION]". Two divergences in $.message: (a) it interpolates the rejected value into the "Value '<v>' at '<path>'" clause where AWS writes "Value at '<path>'"; (b) it enumerates the enum value set in model-declaration order rather than the ordering AWS emits. The positive control (aggregationType=ACCOUNT with aggregationRequest={"accountAggregation":{}}) succeeds and returns a responses list, so only the refusal's message text is wrong.
Why: a client or assertion harness that matches AWS's ValidationException wording for an invalid aggregationType never matches, and because the submitted value is echoed the message text changes with every distinct bad input — AWS's message for this class is stable and request-value-independent, so error-text matching and log fingerprinting behave differently against the emulator than against AWS.
Fix: the shared constraint-violation ValidationException message builder in the inspector2 crate, scalar-enum branch, reached from ListFindingAggregations request validation (suppress the rejected value; emit AWS's enum value-set ordering).
Traces: inspector2.list-finding-aggregations-type-mismatch-rejected.

## Evidence
Check inspector2.list-finding-aggregations-type-mismatch-rejected [mismatch]: AssertionError on the message comparison.
Expected (real AWS): "1 validation error detected: Value at 'aggregationType' failed to satisfy constraint: Member must satisfy enum value set: [AWS_EC2_INSTANCE, PACKAGE, REPOSITORY, ACCOUNT, IMAGE_LAYER, AWS_LAMBDA_FUNCTION, FINDING_TYPE, AWS_ECR_CONTAINER, TITLE, LAMBDA_LAYER, SERVERLESS_FUNCTION, CONTAINER_IMAGE, VM_INSTANCE, AMI]"
Actual (emulator): "1 validation error detected: Value 'NOT_A_REAL_AGGREGATION_TYPE' at 'aggregationType' failed to satisfy constraint: Member must satisfy enum value set: [FINDING_TYPE, PACKAGE, TITLE, REPOSITORY, AMI, AWS_EC2_INSTANCE, AWS_ECR_CONTAINER, IMAGE_LAYER, ACCOUNT, AWS_LAMBDA_FUNCTION, LAMBDA_LAYER, CODE_REPOSITORY, VM_INSTANCE, CONTAINER_IMAGE, SERVERLESS_FUNCTION]"
HTTP status (400) and error code (ValidationException) agree; only $.message diverges.
Fixed text of the message class: "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]". Values from the request/model: <member path> = the rejecting input member name ('aggregationType'); <values> = the AggregationType enum values in AWS's emitted order. The rejected request value ('NOT_A_REAL_AGGREGATION_TYPE') is a per-request value and must not appear.
Smithy model (services/service-com-amazonaws-inspector2-inspector2/smithy-model/com-amazonaws-inspector2-inspector2.json): ListFindingAggregations input aggregationType -> AggregationType (required); AggregationType values declared FINDING_TYPE, PACKAGE, TITLE, REPOSITORY, AMI, AWS_EC2_INSTANCE, AWS_ECR_CONTAINER, IMAGE_LAYER, ACCOUNT, AWS_LAMBDA_FUNCTION, LAMBDA_LAYER, CODE_REPOSITORY, VM_INSTANCE, CONTAINER_IMAGE, SERVERLESS_FUNCTION; errors ValidationException (httpError 400), ThrottlingException (429), InternalServerException (500).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1), service inspector2.
2. Positive control: POST /findings/aggregation/list (ListFindingAggregations) with aggregationType="ACCOUNT" and aggregationRequest={"accountAggregation":{}}. Expect 200 with a "responses" list — this passes today.
3. Negative: POST /findings/aggregation/list with aggregationType="NOT_A_REAL_AGGREGATION_TYPE" and aggregationRequest={"accountAggregation":{}}.
4. Expect HTTP 400, Error.Code ValidationException, and Error.Message "1 validation error detected: Value at 'aggregationType' failed to satisfy constraint: Member must satisfy enum value set: [AWS_EC2_INSTANCE, PACKAGE, REPOSITORY, ACCOUNT, IMAGE_LAYER, AWS_LAMBDA_FUNCTION, FINDING_TYPE, AWS_ECR_CONTAINER, TITLE, LAMBDA_LAYER, SERVERLESS_FUNCTION, CONTAINER_IMAGE, VM_INSTANCE, AMI]". Compare $.message exactly.
5. Observed instead: the message carries "Value 'NOT_A_REAL_AGGREGATION_TYPE' at 'aggregationType'" and the value set in model-declaration order.
6. Re-replay inspector2.list-finding-aggregations-type-mismatch-rejected to confirm.
