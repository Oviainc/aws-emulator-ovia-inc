Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3

# targetRegions names are copied verbatim out of the manifest's governedRegions, so GetEnabledControl serves a Region$name its own modelled RegionName cannot carry

_Filed: 2026-10-29T02:06:24Z_
_Operation: GetEnabledControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An enabled control's `targetRegions` are derived on every read from the deployed landing zone's manifest `governedRegions` list (`state::governed_regions` -> `target_regions_of`), and each string found there is handed straight to the response as `Region { name }`. Nothing on the write path constrains what one of those strings is: the manifest validator only checks that the `governedRegions` value is an array whose items are JSON strings, so an entry that is the EMPTY string, or one longer than 50 characters, is accepted by CreateLandingZone/UpdateLandingZone and then reported by GetEnabledControl as a `Region$name`.

`Region$name` targets `RegionName`, which the model bounds at length 1..50. So the emulator answers 200 with a response member outside the constraint its own model declares for it: `{"targetRegions":[{"name":""}]}` is below the minimum, and a 60-character entry is above the maximum. A client generated from this model (or any strict validator in front of it) cannot accept that body.

The crate already constrains the FORM of a manifest string wherever it matters: `ManifestMemberKind::AccountId` checks `account_id.len() == 12 && all ascii_digit`, precisely because an account id has a form. The region names are the manifest member whose values this service reports back through a length-bounded member, and they are the one member kind checked for JSON type only.

## Evidence

Smithy model (`smithy_query query="shape" shape="com.amazonaws.controltower#Region"`):
  `"name": {"target": "RegionName", "length": {"min": 1, "max": 50}, "doc": "<p>The Amazon Web Services Region name.</p>"}`
and the structure doc: "An Amazon Web Services Region in which Amazon Web Services Control Tower expects to find the control deployed." / "The expected Regions are based on the Regions that are governed by the landing zone."

The manifest member is type-checked only — services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:329-331:
```
    /// A JSON array of strings — AWS Region names. May be empty: a manifest that governs
    /// no region leaves the governed-region list empty rather than omitting it.
    StringList,
```
state.rs:347-352:
```
            Self::StringList => match value {
                Document::Array(items) => {
                    items.iter().all(|item| matches!(item, Document::String(_)))
                }
                _ => false,
            },
```
The sibling kind that DOES constrain a manifest string's form — state.rs:353-358:
```
            Self::AccountId => match value {
                Document::String(account_id) => {
                    account_id.len() == 12 && account_id.bytes().all(|b| b.is_ascii_digit())
                }
```
(`("governedRegions", ManifestMemberKind::StringList)` is state.rs:403.)

The unchecked strings reach the response unchanged — state.rs:1520-1529:
```
        let Some(Document::Array(regions)) = members.get("governedRegions") else {
            return Vec::new();
        };
        regions
            .iter()
            .filter_map(|region| match region {
                Document::String(name) => Some(name.clone()),
```
state.rs:1548-1554 (`target_regions_of` returns that set as-is), and src/ops/get_enabled_control.rs:190 + 213-218:
```
        let target_regions = store.target_regions_of(&control);
...
                target_regions: Some(
                    target_regions
                        .into_iter()
                        .map(|name| Region { name: Some(name) })
                        .collect(),
                ),
```

Test coverage: no test pins an out-of-window region name. tests/all/enabled_control_target_regions.rs drives only well-formed names (`"us-west-2"`, `"eu-west-1"`, `"ap-south-1"`, …, governance_cases() lines 44-86) and tests/all/landing_zone_validation.rs:320-331 only pins `governedRegions` holding a non-string entry and `governedRegions` as an object — nothing about the content of an entry that IS a string.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3", "manifest":{ …well-formed…, "governedRegions":["", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"] }} — answers 200 and persists the landing zone (the manifest validator only checks the items are strings).
2. Wait past the settle span, then POST /enable-control with {"controlIdentifier":"arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} and note the returned arn.
3. POST /get-enabled-control with that arn.
4. Observe targetRegions = [{"name":""},{"name":"aaaa…(60 chars)"}] — a 0-character and a 60-character value for a member the model bounds at RegionName length 1..50, so the response violates the service's own model.
