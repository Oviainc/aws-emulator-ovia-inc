Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__logs.cfn-logs-account-policy-and-storage-tier-policy-singleton-round-trip cloudformation__logs.cfn-logs-delivery-source-destination-and-delivery-round-trip-then-delete cloudformation__logs.cfn-logs-delivery-source-invalid-log-type-rejected cloudformation__logs.deploy-logs-anomaly-detector-invalid-evaluationfrequency-rejected cloudformation__logs.deploy-logs-anomaly-query-transformer-round-trip-then-child-and-stack-delete cloudformation__logs.deploy-logs-scheduledquery-round-trip-then-child-and-stack-delete
Issue-Key: cloudformation__logs|CreateStack|ValidationError
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CloudFormation rejects six modelled AWS::Logs::* resource types as 'Unrecognized resource types'

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry is missing six AWS::Logs::* types, so every template that declares one is refused at CreateStack.
AWS: CreateStack accepts templates declaring AWS::Logs::AccountPolicy, AWS::Logs::DeliverySource, AWS::Logs::DeliveryDestination, AWS::Logs::Delivery, AWS::Logs::LogAnomalyDetector, AWS::Logs::Transformer and AWS::Logs::ScheduledQuery and creates the corresponding CloudWatch Logs resources (each type backs a modelled logs operation: PutAccountPolicy, PutDeliverySource, PutDeliveryDestination, CreateDelivery, CreateLogAnomalyDetector, PutTransformer, CreateScheduledQuery). The recorded runs deploy these stacks to CREATE_COMPLETE, read the resources back through the logs API and then delete the stack.
Emulator: CreateStack is refused with HTTP 400, error code 'ValidationError' and message 'Template format error: Unrecognized resource types: [<types>]' — observed for [AWS::Logs::AccountPolicy], [AWS::Logs::DeliveryDestination, AWS::Logs::DeliverySource, AWS::Logs::Delivery], [AWS::Logs::DeliverySource], [AWS::Logs::LogAnomalyDetector], [AWS::Logs::LogAnomalyDetector, AWS::Logs::Transformer] and [AWS::Logs::ScheduledQuery]. The fixed text is 'Template format error: Unrecognized resource types: [...]'; the bracketed type list comes from the template.
Why: an entire family of CloudWatch Logs resources is undeployable by template — account policies, the vended-logs delivery triple, anomaly detectors, transformers and scheduled queries — and the rejection also hides whether those resources would be validated or rolled back correctly.
Fix: the CloudFormation resource-type registry plus a resource handler per type (create/update/delete mapped onto the corresponding CloudWatch Logs operations).
Traces: the six checks listed in trace_ids.

## Evidence
Expected: each stack reaches CREATE_COMPLETE. Actual, e.g.: 'botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Logs::DeliveryDestination, AWS::Logs::DeliverySource, AWS::Logs::Delivery]'; the same message appears with [AWS::Logs::AccountPolicy], [AWS::Logs::DeliverySource], [AWS::Logs::LogAnomalyDetector], [AWS::Logs::LogAnomalyDetector, AWS::Logs::Transformer] and [AWS::Logs::ScheduledQuery].

## Steps to Reproduce
Replay the six cloudformation__logs checks listed in trace_ids; each calls CreateStack with a template declaring the named AWS::Logs::* type and the call is refused with ValidationError.
