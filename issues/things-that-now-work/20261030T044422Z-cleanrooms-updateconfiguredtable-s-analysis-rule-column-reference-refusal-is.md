Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.update-configured-table-analysis-method-conflict-rejected
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.update-configured-table-analysis-method-conflict-rejected|run#14|AssertionError|assert error.get("Message") == (
Issue-Commit: c6cd310496053acfb8677287c8f8c63a1ffba3db

# UpdateConfiguredTable's analysis-rule column-reference refusal is emulator-worded instead of AWS's "The following columns in the analysis rule aren't present in the allowed columns list" text

> _Note: this issue was identified against the code base at commit `c6cd310496053acfb8677287c8f8c63a1ffba3db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfiguredTable

## Description
One net-new root cause on the cleanrooms control plane, reached through UpdateConfiguredTable's allowedColumns-shrink refusal path. 1 cause must be closed before this issue is submitted.

Cause 1 of 1: when UpdateConfiguredTable narrows allowedColumns so that a column still referenced by an ACTIVE analysis rule would be dropped, the emulator refuses with the correct error code but a wholly emulator-authored sentence.

AWS: Smithy: UpdateConfiguredTable is PATCH /configuredTables/{configuredTableIdentifier} (success 200, required output member configuredTable); its input member allowedColumns targets AllowedColumnList with @length min 1; the operation models ValidationException (httpError 400, documented "The input fails to satisfy the specified constraints.", members message/reason/fieldList, reason from ValidationExceptionReason = FIELD_VALIDATION_FAILED|INVALID_CONFIGURATION|INVALID_QUERY|IAM_SYNCHRONIZATION_DELAY) alongside ConflictException(409) and ResourceNotFoundException(404). The model does not state the wording, so recorded behavior is the authority here: AWS refuses the shrink with HTTP 400, error code ValidationException, and the message template "The following columns in the analysis rule aren't present in the allowed columns list: [<columns>]. To be used in the analysis rule, columns must be specified as allowed columns." The fixed text is "The following columns in the analysis rule aren't present in the allowed columns list: [", "]. To be used in the analysis rule, columns must be specified as allowed columns."; the single variable part is the list of column names that the active analysis rule references and the requested allowedColumns omits — taken from the stored analysis rule's joinColumns/listColumns intersected against the request's allowedColumns. Note what AWS does NOT interpolate: no configured-table id and no analysis-rule type appear in the sentence. The surrounding legs recorded by the same check are also AWS behavior and all pass today: dropping a column the rule does NOT reference succeeds, allowedColumns is unchanged after the rejected update, and after DeleteConfiguredTableAnalysisRule the identical update succeeds.

Emulator: refuses with the correct Code 'ValidationException' but the message "Cannot remove columns from allowedColumns while the <analysisRuleType> analysis rule of configured table <configuredTableId> references them: <columns>" — an emulator-authored sentence that inverts AWS's framing (it describes the forbidden mutation rather than the unsatisfied column-reference constraint), interpolates the rule type and the configured-table id into text AWS interpolates nothing into, renders the column list bare rather than in AWS's square brackets, omits AWS's second sentence ("To be used in the analysis rule, columns must be specified as allowed columns.") entirely, and has no terminating period. The uuid and the rule type observed in the replay are per-run/per-request values and are not themselves part of any requirement; the requirement is AWS's template above.

Why: a client relying on AWS's behavior gets the right error class but not the right diagnosis. Clients, CLI wrappers, CDK/Terraform-style reconcilers and test suites that match or display the documented refusal text do not recognise the emulator's sentence, so the actionable remediation AWS states ("columns must be specified as allowed columns") is never surfaced to the operator; log scrapers and error-classification tables keyed on AWS's sentence never match. Because ValidationException carries no structured field naming the offending columns in this case, the message is the only machine-readable carrier of which columns blocked the update, and the emulator's differing shape (bare list, no brackets) breaks parsers written against AWS.

Fix: the UpdateConfiguredTable handler's allowedColumns validation (the cleanrooms service crate's ops module for update_configured_table) — where it cross-checks the requested allowedColumns against the configured table's stored analysis rules, emit AWS's message template built from the referenced-but-omitted column names only, and drop the interpolated configured-table id and rule type. The accept/reject decision and the no-mutation-on-reject behavior are already correct and must not change. This is a MESSAGE-wording defect only; the emulator's error code and status are right.

Related but not covering: the fixed issue 20261029T213946Z-cleanrooms-updateconfiguredtable-stub.md made UpdateConfiguredTable answer at all, and its successful legs now pass — this is not a leftover refusal (AWS refuses this call too) but the wording of that correct refusal. The open issues 20261029T175308Z-cleanrooms-configured-table-analysis-rule-reads-diverge-allowedjoinoperators.md (analysis-rule READ projection), 20261029T175308Z-cleanrooms-tagging-responses-answer-200-instead-of-204.md (tagging plane) and 20261029T213946Z-cleanrooms-refusal-fidelity-duplicate-analysis-rule-classified-conflictexception.md (duplicate-rule classification, DeleteMembership, GetPrivacyBudgetTemplate, GetConfiguredTableAssociationAnalysisRule, TagResource not-found code) do not mention UpdateConfiguredTable or this column-reference refusal.

Traces: cleanrooms.update-configured-table-analysis-method-conflict-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator: cleanrooms 8/21 passed (38%), 13 failed. Twelve of those failures are already filed (three unimplemented-stub clusters covering ListMembers, DeleteAnalysisTemplate and CreateConfiguredTableAssociationAnalysisRule, plus five checks OWNED by 20261029T213946Z-cleanrooms-refusal-fidelity-duplicate-analysis-rule-classified-conflictexception.md). The failure below is the only net-new one.

Cause 1 — cleanrooms.update-configured-table-analysis-method-conflict-rejected [mismatch], check line 60: `assert error.get("Message") == (...)` failed with

  AssertionError: {'Message': 'Cannot remove columns from allowedColumns while the LIST analysis rule of configured table 0904e089-5cf1-44f6-8c12-b26c0d27f29a references them: id', 'Code': 'ValidationException'}

Expected Message (HTTP 400, Code ValidationException):
  "The following columns in the analysis rule aren't present in the allowed columns list: [id]. To be used in the analysis rule, columns must be specified as allowed columns."
where "id" is the column the active LIST rule's joinColumns references and the request's allowedColumns omits, and the configured-table uuid in the observed message is a per-run value that AWS does not interpolate at all.

The assertions around it all passed, which localizes the defect to the message text: the preceding `assert error.get("Code") == "ValidationException"` (check line ~57) passed; the positive control UpdateConfiguredTable(allowedColumns=['id','value']) dropping the unreferenced column 'extra' succeeded and GetConfiguredTable read back ['id','value'] (check lines 1314-1317 of the replayed source); i.e. the emulator correctly accepts an unreferenced-column shrink and correctly rejects a referenced-column shrink.

Model evidence (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json): UpdateConfiguredTable — http {method: PATCH, uri: /configuredTables/{configuredTableIdentifier}, code: 200}, input member allowedColumns -> AllowedColumnList @length{min:1}, required output configuredTable, errors AccessDenied(403)|Conflict(409)|InternalServer(500)|ResourceNotFound(404)|ServiceQuotaExceeded(402)|Throttling(429)|Validation(400). ValidationException — structure, errorKind client, httpError 400, members message/reason/fieldList, doc "The input fails to satisfy the specified constraints."; ValidationExceptionReason values FIELD_VALIDATION_FAILED, INVALID_CONFIGURATION, INVALID_QUERY, IAM_SYNCHRONIZATION_DELAY. DeleteConfiguredTableAnalysisRule — DELETE /configuredTables/{configuredTableIdentifier}/analysisRule/{analysisRuleType}, success 204, @idempotent.

## Steps to Reproduce
Boot the emulator and replay the cleanrooms behavioral check cleanrooms.update-configured-table-analysis-method-conflict-rejected (suite simparity-behavioral-aws-v1), or by hand:

1. In glue, CreateDatabase and CreateTable with StorageDescriptor.Columns = [id:string, value:string, extra:string].
2. cleanrooms CreateConfiguredTable(tableReference={glue:{databaseName, tableName}}, allowedColumns=['id','value','extra'], analysisMethod='DIRECT_QUERY'); keep the returned configuredTable.id.
3. CreateConfiguredTableAnalysisRule(configuredTableIdentifier=<id>, analysisRuleType='LIST', analysisRulePolicy={v1:{list:{joinColumns:['id'], listColumns:['value']}}}).
4. Positive control (passes today): UpdateConfiguredTable(configuredTableIdentifier=<id>, allowedColumns=['id','value']) succeeds and GetConfiguredTable reports allowedColumns ['id','value'].
5. The defect: UpdateConfiguredTable(configuredTableIdentifier=<id>, allowedColumns=['value']), which would strand the LIST rule's joinColumns reference to 'id'. Observe HTTP 400 Code 'ValidationException' with Message "Cannot remove columns from allowedColumns while the LIST analysis rule of configured table <uuid> references them: id" instead of AWS's "The following columns in the analysis rule aren't present in the allowed columns list: [id]. To be used in the analysis rule, columns must be specified as allowed columns."
6. Confirm the rest of the contract still holds (it does today, and must after the fix): GetConfiguredTable still reports allowedColumns ['id','value']; then DeleteConfiguredTableAnalysisRule(configuredTableIdentifier=<id>, analysisRuleType='LIST') and re-issue the identical UpdateConfiguredTable(allowedColumns=['value']) — it now succeeds and GetConfiguredTable reports ['value'].
