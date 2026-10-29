Issue-Source: trace-sweep-divergence
Issue-Trace: glue-catalog-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-catalog-lifecycle glue-catalog-read-and-error-paths glue-classifier-custom-entity-and-data-quality-ruleset glue-column-statistics-task-settings glue-column-statistics-task-settings-error-paths glue-column-statistics-task-settings-lifecycle glue-glue-identity-center-configuration glue-identity-center-configuration-error-paths glue-identity-center-configuration-lifecycle glue-ml-transform-create-role-error glue-ml-transform-error-paths glue-session-error-paths glue-session-statement-error-paths glue-table-optimizer-error-paths glue-table-optimizer-lifecycle glue-table-optimizer-nonexistent-table-vs-ineligible-table
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateCatalog|4xx->2xx|$
Issue-Key: glue|CreateCatalog|4xx->2xx|status
Issue-Key: glue|CreateDataQualityRuleset|4xx->2xx|$.Message
Issue-Key: glue|CreateDataQualityRuleset|4xx->2xx|$.__type
Issue-Key: glue|CreateDataQualityRuleset|4xx->2xx|status
Issue-Key: glue|CreateColumnStatisticsTaskSettings|4xx->2xx|$
Issue-Key: glue|CreateColumnStatisticsTaskSettings|4xx->2xx|status
Issue-Key: glue|CreateGlueIdentityCenterConfiguration|4xx->2xx|$.Message
Issue-Key: glue|CreateGlueIdentityCenterConfiguration|4xx->2xx|$.__type
Issue-Key: glue|CreateGlueIdentityCenterConfiguration|4xx->2xx|status
Issue-Key: glue|DeleteGlueIdentityCenterConfiguration|4xx->2xx|$
Issue-Key: glue|DeleteGlueIdentityCenterConfiguration|4xx->2xx|status
Issue-Key: glue|CreateMLTransform|4xx->2xx|$.Message
Issue-Key: glue|CreateMLTransform|4xx->2xx|$.__type
Issue-Key: glue|CreateMLTransform|4xx->2xx|status
Issue-Key: glue|GetMLTransforms|2xx->2xx|$.Transforms[*]
Issue-Key: glue|CreateSession|4xx->2xx|$.Message
Issue-Key: glue|CreateSession|4xx->2xx|$.__type
Issue-Key: glue|CreateSession|4xx->2xx|status
Issue-Key: glue|CreateTableOptimizer|4xx->2xx|$
Issue-Key: glue|CreateTableOptimizer|4xx->2xx|status
Issue-Key: glue|DeleteTableOptimizer|4xx->2xx|$
Issue-Key: glue|DeleteTableOptimizer|4xx->2xx|status
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue accepts requests AWS refuses: ten create/delete paths answer 200 instead of 400

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCatalog

## Description
This issue bundles 10 net-new causes with one shared fault: the emulator performs no admission check on these inputs, so a request real Glue refuses with HTTP 400 is accepted with HTTP 200 (usually an empty body) and a resource is created that should not exist. Severity HIGH: the happy path is wrong and the negative path is unobservable.

Cause 1 of 10: CreateCatalog for a native (non-federated) catalog.
AWS: Smithy: CreateCatalog errors include InvalidInputException (client, httpError 400). Creating a plain native catalog is refused: HTTP 400, __type InvalidInputException, Message the fixed text 'Create glue native catalog is not supported.'
Emulator: HTTP 200 with an empty body; the catalog is created.
Why: a client that relies on native catalogs being rejected (and on the account-root catalog being the only native one) proceeds against a catalog AWS would never have.
Fix: CreateCatalog input validation (reject a CatalogInput with no federation configuration).
Traces: glue-catalog-lifecycle#task#3 (also glue-catalog-read-and-error-paths#task#1). Related open issue '20261027T124611Z-glue-3-net-new-root-causes-createcatalog-accepts.md' describes the same area but does not carry this fixture step.

Cause 2 of 10: CreateDataQualityRuleset with a Ruleset that does not parse.
AWS: Smithy: CreateDataQualityRuleset error InvalidInputException (client, 400); Ruleset is a required DataQualityRulesetString. A ruleset whose DQDL does not parse is refused: HTTP 400, __type InvalidInputException, Message the fixed text 'DataQuality rules cannot be parsed'.
Emulator: HTTP 200; the unparseable ruleset is stored.
Why: invalid DQDL is accepted and only fails much later (or never), so the client never learns its rules are malformed.
Fix: CreateDataQualityRuleset validation (parse the Ruleset before storing).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#12.

Cause 3 of 10: CreateColumnStatisticsTaskSettings with a role Glue cannot assume.
AWS: Smithy: InvalidInputException (client, 400). HTTP 400, __type InvalidInputException, Message the fixed text "Service is unable to assume provided role. Please verify role's TrustPolicy".
Emulator: HTTP 200 with an empty body.
Why: a client validating its trust policy through this call believes a broken role works.
Fix: CreateColumnStatisticsTaskSettings validation (role trust check before accepting).
Traces: glue-column-statistics-task-settings#task#4 (also glue-column-statistics-task-settings-error-paths#task#4, glue-column-statistics-task-settings-lifecycle#task#4).

Cause 4 of 10: CreateGlueIdentityCenterConfiguration with an IdentityCenter instance ARN that does not exist.
AWS: Smithy: InvalidInputException (client, 400). HTTP 400, Message the fixed text 'Provided identityCenterInstanceArn is invalid. Please check IdentityCenter instance Arn in your account.'
Emulator: HTTP 200; the configuration is created.
Why: the misconfiguration is never surfaced.
Fix: CreateGlueIdentityCenterConfiguration validation of identityCenterInstanceArn.
Traces: glue-glue-identity-center-configuration#task#2 (also glue-identity-center-configuration-error-paths#task#4, glue-identity-center-configuration-lifecycle#task#2).

Cause 5 of 10: DeleteGlueIdentityCenterConfiguration when no configuration exists.
AWS: Smithy: EntityNotFoundException (client, 400). HTTP 400, __type EntityNotFoundException, Message the fixed text 'Idc application not found. Need to create glue application first.'
Emulator: HTTP 200 with an empty body.
Why: a delete of something that never existed reports success, so cleanup logic cannot distinguish states.
Fix: DeleteGlueIdentityCenterConfiguration handler's existence check.
Traces: glue-glue-identity-center-configuration#task#3.

Cause 6 of 10: CreateMLTransform with a role Glue cannot assume.
AWS: Smithy: CreateMLTransform error InvalidInputException (client, 400). HTTP 400, Message template 'Unable to assume role for arn: <roleArn>' with the Role from the request.
Emulator: HTTP 200; the transform is created.
Why: the client gets a transform that can never run.
Fix: CreateMLTransform validation (assume-role check).
Traces: glue-ml-transform-create-role-error#task#3 (also glue-ml-transform-error-paths#task#3).

Cause 7 of 10: GetMLTransforms lists the transform that should never have been created.
AWS: after the refusal in cause 6, GetMLTransforms returns no Transforms entry for that name.
Emulator: returns one extra Transforms[0] entry (the wrongly-created transform, Status NOT_READY).
Why: listings disagree with AWS and a cleanup loop deletes a resource the client never owns.
Fix: consequence of cause 6 — once CreateMLTransform validates, the listing matches; the GetMLTransforms assertion must be re-replayed separately.
Traces: glue-ml-transform-error-paths#task#5.

Cause 8 of 10: CreateSession with a role lacking the Glue trust relationship.
AWS: Smithy: CreateSession error InvalidInputException (client, 400). HTTP 400, Message template 'Role <roleArn> should be given assume role permissions for Glue Service.' followed by AWS's trailing service-diagnostic clause '(Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <requestId>; Proxy: null)' — the role ARN comes from the request and the request id is per-call.
Emulator: HTTP 200; the session is created.
Why: an interactive-session client believes a role that cannot be assumed is usable.
Fix: CreateSession validation (role trust check) plus the InvalidInputException message assembly.
Traces: glue-session-error-paths#task#1 (also glue-session-statement-error-paths#task#1).

Cause 9 of 10: CreateTableOptimizer against a table with no Iceberg metadata location.
AWS: Smithy: CreateTableOptimizer error InvalidInputException (client, 400). HTTP 400, Message the fixed text 'Glue table metadata filepath is null' plus AWS's trailing '(Service: glue; Status Code: 400; Error Code: InvalidInputException; Request ID: <requestId>; Proxy: null)' clause.
Emulator: HTTP 200 with an empty body; an optimizer is attached to an ineligible table.
Why: compaction is configured on a table that can never be optimized, and the client never sees the eligibility error.
Fix: CreateTableOptimizer validation (require the table's Iceberg metadata location).
Traces: glue-table-optimizer-error-paths#task#4 (also glue-table-optimizer-lifecycle#task#4, glue-table-optimizer-nonexistent-table-vs-ineligible-table#task#5).

Cause 10 of 10: DeleteTableOptimizer on an unsupported table type.
AWS: Smithy: DeleteTableOptimizer error InvalidInputException (client, 400). HTTP 400, Message the fixed text 'Request Validation Failed: Unsupported Table type' plus AWS's trailing service-diagnostic clause.
Emulator: HTTP 200 with an empty body.
Why: as above — the unsupported-table case is invisible.
Fix: DeleteTableOptimizer validation (table-type eligibility check).
Traces: glue-table-optimizer-error-paths#task#5 (also glue-table-optimizer-lifecycle#task#5).

## Evidence
Replay of simparity-wire-aws-v1 — every cause is 'status: expected 400, got 200':
1. CreateCatalog: expected {'__type': 'InvalidInputException', 'Message': 'Create glue native catalog is not supported.'}, got {}.
2. CreateDataQualityRuleset: expected InvalidInputException / 'DataQuality rules cannot be parsed', got 200 with no error members.
3. CreateColumnStatisticsTaskSettings: expected InvalidInputException / "Service is unable to assume provided role. Please verify role's TrustPolicy", got {}.
4. CreateGlueIdentityCenterConfiguration: expected InvalidInputException / 'Provided identityCenterInstanceArn is invalid. Please check IdentityCenter instance Arn in your account.', got 200.
5. DeleteGlueIdentityCenterConfiguration: expected EntityNotFoundException / 'Idc application not found. Need to create glue application first.', got {}.
6. CreateMLTransform: expected InvalidInputException / 'Unable to assume role for arn: arn:aws:iam::000000000000:role/sim2real-glue-unassumed-mltransform-role', got 200.
7. GetMLTransforms 200/200: $.Transforms[0] extra (expected None, got a transform with Status 'NOT_READY').
8. CreateSession: expected InvalidInputException / 'Role arn:aws:iam::000000000000:role/... should be given assume role permissions for Glue Service.\n (Service: AWSGlueJobExecutor; Status Code: 400; ...)', got 200.
9. CreateTableOptimizer: expected InvalidInputException / 'Glue table metadata filepath is null (Service: glue; Status Code: 400; ...)', got {}.
10. DeleteTableOptimizer: expected InvalidInputException / 'Request Validation Failed: Unsupported Table type (Service: glue; Status Code: 400; ...)', got {}.
(Role ARNs and request ids above are that run's values; per-call request ids are not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-catalog-lifecycle#task#3, glue-classifier-custom-entity-and-data-quality-ruleset#task#12, glue-column-statistics-task-settings#task#4, glue-glue-identity-center-configuration#task#2, glue-glue-identity-center-configuration#task#3, glue-ml-transform-create-role-error#task#3, glue-ml-transform-error-paths#task#5, glue-session-error-paths#task#1, glue-table-optimizer-error-paths#task#4, glue-table-optimizer-error-paths#task#5.
3. Each step submits the invalid input described in its cause and asserts a 400 refusal body; the emulator currently answers 200.
