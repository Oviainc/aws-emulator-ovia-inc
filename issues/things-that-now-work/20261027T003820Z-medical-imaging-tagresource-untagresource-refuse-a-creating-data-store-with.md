Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medical-imaging.tag-resource-list-untag-roundtrip
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging|TagResource|ConflictException
Issue-Commit: 0550644949cffe934022bf1b5a8b285c9bc5e880

# TagResource/UntagResource refuse a CREATING data store with an unmodeled ConflictException, so tagging straight after CreateDatastore is impossible

> _Note: this issue was identified against the code base at commit `0550644949cffe934022bf1b5a8b285c9bc5e880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: the medical-imaging emulator gates the tagging write path on the data store having settled into ACTIVE, and refuses the call otherwise with ConflictException (HTTP 409). AWS HealthImaging accepts tagging of a data store that is still CREATING.

AWS: Smithy (services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json) models TagResource with input {resourceArn @httpLabel @required (pattern ^arn:aws(...)?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$), tags @required TagMap}, POST /tags/{resourceArn}, success 200 with an empty body, and its COMPLETE error list is AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). ConflictException is NOT a modeled error of TagResource, nor of UntagResource or ListTagsForResource (same five errors each) — the only operations in this model that declare ConflictException are the lifecycle ones (e.g. CreateDatastore). So there is no modeled status in which TagResource answers 409. Recorded behavior confirms it: the check creates a data store, reads its ARN with GetDatastore, and immediately tags it — against AWS this sequence succeeds (TagResource 200, ListTagsForResource returns exactly the two tags, UntagResource removes exactly the requested key), which is why the check only polls for ACTIVE at the very end, purely so its cleanup DeleteDatastore does not race the creation. Tagging is metadata on the resource record and does not depend on the data store's readiness to serve data-plane work.

Emulator: the call is refused with HTTP 409, error code ConflictException, message "The data store is not in the ACTIVE state, so it cannot serve this request." (the whole sentence is fixed text — this message has no variable parts; it reflects back no datastore id, ARN or status). The refusal comes from the shared tag-write lookup: crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::writable_resource_tags_mut returns UnwritableResource::DatastoreNotActive whenever !datastore.is_active(), and the TagResource handler maps that to ConflictException { DATASTORE_NOT_ACTIVE_MESSAGE }. Because the emulator keeps a newly created data store in CREATING for DATASTORE_ACTIVATION_DELAY, any tag write issued promptly after CreateDatastore is refused. UntagResource goes through the same lookup and therefore has the same refusal; ListTagsForResource (the read path, resource_tags) does not, so the three tagging operations disagree about which stores can be tagged.

Why: a client that creates a data store and tags it in the same code path (the normal provisioning pattern, and what every AWS SDK/IaC caller does) gets a hard 409 instead of a 200. Worse, ConflictException is not in TagResource's modeled error set, so generated SDK clients and retry/error-classification code have no case for it: botocore surfaces it as an unexpected ConflictException rather than a retryable or handled condition, and callers cannot distinguish it from a real lifecycle conflict. Tagging also becomes timing-dependent in the emulator, which no AWS behavior justifies.

Fix: the data-store readiness guard must not be applied to the tagging write path. Remove the ACTIVE precondition from the tag-write lookup (crate::state::writable_resource_tags_mut's !datastore.is_active() branch, and the UnwritableResource::DatastoreNotActive arm in the TagResource and UntagResource handlers), keeping only the resource-existence refusals (ResourceNotFoundException for a data store this emulator does not hold in the request's region, and for a retired image set). With the conflict arm gone, ConflictException should also come back out of the operations' extra_errors widening, so the handlers' error channels match the modeled error sets. TagResource must answer 200 with an empty body, and the tags must be visible to ListTagsForResource immediately, whatever the data store's status.

Traces: medical-imaging.tag-resource-list-untag-roundtrip (fails on the first TagResource call, immediately after CreateDatastore + GetDatastore). Existing open medical-imaging issues (StartDICOMImportJob S3/event propagation, CopyImageSet copiableAttributes, import-job image-set creation) are unrelated to this tagging guard and do not cover it.

## Evidence
Expected (AWS): TagResource(resourceArn=<datastore ARN>, tags={"env":"sim2real","owner":"behavioral"}) -> HTTP 200, empty body; ListTagsForResource(resourceArn) -> {"tags": {"env":"sim2real","owner":"behavioral"}}; UntagResource(resourceArn, tagKeys=["env"]) -> HTTP 200; ListTagsForResource -> {"tags": {"owner":"behavioral"}}. TagResource's modeled errors do not include ConflictException.

Actual (emulator): TagResource -> HTTP 409, code ConflictException, message "The data store is not in the ACTIVE state, so it cannot serve this request." botocore raises: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the TagResource operation: The data store is not in the ACTIVE state, so it cannot serve this request. The check never reaches ListTagsForResource/UntagResource.

Source: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/tag_resource.rs lines 98-101 (ConflictException widened in via extra_errors) and lines 207-220 (UnwritableResource::DatastoreNotActive -> ConflictException { DATASTORE_NOT_ACTIVE_MESSAGE }); services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs lines 1603-1614 (writable_resource_tags_mut's !datastore.is_active() guard), line 297 (DATASTORE_NOT_ACTIVE_MESSAGE), line 71 (DATASTORE_ACTIVATION_DELAY keeps a new store CREATING).

## Steps to Reproduce
1. CreateDatastore(datastoreName=<unique>, clientToken=<unique>) -> note datastoreId (status CREATING).
2. GetDatastore(datastoreId) -> take datastoreProperties.datastoreArn.
3. Immediately (without waiting for datastoreStatus == ACTIVE) call TagResource(resourceArn=<that ARN>, tags={"env":"sim2real","owner":"behavioral"}).
4. Observe HTTP 409 ConflictException "The data store is not in the ACTIVE state, so it cannot serve this request." instead of a 200 with an empty body. The same call after the store reaches ACTIVE succeeds, so the failure is purely the readiness guard. UntagResource against a CREATING store fails the same way.
5. Replay check medical-imaging.tag-resource-list-untag-roundtrip.
