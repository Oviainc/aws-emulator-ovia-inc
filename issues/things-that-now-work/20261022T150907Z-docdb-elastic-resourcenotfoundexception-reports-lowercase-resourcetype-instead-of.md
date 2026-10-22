Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-get-cluster-nonexistent-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-get-cluster-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|GetCluster|4xx->4xx|$.message
Issue-Key: docdb-elastic|GetCluster|4xx->4xx|$.resourceType
Issue-Key: docdb-elastic|GetCluster|4xx->4xx|$headers.content-type
Issue-Commit: 96c577ebafbf068c2577fa32d2ad5c15ed5a506c

# DocDB Elastic ResourceNotFoundException reports lowercase resourceType instead of the AWS resource type name

> _Note: this issue was identified against the code base at commit `96c577ebafbf068c2577fa32d2ad5c15ed5a506c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
The emulator's ResourceNotFoundException for DocDB Elastic populates the required `resourceType` member with a lowercase, internal-looking token rather than the AWS resource type name, so the error body of a 404 is not shape-faithful.

Cause 1 of 1: GetCluster on a cluster ARN that does not exist returns the right error and the right status, but the wrong `resourceType` value.

AWS: Smithy model services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/smithy-model/com-amazonaws-docdbelastic-chimeradblionfishservicelambda.json - service ChimeraDbLionfishServiceLambda (sdkId "DocDB Elastic", apiVersion 2022-11-28, protocol restJson1). GetCluster is GET /cluster/{clusterArn} with clusterArn as a required @httpLabel, and models ResourceNotFoundException @error("client") @httpError(404) with three REQUIRED members: message, resourceId and resourceType, the last documented as "The type of the resource that could not be found." The recorded response for a non-existent cluster is HTTP 404 with ResourceNotFoundException carrying resourceType = "Cluster" - the capitalized DocDB Elastic resource type name, matching the `Cluster` output shape and the `cluster/...` ARN resource. Exact spelling of the `message` string is recorded behavior only (no documented or model-stated rule produces it), so it is cited below as an example and not as the requirement.

Emulator: on the same request the emulator answers HTTP 404 with ResourceNotFoundException (status and error shape correct) but sets $.resourceType to "cluster" instead of "Cluster".

Why: resourceType is a required, machine-readable discriminator on this error, and the only field in the body that tells a client WHICH kind of DocDB Elastic resource was missing. Client code, retry/cleanup logic and infrastructure tooling that branch on the AWS resource type name (equality against "Cluster", or a dispatch table keyed by resource type) take the wrong branch or fall through to an unknown-resource path against the emulator while working against AWS. Because the status and error code are right, the mismatch is silent: the SDK deserializes the error successfully and only the dispatch on resourceType goes wrong.

Fix: the GetCluster not-found path in the DocDB Elastic error construction - whatever builds ResourceNotFoundException should emit the AWS resource type name ("Cluster") rather than reusing the lowercase ARN resource segment / internal resource key. The same constructor is shared by the other cluster lookups, so it should be corrected centrally rather than at the GetCluster call site only.

Traces: docdb-elastic-get-cluster-nonexistent-not-found#task#1 (GetCluster, expected 404 / actual 404; $.resourceType expected "Cluster", got "cluster"). The same step also records a $.message difference - expected "Unable to find the cluster", got "cluster arn:aws:docdb-elastic:<region>:<account>:cluster/<id> not found" - reported here as an example only, since no documented or modeled rule fixes that string.

## Evidence
Replay of fixture docdb-elastic-get-cluster-nonexistent-not-found, step task#1 (operation GetCluster, verdict mismatch):

Expected (recorded AWS): HTTP 404, ResourceNotFoundException, $.resourceType = "Cluster"
Actual (emulator):       HTTP 404, ResourceNotFoundException, $.resourceType = "cluster"

Status matches (404 -> 404); the divergence is the value of the required resourceType member. Model grounding: ResourceNotFoundException.resourceType is @required (String) on a @httpError(404) client error bound to GetCluster (GET /cluster/{clusterArn}), restJson1.

Also recorded on the same step, cited as an example rather than a requirement:
  $.message expected "Unable to find the cluster", got "cluster arn:aws:docdb-elastic:...:cluster/... not found".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture docdb-elastic-get-cluster-nonexistent-not-found, step task#1: send GetCluster (GET /cluster/{clusterArn}, restJson1, sigv4 service name docdb-elastic) for a cluster ARN that was never created.
3. Observe the 404 ResourceNotFoundException body: $.resourceType is "cluster".
4. Expected per the recorded AWS response and the Smithy model: $.resourceType is the resource type name "Cluster".
