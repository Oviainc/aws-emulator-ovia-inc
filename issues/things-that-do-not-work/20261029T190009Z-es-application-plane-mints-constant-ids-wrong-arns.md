Issue-Source: trace-sweep-divergence
Issue-Trace: esdef-application-dead-id-probe
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: esdef-application-dead-id-probe opensearch-application-capability-lifecycle opensearch-switch-default-application-between-two opensearch-application-lifecycle-workflow opensearch-unset-default-application opensearch-application-duplicate-name-conflict opensearch-application-error-paths opensearch-put-default-application-setting-malformed-arn opensearch-application-saved-object-migration opensearch-domain-insights-operations
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateApplication|2xx->2xx|$.arn
Issue-Key: es|CreateApplication|2xx->2xx|$.id
Issue-Key: es|DeleteApplication|2xx->4xx|status
Issue-Key: es|PutDefaultApplicationSetting|2xx->2xx|$
Issue-Key: es|CreateApplication|4xx->4xx|$.message
Issue-Key: es|PutDefaultApplicationSetting|4xx->4xx|$.message
Issue-Key: es|StartMigration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|StartMigration|4xx->4xx|status
Issue-Key: es|DescribeInsightDetails|2xx->2xx|$.Fields[Name="Message"]
Issue-Related: 20261029T190009Z-es-constraint-violation-refusal-messages-are-emulator-composed.md
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: application plane mints constant ids, wrong ARNs, empty bodies and emulator-authored refusals

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
9 net-new root causes in the OpenSearch application plane.

Cause 1 of 9 - CreateApplication mints a constant id. Every CreateApplication returns the same id and the ARN derived from it, so two applications created in one run collide. AWS mints a fresh id per application (matching ^(?!-)[a-z0-9\\-]{3,30}$). The replay shows the identical id handed out across separate creates in separate fixtures. Trace: esdef-application-dead-id-probe#task#1 (the shape assertion on $.id/$.arn fails), confirmed by opensearch-switch-default-application-between-two#setup#1/#setup#2 which create two applications and get one id.

Cause 2 of 9 - the application ARN uses the wrong service token. AWS: arn:aws:opensearch:<region>:<account>:application/<id>. Emulator: arn:aws:es:<region>:<account>:application/<id>. Trace: opensearch-application-capability-lifecycle#setup#1.

Cause 3 of 9 - DeleteApplication of a live application is refused. AWS: 200. Emulator: 409. Trace: esdef-application-dead-id-probe#task#6 (affected also esdef-application-dead-id-probe#teardown#8).

Cause 4 of 9 - PutDefaultApplicationSetting returns an empty body. AWS: 200 with {"applicationArn": "<the ARN that was set>"}. Emulator: 200 with {}. Trace: opensearch-application-lifecycle-workflow#task#4 (affected also opensearch-unset-default-application#task#4).

Cause 5 of 9 - duplicate-name refusal text. AWS: "Application name <the name from the request> already exists". Emulator: 'Application with name <name> already exists.' - different fixed words and a trailing period. Status 409 already matches. Trace: opensearch-application-duplicate-name-conflict#task#2.

Cause 6 of 9 - PutDefaultApplicationSetting malformed-ARN refusal is hand-written. AWS emits the standard constraint report: "1 validation error detected: Value '<the value>' at 'applicationArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\\-]*:[a-z0-9-]+:[a-z0-9-]*:[0-9]+:[a-z0-9-]*/(.+)". Emulator: 'applicationArn is not a valid ARN: <value>'. Trace: opensearch-application-error-paths#task#1.

Cause 7 of 9 - StartMigration resolves the application before validating migrationOptions. AWS: 400 ValidationException with the constraint report for migrationOptions.workspace.type (enum value set [ALL, OBSERVABILITY, ESSENTIALS, SECURITY_ANALYTICS, SEARCH]) and migrationOptions.source.datasourceArn (aoss collection ARN pattern). Emulator: 409 ResourceNotFoundException 'Application does not exist'. Trace: opensearch-application-saved-object-migration#task#1.

Cause 8 of 9 - StartMigration's not-found refusal. AWS: 404 with "Application <the applicationId from the request> does not exist" - the id is interpolated. Emulator: 409 with the id stripped out ('Application does not exist'). Trace: opensearch-application-saved-object-migration#task#2.

Cause 9 of 9 - DescribeInsightDetails omits a modeled field from its 200. AWS includes a Fields entry {Name: 'Message', Type: 'text', Value: 'Please use the Amazon OpenSearch Console to view the Insight Details for a better user experience.'}; the emulator omits it. Trace: opensearch-domain-insights-operations#task#3.

Why: cause 1 is the most damaging - a client that creates two applications gets one identity, so the second create silently aliases the first and every subsequent read, update and delete targets the wrong object. Cause 2 makes every ARN the emulator mints unusable in an IAM policy or a cross-service reference written against AWS. Cause 3 makes application teardown impossible. Cause 4 breaks a client that reads applicationArn back from the call it just made. Causes 5-8 break clients and tests matching the documented refusal wording and, for 7, the error class.

Fix: the application handlers and state - id minting (unique per create), the ARN builder's service token, DeleteApplication's state transition, PutDefaultApplicationSetting's response serializer, and the refusal text/class in CreateApplication, PutDefaultApplicationSetting and StartMigration; plus DescribeInsightDetails' field projection.

## Evidence
Cause 1: "$.id: value_mismatch (expected '<shaped-not-asserted>', got 'sfi9sv1baaaa')" and the same 'sfi9sv1baaaa' reappears for unrelated creates in opensearch-application-capability-lifecycle#setup#1 and opensearch-switch-default-application-between-two#setup#1/#setup#2.
Cause 2: expected 'arn:aws:opensearch:us-west-2:<account>:application/<id>'; actual 'arn:aws:es:us-west-2:<account>:application/<id>'.
Cause 3: "status: expected 200, got 409".
Cause 4: "$: empty_body (expected {'applicationArn': '<arn>'}, got {})".
Cause 5: expected 'Application name <name> already exists'; actual 'Application with name <name> already exists.'.
Cause 6: expected the 1-clause constraint report on 'applicationArn'; actual 'applicationArn is not a valid ARN: <value>'.
Cause 7: expected 400 + ValidationException + the 2-clause report; actual 409 + ResourceNotFoundException + 'Application does not exist'.
Cause 8: expected 404 + 'Application <id> does not exist'; actual 409 + 'Application does not exist'.
Cause 9: "$.Fields[Name=\"Message\"]: missing (expected {'Name': 'Message', 'Type': 'text', 'Value': 'Please use the Amazon OpenSearch Console to view the Insight Details for a better user experience.'}, got None)".

## Steps to Reproduce
1. Replay esdef-application-dead-id-probe step task#1, then opensearch-switch-default-application-between-two steps setup#1 and setup#2: all three creates return the same id.
2. Replay opensearch-application-capability-lifecycle step setup#1 and check the ARN's service token; replay esdef-application-dead-id-probe step task#6 for the DeleteApplication 409 and opensearch-application-lifecycle-workflow step task#4 for the empty PutDefaultApplicationSetting body.
3. Replay opensearch-application-duplicate-name-conflict step task#2, opensearch-application-error-paths step task#1, opensearch-application-saved-object-migration steps task#1/#task#2 and opensearch-domain-insights-operations step task#3.
