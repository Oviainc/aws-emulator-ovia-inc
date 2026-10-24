Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.deploy-conformance-pack-duplicate-create-rejected config.put-resource-config-numeric-precision-preserved config.service-linked-recorder-lifecycle config.put-service-linked-recorder-limit-exceeded-blocked config.server-side-third-party-recorder-registers-managed-resource-types config.select-and-batch-get-resource-config config.deliver-config-snapshot config.get-aggregate-resource-config-oversized-item-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.deploy-conformance-pack-duplicate-create-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: config.put-resource-config-numeric-precision-preserved|run#12|AssertionError|raise AssertionError(
Issue-Key: config.service-linked-recorder-lifecycle|run#18|AssertionError|assert describe_message == _ARN_NOT_FOUND_MESSAGE, (
Issue-Key: config.put-service-linked-recorder-limit-exceeded-blocked|run#14|AssertionError|assert redelete_message == expected_not_found, (
Issue-Key: config.server-side-third-party-recorder-registers-managed-resource-types|run#10|AssertionError|assert message == expected_message, (
Issue-Key: config.select-and-batch-get-resource-config|run#4|AssertionError|assert message == "syntax error at line 1, column 19", (
Issue-Key: config.deliver-config-snapshot|run#11|AssertionError|assert message == "Configuration recorder is turned off. Turn on configuration recorder.", message
Issue-Key: config.get-aggregate-resource-config-oversized-item-rejected|run#12|AssertionError|assert message_short == _NOT_DISCOVERED, message_short
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Config accepts a duplicate conformance pack and an unregistered resource type, and its service-linked-recorder / Select / snapshot / aggregate refusals carry the wrong request-derived message (6 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConformancePack

## Description
Six independent contract defects on Config operations that DO answer: two over-acceptances and four refusals whose error code is right but whose message does not follow the rule AWS's own text follows (each of those messages is built from the request — it names the identifier dimension the caller used, the position in the submitted expression, or the API/precondition involved — so the rule, not the recorded sentence, is the requirement). SIX CAUSES must be closed.

Cause 1 of 6: PutConformancePack accepts a second create for a pack that is still CREATE_IN_PROGRESS.
AWS: PutConformancePack models ResourceInUseException (errorKind client, httpError 400) alongside ConformancePackTemplateValidationException, InsufficientPermissionsException, InvalidParameterValueException and MaxNumberOfConformancePacksExceededException (Smithy: PutConformancePack; input ConformancePackName required, pattern ^[a-zA-Z][-a-zA-Z0-9]*$, TemplateBody @length 1..51200). Recorded behavior: while the first pack is still CREATE_IN_PROGRESS, a second PutConformancePack for the same ConformancePackName carrying a DIFFERENT TemplateBody is refused 400 ResourceInUseException, the second template never takes effect, and the original request still completes normally.
Emulator: the duplicate call answers HTTP 200 with a ConformancePackArn (recorded example: arn:aws:config:<region>:<account>:conformance-pack/<name>/conformance-pack-<id>), so there is no state-conflict guard at all.
Why: the in-progress guard is gone, so a client's retry or a concurrent deployer silently replaces the template of a pack that is still being created; the ResourceInUseException branch every SDK/IaC caller has is dead code, and which template finally wins is unobservable.
Fix: the create/update conflict precondition in the PutConformancePack handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_conformance_pack.rs) — reject with ResourceInUseException while the named pack is in a non-terminal create state.
Traces: config.deploy-conformance-pack-duplicate-create-rejected.

Cause 2 of 6: PutResourceConfig accepts a configuration item for a custom ResourceType that is not registered.
AWS: PutResourceConfig requires ResourceType (@length 1..196), SchemaVersionId, ResourceId and Configuration, returns an empty body, and models InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException and ValidationException, all client/400 (Smithy: PutResourceConfig). Recorded behavior: the operation verifies the custom type against the CloudFormation registry using the recorder's own role, and in the recorded account that call short-circuits to 400 InsufficientPermissionsException (the Config role lacks cloudformation:DescribeType) — a 400 client refusal either way, with nothing recorded.
Emulator: the call is accepted (200, empty body) for the fabricated type 'Sim2Real::PrecisionTest::Resource', and the item is recorded.
Why: a client can inject configuration items for resource types that do not exist; the registration precondition AWS enforces is unobservable, and the recorded inventory is polluted with types AWS would refuse.
Fix: the type-registration/permission precondition in the PutResourceConfig handler (.../src/ops/put_resource_config.rs). NOTE for the fixer: this is the same precondition as Cause 1 of the open issue 20261023T140058Z-config-write-paths-unvalidated-or-wrongly-refused-putresourceconfig.md, which does NOT carry this check id; this check additionally pins the refusal to a 400 client error of the InsufficientPermissionsException class observed on the registry lookup.
Traces: config.put-resource-config-numeric-precision-preserved.

Cause 3 of 6: the service-linked-configuration-recorder surface answers every refusal through the customer-recorder NAME path, ignoring which identifier the request actually used.
AWS: DescribeConfigurationRecorders and DeleteServiceLinkedConfigurationRecorder each accept the recorder by a specific dimension (ConfigurationRecorderNames, Arn, or ServicePrincipal) and both model NoSuchConfigurationRecorderException and ValidationException (client/400); PutServiceLinkedConfigurationRecorder models ConflictException, InsufficientPermissionsException, LimitExceededException and ValidationException (Smithy, all client/400). Recorded behavior: the refusal text is built from the dimension the caller supplied — an Arn lookup answers 'The arn of the configuration recorder you entered cannot be found. Verify the configuration recorder arn, and try again. ', a ServicePrincipal delete answers 'Configuration recorder (AccountId: <account>, Service Principal: <principal>) cannot be found', and an unsupported ServicePrincipal answers 'The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again.'
Emulator: the error CODES match (NoSuchConfigurationRecorderException, ValidationException) but the message always describes the NAME dimension — 'The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.' for both the Arn describe and the ServicePrincipal delete — and the unsupported-principal refusal is a different sentence altogether ("The ServicePrincipal 'not-a-real-integration.amazonaws.com' provided in the request is not a valid service principal for a service-linked configuration recorder.").
Why: the message is the only thing that tells a caller WHICH identifier was not resolved; when every dimension reports 'name', a client (or runbook) that passed an ARN or a service principal is told to check a field it never sent, and the three-way distinction AWS makes is lost.
Fix: the not-found / validation message construction for the recorder lookups, keyed on the supplied identifier (.../src/ops/describe_configuration_recorders.rs, .../src/ops/delete_service_linked_configuration_recorder.rs, .../src/ops/put_service_linked_configuration_recorder.rs).
Traces: config.service-linked-recorder-lifecycle, config.put-service-linked-recorder-limit-exceeded-blocked, config.server-side-third-party-recorder-registers-managed-resource-types.

Cause 4 of 6: SelectResourceConfig's InvalidExpressionException message drops the parse position AWS reports.
AWS: SelectResourceConfig models InvalidExpressionException, InvalidLimitException and InvalidNextTokenException (client/400) with Expression @length 1..4096 (Smithy: SelectResourceConfig). Recorded behavior: a malformed expression is refused with a message that reports the exact position in the submitted expression — recorded example 'syntax error at line 1, column 19' — i.e. the message is computed from the request's own text.
Emulator: same code, but the message is the underlying parser's own prose and reports no position: 'The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: ), found: EOF.'
Why: the position report is the actionable part of the refusal — a client (or a console/CLI user) fixing a query needs to know where in the expression the parser stopped; leaking the embedded SQL parser's wording instead also exposes an implementation detail that is not part of Config's contract.
Fix: map parse failures to Config's own message form, carrying the line/column of the failure, in the Select expression parser/validator behind .../src/ops/select_resource_config.rs.
Traces: config.select-and-batch-get-resource-config.

Cause 5 of 6: DeliverConfigSnapshot's no-running-recorder refusal is a paraphrase of AWS's fixed precondition sentence.
AWS: recorded behavior — with a delivery channel present and a recorder that exists but has never been started, DeliverConfigSnapshot is refused with code NoRunningConfigurationRecorderException and the fixed text 'Configuration recorder is turned off. Turn on configuration recorder.'; the rule is that the message names the precondition and the action that clears it, in the same fixed form AWS uses for this exception.
Emulator: the code matches, the text does not: 'there is no configuration recorder running; use StartConfigurationRecorder to turn one on before delivering a configuration snapshot'.
Why: the same paraphrasing already filed for PutResourceConfig (Cause 6 of 20261023T140058Z-config-write-paths-unvalidated-or-wrongly-refused-putresourceconfig.md) is present on this operation too; clients and runbooks that match the fixed sentence do not match the paraphrase.
Fix: the NoRunningConfigurationRecorderException message in the DeliverConfigSnapshot handler (.../src/ops/deliver_config_snapshot.rs), ideally from one shared constant with the other recorder-precondition sites.
Traces: config.deliver-config-snapshot.

Cause 6 of 6: GetAggregateResourceConfig's ResourceNotDiscoveredException echoes the requested identifier instead of AWS's fixed text.
AWS: recorded behavior — for a never-discovered resource (at an ordinary ResourceId length and at the 768-character maximum alike) GetAggregateResourceConfig answers code ResourceNotDiscoveredException with the fixed, identifier-free sentence 'You have specified a resource that is either unknown or has not been discovered.'
Emulator: the code matches, but the message echoes the request: "The resource 'i-<id>' of type 'AWS::EC2::Instance' in account '<account>' and region '<region>' is either unknown or has not been discovered."
Why: AWS deliberately does NOT reflect the queried identifier here; a client that compares the refusal across identifiers (or logs it) sees a per-request string where AWS returns one constant, and the emulator's extra echo invites callers to depend on content AWS never sends.
Fix: the ResourceNotDiscoveredException message in the GetAggregateResourceConfig handler (.../src/ops/get_aggregate_resource_config.rs).
Traces: config.get-aggregate-resource-config-oversized-item-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 45/108 checks passed).

Cause 1 — config.deploy-conformance-pack-duplicate-create-rejected:
  AssertionError: expected the call to be rejected, but it succeeded: {'ConformancePackArn': 'arn:aws:config:us-west-2:000000000000:conformance-pack/sim2real-cpackdup-<id>/conformance-pack-<id>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  (second put_conformance_pack(ConformancePackName=<same name>, TemplateBody=<different template>) while the first create is still in progress; expected 400 ResourceInUseException)

Cause 2 — config.put-resource-config-numeric-precision-preserved:
  AssertionError: PutResourceConfig accepted configuration for unregistered resource type 'Sim2Real::PrecisionTest::Resource' without a matching AWS CloudFormation type registration
  (check's _EXPECTED_CODES = ('InsufficientPermissionsException',), measured against real AWS)

Cause 3:
  config.service-linked-recorder-lifecycle: AssertionError: expected 'The arn of the configuration recorder you entered cannot be found. Verify the configuration recorder arn, and try again. ', got 'The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.' (DescribeConfigurationRecorders(Arn=<deleted service-linked recorder arn>); code NoSuchConfigurationRecorderException matched)
  config.put-service-linked-recorder-limit-exceeded-blocked: AssertionError: expected 'Configuration recorder (AccountId: 000000000000, Service Principal: observabilityadmin.amazonaws.com) cannot be found', got 'The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.' (redundant DeleteServiceLinkedConfigurationRecorder(ServicePrincipal=observabilityadmin.amazonaws.com); the earlier ConflictException code AND message, the single-recorder listing and the first delete all matched)
  config.server-side-third-party-recorder-registers-managed-resource-types: AssertionError: expected the unrecognized-service-principal message 'The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again.', got "The ServicePrincipal 'not-a-real-integration.amazonaws.com' provided in the request is not a valid service principal for a service-linked configuration recorder." (code ValidationException matched, and the no-side-effect assertions were never reached)

Cause 4 — config.select-and-batch-get-resource-config:
  AssertionError: expected the parser's exact position report, got 'The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: ), found: EOF.' (expected 'syntax error at line 1, column 19'; InvalidExpressionException code matched). Because this is the check's FIRST assertion, its BatchGetResourceConfig unprocessed-key assertions never ran.

Cause 5 — config.deliver-config-snapshot:
  AssertionError: there is no configuration recorder running; use StartConfigurationRecorder to turn one on before delivering a configuration snapshot — asserted against 'Configuration recorder is turned off. Turn on configuration recorder.' (code NoRunningConfigurationRecorderException matched)

Cause 6 — config.get-aggregate-resource-config-oversized-item-rejected:
  AssertionError: The resource 'i-968c5559541c4a20a' of type 'AWS::EC2::Instance' in account '000000000000' and region 'us-west-2' is either unknown or has not been discovered. — asserted against 'You have specified a resource that is either unknown or has not been discovered.' (code ResourceNotDiscoveredException matched)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutConformancePack errors ConformancePackTemplateValidationException / InsufficientPermissionsException / InvalidParameterValueException / MaxNumberOfConformancePacksExceededException / ResourceInUseException, all client/400; PutResourceConfig errors InsufficientPermissionsException / MaxActiveResourcesExceededException / NoRunningConfigurationRecorderException / ValidationException, all client/400, output empty; DescribeConfigurationRecorders and DeleteServiceLinkedConfigurationRecorder both take ServicePrincipal and Arn and model NoSuchConfigurationRecorderException + ValidationException; PutServiceLinkedConfigurationRecorder models ConflictException / InsufficientPermissionsException / LimitExceededException / ValidationException; SelectResourceConfig models InvalidExpressionException / InvalidLimitException / InvalidNextTokenException with Expression @length 1..4096.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-ga-prod-v1 and re-replay the check ids in trace_ids.
2. Cause 1: config put-conformance-pack --conformance-pack-name P --template-body <A>, then immediately config put-conformance-pack --conformance-pack-name P --template-body <B> — the second call returns 200 with a ConformancePackArn instead of 400 ResourceInUseException.
3. Cause 2: start a recorder, then config put-resource-config --resource-type 'Sim2Real::PrecisionTest::Resource' --schema-version-id <id> --resource-id r --configuration '{...}' — accepted with 200 instead of a 400 client refusal.
4. Cause 3: config put-service-linked-configuration-recorder --service-principal observabilityadmin.amazonaws.com, delete it by service principal, then (a) config describe-configuration-recorders --arn <that arn> and (b) delete it again by service principal; both messages talk about the recorder NAME. Also config put-service-linked-configuration-recorder --service-principal not-a-real-integration.amazonaws.com and compare the ValidationException message.
5. Cause 4: config select-resource-config --expression 'SELECT resourceId WHERE (' (any malformed expression) — the InvalidExpressionException message reports no line/column and leaks the SQL parser's wording.
6. Cause 5: create a recorder but do not start it, put a delivery channel, then config deliver-config-snapshot --delivery-channel-name <ch> and compare the NoRunningConfigurationRecorderException message.
7. Cause 6: create an aggregator over this account/region, then config get-aggregate-resource-config --configuration-aggregator-name <agg> --resource-identifier '{"SourceAccountId":...,"SourceRegion":...,"ResourceId":"i-<random>","ResourceType":"AWS::EC2::Instance"}' — the ResourceNotDiscoveredException message echoes the identifier.
