Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.tag-resource-reserved-prefix-denied s3tables.tag-resource-invalid-key-rejected s3tables.tag-resource-malformed-key-rejected s3tables.tag-resource-on-deleted-table-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.tag-resource-reserved-prefix-denied|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: s3tables.tag-resource-invalid-key-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: s3tables.tag-resource-malformed-key-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: s3tables.tag-resource-on-deleted-table-rejected|run#13|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Commit: bf05938345e3744e1c8e3343a2f8d43b66feefe9

# s3tables TagResource: tag-key validation and stale-resource refusals are absent — reserved aws: prefix, malformed key charset and deleted-table ARN all answer 501 NotImplemented instead of 400 BadRequestException / 404 NotFoundException

> _Note: this issue was identified against the code base at commit `bf05938345e3744e1c8e3343a2f8d43b66feefe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
TagResource on S3 Tables answers HTTP 501 with error code `NotImplemented` for every request, so none of its modeled refusal classes are reachable. The handler is the unmodified scaffold (`services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/tag_resource.rs`), which is separately tracked for mere existence by the open issue `20261101T233620Z-s3tables-tagresource-stub.md`; that issue names the operation and 13 OTHER replay units, and carries none of the four checks below, so the refusal semantics stated here are not covered by it. This issue records the three validation/refusal behaviours that must hold once the handler exists.

Cause 1 of 3: reserved `aws:` tag-key prefix is not refused.
AWS: TagResource models BadRequestException (httpError 400, "The request is invalid or malformed.") among its errors (Smithy: TagResource errors = BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500; input `tags` targets `Tags`, a map of TagKey @length 1..128 to TagValue @length 0..256 — the model states no prefix rule, so this is recorded behavior only). Recorded behavior: a TagResource whose `tags` map contains any key beginning with `aws:` is refused with HTTP 400, error code `BadRequestException` and the message `Tags beginning with 'aws:' are reserved for system use.` — that message is entirely fixed text, with no value taken from the request. The rejected key must not be stored (a following ListTagsForResource must not contain it), and a non-reserved key on the same resource must still be accepted.
Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation TagResource is scaffolded but not implemented in this emulator`.
Why: a client that relies on AWS refusing reserved tag keys gets a 501 server-class failure instead of a 400 client-class one — retry logic treats it as a transient server fault, and no client can distinguish "your tag key is reserved" from "the service is broken". Tagging also never happens at all, so the positive control (an ordinary key) is lost too.
Fix: the TagResource handler and its input validation in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/tag_resource.rs — validate each key of `tags` for the reserved `aws:` prefix before any state write, and emit BadRequestException with the fixed message above.
Traces: s3tables.tag-resource-reserved-prefix-denied, s3tables.tag-resource-invalid-key-rejected.

Cause 2 of 3: a tag key outside the allowed character set is not refused.
AWS: Smithy constrains TagKey only by @length 1..128, so the character-set rule is recorded behavior only. Recorded behavior: a tag key containing a character outside letters, digits, whitespace and `_.:/=+@-` (e.g. `bad key!`) is refused with HTTP 400, code `BadRequestException` and the message `The provided tags are invalid. Tag key can only contain alphanumeric characters, spaces, and _.:/=+@-` — all fixed text; the offending key is not interpolated. The malformed key must not land on the resource, and a well-formed key must still be accepted afterwards.
Emulator: HTTP 501, code `NotImplemented`, same scaffold message.
Why: callers that validate tag keys by submitting them and reading the refusal see a server error, and an emulator that later implements TagResource without this rule would silently STORE a key AWS never accepts, so state diverges permanently.
Fix: the same handler's input validation — apply the tag-key character-set rule to every key of `tags` before writing state.
Traces: s3tables.tag-resource-malformed-key-rejected.

Cause 3 of 3: tagging a deleted table's stale ARN is not refused as missing.
AWS: TagResource models NotFoundException (httpError 404, "The request was rejected because the specified resource could not be found."); its `resourceArn` is an httpLabel @required with pattern `^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/.+$`, so a deleted table's ARN is still well-formed and must be resolved against state rather than rejected by pattern. Recorded behavior: TagResource against the ARN of a table that has been deleted is refused with HTTP 404, code `NotFoundException` and the message `The specified table does not exist.` — fixed text, no value interpolated. A still-live sibling table must remain taggable.
Why: a client reading 501 cannot tell that its cached table ARN is stale; worse, an implementation without this existence check would attach tags to a deleted resource identity, resurrecting rows that AWS has no record of.
Fix: the TagResource handler's state lookup — resolve `resourceArn` (table-bucket, namespace or table form) in the service state and answer NotFoundException when the referenced table no longer exists, before applying any tag.
Traces: s3tables.tag-resource-on-deleted-table-rejected.

## Evidence
Replaying the behavioral suite against the emulator:

- s3tables.tag-resource-reserved-prefix-denied — `AssertionError: expected BadRequestException, got 'NotImplemented'` at `assert code == "BadRequestException"`; the check then requires `message == "Tags beginning with 'aws:' are reserved for system use."` for `tags={"aws:reserved-marker": "x"}`.
- s3tables.tag-resource-invalid-key-rejected — `AssertionError: expected BadRequestException, got NotImplemented` for `tags={"aws:sim2real-reserved": "nope"}`, expected message `Tags beginning with 'aws:' are reserved for system use.`
- s3tables.tag-resource-malformed-key-rejected — `AssertionError: expected BadRequestException, got NotImplemented` for `tags={"bad key!": "value"}`, expected message `The provided tags are invalid. Tag key can only contain alphanumeric characters, spaces, and _.:/=+@-`
- s3tables.tag-resource-on-deleted-table-rejected — `AssertionError: expected NotFoundException, got NotImplemented` when tagging the ARN of a table deleted by DeleteTable, expected message `The specified table does not exist.`

In every case the wire response is HTTP 501 with `Error.Code = NotImplemented` and `Error.Message = operation TagResource is scaffolded but not implemented in this emulator`. `NotImplemented` is not a modeled error of TagResource at all (Smithy: BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500).

Related but non-covering: `20261101T233620Z-s3tables-tagresource-stub.md` (asks only that the operation be implemented; its Issue-Trace-Ids list does not include any of the four checks above).

## Steps to Reproduce
1. Boot the s3tables emulator.
2. CreateTableBucket(name=<unique lowercase name>) and keep the returned `arn`.
3. Call TagResource(resourceArn=<bucket arn>, tags={"aws:reserved-marker": "x"}) — expect HTTP 400 BadRequestException with message `Tags beginning with 'aws:' are reserved for system use.`; observe HTTP 501 NotImplemented.
4. Call TagResource(resourceArn=<bucket arn>, tags={"bad key!": "value"}) — expect HTTP 400 BadRequestException with message `The provided tags are invalid. Tag key can only contain alphanumeric characters, spaces, and _.:/=+@-`; observe HTTP 501 NotImplemented.
5. CreateNamespace + CreateTable(format=ICEBERG) twice in that bucket, DeleteTable one of them, then call TagResource(resourceArn=<deleted table ARN>, tags={"stale": "should-not-attach"}) — expect HTTP 404 NotFoundException with message `The specified table does not exist.`; observe HTTP 501 NotImplemented.
6. Confirm ListTagsForResource shows none of the rejected keys, and that a well-formed tag on a live resource is accepted.
