Issue-Source: trace-sweep-divergence
Issue-Trace: glue-crawler-error-paths-and-listing
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-crawler-error-paths-and-listing glue-crawler-lifecycle glue-crawler-invalid-targets-error glue-schema-versions-error-paths
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateCrawler|4xx->2xx|$
Issue-Key: glue|CreateCrawler|4xx->2xx|status
Issue-Key: glue|CreateCrawler|4xx->4xx|$.Message
Issue-Key: glue|DeleteSchemaVersions|4xx->4xx|$.Message
Issue-Key: glue|DeleteSchemaVersions|4xx->4xx|$.__type
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Glue CreateCrawler skips role validation and DeleteSchemaVersions raises an unmodeled exception

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCrawler

## Description
Three wire-fixture clusters in the Glue request-validation path: one input AWS rejects is accepted, and one AWS rejects with the wrong exception type and no message member. Each cause must be closed before this issue is done.

Cause 1 of 3: CreateCrawler accepts a role the service cannot assume, returning 200 where AWS returns 400 InvalidInputException.
AWS: Smithy com.amazonaws.glue#CreateCrawler takes Name (NameString, required), Role (Role, required), Targets (CrawlerTargets, required) and models InvalidInputException (httpError 400, shape doc 'The input provided was not valid'), AlreadyExistsException, OperationTimeoutException and ResourceNumberLimitExceededException; the success response is an empty 200 body. The recording shows AWS validating the supplied Role at create time: a Role whose trust policy does not let Glue assume it is refused with HTTP 400, __type 'InvalidInputException' and the message "Service is unable to assume provided role. Please verify role's TrustPolicy" - that message is AWS's fixed template for this check, so the requirement is the 400/InvalidInputException refusal; the text is an example.
Emulator: CreateCrawler returns HTTP 200 with an empty body and registers the crawler. No role validation happens at all, so a crawler AWS would never have created exists in the emulator's state.
Why: the negative path of every crawler-provisioning flow silently inverts. A client testing that a mis-trusted role is rejected sees success, and code that treats CreateCrawler success as 'this crawler can run' gets a crawler that real Glue would have refused outright - the failure moves from create time to never.
Fix: validate Role in the CreateCrawler handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs) before persisting the crawler - resolve the role in the IAM state and refuse with InvalidInputException (HTTP 400) when its trust policy does not permit the Glue service principal to assume it.
Traces: glue-crawler-error-paths-and-listing#task#2 (cluster also covers glue-crawler-lifecycle#task#3).

Cause 2 of 3: CreateCrawler validates Targets before Role, so a request invalid in both ways reports the wrong rejection.
AWS: as Cause 1 - the recording shows the role trust check running first: a CreateCrawler carrying both an un-assumable Role and an empty Targets is refused with InvalidInputException whose message is the role-trust-policy template, not a targets complaint. Validation precedence is therefore Role before Targets.
Emulator: the same request is refused 400 InvalidInputException but for Targets ('Targets must specify at least one crawl target'), i.e. the role check is absent (Cause 1) so the only check left reports first.
Why: a client correcting the error it is told about fixes Targets, resubmits, and in the emulator succeeds while against AWS it would be refused again for the role - the diagnosis the API hands back points at the wrong field. Closing Cause 1 is necessary but not sufficient: the role check must also run before the targets check.
Fix: in create_crawler.rs order the input checks Role-then-Targets so the first failure reported matches AWS, and keep the existing Targets check for requests whose role is assumable.
Traces: glue-crawler-invalid-targets-error#task#1.

Cause 3 of 3: DeleteSchemaVersions rejects a malformed Versions string with ValidationException and no Message member.
AWS: Smithy com.amazonaws.glue#DeleteSchemaVersions takes SchemaId (required) and Versions (VersionsString, required, length 1..100000, pattern ^[1-9][0-9]*|[1-9][0-9]*-[1-9][0-9]*$) and models exactly AccessDeniedException, ConcurrentModificationException, EntityNotFoundException and InvalidInputException (all httpError 400). ValidationException is not a modeled error of this operation. A Versions value that does not satisfy the pattern is refused with HTTP 400, __type 'InvalidInputException' and a Message built from the request naming the offending parameter - recorded example: 'The parameter value contains one or more characters that are not valid. Parameter Name: Versions'.
Emulator: HTTP 400 with __type 'ValidationException' and no Message member at all in the body.
Why: an AWS SDK maps the error code to a modeled exception class, so a client catching InvalidInputException (the only code this operation can produce for bad input) does not catch ValidationException and the call surfaces as an unmodeled/unknown error; and because the body has no Message, even generic error handling and logs get an empty reason.
Fix: the input validation for DeleteSchemaVersions (services/service-com-amazonaws-glue-awsglue/src/ops/delete_schema_versions.rs and the shared validation error constructor it uses) must raise Glue's InvalidInputException with a Message naming the failing member, rather than the generic cross-service ValidationException - and the error serializer must always emit the Message member for Glue error shapes.
Traces: glue-schema-versions-error-paths#task#2.

## Evidence
Replay of simparity-wire-aws-v1 (121 fixtures, 898 steps, state_reset_failures 0).

Cause 1 - glue-crawler-error-paths-and-listing#task#2 CreateCrawler: expected HTTP 400 with body {'__type': 'InvalidInputException', 'Message': "Service is unable to assume provided role. Please verify role's TrustPolicy"}. Actual: HTTP 200 with an empty body {}. 2 steps affected.

Cause 2 - glue-crawler-invalid-targets-error#task#1 CreateCrawler: status 400 and __type InvalidInputException matched; expected $.Message "Service is unable to assume provided role. Please verify role's TrustPolicy" (the role check running first), actual 'Targets must specify at least one crawl target'.

Cause 3 - glue-schema-versions-error-paths#task#2 DeleteSchemaVersions: expected $.__type 'InvalidInputException' with $.Message 'The parameter value contains one or more characters that are not valid. Parameter Name: Versions'. Actual: $.__type 'ValidationException', $.Message missing (None). Status 400 matched.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture glue-crawler-error-paths-and-listing, step task#2 - CreateCrawler with a Role whose trust policy does not allow glue.amazonaws.com and valid Targets: it returns 200 instead of 400 InvalidInputException.
3. Cause 2: replay fixture glue-crawler-invalid-targets-error, step task#1 - CreateCrawler with that same un-assumable Role and Targets containing no crawl target: the 400 reports the targets problem instead of the role problem.
4. Cause 3: replay fixture glue-schema-versions-error-paths, step task#2 - DeleteSchemaVersions with a Versions string that violates the VersionsString pattern: the 400 body carries __type ValidationException and no Message.
