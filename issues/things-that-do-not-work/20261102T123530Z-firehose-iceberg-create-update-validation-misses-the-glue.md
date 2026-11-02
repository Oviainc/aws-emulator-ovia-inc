Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-kfi-traj-firehose-role-mistakes
Issue-Step: task#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-kfi-traj-firehose-role-mistakes firehose-kfi-traj-iceberg-config-required-members firehose-kfi-traj-iceberg-option-mistakes firehose-kfi-traj-kinesis-source-mistakes firehose-kfi-traj-stream-before-glue-table firehose-kfi-traj-fix-table-via-update-destination
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|$.__type
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|$.message
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|status
Issue-Key: firehose|CreateDeliveryStream|4xx->4xx|$.__type
Issue-Key: firehose|CreateDeliveryStream|4xx->4xx|$.message
Issue-Key: firehose|UpdateDestination|2xx->4xx|status
Issue-Key: firehose|UpdateDestination|4xx->4xx|$.message
Issue-Commit: 81fc6333f2dd5c26c146222a43c6f73baa469449

# firehose: Iceberg create/update validation misses the Glue table check and misstates three refusals (5 causes)

> _Note: this issue was identified against the code base at commit `81fc6333f2dd5c26c146222a43c6f73baa469449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Five net-new wire causes in the validation and refusal wording of the Iceberg destination on CreateDeliveryStream and UpdateDestination. All five are distinct replay keys and each must be re-replayed before this issue is closed. Replay reported owned_by empty for all five. Cause 1 is the same root cause as cause 1 of the open BEHAVIORAL issue 20261028T122732Z-firehose-glue-kinesis-s3-the-apache-iceberg-path.md, but that issue carries no wire replay key for these fixture steps, so these steps are not covered by it; causes 2-5 appear in no open issue.

Cause 1 of 5: CreateDeliveryStream accepts an Iceberg destination whose Glue table is not resolvable, instead of refusing it.
AWS: Smithy: CreateDeliveryStream takes IcebergDestinationConfiguration{CatalogConfiguration (required), DestinationTableConfigurationList of {DestinationDatabaseName, DestinationTableName}, RoleARN (required), S3Configuration (required)} and models InvalidArgumentException (client, HTTP 400, 'The specified input parameter has a value that is not valid'). The recording shows AWS refusing the create with HTTP 400, __type 'InvalidArgumentException' and the message 'Role <RoleARN> is not authorized to perform: glue:GetTable for the given table or the table does not exist.' — the sentence is fixed text and the ONLY variable part is the RoleARN taken verbatim from the request's IcebergDestinationConfiguration.RoleARN. AWS performs the glue:GetTable with that role at create time and deliberately conflates 'not permitted' with 'does not exist', so a role without glue:GetTable, a missing table and a missing database all produce this one message.
Emulator: HTTP 200 — the create succeeds and a stream is minted. No $.__type and no $.message are produced. The create handler resolves roles against IAM and buckets against S3 (src/ops/create_delivery_stream.rs) but performs no Glue catalog resolution for Kind::Iceberg at all.
Why: the caller loses the only create-time signal that its catalog target or its role is wrong; the stream goes ACTIVE and then delivers nowhere. It also corrupts subsequent state — see cause 2.
Fix: the CreateDeliveryStream handler's Iceberg destination validation (src/ops/create_delivery_stream.rs, the Kind::Iceberg arm of sole_destination()/the validation that runs before the stream is registered) must resolve each DestinationTableConfigurationList entry against the Glue catalog using the configuration's RoleARN and raise InvalidArgumentException with AWS's sentence; a refused create must leave no stream behind.
Traces: firehose-kfi-traj-firehose-role-mistakes#task#16 (representative; cluster also covers firehose-kfi-traj-iceberg-config-required-members#task#11, firehose-kfi-traj-iceberg-option-mistakes#task#8 and #task#12, firehose-kfi-traj-kinesis-source-mistakes#task#9, firehose-kfi-traj-stream-before-glue-table#task#6).

Cause 2 of 5: a repeated create that AWS refuses with InvalidArgumentException is refused with ResourceInUseException instead, because the first (wrongly accepted) create left a stream behind.
AWS: as cause 1 — while the Glue table does not exist, EVERY create of that stream is refused with __type 'InvalidArgumentException' and the glue:GetTable sentence, and no stream of that name exists, so the name-collision refusal is never reached.
Emulator: HTTP 400 with __type 'ResourceInUseException' and the message 'Firehose <deliveryStreamName> under accountId <accountId> already exists' (fixed text around the stream name from the request and the account id from the request context), emitted by the name-uniqueness check in src/ops/create_delivery_stream.rs.
Why: the error identity is wrong, so a client that retries on ResourceInUseException (a name collision it can resolve by picking another name) retries forever, while the real fault — an unresolvable Glue table — is never reported. Validation ordering matters here: the destination's catalog must be resolved before the create can be charged against the name.
Fix: same site as cause 1; once the Glue check refuses the first create, no stream is registered and the second create reports the same InvalidArgumentException.
Traces: firehose-kfi-traj-stream-before-glue-table#task#9.

Cause 3 of 5: the missing-required-member validation message inserts a 'null' token AWS does not emit.
AWS: Smithy: IcebergDestinationConfiguration$CatalogConfiguration, $RoleARN and $S3Configuration are @required. The recording shows AWS refusing an absent one with HTTP 400 and the message "1 validation error detected: Value at 'icebergDestinationConfiguration.catalogConfiguration' failed to satisfy constraint: Member must not be null". The fixed text is '<N> validation error(s) detected: Value at '<memberPath>' failed to satisfy constraint: Member must not be null'; the variable parts are the error count and the lowerCamelCase member path from the request. For an ABSENT member there is no value to quote, so no value token appears between 'Value' and 'at'.
Emulator: HTTP 400 with the same status and error identity but the message "1 validation error detected: Value null at 'icebergDestinationConfiguration.catalogConfiguration' failed to satisfy constraint: Member must not be null" — it renders the absent value as the literal 'null' in the slot AWS leaves empty.
Why: a client or test that matches AWS's validation sentence (a very common assertion, since the text is stable and documented by the framework) does not match, and the message is not the one AWS words for this error class. The same template covers several member paths, so the defect is in the renderer, not one message.
Fix: the shared constraint-violation message renderer used by the create path's required-member validation (the validation layer of src/ops/create_delivery_stream.rs / the service's validation message helpers in src/service.rs) must omit the value token entirely when the member is absent, and keep it only where AWS quotes a supplied value.
Traces: firehose-kfi-traj-iceberg-config-required-members#task#8 (representative; cluster also covers firehose-kfi-traj-iceberg-config-required-members#task#9 and #task#10 and firehose-kfi-traj-kinesis-source-mistakes#task#10, i.e. the other required member paths).

Cause 4 of 5: UpdateDestination with an IcebergDestinationUpdate on an Iceberg destination is refused where AWS accepts it.
AWS: Smithy: UpdateDestination takes DeliveryStreamName, CurrentDeliveryStreamVersionId, DestinationId and one *DestinationUpdate member — here IcebergDestinationUpdate (every member optional, so a partial update that carries only DestinationTableConfigurationList is well formed) — and returns an EMPTY output with HTTP 200; its modeled errors are ConcurrentModificationException, InvalidArgumentException, ResourceInUseException and ResourceNotFoundException (all 400). The recording shows HTTP 200 for the update that repoints an Iceberg destination at the now-existing Glue table.
Emulator: HTTP 400.
Why: the documented way to repair a destination ('Use this operation to change the destination type') is unavailable for Iceberg, so a client cannot fix a stream in place and the version id never advances.
Fix: the UpdateDestination handler (src/ops/update_destination.rs, the Kind::Iceberg arm of sole_update()/applied()) must merge an IcebergDestinationUpdate into a stored Iceberg configuration and answer 200 with the empty output, bumping VersionId.
Traces: firehose-kfi-traj-fix-table-via-update-destination#task#13.

Cause 5 of 5: a destination-type switch to or from Iceberg is refused with the wrong message.
AWS: Smithy: UpdateDestination models InvalidArgumentException (400). The recording shows AWS refusing a switch that moves a destination between Iceberg and another type with HTTP 400 and the message 'Changing the destination type to or from Iceberg is not supported at this time.' — wholly fixed text with no variable part, naming the boundary that may not be crossed rather than any parameter.
Emulator: HTTP 400 (correct status, correct InvalidArgumentException identity) with the message 'ExtendedS3DestinationUpdate.RoleARN must be specified when the destination type changes, because no parameter is merged in that case' — the generic type-change merge rule, because the handler's fixed-type table (src/ops/update_destination.rs, enum FixedType and stored_fixed_type()/DestinationUpdateKind::fixed_type(), lines ~526-580) lists only the search-domain, search-serverless and HTTP-endpoint boundaries and omits Iceberg entirely.
Why: the refusal tells the client to supply a RoleARN, which is an invitation to retry a request AWS will never accept; the real constraint — Iceberg destinations cannot be switched into or out of — is never communicated.
Fix: add Iceberg to the FixedType boundary set in src/ops/update_destination.rs and give it AWS's sentence, so the refusal is raised before the type-change merge rule runs.
Traces: firehose-kfi-traj-fix-table-via-update-destination#task#17.

## Evidence
Cause 1 — fixture firehose-kfi-traj-firehose-role-mistakes step task#16 (CreateDeliveryStream):
  expected: HTTP 400, $.__type 'InvalidArgumentException', $.message 'Role <RoleARN from the request> is not authorized to perform: glue:GetTable for the given table or the table does not exist.'
  actual:   HTTP 200, $.__type missing, $.message missing (the stream is created)
  (affected_count 6)

Cause 2 — fixture firehose-kfi-traj-stream-before-glue-table step task#9 (CreateDeliveryStream, HTTP 400 on both sides):
  $.__type:  expected 'InvalidArgumentException',  actual 'ResourceInUseException'
  $.message: expected 'Role <RoleARN> is not authorized to perform: glue:GetTable for the given table or the table does not exist.'
             actual   'Firehose <deliveryStreamName> under accountId <accountId> already exists'

Cause 3 — fixture firehose-kfi-traj-iceberg-config-required-members step task#8 (CreateDeliveryStream, HTTP 400 on both sides):
  $.message: expected "1 validation error detected: Value at 'icebergDestinationConfiguration.catalogConfiguration' failed to satisfy constraint: Member must not be null"
             actual   "1 validation error detected: Value null at 'icebergDestinationConfiguration.catalogConfiguration' failed to satisfy constraint: Member must not be null"
  (affected_count 4 — same template, other member paths)

Cause 4 — fixture firehose-kfi-traj-fix-table-via-update-destination step task#13 (UpdateDestination):
  expected: HTTP 200, empty body
  actual:   HTTP 400

Cause 5 — fixture firehose-kfi-traj-fix-table-via-update-destination step task#17 (UpdateDestination, HTTP 400 on both sides):
  $.message: expected 'Changing the destination type to or from Iceberg is not supported at this time.'
             actual   'ExtendedS3DestinationUpdate.RoleARN must be specified when the destination type changes, because no parameter is merged in that case'

## Steps to Reproduce
1. Cause 1 — replay firehose-kfi-traj-firehose-role-mistakes step task#16: CreateDeliveryStream with an IcebergDestinationConfiguration whose DestinationTableConfigurationList names a Glue database/table the role cannot read (or that does not exist). It must be refused 400 InvalidArgumentException with the glue:GetTable sentence naming the request's RoleARN, and no stream may be left behind (ListDeliveryStreams must not show it).
2. Cause 2 — replay firehose-kfi-traj-stream-before-glue-table steps task#6 then task#9: create the same Iceberg stream twice while the Glue table still does not exist. BOTH creates must report InvalidArgumentException with the glue:GetTable sentence; neither may report ResourceInUseException.
3. Cause 3 — replay firehose-kfi-traj-iceberg-config-required-members step task#8: CreateDeliveryStream with IcebergDestinationConfiguration omitting CatalogConfiguration. The 400 message must read "1 validation error detected: Value at 'icebergDestinationConfiguration.catalogConfiguration' failed to satisfy constraint: Member must not be null" — with no 'null' token after 'Value'. Repeat for the other required members (steps task#9, task#10).
4. Cause 4 — replay firehose-kfi-traj-fix-table-via-update-destination step task#13: with an ACTIVE Iceberg-destination stream, create the Glue table and call UpdateDestination(CurrentDeliveryStreamVersionId=<current>, DestinationId=destinationId-000000000001, IcebergDestinationUpdate={...}). It must answer 200 with an empty body and bump VersionId.
5. Cause 5 — replay the same fixture step task#17: call UpdateDestination on that Iceberg destination with an ExtendedS3DestinationUpdate (a type switch out of Iceberg). It must be refused 400 InvalidArgumentException with exactly 'Changing the destination type to or from Iceberg is not supported at this time.'
