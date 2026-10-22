Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-query-envelope-shared-with-rds
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-query-envelope-shared-with-rds
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: rds|DescribeOptionGroups|4xx->5xx|$.ErrorResponse
Issue-Key: rds|DescribeOptionGroups|4xx->5xx|$headers.content-type
Issue-Key: rds|DescribeOptionGroups|4xx->5xx|status
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# RDS DescribeOptionGroups refused with AmbiguousService 500 JSON instead of Query XML 404

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOptionGroups

## Description
1 cause in this bundle.

Cause 1 of 1: DescribeOptionGroups for a missing option group, issued against the shared RDS Query front door (fixture docdb-query-envelope-shared-with-rds, step task#3).
AWS: RDS uses the awsQuery protocol with endpointPrefix/sigv4Name 'rds' and xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/ (Smithy: com-amazonaws-rds-amazonrdsv19, AmazonRDSv19 2014-10-31). DescribeOptionGroups for an OptionGroupName that does not exist answers HTTP 404 with a text/xml <ErrorResponse><Error><Type>Sender</Type><Code>OptionGroupNotFoundFault</Code>...</Error><RequestId>..</RequestId></ErrorResponse>; the recorded message 'Specified OptionGroupName: <name> not found.' is built from the request and is quoted here as an example. 'AmbiguousService' is not an AWS error code and appears in no AWS model.
Emulator: HTTP 500 with content-type application/x-amz-json-1.1 and the router refusal body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; no ErrorResponse element is produced. The request never reaches the RDS handler: three registered services — service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19 — all declare signing name 'rds', so the router rejects rather than resolving. The refusal is additionally encoded as JSON although the target protocol is awsQuery/XML.
Why: every RDS operation is unreachable. A botocore client raises ResponseParserError ('invalid XML received') rather than a ClientError, so the caller sees neither the 404 status nor the OptionGroupNotFoundFault code and cannot branch on a missing option group.
Fix: the request router's service-resolution table AND the router's error serializer — 'rds' must resolve deterministically to one handler serving the RDS/DocDB/Neptune Query surface (disambiguate on the Query body's Action/Version rather than refusing, since all three models legitimately share endpointPrefix 'rds'), and a router-level refusal for an awsQuery service must be serialized as text/xml <ErrorResponse> with Error/Code and Error/Message instead of a JSON __type body.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice reports this router collision from the BEHAVIORAL corpus (DescribeDBEngineVersions) and carries no wire fixture key, so it does not cover this wire step; note its 'Fix' claim that DocDB/Neptune have their own endpoint prefixes is contradicted by the models, which all declare endpointPrefix 'rds'. The DocumentDB-side manifestations of the same router fault are filed separately under service docdb.
Traces: docdb-query-envelope-shared-with-rds#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1, step docdb-query-envelope-shared-with-rds#task#3 (DescribeOptionGroups with a non-existent OptionGroupName):
  expected: status 404, content-type text/xml, {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'OptionGroupNotFoundFault', 'Message': 'Specified OptionGroupName: <name> not found.'}, 'RequestId': <any>}}
  actual:   status 500, content-type application/x-amz-json-1.1, $.ErrorResponse missing
The body served is the router refusal, captured verbatim on polling steps of the same corpus: {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}.

## Steps to Reproduce
1. Boot the emulator from this checkout (rds, docdb and neptune are all bootstrapped and all claim signing name 'rds').
2. rds = boto3.client('rds', endpoint_url=<emulator>, region_name='us-west-2')
3. rds.describe_option_groups(OptionGroupName='no-such-option-group') -> expected HTTP 404 with a text/xml ErrorResponse whose Error/Code is OptionGroupNotFoundFault; actual HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService",...} (botocore raises ResponseParserError instead of ClientError).
4. Replay fixture docdb-query-envelope-shared-with-rds step task#3 to confirm.
