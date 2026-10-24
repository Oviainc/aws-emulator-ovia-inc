Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
Issue-Key: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied|run#31|AssertionError|assert message == "Entity Not Found", (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Glue GetTable not-found error returns an emulator-authored message instead of AWS's fixed 'Entity Not Found' constant

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
One cause.

Attribution note: the failing check's id leads with the four-service combo token `athena__emr-serverless__glue__s3`, which matches no single bootstrap token. The diverging response here is produced by the glue API (GetTable), so the issue is filed against `glue`, which is stood up and answering.

Related open issue: `20261022T192452Z-glue-error-bodies-use-emulator-authored-message-templates.md` describes this same GetTable message template as its Cause 3, grounded in the WIRE corpus (replay key `glue|GetTable|4xx->4xx|$.Message`, fixture glue-database-table-partition-lifecycle#task#19). The behavioral replay was not marked OWNED by it, so it is filed here with the behavioral check id that reproduces it; a fixer should expect to close both with one change.

Cause 1 of 1: GetTable for a table that does not exist returns the right status and error code but the wrong message body.

AWS: Smithy com.amazonaws.glue#GetTable models EntityNotFoundException with httpError 400 for a table that does not exist (also FederationSourceException, GlueEncryptionException, InvalidInputException, OperationTimeoutException, ResourceNotReadyException, InternalServiceException 500). The model does not pin the message text; recorded behavior only for that: AWS answers the fixed constant message 'Entity Not Found', naming neither the table nor the database. The rule is therefore a constant, not a template interpolated from the request — which is what makes a request-derived message a divergence rather than an example. This is corroborated by the independent wire recording cited in the related issue above.
Emulator: same HTTP 400 and the same error code EntityNotFoundException, but the message is built from the request — e.g. 'Table sim2real_facts_c586e4884d78 not found in database sim2real_deny_db_3fd80938d8ed'.
Why: a client that distinguishes a missing table from the other EntityNotFoundException sources on this operation by matching AWS's constant (provisioning code, log scraping, Hive/Spark metastore client error handling, test assertions) does not match. It also leaks into the emulator's wider behavior: AWS uses different texts per operation here (GetTable's constant vs DeleteTable's request-derived 'Table <Name> not found.'), and collapsing them into one shared template means the two refusals cannot be told apart by message as they can against AWS. Secondarily, the emulator discloses the requested table and database names in an error body where AWS withholds them.
Fix: the table-lookup not-found branch of the glue GetTable handler (the shared table-not-found error constructor it uses) must emit the constant message rather than interpolating DatabaseName/Name; see the file citation in the related issue above for where the wire sweep located it.
Traces: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied (assertion `message == "Entity Not Found"` after the asserted-absent catalog entry).

## Evidence
Replay of simparity-behavioral-ga-prod-v1: 34/36 passed, 2 mismatched.

[mismatch] athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
check_athena__emr_serverless__glue__s3_spark_saveastable_without_glue_createtable_permission_is_denied.py line 424:
  assert message == "Entity Not Found"
  AssertionError: glue.get_table for the denied table said 'Table sim2real_facts_c586e4884d78 not found in database sim2real_deny_db_3fd80938d8ed', expected 'Entity Not Found'

Expected (AWS): GetTable on an absent table -> HTTP 400, Error.Code 'EntityNotFoundException', Error.Message the fixed constant 'Entity Not Found'.
Actual (emulator): HTTP 400 and Error.Code 'EntityNotFoundException' both match; only Error.Message diverges, carrying a template interpolating the requested table and database names. The quoted names are this run's generated values and are examples; the requirement is that the message is the constant and is not request-derived.

Corroborating wire evidence (from the related open issue): fixture glue-database-table-partition-lifecycle#task#19, expected 'Entity Not Found', actual 'Table <tbl> not found in database <db>'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite.
2. Run check athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied, or by hand:
   a. glue CreateDatabase with a fresh DatabaseInput.Name.
   b. glue GetTable with that DatabaseName and a Name that was never created.
3. Inspect the error response: assert the status is 400 and Error.Code is 'EntityNotFoundException' (both already correct), then compare Error.Message with the constant 'Entity Not Found'.
4. Observe the emulator returns a message interpolating the requested Name and DatabaseName instead of the constant.
