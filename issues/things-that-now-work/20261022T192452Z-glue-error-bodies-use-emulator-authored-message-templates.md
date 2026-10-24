Issue-Source: trace-sweep-divergence
Issue-Trace: glue-crawler-error-paths-and-listing
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-crawler-error-paths-and-listing glue-crawler-invalid-targets-error glue-crawler-lifecycle glue-crawler-notfound-and-listing glue-database-table-partition-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetCrawler|4xx->4xx|$.Message
Issue-Key: glue|CreateDatabase|4xx->4xx|$.Message
Issue-Key: glue|GetTable|4xx->4xx|$.Message
Issue-Key: glue|DeleteTable|4xx->4xx|$.Message
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Glue error bodies use emulator-authored message templates instead of AWS wording

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCrawler

## Description
Four wire-fixture clusters where the HTTP status and the error code are correct but the Message member carries an emulator-authored template rather than the one AWS produces. These are message-shape defects only - no status or error code diverges - so they are LOW, but each is a template the recording pins down (two are built from the request, two are AWS's fixed constants) and each must be closed before this issue is done.

Cause 1 of 4: GetCrawler not-found message.
AWS: Smithy com.amazonaws.glue#GetCrawler models EntityNotFoundException (httpError 400) for a crawler that does not exist. The recorded message is built from the request's Name: 'Crawler entry with name <Name> does not exist' - so the rule is the template, not the recorded instance.
Emulator: same 400 and same __type, but Message is 'Crawler <Name> not found'.
Why: a client that matches on AWS's wording to distinguish a missing crawler from another EntityNotFoundException (log scraping, error-message assertions in provisioning code) does not match, and the 'entry with name' phrasing AWS uses only for crawlers is lost.
Fix: use AWS's template in the crawler lookup error in services/service-com-amazonaws-glue-awsglue/src/ops/get_crawler.rs (and whatever shared crawler not-found constructor the listing paths share), interpolating the requested Name.
Traces: glue-crawler-error-paths-and-listing#task#1 (cluster also covers glue-crawler-invalid-targets-error#task#2, glue-crawler-lifecycle#task#2, glue-crawler-notfound-and-listing#task#1).

Cause 2 of 4: CreateDatabase duplicate-name message.
AWS: com.amazonaws.glue#CreateDatabase models AlreadyExistsException (httpError 400) when DatabaseInput.Name already exists. Recorded behavior only for the text: AWS answers the fixed constant 'Database already exists.' and deliberately does not echo the database name.
Emulator: same 400 and code, but Message is 'Database already exists: <name>' - the emulator appends the resource name AWS withholds.
Why: the message is not request-derived in AWS, so a client matching the constant fails, and the emulator discloses the requested name in an error body where AWS does not.
Fix: emit the constant message in the duplicate-database branch of services/service-com-amazonaws-glue-awsglue/src/ops/create_database.rs.
Traces: glue-database-table-partition-lifecycle#task#8.

Cause 3 of 4: GetTable not-found message.
AWS: com.amazonaws.glue#GetTable models EntityNotFoundException (httpError 400). Recorded behavior only for the text: AWS answers the fixed constant 'Entity Not Found' with no table or database name.
Emulator: same 400 and code, Message 'Table <name> not found in database <db>'.
Why: as Cause 2 - the constant is what clients see from AWS for this operation, and the same constant is what the deleted-table read in the companion state issue expects, so the two must agree.
Fix: emit the constant message in the table lookup error of services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs.
Traces: glue-database-table-partition-lifecycle#task#19.

Cause 4 of 4: DeleteTable not-found message.
AWS: com.amazonaws.glue#DeleteTable models EntityNotFoundException (httpError 400). The recorded message is built from the request's Name: 'Table <Name> not found.' - naming the table but not the database, and ending in a period.
Emulator: same 400 and code, Message 'Table <Name> not found in database <DatabaseName>'.
Why: DeleteTable and GetTable use different templates in AWS (Cause 3 vs this one) and the emulator collapses both into one of its own, so a client cannot tell the two refusals apart by message as it can against AWS.
Fix: use the DeleteTable-specific template in services/service-com-amazonaws-glue-awsglue/src/ops/delete_table.rs rather than a shared table-not-found string.
Traces: glue-database-table-partition-lifecycle#teardown#20.

## Evidence
Replay of simparity-wire-aws-v1 (121 fixtures, 898 steps, state_reset_failures 0). In all four clusters expected_status == actual_status == 400 and $.__type matched; only $.Message diverged.

Cause 1 - glue-crawler-error-paths-and-listing#task#1 GetCrawler: expected 'Crawler entry with name sim2real-probe-glue-gluecrawler-missing-49390596 does not exist', actual 'Crawler sim2real-probe-glue-gluecrawler-missing-49390596 not found'. 4 steps affected. (The name is the fixture's input; the requirement is the template around it.)

Cause 2 - glue-database-table-partition-lifecycle#task#8 CreateDatabase: expected 'Database already exists.', actual 'Database already exists: sim2real_probe_glue_sim2real_probe_db_00614834'.

Cause 3 - glue-database-table-partition-lifecycle#task#19 GetTable: expected 'Entity Not Found', actual 'Table sim2real_probe_glue_sim2real_probe_tbl_00614834 not found in database sim2real_probe_glue_sim2real_probe_db_00614834'.

Cause 4 - glue-database-table-partition-lifecycle#teardown#20 DeleteTable: expected 'Table sim2real_probe_glue_sim2real_probe_tbl_00614834 not found.', actual 'Table sim2real_probe_glue_sim2real_probe_tbl_00614834 not found in database sim2real_probe_glue_sim2real_probe_db_00614834'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture glue-crawler-error-paths-and-listing, step task#1 - GetCrawler on a name that does not exist and compare $.Message with 'Crawler entry with name <Name> does not exist'.
3. Cause 2: replay fixture glue-database-table-partition-lifecycle, step task#8 - CreateDatabase twice with the same DatabaseInput.Name and compare $.Message with 'Database already exists.'.
4. Cause 3: same fixture, step task#19 - GetTable on a deleted table and compare $.Message with 'Entity Not Found'.
5. Cause 4: same fixture, step teardown#20 - DeleteTable on an already-deleted table and compare $.Message with 'Table <Name> not found.'.
