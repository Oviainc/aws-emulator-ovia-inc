Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: fb07764462a6e3726541f141027927b0567b370d

# The data-transformation endpoint's REST tagging surface (/tags and /untag) has no handler and falls through to the unroutable-operation path

_Filed: 2026-10-29T11:58:12Z_
_Operation: TagResource | Class: missing_

> _Note: this issue was identified against the code base at commit `fb07764462a6e3726541f141027927b0567b370d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shipped model declares a SECOND tagging trio, on the data-transformation endpoint, in the `com.amazon.awshealthlakedatatransformationfrontendservice` namespace: `TagResource` (`POST /tags/{ResourceArn}`), `UntagResource` (`POST /untag/{ResourceArn}`) and `ListTagsForResource` (`GET /tags/{ResourceArn}`). They tag the `DataTransformationProfile` resource (the model marks that resource `aws.api#taggable` and each request shape carries `references: [{resource: ...#DataTransformationProfile, ids: {ProfileId: ResourceArn}}]`).

No code in this crate serves any of the three. There is no `src/ops/` module for them (they are outside the closure of `com.amazonaws.healthlake#HealthLake`, so the generated routing table cannot name them, exactly as for `ValidateSource` and `TransformData`), and `EmulatedService::override_operation` — the one seam this crate uses for a non-RPC path — routes only two paths: a data store FHIR endpoint path and `/validate-source`. A request to `GET /tags/arn:aws:healthlake:...:dataTransformationProfile/<32 hex>` therefore carries no `X-Amz-Target`, matches neither routed path, fails `routing_key`, and is answered by the framework's unroutable-operation path (a bare 404 with no modelled error shape) instead of the profile's tags.

This is not a feature the crate never attempts: the tag state and every helper the three operations need already exist and are already used by the BOUND awsJson tagging operations for profile ARNs — `ComAmazonawsHealthlakeHealthlakeState::tags_for_mutation` resolves a `dataTransformationProfile/...` ARN to the profile's own tag map, and `DataTransformationProfile::tag_list` renders it. Only the routing of the second (REST) wire onto those helpers is absent. The sibling out-of-closure REST operation `ValidateSource` shows the seam exists and was simply not used here.

All three operations should be settled together: they are one surface over one tag map.

## Evidence

MODEL — `smithy_query(query="operation", operation="com.amazon.awshealthlakedatatransformationfrontendservice#TagResource,com.amazon.awshealthlakedatatransformationfrontendservice#ListTagsForResource")`:
```
"TagResource": { "input": {"ResourceArn": {"target": "String", "httpLabel": true, "required": true}, "Tags": {"target": "TagMap", "required": true, "length": {"min": 0, "max": 50}}},
  "http": {"method": "POST", "uri": "/tags/{ResourceArn}", "code": 200} }
"ListTagsForResource": { "input": {"ResourceArn": {"target": "String", "httpLabel": true, "required": true}},
  "output": {"Tags": {"target": "TagMap", "required": true, "length": {"min": 0, "max": 50}}},
  "http": {"method": "GET", "uri": "/tags/{ResourceArn}", "code": 200}, "readonly": true }
```
and `smithy_query(query="operation", operation="com.amazon.awshealthlakedatatransformationfrontendservice#UntagResource")`:
```
"input": {"ResourceArn": {"httpLabel": true, "required": true}, "TagKeys": {"target": "TagKeyList", "required": true, "length": {"min": 1, "max": 50}}},
"http": {"method": "POST", "uri": "/untag/{ResourceArn}", "code": 200}, "idempotent": true
```

CODE — the only two non-RPC paths this service routes, services/service-com-amazonaws-healthlake-healthlake/src/service.rs:426-434:
```
        if req.headers().get("x-amz-target").is_none() {
            let path = non_rpc::request_path(req.uri());
            if crate::state::parse_datastore_fhir_path(path).is_some() {
                return Some(&self.fhir_rest);
            }
            if req.method() == "POST" && non_rpc::addresses_validate_source(path) {
                return Some(&self.validate_source);
            }
        }
```
and the file's own statement of that scope, src/service.rs:25-27:
```
//!   correctly. THIS service uses it for two things. First, HealthLake serves two
//!   surfaces that are not awsJson1_0 RPC calls and carry no `X-Amz-Target` at all —
//!   a data store's FHIR REST data plane ([`fhir_rest`], on the path a
```

NO HANDLER AND NO ROUTE AT ALL — `grep -rn '/tags|/untag' services/service-com-amazonaws-healthlake-healthlake` matches only `smithy-model/com-amazonaws-healthlake-healthlake.json`; nothing under `src/`.

THE HELPERS ALREADY EXIST (so this is a missing wire, not a new feature) — src/state.rs:1874-1883:
```
            Some(TaggedResource::Profile(id)) => self
                .profiles
                .get_mut(id)
                .filter(|profile| profile.arn == arn)
                .map(|profile| MutableTags {
                    tags: &mut profile.tags,
                    max_tags: MAX_TAGS_PER_PROFILE,
                    noun: PROFILE_NOUN,
                })
                .ok_or(PROFILE_NOT_FOUND_MESSAGE),
```
and the bound awsJson `TagResource` already writes through it, src/ops/tag_resource.rs:160-166:
```
        let target = store
            .tags_for_mutation(&input.resource_arn)
            .map_err(|not_found| {
                TagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: Some(not_found.to_string()),
                })
            })?;
```

PRECEDENT IN THE SAME CRATE for serving an out-of-closure REST operation through the same seam: src/service/validate_source.rs:62-63 (`pub const VALIDATE_SOURCE_PATH_SEGMENT: &str = "validate-source";`) wired at src/service.rs:431-432.

TEST COVERAGE: tests/all/resource_tagging.rs, tests/all/resource_untagging.rs and tests/all/list_tags_for_resource.rs exercise only the awsJson (`X-Amz-Target`) tagging operations; no test sends `GET /tags/...` or `POST /untag/...`.

## Steps to Reproduce

1. CreateDataTransformationProfile and note the returned ProfileId; its ARN is arn:aws:healthlake:<region>:<account>:dataTransformationProfile/<ProfileId>.
2. Call TagResource on the awsJson wire (X-Amz-Target: HealthLake.TagResource) with that ARN and {"Key":"team","Value":"clinical"} — it succeeds, so the tags are stored on the profile record.
3. Now address the data-transformation endpoint's modelled REST surface for the same resource: send GET /tags/arn%3Aaws%3Ahealthlake%3A<region>%3A<account>%3AdataTransformationProfile%2F<ProfileId> with no X-Amz-Target header.
4. Observe the request matches neither routed non-RPC path (src/service.rs:426-434) and is answered by the framework's unroutable-operation path instead of {"Tags":{"team":"clinical"}}.
5. Repeat with POST /tags/{ResourceArn} and POST /untag/{ResourceArn}: same result, nothing is written or removed.
