Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-configuration-check-catalog
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-configuration-check-catalog
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|StartConfigurationChecks|4xx->4xx|$.message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# StartConfigurationChecks enum-list validation message reports element path instead of list value

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationChecks

## Description
Cause 1 of 1: StartConfigurationChecks with an unknown ConfigurationCheckIds entry.
AWS: StartConfigurationChecks models ConfigurationCheckIds as a list of the ConfigurationCheckType enum and ValidationException (client, httpError 400). When an element is not a member of the enum, AWS reports the failure against the LIST member, quoting the whole list value and nesting the element constraint: "1 validation error detected: Value '[SAP_CHECK_NOT_A_REAL_ID]' at 'configurationCheckIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03]]" (recorded response; status 400).
Emulator: returns 400 but reports the failure against the element path and quotes only the element: "1 validation error detected: Value 'SAP_CHECK_NOT_A_REAL_ID' at 'configurationCheckIds.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03]". Both the reported member path and the constraint nesting differ, and the element index used ('.1.member') is not a path AWS emits for this shape.
Why: the message here is built from the request — it names the member and its constraint — so its construction is a requirement. A client that maps the reported member path back onto its request (to highlight the offending field) is pointed at a path that does not exist in the AWS contract, and the enum value set is reported one nesting level shallower than AWS reports it.
Fix: the ssm-sap input-validation message renderer for list-of-enum members — a constraint failure on a list element must be attributed to the list member itself, with the full list value rendered and the element constraint wrapped as "Member must satisfy constraint: [<element constraint>]".
Traces: ssm-sap-configuration-check-catalog#task#2.

## Evidence
Replay of simparity-wire-aws-v1, divergence_key service=ssm-sap|StartConfigurationChecks|status=400->400|keys=$.message, verdict=mismatch, fixture ssm-sap-configuration-check-catalog step task#2:
expected $.message "1 validation error detected: Value '[SAP_CHECK_NOT_A_REAL_ID]' at 'configurationCheckIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03]]"
actual   $.message "1 validation error detected: Value 'SAP_CHECK_NOT_A_REAL_ID' at 'configurationCheckIds.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03]"
Status matched at 400 in both.
Model cited: smithy_query operation StartConfigurationChecks -> input ConfigurationCheckIds target ConfigurationCheckTypeList (list), ApplicationId required; errors include ValidationException httpError 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay simparity-wire-aws-v1.
2. Fixture ssm-sap-configuration-check-catalog, step task#2 — POST /start-configuration-checks with ConfigurationCheckIds=["SAP_CHECK_NOT_A_REAL_ID"].
3. Observe HTTP 400 whose $.message attributes the failure to 'configurationCheckIds.1.member' with an unwrapped enum constraint, instead of to 'configurationCheckIds' with the list value quoted and the element constraint nested.
