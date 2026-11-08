Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-parameters-pagination
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-parameters-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Marker
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].IsModifiable
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].MinimumEngineVersion
Issue-Key: rds|DescribeDBParameters|2xx->4xx|$.DescribeDBParametersResponse
Issue-Key: rds|DescribeDBParameters|2xx->4xx|status
Issue-Commit: 263537c8d0bbf6adc085b4147c3053be10a9244f

# rds parameter pagination markers are raw parameter names, so recorded continuation tokens are rejected with 400

> _Note: this issue was identified against the code base at commit `263537c8d0bbf6adc085b4147c3053be10a9244f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBParameters

## Description
RDS pagination markers are opaque tokens; the emulator mints a marker that is simply the next parameter's name, and its marker parser then refuses the token the service actually issues. There are 3 causes to close.

Cause 1 of 3: DescribeDBParameters emits a plaintext parameter name where AWS emits an opaque (base64) marker.
AWS: Smithy: DescribeDBParameters is @paginated with inputToken=Marker, outputToken=Marker, items=Parameters, pageSize=MaxRecords; Marker is documented as 'an optional pagination token provided by a previous request'. Recorded behavior: the Marker is an opaque, base64-looking blob (the recording shows a token that decodes to the next parameter's name but is wire-opaque, e.g. 'aW5ub2RiX2NvbXByZXNzaW9uX2ZhaWx1cmVfdGhyZXNob2xkX3BjdA=='); clients must pass it back verbatim and must not parse it. The page itself carries full Parameter entries (AllowedValues, ApplyType, DataType, Description, IsModifiable).
Emulator: returns Marker='innodb_compression_level' - the bare parameter name, not the encoded token - and the page's Parameter entries omit AllowedValues, ApplyType, DataType, Description and IsModifiable on every element.
Why: a client that stores or logs the marker, or that round-trips it through a system expecting the recorded token form, diverges from AWS; more importantly the emulator's token is not interchangeable with the service's, so a recorded or externally supplied marker cannot be resumed (see Cause 3).
Fix: the marker encode/decode in the DescribeDBParameters handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_parameters.rs) - emit the opaque encoded form and accept it on input - plus the Parameter projection that drops the descriptive members.
Traces: rds-db-parameters-pagination#task#4.

Cause 2 of 3: the continued page is served from the placeholder catalog and omits the same Parameter members.
AWS: the second page continues the engine's real default parameter set in name order; the recorded page begins with innodb_compression_failure_threshold_pct (integer, AllowedValues 0-9), innodb_compression_pad_pct_max (0-75), innodb_compression_level (10-100), each with ApplyType, DataType, Description and IsModifiable present.
Emulator: answers 200 but every returned Parameter lacks AllowedValues, ApplyType, DataType, Description and IsModifiable, and the entries are not the service's.
Why: paging through the parameter list to build a complete picture of a group yields an incomplete, undescribed list, so validation of a subsequent ModifyDBParameterGroup has no AllowedValues/IsModifiable to check against.
Fix: same handler's page projection, over the corrected parameter catalog (see the companion catalog issue for the seed itself).
Traces: rds-db-parameters-pagination#task#5.

Cause 3 of 3: resuming with the recorded Marker is refused with HTTP 400 where AWS answers 200.
AWS: Smithy binds exactly one error to DescribeDBParameters - DBParameterGroupNotFoundFault (404, awsQueryError code DBParameterGroupNotFound). A well-formed Marker previously issued by the service is a valid input and the operation answers 200 with the next page (the recorded response is a full page of user-source parameters: host_cache_size, information_schema_stats_expiry, innodb_adaptive_flushing_lwm, …, each with ApplyMethod=immediate and Source=user). There is no modeled refusal for a valid marker.
Emulator: answers HTTP 400 and no DescribeDBParametersResponse body at all - its marker parser only understands its own plaintext form, so the service-shaped opaque token is treated as invalid input.
Why: this breaks resumption outright. Any client that persists a marker between calls (or replays one captured against AWS) gets a client error instead of the remaining parameters, and a paginator that treats 400 as fatal aborts the sweep.
Fix: make the DescribeDBParameters marker decoder accept the opaque encoded token (and reject only genuinely malformed input), rather than failing closed on anything that is not a bare parameter name.
Traces: rds-db-parameters-pagination#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, rds answers).

Cause 1 - rds-db-parameters-pagination#task#4, DescribeDBParameters, expected 200 / actual 200: $.DescribeDBParametersResponse.DescribeDBParametersResult.Marker value_mismatch - expected 'aW5ub2RiX2NvbXByZXNzaW9uX2ZhaWx1cmVfdGhyZXNob2xkX3BjdA==' (opaque token form), got 'innodb_compression_level' (bare name). Plus Parameter[0..19].AllowedValues/.ApplyType/.DataType/.Description/.IsModifiable all missing (got None), e.g. Parameter[0] expected AllowedValues '1-65535', ApplyType 'dynamic', DataType 'integer', Description 'Intended for use with master-to-master replication, and can be used to control the operation of AUTO_INCREMENT columns', IsModifiable 'true'.

Cause 2 - rds-db-parameters-pagination#task#5, DescribeDBParameters, 200 vs 200: Parameter[0].AllowedValues '0-9' / ApplyType 'dynamic' / DataType 'integer' / Description 'Sets the cutoff point at which MySQL begins adding padding within compressed pages to avoid expensive compression failures.' / IsModifiable 'true' all missing; same for Parameter[1] (AllowedValues '0-75') and Parameter[2] (AllowedValues '10-100').

Cause 3 - rds-db-parameters-pagination#task#8, DescribeDBParameters, expected status 200, got 400: '$.DescribeDBParametersResponse: missing' - the whole success envelope is absent. The recorded 200 body is a page of Source='user' parameters with ApplyMethod='immediate' (host_cache_size 300, information_schema_stats_expiry 100, innodb_adaptive_flushing_lwm 15, innodb_autoextend_increment 32, innodb_buffer_pool_dump_pct 30, …). The only modeled error for this operation is DBParameterGroupNotFoundFault (404), so a 400 here is unmodeled.

Smithy consulted: DescribeDBParameters - input {DBParameterGroupName @required, Source, Filters, MaxRecords, Marker}, output {Parameters, Marker}, errors [DBParameterGroupNotFoundFault 404 / DBParameterGroupNotFound], paginated {inputToken Marker, outputToken Marker, items Parameters, pageSize MaxRecords}; shape Parameter members {ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion, ApplyMethod, SupportedEngineModes}.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it.
2. CreateDBParameterGroup in the mysql8.0 family, then call DescribeDBParameters(DBParameterGroupName=<group>, MaxRecords=20).
3. Observe the returned Marker is a bare parameter name ('innodb_compression_level') instead of the opaque encoded token the recording carries, and that the Parameter entries carry no AllowedValues/ApplyType/DataType/Description/IsModifiable.
4. Call DescribeDBParameters again with that Marker and observe the second page has the same missing members.
5. Call DescribeDBParameters with the recorded opaque Marker (e.g. 'aW5ub2RiX2NvbXByZXNzaW9uX2ZhaWx1cmVfdGhyZXNob2xkX3BjdA==') and observe HTTP 400 with no DescribeDBParametersResponse body, where AWS answers 200 with the next page.
6. Re-replay rds-db-parameters-pagination steps task#4, task#5 and task#8 to confirm.
