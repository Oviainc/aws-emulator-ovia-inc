Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__inspector2.deploy-inspectorv2-filter-and-scan-config-round-trip-and-delete cloudformation__inspector2.deploy-inspectorv2-filter-invalid-filter-action-rejected-and-rolled-back
Issue-Key: cloudformation__inspector2|CreateStack|ValidationError
Issue-Commit: 53cd3feb2c3f3c67eaa2a19cd2ddf09d89eca5f3

# cloudformation__inspector2: CreateStack refuses AWS::InspectorV2::Filter and AWS::InspectorV2::CodeSecurityScanConfiguration as unrecognized resource types, so no Inspector2 resource can be deployed from a template

> _Note: this issue was identified against the code base at commit `53cd3feb2c3f3c67eaa2a19cd2ddf09d89eca5f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause, grounded in two failing behavioral checks.

Cause 1 of 1: CloudFormation's resource-type registry has no AWS::InspectorV2::* types, so CreateStack rejects the template at format-validation time and no Inspector2 resource is ever provisioned, validated or rolled back through CloudFormation.

AWS: recorded behavior only for the registry contents (the CloudFormation Smithy model does not enumerate resource types; CreateStack declares ValidationError as its template-format refusal and returns 200 with a StackId when the template is accepted). AWS supports AWS::InspectorV2::Filter and AWS::InspectorV2::CodeSecurityScanConfiguration as first-class resource types, so on real AWS both checks' CreateStack calls are ACCEPTED (HTTP 200, a StackId is returned) and the divergence is only visible later: (a) the round-trip template reaches CREATE_COMPLETE, Ref of the Filter and of the ScanConfig equal the arn / scanConfigurationArn Inspector2 itself lists under the declared names, the filter reads back with action SUPPRESS, its description, criteria {title:[{comparison:EQUALS,value:<declared name>}]} and its own template tags, the scan configuration reads back with level ACCOUNT, ruleSetCategories [SCA], periodicScanConfiguration.frequency WEEKLY, scopeSettings {projectSelectionScope: ALL} and its tags, and DeleteStack removes both (ListFilters by arn is empty; GetCodeSecurityScanConfiguration answers ResourceNotFoundException "Scan configuration not found"); (b) a Filter whose FilterAction is outside the enum is refused by early property validation — the stack ends ROLLBACK_COMPLETE, DescribeEvents records exactly one VALIDATION_ERROR row (LogicalResourceId "Filter", ValidationName PROPERTY_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationPath "/Resources/Filter/Properties/FilterAction") whose reason is "<rejected value> is not a valid enum value. Supported values: [NONE, SUPPRESS]" (fixed text: " is not a valid enum value. Supported values: [NONE, SUPPRESS]"; the leading value is taken verbatim from the template property), and Inspector2 lists no filter under the rejected name — while the same template with a valid FilterAction reaches CREATE_COMPLETE.

Emulator: CreateStack itself refuses the request with HTTP 400 ValidationError and the message "Template format error: Unrecognized resource types: [<type list>]" — "[AWS::InspectorV2::Filter, AWS::InspectorV2::CodeSecurityScanConfiguration]" for the round-trip template and "[AWS::InspectorV2::Filter]" for the negative-validation template. The fixed text "Template format error: Unrecognized resource types: " is the emulator's own correct wording for a genuinely unknown type; the defect is that these two Inspector2 types are reported as unknown at all. No stack is created, so nothing downstream (provisioning, Ref/Outputs, property validation, rollback, delete-time removal) is reachable.

Why: a client that deploys Inspector2 through CloudFormation (CDK, SAM, or a plain template) cannot deploy at all — every stack carrying a Filter or a CodeSecurityScanConfiguration fails at the API call with a template-format error, which a client reads as "my template is malformed" rather than "the backend lacks this type". Infrastructure-as-code tests that assert a successful create, that read Ref/Outputs to find the filter ARN, or that assert the enum rollback path, all fail at step one and cannot be written against the emulator.

Fix: CloudFormation's resource-type registry and the handlers it dispatches to in the cloudformation crate — register AWS::InspectorV2::Filter and AWS::InspectorV2::CodeSecurityScanConfiguration, back each with a create/read/delete handler that writes through to the inspector2 crate's filter and code-security-scan-configuration state (so ListFilters / ListCodeSecurityScanConfigurations / GetCodeSecurityScanConfiguration see them), return the real Inspector2 ARN as the resource's physical id for Ref, validate FilterAction against the FilterAction enum during property validation so an out-of-enum value produces the PROPERTY_VALIDATION row and ROLLBACK_COMPLETE above, and delete the underlying Inspector2 resource on stack delete.

Traces: cloudformation__inspector2.deploy-inspectorv2-filter-and-scan-config-round-trip-and-delete, cloudformation__inspector2.deploy-inspectorv2-filter-invalid-filter-action-rejected-and-rolled-back.

## Evidence
Both checks fail on their first CreateStack call.

Expected (AWS): CreateStack returns 200 with a StackId; the stack then settles CREATE_COMPLETE (round-trip check) or ROLLBACK_COMPLETE with a single PROPERTY_VALIDATION FAIL row at /Resources/Filter/Properties/FilterAction reading "<value> is not a valid enum value. Supported values: [NONE, SUPPRESS]" (negative check).

Actual (emulator), cloudformation__inspector2.deploy-inspectorv2-filter-and-scan-config-round-trip-and-delete:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::InspectorV2::Filter, AWS::InspectorV2::CodeSecurityScanConfiguration]

Actual (emulator), cloudformation__inspector2.deploy-inspectorv2-filter-invalid-filter-action-rejected-and-rolled-back:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::InspectorV2::Filter]

The templates are minimal: Resources.Filter = {Type: AWS::InspectorV2::Filter, Properties: {Name, Description, FilterCriteria.Title[{Comparison: EQUALS, Value}], FilterAction, Tags}} and Resources.ScanConfig = {Type: AWS::InspectorV2::CodeSecurityScanConfiguration, Properties: {Name, Level: ACCOUNT, Configuration: {ruleSetCategories: [SCA], periodicScanConfiguration: {frequency: WEEKLY}}, ScopeSettings: {projectSelectionScope: ALL}, Tags}}, with Outputs taking Ref of each. Stack names, filter names and the account/region in the run are per-run values; the requirement is that both resource types are accepted and provisioned.

## Steps to Reproduce
1. Build and boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Call cloudformation CreateStack with a template whose Resources contain one AWS::InspectorV2::Filter (Properties: Name, Description, FilterCriteria {Title: [{Comparison: EQUALS, Value: <name>}]}, FilterAction: SUPPRESS, Tags) and one AWS::InspectorV2::CodeSecurityScanConfiguration (Properties: Name, Level: ACCOUNT, Configuration {ruleSetCategories: [SCA], periodicScanConfiguration {frequency: WEEKLY}}, ScopeSettings {projectSelectionScope: ALL}, Tags), with Outputs taking Ref of each.
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::InspectorV2::Filter, AWS::InspectorV2::CodeSecurityScanConfiguration]" instead of a 200 with a StackId.
4. Repeat with only the Filter resource and FilterAction: BLOCK — the same refusal occurs, so the expected ROLLBACK_COMPLETE plus PROPERTY_VALIDATION row at /Resources/Filter/Properties/FilterAction is never reachable.
5. Equivalent paths exercised by cloudformation__inspector2.deploy-inspectorv2-filter-and-scan-config-round-trip-and-delete and cloudformation__inspector2.deploy-inspectorv2-filter-invalid-filter-action-rejected-and-rolled-back.
