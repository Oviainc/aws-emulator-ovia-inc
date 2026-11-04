Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cloudcontrol-desired-state-tags-are-applied-to-the-native-resource
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol|ListTagsForResource|InvalidResourceId
Issue-Commit: 62368bc9dd377bdd15bb70a03317a8f201bb87d7

# cloudcontrol: DesiredState Tags are never written to the owning service, so SSM ListTagsForResource refuses the just-created parameter with InvalidResourceId

> _Note: this issue was identified against the code base at commit `62368bc9dd377bdd15bb70a03317a8f201bb87d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service.

Related open issue: 20261102T095426Z-cloudcontrol-6-net-new-causes-cloud-control-s.md lists this same check under its Cause 3, but it keyed it on `cloudcontrol|ListTagsForResource|NotImplemented` — SSM's ListTagsForResource is now implemented (services/service-com-amazonaws-ssm-amazonssm/src/ops/list_tags_for_resource.rs) and the refusal has moved to a different code, so that key no longer matches and this failure is reported unowned. It is filed here so it is not lost; a fixer closing it should also check that issue's Cause 1/Cause 3.

Cause 1 of 1: a Cloud Control CreateResource/UpdateResource of AWS::SSM::Parameter reaches a terminal SUCCESS without ever creating the parameter — or its DesiredState Tags — in SSM, so SSM's own tagging read cannot resolve the resource at all.

AWS: Cloud Control is a facade over the resource type's handler. Smithy (cloudcontrol) models CreateResource with required TypeName/DesiredState answering a ProgressEvent, and OperationStatus SUCCESS is documented as "The resource operation has successfully completed" — so after SUCCESS the resource exists in the owning service, with the Tags the DesiredState declared. Smithy (ssm) models ListTagsForResource with required ResourceType (enum, includes `Parameter`) and ResourceId, answering TagList (list, length 0..1000); its client error InvalidResourceId (HTTP 400) is documented "The resource ID isn't valid. Verify that you entered the correct ID and try again." and the shape carries NO message member, so for a real parameter AWS answers 200 with a TagList holding every tag the DesiredState declared, never InvalidResourceId. Likewise, after an UpdateResource whose PatchDocument is a JSON Patch `remove` of /Tags/<key> reaches SUCCESS, the native TagList holds only the surviving tags and Cloud Control's GetResource Properties.Tags agrees with it.

Emulator: immediately after the create request polls to OperationStatus=SUCCESS, SSM refuses the tag read for that parameter name with HTTP 400 / error code `InvalidResourceId` (empty body message, which is correct for that shape — the code is the whole refusal). The reason is that the create is recorded only in Cloud Control's private store: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs builds a `crate::state::ResourceEffect::Create { properties, identifier_property, type_version_id }` into a `RequestRecord::accepted(...)` and the deferred `store.materialise_due(now)` applies that effect inside Cloud Control's own state only. Nothing writes the parameter into SSM's parameter store, and nothing writes the DesiredState `Tags` into SSM's tag store (`crate::state::tagging`, which list_tags_for_resource.rs reads through `store.resource_tags(&region, &resource_type, &resource_id)`). The id SSM was never given does not resolve, so the tag read refuses it. The patch-remove half of the check is unreachable for the same reason.

Why: a client uses Cloud Control to provision and then reads/uses the resource through the owning API. A SUCCESS for a parameter SSM does not have makes the whole provisioning flow a silent no-op: tags set through Cloud Control are unobservable, every tag audit or compliance sweep run against SSM sees nothing, and the client receives a 400 client error ("your ResourceId is invalid") for an id Cloud Control itself just minted — pointing the blame at the caller's input rather than at the missing write. Tag-driven cost allocation and policy tooling therefore silently operates on untagged resources.

Fix: the request-effect materialisation in the cloudcontrol crate (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs `materialise_due`/`ResourceEffect`, reached from ops/create_resource.rs and ops/update_resource.rs) must apply each effect to the state of the service that owns TypeName — for AWS::SSM::Parameter, creating/updating the parameter in SSM's parameter store and writing the DesiredState `Tags` into SSM's tag store (and removing a tag when a JSON Patch removes it) — before the request may reach SUCCESS, terminating FAILED when it cannot.

Traces: cloudcontrol.cloudcontrol-desired-state-tags-are-applied-to-the-native-resource (suite simparity-behavioral-aws-v1).

## Evidence
Failing check cloudcontrol.cloudcontrol-desired-state-tags-are-applied-to-the-native-resource ([error]). The Cloud Control create of AWS::SSM::Parameter with DesiredState {"Name": <name>, "Type": "String", "Value": "tagged", "Tags": {"sim2real": <check>, "wave": "ga-depth"}} polled to a terminal OperationStatus=SUCCESS, and the very next call — ssm.list_tags_for_resource(ResourceType="Parameter", ResourceId=<that same Name>) — raised:

  botocore.errorfactory.InvalidResourceId: An error occurred (InvalidResourceId) when calling the ListTagsForResource operation:

Expected: HTTP 200 with TagList == [{Key: "sim2real", Value: <check>}, {Key: "wave", Value: "ga-depth"}] (order-insensitive), then after UpdateResource with PatchDocument [{"op":"remove","path":"/Tags/wave"}] reaches SUCCESS, TagList == [{Key: "sim2real", Value: <check>}] and GetResource Properties["Tags"] agreeing with it.
Actual: HTTP 400, error code InvalidResourceId, no message member (the ssm model gives InvalidResourceId no message, so the empty message is NOT the defect — the refusal itself is). The parameter name, check id and wave value above are per-run request values filling the template; the fixed requirement is that a resource Cloud Control reported SUCCESS for resolves at the owning service and carries the declared tags.

## Steps to Reproduce
1. Boot the emulator; cc = client("cloudcontrol"), ssm = client("ssm").
2. name = <unique parameter name>; cc.create_resource(TypeName="AWS::SSM::Parameter", DesiredState=json.dumps({"Name": name, "Type": "String", "Value": "tagged", "Tags": {"sim2real": "x", "wave": "ga-depth"}})).
3. Poll cc.get_resource_request_status(RequestToken=...) until OperationStatus is terminal — it reports SUCCESS.
4. ssm.list_tags_for_resource(ResourceType="Parameter", ResourceId=name) — AWS answers 200 with both tags; the emulator answers 400 InvalidResourceId.
5. (Blocked by step 4) cc.update_resource(TypeName="AWS::SSM::Parameter", Identifier=name, PatchDocument=json.dumps([{"op":"remove","path":"/Tags/wave"}])), poll to SUCCESS, then repeat step 4 — only {"sim2real": "x"} must remain, and json.loads(cc.get_resource(TypeName="AWS::SSM::Parameter", Identifier=name)["ResourceDescription"]["Properties"])["Tags"] must equal it.
