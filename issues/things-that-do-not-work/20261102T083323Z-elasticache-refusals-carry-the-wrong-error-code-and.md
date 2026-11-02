Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-tgwc-traj-cache-cluster-id-form-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-tgwc-traj-cache-cluster-id-form-mistakes elasticache-tgwc-traj-cache-subnet-group-vpc-resolution elasticache-vvd-traj-cache-subnet-group-build-mistakes elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes elasticache-vvd-traj-replication-group-id-and-topology-mistakes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeCacheClusters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeReplicationGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeCacheSubnetGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|DescribeCacheSubnetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateCacheSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|CreateCacheSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|CreateReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# ElastiCache refusals carry the wrong error code and non-AWS message text across seven validation paths

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheClusters

## Description
Seven independent root causes on the IMPLEMENTED ElastiCache refusal surface. Every one of them answers HTTP 400 as AWS does, but the awsQuery Error/Code and/or Error/Message the emulator serializes is not the one AWS serves, so a client that keys off the error code or surfaces the message to a user gets something AWS never emits. Each cause is a separate template/code decision in a different handler and must be closed separately.

Cause 1 of 7: DescribeCacheClusters refuses an over-long cluster identifier with its own invented message.
AWS: Smithy: DescribeCacheClusters takes CacheClusterId (String) and the service models InvalidParameterValueException -> awsQueryError code 'InvalidParameterValue', httpResponseCode 400 (protocol awsQuery, xmlNamespace http://elasticache.amazonaws.com/doc/2015-02-02/). The recorded response for an identifier past the length ceiling is HTTP 400, Code 'InvalidParameterValue', Message 'The parameter CacheClusterIdentifier is not a valid identifier because it is longer than 50 characters.' Fixed text: 'The parameter ' <member name as AWS words it> ' is not a valid identifier because it is longer than ' <limit> ' characters.'; the variable parts are the AWS-facing member name (CacheClusterIdentifier, not the wire member CacheClusterId) and the 50-character ceiling. No value from the request is echoed.
Emulator: HTTP 400 with Message 'Cache Cluster ID cannot be longer than 50 characters.' — a different sentence shape with a different subject (prose 'Cache Cluster ID' instead of the parameter name) and no 'is not a valid identifier because' clause.
Why: a client that shows the service message, or matches on it to distinguish a length violation from a character-class violation, sees text AWS never produces; the message is part of the error contract for this refusal class.
Fix: the DescribeCacheClusters input validation in the elasticache crate — the message template used for the identifier-length refusal.
Traces: elasticache-tgwc-traj-cache-cluster-id-form-mistakes#task#3 (cluster also covers #task#5).

Cause 2 of 7: DescribeReplicationGroups words the invalid-identifier refusal with the parameter-name template instead of AWS's replication-group template.
AWS: Smithy: DescribeReplicationGroups takes ReplicationGroupId; InvalidParameterValueException -> code 'InvalidParameterValue', HTTP 400. Recorded Message: 'Replication Group ID specified is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' The whole string is fixed text for this error class — AWS names the resource ('Replication Group ID specified'), it does not name a parameter and it echoes nothing from the request.
Emulator: 'The parameter ReplicationGroupIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' — the character-class clause is right, the leading clause is the generic 'The parameter <X> is not a valid identifier' template. Hypothesis: one shared identifier-validation helper emits the same leading clause for every resource, where AWS varies it per resource (compare Cause 1, where AWS DOES use the parameter-name form).
Why: a client cannot rely on either wording; the same emulator sentence is served where AWS serves two different sentences, so message-based handling and user-facing text both diverge.
Fix: the DescribeReplicationGroups identifier validation / the shared identifier-message helper in the elasticache crate, so the leading clause is per-resource.
Traces: elasticache-tgwc-traj-cache-cluster-id-form-mistakes#task#9.

Cause 3 of 7: DescribeCacheSubnetGroups refuses a malformed subnet-group name with an error code that is not an ElastiCache error at all.
AWS: Smithy: DescribeCacheSubnetGroups models CacheSubnetGroupNotFoundFault (code 'CacheSubnetGroupNotFoundFault', 400) and the service's InvalidParameterValueException maps to awsQueryError code 'InvalidParameterValue', httpResponseCode 400. 'ValidationException' is not a shape in this model and is not an awsQuery error code ElastiCache emits. Recorded response: HTTP 400 with Error/Code 'InvalidParameterValue'.
Emulator: HTTP 400 with Error/Code 'ValidationException'.
Why: SDK error handling is driven by the code — botocore builds InvalidParameterValueException from 'InvalidParameterValue' and will fall through to a generic ClientError for 'ValidationException', so `except client.exceptions.InvalidParameterValueException` never fires and retry/branching logic takes the wrong path.
Fix: the DescribeCacheSubnetGroups validation path's error classification in the elasticache crate — it must raise the modeled InvalidParameterValueException (awsQuery code InvalidParameterValue) rather than a generic validation fault.
Traces: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#7.

Cause 4 of 7: the DescribeCacheSubnetGroups not-found message echoes the requested name upper-cased instead of verbatim.
AWS: recorded behavior — HTTP 400 with Message 'Cache subnet group <name> not found.' The fixed text is 'Cache subnet group ' and ' not found.'; the single variable part is the CacheSubnetGroupName exactly as the request spelled it. (Recorded example: the request asked for a lower-case name and AWS echoed that same lower-case name.)
Emulator: serves the same sentence but with the name transformed to upper case, e.g. '... SIM2REAL-PROBE-TGWC-CSG-91D3A6-GONE not found.' where the request named the lower-case form. Hypothesis: the handler case-folds the name for its store lookup and then formats the message from the folded key instead of from the request value.
Why: a client that correlates the message back to the resource it asked about (logging, test assertions, or parsing the name out of the message) gets a name it never sent; the echo of a request value is part of this message's contract.
Fix: the DescribeCacheSubnetGroups not-found path in the elasticache crate — format the message from the request's CacheSubnetGroupName, keeping any case folding internal to the lookup key.
Traces: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#8.

Cause 5 of 7: CreateCacheSubnetGroup with SubnetIds absent answers a JSON-protocol-style constraint message under a non-ElastiCache code.
AWS: Smithy: CreateCacheSubnetGroup input members CacheSubnetGroupName, CacheSubnetGroupDescription and SubnetIds (SubnetIdentifierList) are all required; the service's InvalidParameterValueException maps to awsQueryError code 'InvalidParameterValue', httpResponseCode 400. Recorded response when SubnetIds is omitted: HTTP 400, Code 'InvalidParameterValue', Message 'The parameter SubnetIds must be provided.' Fixed text: 'The parameter ' <member name> ' must be provided.'; the variable part is the missing member name as the model spells it (SubnetIds).
Emulator: HTTP 400 with Code 'MalformedInput' and Message "1 validation error detected: Value null at 'subnetIds' failed to satisfy constraint: Member must not be null" — a Coral/JSON-protocol required-member message (lower-camel member path, 'N validation errors detected' envelope) leaking into an awsQuery service, under a code ElastiCache does not model.
Why: both halves of the contract are wrong. 'MalformedInput' is not an ElastiCache error code, so the SDK's InvalidParameterValueException is never raised, and the message names 'subnetIds' rather than the modeled member SubnetIds, so a client cannot identify the offending input from the message either.
Fix: the CreateCacheSubnetGroup required-member validation in the elasticache crate — emit the modeled InvalidParameterValueException with the awsQuery 'The parameter <Member> must be provided.' template instead of the generic framework constraint error.
Traces: elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#6 (cluster also covers #task#8 and #task#10).

Cause 6 of 7: CreateReplicationGroup refuses an AuthToken without transit encryption under InvalidParameterCombination instead of InvalidParameterValue, with its own message.
AWS: Smithy: CreateReplicationGroup takes AuthToken (String) and TransitEncryptionEnabled (BooleanOptional) and models BOTH InvalidParameterValueException (awsQueryError code 'InvalidParameterValue', 400) and InvalidParameterCombinationException (code 'InvalidParameterCombination', 400). For this refusal the recorded response is HTTP 400, Code 'InvalidParameterValue', Message 'The AUTH token is only supported when encryption-in-transit is enabled' (fixed text, no trailing period, no values interpolated).
Emulator: HTTP 400, Code 'InvalidParameterCombination', Message 'AuthToken can be specified only on a replication group where TransitEncryptionEnabled is true.'
Why: the emulator picks the other modeled error, so a client branching on InvalidParameterValue (as AWS serves here) misroutes this refusal, and the message it displays is not AWS's wording for the class.
Fix: the CreateReplicationGroup AuthToken/TransitEncryption validation in the elasticache crate — classify as InvalidParameterValueException and use AWS's message text.
Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#8 (cluster also covers #task#11, #task#12 and elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#5, #task#6).

Cause 7 of 7: CreateReplicationGroup leaks the AuthToken password rule in the refusal message instead of AWS's opaque message.
AWS: Smithy: CreateReplicationGroup.AuthToken is a plain String with no modeled length constraint; the refusal is InvalidParameterValueException -> code 'InvalidParameterValue', HTTP 400. Recorded Message: 'Invalid AuthToken provided.' — entirely fixed text, with no values from the request and deliberately no detail about why the token is invalid.
Emulator: HTTP 400 with Message 'Invalid value for AuthToken: the password must be at least 16 and no more than 128 characters in length.'
Why: the message differs from AWS's wording for the class, and it discloses the credential policy AWS keeps out of this response; a client asserting on, or logging, this message diverges.
Fix: the CreateReplicationGroup AuthToken validation message in the elasticache crate.
Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#9 (cluster also covers #task#10, elasticache-vvd-traj-replication-group-id-and-topology-mistakes#task#6, #task#7, #task#8 and elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#7).

Relation to open issues: 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md covers six DIFFERENT refusal paths (not-found, out-of-range parameter, invalid ARN, invalid access-string category) and 20261030T130325Z-elasticache-checks-identifier-validity-after-resolving-the-resource.md covers validation ORDERING; neither owns any of the seven fixture steps above (the replay report lists owned_by as empty for all of them), so these seven causes are net-new.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against this checkout (230 steps, 150 passed, 80 mismatched, score 65.2%). Per cause, expected = recorded AWS response, actual = emulator:
1. DescribeCacheClusters, fixture elasticache-tgwc-traj-cache-cluster-id-form-mistakes step task#3 — status 400 == 400; $.ErrorResponse.Error.Message: expected 'The parameter CacheClusterIdentifier is not a valid identifier because it is longer than 50 characters.', got 'Cache Cluster ID cannot be longer than 50 characters.'
2. DescribeReplicationGroups, same fixture step task#9 — status 400 == 400; $.ErrorResponse.Error.Message: expected 'Replication Group ID specified is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.', got 'The parameter ReplicationGroupIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'
3. DescribeCacheSubnetGroups, fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution step task#7 — status 400 == 400; $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'ValidationException'.
4. DescribeCacheSubnetGroups, same fixture step task#8 — status 400 == 400; $.ErrorResponse.Error.Message: expected 'Cache subnet group sim2real-probe-tgwc-csg-91d3a6-gone not found.', got 'Cache subnet group SIM2REAL-PROBE-TGWC-CSG-91D3A6-GONE not found.' (the generated name is a per-run value; the requirement is that the requested name is echoed verbatim, not upper-cased).
5. CreateCacheSubnetGroup, fixture elasticache-vvd-traj-cache-subnet-group-build-mistakes step task#6 (cluster of 3) — status 400 == 400; $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'MalformedInput'; $.ErrorResponse.Error.Message: expected 'The parameter SubnetIds must be provided.', got "1 validation error detected: Value null at 'subnetIds' failed to satisfy constraint: Member must not be null".
6. CreateReplicationGroup, fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes step task#8 (cluster of 5) — status 400 == 400; $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'InvalidParameterCombination'; $.ErrorResponse.Error.Message: expected 'The AUTH token is only supported when encryption-in-transit is enabled', got 'AuthToken can be specified only on a replication group where TransitEncryptionEnabled is true.'
7. CreateReplicationGroup, same fixture step task#9 (cluster of 6) — status 400 == 400; $.ErrorResponse.Error.Message: expected 'Invalid AuthToken provided.', got 'Invalid value for AuthToken: the password must be at least 16 and no more than 128 characters in length.'
Model evidence (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): service AmazonElastiCacheV9, version 2015-02-02, protocol awsQuery; InvalidParameterValueException -> awsQueryError {code: InvalidParameterValue, httpResponseCode: 400}; InvalidParameterCombinationException -> {code: InvalidParameterCombination, 400}; CreateCacheSubnetGroup requires CacheSubnetGroupName, CacheSubnetGroupDescription, SubnetIds; no 'ValidationException' or 'MalformedInput' shape exists in the model.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Cause 1: elasticache DescribeCacheClusters with a CacheClusterId longer than 50 characters -> expect 400 InvalidParameterValue 'The parameter CacheClusterIdentifier is not a valid identifier because it is longer than 50 characters.'; observe 'Cache Cluster ID cannot be longer than 50 characters.' Re-replay elasticache-tgwc-traj-cache-cluster-id-form-mistakes#task#3 (and #task#5).
3. Cause 2: elasticache DescribeReplicationGroups with a ReplicationGroupId that starts with a digit or ends with a hyphen -> expect the 'Replication Group ID specified is not a valid identifier. ...' message; observe the 'The parameter ReplicationGroupIdentifier ...' form. Re-replay elasticache-tgwc-traj-cache-cluster-id-form-mistakes#task#9.
4. Cause 3: elasticache DescribeCacheSubnetGroups with a malformed CacheSubnetGroupName -> expect Error/Code 'InvalidParameterValue'; observe 'ValidationException'. Re-replay elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#7.
5. Cause 4: elasticache DescribeCacheSubnetGroups naming a lower-case subnet group that does not exist -> expect 'Cache subnet group <that exact name> not found.'; observe the name upper-cased. Re-replay elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#8.
6. Cause 5: elasticache CreateCacheSubnetGroup with CacheSubnetGroupName and CacheSubnetGroupDescription but no SubnetIds -> expect 400 InvalidParameterValue 'The parameter SubnetIds must be provided.'; observe MalformedInput with the '1 validation error detected ...' message. Re-replay elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#6 (and #task#8, #task#10).
7. Cause 6: elasticache CreateReplicationGroup with AuthToken set and TransitEncryptionEnabled absent/false -> expect 400 InvalidParameterValue 'The AUTH token is only supported when encryption-in-transit is enabled'; observe 400 InvalidParameterCombination with the TransitEncryptionEnabled message. Re-replay elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#8 (and #task#11, #task#12, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#5, #task#6).
8. Cause 7: elasticache CreateReplicationGroup with TransitEncryptionEnabled=true and a too-short AuthToken -> expect 400 InvalidParameterValue 'Invalid AuthToken provided.'; observe the length-rule message. Re-replay elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#9 (and #task#10, elasticache-vvd-traj-replication-group-id-and-topology-mistakes#task#6, #task#7, #task#8, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#7).
