Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Every configuration check ends in ERROR with a canned emulator message, and RuleStatusCounts / ConfigurationCheckName / ConfigurationCheckDescription are never populated

_Filed: 2026-10-24T15:17:25Z_
_Operation: GetConfigurationCheckOperation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A configuration check operation started by StartConfigurationChecks is read back by GetConfigurationCheckOperation. Expected (real service): the operation starts INPROGRESS and reaches a terminal status that reflects the evaluation of the named check — SUCCESS when the rules ran — and the record carries RuleStatusCounts (Passed/Failed/Warning/Info/Unknown) plus the check's catalogue prose in ConfigurationCheckName and ConfigurationCheckDescription (the same text ListConfigurationCheckDefinitions reports for that ConfigurationCheckType).

Actual: the emulator hard-codes the terminal status to the model's failure status ERROR with a fixed StatusMessage ("This emulator holds no SAP system to evaluate configuration check rules against, ..."), and the projection sets configuration_check_name: None, configuration_check_description: None, rule_status_counts: None unconditionally. A client polling for a successful check can therefore never observe SUCCESS, and no response of this service ever names or describes a check, even though the check id is one of only three modelled values (SAP_CHECK_01/02/03) whose name/description are static catalogue data. The same projection is used for the ConfigurationCheckOperations returned by StartConfigurationChecks, so those three members are absent there too.

## Evidence

Smithy model: ConfigurationCheckOperation declares Status (OperationStatus), StatusMessage, ConfigurationCheckName ("The name of the configuration check that was performed"), ConfigurationCheckDescription, RuleStatusCounts ("A summary of all the rule results, showing counts for each status type"); ConfigurationCheckType = SAP_CHECK_01|SAP_CHECK_02|SAP_CHECK_03. Handler: services/service-com-amazonaws-ssmsap-ssmsap/src/ops/get_configuration_check_operation.rs reads the stored record and returns `check.view()`; src/state.rs StoredConfigurationCheckOperation::settle sets `self.status = OPERATION_STATUS_ERROR` + CONFIGURATION_CHECK_NOT_EVALUATED_MESSAGE, and StoredConfigurationCheckOperation::view returns configuration_check_name: None, configuration_check_description: None, rule_status_counts: None. Tests (tests/all/configuration_check_reads.rs, tests/all/configuration_checks.rs) pin the current ERROR-with-no-counts record; no test asserts a successful check with rule counts or the check's name/description.

## Steps to Reproduce

1. RegisterApplication an application (e.g. ApplicationId=HDB-TEST, ApplicationType=HANA, Sid=HDB, SapInstanceNumber=00, one credential) and let it settle
2. POST /start-configuration-checks with {"ApplicationId":"HDB-TEST","ConfigurationCheckIds":["SAP_CHECK_01"]} and note the returned Id — observe ConfigurationCheckName and ConfigurationCheckDescription are absent
3. Wait for the operation to reach its terminal status, then POST /get-configuration-check-operation with that OperationId
4. Observe Status=ERROR with the emulator's canned StatusMessage, and no RuleStatusCounts / ConfigurationCheckName / ConfigurationCheckDescription, where the real service reports a completed check with rule counts and the check's catalogue name and description
