Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# The crate reports an untagged resource's TagMap two contradictory ways: GetIPSet emits an empty map while the sibling GetThreatIntelSet omits the member, citing the same modelled constraint

_Filed: 2026-10-25T04:51:29Z_
_Operation: GetIPSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Nine readers in this one crate project the same `TagMap` shape, and they split into two groups that implement opposite rules for a resource that carries no tags, each with a comment asserting its rule is the correct one.

Emit `tags: {}`:
  - `GetIPSet` (src/ops/get_ip_set.rs:181-190)
  - `GetThreatEntitySet` (src/ops/get_threat_entity_set.rs:199-206)
  - `GetTrustedEntitySet` (src/ops/get_trusted_entity_set.rs:197-204)
  - `GetMalwareProtectionPlan` (src/ops/get_malware_protection_plan.rs:225-232)

Omit the member entirely:
  - `GetThreatIntelSet` (src/ops/get_threat_intel_set.rs:195-207)
  - `GetFilter` (src/ops/get_filter.rs:190-198)
  - `GetCustomDetectionRuleAssociation` (src/ops/get_custom_detection_rule_association.rs:183-193)
  - `DescribePublishingDestination` (src/ops/describe_publishing_destination.rs:192-202)

`GetIPSet` and `GetThreatIntelSet` are the sharpest pair: two reads of two detector sub-resources of the same kind (an uploaded list), with the same output member targeting the same `TagMap` shape, answering differently — and `GetIPSet`'s comment justifies the empty map with "`Tags` is a `TagMap`" while `GetThreatIntelSet`'s comment reads the very same shape the other way: "`TagMap` is modelled `len 1..200` ... so an untagged list reports NO map rather than an empty one that would break that constraint". `GetCustomDetectionRuleAssociation` goes further and calls `{}` "an empty one the model cannot carry".

The same crate already treats `{}` as not a value of the member on the way IN: a create carrying `tags: {}` is refused with HTTP 400 because of that minimum (tests/all/modeled_constraint_refusals.rs:236-240), so the four readers above produce on the way out exactly the value the crate refuses on the way in.

The split is visible enough that the crate's own cross-resource tag test cannot assert either form and has to normalise both (tests/all/resource_tag_removal.rs:293-307), and the two forms are pinned in opposite directions by two different suites (tests/all/threat_intel_sets.rs:274-301 vs tests/all/ip_set_lifecycle.rs:207-217). A client reading GuardDuty resources through this emulator therefore has to null-guard some reads and not others, with no rule it can derive.

Scope note for a fixer: `GetDetector` also emits `tags: {}` (src/ops/get_detector.rs:194-202), but that was introduced deliberately by the already-FIXED issue 20261022T092246Z-guardduty-getdetector-projection-drops-data-source-timestamps-database-block, which pinned the empty map from a recording. The detector reader is therefore out of scope here; what is reported is that the other eight readers of the same shape do not follow one rule.

## Evidence

MODEL — the shape every one of these members targets, `smithy_query(query="shape", shape="TagMap")`:
```
{"TagMap": {"shape_id": "com.amazonaws.guardduty#TagMap", "type": "map", "length": {"min": 1, "max": 200}, ...}}
```

SIDE A — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_ip_set.rs:181-190:
```
            // An IPSet always reports a tag map, EMPTY when nothing tagged it, so a
            // client iterates it without a null guard — the same way `GetDetector`
            // reports the detector's tags.
            tags: Some(
                ip_set
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
```

SIDE B — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_threat_intel_set.rs:195-207:
```
            // `TagMap` is modelled `len 1..200` (`smithy_query(shape="TagMap")`), so
            // an untagged list reports NO map rather than an empty one that would
            // break that constraint.
            tags: if set.tags.is_empty() {
                None
            } else {
                Some(
                    set.tags
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            },
```

SIDE B, stated even more strongly — src/ops/get_custom_detection_rule_association.rs:183-193:
```
            // `Tags` is OPTIONAL on this read and `TagMap` is `len 1..200`
            // (`smithy_query(operation="GetCustomDetectionRuleAssociation")`), so
            // an association nothing tagged reports no tag map at all rather than
            // an empty one the model cannot carry.
            tags: (!record.tags.is_empty()).then(|| {
```
and src/ops/describe_publishing_destination.rs:192-202:
```
            // `TagMap` carries `len 1..200`
            // (`smithy_query(operation="DescribePublishingDestination")`), so an
            // untagged destination reports no member rather than an empty map.
            tags: Some(...).filter(|tags| !tags.is_empty()),
```

THE SAME CRATE REFUSES `{}` ON INPUT — tests/all/modeled_constraint_refusals.rs:236-240:
```
            "CreateDetector with an empty tags map: the published minimum is 1 entry".to_string(),
            ...
            Some(json!({"enable": true, "tags": {}})),
            "tags",
```

THE SPLIT IS ALREADY PAPERED OVER IN THE CRATE'S OWN TESTS — tests/all/resource_tag_removal.rs:293-307, a helper shared by every taggable resource class:
```
/// The tags the resource's own read reports.
///
/// A read that reports no `tags` member at all reports an empty map: `TagMap` is
/// `len 1..200`, so a resource carrying nothing has no map to report.
async fn tags_of(...) -> Value {
    ...
    body.get("tags")
        .filter(|tags| !tags.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}))
}
```

COVERAGE THAT PINS BOTH DIRECTIONS (a fix has to settle the rule, and these are the tests that encode it):
- omit: tests/all/threat_intel_sets.rs:274-301 — `/// An untagged list reports NO tags member: TagMap is modelled len 1..200` ... `an_untagged_list_reports_no_tag_map`; tests/all/publishing_destination_lifecycle.rs:207-212 — `"case {case:?}: an untagged destination must report no tags, got {:?}"`.
- emit `{}`: tests/all/ip_set_lifecycle.rs:207-217 — `// \`Tags\` is always reported — empty when nothing tagged the IPSet` / `.unwrap_or_else(|| json!({}))`; tests/all/threat_entity_set_lifecycle.rs:233-237; tests/all/trusted_entity_set_lifecycle.rs:213-217; tests/all/malware_protection_plan.rs:235-237 — `case.tags.clone().unwrap_or_else(|| json!({}))`.

## Steps to Reproduce

1. Boot the emulator and create a detector: POST /detector with {"enable": true}.
2. Create an IPSet carrying no tags: POST /detector/{detectorId}/ipset with {"name":"untagged","format":"TXT","location":"s3://b/k","activate":false}.
3. Create a ThreatIntelSet carrying no tags: POST /detector/{detectorId}/threatintelset with {"name":"untagged","format":"TXT","location":"s3://b/k","activate":false}.
4. GET /detector/{detectorId}/ipset/{ipSetId} — the body carries "tags": {} (src/ops/get_ip_set.rs:184).
5. GET /detector/{detectorId}/threatintelset/{threatIntelSetId} — the body carries no "tags" member at all (src/ops/get_threat_intel_set.rs:198).
6. Both members target com.amazonaws.guardduty#TagMap, whose modelled length minimum is 1, and the same crate refuses tags: {} on the create path with HTTP 400 — so one of the two readers is producing a value the crate itself classifies as not a value of the member.
7. Repeat with GetTrustedEntitySet / GetThreatEntitySet / GetMalwareProtectionPlan (empty map) against GetFilter / DescribePublishingDestination / GetCustomDetectionRuleAssociation (member absent) to see the same split across the rest of the service.
